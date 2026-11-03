//! A GPUI window in the browser, compositing a FerroCAD `Part` document through
//! `gpui_interop`.
//!
//! Step 2's probe, third increment: the scene is no longer hand-built geometry.
//! A real `Document` with `Part::Box` and `Part::Fillet` is recomputed through the
//! OCCT kernel compiled to wasm, tessellated, turned into `ferrocad_stage` draw
//! packets, drawn by `ferrocad_wgpu` into a `gpui_interop::WgpuSurface` on the
//! window's own device, and composited by GPUI with `paint_texture`. Drag orbits
//! and the wheel zooms: the pointer half is `gpui_interop`'s `surface_input`
//! gateway, so the gesture survives leaving the canvas without any state of ours,
//! and the wheel is the view's own handler because the gateway has no scroll.
//!
//! What this still is not: no CPython, no shell. `ferrocad_gpui` embeds CPython
//! and cannot be the wasm crate until that dependency is split; this probe stops
//! at the document layer, which is the last one that does not need Python.
//!
//! There is **no text** on purpose: GPUI panics when asked to resolve a font and
//! none has been added (`DefaultTextSystem::resolve_font`), and font loading is
//! `ferrocad_text`'s job. Status goes to the browser console through the logger
//! `gpui::web_init` installs.

#![cfg(target_family = "wasm")]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use ferrocad_core::{Document, Property};
use ferrocad_geom::TessellationOptions;
use ferrocad_graphics::{Camera, DrawPacket, Projection, Viewport};
use ferrocad_occt::OcctBackend;
use ferrocad_part::{BOX_TYPE, FILLET_TYPE, shape_of};
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_wgpu::{GpuContext, WgpuRenderer};
use gpui::{
    App, Bounds, Context, GpuCanvasContext, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, gpu_canvas, prelude::*, px, rgb, size,
};
use gpui_interop::{
    SurfaceEvent, SurfaceInput, SurfacePoint, SurfaceScroll, WgpuSurface, surface_input,
};
use gpui_wgpu::{ImportedTextureExt as _, WebBackendPreference, WgpuRenderer as GpuiWgpuRenderer};
use wasm_bindgen::prelude::*;

/// The edge the fillet is applied to, as the canvas demo does.
const EDGE: &str = "Edge1";

/// The fillet radius, in millimetres. Large enough to see, well under the radius
/// at which the kernel refuses it (the demo's 10 mm box fails at 10 or more).
const RADIUS: f64 = 3.0;

/// The tessellation tolerance, in millimetres (the demo's).
const TOLERANCE: f64 = 0.2;

/// One wheel line zooms by this factor — the rate `ferrocad_web_viewport` uses.
const ZOOM_PER_LINE: f64 = 1.15;

/// A wheel event delivers lines or pixels; a mouse notch is about this many pixels.
const PIXELS_PER_LINE: f32 = 40.0;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    fn __wasm_call_ctors();
}

#[wasm_bindgen(start)]
pub fn main() {
    // Retain the WASI stubs before the kernel's objects are linked in.
    ferrocad_wasm::install();
    // The C++ runtime behind the kernel, before anything can call it.
    unsafe { __wasm_call_ctors() };
    // Logging and the panic hook; must run before the application.
    gpui::web_init();

    // WebGL2 on purpose: browser WebGPU is absent on Linux desktop Chrome, and
    // `Auto` would probe for it first. This is the backend the probe is about.
    gpui::application_with_web_backend(WebBackendPreference::WebGl).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Probe::new()),
        )
        .expect("the browser window");
        cx.activate(true);
    });
}

struct Probe {
    /// Built on the first paint, when the window's device exists, and kept: the
    /// producer owns the texture across frames, as the real viewport does.
    surface: Rc<RefCell<Option<WgpuSurface>>>,
    /// The FerroCAD renderer, on the window's device. Created with the surface.
    renderer: Rc<RefCell<Option<WgpuRenderer>>>,
    /// The document's packets, built once. The real viewport rebuilds them when
    /// the document changes; a static feature does not need that.
    packets: Rc<Vec<DrawPacket>>,
    camera: Rc<RefCell<Camera>>,
    /// Frames painted, for the console.
    frames: Rc<RefCell<u32>>,
    /// Whether this surface founded the current gesture, and where the pointer was
    /// at the previous move. The gateway holds no state across frames; the producer
    /// pairs a down with its up itself.
    grab: Rc<Cell<bool>>,
    last: Rc<Cell<Option<SurfacePoint>>>,
}

impl Probe {
    fn new() -> Self {
        let packets = match document_packets(RADIUS) {
            Ok(packets) => packets,
            Err(error) => {
                // Never fail the window: an empty stage draws the background, and
                // the reason is in the console.
                log::error!("ferrocad probe: the document did not build: {error}");
                Vec::new()
            }
        };
        log::info!("ferrocad probe: {} packets from the document", packets.len());

        Self {
            surface: Rc::new(RefCell::new(None)),
            renderer: Rc::new(RefCell::new(None)),
            packets: Rc::new(packets),
            camera: Rc::new(RefCell::new(demo_camera())),
            frames: Rc::new(RefCell::new(0)),
            grab: Rc::new(Cell::new(false)),
            last: Rc::new(Cell::new(None)),
        }
    }
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The surface repaints on its own schedule for now; the real viewport
        // draws on demand, and the gateway marks the window for redraw itself.
        window.request_animation_frame();

        let surface = Rc::clone(&self.surface);
        let renderer = Rc::clone(&self.renderer);
        let packets = Rc::clone(&self.packets);
        let camera = Rc::clone(&self.camera);
        let frames = Rc::clone(&self.frames);

        let painted_camera = Rc::clone(&self.camera);
        let canvas = gpu_canvas(move |gpu: &mut GpuCanvasContext| {
            paint(gpu, &surface, &renderer, &packets, &painted_camera, &frames);
        })
        // The element sizes itself from its childless content otherwise: it
        // measured 320x1 inside a 320x320 div.
        .size_full();

        // Input is the gateway's: drag orbits, the wheel zooms (through
        // `SurfaceEvent::Scroll`), and a double-click recentres the view. The
        // gateway founds GPUI's drag on the down, so the gesture keeps arriving
        // past the canvas edge.
        let grab = Rc::clone(&self.grab);
        let last = Rc::clone(&self.last);
        let element = surface_input(canvas, move |input: SurfaceInput| {
            // `Camera::orbit` takes logical pixels, like the canvas demo's pointer
            // listeners; the gateway works in physical ones.
            let scale = input.scale.max(1.0);
            match input.event {
                SurfaceEvent::PointerDown {
                    position,
                    click_count,
                    ..
                } => {
                    // Double-click recentres, as a CAD viewport does. This is also the
                    // check that the gateway forwards the engine's click count.
                    if click_count >= 2 {
                        *camera.borrow_mut() = demo_camera();
                    }
                    grab.set(true);
                    last.set(Some(position));
                }
                SurfaceEvent::Scroll { delta, .. } => {
                    // The sign is the engine's (positive is "up"); scrolling down
                    // zooms out, as the canvas demo does.
                    // Pixel deltas are physical (the gateway scales them like every
                    // other distance), so a notch is 40 *logical* pixels.
                    let notches = match delta {
                        SurfaceScroll::Lines { y, .. } => y,
                        SurfaceScroll::Pixels { y, .. } => y / (PIXELS_PER_LINE * scale),
                    };
                    camera
                        .borrow_mut()
                        .dolly(ZOOM_PER_LINE.powf(-f64::from(notches)));
                }
                SurfaceEvent::PointerEnter { .. } | SurfaceEvent::PointerLeave { .. } => {}
                SurfaceEvent::PointerMove {
                    position, pressed, ..
                } => {
                    if !grab.get() {
                        return;
                    }
                    // The gateway only delivers an up for a gesture the engine's drag is
                    // carrying — left button, past its threshold. A right or middle press
                    // released off the canvas therefore writes no `PointerUp`, and the
                    // first sign the gesture ended is a move with no button pressed. Treat
                    // it as the release rather than orbiting on every later hover.
                    if pressed.is_none() {
                        grab.set(false);
                        last.set(None);
                        return;
                    }
                    if let Some(previous) = last.replace(Some(position)) {
                        camera.borrow_mut().orbit([
                            (position.x - previous.x) / scale,
                            (position.y - previous.y) / scale,
                        ]);
                    }
                }
                SurfaceEvent::PointerUp { .. } => {
                    grab.set(false);
                    last.set(None);
                }
            }
        });

        div()
            .size_full()
            .bg(rgb(0x101014))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p_6()
            .child(div().w(px(320.)).h(px(320.)).child(element))
    }
}

/// One frame: render the document's packets into the surface, then composite it.
fn paint(
    gpu: &mut GpuCanvasContext,
    surface: &Rc<RefCell<Option<WgpuSurface>>>,
    renderer: &Rc<RefCell<Option<WgpuRenderer>>>,
    packets: &Rc<Vec<DrawPacket>>,
    camera: &Rc<RefCell<Camera>>,
    frames: &Rc<RefCell<u32>>,
) {
    let bounds = gpu.bounds();
    let scale = gpu.scale_factor();
    let width = ((f32::from(bounds.size.width) * scale).round() as u32).max(1);
    let height = ((f32::from(bounds.size.height) * scale).round() as u32).max(1);

    // The window's own device. This is the call the native presenter makes, and
    // the one our wasm `GpuRenderer` impl exists for.
    let Some((device, queue)) = gpu.try_device::<GpuiWgpuRenderer>() else {
        log::error!("ferrocad probe: the window renderer lent no device");
        return;
    };

    let mut surface_slot = surface.borrow_mut();
    let mut renderer_slot = renderer.borrow_mut();

    if surface_slot.is_none() {
        let made = match WgpuSurface::new(&device, width, height) {
            Ok(made) => made,
            Err(error) => {
                log::error!("ferrocad probe: WgpuSurface::new failed: {error}");
                return;
            }
        };
        // The renderer draws straight into the surface's view, so it is built for
        // the same (sRGB) format and the same device.
        *renderer_slot = Some(WgpuRenderer::with_sample_count_and_format(
            GpuContext::new((*device).clone(), (*queue).clone()),
            [width, height],
            // MSAA is the untested path on WebGL2; one sample always works.
            1,
            made.format(),
        ));
        *surface_slot = Some(made);
    }

    let surface = surface_slot.as_mut().expect("just built");
    let renderer = renderer_slot.as_mut().expect("just built");
    if let Err(error) = surface.resize(&device, width, height) {
        log::error!("ferrocad probe: WgpuSurface::resize failed: {error}");
        return;
    }
    renderer.resize([width, height]);

    let frame = {
        let mut frames = frames.borrow_mut();
        *frames += 1;
        *frames
    };

    // The pixel size drives the projection; the pose belongs to the input handlers.
    let camera = {
        let mut camera = camera.borrow_mut();
        camera.viewport = Viewport::new([width, height], scale);
        camera
    };

    if let Err(error) = renderer.render(surface.view(), &camera, packets.as_slice()) {
        log::error!("ferrocad probe: ferrocad_wgpu render failed: {error}");
        return;
    }

    // Hand the filled view to GPUI as an imported texture and composite it.
    match surface.view().to_imported_handle() {
        Ok(handle) => {
            gpu.paint_texture(handle, Default::default(), 1.0, false);
            if frame == 1 || frame % 120 == 0 {
                log::info!(
                    "ferrocad probe: {} packets, {}x{}, frame {frame}",
                    packets.len(),
                    width,
                    height
                );
            }
        }
        Err(error) => log::error!("ferrocad probe: to_imported_handle failed: {error}"),
    }
}

/// The `Part` document's draw packets: a 10 mm box with a fillet on one edge,
/// recomputed by the kernel and tessellated for the stage.
fn document_packets(radius: f64) -> Result<Vec<DrawPacket>, String> {
    // Idempotent: the Part module keeps one backend per process.
    ferrocad_part::register(Arc::new(OcctBackend::new()));

    let mut document = Document::new();
    let base = document.add_object("Box", BOX_TYPE);
    // `make_box` builds from the corner at the origin; centre it so the orbit
    // camera, which pivots on the origin, frames the whole box.
    document.set_property(
        base,
        "Placement",
        Property::Placement(Placement::new(
            Vector3::new(-5.0, -5.0, -5.0),
            Rotation::identity(),
        )),
    )?;

    let fillet = document.add_object("Fillet", FILLET_TYPE);
    document.set_property(
        fillet,
        "Base",
        Property::LinkSub("Box".to_string(), vec![EDGE.to_string()]),
    )?;
    document.set_property(fillet, "Radius", Property::Float(radius))?;
    document.recompute()?;

    // A refused fillet is a document state, not an error: the feature keeps its
    // last good shape, so the box is drawn and the reason is only reported. (The
    // radius above is chosen to succeed.)
    let object = document
        .object(fillet)
        .ok_or_else(|| "the fillet disappeared after recompute".to_string())?;
    if object.invalid {
        log::warn!(
            "ferrocad probe: the kernel refused the fillet: {}",
            object.error.as_deref().unwrap_or("no reason recorded")
        );
    }

    let shape = object
        .properties
        .get("Shape")
        .and_then(shape_of)
        .and_then(|property| property.shape())
        .ok_or_else(|| "the fillet produced no shape".to_string())?;

    let backend = ferrocad_part::backend();
    let mesh = backend
        .tessellate(&shape, &TessellationOptions::absolute(TOLERANCE))
        .map_err(|error| error.to_string())?;
    log::info!(
        "ferrocad probe: document tessellated to {} triangles",
        mesh.triangles.len() / 3
    );

    Ok(Stage::new()
        .with_item(
            ShapeItem::from_tessellation(1, &mesh)
                .with_draw_mode(DrawMode::ShadedWithEdges)
                .with_surface_color([0.62, 0.66, 0.72, 1.0]),
        )
        .to_queue()
        .packets)
}

/// The camera pose the canvas demo uses for its 10 mm box.
fn demo_camera() -> Camera {
    Camera {
        // `view` is world→view, and `Placement::new(pos, rot)` composes as
        // `T(pos) · R`: `pos` is a view-space translation applied to the
        // already-rotated point, so only -Z keeps the box centred.
        view: Placement::new(
            Vector3::new(0.0, 0.0, -28.0),
            Rotation::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0), 0.5),
        )
        .to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0_f64.to_radians(),
            near: 0.1,
            far: 100.0,
        },
        ..Camera::identity()
    }
}
