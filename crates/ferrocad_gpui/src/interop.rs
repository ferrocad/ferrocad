//! Zero-copy viewport presentation through gpui's window renderer.
//!
//! The viewport renders into a GPU target on the **window's own wgpu device** and
//! hands the frame to gpui without a CPU copy:
//!
//! - Linux: a `WgpuSharedBuffer` exports the frame as a dma-buf and
//!   `paint_surface` imports it (the same-adapter, cross-device shape). Where the
//!   device cannot export (a GL adapter, or a driver without the Vulkan external
//!   memory extensions), it falls back to a `WgpuSurface` sampled with
//!   `paint_texture` on the same device.
//! - Windows/FreeBSD: the same-device `WgpuSurface` path, which requires the window
//!   itself to be running gpui's wgpu renderer.
//!
//! Either way the renderer runs on `Adapter::wgpu()`: it must be the window
//! renderer's device, because a wgpu texture cannot be exported and a texture from
//! another device cannot be sampled by gpui.
//!
//! This module is compiled only where gpui's wgpu renderer is available (not macOS
//! or wasm). Everywhere else [`zero_copy_presenter`](crate::zero_copy_presenter)
//! returns `None` and the viewport keeps its readback image path.

use std::sync::Arc;

use ferrocad_inventor::{Camera, Hit, RenderQueue, ViewSpec};
use ferrocad_viewport::{Presenter, PresenterStatus};
use ferrocad_wgpu::{GpuContext, WgpuRenderer as FerrocadRenderer};
use gpui::{Corners, GpuCanvasContext};
#[cfg(target_os = "linux")]
use gpui_interop::WgpuSharedBuffer;
use gpui_interop::{WgpuSurface, attach};
use gpui_wgpu::wgpu;
use gpui_wgpu::{ImportedTextureExt as _, WgpuRenderer as WindowRenderer};

/// The viewport target's sRGB format, matching `ferrocad_wgpu`'s `TARGET_FORMAT`
/// and the imported-texture contract.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// MSAA count for the scene target. Matches the image path's 4× default.
const SAMPLES: u32 = 4;

/// Build the host presenter. Installing it is always safe: if the window has no
/// compatible device, it reports [`PresenterStatus::Unsupported`] and the viewport
/// keeps the image path.
pub fn presenter() -> Box<dyn Presenter> {
    Box::new(GpuiInteropPresenter::new())
}

/// Where a frame goes once it has been rendered.
enum Output {
    /// The exported dma-buf shape (Linux): render into `view()`, submit through
    /// `WgpuSharedBuffer`, and hand gpui the fence-carrying handle.
    #[cfg(target_os = "linux")]
    Exported(Box<WgpuSharedBuffer>),
    /// The same-device shape: gpui samples the target's texture directly.
    SameDevice(WgpuSurface),
}

impl Output {
    fn view(&self) -> &wgpu::TextureView {
        match self {
            #[cfg(target_os = "linux")]
            Self::Exported(buffer) => buffer.view(),
            Self::SameDevice(surface) => surface.view(),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Exported(_) => "dma-buf + sync_file",
            Self::SameDevice(_) => "same-device view",
        }
    }
}

/// The host's zero-copy presenter: a renderer on the window's wgpu device plus the
/// surface it hands to gpui.
pub struct GpuiInteropPresenter {
    status: PresenterStatus,
    /// The element's physical size. It can arrive before the device does.
    size: [u32; 2],
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    renderer: Option<FerrocadRenderer>,
    output: Option<Output>,
    /// The frame encoded by `draw`, submitted by `present` (the dma-buf path needs
    /// the command buffer so it can register its export fence on the submission).
    commands: Option<wgpu::CommandBuffer>,
}

impl GpuiInteropPresenter {
    /// A presenter in the [`Pending`](PresenterStatus::Pending) state.
    pub fn new() -> Self {
        Self {
            status: PresenterStatus::Pending,
            size: [640, 480],
            device: None,
            queue: None,
            renderer: None,
            output: None,
            commands: None,
        }
    }

    /// Allocate the output target. Prefer the exported dma-buf on Linux; every
    /// other case, and every export failure, uses the same-device view.
    fn create_output(device: &wgpu::Device, size: [u32; 2]) -> Option<Output> {
        #[cfg(target_os = "linux")]
        {
            match WgpuSharedBuffer::new(device, size[0], size[1], FORMAT) {
                Ok(buffer) => return Some(Output::Exported(Box::new(buffer))),
                Err(error) => {
                    eprintln!(
                        "ferrocad: dma-buf viewport unavailable ({error:#}); \
                         using the same-device view"
                    );
                }
            }
        }
        WgpuSurface::new(device, size[0], size[1])
            .ok()
            .map(Output::SameDevice)
    }

    /// Reallocate the renderer's MSAA/depth targets and the output after a resize.
    fn rebuild(&mut self) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        // A command buffer encoded for the old target must not reach the new one.
        self.commands = None;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize(self.size);
        }
        match Self::create_output(device, self.size) {
            Some(output) => self.output = Some(output),
            None => {
                self.output = None;
                self.status = PresenterStatus::Unsupported;
            }
        }
    }
}

impl Default for GpuiInteropPresenter {
    fn default() -> Self {
        Self::new()
    }
}

impl Presenter for GpuiInteropPresenter {
    fn status(&self) -> PresenterStatus {
        self.status
    }

    fn set_size(&mut self, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if size == self.size {
            return;
        }
        self.size = size;
        if self.status == PresenterStatus::Ready {
            self.rebuild();
        }
    }

    fn prepare(&mut self, gpu: &mut GpuCanvasContext) {
        if self.status != PresenterStatus::Pending {
            return;
        }

        // The window's renderer, and so its device, exists only during paint.
        let Ok(interop) = attach(gpu.try_device::<WindowRenderer>()) else {
            self.status = PresenterStatus::Unsupported;
            return;
        };
        let Some((device, queue)) = interop.adapter().wgpu() else {
            self.status = PresenterStatus::Unsupported;
            return;
        };

        let size = self.size;
        let renderer = FerrocadRenderer::with_sample_count(
            GpuContext::new(device.as_ref().clone(), queue.as_ref().clone()),
            size,
            SAMPLES,
        );
        let Some(output) = Self::create_output(&device, size) else {
            self.status = PresenterStatus::Unsupported;
            return;
        };
        eprintln!("ferrocad: viewport presenting through {}", output.label());

        self.device = Some(device);
        self.queue = Some(queue);
        self.renderer = Some(renderer);
        self.output = Some(output);
        self.status = PresenterStatus::Ready;
    }

    fn draw(&mut self, views: &[ViewSpec<'_>]) -> bool {
        if self.status != PresenterStatus::Ready {
            return false;
        }
        let (Some(renderer), Some(output)) = (self.renderer.as_mut(), self.output.as_ref()) else {
            return false;
        };
        match renderer.encode_views(output.view(), views) {
            Ok(commands) => {
                self.commands = Some(commands);
                true
            }
            Err(error) => {
                eprintln!("ferrocad: zero-copy encode failed: {error:?}");
                self.status = PresenterStatus::Unsupported;
                false
            }
        }
    }

    fn present(&mut self, gpu: &mut GpuCanvasContext) {
        let Some(commands) = self.commands.take() else {
            return;
        };
        let (Some(queue), Some(output)) = (self.queue.as_ref(), self.output.as_mut()) else {
            return;
        };

        let mut failed = false;
        match output {
            #[cfg(target_os = "linux")]
            Output::Exported(buffer) => match buffer.submit(queue, Some(commands)) {
                Ok(handle) => gpu.paint_surface(handle),
                Err(error) => {
                    eprintln!("ferrocad: dma-buf submit failed: {error:#}");
                    failed = true;
                }
            },
            Output::SameDevice(surface) => {
                queue.submit(Some(commands));
                match surface.view().to_imported_handle() {
                    Ok(handle) => gpu.paint_texture(handle, Corners::default(), 1.0, false),
                    Err(error) => {
                        eprintln!("ferrocad: imported viewport texture failed: {error:#}");
                        failed = true;
                    }
                }
            }
        }
        if failed {
            self.status = PresenterStatus::Unsupported;
        }
    }

    fn pick_at(&mut self, queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
        if self.status != PresenterStatus::Ready {
            return None;
        }
        // The renderer already lives on the window's device, so this is the same
        // G-buffer pass the image path uses, just without a second device.
        self.renderer.as_mut()?.pick_at(queue, camera, device)
    }

    fn supports_pick(&self) -> bool {
        self.status == PresenterStatus::Ready && self.renderer.is_some()
    }
}
