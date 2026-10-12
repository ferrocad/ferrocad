//! Proof of concept for the whole render roundtrip, minus the `pivy.coin` bindings:
//!
//! ```text
//! ferrocad_inventor (scene → compile → batches)
//!     → ferrocad_wgpu (render + readback)
//!         → ferrocad_gpui (InventorViewport shows the image)
//! ```
//!
//! It draws the same sample scene the app shell shows, and moving the pointer over
//! the cube picks it and a listener highlights it — the hover path FreeCAD drives
//! through `View3DInventorViewer` + `SoEventCallback`.
//!
//! ```text
//! # window (needs a display server and a GPU):
//! cargo run -p ferrocad_gpui --example inventor_roundtrip
//!
//! # headless: simulate a hover, render, read the frame back, and write a PNG:
//! cargo run -p ferrocad_gpui --example inventor_roundtrip -- --headless frame.png
//! ```

use ferrocad_gpui::InventorViewport;
use ferrocad_gpui::scene::{HoverHighlight, sample_camera, sample_scene};
use ferrocad_inventor::{
    Event, EventListener, InventorView, Scene, Viewport, compile_with, pick_at,
};
use ferrocad_wgpu::WgpuRenderer;
use gpui::{App, AppContext, WindowBounds, WindowOptions, application, bounds, point, px, size};

/// The offscreen render target size (device pixels; 1x here for the PoC).
const SIZE: [u32; 2] = [640, 480];

fn main() {
    let camera = sample_camera(Viewport::new(SIZE, 1.0));
    let scene = sample_scene();

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
            cx.new(|_cx| {
                InventorViewport::new(renderer, camera, scene)
                    .with_listener(Box::new(HoverHighlight::default()))
            })
        })
        .unwrap();
    });
}

/// Headless: simulate a hover over the cube's centre, render, and write a PNG.
fn render_headless(mut scene: Scene, camera: ferrocad_inventor::Camera, path: &str) {
    let queue = compile_with(&scene, &camera.viewport);
    let [width, height] = camera.viewport.device_size();
    let centre = camera
        .viewport
        .to_device([width as f32 / 2.0, height as f32 / 2.0]);
    let hit = pick_at(&queue, &camera, centre);
    println!("ferrocad: pick at centre -> {hit:?}");

    // Run the same listener the window would, then recompile as it would.
    HoverHighlight::default().on_event(
        &Event::MouseMove { position: centre },
        hit,
        &mut scene,
    );
    let queue = compile_with(&scene, &camera.viewport);

    let mut renderer = WgpuRenderer::new_default(SIZE).unwrap_or_else(|e| {
        eprintln!("ferrocad: could not create a GPU renderer: {e}");
        std::process::exit(1);
    });
    renderer.draw(&queue, &camera).unwrap_or_else(|e| {
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
