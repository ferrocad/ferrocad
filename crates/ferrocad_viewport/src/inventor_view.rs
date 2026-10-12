//! Presenting an [`InventorView`] inside a gpui window (the image path).
//!
//! `bite-gpui` exposes no wgpu shared-surface primitive yet — its `PaintSurface`
//! primitive is macOS/CoreVideo only, and the wgpu backend treats
//! `PrimitiveBatch::Surfaces` as a no-op — so this presents the rendered frame the
//! MVP way: draw, read the pixels back with [`InventorView::image`], wrap them as a
//! gpui [`RenderImage`], and paint them with [`img`].
//!
//! The viewport also owns the interaction loop, mirroring FreeCAD's
//! `View3DInventorViewer`: pointer events arrive in **logical** coordinates, are
//! converted to device pixels through the camera's [`Viewport`], picked against the
//! compiled scene, and delivered to an [`EventListener`] (the `SoEventCallback`
//! analogue) which may edit the scene — for example, to highlight a hovered object.

use std::sync::Arc;
use std::time::Duration;

use ferrocad_inventor::{
    Camera, Event, EventListener, Hit, ImageData, InventorView, PickId, RenderQueue, Scene,
    ViewSpec, Viewport, compile_with, pick_at_hit,
};
use ferrocad_stage::Stage;
use ferrocad_stage::{
    MouseButton as StageButton, PointerEvent, PointerKind, Tool, snap_to_geometry,
};
use gpui::{
    AnyElement, AppContext, Context, DragMoveEvent, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Render, RenderImage, Window, div, img,
    prelude::*,
};
use image::{Frame, ImageBuffer, Rgba};

use crate::navicube::{NaviCube, NaviTarget, SnapAnimation};

/// A gpui view that presents an [`InventorView`] (image path) and routes pointer
/// events into the scene.
pub struct InventorViewport<V: InventorView> {
    view: V,
    camera: Camera,
    scene: Scene,
    /// Declarative items drawn beside the inventor scene (the modern stack). They are
    /// compiled into the same queue, so both render in one pass over one depth buffer.
    stage: Stage,
    /// Set when the stage changed, so the queue is rebuilt before the next draw.
    stage_dirty: bool,
    /// The active interactive tool (the `SoEventCallback` replacement), if any.
    tool: Option<Box<dyn Tool>>,
    /// The tool's transient visuals, rebuilt on every dispatched event. Drawn as an
    /// overlay (on top, depth off) and never picked, so a rubber band cannot shadow the
    /// geometry under the cursor.
    transient: RenderQueue,
    /// The grid spacing for grid snapping, if enabled.
    snap_grid: Option<f64>,
    queue: RenderQueue,
    listeners: Vec<Box<dyn EventListener>>,
    /// The navigation-cube overlay, when enabled.
    navicube: Option<NaviCube>,
    /// An in-progress navigation-cube orbit drag (from the first `on_drag_move`).
    navicube_drag: Option<NavicubeDrag>,
    /// An in-progress animated snap of the camera onto a cube piece.
    snap: Option<SnapAnimation>,
    /// The object currently under the pointer.
    hover: Option<PickId>,
    frame: Option<Arc<RenderImage>>,
}

/// A navigation-cube orbit drag in progress.
struct NavicubeDrag {
    /// Whether the drag began over the cube (otherwise it is ignored).
    on_cube: bool,
    /// The last logical pointer position, for the next delta.
    last: [f32; 2],
    /// Accumulated movement, so a click (no drag) can snap instead of orbiting.
    moved: f32,
}

/// The marker type gpui's drag system tracks for the navigation cube.
struct NavicubeDragMarker;

/// The (invisible) preview gpui renders while a navigation-cube drag is active.
struct NavicubeDragPreview;

impl Render for NavicubeDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl<V: InventorView> InventorViewport<V> {
    /// Wrap `view`, drawing `scene` with `camera`.
    pub fn new(view: V, camera: Camera, scene: Scene) -> Self {
        let stage = Stage::new();
        let queue = merge_queue(&scene, &stage, &camera.viewport);
        Self {
            view,
            camera,
            scene,
            stage,
            stage_dirty: false,
            tool: None,
            transient: RenderQueue::default(),
            snap_grid: None,
            queue,
            listeners: Vec::new(),
            navicube: None,
            navicube_drag: None,
            snap: None,
            hover: None,
            frame: None,
        }
    }

    /// Add a listener that reacts to pointer events (a `SoEventCallback`). Every
    /// listener sees every event, in the order added.
    pub fn with_listener(mut self, listener: Box<dyn EventListener>) -> Self {
        self.listeners.push(listener);
        self
    }

    /// Show a navigation cube in the viewport corner.
    pub fn with_navicube(mut self, navicube: NaviCube) -> Self {
        self.navicube = Some(navicube);
        self
    }

    /// Activate an interactive tool: pointer events are dispatched to it and its
    /// transient visuals are drawn over the scene (see [`Tool`]).
    pub fn with_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tool = Some(tool);
        self
    }

    /// Replace (or clear) the active tool.
    pub fn set_tool(&mut self, tool: Option<Box<dyn Tool>>) {
        self.tool = tool;
        self.rebuild_transient();
    }

    /// The active tool, if any.
    pub fn tool(&self) -> Option<&dyn Tool> {
        self.tool.as_deref()
    }

    /// Set the grid snapping spacing (device-independent world units); `None` disables
    /// grid snapping (geometry snapping still applies over a hit).
    pub fn set_snap_grid(&mut self, grid: Option<f64>) {
        self.snap_grid = grid;
    }

    /// The wrapped view (e.g. to borrow its shared surface once that path exists).
    pub fn view(&self) -> &V {
        &self.view
    }

    /// The scene currently being drawn.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The declarative stage drawn beside the inventor scene.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The stage, for adding or removing items. The queue is rebuilt before the next
    /// draw (call this from an event handler that then notifies, e.g. `cx.notify()`).
    pub fn stage_mut(&mut self) -> &mut Stage {
        self.stage_dirty = true;
        &mut self.stage
    }

    /// Replace the stage wholesale; the queue is rebuilt before the next draw.
    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_dirty = true;
    }

    /// Replace the camera and recompile (the device-pixel ratio lives in the camera).
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.recompile();
    }

    /// The object currently under the pointer, if any.
    pub fn hovered(&self) -> Option<PickId> {
        self.hover
    }

    /// Deliver a pointer move, in **logical** window coordinates (top-left origin).
    ///
    /// Over the navigation cube this hovers its faces; otherwise it picks the main
    /// scene. Returns whether the scene changed (and so needs repainting).
    pub fn handle_pointer(&mut self, logical: [f32; 2]) -> bool {
        let device = self.camera.viewport.to_device(logical);
        let mut changed = false;
        if let Some(cube) = self.navicube.as_mut() {
            if cube.rect().contains(device) {
                let piece = match cube.hit(device) {
                    Some(NaviTarget::Piece(piece)) => Some(piece),
                    _ => None,
                };
                self.hover = None;
                return cube.set_hilite(piece);
            }
            changed = cube.set_hilite(None);
        }
        changed |= self.dispatch_tool(PointerKind::Move, device, None);
        changed | self.pick_main(device)
    }

    /// Deliver a mouse-button change, in **logical** window coordinates.
    ///
    /// A press on the cube begins a possible orbit drag; a release that did not orbit
    /// snaps the main camera onto the piece under the pointer. Otherwise the event
    /// goes to the main scene.
    pub fn handle_button(&mut self, logical: [f32; 2], button: u8, down: bool) -> bool {
        // Any button event ends a (possibly stale) orbit drag or snap.
        self.navicube_drag = None;
        self.snap = None;
        let device = self.camera.viewport.to_device(logical);
        if self
            .navicube
            .as_ref()
            .is_some_and(|cube| cube.rect().contains(device))
        {
            if down {
                return false; // the orbit starts on the first drag-move
            }
            if let Some(target) = self.navicube.as_ref().and_then(|cube| cube.hit(device)) {
                // Animate the snap rather than jumping: a piece snaps onto its face,
                // edge or corner; the Home button restores the default orientation.
                let end = match target {
                    NaviTarget::Piece(piece) => piece.orientation(),
                    NaviTarget::Home => crate::navicube::home_orientation(),
                };
                self.snap = Some(SnapAnimation::to(end, &self.camera, Duration::from_millis(220)));
                return true;
            }
            return false;
        }
        let kind = if down {
            PointerKind::Down
        } else {
            PointerKind::Up
        };
        let mut changed = self.dispatch_tool(kind, device, stage_button(button));
        let hit = pick_at_hit(&self.queue, &self.camera, device);
        let event = Event::MouseButton {
            position: device,
            button,
            down,
        };
        changed |= self.dispatch(&event, hit.as_ref());
        changed
    }

    /// Deliver a pointer event to the active tool (if any) and rebuild its transient
    /// visuals. The pick is taken from the scene-and-stage queue, so a tool never snaps
    /// to its own transient items. Returns whether a tool is active.
    fn dispatch_tool(
        &mut self,
        kind: PointerKind,
        device: [i32; 2],
        button: Option<StageButton>,
    ) -> bool {
        if self.tool.is_none() {
            return false;
        }
        let hit = pick_at_hit(&self.queue, &self.camera, device);
        let event = PointerEvent {
            kind,
            position: device,
            button,
            hit,
        };
        let snap = snap_to_geometry(&event, &self.camera, self.snap_grid);
        if let Some(tool) = self.tool.as_mut() {
            tool.on_event(&event, snap);
        }
        self.rebuild_transient();
        true
    }

    /// Rebuild the transient queue from the active tool's items.
    fn rebuild_transient(&mut self) {
        let mut queue = RenderQueue::default();
        if let Some(tool) = self.tool.as_ref() {
            for item in tool.items() {
                item.emit(&mut queue);
            }
        }
        self.transient = queue;
    }

    /// Handle a gpui drag-move: orbit the main camera when the drag began over the
    /// navigation cube. Reports whether the view changed.
    fn navicube_drag_move(&mut self, logical: [f32; 2]) -> bool {
        self.snap = None; // a drag cancels any in-progress snap
        let device = self.camera.viewport.to_device(logical);
        let on_cube = self
            .navicube
            .as_ref()
            .is_some_and(|cube| cube.rect().contains(device));
        let state = self.navicube_drag.get_or_insert(NavicubeDrag {
            on_cube,
            last: logical,
            moved: 0.0,
        });
        if !state.on_cube {
            return false;
        }
        let delta = [logical[0] - state.last[0], logical[1] - state.last[1]];
        state.last = logical;
        state.moved += (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
        self.camera.view = crate::navicube::drag_view(&self.camera, delta);
        self.recompile();
        true
    }

    /// Deliver a pointer leave.
    pub fn handle_pointer_leave(&mut self) -> bool {
        self.hover = None;
        self.dispatch(&Event::MouseLeave, None)
    }

    /// Pick the main scene at a device position and run the listeners.
    fn pick_main(&mut self, device: [i32; 2]) -> bool {
        let hit = pick_at_hit(&self.queue, &self.camera, device);
        self.hover = hit.as_ref().map(|hit| hit.id);
        let event = Event::MouseMove { position: device };
        self.dispatch(&event, hit.as_ref())
    }

    /// Run the listeners on an event and recompile if any changed the scene.
    ///
    /// Every listener sees every event (an observation-only listener, like the
    /// Python `SoEventCallback` bridge, must not stop a later one from running).
    fn dispatch(&mut self, event: &Event, hit: Option<&Hit>) -> bool {
        let mut changed = false;
        for listener in &mut self.listeners {
            if listener.on_event(event, hit, &mut self.scene) {
                changed = true;
            }
        }
        if changed {
            self.recompile();
        }
        changed
    }

    fn recompile(&mut self) {
        self.queue = merge_queue(&self.scene, &self.stage, &self.camera.viewport);
        self.stage_dirty = false;
    }

    /// Step a running snap animation, applying the interpolated camera. Returns
    /// whether another frame is needed.
    fn step_snap(&mut self) -> bool {
        let Some(snap) = self.snap.as_ref() else {
            return false;
        };
        let view = snap.view_at(snap.elapsed());
        let done = snap.is_done();
        self.camera.view = view;
        self.recompile();
        if done {
            self.snap = None;
            false
        } else {
            true
        }
    }

    /// Draw the scene (and the navigation cube, when enabled) and read it back,
    /// refreshing the cached frame.
    fn refresh(&mut self) {
        if self.stage_dirty {
            self.recompile();
        }
        if let Some(cube) = self.navicube.as_mut() {
            cube.sync(&self.camera);
        }

        // One pass: the scene (legacy + stage) first, then the tool's transient overlay
        // (depth off, on top), then the navigation cube.
        let mut views = Vec::with_capacity(3);
        views.push(ViewSpec {
            queue: &self.queue,
            camera: &self.camera,
            rect: None,
            overlay: false,
        });
        if !self.transient.is_empty() {
            views.push(ViewSpec {
                queue: &self.transient,
                camera: &self.camera,
                rect: None,
                overlay: true,
            });
        }
        if let Some(cube) = self.navicube.as_ref() {
            views.push(cube.view());
        }

        if self.view.draw_frame(&views).is_ok() {
            if let Ok(image) = self.view.image() {
                self.frame = Some(Arc::new(render_image(image)));
            }
        }
    }
}

impl<V: InventorView + 'static> Render for InventorViewport<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // MVP: render and read back during paint, on the UI thread. A snap animation
        // keeps requesting frames until it settles.
        if self.step_snap() {
            window.request_animation_frame();
        }
        self.refresh();

        let content: AnyElement = match &self.frame {
            Some(image) => img(image.clone())
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Fill)
                .into_any_element(),
            None => div().w_full().h_full().into_any_element(),
        };

        div()
            .w_full()
            .h_full()
            .id("ferrocad-inventor-viewport")
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.handle_pointer([event.position.x.0, event.position.y.0]) {
                    cx.notify();
                }
            }))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                if this.handle_button(
                    [event.position.x.0, event.position.y.0],
                    mouse_button(event.button),
                    true,
                ) {
                    cx.notify();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                if this.handle_button(
                    [event.position.x.0, event.position.y.0],
                    mouse_button(event.button),
                    false,
                ) {
                    cx.notify();
                }
            }))
            .on_drag(NavicubeDragMarker, |_, _, _, cx| {
                cx.new(|_| NavicubeDragPreview)
            })
            .on_drag_move::<NavicubeDragMarker>(cx.listener(
                |this, event: &DragMoveEvent<NavicubeDragMarker>, _window, cx| {
                    let position = [event.event.position.x.0, event.event.position.y.0];
                    if this.navicube_drag_move(position) {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_exit(cx.listener(|this, _event: &MouseExitEvent, _window, cx| {
                if this.handle_pointer_leave() {
                    cx.notify();
                }
            }))
            .child(content)
    }
}

/// Compile the inventor scene and append the stage's batches, so the two producers
/// share one queue (and thus one render pass and one depth buffer).
fn merge_queue(scene: &Scene, stage: &Stage, viewport: &Viewport) -> RenderQueue {
    let mut queue = compile_with(scene, viewport);
    stage.emit_batches(&mut queue);
    queue
}

/// Map a gpui mouse button to Coin's 1-based button number.
fn mouse_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        _ => 0,
    }
}

/// Map Coin's 1-based button number to a stage mouse button.
fn stage_button(button: u8) -> Option<StageButton> {
    match button {
        1 => Some(StageButton::Left),
        2 => Some(StageButton::Middle),
        3 => Some(StageButton::Right),
        _ => None,
    }
}

/// Wrap readback pixels as a gpui [`RenderImage`].
///
/// gpui's `RenderImage` holds **BGRA** (see its docs), while [`ImageData`] is RGBA,
/// so the two colour channels are swapped here. The image is not flipped: the wgpu
/// readback is already top-left origin, matching gpui.
pub fn render_image(image: ImageData) -> RenderImage {
    let mut bgra = image.rgba;
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2); // R <-> B
    }
    let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(image.width, image.height, bgra)
            .expect("ImageData length must equal width * height * 4");
    RenderImage::new([Frame::new(buffer)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::{Node, NodeKind, Topology, compile_with};

    /// A view that returns fixed pixels, so the gpui side needs no GPU.
    struct FakeView {
        pixels: ImageData,
    }

    impl InventorView for FakeView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [self.pixels.width, self.pixels.height]
        }
        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Ok(self.pixels.clone())
        }
    }

    fn fake(pixels: ImageData) -> FakeView {
        FakeView { pixels }
    }

    #[test]
    fn render_image_keeps_size_and_swizzles_to_bgra() {
        let rendered = render_image(ImageData {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 255],
        });
        assert_eq!(rendered.size(0).width.0, 1);
        assert_eq!(rendered.as_bytes(0).unwrap(), &[30, 20, 10, 255]);
    }

    /// A triangle at the origin of an identity camera (device == logical at 1x).
    fn triangle_scene() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    Node {
                        kind: NodeKind::Material(vec![[1.0, 1.0, 1.0, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(vec![
                            [-0.5, -0.5, 0.5],
                            [0.5, -0.5, 0.5],
                            [0.0, 0.5, 0.5],
                        ]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Mesh {
                            indices: vec![0, 1, 2],
                            normal_indices: vec![],
                            topology: Topology::Triangles,
                        },
                        children: vec![],
                    },
                ],
            },
        }
    }

    /// A listener that records whether the pointer was over an object.
    #[derive(Default)]
    struct HoverProbe {
        hovered: bool,
    }

    impl EventListener for HoverProbe {
        fn on_event(&mut self, _event: &Event, hit: Option<&Hit>, _scene: &mut Scene) -> bool {
            let next = hit.is_some();
            let changed = next != self.hovered;
            self.hovered = next;
            changed
        }
    }

    #[test]
    fn pointer_events_drive_picking_and_the_listener() {
        let camera = Camera {
            viewport: ferrocad_inventor::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_listener(Box::new(HoverProbe::default()));

        // Centre hits the triangle; a corner misses.
        assert!(viewport.handle_pointer([50.0, 50.0]));
        assert_eq!(viewport.hovered(), Some(PickId(0)));
        assert!(viewport.handle_pointer([5.0, 5.0]));
        assert_eq!(viewport.hovered(), None);
        // Leaving is a no-op once nothing is hovered.
        assert!(!viewport.handle_pointer_leave());
    }

    #[test]
    fn viewport_compiles_the_scene_it_is_given() {
        let camera = Camera {
            viewport: ferrocad_inventor::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let scene = triangle_scene();
        let expected = compile_with(&scene, &camera.viewport).len();
        let viewport = InventorViewport::new(fake(ImageData::default()), camera, scene);
        assert_eq!(viewport.queue.len(), expected);
        assert_eq!(viewport.scene().root.children.len(), 3);
    }

    #[test]
    fn a_stage_item_is_merged_into_the_viewport_queue() {
        use ferrocad_stage::{DrawMode, ShapeItem, Stage};

        let camera = Camera {
            viewport: ferrocad_inventor::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        let base = viewport.queue.len();
        assert!(base >= 1, "the inventor scene compiles to at least one packet");

        // An item added through the stage accessor is merged on the next draw (refresh
        // rebuilds the queue, then renders it).
        viewport.stage_mut().add_item(
            ShapeItem::new(5)
                .with_draw_mode(DrawMode::Shaded)
                .with_shape(
                    vec![[0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [0.0, 1.0, 0.5]],
                    vec![],
                    vec![0, 1, 2],
                    vec![],
                ),
        );
        viewport.refresh();
        assert_eq!(viewport.queue.len(), base + 1);
        assert!(viewport.queue.packets.iter().any(|p| p.state.id == PickId(5)));

        // Replacing the stage drops the item again.
        viewport.set_stage(Stage::new());
        viewport.refresh();
        assert_eq!(viewport.queue.len(), base);
    }

    #[test]
    fn a_tool_receives_pointer_events_and_its_items_are_drawn() {
        use ferrocad_inventor::Topology;
        use ferrocad_stage::PolylineTool;

        let camera = Camera {
            viewport: ferrocad_inventor::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        )
        .with_tool(Box::new(PolylineTool::new()));

        // Nothing transient is drawn until the tool sees an event.
        assert!(viewport.transient.is_empty());

        // A left press over the scene is dispatched to the tool, which emits a band and a
        // marker (wireframe) for the overlay.
        assert!(viewport.handle_button([50.0, 50.0], 1, true));
        assert!(
            !viewport.transient.is_empty(),
            "the tool should emit transient visuals"
        );
        assert!(
            viewport
                .transient
                .packets
                .iter()
                .all(|packet| packet.topology == Topology::Lines),
            "transient visuals are wireframe"
        );

        // The draw path (scene + transient overlay) runs cleanly on the fake view.
        viewport.refresh();
    }
}
