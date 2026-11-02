//! The Linux module: a dma-buf producer's surface, ordered by a dma-fence.
//!
//! This is the **producer half** of the Linux surface arm, the mirror of `windows.rs`: allocate a
//! shareable image on the producer's own device, export it as a **dma-buf** (one fd per plane, with
//! the DRM modifier and the plane offsets/strides) and hand GPUI the `gpui::DmaBufHandle` the
//! `surface()` element takes. The consumer half is core — `gpui_wgpu` imports under the modifier and
//! samples it — so nothing here opens or waits on a buffer.
//!
//! Today this covers `BGRA8`/`RGBA8` and two-plane `NV12`, either linear or on a driver-tiled
//! modifier the renderer can import ([`Tiling`]), exported with `vkGetMemoryFdKHR` as one dma-buf
//! with a plane per memory plane, and ordered by a [`Fence`] that exports the acquire `sync_file`
//! from `VK_KHR_external_fence_fd`. The payload is the same [`gpui_engine::DmaBufHandle`] either way,
//! so the consumer never changes.
//!
//! A VA-API producer does **not** use this module: `vaExportSurfaceHandle` hands it a dma-buf
//! directly. This is for a producer that has no driver-level export — a `wgpu`/3D engine or another
//! process — and must allocate and export its own.

use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use ash::vk;
use gpui_engine::{DmaBufHandle, DmaBufPlane, SurfaceFormatKind, TransferFunction};

/// The device extensions an image needs to be exported as a dma-buf, tiled or not.
const DEVICE_EXTENSIONS: [&str; 4] = [
    "VK_KHR_external_memory_fd",
    "VK_EXT_external_memory_dma_buf",
    "VK_EXT_image_drm_format_modifier",
    "VK_KHR_external_fence_fd",
];

/// The instance extension that names a physical device's DRM nodes. Enabled on the instance
/// [`GpuDevice::open`] makes, and read through when the renderer's instance carries it too.
const DRM_EXTENSION: &str = "VK_EXT_physical_device_drm";

/// How the producer's image is laid out in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tiling {
    /// A linear buffer: uncompressed, always importable, and the slowest to sample.
    Linear,
    /// A driver-tiled modifier the renderer can import — when the device offers one **and** it is the
    /// consumer's own.
    ///
    /// A DRM modifier is meaningful only to the vendor that defines it: `Y_TILED` is Intel's, NVIDIA
    /// lists its own block, llvmpipe only `LINEAR`. The modifier is picked from the *device's* list,
    /// so tiling is sound only when that device is the one the renderer samples on — which is what
    /// `Adapter::gpu_device` lends, and what [`GpuDevice::open`] does not. On a device the producer
    /// opened itself the request is **refused and the buffer exported linear**, with a warning: the
    /// alternative is a buffer the consumer wedges on rather than refuses.
    Tiled,
}

/// One plane's placement in a buffer whose producer chose the layout — the offsets and strides a
/// [`SharedBuffer::export_at`] export declares, rather than the ones the driver would have picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLayout {
    /// The byte offset of the plane's first row within the exported object.
    pub offset: u64,
    /// The bytes between the start of one row and the next — at least the row's own width.
    pub stride: u32,
}

/// What a producer must declare and allocate for a linear buffer this device will sample: the plane
/// layout the device laid out itself, and the size the buffer must hold.
///
/// Read from [`GpuDevice::linear_layout`], **before** a producer exports. [`native`](Self::native) is
/// the answer to "where will the driver read the planes" — a driver samples only the layout it
/// produced itself, so a producer declares *this* rather than a layout of its own choosing. On a
/// device whose linear `NV12` is the canonical DRM layout it is that layout; on one that pads (an
/// NVIDIA 930M puts chroma at 65536) it is the padded one, and a canonical declaration there is
/// refused rather than sampled. [`required`](Self::required) is the size the buffer must hold for
/// that layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearLayout {
    /// The plane layout the device gives a plain `LINEAR` image of this format and extent — the
    /// offsets and strides to declare. One [`PlaneLayout`] per plane, in plane order.
    pub native: Vec<PlaneLayout>,
    /// The bytes the device's image for [`native`](Self::native) requires: the size the exported
    /// buffer must hold. Larger than the layout's own span where the driver pads beyond the planes.
    pub required: u64,
    /// The bytes the plane layouts the producer proposed (`planes`) span — how far the producer's own
    /// idea is from [`native`](Self::native). Equal to the native span where the two agree.
    pub declared: u64,
}

impl LinearLayout {
    /// Whether a buffer of exactly [`declared`](Self::declared) bytes can back the device's image —
    /// the test the renderer's importer makes. `false` means a producer must pad to
    /// [`required`](Self::required), or declare [`native`](Self::native) if its proposal is short.
    pub fn backed(&self) -> bool {
        self.required <= self.declared
    }
}

/// The GPU a window draws on, as a producer that must render or decode on the **same** GPU names it.
///
/// A dma-buf is importable only on the GPU it was exported from, so a producer that owns its device —
/// a 3D engine, a VA decoder, a camera — has to land on the renderer's GPU. It cannot borrow the
/// renderer's device to do so (a foreign API or process has its own), so what it needs is the GPU's
/// *identity*, in terms it can match from its own instance:
///
/// - [`render_node`](Self::render_node), the DRM render node — what a non-Vulkan producer opens
///   (`vaGetDisplayDRM`, a camera, `gbm`);
/// - [`name`](Self::name), the device name Vulkan reports — what a Vulkan producer matches by.
///
/// Read it inside a `gpu_canvas` with `Adapter::gpu_id`, or, for a producer that runs off the paint,
/// after a canvas has remembered it: [`gpu_id`].
///
/// This is the producer's matchable identity, not the core's `gpui::GpuId`: the latter names an
/// adapter for `gpui::GpuPreference::Device`, while this one is what a foreign API or process can
/// check itself — a Vulkan device name and a DRM render node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuId {
    /// The device name Vulkan reports, e.g. `Intel(R) HD Graphics 520`.
    pub name: String,
    /// The DRM render node the GPU backs — `/dev/dri/renderD128` — when the driver reports one, which
    /// is what a non-Vulkan producer opens. `None` when `VK_EXT_physical_device_drm` is unavailable.
    pub render_node: Option<PathBuf>,
}

impl GpuId {
    /// Read the identity of `physical_device` on `instance`.
    pub(crate) fn of(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Self {
        // SAFETY: both calls only read the device; the structs are filled out and copied.
        unsafe {
            let properties = instance.get_physical_device_properties(physical_device);
            let name = extension_name(&properties.device_name).to_owned();

            let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
            instance.get_physical_device_properties2(physical_device, &mut properties);
            let render_node = (drm.has_render != 0)
                .then(|| PathBuf::from(format!("/dev/dri/renderD{}", drm.render_minor)));

            Self { name, render_node }
        }
    }

    /// The producer-facing identity of an adapter the application enumerated.
    ///
    /// [`gpui::GpuAdapterInfo`] carries the adapter's PCI ids (the permanent part of its `GpuId`)
    /// and the Vulkan device name; the render node a non-Vulkan producer opens comes from
    /// `VK_EXT_physical_device_drm`, which no `wgpu` adapter exposes. When the platform's session
    /// handle is a DRM render node — which is the Linux backend's usual answer — it names the exact
    /// card this process enumerated, so the node is taken from it directly; otherwise this opens a
    /// short-lived instance and matches the physical device by PCI id, creating no device, and
    /// returns `None` when no Vulkan device matches (an OpenGL adapter, or a name-only backend).
    ///
    /// Without a session handle the permanent pair is the match, so two identical cards resolve to
    /// the first matching Vulkan device.
    ///
    /// This is how an application turns a [`gpui::GpuPreference::Custom`] choice into the identity a
    /// producer uses: the preference picks the renderer's adapter, this names the same GPU to the
    /// producer, and [`Adapter::gpu_id`] confirms it once a canvas exists.
    #[cfg(feature = "gpui")]
    pub fn from_adapter(info: &gpui::GpuAdapterInfo) -> Option<Self> {
        // The session handle is the exact adapter in this process. On Linux it is the render node
        // itself, so nothing has to be re-derived through an instance and a PCI match.
        if let Some(gpui::SessionGpuId::Drm { render_minor }) = info.id.session {
            return Some(Self {
                name: info.name.clone(),
                render_node: Some(PathBuf::from(format!("/dev/dri/renderD{render_minor}"))),
            });
        }

        // A backend that reports no PCI ids (OpenGL often reports both as zero) cannot be matched
        // this way, and matching zero against zero would name the wrong GPU.
        let vendor_id = info.vendor_id()?;
        let device_id = info.device_id()?;
        if device_id == 0 {
            return None;
        }
        // See `LOADER`: the loader's global entry points must not be initialised concurrently.
        let _loading = LOADER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = unsafe { ash::Entry::load() }.ok()?;
        let instance = create_instance(&entry).ok()?;
        let devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
        let physical_device = devices.into_iter().find(|physical_device| {
            // SAFETY: `physical_device` came from this instance.
            let properties =
                unsafe { instance.get_physical_device_properties(*physical_device) };
            properties.vendor_id == vendor_id && properties.device_id == device_id
        })?;
        Some(Self::of(&instance, physical_device))
    }

    /// Whether `info` names this GPU.
    ///
    /// The platform reports the same Vulkan device name, so the name is the join a
    /// [`gpui::GpuPreference::Custom`] predicate can use without opening anything. Use
    /// [`from_adapter`](Self::from_adapter) when the render node is needed as well.
    #[cfg(feature = "gpui")]
    pub fn matches_adapter(&self, info: &gpui::GpuAdapterInfo) -> bool {
        self.name == info.name
    }
}

/// The renderer's GPU, remembered when a canvas last read it — for a producer that runs off the
/// paint, which is every decoder and capture loop.
static RENDERER_GPU: OnceLock<GpuId> = OnceLock::new();

/// Remember `id` as the GPU the window draws on, for [`gpu_id`] to hand a producer off the paint.
///
/// First write wins: a process is assumed to draw through one GPU, which is what the renderer's own
/// adapter steering assumes too.
pub fn remember(id: GpuId) {
    let _ = RENDERER_GPU.set(id);
}

/// The remembered GPU, or `None` until a `gpu_canvas` has read it with `Adapter::gpu_id`.
pub fn gpu_id() -> Option<GpuId> {
    RENDERER_GPU.get().cloned()
}

/// A Vulkan device a producer renders on and exports from.
///
/// [`GpuDevice::open`] opens its own instance and device on a node that can export a dma-buf. Where a
/// window's renderer is known, `Adapter::gpu_device` lends its `vk::PhysicalDevice` instead and
/// [`GpuDevice::on`] makes a second logical device on it — the Linux counterpart of a LUID match — so
/// the export lands on the GPU the window draws through; `open` stays the fallback.
pub struct GpuDevice {
    // Held so an instance this value made outlives the device; `None` when the instance is the
    // renderer's, lent through the `adapter`.
    _entry: Option<ash::Entry>,
    instance: ash::Instance,
    /// Whether the instance is this value's to destroy, or the renderer's.
    owns_instance: bool,
    /// Whether this device is the renderer's own (`GpuDevice::on`) rather than one the producer
    /// opened (`GpuDevice::open`). [`Tiling::Tiled`] is honoured only on the former.
    renderer_bound: bool,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    external_memory_fd: ash::khr::external_memory_fd::Device,
    drm_format_modifier: ash::ext::image_drm_format_modifier::Device,
    queue: vk::Queue,
    queue_family_index: u32,
    command_pool: vk::CommandPool,
}

/// Serialises the Vulkan loader's **global** entry points, which only [`GpuDevice::open`] calls.
///
/// `vkEnumerateInstanceExtensionProperties` and its siblings are instance-independent: the loader scans
/// and initialises the installed ICDs on the first call, and that scan is not safe to run from two
/// threads at once. A parallel test run — several `open()`s at the same instant — has been seen to crash
/// inside the loader on a null callee, at exactly that first call. Holding a process-wide lock across
/// the load and the enumeration makes the first use single-threaded, which is all the loader needs;
/// the renderer-bound [`GpuDevice::on`] creates no instance and takes no lock.
static LOADER: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl GpuDevice {
    /// Open a device that can export a dma-buf, or an error if none on this machine can.
    ///
    /// It is **not** the renderer's device, and it does not pretend to be: [`Tiling::Tiled`] is
    /// refused on it and the buffer is exported linear, with a warning. Borrow the renderer's device
    /// with `Adapter::gpu_device` when there is one to borrow from.
    pub fn open() -> Result<Self> {
        // See `LOADER`: the loader's global entry points are not safe to initialise concurrently, and
        // this is the only path that reaches them.
        let _loading = LOADER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let entry = unsafe { ash::Entry::load() }.map_err(|e| anyhow!("load libvulkan: {e}"))?;
        let instance = create_instance(&entry)?;

        let (physical_device, queue_family_index) = pick_device(&instance).ok_or_else(|| {
            anyhow!("no Vulkan device exports a dma-buf (needs {DEVICE_EXTENSIONS:?})")
        })?;

        Self::assemble(
            Some(entry),
            instance,
            true,
            false,
            physical_device,
            queue_family_index,
        )
    }

    /// A device on `physical_device` of the renderer's `instance` — the `adapter`'s answer on Linux,
    /// which puts the export on the GPU the window draws on rather than whichever one `open` finds.
    pub fn on(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Result<Self> {
        ensure_extensions(instance, physical_device)?;
        let queue_family_index = render_family(instance, physical_device)
            .ok_or_else(|| anyhow!("the renderer's device has no graphics queue family"))?;
        Self::assemble(
            None,
            instance.clone(),
            false,
            true,
            physical_device,
            queue_family_index,
        )
    }

    /// Build on an already-chosen physical device and queue family.
    fn assemble(
        entry: Option<ash::Entry>,
        instance: ash::Instance,
        owns_instance: bool,
        renderer_bound: bool,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
    ) -> Result<Self> {
        let device = create_device(&instance, physical_device, queue_family_index)?;

        let external_memory_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let drm_format_modifier =
            ash::ext::image_drm_format_modifier::Device::new(&instance, &device);

        // SAFETY: the device is live; the queue and pool belong to it and are destroyed with it.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family_index);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|e| anyhow!("create the command pool: {e:?}"))?;

        Ok(Self {
            _entry: entry,
            instance,
            owns_instance,
            renderer_bound,
            physical_device,
            device,
            external_memory_fd,
            drm_format_modifier,
            queue,
            queue_family_index,
            command_pool,
        })
    }

    /// The Vulkan device a producer builds its pipelines on and submits to — the renderer's own GPU,
    /// a second logical device on it so the producer's queue never races the renderer's.
    pub fn device(&self) -> &ash::Device {
        &self.device
    }

    fn instance(&self) -> &ash::Instance {
        &self.instance
    }

    fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    fn external_memory_fd(&self) -> &ash::khr::external_memory_fd::Device {
        &self.external_memory_fd
    }

    fn drm_format_modifier(&self) -> &ash::ext::image_drm_format_modifier::Device {
        &self.drm_format_modifier
    }

    /// The queue [`device`](Self::device) submits on.
    pub fn queue(&self) -> vk::Queue {
        self.queue
    }

    fn command_pool(&self) -> vk::CommandPool {
        self.command_pool
    }

    /// The queue family [`queue`](Self::queue) belongs to.
    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    /// The GPU this device is on — the identity a producer that must match the renderer's GPU reads.
    pub fn gpu_id(&self) -> GpuId {
        GpuId::of(&self.instance, self.physical_device)
    }

    /// The first memory type the image allows that carries every flag in `required` — for a producer
    /// allocating its own buffers and attachments on this device.
    pub fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32> {
        // SAFETY: the call reads the physical device's memory properties.
        let properties = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        (0..properties.memory_type_count)
            .find(|i| {
                bits & (1u32 << i) != 0
                    && properties.memory_types[*i as usize]
                        .property_flags
                        .contains(required)
            })
            .ok_or_else(|| anyhow!("no memory type carrying {required:?} for the image"))
    }

    /// What a producer must declare and how big its buffer must be, **before** it exports: the plane
    /// layout this device gives a plain `LINEAR` image, and the size the exported buffer must hold.
    ///
    /// [`native`](LinearLayout::native) is read from a plain `LINEAR` image — the layout the device
    /// laid out itself, and the only one its importer will sample — and
    /// [`required`](LinearLayout::required) is the memory the image the importer builds from that
    /// layout needs, so the answer is what the import will meet rather than a guess. `planes` is the
    /// layout the producer *proposed*, and [`declared`](LinearLayout::declared) its span, so a
    /// producer can see how far its own idea is from the device's; an error is a device that will not
    /// make such an image at all.
    pub fn linear_layout(
        &self,
        format: SurfaceFormatKind,
        width: u32,
        height: u32,
        planes: &[PlaneLayout],
    ) -> Result<LinearLayout> {
        let vk_format = vk_format(format)?;
        let geometry = plane_geometry(format, width, height);
        anyhow::ensure!(!geometry.is_empty(), "{format:?} has no plane layout to report on");
        anyhow::ensure!(
            geometry.len() == planes.len(),
            "{format:?} has {} planes, but {} layouts were given",
            geometry.len(),
            planes.len(),
        );
        let declared = declared_span(&geometry, planes)?;

        // The layout the device laid out itself — the only one its importer samples, and so what a
        // producer must declare. Read from a plain image, not from one created with the proposal:
        // a driver echoes an explicit declaration back, so that query could never disagree.
        let native = self.native_plane_layouts(vk_format, width, height, geometry.len())?;

        // The image the importer will build from a handle declaring `native`, so `required` is the
        // memory the import will meet rather than a guess.
        let layouts: Vec<vk::SubresourceLayout> = native
            .iter()
            .map(|plane| {
                vk::SubresourceLayout::default()
                    .offset(plane.offset)
                    .row_pitch(u64::from(plane.stride))
            })
            .collect();
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(DmaBufHandle::LINEAR)
            .plane_layouts(&layouts);
        let info = vk::ImageCreateInfo::default()
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
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut explicit);
        // SAFETY: `info` chains locals that outlive the call, and the image is destroyed below.
        let image = unsafe { self.device.create_image(&info, None) }.map_err(|error| {
            anyhow!("the device will not make a {format:?} image with these plane layouts: {error:?}")
        })?;
        // SAFETY: the image is live and bound to no memory yet.
        let required = unsafe { self.device.get_image_memory_requirements(image) }.size;
        unsafe { self.device.destroy_image(image, None) };
        Ok(LinearLayout {
            native,
            required,
            declared,
        })
    }

    /// The plane layout the device gives a plain, unbound `LINEAR` image of `format` — the offsets and
    /// strides it laid out itself, which are the only ones its importer samples.
    ///
    /// No memory is allocated: `vkCreateImage` binds none until `vkBindImageMemory`, so this is a
    /// create, a query per plane, and a destroy. The aspects are a plain image's (`COLOR`, or
    /// `PLANE_0`/`PLANE_1`), not a modifier image's memory planes — mixing them returns nonsense.
    fn native_plane_layouts(
        &self,
        vk_format: vk::Format,
        width: u32,
        height: u32,
        plane_count: usize,
    ) -> Result<Vec<PlaneLayout>> {
        let info = vk::ImageCreateInfo::default()
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
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `info` chains nothing, and the image is destroyed below.
        let image = unsafe { self.device.create_image(&info, None) }
            .map_err(|error| anyhow!("the device will not make a plain {vk_format:?} image: {error:?}"))?;
        let mut native = Vec::with_capacity(plane_count);
        for plane in 0..plane_count {
            // SAFETY: `image` is live and bound to no memory; the call only reads its layout.
            let layout = unsafe {
                self.device.get_image_subresource_layout(
                    image,
                    vk::ImageSubresource {
                        aspect_mask: plane_aspect(plane, plane_count),
                        mip_level: 0,
                        array_layer: 0,
                    },
                )
            };
            native.push(PlaneLayout {
                offset: layout.offset,
                stride: layout.row_pitch as u32,
            });
        }
        // SAFETY: the image was created above and is bound to no memory.
        unsafe { self.device.destroy_image(image, None) };
        Ok(native)
    }
}

impl Drop for GpuDevice {
    fn drop(&mut self) {
        // SAFETY: the pool and device were made by this value; the instance only if it is ours.
        unsafe {
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            if self.owns_instance {
                self.instance.destroy_instance(None);
            }
        }
    }
}

/// The Vulkan handles an export needs, whether the producer opened the device
/// ([`GpuDevice`]) or borrowed one that already exists — a wgpu device's own `VkDevice`.
///
/// Only what `vkCreateImage`, the modifier query and the memory allocation read is here; the
/// queue and command pool belong to [`GpuDevice`]'s submission helpers and are not part of an
/// export.
pub(crate) trait ExportDevice {
    fn instance(&self) -> &ash::Instance;
    fn physical_device(&self) -> vk::PhysicalDevice;
    fn device(&self) -> &ash::Device;
    fn external_memory_fd(&self) -> &ash::khr::external_memory_fd::Device;
    fn drm_format_modifier(&self) -> &ash::ext::image_drm_format_modifier::Device;

    /// Whether the device is the renderer's own, which is what makes a DRM modifier's importability
    /// knowable. [`Tiling::Tiled`] is honoured only when this is true.
    fn renderer_bound(&self) -> bool;

    /// The first memory type the image allows that carries every flag in `required`.
    fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32> {
        // SAFETY: the call reads the physical device's memory properties.
        let properties = unsafe {
            self.instance()
                .get_physical_device_memory_properties(self.physical_device())
        };
        (0..properties.memory_type_count)
            .find(|i| {
                bits & (1u32 << i) != 0
                    && properties.memory_types[*i as usize]
                        .property_flags
                        .contains(required)
            })
            .ok_or_else(|| anyhow!("no memory type carrying {required:?} for the image"))
    }
}

impl ExportDevice for GpuDevice {
    fn instance(&self) -> &ash::Instance {
        GpuDevice::instance(self)
    }

    fn physical_device(&self) -> vk::PhysicalDevice {
        GpuDevice::physical_device(self)
    }

    fn device(&self) -> &ash::Device {
        GpuDevice::device(self)
    }

    fn external_memory_fd(&self) -> &ash::khr::external_memory_fd::Device {
        GpuDevice::external_memory_fd(self)
    }

    fn drm_format_modifier(&self) -> &ash::ext::image_drm_format_modifier::Device {
        GpuDevice::drm_format_modifier(self)
    }

    fn renderer_bound(&self) -> bool {
        self.renderer_bound
    }

    fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32> {
        GpuDevice::memory_type(self, bits, required)
    }
}

/// Allocate an image backed by an exported dma-buf: the image, its memory, and the handle the
/// renderer imports under.
///
/// `vk_format` is the Vulkan form of `format`. It is a parameter rather than derived here because a
/// caller that renders with a GPU API writes through that API's sRGB form (`_SRGB`), while a caller
/// that uploads already-encoded bytes uses `_UNORM` — both carry the same DRM layout and the same
/// [`SurfaceFormatKind`].
///
/// `tiling` is honoured as [`SharedBuffer::new`] honours it: a modifier is picked only where the
/// device's layout is the consumer's (`renderer_bound`), otherwise the buffer is linear with a
/// warning.
pub(crate) fn allocate_export_image<D: ExportDevice + ?Sized>(
    device: &D,
    width: u32,
    height: u32,
    format: SurfaceFormatKind,
    vk_format: vk::Format,
    tiling: Tiling,
    rendered: bool,
) -> Result<(vk::Image, vk::DeviceMemory, DmaBufHandle)> {
    let plane_count = format.plane_count();
    let usage = usage_for(rendered);
    let required = required_features(usage);
    let modifier = match tiling {
        Tiling::Linear => None,
        Tiling::Tiled if !device.renderer_bound() => {
            // A modifier is a claim about the memory layout of a GPU the consumer shares, and only
            // the renderer's own device can make it truthfully — a different vendor's modifier is
            // not importable at all. Refusing here exports a buffer the consumer can sample, where
            // honouring it could wedge the device instead.
            log::warn!(
                "gpui_interop: Tiling::Tiled needs the renderer's own device; this one was opened \
                 by the producer, so the modifier it would pick is not known to be importable. \
                 Exporting a linear buffer — borrow the renderer's device with \
                 `Adapter::gpu_device` to tile."
            );
            None
        }
        Tiling::Tiled => {
            let modifier = tiled_modifier(device, vk_format, required);
            if modifier.is_none() {
                log::warn!(
                    "gpui_interop: this device offers no {format:?} modifier the renderer can \
                     import; exporting a linear buffer"
                );
            }
            modifier
        }
    };

    ensure_image_supported(device, vk_format, width, height, usage, required, modifier)?;

    let image = create_image(device, vk_format, width, height, usage, modifier)?;

    let requirements = unsafe { device.device().get_image_memory_requirements(image) };
    let memory_type_index = device.memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )?;

    let mut export = vk::ExportMemoryAllocateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    // A dedicated allocation, as the two working dma-buf paths here make — `gpui_wgpu`'s importer
    // and `gpui_va`'s tiled fill. An image that is imported or exported wants one, and a driver is
    // allowed to require it.
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index)
        .push_next(&mut export)
        .push_next(&mut dedicated);

    // SAFETY: the allocation chains a local `export`, the size and type come from the image, and
    // the memory is freed by the caller on every path.
    let memory = match unsafe { device.device().allocate_memory(&allocate_info, None) } {
        Ok(memory) => memory,
        Err(e) => {
            unsafe { device.device().destroy_image(image, None) };
            return Err(anyhow!("allocate the image memory: {e:?}"));
        }
    };
    // SAFETY: `memory` was allocated for `image` (same requirements) and is bound once.
    if let Err(e) = unsafe { device.device().bind_image_memory(image, memory, 0) } {
        unsafe { device.device().free_memory(memory, None) };
        unsafe { device.device().destroy_image(image, None) };
        return Err(anyhow!("bind the image memory: {e:?}"));
    }

    let result = (|| -> Result<DmaBufHandle> {
        // The modifier the driver actually chose, which is what the renderer imports under.
        let drm = modifier.is_some();
        let modifier = match modifier {
            Some(_) => {
                let mut properties = vk::ImageDrmFormatModifierPropertiesEXT::default();
                // SAFETY: `image` is live and bound.
                unsafe {
                    device
                        .drm_format_modifier()
                        .get_image_drm_format_modifier_properties(image, &mut properties)
                }
                .map_err(|e| anyhow!("read the image's modifier: {e:?}"))?;
                properties.drm_format_modifier
            }
            None => DmaBufHandle::LINEAR,
        };

        // SAFETY: the fd is freshly exported for this memory, and takes ownership below.
        let fd = unsafe {
            device.external_memory_fd().get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        }
        .map_err(|e| anyhow!("export the dma-buf fd: {e:?}"))?;
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut planes = Vec::with_capacity(plane_count);
        for plane in 0..plane_count {
            // SAFETY: `image` is live and the subresource names `plane`.
            let layout = unsafe {
                device.device().get_image_subresource_layout(
                    image,
                    vk::ImageSubresource {
                        aspect_mask: subresource_aspect(drm, plane, plane_count),
                        mip_level: 0,
                        array_layer: 0,
                    },
                )
            };
            let plane_fd = fd
                .try_clone()
                .map_err(|e| anyhow!("duplicate the dma-buf fd for plane {plane}: {e}"))?;
            planes.push(DmaBufPlane::new(
                plane_fd,
                layout.offset,
                layout.row_pitch as u32,
            ));
        }
        Ok(DmaBufHandle::new(
            width, height, format, modifier, planes, None,
        ))
    })();

    match result {
        Ok(handle) => Ok((image, memory, handle)),
        Err(error) => {
            unsafe { device.device().free_memory(memory, None) };
            unsafe { device.device().destroy_image(image, None) };
            Err(error)
        }
    }
}

/// A committed, shareable surface — the producer's render target, or bytes pushed at a chosen
/// layout, exported as a dma-buf.
///
/// The memory behind it is what the exported fd names, so this value must outlive every use of the
/// [`DmaBufHandle`] it handed out: dropping it frees the memory the renderer is still sampling. A
/// producer keeps it for as long as the surface is live — a pool keeps a ring of them.
///
/// It also frees through the [`GpuDevice`] it was made on, so that device must outlive it — and,
/// because Rust drops a struct's fields in declaration order, a type holding both must declare the
/// buffers *before* the device.
pub struct SharedBuffer {
    device: ash::Device,
    backing: Backing,
    memory: vk::DeviceMemory,
    handle: DmaBufHandle,
}

/// What the exported memory backs: the image [`SharedBuffer::new`] and [`SharedBuffer::render_target`]
/// allocate, which a pipeline draws into or a `fill` uploads through, or the plain buffer
/// [`SharedBuffer::export_at`] allocates to hold bytes at a plane layout the producer declared. Only
/// the image has a `vk::Image`; a pushed buffer is memory the consumer samples through its own view.
enum Backing {
    Image(vk::Image),
    Buffer(vk::Buffer),
}

impl SharedBuffer {
    /// Allocate a shareable image of `width` × `height` in `format`, on `device`, with `tiling`.
    ///
    /// `format` may be a single-plane RGB format ([`SurfaceFormatKind::bgra8`] or
    /// [`SurfaceFormatKind::rgba8`]) or two-plane [`SurfaceFormatKind::nv12`]; a format with no
    /// Vulkan mapping, or one the device cannot make in the chosen tiling, is refused before
    /// `vkCreateImage` would answer a bare `VK_ERROR_FORMAT_NOT_SUPPORTED`.
    /// [`Tiling::Tiled`] asks for a driver modifier the renderer can import; it is honoured only on
    /// the renderer's **own** device and falls back to linear — with a warning — where the device
    /// offers no such modifier or is one the producer opened itself. `handle().modifier` is what was
    /// in fact chosen.
    pub fn new(
        device: &GpuDevice,
        width: u32,
        height: u32,
        format: SurfaceFormatKind,
        tiling: Tiling,
    ) -> Result<Self> {
        Self::allocate(device, width, height, format, tiling, false)
    }

    /// Allocate a shareable image a producer **renders into** — [`new`](Self::new) with
    /// `COLOR_ATTACHMENT` added, which a pipeline needs and a linear image cannot carry.
    ///
    /// Because linear tiling cannot be rendered to, a rendered buffer must be [`Tiling::Tiled`]; where
    /// the device offers no importable modifier this is refused rather than falling back to a linear
    /// image that cannot be drawn into.
    pub fn render_target(
        device: &GpuDevice,
        width: u32,
        height: u32,
        format: SurfaceFormatKind,
        tiling: Tiling,
    ) -> Result<Self> {
        Self::allocate(device, width, height, format, tiling, true)
    }

    /// Export `content` as a linear dma-buf whose planes lie at the layout **the producer declares**.
    ///
    /// This is the *producer-pushed* contract, the complement of [`new`](Self::new): where `new` lets
    /// the driver choose the image's layout and reports it back in [`handle`](Self::handle), this
    /// exports bytes the producer has already placed, and the handle carries the offsets and strides
    /// given here. It is the shape a producer whose layout comes from somewhere other than
    /// `vkGetImageSubresourceLayout` needs — a decoder, a capture pipeline, a canonical DRM `NV12`
    /// layout — and the one a consumer that honours the handle reads correctly where an image the
    /// driver laid out would not: an NVIDIA linear `NV12` pads luma to 64 KiB, so a producer placing
    /// chroma directly after luma at 16384 would otherwise have it read from padding.
    ///
    /// `planes` holds one [`PlaneLayout`] per plane of `format`, in plane order; `content` is the
    /// whole object — the plane bytes in place, gaps and all — and its length is the exported size. A
    /// layout whose rows do not fit inside `content` is refused. The memory is device-local (a dma-buf
    /// is the memory real frames live in) and filled by a staging copy on the GPU, since the NVIDIA
    /// driver faults on a `HOST_VISIBLE` exportable allocation; the buffer is ready to hand over when
    /// this returns. Unlike [`new`](Self::new) it has no image — [`image`](Self::image) is `None` — so
    /// [`clear`](Self::clear) and [`fill`](Self::fill) do not apply.
    pub fn export_at(
        device: &GpuDevice,
        width: u32,
        height: u32,
        format: SurfaceFormatKind,
        planes: &[PlaneLayout],
        content: &[u8],
    ) -> Result<Self> {
        let geometry = plane_geometry(format, width, height);
        if geometry.is_empty() {
            return Err(anyhow!("{format:?} has no plane layout to export"));
        }
        if geometry.len() != planes.len() {
            return Err(anyhow!(
                "{format:?} has {} planes, but {} layouts were given",
                geometry.len(),
                planes.len(),
            ));
        }
        check_plane_layouts(&geometry, planes, content.len())?;

        let size = content.len() as u64;
        let mut external = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // SAFETY: created, filled and exported once; the memory is freed when the returned value drops.
        let buffer = unsafe {
            device.device().create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .push_next(&mut external),
                None,
            )
        }
        .map_err(|e| anyhow!("create the exported buffer: {e:?}"))?;

        let requirements = unsafe { device.device().get_buffer_memory_requirements(buffer) };
        let properties = unsafe {
            device
                .instance()
                .get_physical_device_memory_properties(device.physical_device())
        };
        let Some(memory_type_index) =
            export_memory_type(&properties, requirements.memory_type_bits)
        else {
            unsafe { device.device().destroy_buffer(buffer, None) };
            return Err(anyhow!("no memory type for the exported buffer"));
        };

        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // A dedicated allocation, as the image paths and the consumer's importer make; a driver is
        // allowed to require one, and NVIDIA needs it for a linear export to be sampled at all.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        // SAFETY: the allocation chains locals that outlive the call, and its size and type come from
        // the buffer.
        let memory = match unsafe {
            device.device().allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type_index)
                    .push_next(&mut export)
                    .push_next(&mut dedicated),
                None,
            )
        } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe { device.device().destroy_buffer(buffer, None) };
                return Err(anyhow!("allocate the exported buffer's memory: {e:?}"));
            }
        };
        // SAFETY: `memory` was allocated for `buffer` (same requirements) and is bound once.
        if let Err(e) = unsafe { device.device().bind_buffer_memory(buffer, memory, 0) } {
            unsafe { device.device().free_memory(memory, None) };
            unsafe { device.device().destroy_buffer(buffer, None) };
            return Err(anyhow!("bind the exported buffer: {e:?}"));
        }

        // The bytes live in device-local memory the CPU cannot map, so they are staged and copied on
        // the GPU rather than mapped in place. Everything after this point frees both on failure.
        let fd = (|| -> Result<OwnedFd> {
            let staging = Staging::new(device, size)?;
            staging.write(&[content])?;
            submit_once(device, |command| {
                // SAFETY: `command` is recording, and both buffers live on `device`; the copy covers
                // the whole of each.
                unsafe {
                    device.device().cmd_copy_buffer(
                        command,
                        staging.buffer,
                        buffer,
                        &[vk::BufferCopy::default().size(size)],
                    );
                }
            })?;
            // SAFETY: the fd is freshly exported for this memory, and takes ownership below.
            let raw = unsafe {
                device.external_memory_fd().get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
                )
            }
            .map_err(|e| anyhow!("export the dma-buf fd: {e:?}"))?;
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        })();
        let fd = match fd {
            Ok(fd) => fd,
            Err(e) => {
                unsafe { device.device().free_memory(memory, None) };
                unsafe { device.device().destroy_buffer(buffer, None) };
                return Err(e);
            }
        };

        let mut described = Vec::with_capacity(planes.len());
        for (plane, layout) in planes.iter().enumerate() {
            match fd.try_clone() {
                Ok(plane_fd) => {
                    described.push(DmaBufPlane::new(plane_fd, layout.offset, layout.stride))
                }
                Err(e) => {
                    unsafe { device.device().free_memory(memory, None) };
                    unsafe { device.device().destroy_buffer(buffer, None) };
                    return Err(anyhow!("duplicate the dma-buf fd for plane {plane}: {e}"));
                }
            }
        }
        let handle =
            DmaBufHandle::new(width, height, format, DmaBufHandle::LINEAR, described, None);

        Ok(Self {
            device: device.device().clone(),
            backing: Backing::Buffer(buffer),
            memory,
            handle,
        })
    }

    /// Allocate the shareable image; `rendered` adds `COLOR_ATTACHMENT` to the usage.
    fn allocate(
        device: &GpuDevice,
        width: u32,
        height: u32,
        format: SurfaceFormatKind,
        tiling: Tiling,
        rendered: bool,
    ) -> Result<Self> {
        let vk_format = vk_format(format)?;
        let (image, memory, handle) =
            allocate_export_image(device, width, height, format, vk_format, tiling, rendered)?;
        Ok(Self {
            device: device.device().clone(),
            backing: Backing::Image(image),
            memory,
            handle,
        })
    }

    /// Declare how this buffer's values relate to light, for the renderer's compositor.
    ///
    /// A producer whose pipeline writes **linear** light — a 3D engine's render target, a physically
    /// based pass — says so here, and GPUI encodes it for its display-encoded target. The default
    /// follows the format (`Srgb` for RGB, the video curve for YCbCr), so an sRGB producer need not
    /// call this.
    pub fn with_transfer(mut self, transfer: TransferFunction) -> Self {
        self.handle.transfer = transfer;
        self
    }

    /// The description the `surface()` element takes. Cheap to clone — the planes share their fds.
    pub fn handle(&self) -> DmaBufHandle {
        self.handle.clone()
    }

    /// The producer's own render target, for a buffer [`new`](Self::new) or
    /// [`render_target`](Self::render_target) made. `None` for one [`export_at`](Self::export_at)
    /// pushed, which backs a plain buffer rather than an image.
    pub fn image(&self) -> Option<vk::Image> {
        match self.backing {
            Backing::Image(image) => Some(image),
            Backing::Buffer(_) => None,
        }
    }

    /// Clear the image to a solid `rgba`, ready to hand over — the producer's simplest render, and
    /// enough for a demo or a fixture. Single-plane RGB formats only: a multi-plane YCbCr buffer has
    /// no one clear colour, so [`fill`](Self::fill) is the way to write one.
    ///
    /// It waits for the clear on the CPU rather than handing back an acquire fence, so the buffer is
    /// ready when this returns. A producer with real work signals a [`Fence`] from its own submission
    /// instead.
    pub fn clear(&self, device: &GpuDevice, rgba: [f32; 4]) -> Result<()> {
        let Backing::Image(image) = self.backing else {
            return Err(anyhow!(
                "a buffer pushed with export_at is written at export; it has no image to clear"
            ));
        };
        submit_once(device, |buffer| {
            // SAFETY: `buffer` is recording, and the image belongs to `device` and this value.
            unsafe {
                let to_transfer = image_barrier(
                    image,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ,
                );
                device.device().cmd_pipeline_barrier(
                    buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                device.device().cmd_clear_color_image(
                    buffer,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: rgba },
                    &[vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }],
                );
                let to_general = image_barrier(
                    image,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ,
                );
                device.device().cmd_pipeline_barrier(
                    buffer,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_general],
                );
            }
        })
    }

    /// Fill the image from host `planes`, ready to hand over — the producer's render for a format
    /// [`clear`](Self::clear) cannot write, because it has no one clear colour.
    ///
    /// `planes` holds one slice per plane of the format, in plane order and tightly packed at each
    /// plane's own resolution: for [`SurfaceFormatKind::nv12`], the luma plane (`width` × `height`
    /// bytes) then the interleaved chroma (`width/2` × `height/2` × 2 bytes). A plane of the wrong
    /// length, or a count the format does not have, is refused. Like `clear` it waits on the CPU
    /// rather than handing back an acquire fence, so the buffer is ready when this returns.
    pub fn fill(&self, device: &GpuDevice, planes: &[&[u8]]) -> Result<()> {
        let Backing::Image(image) = self.backing else {
            return Err(anyhow!(
                "a buffer pushed with export_at is filled at export; it has no image to fill"
            ));
        };
        let geometry = plane_geometry(self.handle.format, self.handle.width, self.handle.height);
        if geometry.is_empty() {
            return Err(anyhow!(
                "{:?} has no plane geometry to fill",
                self.handle.format
            ));
        }
        if geometry.len() != planes.len() {
            return Err(anyhow!(
                "{:?} has {} planes, but {} were given",
                self.handle.format,
                geometry.len(),
                planes.len(),
            ));
        }
        let mut offsets = Vec::with_capacity(geometry.len());
        let mut total = 0u64;
        for (plane, (bytes, _, _)) in geometry.iter().enumerate() {
            if planes[plane].len() != *bytes {
                return Err(anyhow!(
                    "plane {plane} is {} bytes; {:?} at {}×{} wants {bytes}",
                    planes[plane].len(),
                    self.handle.format,
                    self.handle.width,
                    self.handle.height,
                ));
            }
            offsets.push(total);
            total += *bytes as u64;
        }

        let plane_count = geometry.len();
        // A DRM modifier image names its planes by memory plane in a *barrier* but by plane in a
        // *copy*; the two helpers carry that split.
        let drm = self.handle.modifier != DmaBufHandle::LINEAR;
        let regions: Vec<vk::BufferImageCopy> = geometry
            .iter()
            .enumerate()
            .map(|(plane, &(_, width, height))| {
                vk::BufferImageCopy::default()
                    .buffer_offset(offsets[plane])
                    .buffer_row_length(width)
                    .buffer_image_height(height)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: plane_aspect(plane, plane_count),
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
            })
            .collect();

        let staging = Staging::new(device, total)?;
        staging.write(planes)?;
        submit_once(device, |buffer| {
            // SAFETY: `buffer` is recording; the image and the staging buffer are live, and the
            // copy's offsets and extents were checked against the image above.
            unsafe {
                let to_transfer: Vec<_> = (0..plane_count)
                    .map(|plane| {
                        image_barrier(
                            image,
                            subresource_aspect(drm, plane, plane_count),
                            vk::ImageLayout::UNDEFINED,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::AccessFlags::empty(),
                            vk::AccessFlags::TRANSFER_WRITE,
                        )
                    })
                    .collect();
                device.device().cmd_pipeline_barrier(
                    buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &to_transfer,
                );
                device.device().cmd_copy_buffer_to_image(
                    buffer,
                    staging.buffer,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &regions,
                );
                let to_general: Vec<_> = (0..plane_count)
                    .map(|plane| {
                        image_barrier(
                            image,
                            subresource_aspect(drm, plane, plane_count),
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::ImageLayout::GENERAL,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::AccessFlags::SHADER_READ,
                        )
                    })
                    .collect();
                device.device().cmd_pipeline_barrier(
                    buffer,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &to_general,
                );
            }
        })
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        // SAFETY: the backing and the memory were created by this value and are destroyed once, in the
        // order Vulkan needs — the image or buffer before the memory bound to it, since `vkFreeMemory`
        // requires the memory no longer be bound to either.
        unsafe {
            match self.backing {
                Backing::Image(image) => self.device.destroy_image(image, None),
                Backing::Buffer(buffer) => self.device.destroy_buffer(buffer, None),
            }
            self.device.free_memory(self.memory, None);
        }
    }
}

/// A fence signalled when the producer's work on a buffer completes — the
/// [`DmaBufHandle::acquire_fence`](gpui_engine::DmaBufHandle) a recycling renderer waits on.
///
/// A producer that renders into a [`SharedBuffer`] signals this from its own submission and hands
/// over [`sync_file`](Self::sync_file); a producer with no queue of its own (a decoder handing over a
/// filled buffer) uses [`signal`](Self::signal), which submits an empty list.
pub struct Fence {
    device: ash::Device,
    external_fence_fd: ash::khr::external_fence_fd::Device,
    fence: vk::Fence,
}

impl Fence {
    /// Create an unsignalled fence.
    pub fn new(device: &GpuDevice) -> Result<Self> {
        let fence = unsafe {
            device
                .device()
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|e| anyhow!("create the fence: {e:?}"))?;
        Ok(Self {
            device: device.device().clone(),
            external_fence_fd: ash::khr::external_fence_fd::Device::new(
                device.instance(),
                device.device(),
            ),
            fence,
        })
    }

    /// Signal the fence from `device`'s queue with an empty command list — the "this buffer is ready"
    /// a producer that has no pass of its own hands over. A producer that renders signals the fence
    /// from the submission that did the rendering instead.
    pub fn signal(&self, device: &GpuDevice) -> Result<()> {
        // SAFETY: the pool, queue and fence belong to `device`; the buffer is freed on every path.
        unsafe {
            let allocate = vk::CommandBufferAllocateInfo::default()
                .command_pool(device.command_pool())
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let buffers = device
                .device()
                .allocate_command_buffers(&allocate)
                .map_err(|e| anyhow!("allocate a command buffer: {e:?}"))?;
            let buffer = buffers[0];
            let result = (|| -> Result<()> {
                device
                    .device()
                    .begin_command_buffer(buffer, &vk::CommandBufferBeginInfo::default())
                    .map_err(|e| anyhow!("begin the command buffer: {e:?}"))?;
                device
                    .device()
                    .end_command_buffer(buffer)
                    .map_err(|e| anyhow!("end the command buffer: {e:?}"))?;
                let buffers = [buffer];
                let submit = vk::SubmitInfo::default().command_buffers(&buffers);
                device
                    .device()
                    .queue_submit(device.queue(), &[submit], self.fence)
                    .map_err(|e| anyhow!("submit: {e:?}"))?;
                Ok(())
            })();
            device
                .device()
                .free_command_buffers(device.command_pool(), &buffers);
            result
        }
    }

    /// The `sync_file` the renderer waits on, for `DmaBufHandle::acquire_fence`.
    ///
    /// Export it after the submission that signals the fence: the returned fd is the one the renderer
    /// waits on, and the fence may not be exported this way twice.
    pub fn sync_file(&self) -> Result<OwnedFd> {
        // SAFETY: `fence` is this value's and unsignalled; the fd it returns is owned here.
        let fd = unsafe {
            self.external_fence_fd.get_fence_fd(
                &vk::FenceGetFdInfoKHR::default()
                    .fence(self.fence)
                    .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD),
            )
        }
        .map_err(|e| anyhow!("export the fence fd: {e:?}"))?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// The fence itself, for a producer that submits its own work against it.
    pub fn fence(&self) -> vk::Fence {
        self.fence
    }
}

impl Drop for Fence {
    fn drop(&mut self) {
        // SAFETY: created by this value and destroyed once.
        unsafe { self.device.destroy_fence(self.fence, None) };
    }
}

/// A modifier of `format` the device can sample and render to — the tiled choice, or `None` where
/// the device offers none.
pub(crate) fn tiled_modifier<D: ExportDevice + ?Sized>(
    device: &D,
    format: vk::Format,
    required: vk::FormatFeatureFlags,
) -> Option<u64> {
    supported_modifiers(device, format)
        .into_iter()
        .filter(|p| p.drm_format_modifier != DmaBufHandle::LINEAR)
        .find(|p| p.drm_format_modifier_tiling_features.contains(required))
        .map(|p| p.drm_format_modifier)
}

/// Create the shareable image, on `modifier` when one was chosen and linear otherwise.
fn create_image<D: ExportDevice + ?Sized>(
    device: &D,
    format: vk::Format,
    width: u32,
    height: u32,
    usage: vk::ImageUsageFlags,
    modifier: Option<u64>,
) -> Result<vk::Image> {
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

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
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external);

    // SAFETY: `info` chains locals that outlive the call, and the image is owned by the caller.
    let image = match modifier {
        Some(modifier) => {
            let modifiers = [modifier];
            let mut drm = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
                .drm_format_modifiers(&modifiers);
            let info = info
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .push_next(&mut drm);
            unsafe { device.device().create_image(&info, None) }
        }
        None => {
            let info = info.tiling(vk::ImageTiling::LINEAR);
            unsafe { device.device().create_image(&info, None) }
        }
    };
    image.map_err(|e| anyhow!("create the image: {e:?}"))
}

/// Record `record` into a fresh command buffer, submit it on `device`'s queue, and wait for the GPU
/// to finish — the one-shot submission a producer renders a buffer with, so it hands over a finished
/// buffer rather than an acquire fence.
fn submit_once(device: &GpuDevice, record: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
    // SAFETY: the command buffer is allocated from the device's pool, recorded once and freed on
    // every path; the fence is waited on and destroyed.
    unsafe {
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(device.command_pool())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let buffers = device
            .device()
            .allocate_command_buffers(&allocate)
            .map_err(|error| anyhow!("allocate a command buffer: {error:?}"))?;
        let buffer = buffers[0];
        let result = (|| -> Result<()> {
            device
                .device()
                .begin_command_buffer(buffer, &vk::CommandBufferBeginInfo::default())
                .map_err(|error| anyhow!("begin the command buffer: {error:?}"))?;
            record(buffer);
            device
                .device()
                .end_command_buffer(buffer)
                .map_err(|error| anyhow!("end the command buffer: {error:?}"))?;

            let buffers = [buffer];
            let submit = vk::SubmitInfo::default().command_buffers(&buffers);
            let fence = device
                .device()
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|error| anyhow!("create the submission's fence: {error:?}"))?;
            device
                .device()
                .queue_submit(device.queue(), &[submit], fence)
                .map_err(|error| anyhow!("submit: {error:?}"))?;
            device
                .device()
                .wait_for_fences(&[fence], true, u64::MAX)
                .map_err(|error| anyhow!("wait for the submission: {error:?}"))?;
            device.device().destroy_fence(fence, None);
            Ok(())
        })();
        device
            .device()
            .free_command_buffers(device.command_pool(), &buffers);
        result
    }
}

/// A host-visible staging buffer and the memory behind it, freed together when it drops — what a
/// [`SharedBuffer::fill`] uploads host bytes through.
struct Staging {
    device: ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
}

impl Staging {
    /// Allocate and bind a `size`-byte buffer the fill uploads through.
    fn new(device: &GpuDevice, size: u64) -> Result<Self> {
        // SAFETY: the buffer is created and bound once; every error path frees what it made.
        unsafe {
            let buffer = device
                .device()
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(|error| anyhow!("create the staging buffer: {error:?}"))?;
            let requirements = device.device().get_buffer_memory_requirements(buffer);
            let memory = match device.memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            ) {
                Ok(memory_type_index) => device
                    .device()
                    .allocate_memory(
                        &vk::MemoryAllocateInfo::default()
                            .allocation_size(requirements.size)
                            .memory_type_index(memory_type_index),
                        None,
                    )
                    .map_err(|error| anyhow!("allocate the staging memory: {error:?}"))?,
                Err(error) => {
                    device.device().destroy_buffer(buffer, None);
                    return Err(error);
                }
            };
            if let Err(error) = device.device().bind_buffer_memory(buffer, memory, 0) {
                device.device().free_memory(memory, None);
                device.device().destroy_buffer(buffer, None);
                return Err(anyhow!("bind the staging memory: {error:?}"));
            }
            Ok(Self {
                device: device.device().clone(),
                buffer,
                memory,
                size,
            })
        }
    }

    /// Copy `planes` into the buffer, back to back.
    fn write(&self, planes: &[&[u8]]) -> Result<()> {
        // SAFETY: the mapping covers the whole allocation and is released before returning.
        unsafe {
            let mapped = self
                .device
                .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
                .map_err(|error| anyhow!("map the staging memory: {error:?}"))?
                as *mut u8;
            let mut offset = 0usize;
            for plane in planes {
                std::ptr::copy_nonoverlapping(plane.as_ptr(), mapped.add(offset), plane.len());
                offset += plane.len();
            }
            self.device.unmap_memory(self.memory);
        }
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // SAFETY: both were made by this value and are freed once.
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// A full-image layout transition of `aspect`, from `from` to `to`, ordering `src_access` before
/// `dst_access`. `aspect` is one plane of a multi-plane image, or the colour aspect of a single one —
/// not a memory plane, which only the layout query names.
fn image_barrier(
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    from: vk::ImageLayout,
    to: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .old_layout(from)
        .new_layout(to)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
}

/// Create the instance [`GpuDevice::open`] runs on, carrying `VK_EXT_physical_device_drm` when the
/// loader offers it, so [`GpuId::of`] can name the GPU's DRM render node.
fn create_instance(entry: &ash::Entry) -> Result<ash::Instance> {
    let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
    // SAFETY: the call reads the loader's extension list.
    let available = unsafe { entry.enumerate_instance_extension_properties(None) }
        .map_err(|e| anyhow!("enumerate instance extensions: {e:?}"))?;
    let drm = CString::new(DRM_EXTENSION).expect("a literal without a NUL");
    let names: Vec<*const c_char> = available
        .iter()
        .filter(|e| extension_name(&e.extension_name) == DRM_EXTENSION)
        .map(|_| drm.as_ptr())
        .collect();

    // SAFETY: `names` points to locals that outlive the call.
    unsafe {
        entry
            .create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&names),
                None,
            )
            .map_err(|e| anyhow!("create the Vulkan instance: {e:?}"))
    }
}

/// Create a device on `physical_device`'s `queue_family_index`, with the extensions the export needs.
fn create_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> Result<ash::Device> {
    let extensions: Vec<CString> = DEVICE_EXTENSIONS
        .iter()
        .map(|e| CString::new(*e).expect("a literal without a NUL"))
        .collect();
    let extension_ptrs: Vec<*const c_char> = extensions.iter().map(|e| e.as_ptr()).collect();

    let priorities = [1.0f32];
    let queue_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities);
    // SAFETY: the pointers are to locals that outlive the call.
    unsafe {
        instance.create_device(
            physical_device,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(std::slice::from_ref(&queue_info))
                .enabled_extension_names(&extension_ptrs),
            None,
        )
    }
    .map_err(|e| anyhow!("create the Vulkan device: {e:?}"))
}

/// Whether `physical_device` supports every extension the export needs.
fn ensure_extensions(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Result<()> {
    // SAFETY: the call reads the device's extension list.
    let properties = unsafe { instance.enumerate_device_extension_properties(physical_device) }
        .map_err(|e| anyhow!("enumerate device extensions: {e:?}"))?;
    for wanted in DEVICE_EXTENSIONS {
        if !properties
            .iter()
            .any(|p| extension_name(&p.extension_name) == wanted)
        {
            return Err(anyhow!("the renderer's device does not support {wanted}"));
        }
    }
    Ok(())
}

/// The first graphics queue family of `physical_device`.
fn render_family(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Option<u32> {
    // SAFETY: the call reads the device's families.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    families
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|index| index as u32)
}

/// A device that can export a dma-buf, with a queue family that can render — or `None`.
fn pick_device(instance: &ash::Instance) -> Option<(vk::PhysicalDevice, u32)> {
    // SAFETY: the instance is live; the call only reads.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
    for physical_device in physical_devices {
        // SAFETY: both calls read; the properties are copied out.
        let properties =
            unsafe { instance.enumerate_device_extension_properties(physical_device) }.ok()?;
        let exports = DEVICE_EXTENSIONS.iter().all(|wanted| {
            properties
                .iter()
                .any(|p| extension_name(&p.extension_name) == *wanted)
        });
        if !exports {
            continue;
        }
        // SAFETY: the call reads the device's families.
        let families =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let Some(index) = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        else {
            continue;
        };
        return Some((physical_device, index as u32));
    }
    None
}

/// Every DRM format modifier `format` supports, with the features each carries.
fn supported_modifiers<D: ExportDevice + ?Sized>(
    device: &D,
    format: vk::Format,
) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
    // SAFETY: both calls read; the list is filled into a buffer sized from the first call's count.
    unsafe {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        device.instance().get_physical_device_format_properties2(
            device.physical_device(),
            format,
            &mut properties,
        );

        let count = list.drm_format_modifier_count as usize;
        if count == 0 {
            return Vec::new();
        }
        let mut modifiers = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut modifiers);
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        device.instance().get_physical_device_format_properties2(
            device.physical_device(),
            format,
            &mut properties,
        );
        modifiers
    }
}

/// The properties the device reports for `format` — the feature bits each tiling carries.
fn format_properties<D: ExportDevice + ?Sized>(
    device: &D,
    format: vk::Format,
) -> vk::FormatProperties {
    // SAFETY: the call only reads; the properties are copied out.
    unsafe {
        device
            .instance()
            .get_physical_device_format_properties(device.physical_device(), format)
    }
}

/// Refuse, before `vkCreateImage` would, a format the device cannot make in the chosen tiling — the
/// feature bits that tiling carries, and the extent the device caps it at.
///
/// `vkCreateImage` answers an unsupported request with a bare `VK_ERROR_FORMAT_NOT_SUPPORTED`, which
/// names neither the format nor what is missing. This names both, so a caller can pick another format
/// or tiling instead of guessing.
fn ensure_image_supported<D: ExportDevice + ?Sized>(
    device: &D,
    format: vk::Format,
    width: u32,
    height: u32,
    usage: vk::ImageUsageFlags,
    required: vk::FormatFeatureFlags,
    modifier: Option<u64>,
) -> Result<()> {
    let tiling = match modifier {
        Some(_) => vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT,
        None => vk::ImageTiling::LINEAR,
    };
    let features = match modifier {
        None => format_properties(device, format).linear_tiling_features,
        Some(modifier) => supported_modifiers(device, format)
            .into_iter()
            .find(|properties| properties.drm_format_modifier == modifier)
            .map(|properties| properties.drm_format_modifier_tiling_features)
            .ok_or_else(|| {
                anyhow!("the device lists no DRM modifier {modifier:#x} for {format:?}")
            })?,
    };
    if !features.contains(required) {
        return Err(anyhow!(
            "the device cannot make a {tiling:?} {format:?} image: it needs {required:?}, and the \
             format carries {features:?}"
        ));
    }

    // SAFETY: the calls only read; `info` chains a local that outlives the call.
    let max_extent = unsafe {
        let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier.unwrap_or(0));
        let mut info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(tiling)
            .usage(usage);
        if modifier.is_some() {
            info = info.push_next(&mut drm);
        }
        let mut properties = vk::ImageFormatProperties2::default();
        device
            .instance()
            .get_physical_device_image_format_properties2(
                device.physical_device(),
                &info,
                &mut properties,
            )
            .map_err(|error| {
                anyhow!("ask the device for its {tiling:?} {format:?} limits: {error:?}")
            })?;
        properties.image_format_properties.max_extent
    };
    if width > max_extent.width || height > max_extent.height {
        return Err(anyhow!(
            "the device's {format:?} images cap at {}×{}, asked for {width}×{height}",
            max_extent.width,
            max_extent.height,
        ));
    }
    Ok(())
}

/// The Vulkan format a [`SurfaceFormatKind`] maps to.
fn vk_format(format: SurfaceFormatKind) -> Result<vk::Format> {
    match format {
        SurfaceFormatKind::Bgra8(_) => Ok(vk::Format::B8G8R8A8_UNORM),
        SurfaceFormatKind::Rgba8(_) => Ok(vk::Format::R8G8B8A8_UNORM),
        SurfaceFormatKind::Nv12(_) => Ok(vk::Format::G8_B8R8_2PLANE_420_UNORM),
        other => Err(anyhow!(
            "{other:?} has no Vulkan format in the dma-buf hatch yet"
        )),
    }
}

/// The usage the hatch makes of an image: the producer writes it with transfers ([`clear`] and
/// [`fill`]) and the consumer samples it. `rendered` adds `COLOR_ATTACHMENT`, which only
/// [`SharedBuffer::render_target`] asks for — a linear image cannot carry it, so the check refuses one.
///
/// [`clear`]: SharedBuffer::clear
/// [`fill`]: SharedBuffer::fill
fn usage_for(rendered: bool) -> vk::ImageUsageFlags {
    let base = vk::ImageUsageFlags::SAMPLED
        | vk::ImageUsageFlags::TRANSFER_SRC
        | vk::ImageUsageFlags::TRANSFER_DST;
    if rendered {
        base | vk::ImageUsageFlags::COLOR_ATTACHMENT
    } else {
        base
    }
}

/// The format feature bits `usage` needs, so a capability check and the image agree on what it is
/// made for. Transfer features are guaranteed for every format, so only sampling and colour attachment
/// carry one.
fn required_features(usage: vk::ImageUsageFlags) -> vk::FormatFeatureFlags {
    let mut required = vk::FormatFeatureFlags::empty();
    if usage.contains(vk::ImageUsageFlags::SAMPLED) {
        required |= vk::FormatFeatureFlags::SAMPLED_IMAGE;
    }
    if usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
        required |= vk::FormatFeatureFlags::COLOR_ATTACHMENT;
    }
    required
}

/// Each plane of `format` at `width` × `height`, as `(byte length, extent width, extent height)` in
/// plane order — the geometry [`SharedBuffer::fill`] uploads against.
fn plane_geometry(format: SurfaceFormatKind, width: u32, height: u32) -> Vec<(usize, u32, u32)> {
    match format {
        SurfaceFormatKind::Bgra8(_) | SurfaceFormatKind::Rgba8(_) => {
            vec![((width as usize * height as usize) * 4, width, height)]
        }
        SurfaceFormatKind::Nv12(_) => vec![
            (width as usize * height as usize, width, height),
            (
                (width as usize / 2) * (height as usize / 2) * 2,
                width / 2,
                height / 2,
            ),
        ],
        _ => Vec::new(),
    }
}

/// The first memory type in `bits` whose flags satisfy `wanted`.
fn memory_type_index(
    properties: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    wanted: impl Fn(vk::MemoryPropertyFlags) -> bool,
) -> Option<u32> {
    (0..properties.memory_type_count).find(|index| {
        bits & (1u32 << index) != 0 && wanted(properties.memory_types[*index as usize].property_flags)
    })
}

/// The memory type an exported buffer takes: device-local, preferring a heap the CPU cannot map — the
/// shape the two working dma-buf paths here allocate with, and the one the NVIDIA driver tolerates
/// where a `HOST_VISIBLE` exportable allocation faults.
fn export_memory_type(properties: &vk::PhysicalDeviceMemoryProperties, bits: u32) -> Option<u32> {
    memory_type_index(properties, bits, |flags| {
        flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            && !flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE)
    })
    .or_else(|| {
        memory_type_index(properties, bits, |flags| {
            flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
    })
    .or_else(|| memory_type_index(properties, bits, |_| true))
}

/// The bytes `planes` span for `geometry`: how far the furthest plane's rows reach. Refuses a stride
/// too narrow to hold a row.
fn declared_span(geometry: &[(usize, u32, u32)], planes: &[PlaneLayout]) -> Result<u64> {
    let mut span = 0u64;
    for (plane, ((bytes, plane_width, plane_height), layout)) in
        geometry.iter().zip(planes).enumerate()
    {
        let row_bytes = (*bytes / *plane_height as usize) as u64;
        anyhow::ensure!(
            u64::from(layout.stride) >= row_bytes,
            "plane {plane} declares a stride of {}, but a {plane_width}-wide row needs {row_bytes}",
            layout.stride,
        );
        span = span.max(
            layout.offset + u64::from(layout.stride) * (u64::from(*plane_height) - 1) + row_bytes,
        );
    }
    Ok(span)
}

/// Check the declared planes fit inside `content` — the shape [`SharedBuffer::export_at`] enforces
/// before it allocates, and the same span [`GpuDevice::linear_layout`] reports.
fn check_plane_layouts(
    geometry: &[(usize, u32, u32)],
    planes: &[PlaneLayout],
    content: usize,
) -> Result<()> {
    let span = declared_span(geometry, planes)?;
    anyhow::ensure!(
        span <= content as u64,
        "the declared plane layout spans {span} bytes, past the {content} bytes of content given",
    );
    Ok(())
}

/// The aspect a `fill`'s **copy** names for `plane`: the colour aspect for a one-plane image, else
/// that plane's own. A barrier names a DRM image's plane by memory plane instead — see
/// [`subresource_aspect`].
fn plane_aspect(plane: usize, plane_count: usize) -> vk::ImageAspectFlags {
    if plane_count == 1 {
        vk::ImageAspectFlags::COLOR
    } else if plane == 0 {
        vk::ImageAspectFlags::PLANE_0
    } else {
        vk::ImageAspectFlags::PLANE_1
    }
}

/// The aspect a subresource of `plane` is named by: a DRM-modifier image is asked about by *memory
/// plane*, a linear one by colour aspect (or by plane, if the format has more than one).
fn subresource_aspect(drm: bool, plane: usize, plane_count: usize) -> vk::ImageAspectFlags {
    if drm {
        if plane == 0 {
            vk::ImageAspectFlags::MEMORY_PLANE_0_EXT
        } else {
            vk::ImageAspectFlags::MEMORY_PLANE_1_EXT
        }
    } else if plane_count == 1 {
        vk::ImageAspectFlags::COLOR
    } else if plane == 0 {
        vk::ImageAspectFlags::PLANE_0
    } else {
        vk::ImageAspectFlags::PLANE_1
    }
}

/// A `[c_char; N]` extension name as a `&str`, stopping at the NUL the loader wrote.
fn extension_name(raw: &[c_char]) -> &str {
    // SAFETY: the loader NUL-terminates the name in the fixed array.
    unsafe { CStr::from_ptr(raw.as_ptr()) }
        .to_str()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device on the GPU `ZED_DEVICE_ID` names, or the first that exports a dma-buf.
    ///
    /// [`GpuDevice::open`] reaches only the first that exports, which is the integrated GPU on a laptop
    /// — enough for the unit tests, but a probe that must run against a *specific* GPU needs the
    /// variable honoured, which `open` deliberately does not.
    fn probe_device() -> Option<GpuDevice> {
        // See `LOADER`: the loader's global entry points are not safe to initialise concurrently.
        let _loading = LOADER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = unsafe { ash::Entry::load() }.ok()?;
        let instance = create_instance(&entry).ok()?;
        let wanted = std::env::var("ZED_DEVICE_ID")
            .ok()
            .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok());
        let devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
        let chosen = devices.iter().copied().find(|physical| {
            let properties = unsafe { instance.get_physical_device_properties(*physical) };
            wanted.is_none_or(|id| properties.device_id == id)
        })?;
        let family = render_family(&instance, chosen)?;
        ensure_extensions(&instance, chosen).ok()?;
        GpuDevice::assemble(Some(entry), instance, true, false, chosen, family).ok()
    }

    /// How a linear two-plane `NV12` image is made, for the layout probe.
    enum LinearNv12<'a> {
        /// Plain `LINEAR` tiling: the layout the driver chooses for itself — a consumer's default read.
        Native,
        /// The producer's layout, carried through `ImageDrmFormatModifierExplicitCreateInfoEXT`.
        Declared(&'a [vk::SubresourceLayout]),
    }

    /// The layout the device reports for a linear two-plane `NV12` `width`x`height` image, per plane,
    /// with its memory requirement — created `how`, then **asked** rather than assumed.
    fn reported_layout(
        device: &GpuDevice,
        width: u32,
        height: u32,
        how: LinearNv12<'_>,
    ) -> Result<(vk::SubresourceLayout, vk::SubresourceLayout, u64), vk::Result> {
        let drm = matches!(how, LinearNv12::Declared(_));
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(DmaBufHandle::LINEAR);
        if let LinearNv12::Declared(layouts) = how {
            explicit = explicit.plane_layouts(layouts);
        }
        let mut info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(if drm {
                vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
            } else {
                vk::ImageTiling::LINEAR
            })
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        if drm {
            info = info.push_next(&mut explicit);
        }
        // SAFETY: `info` chains locals that outlive the call, and the image is destroyed below.
        let image = unsafe { device.device().create_image(&info, None) }?;
        let plane = |index: usize| unsafe {
            device.device().get_image_subresource_layout(
                image,
                vk::ImageSubresource {
                    aspect_mask: subresource_aspect(drm, index, 2),
                    mip_level: 0,
                    array_layer: 0,
                },
            )
        };
        let luma = plane(0);
        let chroma = plane(1);
        // SAFETY: the image is live and bound to no memory yet.
        let size = unsafe { device.device().get_image_memory_requirements(image) }.size;
        unsafe { device.device().destroy_image(image, None) };
        Ok((luma, chroma, size))
    }

    /// [`GpuDevice::linear_layout`] must report what the device actually does with a declared linear
    /// layout — the information a producer acts on *before* it exports. The report is checked against an
    /// independent measurement rather than trusted, and printed for the device under test: the
    /// integrated Intel device's canonical `NV12` layout *is* the one it gives a plain image, while an
    /// NVIDIA 930M gives chroma at 65536 and requires 128 KiB for a 6 KiB span.
    #[test]
    fn the_linear_layout_report_matches_the_device() {
        let Some(device) = probe_device() else {
            eprintln!("skipping: no Vulkan device for the layout report");
            return;
        };
        let (width, height) = (64u32, 64u32);
        let luma_bytes = u64::from(width) * u64::from(height);
        let chroma_bytes = u64::from(width / 2) * u64::from(height / 2) * 2;
        let span = luma_bytes + chroma_bytes;
        // The DRM `NV12` convention: chroma directly after the luma.
        let planes = [
            PlaneLayout {
                offset: 0,
                stride: width,
            },
            PlaneLayout {
                offset: luma_bytes,
                stride: width,
            },
        ];

        let (native_luma, native_chroma, native_memory) =
            match reported_layout(&device, width, height, LinearNv12::Native) {
                Ok(measured) => measured,
                Err(error) => {
                    eprintln!("skipping: no plain linear NV12 image ({error:?})");
                    return;
                }
            };
        eprintln!(
            "native linear NV12: luma {}@{}, chroma {}@{}, memory {native_memory}",
            native_luma.row_pitch,
            native_luma.offset,
            native_chroma.row_pitch,
            native_chroma.offset,
        );

        let report = match device.linear_layout(SurfaceFormatKind::nv12(), width, height, &planes) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("the device will not make the declared layout: {error:#}");
                return;
            }
        };
        // What the device gives a plain image, plane by plane — the layout a producer must declare,
        // checked against the measurement rather than trusted.
        assert_eq!(
            report.native,
            vec![
                PlaneLayout {
                    offset: native_luma.offset,
                    stride: native_luma.row_pitch as u32,
                },
                PlaneLayout {
                    offset: native_chroma.offset,
                    stride: native_chroma.row_pitch as u32,
                },
            ],
            "the report's native layout must be the plain image's own",
        );

        // The requirement of the image the importer builds from that native layout.
        let native_layouts = [
            vk::SubresourceLayout::default()
                .offset(native_luma.offset)
                .size(luma_bytes)
                .row_pitch(native_luma.row_pitch),
            vk::SubresourceLayout::default()
                .offset(native_chroma.offset)
                .size(chroma_bytes)
                .row_pitch(native_chroma.row_pitch),
        ];
        let measured =
            match reported_layout(&device, width, height, LinearNv12::Declared(&native_layouts)) {
                Ok((_, _, size)) => size,
                Err(error) => {
                    eprintln!("the device refused its own layout when asked directly ({error:?})");
                    return;
                }
            };
        eprintln!(
            "reported linear NV12: native {:?}, required {}, declared {} (measured requirement \
             {measured})",
            report.native, report.required, report.declared,
        );

        assert_eq!(
            report.declared, span,
            "the report's declared span is the layout the producer proposed",
        );
        assert_eq!(
            report.required, measured,
            "the report's requirement must be the device's own, measured independently",
        );
        assert!(
            report.required >= report.declared,
            "an image never needs less than its own layout spans",
        );
        assert_eq!(
            report.backed(),
            report.required == report.declared,
            "a layout is backed exactly when the device needs no more than it spans",
        );
    }

    #[test]
    fn a_linear_buffer_exports_a_dma_buf() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::bgra8(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        let handle = buffer.handle();
        assert_eq!(handle.modifier, DmaBufHandle::LINEAR, "asked for linear");
        assert_eq!(handle.planes.len(), 1, "a single-plane format");
        assert!(
            handle.planes[0].stride >= 64 * 4,
            "a BGRA8 row is at least four bytes a pixel"
        );
        assert_eq!(handle.width, 64);
        assert_eq!(handle.height, 64);
    }

    /// A producer whose pipeline writes linear light declares it on the handle the renderer takes;
    /// the default for an RGB buffer is the sRGB encoding every desktop producer emits.
    #[test]
    fn a_shared_buffer_can_declare_linear_light() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 4, 4, SurfaceFormatKind::rgba8(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        assert_eq!(buffer.handle().transfer, TransferFunction::Srgb);

        let linear = buffer.with_transfer(TransferFunction::Linear);
        assert_eq!(linear.handle().transfer, TransferFunction::Linear);
    }

    #[test]
    fn a_gpu_reports_the_render_node_it_backs() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let id = device.gpu_id();
        eprintln!("gpu: {} render node {:?}", id.name, id.render_node);
        assert!(!id.name.is_empty(), "a Vulkan device reports a name");
        if let Some(node) = &id.render_node {
            assert!(
                node.to_string_lossy().contains("renderD"),
                "a DRM render node names a render node: {node:?}"
            );
        }
    }

    /// A producer-opened device refuses `Tiling::Tiled`: the modifier it would pick is not known to be
    /// importable by the consumer, so the buffer is exported linear rather than risk a buffer the
    /// renderer wedges on. Real tiled coverage needs the renderer's own device —
    /// `tests/linux_dmabuf_readback.rs`, behind the `readback-test` feature.
    #[test]
    fn a_producers_own_device_refuses_tiling() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::bgra8(), Tiling::Tiled)
            .expect("allocate a shareable buffer");
        let handle = buffer.handle();
        assert_eq!(
            handle.modifier,
            DmaBufHandle::LINEAR,
            "a device the producer opened itself must not hand out a modifier"
        );
        assert_eq!(handle.planes.len(), 1, "a single-plane format");
        assert!(
            handle.planes[0].stride >= 64 * 4,
            "a linear row is at least four bytes a pixel"
        );
    }

    #[test]
    fn a_two_plane_nv12_buffer_exports_two_planes() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::nv12(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        let handle = buffer.handle();
        assert_eq!(handle.planes.len(), 2, "NV12 is two planes");
        assert!(
            handle.planes[0].stride >= 64,
            "a luma row is at least a byte a pixel"
        );
        assert!(
            handle.planes[1].stride >= 32,
            "a chroma row is at least a byte every two pixels"
        );
        assert!(
            handle.planes[1].offset >= handle.planes[0].stride as u64 * 64,
            "the chroma plane follows the luma plane"
        );
        eprintln!(
            "nv12: luma offset {} stride {}, chroma offset {} stride {}",
            handle.planes[0].offset,
            handle.planes[0].stride,
            handle.planes[1].offset,
            handle.planes[1].stride,
        );
    }

    #[test]
    fn a_pushed_buffer_exports_at_the_layout_it_declares() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        // A whole page past the luma's end, which no driver's own linear layout would choose — the
        // offset a consumer that fell back to its own layout would read as padding.
        let luma = 64usize * 64;
        let chroma_offset = luma + 4096;
        let content = vec![0u8; chroma_offset + 32 * 32 * 2];
        let buffer = SharedBuffer::export_at(
            &device,
            64,
            64,
            SurfaceFormatKind::nv12(),
            &[
                PlaneLayout {
                    offset: 0,
                    stride: 64,
                },
                PlaneLayout {
                    offset: chroma_offset as u64,
                    stride: 64,
                },
            ],
            &content,
        )
        .expect("export at a declared layout");
        let handle = buffer.handle();
        assert_eq!(handle.modifier, DmaBufHandle::LINEAR, "a pushed buffer is linear");
        assert_eq!(handle.planes[0].offset, 0);
        assert_eq!(
            handle.planes[1].offset, chroma_offset as u64,
            "the declared chroma offset, not the driver's",
        );
        assert_eq!(handle.planes[1].stride, 64);
        assert!(buffer.image().is_none(), "a pushed buffer backs no image");
    }

    #[test]
    fn a_pushed_buffer_refuses_a_layout_that_runs_past_its_content() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        // Luma alone fits the 4096 bytes; chroma at 4096 does not.
        let content = vec![0u8; 64 * 64];
        let refused = SharedBuffer::export_at(
            &device,
            64,
            64,
            SurfaceFormatKind::nv12(),
            &[
                PlaneLayout {
                    offset: 0,
                    stride: 64,
                },
                PlaneLayout {
                    offset: 4096,
                    stride: 64,
                },
            ],
            &content,
        );
        assert!(
            refused.is_err(),
            "a chroma plane past the content is refused",
        );
    }

    #[test]
    fn a_fence_exports_a_sync_file() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let fence = Fence::new(&device).expect("create a fence");
        fence.signal(&device).expect("signal the fence");
        let sync_file = fence.sync_file().expect("export a sync_file");
        assert!(
            std::os::fd::AsRawFd::as_raw_fd(&sync_file) >= 0,
            "a sync_file is a valid descriptor"
        );
    }

    #[test]
    fn a_cleared_buffer_is_ready_to_hand_over() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::bgra8(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        buffer
            .clear(&device, [0.5, 0.25, 0.125, 1.0])
            .expect("clear the buffer");
        // The handle is unchanged — clearing writes through the same dma-buf.
        assert_eq!(buffer.handle().planes.len(), 1);
    }

    #[test]
    fn a_filled_nv12_buffer_is_ready_to_hand_over() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::nv12(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        // A full-resolution luma plane, then a half-resolution interleaved chroma plane.
        let luma = vec![128u8; 64 * 64];
        let mut chroma = vec![0u8; 32 * 32 * 2];
        for pair in chroma.as_chunks_mut::<2>().0 {
            pair[0] = 90;
            pair[1] = 120;
        }
        buffer
            .fill(&device, &[&luma, &chroma])
            .expect("fill the two planes");
        assert_eq!(buffer.handle().planes.len(), 2, "NV12 stays two planes");
    }

    #[test]
    fn a_fill_of_the_wrong_size_is_refused() {
        let Ok(device) = GpuDevice::open() else {
            eprintln!("skipping: no Vulkan device exports a dma-buf");
            return;
        };
        let buffer = SharedBuffer::new(&device, 64, 64, SurfaceFormatKind::nv12(), Tiling::Linear)
            .expect("allocate a shareable buffer");
        let luma = vec![0u8; 64 * 64];
        let short_chroma = vec![0u8; 16];
        let error = buffer
            .fill(&device, &[&luma, &short_chroma])
            .expect_err("a chroma plane of the wrong length is refused");
        assert!(
            error.to_string().contains("plane 1"),
            "the error should name the plane: {error}"
        );
    }

    /// The session handle the platform already resolved names the exact card, so `from_adapter`
    /// takes the render node from it rather than opening an instance and matching PCI.
    #[cfg(feature = "gpui")]
    #[test]
    fn from_adapter_reads_the_sessions_render_node() {
        let info = gpui::GpuAdapterInfo::new(
            gpui::GpuId {
                permanent: gpui::PermanentGpuId::Pci {
                    vendor_id: Some(0x8086),
                    device_id: 0x1916,
                    subsys_id: None,
                    revision: None,
                },
                session: Some(gpui::SessionGpuId::Drm { render_minor: 128 }),
            },
            "Intel(R) HD Graphics 520",
            gpui::GpuDeviceType::Integrated,
            gpui::GpuBackend::Vulkan,
            false,
        );
        let gpu = GpuId::from_adapter(&info).expect("a DRM session names a render node");
        assert_eq!(gpu.name, "Intel(R) HD Graphics 520");
        assert_eq!(gpu.render_node, Some(PathBuf::from("/dev/dri/renderD128")));
    }

    /// An adapter with no PCI identity and no render node cannot be mapped; guessing would name the
    /// wrong GPU to a producer.
    #[cfg(feature = "gpui")]
    #[test]
    fn from_adapter_refuses_an_adapter_it_cannot_name() {
        let info = gpui::GpuAdapterInfo::new(
            gpui::GpuId {
                permanent: gpui::PermanentGpuId::Name("llvmpipe".into()),
                session: None,
            },
            "llvmpipe",
            gpui::GpuDeviceType::Cpu,
            gpui::GpuBackend::Vulkan,
            true,
        );
        assert!(GpuId::from_adapter(&info).is_none());
    }
}
