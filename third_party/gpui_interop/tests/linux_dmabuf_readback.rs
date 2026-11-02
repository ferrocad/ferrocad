#![cfg(all(target_os = "linux", feature = "readback-test"))]
//! The hatch, end to end: the dma-bufs a [`SharedBuffer`] exports, composited and read back.
//!
//! The crate's unit tests (`src/linux.rs`) prove the hatch produces the *descriptor* it claims — an
//! fd, a modifier, per-plane offsets and strides. They cannot prove the thing that actually matters:
//! that a renderer can import that image and sample the bytes the producer wrote. This test does, by
//! compositing a linear and a driver-tiled RGB buffer and a linear two-plane `NV12` buffer side by
//! side in one headless window and reading the frame back.
//!
//! ```text
//! attach(gpu.try_device::<WgpuHeadlessRenderer>())  // the renderer's own device, as a producer sees
//!     .adapter().gpu_device()                       // it: a second dma-buf-capable device on that GPU
//! SharedBuffer::new(..).clear(..)                   // a shareable RGB image, written on the GPU
//! SharedBuffer::new(..).fill(..)                    // ...or a two-plane NV12 one, from host bytes
//! gpu.paint_surface(buffer.handle())                // handed to the scene
//! // ...the window composites each, and the pixel is read back and compared to the clear colour
//! ```
//!
//! The `NV12` arm is the one that exercises [`SharedBuffer::fill`]: `clear` writes a single RGB
//! colour, which a two-plane YCbCr image has no room for, and the shader converts the sample back
//! through the BT.601 matrix the planes carry the inverse of — so the arm compares against that
//! matrix's own output rather than a byte-for-byte clear.
//!
//! # Why the buffers are built through `attach`, not `GpuDevice::open`
//!
//! A dma-buf is importable on the GPU it was exported from, and a DRM modifier is meaningful only to
//! the vendor that defines it — the tiled one is picked from the *device's* own
//! `VkDrmFormatModifierProperties`. So the producer's device has to be the consumer's, or the two ends
//! disagree: `GpuDevice::open` picks its own GPU and a test built on it silently exports an Intel
//! `Y_TILED` buffer for an NVIDIA renderer to import. [`attach`] and
//! [`Adapter::gpu_device`](gpui_interop::Adapter::gpu_device) name the renderer's own
//! `vk::PhysicalDevice`, so the modifier is chosen against the GPU that will sample it — which is also
//! exactly the adapter step the crate exists for.
//!
//! # Why it is opt-in
//!
//! Behind the `readback-test` feature for the same reason the core's own dma-buf test is behind
//! `test-support`: driving a window pulls in the facade's platform bundle, and the test needs a real
//! Vulkan device. A machine without one skips rather than fails, but building it at all needs the
//! windowing stack's system libraries, which the crate's ordinary `cargo test` deliberately does not:
//!
//! ```sh
//! cargo test -p gpui_interop --features readback-test --test linux_dmabuf_readback
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyWindowHandle, AppContext as _, Context, GpuCanvasContext, IntoElement, Render,
    SurfaceFormatKind, Window, div, gpu_canvas, prelude::*, px, size,
};
use gpui_interop::{GpuDevice, SharedBuffer, Tiling, attach};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};

/// The edge of each produced surface, in pixels; the window holds three of them side by side.
const EDGE: u32 = 64;

/// The colour the RGB buffers are cleared to, as the RGBA `clear` takes. `Rgba8`, so the bytes stored
/// are these four in order; a round trip that scrambles a channel comes back a different colour.
const CLEAR: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 0.0, 1.0];

/// [`CLEAR`] in bytes — what a read-back of an RGB buffer must return.
const EXPECTED: [u8; 4] = [128, 128, 0, 255];

/// Which of the producer's three buffers a tile paints.
#[derive(Clone, Copy)]
enum Which {
    Linear,
    Tiled,
    Nv12,
}

/// The producer's half: a dma-buf-capable device on the renderer's GPU and the three buffers it wrote.
///
/// Declaration order is load-bearing: a `SharedBuffer` frees its memory through the device it was made
/// on, so the device is declared *last* and Rust drops it after them.
struct Producer {
    linear: SharedBuffer,
    tiled: SharedBuffer,
    nv12: SharedBuffer,
    _device: GpuDevice,
}

impl Producer {
    /// Build on the device the canvas lends — the renderer's own GPU — or `None` when it lends none
    /// or cannot export a dma-buf there.
    fn build(gpu: &mut GpuCanvasContext) -> Option<Self> {
        let interop = attach(gpu.try_device::<WgpuHeadlessRenderer>()).ok()?;
        // Reading the GPU also remembers it process-wide, for a producer that runs off the paint.
        interop.adapter().gpu_id();
        let device = interop.adapter().gpu_device()?;
        let linear = cleared(&device, Tiling::Linear)?;
        let tiled = cleared(&device, Tiling::Tiled)?;
        let nv12 = nv12_filled(&device)?;
        Some(Self {
            linear,
            tiled,
            nv12,
            _device: device,
        })
    }
}

/// A shareable RGB buffer in `tiling`, cleared to [`CLEAR`]. `None` when the device cannot produce one.
fn cleared(device: &GpuDevice, tiling: Tiling) -> Option<SharedBuffer> {
    let buffer = SharedBuffer::new(device, EDGE, EDGE, SurfaceFormatKind::rgba8(), tiling).ok()?;
    buffer.clear(device, CLEAR).ok()?;
    Some(buffer)
}

/// A shareable linear `NV12` buffer whose two planes carry the bytes the shader's BT.601 matrix maps
/// back to [`CLEAR`] — the two-plane arm `clear` cannot write.
fn nv12_filled(device: &GpuDevice) -> Option<SharedBuffer> {
    let (y, cb, cr) = nv12_from_rgb(EXPECTED);
    let luma = vec![y; (EDGE * EDGE) as usize];
    let mut chroma = vec![0u8; (EDGE / 2 * EDGE / 2 * 2) as usize];
    for pair in chroma.as_chunks_mut::<2>().0 {
        pair[0] = cb;
        pair[1] = cr;
    }
    let buffer = SharedBuffer::new(
        device,
        EDGE,
        EDGE,
        SurfaceFormatKind::nv12(),
        Tiling::Linear,
    )
    .ok()?;
    buffer.fill(device, &[&luma, &chroma]).ok()?;
    Some(buffer)
}

/// The window's view: three tiles, one per buffer. The producer is built at paint time — the
/// renderer's device exists only then — so the slot is shared and filled on the first paint.
struct View {
    producer: Rc<RefCell<Option<Producer>>>,
    /// Whether the bridge built at all, so the test can tell a skip from a wrong pixel.
    built: Rc<Cell<bool>>,
}

impl Render for View {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(tile(
                self.producer.clone(),
                self.built.clone(),
                Which::Linear,
            ))
            .child(tile(
                self.producer.clone(),
                self.built.clone(),
                Which::Tiled,
            ))
            .child(tile(self.producer.clone(), self.built.clone(), Which::Nv12))
    }
}

/// One tile: build the producer on the first paint, then composite the buffer `which` names.
fn tile(
    slot: Rc<RefCell<Option<Producer>>>,
    built: Rc<Cell<bool>>,
    which: Which,
) -> impl IntoElement {
    gpu_canvas(move |gpu| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match Producer::build(gpu) {
                Some(producer) => {
                    *slot = Some(producer);
                    built.set(true);
                }
                None => return,
            }
        }
        let handle = match which {
            Which::Linear => slot.as_ref().expect("just built").linear.handle(),
            Which::Tiled => slot.as_ref().expect("just built").tiled.handle(),
            Which::Nv12 => slot.as_ref().expect("just built").nv12.handle(),
        };
        gpu.paint_surface(handle);
    })
    .w(px(EDGE as f32))
    .h(px(EDGE as f32))
}

/// Two linear RGB buffers and a two-plane `NV12` one, each exported as a dma-buf on the renderer's own
/// GPU, composited into one headless window, and read back.
#[test]
fn an_exported_buffer_composites_byte_for_byte() {
    if !gpui::current_headless_renderer().is_ok_and(|renderer| renderer.is_some()) {
        eprintln!("skipping: no headless renderer (no Vulkan adapter)");
        return;
    }

    let text_system = Arc::new(CosmicTextSystem::new("fallback"));
    let mut cx = gpui::HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        gpui::current_headless_renderer()
    });

    let built = Rc::new(Cell::new(false));
    let window = cx
        .open_window(
            size(px((EDGE * 3) as f32), px(EDGE as f32)),
            |_window, cx| {
                cx.new(|_| View {
                    producer: Rc::new(RefCell::new(None)),
                    built: built.clone(),
                })
            },
        )
        .expect("a window to composite into");
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .expect("draw a frame");

    if !built.get() {
        eprintln!("skipping: the renderer's device lent none that exports a dma-buf");
        return;
    }

    // A canvas has run, so the renderer's GPU is remembered for a producer off the paint.
    assert!(
        gpui_interop::gpu_id().is_some(),
        "reading the renderer's GPU with Adapter::gpu_id should remember it"
    );

    let image = cx.capture_screenshot(window).expect("read the frame back");
    // The left tile is the linear buffer and the middle the tiled one; both were cleared to CLEAR, so
    // both read back exactly.
    for (label, x) in [("linear", EDGE / 2), ("tiled", EDGE + EDGE / 2)] {
        assert_eq!(
            image.get_pixel(x, EDGE / 2).0,
            EXPECTED,
            "the {label} buffer did not round trip"
        );
    }

    // The right tile is the NV12 buffer: the bytes its planes carry are the inverse of the shader's
    // BT.601 matrix, so the composed pixel returns to CLEAR within rounding.
    let (y, cb, cr) = nv12_from_rgb(EXPECTED);
    let expected = ycbcr_to_rgb(
        f32::from(y) / 255.0,
        f32::from(cb) / 255.0,
        f32::from(cr) / 255.0,
    );
    let pixel = image.get_pixel(EDGE * 2 + EDGE / 2, EDGE / 2).0;
    for channel in 0..3 {
        let want = (expected[channel] * 255.0).round() as i32;
        let got = i32::from(pixel[channel]);
        assert!(
            (got - want).abs() <= 2,
            "the nv12 buffer's channel {channel} is {got}, but the shader's matrix predicts {want}",
        );
    }
    assert_eq!(pixel[3], 255, "the nv12 surface should be opaque");
}

/// The `fs_surface` matrix, on the CPU: `ycbcr_to_RGB * vec4(y, cb, cr, 1)` — the same conversion the
/// core's `surface_dmabuf` test predicts the composed pixel with.
fn ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> [f32; 4] {
    let columns = [
        [1.0, 1.0, 1.0],
        [0.0, -0.3441, 1.7720],
        [1.4020, -0.7141, 0.0],
        [-0.7010, 0.5291, -0.8860],
    ];
    let mut rgb = [0.0f32; 4];
    for channel in 0..3 {
        rgb[channel] = (columns[0][channel] * y
            + columns[1][channel] * cb
            + columns[2][channel] * cr
            + columns[3][channel])
            .clamp(0.0, 1.0);
    }
    rgb[3] = 1.0;
    rgb
}

/// The Y, Cb and Cr bytes the renderer's BT.601 conversion maps back to `rgb` — the inverse of
/// [`ycbcr_to_rgb`], so the two-plane buffer can carry the shared fixture colour.
fn nv12_from_rgb(rgb: [u8; 4]) -> (u8, u8, u8) {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    let (r, g, b) = (r + 0.7010, g - 0.5291, b + 0.8860);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = -0.168736 * r - 0.331264 * g + 0.5 * b;
    let cr = 0.5 * r - 0.418688 * g - 0.081312 * b;
    (
        (y.clamp(0.0, 1.0) * 255.0).round() as u8,
        (cb.clamp(0.0, 1.0) * 255.0).round() as u8,
        (cr.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}
