//! `interop_linux`: a dma-buf produced outside GPUI's renderer, composited with no CPU copy.
//!
//! ```sh
//! cargo run -p gpui_interop --example interop_linux --features example
//! ```
//!
//! The window shows two solid tiles. Each is a `SharedBuffer` — a shareable Vulkan image the producer
//! allocated and wrote on its own device — handed to `gpu_canvas(..)` as a `gpui::DmaBufHandle`; the
//! window's renderer imports the dma-buf under its DRM modifier and samples it into the frame. The
//! pixels never touch the CPU.
//!
//! # What the crate is for
//!
//! The producer's device is **not** the window renderer's — the case this crate exists for. The
//! whole bridge is the few lines below, all inside the canvas whose callback is the only place the
//! renderer's device is reachable:
//!
//! ```text
//! attach(gpu.try_device::<WgpuRenderer>())   // reach the window's renderer
//!     .adapter().gpu_device()                // a dma-buf-capable device on the same GPU
//! SharedBuffer::new(&device, ..)             // a shareable image it can export
//! buffer.clear(&device, colour)              // render into it
//! gpu.paint_surface(buffer.handle())         // hand it to the scene
//! ```
//!
//! `Adapter::gpu_device` is Linux's answer to the Windows LUID match: the renderer's device names
//! its physical device, and the crate makes a *second* logical device on it, so the producer's queue
//! never races the renderer's while the export still lands on the GPU the window draws on. The
//! consumer half — import, the modifier-aware sampling, the wait — is core; the crate only produces.
//!
//! The fallback is honest about the alternative: with no renderer to match, the producer opens its
//! own device ([`GpuDevice::open`]). That works only when it happens to land on the same physical
//! GPU the window draws through — on a two-GPU machine it may not — and the header reports which of
//! the two paths the tiles were built on.

#[cfg(target_os = "linux")]
fn main() {
    demo::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "interop_linux is a Linux example: it exports a dma-buf and composites it through the \
         window's renderer. Run it on Linux."
    );
}

#[cfg(target_os = "linux")]
mod demo {
    use std::cell::RefCell;
    use std::rc::Rc;

    use anyhow::{Context as _, Result};
    use gpui::{
        AnyElement, App, Bounds, Context, DmaBufHandle, GpuCanvasContext, IntoElement, Render,
        SurfaceFormatKind, Window, WindowBounds, WindowOptions, div, gpu_canvas, prelude::*, px,
        rgb, size,
    };
    use gpui_interop::{GpuDevice, SharedBuffer, Tiling, attach};
    use gpui_wgpu::WgpuRenderer;

    /// The edge of each produced surface, in pixels, and of the tile it lands in.
    const TILE: u32 = 192;

    /// The colour each tile is cleared to. Solid, distinct colours, so a tile that comes up black is
    /// a surface that did not arrive rather than one that arrived carrying the wrong bytes.
    const LINEAR_COLOUR: [f32; 4] = [0.85, 0.35, 0.13, 1.0];
    const TILED_COLOUR: [f32; 4] = [0.13, 0.62, 0.58, 1.0];

    pub fn run() {
        gpui::application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(720.), px(520.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Showcase::new()),
            )
            .expect("a window to composite into");
            cx.activate(true);
        });
    }

    /// The window's view. The producer is built lazily at paint time — the renderer's device exists
    /// only then — so the view holds a shared slot the canvases fill on the first frame.
    struct Showcase {
        producer: Rc<RefCell<Option<Producer>>>,
        /// Whether a frame has already been requested to caption the device the producer landed on.
        /// The captions are read during `render`, which runs before the first paint builds it.
        captioned: bool,
    }

    impl Showcase {
        fn new() -> Self {
            Self {
                producer: Rc::new(RefCell::new(None)),
                captioned: false,
            }
        }
    }

    impl Render for Showcase {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let origin = self
                .producer
                .borrow()
                .as_ref()
                .map(|producer| producer.origin);
            if origin.is_none() && !self.captioned {
                // The producer is built during the paint that follows this render; ask for one more
                // frame so the header can name the device it landed on.
                self.captioned = true;
                cx.notify();
            }
            let device_line = match origin {
                Some(origin) => format!("device: {origin}"),
                None => "device: negotiating with the window's renderer…".to_owned(),
            };

            let tiles = tiles(self.producer.clone());
            div()
                .size_full()
                .bg(rgb(0x101014))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_8()
                .p_8()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_2xl()
                                .text_color(rgb(0xffffff))
                                .child("External dma-buf surfaces"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x9a9aa2))
                                .text_center()
                                .max_w(px(560.))
                                .child(
                                    "gpui_interop · Linux · a producer's Vulkan image, exported as a \
                                     dma-buf and sampled by the window's renderer — no CPU copy",
                                ),
                        )
                        .child(div().text_sm().text_color(rgb(0x6f8fbf)).child(device_line)),
                )
                .child(div().flex().flex_wrap().justify_center().gap_6().children(tiles))
        }
    }

    /// The two tiles, one per tiling the hatch can ask for.
    fn tiles(slot: Rc<RefCell<Option<Producer>>>) -> Vec<AnyElement> {
        vec![
            tile(
                slot.clone(),
                |producer| producer.linear.handle(),
                "Linear · a plain buffer, always importable",
            ),
            tile(
                slot,
                |producer| producer.tiled.handle(),
                "Tiled · a driver modifier the renderer imports",
            ),
        ]
    }

    /// One tile: a `gpu_canvas` that builds the producer on the first paint (if it is not built
    /// already) and composites the buffer `pick` selects.
    fn tile(
        slot: Rc<RefCell<Option<Producer>>>,
        pick: fn(&Producer) -> DmaBufHandle,
        caption: &'static str,
    ) -> AnyElement {
        panel(
            gpu_canvas(move |gpu| {
                let mut slot = slot.borrow_mut();
                if slot.is_none() {
                    match Producer::build(gpu) {
                        Ok(producer) => *slot = Some(producer),
                        Err(error) => {
                            log::error!("interop_linux: cannot produce a dma-buf: {error:#}");
                            return;
                        }
                    }
                }
                let handle = pick(slot.as_ref().expect("the producer was just built"));
                gpu.paint_surface(handle);
            })
            // Shown when no surface arrives: a dark box under the caption, rather than nothing.
            .bg(rgb(0x23232c))
            .size_full(),
            caption,
        )
    }

    /// The producer's half: a dma-buf-capable Vulkan device and the buffers it cleared on it.
    ///
    /// Field order is load-bearing. A `SharedBuffer` frees its memory through the device it was made
    /// on, so the device must be destroyed **last**: declaring it after the buffers lets Rust drop them
    /// first, which is the order Vulkan needs.
    struct Producer {
        /// Where the device came from, for the header: the renderer's GPU, or one the producer
        /// opened itself.
        origin: &'static str,
        /// A linear buffer and a driver-tiled one, both on that device.
        linear: SharedBuffer,
        tiled: SharedBuffer,
        /// The device the buffers were made on; dropped after them (see `SharedBuffer`'s own note).
        _device: GpuDevice,
    }

    impl Producer {
        fn build(gpu: &mut GpuCanvasContext) -> Result<Self> {
            // The crate's whole job in one call: attach to the device the canvas lends, and ask for
            // a dma-buf-capable device on the GPU the window draws through.
            let attached = attach(gpu.try_device::<WgpuRenderer>())
                .ok()
                .and_then(|interop| interop.adapter().gpu_device());

            // With no renderer to match — or a renderer that is not wgpu — the producer opens a
            // device of its own. It works only when that device is on the GPU the window draws on.
            let (device, origin) = match attached {
                Some(device) => (device, "the renderer's GPU, via Adapter::gpu_device()"),
                None => (
                    GpuDevice::open().context("opening a device of the producer's own")?,
                    "a device the producer opened itself",
                ),
            };

            let linear = cleared(&device, Tiling::Linear, LINEAR_COLOUR)?;
            let tiled = cleared(&device, Tiling::Tiled, TILED_COLOUR)?;
            Ok(Self {
                origin,
                linear,
                tiled,
                _device: device,
            })
        }
    }

    /// Allocate a shareable buffer in `tiling` and clear it to `colour`, ready to hand over.
    ///
    /// `clear` waits for the clear on the CPU, so the buffer is ready when this returns; a producer
    /// with real work signals a `Fence` from its own submission instead.
    fn cleared(device: &GpuDevice, tiling: Tiling, colour: [f32; 4]) -> Result<SharedBuffer> {
        let buffer = SharedBuffer::new(device, TILE, TILE, SurfaceFormatKind::bgra8(), tiling)
            .context("allocating a shareable buffer")?;
        buffer
            .clear(device, colour)
            .context("clearing the buffer")?;
        Ok(buffer)
    }

    /// A square tile with a caption under it, so each tiling is labelled where it is shown.
    fn panel(content: impl IntoElement, caption: &str) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(240.))
            .child(div().w(px(TILE as f32)).h(px(TILE as f32)).child(content))
            .child(
                div()
                    .text_color(rgb(0xd8d8d8))
                    .text_sm()
                    .text_center()
                    .child(caption.to_owned()),
            )
            .into_any_element()
    }
}
