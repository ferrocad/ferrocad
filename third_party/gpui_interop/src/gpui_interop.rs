//! `gpui-interop`: composite pixels produced outside GPUI's renderer into a GPUI scene.
//!
//! A producer whose graphics device is **not** the window renderer's — another API, another adapter,
//! another process — renders a frame and hands it to the scene through the `surface()` element,
//! without a CPU copy of the pixels.
//!
//! The crate also covers the case where the window renderer *is* wgpu: [`Adapter::wgpu`] hands the
//! producer that renderer's own device and queue, and [`WgpuSurface`] is a render target the same
//! device composites with `paint_texture`. That is the no-export, no-fence path a wgpu viewport
//! wants. Either way, the pixels land in the scene without a round trip through the CPU.
//!
//! # The shape of the bridge
//!
//! [`attach`] takes the device a `gpu_canvas` callback lends
//! (`GpuCanvasContext::device`) and yields an [`Interop`], or [`Unavailable`] when the
//! canvas lent no device — an offscreen or foreign renderer. It is called inside the canvas, not
//! before a frame is painted: the window's renderer, and so its device, does not exist until then.
//! From there:
//!
//! - [`Interop::adapter`] returns an [`Adapter`], the producer's view of the window's device.
//! - With the `wgpu` feature, `Adapter::wgpu` returns a wgpu `Device` and `Queue` the producer can
//!   render on. When the window renderer *is* wgpu — the Linux default, or a Windows window whose
//!   factory installed `WgpuRenderer` — it is the renderer's own device and queue, cloned: a
//!   [`WgpuSurface`] rendered there is composited by the same device with
//!   `GpuCanvasContext::paint_texture`, which is the no-copy path a wgpu viewport wants. On Windows
//!   with the Direct3D 11 renderer it matches the adapter by LUID and makes a separate DX12 device,
//!   and the frame then leaves through the native [`SharedSurface`] transport.
//! - On Linux, `WgpuSharedBuffer` is the wgpu producer's **native** arm: it allocates an exportable
//!   image on the wgpu device's own `VkDevice`, wraps it back into wgpu so the producer renders with
//!   the ordinary API, and exports the frame as a dma-buf with its modifier, plane layout and a
//!   `sync_file` fence. `GpuCanvasContext::paint_surface` imports that handle exactly as it does a
//!   decoder's, which is the shape a renderer of another backend consumes.
//! - On Linux, [`SharedBuffer`] is the producer half of the dma-buf transport for a producer that
//!   owns its device: a Vulkan pipeline renders into a shareable image and exports it as a dma-buf,
//!   and GPUI imports it under its modifier. Single- or two-plane, linear or on a driver-tiled
//!   modifier the renderer can import, and ordered by a [`Fence`] that exports the acquire
//!   `sync_file`. It covers both sharing contracts: [`SharedBuffer::new`] lets the driver choose the
//!   image's layout and reports it back, while [`SharedBuffer::export_at`] pushes bytes at a plane
//!   layout the producer declares — the shape a decoder or capture pipeline whose layout comes from
//!   elsewhere needs. A wgpu producer does **not** use it: it renders into a `WgpuSharedBuffer` (or,
//!   same-device, a `WgpuSurface`).
//! - On Windows, [`SharedSurface`] / [`Fence`] are the producer half of the Direct3D transport: a
//!   Direct3D 12 producer renders into a shared texture and signals a fence, and hands GPUI the
//!   `DirectXSource::Shared` handles — which the renderer opens on its own Direct3D 11 device, with
//!   the shared fence ordering the two queues.
//!
//! # Lifetime
//!
//! A hand-off is a pipeline, not a copy: the consumer samples the buffer after `paint_surface`
//! returns. `WgpuSharedBuffer::submit` submits the write and returns immediately, so a producer that
//! renders continuously must keep a ring and wait for `DmaBufHandle::release` before writing a slot
//! again — or present one frame behind the one being rendered. A fixed "reuse after N submissions"
//! rule is a heuristic and eventually races a deep consumer queue. GPUI also drops a surface whose
//! acquire fence has not signalled within 16 ms, so a frame that runs long is safer presented one
//! frame late than as a black frame.
//!
//! `WgpuSharedBufferRing` (Linux, `wgpu` feature) implements that shape: it owns the slots and
//! release descriptors and only presents a frame whose fence has already signalled, repeating the
//! last good frame while a new one is still running.
//!
//! # The other direction
//!
//! With the `guest` feature, [`Guest`] runs the inverse: headless GPUI on a worker thread for a host
//! loop that owns the window and its cadence. Frames leave as shareable surfaces, so the host samples
//! them on the same device; work and input enter through [`Guest::update`]. Only the frame handle and
//! plain commands cross the thread; the `!Send` renderer stays pinned to the guest.
//!
//! # Sketch
//!
//! ```rust
//! use gpui_interop::{attach, Unavailable, WgpuSurface};
//!
//! fn bridge(gpu: &mut gpui::GpuCanvasContext) -> Result<(), Unavailable> {
//!     // The window renderer, where it is wgpu (the Linux default).
//!     let interop = attach(gpu.try_device::<gpui_wgpu::WgpuRenderer>())?;
//!
//!     // With the `wgpu` feature: the renderer's own device and queue, cloned.
//!     let (device, queue) = interop.adapter().wgpu().expect("the renderer's own device");
//!
//!     // A surface the ordinary `wgpu` API renders into; `paint_texture` accepts its view on the
//!     // same device.
//!     let surface = WgpuSurface::new(&device, 640, 480).expect("a render target");
//!     let _ = (queue, surface);
//!     Ok(())
//! }
//! ```
//!
//! The `wgpu` line requires the `wgpu` feature; without it, `Adapter::wgpu` and [`WgpuSurface`] are
//! not compiled in. The Linux `SharedBuffer`/`Fence` transport and the Windows handles remain
//! available without it, because neither is a `wgpu` client.
//!
//! On Windows, the same snippet is the path when the window's factory installed `WgpuRenderer`.
//! Under the default Direct3D 11 renderer `Adapter::wgpu` makes a separate DX12 device, and the
//! hand-off is [`SharedSurface`] instead — see the module documentation of `windows`.
//!
//! # Provisional
//!
//! The crate is provisional and deliberately incomplete, and its API may change. Only the platform
//! backends that are implemented today are exposed; the rest arrive as they are written.

mod adapter;
#[cfg(feature = "guest")]
mod guest;
#[cfg(feature = "gpui")]
mod input;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(target_os = "linux", feature = "wgpu"))]
mod wgpu_dmabuf;
#[cfg(all(target_os = "linux", feature = "wgpu"))]
mod wgpu_dmabuf_ring;
#[cfg(feature = "wgpu")]
mod wgpu_surface;
#[cfg(target_os = "windows")]
mod windows;

pub use adapter::{Adapter, Unavailable};
#[cfg(feature = "guest")]
pub use guest::{Guest, GuestFrame, GuestOptions};
#[cfg(feature = "gpui")]
pub use input::{
    SurfaceButton, SurfaceEvent, SurfaceInput, SurfaceModifiers, SurfacePoint, SurfaceScroll,
    surface_input,
};
#[cfg(target_os = "linux")]
pub use linux::{
    Fence, GpuDevice, GpuId, LinearLayout, PlaneLayout, SharedBuffer, Tiling, gpu_id, remember,
};
#[cfg(all(target_os = "linux", feature = "wgpu"))]
pub use wgpu_dmabuf::WgpuSharedBuffer;
#[cfg(all(target_os = "linux", feature = "wgpu"))]
pub use wgpu_dmabuf_ring::{RingFrame, WgpuSharedBufferRing};
#[cfg(feature = "wgpu")]
pub use wgpu_surface::WgpuSurface;
#[cfg(target_os = "windows")]
pub use windows::{Fence, SharedSurface};

use std::any::Any;
use std::rc::Rc;

/// Negotiate with the window renderer a `gpu_canvas` callback reaches.
///
/// Called inside the canvas, with the device the callback lends — `gpu.device::<R>()`, or
/// `gpu.try_device::<R>()` for the fallible form. There is nothing to attach to before a frame is
/// painted, because the window's renderer, and its device, do not exist until then.
///
/// Returns [`Unavailable::NoDevice`] when the canvas lent no device — an offscreen or foreign
/// renderer — so this is a recoverable error rather than a panic.
pub fn attach<D: 'static>(device: Option<D>) -> Result<Interop, Unavailable> {
    let device = device.ok_or(Unavailable::NoDevice)?;
    Ok(Interop {
        device: Rc::new(device),
    })
}

/// A window's renderer, as a producer sees it.
pub struct Interop {
    /// The window renderer's device, erased. The per-platform modules downcast it to the one type
    /// their transport needs — an `ID3D11Device`, a `(wgpu::Device, wgpu::Queue)` — which is what
    /// keeps the facade free of the platform graphics crates.
    device: Rc<dyn Any>,
}

impl Interop {
    /// The producer-side device, matched to the window's renderer.
    pub fn adapter(&self) -> Adapter<'_> {
        Adapter::new(&self.device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unavailable_reasons_are_distinct() {
        assert_eq!(Unavailable::NoDevice, Unavailable::NoDevice);
        assert_ne!(Unavailable::NoAdapter, Unavailable::Unsupported);
    }
}
