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

use ferrocad_inventor::{
    Camera, Event, EventListener, Hit, ImageData, InventorView, PickId, RenderQueue, Scene,
    compile_with, pick_at_hit,
};
use gpui::{
    AnyElement, Context, IntoElement, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Render, RenderImage, Window, div, img, prelude::*,
};
use image::{Frame, ImageBuffer, Rgba};

/// A gpui view that presents an [`InventorView`] (image path) and routes pointer
/// events into the scene.
pub struct InventorViewport<V: InventorView> {
    view: V,
    camera: Camera,
    scene: Scene,
    queue: RenderQueue,
    listeners: Vec<Box<dyn EventListener>>,
    /// The object currently under the pointer.
    hover: Option<PickId>,
    frame: Option<Arc<RenderImage>>,
}

impl<V: InventorView> InventorViewport<V> {
    /// Wrap `view`, drawing `scene` with `camera`.
    pub fn new(view: V, camera: Camera, scene: Scene) -> Self {
        let queue = compile_with(&scene, &camera.viewport);
        Self {
            view,
            camera,
            scene,
            queue,
            listeners: Vec::new(),
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

    /// The wrapped view (e.g. to borrow its shared surface once that path exists).
    pub fn view(&self) -> &V {
        &self.view
    }

    /// The scene currently being drawn.
    pub fn scene(&self) -> &Scene {
        &self.scene
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
    /// Returns whether the scene changed (and so needs repainting).
    pub fn handle_pointer(&mut self, logical: [f32; 2]) -> bool {
        let device = self.camera.viewport.to_device(logical);
        let hit = pick_at_hit(&self.queue, &self.camera, device);
        self.hover = hit.as_ref().map(|hit| hit.id);
        let event = Event::MouseMove { position: device };
        self.dispatch(&event, hit.as_ref())
    }

    /// Deliver a mouse-button change, in **logical** window coordinates.
    pub fn handle_button(&mut self, logical: [f32; 2], button: u8, down: bool) -> bool {
        let device = self.camera.viewport.to_device(logical);
        let hit = pick_at_hit(&self.queue, &self.camera, device);
        let event = Event::MouseButton {
            position: device,
            button,
            down,
        };
        self.dispatch(&event, hit.as_ref())
    }

    /// Deliver a pointer leave.
    pub fn handle_pointer_leave(&mut self) -> bool {
        self.hover = None;
        self.dispatch(&Event::MouseLeave, None)
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
        self.queue = compile_with(&self.scene, &self.camera.viewport);
    }

    /// Draw the scene and read it back, refreshing the cached frame.
    fn refresh(&mut self) {
        if self.view.draw(&self.queue, &self.camera).is_ok() {
            if let Ok(image) = self.view.image() {
                self.frame = Some(Arc::new(render_image(image)));
            }
        }
    }
}

impl<V: InventorView + 'static> Render for InventorViewport<V> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // MVP: render and read back during paint, on the UI thread.
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
            .on_mouse_exit(cx.listener(|this, _event: &MouseExitEvent, _window, cx| {
                if this.handle_pointer_leave() {
                    cx.notify();
                }
            }))
            .child(content)
    }
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
}
