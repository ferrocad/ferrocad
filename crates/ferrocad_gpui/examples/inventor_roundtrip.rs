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
    Camera, Event, EventListener, InventorView, Scene, ViewSpec, Viewport, compile_with, pick_at_hit,
};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_wgpu::WgpuRenderer;
use gpui::{App, AppContext, WindowBounds, WindowOptions, application, bounds, point, px, size};

/// The offscreen render target size (device pixels; 1x here for the PoC).
const SIZE: [u32; 2] = [640, 480];

/// The sample camera, tilted toward an axonometric view so both the scene and the
/// navigation cube show three faces.
fn camera(viewport: Viewport) -> Camera {
    let direction = Vector3::new(1.0, 0.9, 1.0).normalize();
    let mut camera = sample_camera(viewport);
    camera.view = Placement::new(
        Vector3::new(0.0, 0.0, -4.0),
        Rotation::from_vectors(direction, Vector3::new(0.0, 0.0, 1.0)),
    )
    .to_matrix();
    camera
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
    if args.next().as_deref() == Some("--headless") {
        let path = args.next().unwrap_or_else(|| "frame.png".to_string());
        render_headless(scene, camera, &path);
        return;
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

/// Headless: simulate a hover over the cube's centre, render, and write a PNG.
fn render_headless(mut scene: Scene, camera: ferrocad_inventor::Camera, path: &str) {
    let (queue, directives) = compile_with(&scene, &camera.viewport);
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
    let (queue, _) = compile_with(&scene, &camera.viewport);

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
