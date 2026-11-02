//! Importing a dma-buf into `wgpu` textures, on the Vulkan backend.
//!
//! A [`DmaBufHandle`](gpui_engine::DmaBufHandle) is a buffer that another component — a compositor,
//! video decoder, camera or second GPU process — exports as a dma-buf: one file descriptor per plane
//! plus the layout needed to interpret it (a format, a DRM format modifier, and a byte offset and
//! row stride per plane). The renderer *consumes* such a handle to composite the buffer as a
//! surface; it does not produce or negotiate one.
//!
//! `wgpu` can adopt a raw `VkImage` (`texture_from_raw`) but cannot create one over an external
//! allocation, so the import is done with `ash` against the very device `wgpu` draws on: create the
//! image, import the descriptor into a **dedicated** allocation, bind it, and hand the image to
//! `wgpu`. The dedicated allocation is not decoration — a discrete GPU can refuse the import on
//! every memory type without it, even while advertising the format as `IMPORTABLE`, so a dedicated
//! allocation is what imports reliably across drivers.
//!
//! A descriptor is consumed by the import (the driver takes ownership), so each plane imports a
//! `dup` of its descriptor; that is also what lets an `NV12` buffer's two planes, which may share one
//! descriptor, become two independently-owned textures.
//!
//! # The producer's contract
//!
//! The importer validates the handle and refuses one it cannot honour, rather than compositing a
//! wrong image. A producer must therefore:
//!
//! - **Declare a layout the device itself produces.** For a linear buffer the importer builds a plain,
//!   unbound `LINEAR` image of the handle's format and extent and requires the handle's plane offsets
//!   and row pitches to equal the ones the driver gives *it*. A driver samples only a layout it laid
//!   out itself, and an image created with the handle's layouts *echoes* them back, so this is the only
//!   comparison that decides anything: the NVIDIA 930M accepts a linear `NV12` declaring chroma at
//!   4096, reports 4096 back, and then hangs the command processor when the image is sampled, where
//!   its own layout puts chroma at 65536. A producer reads the layout to declare from `gpui_interop`'s
//!   device report. A tiled buffer keeps the explicit `VkImageDrmFormatModifierExplicitCreateInfoEXT`
//!   path — it needs `VK_EXT_image_drm_format_modifier` on the device, and `WgpuContext` enables it.
//! - **Carry one plane per format.** [`SurfaceFormatKind::plane_count`](gpui_engine::SurfaceFormatKind::plane_count)
//!   says how many a format needs — one for packed RGB, two for semi-planar YCbCr, three for planar;
//!   a handle whose plane count does not match its format is refused.
//!
//! - **Hold enough memory to back the declared layout.** A driver may accept the declared plane
//!   layouts and still size the image for its own padded layout: an NVIDIA linear `NV12` reports the
//!   declared chroma offset and then asks for 128 KiB, its own 64 KiB-padded layout, where a canonical
//!   buffer holds 6 KiB. The buffer's own size is checked against the image's requirement, and a
//!   buffer too small to back it is refused (the shortfall named) rather than bound and sampled past
//!   its end — which faults on the GPU rather than compositing wrongly.
//!
//! See [`DmaBufHandle`](gpui_engine::DmaBufHandle) for the descriptor itself and the full set of
//! invariants a dma-buf import requires.

use std::io::{Seek as _, SeekFrom};
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use ash::vk;
use gpui_engine::{
    DmaBufCapabilities, DmaBufFormatCapability, DmaBufHandle, DmaBufPlane, RgbOrder, SampleDepth,
    Subsampling, SurfaceFormatKind,
};

/// A plane's texture, adopted into `wgpu`, and the view the renderer samples.
///
/// For a single-plane buffer the view is the whole texture. For `Nv12` the texture is the **one**
/// multi-planar image over the buffer and the view is one of its two planes, so the pair of entries
/// the importer returns are two views of one image — the shape a decoder exports — rather than two
/// independently imported ones. That keeps the buffer the object its modifier describes, and avoids
/// the per-plane `R8`/`RG8` representations a copy or a later feature would trip over.
#[derive(Clone)]
pub(crate) struct PlaneTexture {
    /// The wgpu texture `wgpu` owns, over the imported image. Held so the image — and the imported
    /// memory behind it — outlives the view taken from it.
    #[allow(
        dead_code,
        reason = "the view samples the image, but this keeps it alive"
    )]
    pub texture: wgpu::Texture,
    /// The view of that image this plane samples.
    pub view: wgpu::TextureView,
}

/// Import a dma-buf as the texture(s) the surface pipeline composites.
///
/// One texture for a packed RGB buffer, two (the plane views of one multi-planar image) for `NV12`.
pub(crate) fn import_dmabuf(
    device: &wgpu::Device,
    handle: &DmaBufHandle,
) -> Result<Vec<PlaneTexture>> {
    let format = handle.format;
    let dimensions = format.dimensions();
    let expected = dimensions.plane_count();
    anyhow::ensure!(
        handle.planes.len() == expected,
        "{format} needs {expected} plane(s), but the handle carries {}",
        handle.planes.len(),
    );

    let hal = unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }
        .context("importing a dma-buf needs the Vulkan backend")?;
    let instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();

    if !dimensions.is_yuv() {
        // A packed RGB plane, sampled straight through: its `VkFormat` is its component order at its
        // bit depth.
        let (wgpu_format, vk_format) = match (dimensions.rgb_order(), dimensions.depth()) {
            (RgbOrder::Bgr, SampleDepth::Bits8) => {
                (wgpu::TextureFormat::Bgra8Unorm, vk::Format::B8G8R8A8_UNORM)
            }
            (RgbOrder::Rgb, SampleDepth::Bits8) => {
                (wgpu::TextureFormat::Rgba8Unorm, vk::Format::R8G8B8A8_UNORM)
            }
            _ => anyhow::bail!("unsupported RGB surface format: {format}"),
        };
        return Ok(vec![import_plane(
            device,
            &hal,
            instance,
            physical,
            &handle.planes[0],
            handle.modifier,
            vk_format,
            wgpu_format,
            handle.width,
            handle.height,
        )?]);
    }

    // A YCbCr buffer: plane 0 is luma at full resolution; plane 1 is interleaved chroma at half, in
    // both dimensions. Both are planes of the one image the buffer describes. Only the 8-bit 4:2:0
    // family is imported so far; a format outside it is dropped rather than sampled wrong.
    anyhow::ensure!(
        dimensions.subsampling() == Subsampling::C420 && dimensions.depth() == SampleDepth::Bits8,
        "unsupported YCbCr surface format: {format}",
    );
    import_luma_chroma(device, &hal, instance, physical, handle)
}

/// What `device` can import as a dma-buf surface.
///
/// The renderer's own answer, asked of the device it already has: no device is created, no memory is
/// allocated, and the format table comes from `vkGetPhysicalDeviceFormatProperties`. `None` when the
/// device is not Vulkan — the importer has no other arm.
///
/// The reported formats are the importer's closed set (the packed 8-bit RGB pair and the 8-bit 4:2:0
/// pair), not every format VideoCore or the driver knows. A modifier is reported only when the
/// importer can bind it: `LINEAR` when the format's linear tiling carries `SAMPLED_IMAGE`, and a
/// driver modifier only when the device took `VK_EXT_image_drm_format_modifier` (otherwise the
/// importer refuses tiled buffers, so reporting one would invite a buffer it would drop).
pub fn dmabuf_capabilities(device: &wgpu::Device) -> Option<DmaBufCapabilities> {
    let hal = unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }?;
    let instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();
    let tiled = honours_plane_layouts(&hal);

    let formats = importable_formats()
        .into_iter()
        .map(|(format, vk_format)| {
            let properties =
                unsafe { instance.get_physical_device_format_properties(physical, vk_format) };
            let mut modifiers = Vec::new();
            if properties
                .linear_tiling_features
                .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
            {
                modifiers.push(DmaBufHandle::LINEAR);
            }
            if tiled {
                for properties in drm_modifiers(instance, physical, vk_format) {
                    if properties
                        .drm_format_modifier_tiling_features
                        .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
                        && !modifiers.contains(&properties.drm_format_modifier)
                    {
                        modifiers.push(properties.drm_format_modifier);
                    }
                }
            }
            DmaBufFormatCapability { format, modifiers }
        })
        .collect();

    Some(DmaBufCapabilities::new(formats))
}

/// The formats the importer handles, and the Vulkan format each is created as.
fn importable_formats() -> [(SurfaceFormatKind, vk::Format); 4] {
    [
        (SurfaceFormatKind::bgra8(), vk::Format::B8G8R8A8_UNORM),
        (SurfaceFormatKind::rgba8(), vk::Format::R8G8B8A8_UNORM),
        (
            SurfaceFormatKind::nv12(),
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
        ),
        // NV21 has no Vulkan format of its own: the plane layout is NV12's, and the chroma order is
        // the sampler's swap. The importer creates the same image and lets the shader do it.
        (
            SurfaceFormatKind::nv21(),
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
        ),
    ]
}

/// Every DRM format modifier `format` supports, with the features each carries.
fn drm_modifiers(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    format: vk::Format,
) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
    // SAFETY: both calls read; the second fills a buffer sized from the first call's count.
    unsafe {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        instance.get_physical_device_format_properties2(physical, format, &mut properties);

        let count = list.drm_format_modifier_count as usize;
        if count == 0 {
            return Vec::new();
        }
        let mut modifiers = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut modifiers);
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        instance.get_physical_device_format_properties2(physical, format, &mut properties);
        modifiers
    }
}

/// Whether `hal` carries `VK_EXT_image_drm_format_modifier`, which an explicit plane layout needs.
///
/// A capability check, not a convenience: `VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT` is invalid to use
/// without the extension, so the importer has to ask rather than learn it from a failing
/// `vkCreateImage`. `WgpuContext::device_with_surface_import` enables the extension where the adapter
/// supports it and reports whether the device took it.
fn honours_plane_layouts(hal: &wgpu::hal::vulkan::Device) -> bool {
    hal.enabled_device_extensions()
        .contains(&vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME)
}

/// Refuse a handle whose declared plane layout is not one the driver itself produces.
///
/// A driver samples only a layout it laid out itself, so this is the comparison that decides whether an
/// imported image can be sampled at all — and it must be made against a **plain** image: an image
/// created with the handle's layouts echoes them back through `vkGetImageSubresourceLayout`, so
/// comparing that can never disagree. Measured: an NVIDIA 930M accepts a linear `NV12` declaring chroma
/// at 4096, reports 4096 back, and then hangs the command processor when the image is sampled, where
/// its own layout puts chroma at 65536.
///
/// The comparison is on the **plane offsets**, not the row pitches. An offset is the hazard: the part's
/// texturing hardware maps planes on fixed page boundaries, so a chroma plane at 4096 where the driver
/// wants 65536 wedges it. A pitch is not: the producer's memory is laid out at whatever pitch it
/// declares, and the driver produces two of them for one format depending on how the image is made —
/// measured on the same 930M, a plain `LINEAR` `NV12` gets a pitch of 64 while an *exportable* one (the
/// image a `SharedBuffer` exports) gets 128, both at the same offsets. Checking pitches would refuse
/// the exportable one, which is a buffer the driver laid out and samples itself.
///
/// The probe costs nothing — `vkCreateImage` allocates no memory until memory is bound, so a plain
/// `LINEAR` image of the same format and extent is created, asked, and destroyed with no submission.
/// `aspects` names the planes `declared` describes, in order, as a plain linear image names them
/// (`PLANE_0`/`PLANE_1`, or `COLOR` for a single-plane format).
fn ensure_native_layout(
    raw: &ash::Device,
    format: vk::Format,
    width: u32,
    height: u32,
    aspects: &[vk::ImageAspectFlags],
    declared: &[vk::SubresourceLayout],
) -> Result<()> {
    let probe = create_plain_linear_image(raw, format, width, height)?;
    for (index, (aspect, declared)) in aspects.iter().zip(declared).enumerate() {
        let subresource = vk::ImageSubresource {
            aspect_mask: *aspect,
            mip_level: 0,
            array_layer: 0,
        };
        // SAFETY: `probe` is live and bound to no memory; the call only reads its layout.
        let native = unsafe { raw.get_image_subresource_layout(probe, subresource) };
        if native.offset != declared.offset {
            // SAFETY: the probe was created above and has never been bound or used.
            unsafe { raw.destroy_image(probe, None) };
            anyhow::bail!(
                "the renderer's device lays a {format:?} {width}×{height} linear image's plane {index} \
                 out at offset {}, and the handle declares offset {}. This device cannot sample a \
                 layout it did not choose — export the layout it reports for a plain linear image, or \
                 a tiled modifier it lists",
                native.offset,
                declared.offset,
            );
        }
    }
    // SAFETY: the probe was created above and has never been bound or used.
    unsafe { raw.destroy_image(probe, None) };
    Ok(())
}

/// A plain, unbound `LINEAR` image of `format` — the layout the driver chooses for itself. It costs no
/// memory (`vkCreateImage` allocates nothing until memory is bound), and the caller destroys it.
fn create_plain_linear_image(
    raw: &ash::Device,
    format: vk::Format,
    width: u32,
    height: u32,
) -> Result<vk::Image> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::LINEAR)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: `info` chains nothing, and the image is owned by the caller.
    unsafe { raw.create_image(&info, None) }.context("create a plain linear layout probe")
}

/// Refuse an image the buffer cannot back, naming the shortfall so a producer can size its export.
///
/// A driver may accept the handle's declared plane layouts and still require more memory than those
/// layouts span: an NVIDIA linear `NV12` reports the declared chroma offset and then asks for 128 KiB,
/// its own 64 KiB-padded layout, where the canonical layout the producer exported holds 6 KiB (measured
/// on a 930M). Binding the smaller buffer and sampling it reads past the end of the dma-buf, which
/// faults on the GPU instead of compositing wrongly. The descriptor's own size is therefore the ground
/// truth: an image needing more than the buffer holds is destroyed and the surface refused, and the
/// numbers are named so the producer can export a buffer this device can back.
fn ensure_buffer_backs_image(
    raw: &ash::Device,
    image: vk::Image,
    requirements: vk::MemoryRequirements,
    bind_offset: u64,
    fd: &OwnedFd,
) -> Result<()> {
    let available = buffer_size(fd)?;
    let needed = bind_offset + requirements.size;
    if needed > available {
        // SAFETY: the image was created above and is neither bound to memory nor used.
        unsafe { raw.destroy_image(image, None) };
        anyhow::bail!(
            "the renderer's device needs {needed} bytes for an image bound at offset {bind_offset}, \
             but the dma-buf holds {available}: this device cannot back the declared layout. Export a \
             buffer of at least {needed} bytes, or one whose plane layout it backs with what it has",
        );
    }
    Ok(())
}

/// How many bytes a dma-buf holds — `lseek(SEEK_END)`, the one size the descriptor itself carries.
fn buffer_size(fd: &OwnedFd) -> Result<u64> {
    let mut file = std::fs::File::from(fd.try_clone().context("duplicate the descriptor")?);
    file.seek(SeekFrom::End(0)).context("measure the dma-buf")
}

/// Import an `Nv12` buffer's two planes as the **one** multi-planar image they describe, and view
/// each plane.
///
/// The buffer is a single object at the two planes' offsets, which is what `vaExportSurfaceHandle`
/// and GBM produce and what the plane layouts below state. Importing it as one `G8_B8R8_2PLANE_420`
/// image (rather than two separate `R8`/`RG8` ones) is what keeps the buffer in the shape its
/// modifier describes.
fn import_luma_chroma(
    device: &wgpu::Device,
    hal: &wgpu::hal::vulkan::Device,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    handle: &DmaBufHandle,
) -> Result<Vec<PlaneTexture>> {
    let raw = hal.raw_device();
    let luma = &handle.planes[0];
    let chroma = &handle.planes[1];
    let tiled = handle.modifier != DmaBufHandle::LINEAR;
    let explicit = honours_plane_layouts(hal);
    anyhow::ensure!(
        explicit || !tiled,
        "a tiled Nv12 dma-buf needs VK_EXT_image_drm_format_modifier on the renderer's device, and \
         this one does not have it",
    );

    // The image binds at the object's base (`luma.offset`), so each plane's layout is its offset
    // from that base; its row pitch is the plane's stride. The driver validates both.
    let plane_layouts = [
        vk::SubresourceLayout::default()
            .offset(0)
            .row_pitch(u64::from(luma.stride)),
        vk::SubresourceLayout::default()
            .offset(chroma.offset.saturating_sub(luma.offset))
            .row_pitch(u64::from(chroma.stride)),
    ];
    // Only a *linear* handle is checked against the device's own layout: a tiled buffer's layout is the
    // modifier's, and there is no plain image to compare it with. Tiled is already gated on the device
    // taking `VK_EXT_image_drm_format_modifier` above.
    if !tiled {
        ensure_native_layout(
            raw,
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
            handle.width,
            handle.height,
            &[vk::ImageAspectFlags::PLANE_0, vk::ImageAspectFlags::PLANE_1],
            &plane_layouts,
        )?;
    }
    let mut modifier_layout = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(handle.modifier)
        .plane_layouts(&plane_layouts);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
        .extent(vk::Extent3D {
            width: handle.width,
            height: handle.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        // `DRM_FORMAT_MODIFIER_EXT` carries an explicit layout for `DRM_FORMAT_MOD_LINEAR` as well as
        // for a vendor modifier, and is the only way to make the driver read the planes where the
        // handle says they are rather than where the driver would put them.
        .tiling(if explicit {
            vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
        } else {
            vk::ImageTiling::LINEAR
        })
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image_info = if explicit {
        image_info.push_next(&mut modifier_layout)
    } else {
        image_info
    };
    let image =
        unsafe { raw.create_image(&image_info, None) }.context("create the imported image")?;
    let requirements = unsafe { raw.get_image_memory_requirements(image) };
    ensure_buffer_backs_image(raw, image, requirements, luma.offset, &luma.fd)?;

    let fd = luma
        .fd
        .try_clone()
        .context("duplicate the plane descriptor for the import")?
        .into_raw_fd();
    let memory = import_memory(
        instance,
        physical,
        raw,
        image,
        requirements,
        luma.offset,
        fd,
    )?;
    unsafe { raw.bind_image_memory(image, memory, luma.offset) }
        .context("bind the imported image at the object's base")?;

    let size = wgpu::Extent3d {
        width: handle.width,
        height: handle.height,
        depth_or_array_layers: 1,
    };
    let hal_texture = unsafe {
        hal.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some("dma-buf surface"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::NV12,
                usage: wgpu::TextureUses::RESOURCE,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: vec![wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm],
            },
            None,
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
        )
    };
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu::hal::vulkan::Api>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("dma-buf surface"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::NV12,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm],
            },
        )
    };

    let luma_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("dma-buf surface luma"),
        format: Some(wgpu::TextureFormat::R8Unorm),
        aspect: wgpu::TextureAspect::Plane0,
        ..Default::default()
    });
    let chroma_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("dma-buf surface chroma"),
        format: Some(wgpu::TextureFormat::Rg8Unorm),
        aspect: wgpu::TextureAspect::Plane1,
        ..Default::default()
    });
    Ok(vec![
        PlaneTexture {
            texture: texture.clone(),
            view: luma_view,
        },
        PlaneTexture {
            texture,
            view: chroma_view,
        },
    ])
}

/// How long the renderer waits for a producer's `sync_file` before dropping the surface for that
/// frame — about one frame at 60 Hz, so a producer that is merely a frame late still composites.
///
/// A producer further behind loses a frame rather than stalling the UI, the same "fault softly"
/// rule the rest of the surface path follows.
pub(crate) const ACQUIRE_FENCE_TIMEOUT: Duration = Duration::from_millis(16);

/// Wait, up to `timeout`, for a surface's acquire fence to signal; `true` if it signalled in time.
///
/// A `sync_file` becomes readable when its fence signals, so this is a `poll(2)`. It is a *host*
/// wait on purpose: `wgpu` owns its queue and gives no hook to make one of its submissions wait on
/// an external semaphore, and a queue-level wait has no timeout — a producer that never signals
/// would stall the renderer forever. A poll can give up, so a broken producer costs a frame rather
/// than the application.
///
/// The common case is free: a producer at or ahead of the frame rate has already signalled, and
/// `poll` returns at once.
pub(crate) fn wait_for_acquire_fence(fence: &OwnedFd, timeout: Duration) -> bool {
    let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let mut descriptor = libc::pollfd {
        fd: fence.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        match unsafe { libc::poll(&mut descriptor, 1, timeout_ms) } {
            // Readable: the fence signalled. An invalid descriptor (`POLLNVAL`) reports ready too
            // and falls through to the import, which drops the surface.
            1.. => return true,
            // Timed out: the producer is not done, so give up rather than block the frame.
            0 => return false,
            // `-1`: retry an interrupted wait (a signal), otherwise treat the fence as unusable and
            // drop the surface.
            _ => {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
        }
    }
}

/// Signal a producer's release descriptor: the buffer has been sampled and its surface may be
/// recycled. The descriptor is an `eventfd` the producer made (a VA-API playback's, say); writing
/// eight bytes increments it, which makes the producer's `poll` return.
///
/// Called from the `on_submitted_work_done` callback of the submission that sampled the buffer, so
/// the GPU has finished with it — the mirror of [`wait_for_acquire_fence`], which the producer's
/// decoder orders the other way.
pub(crate) fn signal_release(release: &OwnedFd) {
    let counter: u64 = 1;
    // SAFETY: `release` is a valid open descriptor, and the pointer and length describe one `u64`.
    let written = unsafe {
        libc::write(
            release.as_raw_fd(),
            std::ptr::addr_of!(counter).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    if written < 0 {
        log::warn!(
            "a dma-buf release descriptor did not signal: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Import one plane's descriptor as a `VkImage`, and adopt it into a `wgpu` texture.
///
/// `modifier` is the buffer's DRM format modifier: [`DmaBufHandle::LINEAR`](gpui_engine::DmaBufHandle::LINEAR)
/// makes a linear image, and any other value makes a tiled one whose single-`plane` layout is given
/// explicitly, which is the shape `vaExportSurfaceHandle` and GBM exports describe. A tiled image
/// needs `VK_EXT_image_drm_format_modifier` on the device, which `WgpuContext` enables; a device
/// without it refuses the image, and the surface is dropped rather than sampled wrong.
#[expect(clippy::too_many_arguments)]
fn import_plane(
    device: &wgpu::Device,
    hal: &wgpu::hal::vulkan::Device,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    plane: &DmaBufPlane,
    modifier: u64,
    vk_format: vk::Format,
    wgpu_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> Result<PlaneTexture> {
    let raw = hal.raw_device();
    let tiled = modifier != DmaBufHandle::LINEAR;
    let explicit = honours_plane_layouts(hal);
    anyhow::ensure!(
        explicit || !tiled,
        "a tiled dma-buf needs VK_EXT_image_drm_format_modifier on the renderer's device, and this \
         one does not have it",
    );
    // The image binds at the plane's offset within the object, so the plane's own layout starts at
    // zero; its row pitch is the buffer's. The driver validates both against the modifier.
    let plane_layout = vk::SubresourceLayout::default()
        .offset(0)
        .row_pitch(u64::from(plane.stride));
    // Only a *linear* handle is checked against the device's own layout: a tiled buffer's layout is the
    // modifier's, and there is no plain image to compare it with. Tiled is already gated on the device
    // taking `VK_EXT_image_drm_format_modifier` above.
    if !tiled {
        ensure_native_layout(
            raw,
            vk_format,
            width,
            height,
            &[vk::ImageAspectFlags::COLOR],
            &[plane_layout],
        )?;
    }
    let mut modifier_layout = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(modifier)
        .plane_layouts(std::slice::from_ref(&plane_layout));
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk_format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        // `DRM_FORMAT_MODIFIER_EXT` carries an explicit layout for `DRM_FORMAT_MOD_LINEAR` as well as
        // for a vendor modifier, and is the only way to make the driver read the plane where the
        // handle says it is rather than where the driver would put it.
        .tiling(if explicit {
            vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
        } else {
            vk::ImageTiling::LINEAR
        })
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image_info = if explicit {
        image_info.push_next(&mut modifier_layout)
    } else {
        image_info
    };
    let image =
        unsafe { raw.create_image(&image_info, None) }.context("create the imported image")?;
    let requirements = unsafe { raw.get_image_memory_requirements(image) };
    ensure_buffer_backs_image(raw, image, requirements, plane.offset, &plane.fd)?;

    // The descriptor is consumed by the import, so hand over a duplicate and let the driver own it.
    let fd = plane
        .fd
        .try_clone()
        .context("duplicate the plane descriptor for the import")?
        .into_raw_fd();

    let memory = import_memory(
        instance,
        physical,
        raw,
        image,
        requirements,
        plane.offset,
        fd,
    )?;
    unsafe { raw.bind_image_memory(image, memory, plane.offset) }
        .context("bind the imported image at the plane's offset")?;

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hal_texture = unsafe {
        hal.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some("dma-buf surface plane"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUses::RESOURCE,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: vec![],
            },
            None,
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
        )
    };
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu::hal::vulkan::Api>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("dma-buf surface plane"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("dma-buf surface plane view"),
        ..Default::default()
    });
    Ok(PlaneTexture { texture, view })
}

/// Import `fd` into a dedicated allocation for `image`, trying every memory type it can live in.
///
/// `offset` is where the image binds within the buffer, so the allocation must span
/// `offset + requirements.size` — a plane at a non-zero offset (an `NV12` buffer's chroma) needs the
/// whole buffer, not just its own tail. Which memory type a driver accepts a dma-buf into is not
/// always the obvious one — a discrete GPU can refuse the import on every type until the allocation
/// is dedicated, and the one type that imports may not be the one advertising the format as
/// `IMPORTABLE` — so this tries them all rather than guessing.
fn import_memory(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    raw: &ash::Device,
    image: vk::Image,
    requirements: vk::MemoryRequirements,
    offset: u64,
    fd: std::os::fd::RawFd,
) -> Result<vk::DeviceMemory> {
    let properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    let mut last_error = None;
    for index in 0..properties.memory_type_count {
        if requirements.memory_type_bits & (1 << index) == 0 {
            continue;
        }
        let mut import_info = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(offset + requirements.size)
            .memory_type_index(index)
            .push_next(&mut import_info)
            .push_next(&mut dedicated);
        match unsafe { raw.allocate_memory(&allocate_info, None) } {
            Ok(memory) => return Ok(memory),
            Err(error) => last_error = Some(error),
        }
    }
    Err(anyhow::anyhow!(
        "no memory type accepted the dma-buf import (last: {last_error:?})",
    ))
}

/// Reuses the textures imported for a dma-buf across frames.
///
/// The Apple backend has the platform's `CVMetalTextureCache` for exactly this: a CoreVideo texture cache
/// held on the renderer that hands back the same `MTLTexture` for the same `CVPixelBuffer`. There is
/// no such facility for dma-bufs, so this is it — and it is what keeps painting a live surface from
/// re-creating a `VkImage`, a dedicated allocation and a `wgpu::Texture` every frame.
///
/// Identity is the handle's *descriptors*, compared by `Arc` identity, never the numbers. A producer
/// that keeps its handle (the natural way — `surface(handle.clone())` each frame) hits, and a
/// different buffer never aliases, even if the kernel hands the same descriptor number back.
pub(crate) struct DmaBufTextureCache {
    entries: Vec<CacheEntry>,
    frame: u64,
}

struct CacheEntry {
    handle: DmaBufHandle,
    textures: Vec<PlaneTexture>,
    last_used: u64,
}

/// At most this many buffers stay imported. A producer streams from a small pool — two or three
/// buffers — so this covers the recycling without pinning an unbounded number of its buffers.
const CAPACITY: usize = 8;

impl DmaBufTextureCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            frame: 0,
        }
    }

    /// The textures for `handle`, importing them the first time this buffer is seen since it was
    /// last evicted.
    ///
    /// Clones are returned (cheap handles into the same allocation); the cache keeps the originals,
    /// and with them the imported memory.
    pub(crate) fn textures(
        &mut self,
        device: &wgpu::Device,
        handle: &DmaBufHandle,
    ) -> Result<Vec<PlaneTexture>> {
        self.frame += 1;
        let frame = self.frame;

        if let Some(index) = self.find(handle) {
            self.entries[index].last_used = frame;
            return Ok(self.entries[index].textures.clone());
        }

        let imported = import_dmabuf(device, handle)?;

        self.evict_if_full();
        self.entries.push(CacheEntry {
            handle: handle.clone(),
            textures: imported.clone(),
            last_used: frame,
        });
        Ok(imported)
    }

    /// The index of the entry for `handle`, if this buffer is already imported.
    fn find(&self, handle: &DmaBufHandle) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| same_buffer(&entry.handle, handle))
    }

    /// Make room for one more entry, dropping the least recently used when full, so a recycled
    /// buffer is re-imported rather than pinned forever.
    fn evict_if_full(&mut self) {
        if self.entries.len() < CAPACITY {
            return;
        }
        if let Some((index, _)) = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used)
        {
            self.entries.swap_remove(index);
        }
    }
}

/// Whether two handles describe the same buffer: the same descriptors (by `Arc` identity), layout
/// and format. Deliberately ignores the acquire fence, which is per frame, not per buffer.
fn same_buffer(a: &DmaBufHandle, b: &DmaBufHandle) -> bool {
    a.width == b.width
        && a.height == b.height
        && a.format == b.format
        && a.modifier == b.modifier
        && a.planes.len() == b.planes.len()
        && a.planes
            .iter()
            .zip(&b.planes)
            .all(|(a, b)| Arc::ptr_eq(&a.fd, &b.fd) && a.offset == b.offset && a.stride == b.stride)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_engine::SurfaceFormatKind;
    use std::os::fd::OwnedFd;

    /// A real descriptor without a GPU: `/dev/null` is an open file, and any open file is a valid
    /// plane descriptor as far as the descriptor type is concerned.
    fn descriptor() -> OwnedFd {
        std::fs::File::open("/dev/null").expect("/dev/null").into()
    }

    fn handle(width: u32, offset: u64, stride: u32) -> DmaBufHandle {
        DmaBufHandle::new(
            width,
            2,
            SurfaceFormatKind::bgra8(),
            DmaBufHandle::LINEAR,
            [DmaBufPlane::new(descriptor(), offset, stride)],
            None,
        )
    }

    #[test]
    fn a_cloned_handle_is_the_same_buffer() {
        // The natural producer pattern: keep the handle, hand the renderer a clone each frame.
        let handle = handle(4, 0, 16);
        assert!(same_buffer(&handle, &handle.clone()));
    }

    #[test]
    fn layout_and_format_differences_are_different_buffers() {
        let base = handle(4, 0, 16);
        assert!(!same_buffer(&base, &handle(8, 0, 16)), "width");
        assert!(!same_buffer(&base, &handle(4, 16, 16)), "offset");
        assert!(!same_buffer(&base, &handle(4, 0, 32)), "stride");

        let rgba = DmaBufHandle::new(
            4,
            2,
            SurfaceFormatKind::rgba8(),
            DmaBufHandle::LINEAR,
            [DmaBufPlane::new(descriptor(), 0, 16)],
            None,
        );
        assert!(!same_buffer(&base, &rgba), "format");
    }

    #[test]
    fn a_different_descriptor_is_a_different_buffer() {
        // Two handles built from separate `/dev/null` opens: different descriptors, so no aliasing
        // even though they describe the same shape.
        assert!(!same_buffer(&handle(4, 0, 16), &handle(4, 0, 16)));
    }

    #[test]
    fn the_fence_does_not_affect_identity() {
        // The fence is per frame, not per buffer: a producer that attaches a fresh sync_file each
        // frame must still hit the cache.
        let handle = handle(4, 0, 16);
        let mut with_fence = handle.clone();
        with_fence.acquire_fence = Some(Arc::new(descriptor()));
        assert!(same_buffer(&handle, &with_fence));
    }

    fn entry(handle: DmaBufHandle, last_used: u64) -> CacheEntry {
        CacheEntry {
            handle,
            textures: Vec::new(),
            last_used,
        }
    }

    #[test]
    fn find_locates_the_matching_buffer() {
        let buffer = handle(4, 0, 16);
        let other = handle(8, 0, 16);
        let mut cache = DmaBufTextureCache::new();
        assert_eq!(cache.find(&buffer), None);
        cache.entries.push(entry(buffer.clone(), 1));
        cache.entries.push(entry(other.clone(), 1));
        assert_eq!(cache.find(&buffer), Some(0));
        assert_eq!(cache.find(&other), Some(1));
    }

    #[test]
    fn a_full_cache_evicts_the_least_recently_used() {
        let buffers: Vec<_> = (0..CAPACITY)
            .map(|i| handle((i + 1) as u32, 0, 16))
            .collect();
        let mut cache = DmaBufTextureCache::new();
        for (i, buffer) in buffers.iter().enumerate() {
            cache.entries.push(entry(buffer.clone(), i as u64));
        }
        // Touch everything but the oldest.
        for entry in &mut cache.entries[1..] {
            entry.last_used = 100;
        }
        cache.evict_if_full();
        assert_eq!(cache.entries.len(), CAPACITY - 1);
        assert!(
            cache.find(&buffers[0]).is_none(),
            "the oldest entry was evicted"
        );
        for buffer in &buffers[1..] {
            assert!(cache.find(buffer).is_some());
        }
    }

    #[test]
    fn a_cache_with_room_evicts_nothing() {
        let mut cache = DmaBufTextureCache::new();
        cache.entries.push(entry(handle(4, 0, 16), 1));
        cache.evict_if_full();
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn an_already_signalled_fence_does_not_block() {
        // `/dev/null` is always readable — the shape of a `sync_file` whose fence has signalled.
        let fence: OwnedFd = std::fs::File::open("/dev/null").expect("/dev/null").into();
        assert!(wait_for_acquire_fence(&fence, Duration::from_millis(100)));
    }

    #[test]
    fn a_fence_that_never_signals_times_out() {
        // A pipe with no data is the shape of a producer that has not finished; the wait must give
        // up rather than hang. The write end is held open (a dropped writer reads as EOF, which is
        // `ready`), so nothing ever signals.
        let (read_end, _write_end) = std::io::pipe().expect("a pipe");
        let fence = OwnedFd::from(read_end);
        let started = std::time::Instant::now();
        assert!(!wait_for_acquire_fence(&fence, Duration::from_millis(10)));
        assert!(
            started.elapsed() >= Duration::from_millis(5),
            "it should have waited"
        );
    }
}
