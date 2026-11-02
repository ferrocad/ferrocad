//! A GPUI window in the browser, compositing a FerroCAD wgpu scene through
//! `gpui_interop`.
//!
//! Step 2's probe, second increment: the square from the first increment is
//! replaced by the real render path. `ferrocad_stage` emits the draw packets,
//! `ferrocad_wgpu` — the renderer the canvas demo uses — draws them into a
//! `gpui_interop::WgpuSurface` on the window's own device, and GPUI composites
//! the result with `paint_texture`. The camera orbits a little every frame, so a
//! screenshot is evidence of a live surface rather than a cached frame.
//!
//! What this still is not: no document, no CPython, no OCCT. The scene is a
//! hand-built cube in the stage's own vocabulary, because the point here is the
//! GPU seam, not the document layer — `ferrocad_gpui` cannot be the wasm crate
//! until its PyO3 dependency is split.
//!
//! There is **no text** on purpose: GPUI panics when asked to resolve a font and
//! none has been added (`DefaultTextSystem::resolve_font`), and font loading is
//! `ferrocad_text`'s job. Status goes to the browser console through the logger
//! `gpui::web_init` installs.

#![cfg(target_family = "wasm")]

use std::{cell::RefCell, rc::Rc};

use ferrocad_geom::Tessellation;
use ferrocad_graphics::{Camera, Projection, Viewport, DrawPacket};
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_wgpu::{GpuContext, WgpuRenderer};
use gpui::{
    App, Bounds, Context, GpuCanvasContext, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, gpu_canvas, prelude::*, px, rgb, size,
};
use gpui_interop::WgpuSurface;
use gpui_wgpu::{ImportedTextureExt as _, WebBackendPreference, WgpuRenderer as GpuiWgpuRenderer};
use wasm_bindgen::prelude::*;

/// The cube's half extent, in model units. The camera pose below is the demo's,
/// which was chosen for a 10 mm box.
const HALF_EXTENT: f32 = 5.0;

#[wasm_bindgen(start)]
pub fn main() {
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
    /// The stage's packets, built once. The real viewport rebuilds them per frame;
    /// a static cube does not need that.
    packets: Rc<Vec<DrawPacket>>,
    camera: Rc<RefCell<Camera>>,
    /// Frames painted, for the console.
    frames: Rc<RefCell<u32>>,
}

impl Probe {
    fn new() -> Self {
        Self {
            surface: Rc::new(RefCell::new(None)),
            renderer: Rc::new(RefCell::new(None)),
            packets: Rc::new(cube_packets()),
            camera: Rc::new(RefCell::new(demo_camera())),
            frames: Rc::new(RefCell::new(0)),
        }
    }
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The camera moves every frame, so keep asking for frames. The real
        // viewport does this while it animates.
        window.request_animation_frame();

        let surface = Rc::clone(&self.surface);
        let renderer = Rc::clone(&self.renderer);
        let packets = Rc::clone(&self.packets);
        let camera = Rc::clone(&self.camera);
        let frames = Rc::clone(&self.frames);

        div()
            .size_full()
            .bg(rgb(0x101014))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                div()
                    .w(px(320.))
                    .h(px(320.))
                    .child(
                        gpu_canvas(move |gpu: &mut GpuCanvasContext| {
                            paint(gpu, &surface, &renderer, &packets, &camera, &frames);
                        })
                        // The element sizes itself from its childless content
                        // otherwise: it measured 320x1 inside a 320x320 div.
                        .size_full(),
                    ),
            )
    }
}

/// One frame: render the stage into the surface, then composite it.
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

    // Turn the model a little each frame: a live surface, not a cached one.
    let camera = {
        let mut camera = camera.borrow_mut();
        camera.viewport = Viewport::new([width, height], scale);
        camera.orbit([0.8, 0.3]);
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

/// The stage's packets for one shaded cube with its edges drawn.
fn cube_packets() -> Vec<DrawPacket> {
    Stage::new()
        .with_item(
            ShapeItem::from_tessellation(1, &cube(HALF_EXTENT))
                .with_draw_mode(DrawMode::ShadedWithEdges)
                .with_surface_color([0.62, 0.66, 0.72, 1.0]),
        )
        .to_queue()
        .packets
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

/// A cube of half extent `h`: eight shared corners, twelve triangles and the
/// twelve edges, in the stage's tessellation vocabulary.
///
/// Positions are shared rather than duplicated per face (the kernel duplicates
/// them so each triangle can carry its own part id); with no normals the
/// renderer shades flat, which is what a box wants.
fn cube(h: f32) -> Tessellation {
    let positions = vec![
        [-h, -h, -h],
        [h, -h, -h],
        [h, h, -h],
        [-h, h, -h],
        [-h, -h, h],
        [h, -h, h],
        [h, h, h],
        [-h, h, h],
    ];
    // Two triangles per face; winding is irrelevant while culling is off.
    let triangles = vec![
        4, 5, 6, 4, 6, 7, // +z
        1, 0, 3, 1, 3, 2, // -z
        0, 4, 7, 0, 7, 3, // -x
        5, 1, 2, 5, 2, 6, // +x
        7, 6, 2, 7, 2, 3, // +y
        0, 1, 5, 0, 5, 4, // -y
    ];
    let edges = vec![
        0, 1, 1, 2, 2, 3, 3, 0, // -z
        4, 5, 5, 6, 6, 7, 7, 4, // +z
        0, 4, 1, 5, 2, 6, 3, 7, // connecting
    ];
    Tessellation {
        triangle_parts: vec![0; triangles.len() / 3],
        segment_parts: vec![0; edges.len() / 2],
        vertices: vec![0, 1, 2, 3, 4, 5, 6, 7],
        vertex_parts: vec![0; 8],
        parts: vec!["Cube".to_string()],
        positions,
        triangles,
        edges,
        ..Tessellation::default()
    }
}
