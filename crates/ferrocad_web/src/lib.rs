//! The demo host: OCCT in wasm, drawn by [`WebViewport`].
//!
//! `#[wasm_bindgen(start)]` runs on `init()`, finds `#cad`, builds a filleted box
//! with the real kernel, and draws it every animation frame. The surface, device and
//! orbit/zoom input live in `ferrocad_web_viewport`; this crate is the scene and the
//! demo pose, plus the smoke-test export.
//!
//! Everything is gated on `wasm32`: on other targets the crate is empty, so a
//! workspace `cargo check` does not need the browser APIs.

#![cfg(target_arch = "wasm32")]

mod wasi;

use std::{cell::RefCell, rc::Rc};

use ferrocad_geom::{ElementKind, GeomError, GeometryBackend, Tessellation, TessellationOptions};
use ferrocad_graphics::{Camera, Projection, RenderQueue};
use ferrocad_occt::OcctBackend;
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_web_viewport::WebViewport;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    fn __wasm_call_ctors();
}

#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    // wasm-bindgen 0.2.129's glue reads the `__instance_terminated` export.
    // The static is `#[no_mangle]` but `--gc-sections` drops it when nothing
    // references it, so keep it reachable from this exported entry point.
    unsafe { core::ptr::read_volatile(&raw const wasm_bindgen::__rt::__instance_terminated) };
    // Before anything that touches the kernel; also runs under Node, where there
    // is no canvas and `triangles()` is the only thing a test calls.
    unsafe { __wasm_call_ctors() };

    let Some(canvas) = canvas() else {
        return Ok(());
    };

    wasm_bindgen_futures::spawn_local(async move {
        match App::new(canvas).await {
            Ok(app) => request_frame(Rc::new(RefCell::new(app))),
            Err(error) => web_sys::console::error_1(&error),
        }
    });
    Ok(())
}

fn canvas() -> Option<web_sys::HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id("cad")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

struct App {
    viewport: WebViewport,
    queue: RenderQueue,
}

impl App {
    async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsValue> {
        let camera = Camera {
            // `view` is world→view, and `Placement::new(pos, rot)` composes as
            // `T(pos) · R`: `pos` is a view-space translation applied to the
            // already-rotated point. A lateral term therefore slides the box (at
            // the origin) off the view axis; only -Z keeps it centred.
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
            // The canvas owns the size; the viewport replaces this.
            ..Camera::identity()
        };
        let viewport = WebViewport::new(canvas, camera).await?;

        let mesh = filleted_box().map_err(|error| JsValue::from_str(&error.to_string()))?;
        let queue = Stage::new()
            .with_item(
                ShapeItem::from_tessellation(1, &mesh)
                    .with_draw_mode(DrawMode::ShadedWithEdges)
                    .with_surface_color([0.62, 0.66, 0.72, 1.0]),
            )
            .to_queue();

        Ok(Self { viewport, queue })
    }

    fn frame(&mut self) {
        self.viewport.draw(&self.queue.packets);
    }
}

/// The triangle count of a box filleted on one edge, for the smoke test and for
/// hosts that want to check the kernel before creating a canvas.
#[wasm_bindgen]
pub fn triangles() -> u32 {
    filleted_box()
        .map(|mesh| (mesh.triangles.len() / 3) as u32)
        .unwrap_or(0)
}

fn filleted_box() -> Result<Tessellation, GeomError> {
    let backend = OcctBackend::new();
    let shape = backend.make_box(10.0, 10.0, 10.0)?;
    let edges = backend.elements(&shape, ElementKind::Edge);
    let edge = edges.first().ok_or("the box has no edges")?;
    let filleted = backend.fillet(&shape, std::slice::from_ref(edge), 1.0)?;
    // `make_box` builds from the corner at the origin; centre it so the orbit
    // camera, which looks at the origin, frames the whole box.
    let centred = backend.place(
        &filleted.shape,
        &Placement::new(Vector3::new(-5.0, -5.0, -5.0), Rotation::identity()),
    )?;
    backend.tessellate(&centred, &TessellationOptions::absolute(0.2))
}

fn request_frame(app: Rc<RefCell<App>>) {
    let next = app.clone();
    let callback = Closure::once_into_js(move || {
        app.borrow_mut().frame();
        request_frame(next);
    });
    web_sys::window()
        .expect("window")
        .request_animation_frame(callback.unchecked_ref())
        .expect("requestAnimationFrame");
}
