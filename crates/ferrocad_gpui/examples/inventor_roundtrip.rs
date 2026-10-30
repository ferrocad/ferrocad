//! Proof of concept for the whole render roundtrip, now including the `pivy.coin`
//! bindings:
//!
//! ```text
//! Python workbench (`from pivy import coin`)
//!     → ferrocad_inventor_py (extract) → ferrocad_inventor (compile → batches)
//!         → ferrocad_wgpu (render + readback)
//!             → ferrocad_viewport (InventorViewport shows the image)
//! ```
//!
//! The cube is built by a Python script (the twin of the Rust `sample_scene`), so
//! the whole stack is exercised end to end. Moving the pointer over the cube picks
//! it and a listener highlights it — the hover path FreeCAD drives through
//! `View3DInventorViewer` + `SoEventCallback`.
//!
//! ```text
//! # window (needs a display server and a GPU):
//! cargo run -p ferrocad_gpui --example inventor_roundtrip
//!
//! # headless: simulate a hover, render, read the frame back, and write a PNG:
//! cargo run -p ferrocad_gpui --example inventor_roundtrip -- --headless frame.png
//! ```

use ferrocad_viewport::InventorViewport;
use ferrocad_viewport::scene::{HoverHighlight, sample_camera, sample_scene};
use ferrocad_inventor::{
    Camera, Event, EventListener, InventorView, Node, NodeKind, Scene, ViewSpec, Viewport,
    compile_diagnostic, pick_at_hit,
};
use ferrocad_wgpu::WgpuRenderer;
use gpui::{App, AppContext, WindowBounds, WindowOptions, application, bounds, point, px, size};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::ffi::CString;

/// The offscreen render target size (device pixels; 1x here for the PoC).
const SIZE: [u32; 2] = [640, 480];

/// The sample camera: a Z-up axonometric view, so the scene and the navigation cube
/// show three faces with world `+Z` up.
fn camera(viewport: Viewport) -> Camera {
    sample_camera(viewport)
}

/// Build the scene the workbench way — a `from pivy import coin` script extracted
/// into a [`Scene`] — and fall back to the Rust twin if the interpreter is absent.
fn build_scene() -> Scene {
    ferrocad_gpui::python::inventor_scene().unwrap_or_else(|e| {
        eprintln!("ferrocad: python scene unavailable ({e}); using the Rust sample");
        sample_scene()
    })
}

fn main() {
    let camera = camera(Viewport::new(SIZE, 1.0));
    let scene = build_scene();

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--headless") => {
            let path = args.next().unwrap_or_else(|| "frame.png".to_string());
            render_headless(scene, camera, &path);
            return;
        }
        Some("--coin-overlay") => {
            let path = args
                .next()
                .unwrap_or_else(|| "coin_overlay.png".to_string());
            render_headless(with_coin_overlay(scene), camera, &path);
            return;
        }
        _ => {}
    }

    let renderer = WgpuRenderer::new_default(SIZE).unwrap_or_else(|e| {
        eprintln!("ferrocad: could not create a GPU renderer: {e}");
        std::process::exit(1);
    });

    application().run(move |cx: &mut App| {
        cx.open_window(window_options(), move |_window, cx| {
            let navicube = ferrocad_viewport::NaviCube::new(camera.viewport);
            cx.new(|_cx| {
                let viewport = InventorViewport::new(renderer, camera, scene)
                    .with_listener(Box::new(HoverHighlight::default()))
                    .with_navicube(navicube);
                match ferrocad_gpui::zero_copy_presenter() {
                    Some(presenter) => viewport.with_presenter(presenter),
                    None => viewport,
                }
            })
        })
        .unwrap();
    });
}

/// Register the Coin/WGSL grid mod and merge its overlay into the scene.
///
/// This exercises the G5 path end to end without a window: the mod builds its
/// `SoShaderProgram` tree, `FerroCADGui.addOverlay` extracts it, and the resulting
/// scene is compiled and drawn by the same renderer the shell uses.
fn with_coin_overlay(base: Scene) -> Scene {
    const SCRIPT: &str = r#"
import importlib.util, os
import FreeCAD
# Create the document before the mod attaches its observer and activates, so the
# module's own manager builds exactly one grid for it.
doc = FreeCAD.newDocument("Overlay")
path = os.path.abspath("mods/GridOverlayCoin/InitGui.py")
spec = importlib.util.spec_from_file_location("GridOverlayCoin", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
mod.manager.refresh()
"#;

    let overlays = Python::with_gil(|py| -> Result<Vec<Scene>, String> {
        ferrocad_inventor_py::install(py).map_err(|e| e.to_string())?;
        let globals = PyDict::new(py);
        let code = CString::new(SCRIPT).map_err(|e| e.to_string())?;
        py.run(&code, Some(&globals), None).map_err(|e| e.to_string())?;
        Ok(ferrocad_inventor_py::overlay_scenes())
    });

    match overlays {
        Ok(overlays) if !overlays.is_empty() => {
            let count = overlays.len();
            let mut root = Node {
                kind: NodeKind::Separator,
                children: vec![base.root],
            };
            root.children
                .extend(overlays.into_iter().map(|scene| scene.root));
            println!("ferrocad: {count} coin overlay(s) registered");
            Scene { root }
        }
        Ok(_) => {
            eprintln!("ferrocad: the coin mod registered no overlay");
            base
        }
        Err(error) => {
            eprintln!("ferrocad: coin overlay failed: {error}");
            base
        }
    }
}

/// Headless: simulate a hover over the cube's centre, render, and write a PNG.
fn render_headless(mut scene: Scene, camera: ferrocad_inventor::Camera, path: &str) {
    // A tool, not the viewport: keep the frame if one program is malformed and report it.
    let (queue, directives, error) = compile_diagnostic(&scene, &camera.viewport, None);
    if let Some(error) = error {
        eprintln!("ferrocad: shader program: {error}");
    }
    let [width, height] = camera.viewport.device_size();
    let centre = camera
        .viewport
        .to_device([width as f32 / 2.0, height as f32 / 2.0]);
    let hit = pick_at_hit(&queue, &camera, centre);
    println!("ferrocad: pick at centre -> {:?}", hit.as_ref().map(|h| h.id));

    // Run the same listener the window would, then recompile as it would.
    HoverHighlight::default().on_event(
        &Event::MouseMove { position: centre },
        hit.as_ref(),
        &mut scene,
        &directives,
    );

    // Deliver the pick to any Python `SoEventCallback`: a click at the centre.
    ferrocad_gpui::python::dispatch(
        &directives,
        &Event::MouseButton {
            position: centre,
            button: 1,
            down: true,
        },
        hit.as_ref(),
    );
    let (queue, _, error) = compile_diagnostic(&scene, &camera.viewport, None);
    if let Some(error) = error {
        eprintln!("ferrocad: shader program: {error}");
    }

    let mut renderer = WgpuRenderer::new_default(SIZE).unwrap_or_else(|e| {
        eprintln!("ferrocad: could not create a GPU renderer: {e}");
        std::process::exit(1);
    });
    // Composite the navigation cube over the scene in the corner.
    let mut navicube = ferrocad_viewport::NaviCube::new(camera.viewport);
    navicube.sync(&camera);
    let views = [
        ViewSpec {
            queue: &queue,
            camera: &camera,
            rect: None,
            overlay: false,
            workplane: None,
        },
        navicube.view(),
    ];
    renderer.draw_frame(&views).unwrap_or_else(|e| {
        eprintln!("ferrocad: draw failed: {e}");
        std::process::exit(1);
    });
    let image = renderer.image().unwrap_or_else(|e| {
        eprintln!("ferrocad: readback failed: {e}");
        std::process::exit(1);
    });
    let buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
        .expect("readback buffer size");
    buffer.save(path).unwrap_or_else(|e| {
        eprintln!("ferrocad: could not write {path}: {e}");
        std::process::exit(1);
    });
    println!("ferrocad: wrote {path} ({}x{})", image.width, image.height);
}

fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds(
            point(px(60.), px(60.)),
            size(px(680.), px(540.)),
        ))),
        ..Default::default()
    }
}
