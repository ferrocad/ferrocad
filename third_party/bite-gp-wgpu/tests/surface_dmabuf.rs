//! An external surface composited through the authoring API, end to end.
//!
//! Two authoring layers reach one: the `surface()` element (single-plane `Bgra8`/`Rgba8`, two-plane
//! `Nv12`) and `gpu_canvas(..)`'s paint callback (a same-device `wgpu::TextureView`). The producer
//! is a bare Vulkan device (not wgpu), the consumer is the platform's headless renderer — the same
//! one a window drives — and the pixels are read back with `capture_screenshot`.
//!
//! Opt in, because it needs a GPU and the crate's own unit-test target does not build:
//!
//! ```text
//! ZED_DEVICE_ID=1916 cargo test -p gpui_wgpu --features test-support --test surface_dmabuf
//! ```
//!
//! It skips (rather than fails) where there is no Vulkan device, and it pins the adapter when you
//! pass `ZED_DEVICE_ID`.
//!
//! # Running this on a machine with two GPUs
//!
//! The headless renderer chooses an adapter itself, and on a laptop it may choose the discrete one.
//! A userspace fault *while GPU work is in flight* can wedge that GPU: an earlier revision dropped
//! the Vulkan loader before the device it had made, and its crash produced `NVRM: Xid 13` followed by
//! repeated `Xid 158` (`NV_UFLUSH_FB_FLUSH` timeout) on an NVIDIA 930M, hanging the machine until
//! reboot. This test keeps the loader alive, and the producer opens its device on the same GPU the
//! renderer chose (it honours `ZED_DEVICE_ID` too), so export and import stay on one device; you can
//! pin the adapter explicitly:
//!
//! ```text
//! ZED_DEVICE_ID=1916 cargo test -p gpui_wgpu --features test-support --test surface_dmabuf
//! ```
//!
//! `ZED_DEVICE_ID` is a **four-digit hexadecimal** PCI device id; `1916` is the Intel HD 520 on the
//! machine this was written on (a decimal id like `6422` parses as the hex `0x6422`, matches nothing,
//! and silently falls back to the default adapter — the discrete one here).

#![cfg(all(target_os = "linux", feature = "test-support"))]

use std::cell::Cell;
use std::io::Write as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::rc::Rc;

use ash::vk;
use gpui::{
    AnyWindowHandle, AppContext as _, Context, Corners, DmaBufHandle, DmaBufPlane,
    HeadlessAppContext, ImportedTextureHandle, IntoElement, Render, SurfaceFormatKind,
    TransferFunction, Window, div, gpu_canvas, prelude::*, px, size, surface,
};
use gpui_wgpu::{CosmicTextSystem, ImportedTextureExt as _, WgpuRenderer};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
/// The one fixture colour, olive green, shared by every row that asserts a colour. Opaque, so the
/// surface's premultiplied-alpha blend leaves it unchanged.
const COLOUR: [u8; 4] = [128, 128, 0, 255];
/// `COLOUR` in the byte order a `Bgra8` buffer stores: blue, green, red, alpha.
const BGRA_COLOUR: [u8; 4] = [COLOUR[2], COLOUR[1], COLOUR[0], COLOUR[3]];

/// The first memory type carrying `flags`.
fn memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    properties
        .memory_types
        .iter()
        .enumerate()
        .find(|(_, memory_type)| memory_type.property_flags.contains(flags))
        .map(|(index, _)| index as u32)
}

/// Keeps the exported dma-buf alive: the loader must outlive the device it made, and the device the
/// memory it exported.
struct Producer {
    _entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    /// The buffers and memories exported through so far, freed when the device goes.
    exports: Vec<(vk::Buffer, vk::DeviceMemory)>,
}

impl Drop for Producer {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Open a producer device on the GPU `ZED_DEVICE_ID` names when it is set, else the integrated one,
/// else the first available. Honouring the pin keeps the producer on the renderer's GPU: a dma-buf
/// exported on one device and imported on another is a different configuration, and not the one this
/// fixture means to exercise.
fn open_producer() -> Option<Producer> {
    fn inner() -> anyhow::Result<Producer> {
        let entry = unsafe { ash::Entry::load() }
            .map_err(|error| anyhow::anyhow!("load vulkan: {error}"))?;
        let app_info = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app_info),
                None,
            )
        }?;
        let devices = unsafe { instance.enumerate_physical_devices() }?;
        let wanted = std::env::var("ZED_DEVICE_ID")
            .ok()
            .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok());
        let properties =
            |device: vk::PhysicalDevice| unsafe { instance.get_physical_device_properties(device) };
        let physical = devices
            .iter()
            .copied()
            .find(|device| wanted.is_some_and(|id| properties(*device).device_id == id))
            .or_else(|| {
                devices.iter().copied().find(|device| {
                    properties(*device).device_type == vk::PhysicalDeviceType::INTEGRATED_GPU
                })
            })
            .or_else(|| devices.first().copied())
            .ok_or_else(|| anyhow::anyhow!("no Vulkan device for the producer"))?;
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let family = families
            .iter()
            .position(|properties| {
                properties.queue_flags.intersects(
                    vk::QueueFlags::TRANSFER | vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
                )
            })
            .ok_or_else(|| anyhow::anyhow!("no queue family for the producer"))?
            as u32;
        let priorities = [1.0f32];
        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities);
        let extensions = [
            vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
            vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
        ];
        let device = unsafe {
            instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(std::slice::from_ref(&queue_info))
                    .enabled_extension_names(&extensions),
                None,
            )
        }?;
        let queue = unsafe { device.get_device_queue(family, 0) };
        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }?;
        Ok(Producer {
            _entry: entry,
            instance,
            device,
            physical_device: physical,
            queue,
            command_pool,
            exports: Vec::new(),
        })
    }
    inner().ok()
}

impl Producer {
    /// The plane layout and memory requirement the device gives a plain `LINEAR` `WIDTH`×`HEIGHT`
    /// `NV12` image — the layout a producer must declare for the renderer's importer to accept the
    /// buffer, and how much the buffer must hold. `None` where the device will not make one.
    fn native_nv12(&self) -> Option<([(u64, u32); 2], u64)> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
            .extent(vk::Extent3D {
                width: WIDTH,
                height: HEIGHT,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { self.device.create_image(&info, None) }.ok()?;
        let plane = |aspect_mask| unsafe {
            self.device.get_image_subresource_layout(
                image,
                vk::ImageSubresource {
                    aspect_mask,
                    mip_level: 0,
                    array_layer: 0,
                },
            )
        };
        let luma = plane(vk::ImageAspectFlags::PLANE_0);
        let chroma = plane(vk::ImageAspectFlags::PLANE_1);
        // SAFETY: the image is live and bound to no memory yet.
        let required = unsafe { self.device.get_image_memory_requirements(image) }.size;
        unsafe { self.device.destroy_image(image, None) };
        Some((
            [
                (luma.offset, luma.row_pitch as u32),
                (chroma.offset, chroma.row_pitch as u32),
            ],
            required,
        ))
    }

    /// Allocate a linear dma-buf holding `content` and export its fd.
    ///
    /// The memory is device-local and filled by a staging copy on the GPU: the NVIDIA driver faults
    /// when a `HOST_VISIBLE` allocation is exported, so the bytes cannot be mapped into the buffer that
    /// becomes the dma-buf. This is the shape `gpui_interop`'s `SharedBuffer` uses, for that reason.
    fn export(&mut self, content: &[u8]) -> Option<OwnedFd> {
        let size = content.len() as u64;
        let properties = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        let device_local = memory_type(&properties, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        let host_visible = memory_type(
            &properties,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;

        // The exported buffer, in memory the driver hands out as a dma-buf.
        let exported = unsafe {
            self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .ok()?;
        let requirements = unsafe { self.device.get_buffer_memory_requirements(exported) };
        let mut export_info = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let exported_memory = unsafe {
            self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(device_local)
                    .push_next(&mut export_info),
                None,
            )
        }
        .ok()?;
        unsafe { self.device.bind_buffer_memory(exported, exported_memory, 0) }.ok()?;

        // The host-visible staging buffer the bytes go through.
        let staging = unsafe {
            self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .ok()?;
        let staging_requirements = unsafe { self.device.get_buffer_memory_requirements(staging) };
        let staging_memory = unsafe {
            self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(staging_requirements.size)
                    .memory_type_index(host_visible),
                None,
            )
        }
        .ok()?;
        unsafe { self.device.bind_buffer_memory(staging, staging_memory, 0) }.ok()?;
        unsafe {
            let mapped = self
                .device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .ok()? as *mut u8;
            std::ptr::copy_nonoverlapping(content.as_ptr(), mapped, content.len());
            self.device.unmap_memory(staging_memory);
        }

        // Copy it in on the GPU and wait, so the buffer is ready when this returns.
        unsafe {
            let allocate = vk::CommandBufferAllocateInfo::default()
                .command_pool(self.command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let commands = self.device.allocate_command_buffers(&allocate).ok()?;
            let command = commands[0];
            self.device
                .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
                .ok()?;
            self.device.cmd_copy_buffer(
                command,
                staging,
                exported,
                &[vk::BufferCopy::default().size(size)],
            );
            self.device.end_command_buffer(command).ok()?;
            let submit = vk::SubmitInfo::default().command_buffers(&commands);
            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .ok()?;
            self.device
                .queue_submit(self.queue, &[submit], fence)
                .ok()?;
            self.device.wait_for_fences(&[fence], true, u64::MAX).ok()?;
            self.device.destroy_fence(fence, None);
            self.device
                .free_command_buffers(self.command_pool, &commands);
        }

        let external_memory_fd =
            ash::khr::external_memory_fd::Device::new(&self.instance, &self.device);
        let fd = unsafe {
            external_memory_fd.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(exported_memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        }
        .ok()?;
        self.exports.push((exported, exported_memory));
        self.exports.push((staging, staging_memory));
        Some(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// Allocate a linear dma-buf holding `content` on the producer device, and export its fd.
/// `None` when there is no Vulkan device to allocate on.
fn produce_dmabuf(content: &[u8]) -> Option<(Producer, OwnedFd)> {
    let mut producer = open_producer()?;
    let fd = producer.export(content)?;
    Some((producer, fd))
}

struct SurfaceView {
    handle: DmaBufHandle,
}

impl Render for SurfaceView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(surface(self.handle.clone()).w_full().h_full())
    }
}

/// Mount a view painting `handle`, draw a frame, and return the centre pixel.
fn composite(handle: DmaBufHandle) -> anyhow::Result<[u8; 4]> {
    let text_system = std::sync::Arc::new(CosmicTextSystem::new("fallback"));
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), || {
        gpui::current_headless_renderer()
    });

    let window = cx.open_window(size(px(WIDTH as f32), px(HEIGHT as f32)), |_window, cx| {
        cx.new(|_| SurfaceView { handle })
    })?;
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })?;

    let image = cx.capture_screenshot(window)?;
    Ok(image.get_pixel(WIDTH / 2, HEIGHT / 2).0)
}

/// What a producer negotiates with: the device's own report of the formats and layouts it accepts.
///
/// The report is the importer's closed set, not everything Vulkan knows: a format outside it is
/// absent and a layout outside an entry is refused. Every entry carries linear, because the linear
/// import path predates the modifier extension; a driver modifier appears only where the device took
/// `VK_EXT_image_drm_format_modifier`, which is also the condition the importer refuses tiled
/// buffers without.
#[test]
fn the_device_reports_the_dmabuf_layouts_it_accepts() {
    use gpui::GpuRenderer as _;

    let renderer = match gpui::current_headless_renderer() {
        Ok(Some(renderer)) => renderer,
        Ok(None) | Err(_) => {
            eprintln!("skipping: no headless renderer");
            return;
        }
    };
    let renderer: &dyn std::any::Any = &*renderer;
    let Some(headless) = renderer.downcast_ref::<gpui_wgpu::WgpuHeadlessRenderer>() else {
        eprintln!("skipping: the headless renderer is not wgpu");
        return;
    };
    let (device, _queue) = headless
        .device()
        .expect("the headless renderer has a device");
    let Some(capabilities) = gpui_wgpu::dmabuf_capabilities(&device) else {
        // A machine with no Vulkan driver renders through GL, and the importer has no arm for it;
        // this is the same gap the other dma-buf tests skip on when there is no Vulkan producer.
        eprintln!("skipping: the headless renderer is not Vulkan");
        return;
    };

    let formats: Vec<_> = capabilities
        .formats()
        .iter()
        .map(|entry| entry.format)
        .collect();
    assert_eq!(
        formats,
        vec![
            SurfaceFormatKind::bgra8(),
            SurfaceFormatKind::rgba8(),
            SurfaceFormatKind::nv12(),
            SurfaceFormatKind::nv21(),
        ],
        "the report is the importer's closed set, in its order",
    );

    for format in &formats {
        assert!(
            capabilities.supports(format, DmaBufHandle::LINEAR),
            "{format} must be importable linearly: every GPU can sample its own linear memory",
        );
    }
    assert!(
        !capabilities.supports(&SurfaceFormatKind::i420(), DmaBufHandle::LINEAR),
        "the importer has no I420 arm, so a report that offered one would invite a dropped frame",
    );
    assert_eq!(capabilities.modifiers(&SurfaceFormatKind::i420()), None);

    // Which modifiers exist is the machine's, so this is a shape check, not a hardware assertion:
    // no duplicate layouts, and the list is worth printing when a refusal is investigated.
    for entry in capabilities.formats() {
        let mut unique = entry.modifiers.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            entry.modifiers.len(),
            "duplicate modifiers in the {} entry: {:#x?}",
            entry.format,
            entry.modifiers,
        );
        eprintln!("{} import modifiers: {:#x?}", entry.format, entry.modifiers);
    }
}

/// Lay a row pattern into `content` at `plane` (`offset`, `stride`), honouring the stride — the
/// whole-object layout a dma-buf handle declares, where a row may be padded past the pixels in it.
fn fill_plane(content: &mut [u8], plane: (u64, u32), width: usize, rows: usize, pattern: &[u8]) {
    let base = plane.0 as usize;
    let stride = plane.1 as usize;
    for row in 0..rows {
        let start = base + row * stride;
        for (column, byte) in content[start..start + width].iter_mut().enumerate() {
            *byte = pattern[column % pattern.len()];
        }
    }
}

/// A view that paints nothing — the window an empty surface leaves behind.
struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// The centre pixel of a window with nothing painted in it — what a *dropped* surface leaves behind,
/// and the reference a refusal is checked against: a composited tile would never match it.
fn empty_window_pixel() -> anyhow::Result<[u8; 4]> {
    let text_system = std::sync::Arc::new(CosmicTextSystem::new("fallback"));
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), || {
        gpui::current_headless_renderer()
    });

    let window = cx.open_window(size(px(WIDTH as f32), px(HEIGHT as f32)), |_window, cx| {
        cx.new(|_| EmptyView)
    })?;
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })?;

    let image = cx.capture_screenshot(window)?;
    Ok(image.get_pixel(WIDTH / 2, HEIGHT / 2).0)
}

/// A view painting a same-device texture through `gpu_canvas(..)`, and whether the paint-time
/// callback found a device to make it on.
struct CanvasView {
    colour: [u8; 4],
    painted: Rc<Cell<bool>>,
}

impl Render for CanvasView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let colour = self.colour;
        let painted = self.painted.clone();
        gpu_canvas(move |gpu| {
            let Some(handle) = imported_texture(gpu.try_device::<WgpuRenderer>(), colour) else {
                return;
            };
            painted.set(true);
            gpu.paint_texture(handle, Corners::default(), 1.0, false);
        })
        .size_full()
    }
}

/// Make `colour` a texture on the renderer's own device and wrap it as an [`ImportedTextureHandle`].
/// `None` when the window's renderer lends no device.
fn imported_texture(
    device: Option<(std::sync::Arc<wgpu::Device>, std::sync::Arc<wgpu::Queue>)>,
    colour: [u8; 4],
) -> Option<ImportedTextureHandle> {
    // A producer reaches the renderer's device through the canvas's typed door. A window whose
    // renderer lends none hands back `None` — which is what the headless harness here does.
    let (device, queue) = device?;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface_dmabuf_gpu_canvas"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // sRGB, the colour space `to_imported_handle` requires; `colour` is already sRGB-encoded.
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        // `TEXTURE_BINDING` is what the handle builder checks, `COPY_DST` is how the colour gets in.
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        content.extend_from_slice(&colour);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &content,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: Some(HEIGHT),
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    texture
        .create_view(&wgpu::TextureViewDescriptor::default())
        .to_imported_handle()
        .ok()
}

/// Mount a `gpu_canvas` whose callback fills `colour` on the window's device, draw a frame, and
/// return the centre pixel. `None` when the window's renderer lends no device.
fn composite_canvas(colour: [u8; 4]) -> anyhow::Result<Option<[u8; 4]>> {
    let text_system = std::sync::Arc::new(CosmicTextSystem::new("fallback"));
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), || {
        gpui::current_headless_renderer()
    });

    let painted = Rc::new(Cell::new(false));
    let window = cx.open_window(size(px(WIDTH as f32), px(HEIGHT as f32)), |_window, cx| {
        cx.new(|_| CanvasView {
            colour,
            painted: painted.clone(),
        })
    })?;
    let window: AnyWindowHandle = window.into();

    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let _ = window.draw(cx);
    })?;

    if !painted.get() {
        return Ok(None);
    }
    let image = cx.capture_screenshot(window)?;
    Ok(Some(image.get_pixel(WIDTH / 2, HEIGHT / 2).0))
}

/// The renderer the platform builds for a headless context; `None` where there is no adapter, which
/// is a skip rather than a failure.
fn gpu_available() -> bool {
    gpui::current_headless_renderer().is_ok_and(|renderer| renderer.is_some())
}

#[test]
fn a_single_plane_surface_reads_back_byte_for_byte() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        content.extend_from_slice(&COLOUR);
    }
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::rgba8(),
        DmaBufHandle::LINEAR,
        [DmaBufPlane::new(fd, 0, WIDTH * 4)],
        None,
    );

    let pixel = composite(handle).expect("composite the surface");
    assert_eq!(pixel, COLOUR);
}

/// The renderer's half of the dma-buf handshake: it signals a surface's release descriptor once the
/// submission that sampled it has completed, so a producer knows it may recycle the surface.
#[test]
fn the_renderer_signals_a_surfaces_release_descriptor() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        content.extend_from_slice(&COLOUR);
    }
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };

    // The descriptor a producer waits on; the renderer writes to it once it is done with the buffer.
    let release = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(release >= 0, "create an eventfd");
    // SAFETY: `eventfd` returned a fresh descriptor this process owns.
    let release = unsafe { OwnedFd::from_raw_fd(release) };

    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::rgba8(),
        DmaBufHandle::LINEAR,
        [DmaBufPlane::new(fd, 0, WIDTH * 4)],
        None,
    )
    // The handle takes ownership of the descriptor, so hand it a duplicate and keep the original to
    // wait on.
    .with_release(
        release
            .try_clone()
            .expect("duplicate the release descriptor"),
    );

    let _ = composite(handle).expect("composite the surface");

    // The signalling happens from the completion callback of the frame's submission, so give it a
    // moment rather than expecting it the instant `draw` returns.
    let mut descriptor = libc::pollfd {
        fd: release.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, 5000) };
    assert_eq!(
        ready, 1,
        "the renderer should signal the release descriptor"
    );
}

#[test]
fn a_single_bgra_plane_reads_back_as_the_source_colour() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    // A `Bgra8` buffer stores blue, green, red, alpha, so the same fixture colour goes in
    // channel-swapped; the sampler maps the bytes back to RGBA, so it must come out as `COLOUR`.
    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        content.extend_from_slice(&BGRA_COLOUR);
    }
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::bgra8(),
        DmaBufHandle::LINEAR,
        [DmaBufPlane::new(fd, 0, WIDTH * 4)],
        None,
    );

    let pixel = composite(handle).expect("composite the surface");
    assert_eq!(pixel, COLOUR);
}

/// Linear light is encoded for GPUI's non-sRGB target. The row above asserts the other half: the
/// same bytes declared sRGB (the default) come back unchanged.
#[test]
fn a_linear_bgra_plane_is_encoded_to_srgb() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }

    // A mid-linear value, where the curve is far from the identity: 64/255 linear is about
    // 137/255 sRGB.
    const LINEAR: u8 = 64;
    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        // BGRA with all three channels equal, so the channel order does not matter here.
        content.extend_from_slice(&[LINEAR, LINEAR, LINEAR, 255]);
    }
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::bgra8(),
        DmaBufHandle::LINEAR,
        [DmaBufPlane::new(fd, 0, WIDTH * 4)],
        None,
    )
    .with_transfer(TransferFunction::Linear);

    let pixel = composite(handle).expect("composite the surface");
    let expected = srgb_encode(LINEAR);
    assert!(
        pixel[0].abs_diff(expected) <= 1,
        "linear {LINEAR} should composite as sRGB {expected}, got {pixel:?}",
    );
    assert_eq!(pixel[3], 255);
}

/// sRGB-encode one 8-bit linear sample: the shader's `linear_to_srgb` in Rust, so the expected value
/// is computed the same way the GPU computes it.
fn srgb_encode(linear: u8) -> u8 {
    let linear = linear as f32 / 255.0;
    let encoded = if linear < 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

#[test]
fn a_surface_with_a_signalled_acquire_fence_is_waited_on() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let mut content = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for _ in 0..(WIDTH * HEIGHT) {
        content.extend_from_slice(&COLOUR);
    }
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    // A `sync_file` is readable once its fence signals; a pipe holding a byte has that same shape,
    // so the renderer's wait returns at once and the surface still composites.
    let (read_end, mut write_end) = std::io::pipe().expect("a pipe to stand in for a sync_file");
    write_end.write_all(&[1]).expect("signal the fence");
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::rgba8(),
        DmaBufHandle::LINEAR,
        [DmaBufPlane::new(fd, 0, WIDTH * 4)],
        Some(OwnedFd::from(read_end)),
    );

    let pixel = composite(handle).expect("composite the surface");
    assert_eq!(pixel, COLOUR);
}

/// The shader's BT.601 conversion, on a buffer laid out the way this device lays a linear `NV12` out.
///
/// The layout is the device's, not a fixed canonical one: a producer that asserts the canonical DRM
/// layout is refused on a device that pads it (an NVIDIA 930M puts chroma at 65536), so the fixture
/// asks the device and follows the answer — which is what a real producer must do too.
#[test]
fn an_nv12_surface_converts_through_the_shader() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let Some(mut producer) = open_producer() else {
        eprintln!("skipping: no Vulkan device for the producer");
        return;
    };
    let Some((planes, required)) = producer.native_nv12() else {
        eprintln!("skipping: the producer's device will not make a plain linear NV12 image");
        return;
    };
    let [luma, chroma] = planes;
    // The shared colour in the two-plane form: full-resolution luma, then half-resolution
    // interleaved chroma, carrying the bytes the BT.601 conversion maps back to olive, placed where
    // the device's layout says.
    let (y, cb, cr) = nv12_from_rgb(COLOUR);
    let mut content = vec![0u8; required as usize];
    fill_plane(&mut content, luma, WIDTH as usize, HEIGHT as usize, &[y]);
    fill_plane(
        &mut content,
        chroma,
        WIDTH as usize,
        (HEIGHT / 2) as usize,
        &[cb, cr],
    );
    let Some(fd) = producer.export(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let chroma_fd = fd.try_clone().expect("duplicate the dma-buf");
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::nv12(),
        DmaBufHandle::LINEAR,
        [
            DmaBufPlane::new(fd, luma.0, luma.1),
            DmaBufPlane::new(chroma_fd, chroma.0, chroma.1),
        ],
        None,
    );

    let pixel = composite(handle).expect("composite the surface");
    // The same BT.601 matrix the shader applies, on the CPU, to the bytes the planes carry.
    let expected = ycbcr_to_rgb(
        f32::from(y) / 255.0,
        f32::from(cb) / 255.0,
        f32::from(cr) / 255.0,
    );
    for channel in 0..3 {
        let want = (expected[channel] * 255.0).round() as i32;
        let got = i32::from(pixel[channel]);
        assert!(
            (got - want).abs() <= 2,
            "channel {channel} is {got}, but the shader's matrix predicts {want}",
        );
    }
    assert_eq!(pixel[3], 255, "the surface should be opaque");
}

/// A handle that declares a layout the driver does not produce is refused, not sampled.
///
/// A producer's plane offsets are only readable where the driver would have put them itself: an image
/// created with `ImageDrmFormatModifierExplicitCreateInfoEXT` declares the layout and the driver
/// echoes it back, but a layout the driver did not choose is one it cannot sample — measured on an
/// NVIDIA 930M, sampling such an image never completes and hangs the command processor. This buffer
/// pads its luma plane, so its chroma is at 8192 where the driver's own linear layout puts it at 4096,
/// and the importer drops the surface rather than bind it. The window stays empty, and the fixture
/// colour would never match it.
#[test]
fn a_linear_nv12_surface_declaring_a_layout_the_driver_does_not_produce_is_refused() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let luma_size = (WIDTH * HEIGHT) as usize;
    let chroma_size = (WIDTH / 2 * HEIGHT / 2 * 2) as usize;
    // A whole page past the luma's end, which no driver's own linear layout would choose.
    let chroma_offset = luma_size + 4096;
    let (y, cb, cr) = nv12_from_rgb(COLOUR);

    let mut content = vec![0u8; chroma_offset + chroma_size];
    content[..luma_size].fill(y);
    for pair in content[chroma_offset..].chunks_exact_mut(2) {
        pair[0] = cb;
        pair[1] = cr;
    }

    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let chroma_fd = fd.try_clone().expect("duplicate the dma-buf");
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::nv12(),
        DmaBufHandle::LINEAR,
        [
            DmaBufPlane::new(fd, 0, WIDTH),
            DmaBufPlane::new(chroma_fd, chroma_offset as u64, WIDTH),
        ],
        None,
    );

    let background = empty_window_pixel().expect("capture an empty window");
    let pixel = composite(handle).expect("composite the refused surface");
    assert_eq!(
        pixel, background,
        "the surface was not refused: the handle declares chroma at {chroma_offset}, where this \
         device does not lay its own linear NV12 out. Sampling a layout the driver did not produce \
         reads the padding as chroma (which composites green), and on some devices never completes",
    );
}

/// A buffer too small to hold the layout its handle declares is refused, not bound and sampled.
///
/// The fixture exports exactly the tight `NV12` span — luma then chroma, 6144 bytes of a 64×64 — and
/// the handle then declares its chroma a page *past* the luma, so the image the importer would make
/// spans 10240 bytes of a 6144-byte dma-buf. A renderer that bound it anyway would sample past the end
/// of the buffer (which faults on the GPU rather than compositing wrongly); the importer must drop the
/// surface instead. The window stays empty, and the fixture colour would never match it.
#[test]
fn a_buffer_too_small_for_its_declared_layout_is_refused() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    let luma_size = (WIDTH * HEIGHT) as usize;
    let chroma_size = (WIDTH / 2 * HEIGHT / 2 * 2) as usize;
    // A mid-grey card, so a tile the renderer *did* composite could not be mistaken for the empty
    // background the refusal leaves.
    let content = vec![128u8; luma_size + chroma_size];
    let Some((_producer, fd)) = produce_dmabuf(&content) else {
        eprintln!("skipping: could not allocate a dma-buf");
        return;
    };
    let chroma_fd = fd.try_clone().expect("duplicate the dma-buf");
    let handle = DmaBufHandle::new(
        WIDTH,
        HEIGHT,
        SurfaceFormatKind::nv12(),
        DmaBufHandle::LINEAR,
        [
            DmaBufPlane::new(fd, 0, WIDTH),
            DmaBufPlane::new(chroma_fd, (luma_size + 4096) as u64, WIDTH),
        ],
        None,
    );

    let background = empty_window_pixel().expect("capture an empty window");
    let pixel = composite(handle).expect("composite the refused surface");
    assert_eq!(
        pixel, background,
        "the surface was not refused: a 6144-byte buffer was bound to an image declaring 10240 bytes, \
         which samples the buffer's end",
    );
}

#[test]
fn a_gpu_canvas_composites_a_same_device_texture() {
    if !gpu_available() {
        eprintln!("skipping: no Vulkan adapter for the headless renderer");
        return;
    }
    // The other authoring layer: `gpu_canvas(..)`'s paint callback wants a texture on the renderer's
    // own device, which a producer reaches through the canvas's typed door. The headless harness
    // used here holds its renderer erased as a `SceneRenderer` and does not implement `GpuRenderer`,
    // so no device is lent and the callback paints nothing: skip rather than fail, and say which
    // piece is not reachable from this test.
    let Some(pixel) = composite_canvas(BGRA_COLOUR).expect("composite the canvas") else {
        eprintln!(
            "skipping: the headless window's renderer lends no device — the authoring TestWindow \
             does not implement GpuRenderer — so no same-device texture can be made"
        );
        return;
    };
    assert_eq!(pixel, COLOUR);
}

/// The `fs_surface` matrix, on the CPU: `ycbcr_to_RGB * vec4(y, cb, cr, 1)`.
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
