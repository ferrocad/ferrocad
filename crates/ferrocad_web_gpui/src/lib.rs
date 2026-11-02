//! A GPUI window in the browser, compositing a wgpu surface through
//! `gpui_interop`.
//!
//! Step 2's smallest increment: it proves the seam the `ferrocad_gpui` interop
//! presenter needs on wasm, end to end in a real browser —
//!
//! 1. `GpuCanvasContext::try_device::<WgpuRenderer>()` answers, i.e. the browser
//!    renderer lends its own `wgpu::Device` and `Queue` (our wasm
//!    `GpuRenderer` impl in `third_party/bite-gp-wgpu/src/web_renderer.rs`);
//! 2. a `WgpuSurface` allocated on that device can be rendered into with an
//!    ordinary wgpu command encoder;
//! 3. `view().to_imported_handle()` typechecks on wasm (the vendored
//!    `wasm-interop` feature plus wgpu's `fragile-send-sync-non-atomic-wasm`),
//!    and `paint_texture` composites it into GPUI's scene.
//!
//! Everything here is deliberately smaller than the real shell: no document, no
//! CPython, no OCCT, no image path. If the coloured square animates, the seam
//! works and `ferrocad_viewport` can start drawing real scenes through it.
//!
//! There is **no text** on purpose: GPUI panics when asked to resolve a font and
//! none has been added (`DefaultTextSystem::resolve_font`), and font loading is
//! `ferrocad_text`'s job, not this probe's. The paint status goes to the console
//! through the logger `gpui::web_init` installs.

#![cfg(target_family = "wasm")]

use std::{cell::RefCell, rc::Rc};

use gpui::{
    App, Bounds, Context, GpuCanvasContext, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, gpu_canvas, prelude::*, px, rgb, size,
};
use gpui_interop::WgpuSurface;
use gpui_wgpu::{ImportedTextureExt as _, WebBackendPreference, WgpuRenderer};
use wasm_bindgen::prelude::*;

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
    /// Frames painted, which also drives the colour.
    frames: Rc<RefCell<u32>>,
}

impl Probe {
    fn new() -> Self {
        Self {
            surface: Rc::new(RefCell::new(None)),
            frames: Rc::new(RefCell::new(0)),
        }
    }
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // A surface that is only composited once cannot be told from a stale
        // frame, so keep asking. The real viewport does this while it animates.
        window.request_animation_frame();

        let surface = Rc::clone(&self.surface);
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
                            paint(gpu, &surface, &frames);
                        })
                        // The element sizes itself from its childless content
                        // otherwise: it measured 320x1 inside a 320x320 div.
                        .size_full(),
                    ),
            )
    }
}

/// Render one frame into the surface and composite it into the canvas.
fn paint(
    gpu: &mut GpuCanvasContext,
    surface: &Rc<RefCell<Option<WgpuSurface>>>,
    frames: &Rc<RefCell<u32>>,
) {
    let bounds = gpu.bounds();
    let scale = gpu.scale_factor();
    let width = ((f32::from(bounds.size.width) * scale).round() as u32).max(1);
    let height = ((f32::from(bounds.size.height) * scale).round() as u32).max(1);

    // Step 1: the window's own device. This is the call the native presenter
    // makes, and the one our wasm `GpuRenderer` impl exists for.
    let Some((device, queue)) = gpu.try_device::<WgpuRenderer>() else {
        log::error!("ferrocad probe: the window renderer lent no device");
        return;
    };

    // Step 2: an offscreen target on that device.
    let mut slot = surface.borrow_mut();
    if slot.is_none() {
        match WgpuSurface::new(&device, width, height) {
            Ok(made) => *slot = Some(made),
            Err(error) => {
                log::error!("ferrocad probe: WgpuSurface::new failed: {error}");
                return;
            }
        }
    }
    let surface = slot.as_mut().expect("just built");
    if let Err(error) = surface.resize(&device, width, height) {
        log::error!("ferrocad probe: WgpuSurface::resize failed: {error}");
        return;
    }

    let frame = {
        let mut frames = frames.borrow_mut();
        *frames += 1;
        *frames
    };

    // A colour that moves: a still screenshot then still shows a live surface,
    // and two screenshots cannot agree by accident.
    let phase = frame as f32 * 0.06;
    let clear = wgpu::Color {
        r: 0.5 + 0.35 * f64::from(phase.sin()),
        g: 0.42,
        b: 0.5 + 0.35 * f64::from(phase.cos()),
        a: 1.0,
    };

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ferrocad-web-gpui probe"),
    });
    {
        // Dropping the pass ends it; the encoder is finished below.
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("probe clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    queue.submit(Some(encoder.finish()));

    // Step 3: hand the view to GPUI as an imported texture and composite it.
    match surface.view().to_imported_handle() {
        Ok(handle) => {
            gpu.paint_texture(handle, Default::default(), 1.0, false);
            if frame == 1 || frame % 120 == 0 {
                log::info!("ferrocad probe: painted {width}x{height} (frame {frame})");
            }
        }
        Err(error) => log::error!("ferrocad probe: to_imported_handle failed: {error}"),
    }
}
