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

use ferrocad_graphics::{Camera, Hit, PickId, RenderQueue, Viewport, pick_at_hit};
use ferrocad_inventor::{
    Event, EventListener, ImageData, InventorView, Scene, ViewSpec, compile_with,
};
use ferrocad_stage::Stage;
use ferrocad_stage::{
    GizmoCommitEvent, GizmoHandle, MouseButton as StageButton, PointerEvent, PointerKind, Tool,
    TransformGizmo, snap_to_geometry,
};
use ferrocad_types::Matrix4;
use gpui::{
    AnyElement, AppContext, Bounds, Context, DragMoveEvent, IntoElement, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage,
    Window, div, img, prelude::*,
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
    /// The active transform gizmo, if any. Drawn on top and picked **before** the tool
    /// and the scene, so a drag on a handle takes priority.
    gizmo: Option<TransformGizmo>,
    /// The gizmo's handle batches (rebuilt when its scale, origin, mode or hover change).
    gizmo_queue: RenderQueue,
    /// The last gizmo commit, for the host to apply to the document.
    gizmo_commit: Option<GizmoCommitEvent>,
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
    /// The element's window-space bounds, captured at prepaint. gpui reports pointer
    /// positions relative to the **window**, so this is subtracted to make them
    /// viewport-local (see [`to_local`](Self::to_local)).
    last_bounds: Option<Bounds<Pixels>>,
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
            gizmo: None,
            gizmo_queue: RenderQueue::default(),
            gizmo_commit: None,
            queue,
            listeners: Vec::new(),
            navicube: None,
            navicube_drag: None,
            snap: None,
            hover: None,
            frame: None,
            last_bounds: None,
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

    /// Show a transform gizmo (drawn on top, picked before the tool and the scene).
    pub fn with_gizmo(mut self, gizmo: TransformGizmo) -> Self {
        self.gizmo = Some(gizmo);
        self
    }

    /// Replace (or clear) the transform gizmo.
    pub fn set_gizmo(&mut self, gizmo: Option<TransformGizmo>) {
        self.gizmo = gizmo;
        self.gizmo_commit = None;
        self.rebuild_gizmo_queue();
    }

    /// The active gizmo, if any.
    pub fn gizmo(&self) -> Option<&TransformGizmo> {
        self.gizmo.as_ref()
    }

    /// The active gizmo, mutably (e.g. to move its origin).
    pub fn gizmo_mut(&mut self) -> Option<&mut TransformGizmo> {
        self.gizmo.as_mut()
    }

    /// The last gizmo commit (a drag's final transform), if a drag has ended.
    pub fn last_gizmo_commit(&self) -> Option<&GizmoCommitEvent> {
        self.gizmo_commit.as_ref()
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

    /// Convert a **window-relative** logical point (what gpui reports to an element's
    /// mouse listeners) into a **viewport-local** logical point, by subtracting the
    /// element's window-space origin captured at prepaint.
    ///
    /// Everything below the host — [`handle_pointer`](Self::handle_pointer), picking and
    /// the camera's [`Viewport`] — works in viewport-local coordinates, so this runs at
    /// the gpui boundary. Before the first prepaint (no bounds yet) the point is passed
    /// through unchanged.
    pub(crate) fn to_local(&self, position: Point<Pixels>) -> [f32; 2] {
        match self.last_bounds {
            Some(bounds) => [
                (position.x - bounds.origin.x).0,
                (position.y - bounds.origin.y).0,
            ],
            None => [position.x.0, position.y.0],
        }
    }

    /// Keep the camera's [`Viewport`] (and the backend's render target) at the element's
    /// **physical** size, from the window-space bounds captured at the last prepaint.
    ///
    /// gpui reports an element's bounds in **logical** window pixels; multiplied by the
    /// window scale factor that is the device size the renderer, picking and the overlays
    /// work in. Returns whether the size changed. A no-op until the first prepaint.
    fn sync_size(&mut self, window: &Window) -> bool {
        match self.last_bounds {
            Some(bounds) => self.apply_element_size(
                [bounds.size.width.0, bounds.size.height.0],
                window.scale_factor(),
            ),
            None => false,
        }
    }

    /// Apply a logical element size (and display scale) to the camera viewport, the
    /// render target and the navigation-cube overlay, recompiling if the device-pixel
    /// ratio changed (it seeds screen-space nodes). Returns whether anything changed.
    fn apply_element_size(&mut self, logical: [f32; 2], scale: f32) -> bool {
        let scale = scale.max(1.0);
        let device = [
            (logical[0] * scale).round().max(1.0) as u32,
            (logical[1] * scale).round().max(1.0) as u32,
        ];
        let viewport = Viewport::new(device, scale);
        if viewport == self.camera.viewport {
            return false;
        }
        self.camera.viewport = viewport;
        self.view.resize(device);
        if let Some(cube) = self.navicube.as_mut() {
            cube.set_viewport(viewport);
        }
        self.recompile();
        true
    }

    /// Deliver a pointer move, in **viewport-local logical** pixels (top-left origin).
    ///
    /// Over the navigation cube this hovers its faces; over the gizmo it drags or
    /// hovers a handle; over the scene it picks the main scene. Returns whether the
    /// scene changed (and so needs repainting).
    pub fn handle_pointer(&mut self, logical: [f32; 2]) -> bool {
        let device = self.camera.viewport.to_device(logical);
        let mut changed = false;
        if let Some(cube) = self.navicube.as_mut() {
            if cube.rect().contains(device) {
                // Hover highlights pieces only (not the Home button), as before.
                let target = cube
                    .hit(device)
                    .filter(|target| matches!(target, NaviTarget::Piece(_)));
                self.hover = None;
                return cube.set_hilite(target);
            }
            changed = cube.set_hilite(None);
        }

        // The gizmo takes priority: an active drag consumes the move; otherwise hover.
        if self.gizmo.as_ref().is_some_and(|gizmo| gizmo.is_dragging()) {
            let event = match self.gizmo.as_mut() {
                Some(gizmo) => gizmo.drag_to(&self.camera, device),
                None => None,
            };
            if let Some(event) = event {
                self.apply_gizmo_preview(Some(event.total_transform));
                return true;
            }
        } else if self.gizmo.is_some() {
            let handle = self.gizmo_handle_at(device);
            let recoloured = match self.gizmo.as_mut() {
                Some(gizmo) => gizmo.set_hovered(handle),
                None => false,
            };
            if recoloured {
                self.rebuild_gizmo_queue();
                return true;
            }
        }

        changed |= self.dispatch_tool(PointerKind::Move, device, None);
        changed | self.pick_main(device)
    }

    /// Deliver a mouse-button change, in **viewport-local logical** pixels.
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
        // The gizmo takes priority over the tool and the scene.
        if self.gizmo.is_some() {
            if down {
                if let Some(handle) = self.gizmo_handle_at(device) {
                    let started = match self.gizmo.as_mut() {
                        Some(gizmo) => gizmo.begin_drag(handle, &self.camera, device),
                        None => false,
                    };
                    if started {
                        self.rebuild_gizmo_queue();
                        return true;
                    }
                }
            } else if self.gizmo.as_ref().is_some_and(|gizmo| gizmo.is_dragging()) {
                let commit = match self.gizmo.as_mut() {
                    Some(gizmo) => gizmo.end_drag(),
                    None => None,
                };
                if let Some(commit) = commit {
                    self.gizmo_commit = Some(commit);
                    // The host applies the committed transform to the document; clear the
                    // preview it was showing.
                    self.apply_gizmo_preview(None);
                    self.rebuild_gizmo_queue();
                    return true;
                }
            }
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

    /// The gizmo handle under a device position, if any.
    fn gizmo_handle_at(&self, device: [i32; 2]) -> Option<GizmoHandle> {
        let hit = pick_at_hit(&self.gizmo_queue, &self.camera, device)?;
        GizmoHandle::from_pick_id(hit.id.0)
    }

    /// Rebuild the gizmo's handle batches from the active gizmo.
    fn rebuild_gizmo_queue(&mut self) {
        let mut queue = RenderQueue::default();
        if let Some(gizmo) = self.gizmo.as_ref() {
            for item in gizmo.items() {
                item.emit(&mut queue);
            }
        }
        self.gizmo_queue = queue;
    }

    /// Preview `transform` on the gizmo's target stage item (or clear the preview).
    fn apply_gizmo_preview(&mut self, transform: Option<Matrix4>) {
        let Some(target) = self.gizmo.as_ref().map(TransformGizmo::target_id) else {
            return;
        };
        let mut touched = false;
        for item in self.stage.items_mut() {
            if item.entity_id() == target {
                item.set_temporary_transform(transform);
                touched = true;
            }
        }
        if touched {
            self.stage_dirty = true;
        }
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
    ///
    /// The backend's hardware G-buffer pick is preferred (it resolves the exact
    /// rasterised pixel); a backend without one falls back to the CPU ray cast.
    fn pick_main(&mut self, device: [i32; 2]) -> bool {
        let hit = if self.view.supports_pick() {
            self.view.pick_at(&self.queue, &self.camera, device)
        } else {
            pick_at_hit(&self.queue, &self.camera, device)
        };
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

        // The gizmo's screen-invariant scale follows the camera; rebuild when it changes.
        let gizmo_rebuilt = match self.gizmo.as_mut() {
            Some(gizmo) => gizmo.update(&self.camera),
            None => false,
        };
        if gizmo_rebuilt {
            self.rebuild_gizmo_queue();
        }

        // One pass: the scene (legacy + stage) first, then the overlays (the tool's
        // transient visuals and the gizmo, depth off and on top), then the navigation cube.
        let mut views = Vec::with_capacity(4);
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
        if !self.gizmo_queue.is_empty() {
            views.push(ViewSpec {
                queue: &self.gizmo_queue,
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
        // Follow the element's physical size before drawing (the bounds were captured at
        // the previous prepaint). A change needs one more frame to take effect.
        if self.sync_size(window) {
            window.request_animation_frame();
        }
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

        let entity = cx.entity();
        div()
            .w_full()
            .h_full()
            // Record the element's window-space bounds each frame, so the window-relative
            // positions gpui reports can be made viewport-local (see `to_local`).
            .on_children_prepainted(move |bounds: Vec<Bounds<Pixels>>, _window, cx| {
                if let Some(bounds) = bounds.first().copied() {
                    entity.update(cx, |this, _cx| this.last_bounds = Some(bounds));
                }
            })
            .id("ferrocad-inventor-viewport")
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let local = this.to_local(event.position);
                if this.handle_pointer(local) {
                    cx.notify();
                }
            }))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                let local = this.to_local(event.position);
                if this.handle_button(local, mouse_button(event.button), true) {
                    cx.notify();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                let local = this.to_local(event.position);
                if this.handle_button(local, mouse_button(event.button), false) {
                    cx.notify();
                }
            }))
            .on_drag(NavicubeDragMarker, |_, _, _, cx| {
                cx.new(|_| NavicubeDragPreview)
            })
            .on_drag_move::<NavicubeDragMarker>(cx.listener(
                |this, event: &DragMoveEvent<NavicubeDragMarker>, _window, cx| {
                    let position = this.to_local(event.event.position);
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
    use ferrocad_graphics::{Projection, Topology};
    use ferrocad_inventor::{Node, NodeKind, compile_with};
    use gpui::{Bounds, point, px, size};

    /// A view that returns fixed pixels, so the gpui side needs no GPU.
    struct FakeView {
        pixels: ImageData,
        resize_calls: u32,
        last_resize: Option<[u32; 2]>,
    }

    impl InventorView for FakeView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [self.pixels.width, self.pixels.height]
        }
        fn resize(&mut self, size: [u32; 2]) {
            self.resize_calls += 1;
            self.last_resize = Some(size);
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
        FakeView {
            pixels,
            resize_calls: 0,
            last_resize: None,
        }
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
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
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
    fn to_local_subtracts_the_element_window_origin() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene());

        // gpui reports window-relative positions; with the element at window (80, 40) a
        // report of (100, 60) is element-local (20, 20).
        viewport.last_bounds = Some(Bounds::new(
            point(px(80.0), px(40.0)),
            size(px(640.0), px(480.0)),
        ));
        assert_eq!(viewport.to_local(point(px(100.0), px(60.0))), [20.0, 20.0]);

        // Before the first prepaint there is no origin; pass the point through.
        viewport.last_bounds = None;
        assert_eq!(viewport.to_local(point(px(100.0), px(60.0))), [100.0, 60.0]);
    }

    #[test]
    fn the_element_size_drives_the_camera_viewport_and_the_backend() {
        let camera = Camera {
            viewport: Viewport::new([640, 480], 1.0),
            ..Camera::identity()
        };
        let mut viewport =
            InventorViewport::new(fake(ImageData::default()), camera, triangle_scene())
                .with_navicube(NaviCube::new(Viewport::new([640, 480], 1.0)));
        let cube_before = viewport.navicube.as_ref().unwrap().rect();

        // An 800x600 logical element at 2x is 1600x1200 device pixels.
        assert!(viewport.apply_element_size([800.0, 600.0], 2.0));
        assert_eq!(viewport.camera.viewport.device_size(), [1600, 1200]);
        assert_eq!(viewport.camera.viewport.device_pixel_ratio(), 2.0);
        assert_eq!(viewport.view().last_resize, Some([1600, 1200]));
        assert_eq!(viewport.view().resize_calls, 1);
        assert_ne!(
            viewport.navicube.as_ref().unwrap().rect(),
            cube_before,
            "the navigation cube follows the resize"
        );

        // A redundant report does not touch the backend again.
        assert!(!viewport.apply_element_size([800.0, 600.0], 2.0));
        assert_eq!(viewport.view().resize_calls, 1, "an unchanged size is a no-op");
    }

    /// A view with a hardware pick path: it reports id `42` wherever there is a queue to
    /// pick, ignoring the scene, so a test can tell it apart from the CPU ray cast (which
    /// would report `PickId(0)` at the centre).
    struct HardwarePickView;

    impl InventorView for HardwarePickView {
        type Surface = ();
        type Error = std::io::Error;

        fn size(&self) -> [u32; 2] {
            [100, 100]
        }
        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Ok(())
        }
        fn borrow_surface(&self) -> Option<&()> {
            None
        }
        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Ok(ImageData::default())
        }
        fn supports_pick(&self) -> bool {
            true
        }
        fn pick_at(
            &mut self,
            queue: &RenderQueue,
            _camera: &Camera,
            device: [i32; 2],
        ) -> Option<Hit> {
            (!queue.is_empty()).then_some(Hit {
                id: PickId(42),
                point: [device[0] as f32, device[1] as f32, 0.0],
                normal: [0.0, 0.0, 1.0],
                element: 0,
                topology: Topology::Triangles,
                distance: 1.0,
            })
        }
    }

    #[test]
    fn the_viewport_prefers_a_hardware_pick_when_the_backend_has_one() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        let mut viewport = InventorViewport::new(HardwarePickView, camera, triangle_scene())
            .with_listener(Box::new(HoverProbe::default()));

        // The CPU path would report `PickId(0)` over the triangle; the backend's hardware
        // pick reports 42, so seeing 42 proves it was preferred.
        assert!(viewport.handle_pointer([50.0, 50.0]));
        assert_eq!(viewport.hovered(), Some(PickId(42)));
    }

    #[test]
    fn viewport_compiles_the_scene_it_is_given() {
        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
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
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
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
        use ferrocad_graphics::Topology;
        use ferrocad_stage::PolylineTool;

        let camera = Camera {
            viewport: ferrocad_graphics::Viewport::new([100, 100], 1.0),
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

    /// A 45° perspective camera at `(0, 0, 5)` looking toward the origin.
    fn perspective_camera(size: [u32; 2]) -> Camera {
        use ferrocad_types::{Placement, Rotation, Vector3};
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -5.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: ferrocad_graphics::Viewport::new(size, 1.0),
        }
    }

    #[test]
    fn a_gizmo_drag_previews_a_transform_on_the_target_item() {
        use ferrocad_graphics::pick_at_hit;
        use ferrocad_stage::{GizmoHandle, GizmoMode, ShapeItem, Stage, TransformGizmo};
        use ferrocad_types::Vector3;

        let size = [100u32, 100u32];
        let camera = perspective_camera(size);
        let viewport = camera.viewport;

        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(42).with_shape(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![],
            vec![0, 1, 2],
            vec![],
        ));

        let mut view = InventorViewport::new(
            fake(ImageData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            }),
            camera,
            triangle_scene(),
        );
        view.set_stage(stage);
        view.set_gizmo(Some(TransformGizmo::new(
            42,
            Vector3::zero(),
            GizmoMode::Translate,
        )));
        view.refresh();

        // Find a device position that hits the X axis handle.
        let mut grab = None;
        for x in 0..100i32 {
            for y in 0..100i32 {
                if let Some(hit) = pick_at_hit(&view.gizmo_queue, &view.camera, [x, y]) {
                    if GizmoHandle::from_pick_id(hit.id.0) == Some(GizmoHandle::AxisX) {
                        grab = Some([x, y]);
                    }
                }
            }
        }
        let grab = grab.expect("the X axis handle should be pickable");

        let preview = |view: &InventorViewport<FakeView>| {
            view.stage()
                .items()
                .iter()
                .find(|item| item.entity_id() == 42)
                .and_then(|item| item.temporary_transform())
        };

        // Press on the handle, then drag right: the target shows a live preview.
        assert!(view.handle_button(viewport.to_logical(grab), 1, true));
        assert!(view.gizmo().expect("gizmo").is_dragging());
        assert!(view.handle_pointer(viewport.to_logical([grab[0] + 12, grab[1]])));
        assert!(preview(&view).is_some(), "the drag previews a transform");
        assert!(view.last_gizmo_commit().is_none());

        // Release: a commit is recorded and the preview cleared.
        assert!(view.handle_button(viewport.to_logical([grab[0] + 12, grab[1]]), 1, false));
        assert!(view.last_gizmo_commit().is_some());
        assert!(preview(&view).is_none(), "the preview is cleared on commit");
    }
}
