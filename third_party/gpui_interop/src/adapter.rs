//! Matching a foreign device to the window's renderer.
//!
//! Cross-device sharing needs the **same physical adapter**, and that is not assertable from inside
//! either device — a Direct3D 11 device and a wgpu adapter report their identity in different terms.
//! [`Adapter`] is the producer's view of the window's device; [`Adapter::wgpu`] returns a wgpu device
//! and queue only when one matches.
//!
//! There are two mechanisms, and the first is not a match at all:
//!
//! - **The renderer already is wgpu.** A Linux window (and a Windows window whose factory installed
//!   `WgpuRenderer`) lends `(wgpu::Device, wgpu::Queue)` through `GpuCanvasContext::device`, and that
//!   *is* the device the producer must render on. [`Adapter::wgpu`] returns it unchanged — same
//!   logical device, same queue — so a wgpu producer renders exactly where GPUI samples. This is the
//!   same-device path, and [`WgpuSurface`](crate::WgpuSurface) is the surface it renders into.
//! - **The renderer is Direct3D 11.** On Windows the window's `ID3D11Device` names its adapter by
//!   LUID, and the same LUID identifies the wgpu DX12 adapter, so a match can be found without the
//!   caller handing over its device. The LUID — not the adapter name — is the identity, because DXGI
//!   and wgpu enumerate a *different number* of `Microsoft Basic Render Driver` entries, so the names
//!   collide where the LUIDs do not. [`Adapter::wgpu`] then requests a **separate** DX12 device on
//!   that adapter; the frame leaves through the Direct3D transport ([`SharedSurface`](crate::SharedSurface)),
//!   not as a bare wgpu texture.
//!
//! When neither applies [`Adapter::wgpu`] answers `None`. macOS has no wgpu renderer to lend, and its
//! renderer is Metal, which [`Adapter::wgpu`] does not match. On Linux the renderer also names its
//! `vk::PhysicalDevice`, and [`Adapter::gpu_device`] is the separate door a **Vulkan** exporter uses
//! — a second dma-buf-capable logical device on that GPU. A wgpu producer does not want it: it wants
//! [`Adapter::wgpu`], which is the renderer's own device.

use std::any::Any;
use std::rc::Rc;
#[cfg(feature = "wgpu")]
use std::sync::Arc;

/// Why a window cannot be bridged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// The renderer lends no device — an offscreen or foreign renderer.
    NoDevice,
    /// No wgpu adapter matches the window's device.
    NoAdapter,
    /// This platform's module is not built.
    Unsupported,
}

/// The producer-side device, matched to the window's renderer.
pub struct Adapter<'a> {
    #[allow(
        dead_code,
        reason = "held for the platform modules; without the `wgpu` feature none reads it"
    )]
    device: &'a Rc<dyn Any>,
}

impl<'a> Adapter<'a> {
    pub(crate) fn new(device: &'a Rc<dyn Any>) -> Self {
        Self { device }
    }

    /// A wgpu device and queue the producer can render on: the window renderer's own when it is
    /// wgpu, or a matching one it made on the renderer's adapter.
    ///
    /// - When the window renderer *is* wgpu — the Linux default, or a Windows window whose factory
    ///   installed `WgpuRenderer` — this is exactly the renderer's device and queue, cloned, not a
    ///   second device. Render into a [`WgpuSurface`](crate::WgpuSurface) there and the canvas
    ///   composites it with `GpuCanvasContext::paint_texture`: same device, no copy, no export.
    /// - On Windows with the Direct3D 11 renderer, this reads the device's adapter LUID and requests
    ///   a separate DX12 device and queue on that adapter; the hand-off is then the Direct3D one
    ///   ([`SharedSurface`](crate::SharedSurface)), not a bare wgpu texture.
    /// - `None` when the renderer is neither wgpu nor Direct3D 11, or no adapter matches.
    #[cfg(feature = "wgpu")]
    pub fn wgpu(&self) -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        // The renderer that is itself wgpu lends the one device a same-device producer may use. No
        // match is needed -- the Windows LUID path below would make a *second* DX12 device, which a
        // sampled texture cannot cross back from.
        if let Some((device, queue)) = self
            .device
            .downcast_ref::<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>()
        {
            return Some((Arc::clone(device), Arc::clone(queue)));
        }

        #[cfg(target_os = "windows")]
        {
            matched::wgpu_device(self.device)
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    /// A dma-buf-capable device on the window's GPU, for a producer that renders and exports.
    ///
    /// Linux's answer to the Windows LUID match: the renderer's device names its physical device,
    /// and this makes a *second* logical device on it — so a producer's queue never races the
    /// renderer's — carrying the extensions the export needs.
    #[cfg(all(target_os = "linux", feature = "wgpu"))]
    pub fn gpu_device(&self) -> Option<crate::GpuDevice> {
        use std::sync::Arc;

        let (device, _queue) = self
            .device
            .downcast_ref::<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>()?;
        // SAFETY: the HAL device is borrowed only long enough to read the raw handles below.
        let hal = unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }?;
        crate::GpuDevice::on(
            hal.shared_instance().raw_instance(),
            hal.raw_physical_device(),
        )
        .ok()
    }

    /// The GPU the window draws on, for a producer that owns its device — a 3D engine, a decoder, a
    /// capture loop — and must land on the same GPU to share a dma-buf.
    ///
    /// Unlike [`gpu_device`](Self::gpu_device) it creates no device: it only names the renderer's
    /// GPU, as a [`GpuId`](crate::GpuId) a producer matches from its own instance. It also
    /// **remembers** the answer for [`gpui_interop::gpu_id`](crate::gpu_id), so a producer that runs
    /// off the paint can read it after a canvas has run once.
    #[cfg(all(target_os = "linux", feature = "wgpu"))]
    pub fn gpu_id(&self) -> Option<crate::GpuId> {
        use std::sync::Arc;

        let (device, _queue) = self
            .device
            .downcast_ref::<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>()?;
        // SAFETY: the HAL device is borrowed only long enough to read the raw handles below.
        let hal = unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }?;
        let id = crate::GpuId::of(
            hal.shared_instance().raw_instance(),
            hal.raw_physical_device(),
        );
        crate::remember(id.clone());
        Some(id)
    }
}

/// The Windows mechanism: the window's `ID3D11Device` → its adapter LUID → the wgpu DX12 adapter with
/// the same LUID.
#[cfg(all(target_os = "windows", feature = "wgpu"))]
mod matched {
    use std::any::Any;
    use std::future::Future;
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    use windows::Win32::Foundation::LUID;
    use windows::Win32::Graphics::Direct3D11::ID3D11Device;
    use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
    use windows::core::Interface;

    /// A wgpu device and queue on the adapter the window's Direct3D 11 device runs on, or `None`
    /// when the device lends no LUID or no DX12 adapter matches it.
    pub(super) fn wgpu_device(
        device: &Rc<dyn Any>,
    ) -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        let window_luid = device_adapter_luid(device.downcast_ref::<ID3D11Device>()?)?;

        let instance = wgpu::Instance::new(dx12_instance());
        let adapters = block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        let adapter = adapters.iter().find(|adapter| {
            wgpu_adapter_luid(adapter).is_some_and(|l| same_luid(l, window_luid))
        })?;

        let (device, queue) = block_on(adapter.request_device(&device_descriptor(adapter))).ok()?;
        Some((Arc::new(device), Arc::new(queue)))
    }

    /// The LUID of the adapter a Direct3D 11 device was created on: `IDXGIDevice::GetAdapter` then
    /// `IDXGIAdapter::GetDesc`.
    fn device_adapter_luid(device: &ID3D11Device) -> Option<LUID> {
        // SAFETY: the calls are the documented query path for a device's adapter; every value they
        // return is owned by this scope.
        unsafe {
            let dxgi_device: IDXGIDevice = device.cast().ok()?;
            let adapter: IDXGIAdapter = dxgi_device.GetAdapter().ok()?;
            Some(adapter.GetDesc().ok()?.AdapterLuid)
        }
    }

    /// The LUID of a wgpu DX12 adapter, read from the HAL adapter rather than the reported name.
    fn wgpu_adapter_luid(adapter: &wgpu::Adapter) -> Option<LUID> {
        // SAFETY: `raw_adapter` borrows the HAL adapter behind `as_hal`'s guard, which lives as long
        // as `adapter`, and `GetDesc` only reads.
        unsafe {
            let hal = adapter.as_hal::<wgpu::hal::dx12::Api>()?;
            Some(hal.raw_adapter().GetDesc().ok()?.AdapterLuid)
        }
    }

    /// Identity is the LUID, not the adapter name.
    fn same_luid(a: LUID, b: LUID) -> bool {
        a.LowPart == b.LowPart && a.HighPart == b.HighPart
    }

    /// A DX12-only instance: the window's renderer is Direct3D, so DX12 is the only backend that can
    /// share its adapter.
    fn dx12_instance() -> wgpu::InstanceDescriptor {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::DX12;
        descriptor
    }

    /// The producer's device, capped to the adapter's limits the way `gpui_wgpu` caps its own.
    fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
        wgpu::DeviceDescriptor {
            label: Some("gpui_interop_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults()
                .using_resolution(adapter.limits())
                .using_alignment(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }
    }

    /// `enumerate_adapters` is async but resolves without parking, so a no-op waker and a yield loop
    /// suffice — no async runtime is needed.
    fn block_on<F: Future>(future: F) -> F::Output {
        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}
