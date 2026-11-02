//! The demo host: a `Part` document in the browser, drawn by [`WebViewport`].
//!
//! `#[wasm_bindgen(start)]` runs on `init()`: it builds the document (a 10 mm box
//! with a fillet on one edge) through the same `Document::recompute` path the
//! desktop app uses, finds `#cad`, and draws the recomputed shape every animation
//! frame. [`set_fillet_radius`] rebuilds it, so the page can drive a real feature
//! and show what the kernel says when it refuses.
//!
//! The surface, device and orbit/zoom input live in `ferrocad_web_viewport`; the
//! document lives in [`scene`]. Everything is gated on `wasm32`, so a workspace
//! `cargo check` does not need the browser APIs.

#![cfg(target_arch = "wasm32")]

mod scene;
mod wasi;

use std::{cell::RefCell, rc::Rc};

use ferrocad_graphics::{Camera, Projection};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_web_viewport::WebViewport;
use scene::{Scene, DEFAULT_RADIUS};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    fn __wasm_call_ctors();
}

thread_local! {
    /// The document the page's radius control drives. `main` installs it before
    /// the viewport exists, so an exported call always finds it.
    static SCENE: RefCell<Option<Rc<RefCell<Scene>>>> = const { RefCell::new(None) };
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

    // The document is built before the surface: it is fast, and a page that calls
    // `set_fillet_radius` early must find it.
    let scene = match Scene::new(DEFAULT_RADIUS) {
        Ok(scene) => Rc::new(RefCell::new(scene)),
        Err(error) => {
            web_sys::console::error_1(&JsValue::from_str(&error));
            return Ok(());
        }
    };
    SCENE.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&scene)));

    let Some(canvas) = canvas() else {
        return Ok(());
    };
    wasm_bindgen_futures::spawn_local(async move {
        match App::new(canvas, scene).await {
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
    scene: Rc<RefCell<Scene>>,
}

impl App {
    async fn new(
        canvas: web_sys::HtmlCanvasElement,
        scene: Rc<RefCell<Scene>>,
    ) -> Result<Self, JsValue> {
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
        Ok(Self { viewport, scene })
    }

    fn frame(&mut self) {
        let scene = self.scene.borrow();
        self.viewport.draw(scene.packets());
    }
}

/// The document's state after the last recompute, for the page.
#[wasm_bindgen]
pub struct Status {
    ok: bool,
    error: Option<String>,
    radius: f64,
    faces: u32,
    triangles: u32,
}

impl Status {
    fn of(scene: &Scene) -> Self {
        Self {
            ok: scene.error().is_none(),
            error: scene.error().map(str::to_string),
            radius: scene.radius(),
            faces: scene.faces(),
            triangles: scene.triangles(),
        }
    }
}

#[wasm_bindgen]
impl Status {
    /// Whether the last recompute produced a valid shape.
    #[wasm_bindgen(getter)]
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// The kernel's reason when `ok` is false.
    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }

    /// The radius the document holds, valid or not.
    #[wasm_bindgen(getter)]
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Faces in the last built shape.
    #[wasm_bindgen(getter)]
    pub fn faces(&self) -> u32 {
        self.faces
    }

    /// Triangles in the last built shape.
    #[wasm_bindgen(getter)]
    pub fn triangles(&self) -> u32 {
        self.triangles
    }
}

/// Recompute the document's fillet at `radius` and report the result.
///
/// The geometry on screen is the last **valid** result: `Part::Fillet` keeps its
/// last good shape when the kernel refuses, so a radius that cannot exist leaves
/// the box visible and reports why — the state the failure model is about.
#[wasm_bindgen]
pub fn set_fillet_radius(radius: f64) -> Result<Status, JsValue> {
    with_scene(|scene| {
        let mut scene = scene.borrow_mut();
        scene
            .set_radius(radius)
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(Status::of(&scene))
    })
}

/// The document's state, without rebuilding it.
#[wasm_bindgen]
pub fn fillet_status() -> Result<Status, JsValue> {
    with_scene(|scene| Ok(Status::of(&scene.borrow())))
}

fn with_scene<T>(f: impl FnOnce(Rc<RefCell<Scene>>) -> Result<T, JsValue>) -> Result<T, JsValue> {
    let scene = SCENE.with(|slot| slot.borrow().clone()).ok_or_else(|| {
        JsValue::from_str("the FerroCAD document has not been built (init() failed)")
    })?;
    f(scene)
}

/// The demo document's triangle count, for the Node smoke test.
///
/// Builds the same `Part` document as the page — box, fillet, `recompute` — so the
/// smoke test covers the workbench and document layers, not just the kernel.
#[wasm_bindgen]
pub fn triangles() -> u32 {
    Scene::new(DEFAULT_RADIUS)
        .map(|scene| scene.triangles())
        .unwrap_or(0)
}

/// Exercise `BRepFeat_SplitShape` and report upstream's two groups.
///
/// The page never splits a shape, so nothing else here would notice if `TKFeat`
/// fell out of the wasm kernel again: `split_shape` stays reachable through the
/// `GeometryBackend` vtable, and a missing toolkit only shows up as a link error
/// when something builds *this* module. The Node smoke test calls this so the
/// toolkit is exercised by behaviour too, not only by linking.
///
/// It is the native `make_split_shape_returns_two_groups` case from
/// `ferrocad_part_py`: a 1×2×2 box, its first face, and one of that face's edges
/// shifted 0.5 mm along the face as the cutter. Upstream splits the face into one
/// `DirectLeft` and one `Left` piece.
///
/// Returns `[direct, left]` piece counts, or the kernel's error.
#[wasm_bindgen]
pub fn split_check() -> Result<Vec<u32>, JsValue> {
    use ferrocad_geom::GeometryBackend as _;

    let backend = ferrocad_occt::OcctBackend::new();
    let to_js = |error: ferrocad_geom::GeomError| JsValue::from_str(&error.to_string());

    let boxed = backend.make_box(1.0, 2.0, 2.0).map_err(to_js)?;
    let face = backend
        .resolve(&boxed, "Face1")
        .ok_or_else(|| JsValue::from_str("the box has no Face1"))?;
    let edge = backend
        .resolve(&face, "Edge1")
        .ok_or_else(|| JsValue::from_str("the face has no Edge1"))?;
    let cutter = backend
        .place(
            &edge,
            &Placement::new(Vector3::new(0.0, 0.5, 0.0), Rotation::identity()),
        )
        .map_err(to_js)?;
    let (direct, left) = backend
        .split_shape(&face, &[(cutter, face.clone())], false)
        .map_err(to_js)?;
    Ok(vec![direct.len() as u32, left.len() as u32])
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
