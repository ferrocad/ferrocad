//! `WgpuRenderer` as a window's `GpuRenderer` in the browser.
//!
//! The native implementation lives in `platform_renderer`, which is not compiled
//! for wasm, and it lends the device from the shared `WgpuContext` slot. The
//! browser renderer is built from a surface and deliberately has no such slot
//! (`WgpuRenderer::new_from_surface` passes `None`, and `gpu_context()` is
//! native-only). The device and the queue are in the renderer's core on every
//! target, so the browser reads them from there instead.
//!
//! This is what makes `GpuCanvasContext::try_device::<WgpuRenderer>()` answer in
//! the browser — the door `gpui_interop`'s same-device producer knocks on.

use std::sync::Arc;

use gpui_engine::GpuRenderer;

use crate::WgpuRenderer;

impl GpuRenderer for WgpuRenderer {
    /// The pair the native implementation returns, so a producer written against
    /// either target accepts it unchanged.
    type Device = (Arc<wgpu::Device>, Arc<wgpu::Queue>);

    /// `None` while the renderer is released or mid-recovery, like the native one.
    fn device(&self) -> Option<Self::Device> {
        let core = self.core()?;
        Some((
            Arc::clone(&core.resources.device),
            Arc::clone(&core.resources.queue),
        ))
    }
}
