//! A wgpu producer's exportable surface, handed to GPUI as a dma-buf.
//!
//! This is the cross-device shape the renderer's `surface()` path consumes: the producer renders
//! with the ordinary `wgpu` API into an **exportable** image, the image is exported as a dma-buf with
//! its DRM modifier and plane layout, and GPUI imports that native handle on its own device. Nothing
//! about the producer's `wgpu` texture crosses the boundary, so the same shape works for a window
//! renderer of any backend — only the native representation differs per platform.
//!
//! The image is allocated by this module rather than by wgpu, because a `wgpu` texture is allocated
//! without the external-memory flags an export needs and exposes no export API. The allocation is a
//! `VkImage` on the wgpu device's own `VkDevice`, with `VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT`
//! and a DRM modifier; it is then wrapped back into `wgpu` with `texture_from_raw` +
//! `create_texture_from_hal`, so the producer draws into it through the normal API. The same
//! allocation is what the exported fd names.
//!
//! # Requirements
//!
//! The wgpu device must be the window renderer's own (what `Adapter::wgpu` returns on Linux), or at
//! least a device on the **same physical GPU**, and its Vulkan device must have
//! `VK_EXT_image_drm_format_modifier`, `VK_KHR_external_memory_fd`,
//! `VK_EXT_external_memory_dma_buf` and `VK_KHR_external_semaphore_fd` enabled. `gpui_wgpu` enables
//! the modifier and semaphore extensions on the renderer's device where the driver supports them;
//! wgpu-hal enables the two external-memory extensions by default. A device created by a plain
//! `wgpu::Adapter::request_device` has the first one missing, and is refused here with an error
//! naming it rather than exporting a buffer the renderer cannot import.
//!
//! # Synchronisation
//!
//! [`WgpuSharedBuffer::submit`] registers an exportable binary semaphore on the queue submission
//! that renders the frame, then exports it as a `sync_file` — the same fence shape every dma-buf
//! producer hands over, and the one the renderer polls. The device needs
//! `VK_KHR_external_semaphore_fd`, which `gpui_wgpu` enables on the renderer's device where the
//! driver supports it; a device without it is refused at construction rather than handing over an
//! unsynchronised buffer.
//!
//! # Lifetime
//!
//! [`WgpuSharedBuffer::submit`] submits the frame and returns; it does not wait for it to finish.
//! The handle's acquire fence orders the consumer's sample *after* the write, but says nothing about
//! the next write. A producer that renders continuously must keep a ring and, before reusing a slot,
//! wait for the consumer's release ([`DmaBufHandle::release`], signalled by GPUI from the submission
//! that sampled the surface). Reusing a single buffer, or reusing one after a fixed number of
//! submissions, will eventually overwrite a frame the consumer is still sampling.
//!
//! GPUI polls the acquire fence for only 16 ms (`gpui_wgpu`'s `ACQUIRE_FENCE_TIMEOUT`); a frame that
//! takes longer to finish is dropped and presents as black. Presenting the previous completed frame
//! (one frame of lead) is the simple way to stay inside that budget.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use ash::vk;
use gpui_engine::{DmaBufHandle, SurfaceFormatKind};

use crate::linux::{ExportDevice, Tiling, allocate_export_image};

/// A wgpu-rendered image exported as a dma-buf the renderer imports as a surface.
///
/// The texture is single-mip, single-sample, tiled on a DRM modifier, and `COLOR_ATTACHMENT`-capable.
/// Holding this value keeps the image and its exported memory alive; it must outlive every use of a
/// handle from [`handle`](Self::handle) or [`submit`](Self::submit). A producer that draws the next
/// frame while the renderer samples the last one keeps a ring of these.
pub struct WgpuSharedBuffer {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    handle: DmaBufHandle,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    /// The raw device the semaphore was made on, kept to destroy it.
    device: ash::Device,
    /// The binary semaphore the next submission signals and exports as the acquire fence.
    semaphore: vk::Semaphore,
    external_semaphore_fd: ash::khr::external_semaphore_fd::Device,
}

impl WgpuSharedBuffer {
    /// Allocate an exportable surface of `width` × `height` in `format`.
    ///
    /// `format` must be one GPUI samples: `Rgba8UnormSrgb` or `Bgra8UnormSrgb`. The image is the
    /// matching Vulkan `_SRGB` form, so a shader writes linear values and the hardware stores
    /// display-encoded bytes — the same contract `WgpuSurface` has on the same-device path.
    ///
    /// The image is always tiled: a linear image cannot carry `COLOR_ATTACHMENT` on the devices this
    /// path supports, so there is no linear fallback. A device with no importable modifier, no
    /// export extensions, or no external-semaphore fence is an error naming the shortfall.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!(
                "an exported wgpu surface must have a non-zero extent, but {width}x{height} was asked for"
            );
        }
        let (surface_format, vk_format) = formats(format)?;

        // SAFETY: the HAL device is borrowed only for this call; every raw handle it reads is used
        // while the guard is alive, and the image and memory outlive it in the wgpu texture.
        let hal_guard = unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }
            .ok_or_else(|| anyhow!("an exported wgpu surface needs a Vulkan wgpu device"))?;
        let hal: &wgpu::hal::vulkan::Device = &hal_guard;
        ensure_extensions(hal)?;

        let instance = hal.shared_instance().raw_instance();
        let raw_device = hal.raw_device();
        let external_memory_fd = ash::khr::external_memory_fd::Device::new(instance, raw_device);
        let drm_format_modifier =
            ash::ext::image_drm_format_modifier::Device::new(instance, raw_device);

        // A binary semaphore, exported by handle type `SYNC_FD`: the submission that renders the
        // frame signals it, `submit` exports the signal as the acquire fence, and the export
        // consumes it so the next submission can signal it again.
        let mut export_semaphore = vk::ExportSemaphoreCreateInfo::default()
            .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        // SAFETY: `export_semaphore` outlives the call; the semaphore is destroyed when this value
        // drops, or on the error path below.
        let semaphore = match unsafe {
            raw_device.create_semaphore(
                &vk::SemaphoreCreateInfo::default().push_next(&mut export_semaphore),
                None,
            )
        } {
            Ok(semaphore) => semaphore,
            Err(error) => return Err(anyhow!("create the acquire semaphore: {error:?}")),
        };

        let export_device = WgpuExportDevice {
            hal,
            external_memory_fd,
            drm_format_modifier,
        };
        let (image, memory, handle) = match allocate_export_image(
            &export_device,
            width,
            height,
            surface_format,
            vk_format,
            Tiling::Tiled,
            true,
        ) {
            Ok(allocated) => allocated,
            Err(error) => {
                // SAFETY: the semaphore was made above and is destroyed once.
                unsafe { raw_device.destroy_semaphore(semaphore, None) };
                return Err(error);
            }
        };

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let hal_descriptor = wgpu::hal::TextureDescriptor {
            label: Some("gpui_interop_wgpu_shared_buffer"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUses::COLOR_TARGET,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        // The wgpu texture owns the raw image and memory: `destroy_texture` does not free either
        // when a drop callback is supplied, so this is where they are released — after wgpu has
        // destroyed its own view of the texture, and in the order bind/allocate made them.
        let owner = raw_device.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || {
            // SAFETY: `image` and `memory` were created by this device and are freed exactly once,
            // by the callback the texture's drop invokes.
            unsafe {
                owner.destroy_image(image, None);
                owner.free_memory(memory, None);
            }
        });
        // SAFETY: `image` was created with `hal_descriptor`'s extent, format, usage and tiling on
        // this device; the memory is bound; `TextureMemory::External` says wgpu does not own the
        // memory, and the callback owns the image and memory.
        let hal_texture = unsafe {
            hal.texture_from_raw(
                image,
                &hal_descriptor,
                Some(drop_callback),
                wgpu::hal::vulkan::TextureMemory::External,
            )
        };
        // SAFETY: `hal_texture` came from this device's HAL, and the wgpu descriptor matches the one
        // the HAL texture was made against.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::vulkan::Api>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("gpui_interop_wgpu_shared_buffer"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("gpui_interop_wgpu_shared_buffer_view"),
            ..Default::default()
        });

        Ok(Self {
            texture,
            view,
            handle,
            format,
            width,
            height,
            device: raw_device.clone(),
            semaphore,
            external_semaphore_fd: ash::khr::external_semaphore_fd::Device::new(
                instance, raw_device,
            ),
        })
    }

    /// The texture the producer's render pass attaches.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The view the producer renders through.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The wgpu format this surface was allocated in.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The surface's width and height, in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The buffer's native description, without an acquire fence.
    ///
    /// Use it for a buffer whose contents are already complete; after a submission, call
    /// [`submit`](Self::submit) so the renderer waits on the work that rendered the frame.
    pub fn handle(&self) -> DmaBufHandle {
        self.handle.clone()
    }

    /// The native handle for the frame just submitted, carrying its acquire fence.
    ///
    /// Add the commands that render into [`view`](Self::view) to `commands`, then submit them
    /// here: this registers the exportable semaphore on that submission, submits on `queue`, and
    /// exports the semaphore as the `sync_file` the renderer polls. The buffer must not be written
    /// again until the renderer has sampled the frame. Attach a release descriptor to the returned
    /// handle ([`DmaBufHandle::release`]) and wait for it before reusing the slot; a ring without
    /// that wait is a heuristic, because a frame can stay in the consumer's queue for more
    /// submissions than the ring is deep. GPUI also drops a surface whose acquire fence has not
    /// signalled within 16 ms, so a producer that renders a frame this long should present the
    /// previous completed frame instead of the one just submitted. [`WgpuSharedBufferRing`] wraps
    /// that rule for a producer that renders continuously.
    pub fn submit(
        &mut self,
        queue: &wgpu::Queue,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> Result<DmaBufHandle> {
        // The HAL queue's signal list is drained by its next submission, so the semaphore is
        // registered before `queue.submit` and signals when that work completes.
        let hal_queue = unsafe { queue.as_hal::<wgpu::hal::vulkan::Api>() }
            .ok_or_else(|| anyhow!("an exported wgpu surface needs a Vulkan wgpu queue"))?;
        hal_queue.add_signal_semaphore(self.semaphore, None);
        queue.submit(commands);

        // SAFETY: the semaphore is live and was handed to this device; `SYNC_FD` transfers the
        // signal to the returned descriptor, which this handle then owns and closes.
        let fd = unsafe {
            self.external_semaphore_fd.get_semaphore_fd(
                &vk::SemaphoreGetFdInfoKHR::default()
                    .semaphore(self.semaphore)
                    .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
            )
        }
        .map_err(|error| anyhow!("export the acquire fence: {error:?}"))?;
        // SAFETY: `fd` is a fresh descriptor whose ownership moves into the handle.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut handle = self.handle.clone();
        handle.acquire_fence = Some(Arc::new(fd));
        Ok(handle)
    }
}

impl Drop for WgpuSharedBuffer {
    fn drop(&mut self) {
        // SAFETY: the semaphore was created by this value's device and is destroyed once. A
        // `SYNC_FD` export has already moved any pending signal into the fence's descriptor, so
        // destroying the semaphore here does not lose one.
        unsafe { self.device.destroy_semaphore(self.semaphore, None) };
    }
}

/// The Vulkan handles a wgpu device lends the shared exporter.
struct WgpuExportDevice<'a> {
    hal: &'a wgpu::hal::vulkan::Device,
    external_memory_fd: ash::khr::external_memory_fd::Device,
    drm_format_modifier: ash::ext::image_drm_format_modifier::Device,
}

impl ExportDevice for WgpuExportDevice<'_> {
    fn instance(&self) -> &ash::Instance {
        self.hal.shared_instance().raw_instance()
    }

    fn physical_device(&self) -> vk::PhysicalDevice {
        self.hal.raw_physical_device()
    }

    fn device(&self) -> &ash::Device {
        self.hal.raw_device()
    }

    fn external_memory_fd(&self) -> &ash::khr::external_memory_fd::Device {
        &self.external_memory_fd
    }

    fn drm_format_modifier(&self) -> &ash::ext::image_drm_format_modifier::Device {
        &self.drm_format_modifier
    }

    fn renderer_bound(&self) -> bool {
        // `Adapter::wgpu` lends the renderer's own device where the renderer is wgpu, and the caller
        // is on the same physical GPU otherwise; either way the modifier is read from the device
        // that will import it.
        true
    }
}

/// The wgpu and Vulkan forms of a surface format, and the handle's semantic format.
fn formats(format: wgpu::TextureFormat) -> Result<(SurfaceFormatKind, vk::Format)> {
    match format {
        wgpu::TextureFormat::Rgba8UnormSrgb => {
            Ok((SurfaceFormatKind::rgba8(), vk::Format::R8G8B8A8_SRGB))
        }
        wgpu::TextureFormat::Bgra8UnormSrgb => {
            Ok((SurfaceFormatKind::bgra8(), vk::Format::B8G8R8A8_SRGB))
        }
        _ => bail!(
            "an exported wgpu surface must sample as sRGB (Rgba8UnormSrgb or Bgra8UnormSrgb), \
             but {format:?} was asked for"
        ),
    }
}

/// Refuse a wgpu device whose Vulkan device does not carry what an export needs.
fn ensure_extensions(device: &wgpu::hal::vulkan::Device) -> Result<()> {
    let enabled = device.enabled_device_extensions();
    for required in [
        vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
        vk::KHR_EXTERNAL_MEMORY_FD_NAME,
        vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
        vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME,
    ] {
        if !enabled.contains(&required) {
            bail!(
                "an exported wgpu surface needs {} on the wgpu device; the renderer's own wgpu \
                 device has it, a plain request_device does not",
                required.to_string_lossy()
            );
        }
    }
    Ok(())
}
