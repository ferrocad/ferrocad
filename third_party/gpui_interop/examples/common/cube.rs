#![allow(
    dead_code,
    reason = "one module, two examples, each using a subset of it"
)]
//! The shared foreign-viewport cube: a producer that owns its device draws a cube.
//!
//! This is the producer half of two examples — `interop_cube`, which turns the cube on a clock, and
//! `interop_cube_drag`, which turns it with the pointer through the input gateway — so the cube, its
//! pipeline and its ring live here, and each example owns only its window and its view.
//!
//! Nothing about the cube is GPUI's: it is rendered by a Vulkan pipeline this module builds on the
//! [`GpuDevice`] the crate hands out, into a [`SharedBuffer`], and exported as a dma-buf the window's
//! renderer composites with no CPU copy. It is the case the crate exists for, one step past
//! `interop_linux`: not a buffer the hatch *cleared*, but a frame a producer **rendered** with its own
//! pipelines.
//!
//! [`Cube::build`] reaches the renderer through `attach(..).adapter().gpu_device()` and draws into a
//! `SharedBuffer::render_target(.., Tiling::Tiled)` — a rendered buffer carries `COLOR_ATTACHMENT`,
//! which a linear image cannot, so it is always tiled and always on the renderer's own device, and
//! [`GpuDevice::open`] is deliberately not the fallback here that it is in `interop_linux`. A ring of
//! three buffers cycles, each reused only after the renderer signals the `release` descriptor it was
//! handed — `gpui_va`'s pattern, so a release that never comes costs a timeout rather than the cube —
//! and each hand-over carries a fresh [`Fence`] `sync_file`.
//!
//! The cube's WGSL is compiled to SPIR-V at run time by `naga` — the frontend and backend `wgpu` itself
//! uses — so no offline shader compiler is needed. `naga`'s default writer options flip the clip-space Y
//! for Vulkan, so the projection below is the ordinary y-up, zero-to-one-depth one a `wgpu` shader is
//! written against, and the cube comes up the right way round.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use ash::vk;
use gpui::{DmaBufHandle, GpuCanvasContext, Point, SurfaceFormatKind};
use gpui_interop::{Fence, GpuDevice, SharedBuffer, Tiling, attach};
use gpui_wgpu::WgpuRenderer;

/// How many buffers the producer cycles through. Two is the minimum that lets a frame render while
/// the previous one is sampled; three leaves room for a compositor that is a frame behind.
const SLOTS: usize = 3;

/// How long a handed-out buffer waits for its release before the ring reclaims it anyway — `gpui_va`'s
/// rule, so a buffer the compositor never sampled does not freeze the cube for good.
const RELEASE_TIMEOUT: Duration = Duration::from_millis(250);

/// The colour the frame is cleared to before the cube is drawn — a slate backdrop the surface owns
/// outright, so nothing behind the viewport shows through it.
pub const OPAQUE: [f32; 4] = [0.055, 0.06, 0.08, 1.0];

/// Nothing. The frame is cleared to transparent, so what GPUI drew beneath the surface shows through
/// it and only the cube's own pixels occlude them. The consumer has to blend the surface for this to
/// mean anything, which the `surfaces` pipeline does under the window's alpha mode.
pub const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// Where the eye sits from the cube's centre, in world units, before anything zooms. At this distance
/// the cube fills a little under half the viewport.
const DEFAULT_DISTANCE: f32 = 5.0;

/// How near and how far the eye may be pushed. Near enough that the cube fills the viewport; far
/// enough that it is a small thing in the middle of one, which is what shows the layer behind it.
const MIN_DISTANCE: f32 = 2.5;
const MAX_DISTANCE: f32 = 12.0;

/// The depth format. A depth-stencil attachment the cube tests against, and which every Vulkan
/// implementation this example targets can make.
const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// The cube's shaders. One uniform (the MVP), a per-vertex normal, and a flat shade that makes the
/// rotation legible without a texture.
const SHADER: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = uniforms.mvp * vec4<f32>(position, 1.0);
    output.normal = normal;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(input.normal);
    let light = normalize(vec3<f32>(0.35, 0.75, 0.55));
    let shade = 0.22 + 0.78 * max(dot(n, light), 0.0);
    let tint = vec3<f32>(0.42, 0.48, 0.62) + 0.5 * abs(n);
    return vec4<f32>(tint * shade, 1.0);
}
"#;

/// The producer's half: a dma-buf-capable device on the renderer's GPU, a cube pipeline on it, and
/// the ring of shareable buffers it renders into.
///
/// Declaration order is load-bearing twice over. A [`SharedBuffer`] frees its memory through the
/// device it was made on, and the ring's framebuffers reference the render pass in [`Graphics`], so
/// the slots come first, the graphics after, and the device **last**: Rust drops a struct's fields
/// in declaration order, which is the order Vulkan needs.
pub struct Cube {
    slots: Vec<Slot>,
    graphics: Graphics,
    device: GpuDevice,
    /// The device name, for a header that wants to name it.
    gpu: String,
    /// The colour each frame is cleared to before the cube is drawn: [`OPAQUE`] for the surface's own
    /// backdrop, [`TRANSPARENT`] to let what is behind it show through.
    background: [f32; 4],
    /// The eye's distance from the cube's centre, in world units. Larger is a smaller cube — this is
    /// the zoom.
    distance: f32,
    /// How far the cube has turned about Y, in radians.
    yaw: f32,
    /// How far it has turned about X, in radians.
    pitch: f32,
    /// The last frame handed over, re-shown on a frame with no free buffer.
    last: Option<DmaBufHandle>,
    /// Frames rendered into a free buffer, and frames skipped because none was free.
    rendered: u64,
    dropped: u64,
}

impl Cube {
    /// Build the ring and the pipeline on `gpu`, rendering `size`×`size` frames cleared to
    /// `background` — [`OPAQUE`] or [`TRANSPARENT`].
    pub fn build(gpu: &mut GpuCanvasContext, size: u32, background: [f32; 4]) -> Result<Self> {
        // The cube renders into a driver-tiled image, so it has to render on the GPU the window
        // draws on; there is no linear fallback, since a linear image cannot be a colour attachment.
        let device = attach(gpu.try_device::<WgpuRenderer>())
            .ok()
            .and_then(|interop| interop.adapter().gpu_device())
            .context(
                "a rendered, driver-tiled buffer needs the renderer's own GPU, and this window's \
                     renderer lent no dma-buf device (is it wgpu on Vulkan?)",
            )?;

        let name = device.gpu_id().name;
        let graphics = Graphics::build(&device, size)?;
        let sets = graphics.allocate_sets(&device, SLOTS)?;
        let commands = graphics.allocate_commands(&device, SLOTS)?;
        let slots = (0..SLOTS)
            .map(|index| Slot::new(&device, &graphics, size, sets[index], commands[index]))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            slots,
            graphics,
            device,
            gpu: name,
            background,
            distance: DEFAULT_DISTANCE,
            yaw: 0.0,
            pitch: 0.0,
            last: None,
            rendered: 0,
            dropped: 0,
        })
    }

    /// Turn the cube: add a yaw about Y and a pitch about X, in radians.
    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch += pitch;
    }

    /// Return the cube to its starting orientation and distance.
    pub fn reset(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.0;
        self.distance = DEFAULT_DISTANCE;
    }

    /// Zoom: move the eye `factor` times as far from the cube, so a factor below one brings it nearer
    /// and makes the cube larger. Clamped to the range the projection stays sane over.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// The GPU the cube was built on, in Vulkan's name for it.
    pub fn gpu(&self) -> &str {
        &self.gpu
    }

    /// How many frames have been rendered into a free buffer.
    pub fn rendered(&self) -> u64 {
        self.rendered
    }

    /// How many frames were skipped because no buffer was free — a drop, in the ring's terms.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Whether `uv` — a point in the canvas's own frame, `0..1` and origin top-left — falls on the
    /// cube's silhouette.
    ///
    /// A `gpu_canvas` composited over this producer's frames can hand this to
    /// [`GpuCanvas::occlude_where`](gpui::GpuCanvas), so the canvas claims the pointer over the cube
    /// and declines it everywhere else, and the layer drawn behind stays live in the gaps.
    ///
    /// The answer is the convex hull of the cube's eight projected corners, which *is* the silhouette:
    /// a cube is the convex hull of its corners, and a projection carries a convex body to the convex
    /// hull of the projections of its corners. So it is exact — a hexagon as the cube turns, a square
    /// when a face is square to the eye — where the axis-aligned box around the corners would also
    /// claim the empty wedges beside it. The cube is centred on the origin and the projection is
    /// symmetric, so the hull stays centred as the cube turns and shrinks as the eye pulls back, which
    /// is when the layer behind shows through.
    pub fn covers(&self, uv: Point<f32>) -> bool {
        let mvp = self.mvp();
        let mut corners = [Point::default(); 8];
        let mut count = 0;
        for x in [-1.0f32, 1.0] {
            for y in [-1.0f32, 1.0] {
                for z in [-1.0f32, 1.0] {
                    let clip = transform(&mvp, [x, y, z, 1.0]);
                    // The eye never enters the cube over the zoom's range, so every corner is in
                    // front of it; a guard keeps a future change from turning that into a NaN.
                    if clip[3] <= 0.0 {
                        continue;
                    }
                    // The projection is the y-up one a wgpu shader is written against, and the canvas's
                    // frame has y down; `naga` flips y for Vulkan on the way out, which is why the
                    // rendered image agrees with this.
                    corners[count] = Point {
                        x: (clip[0] / clip[3] + 1.0) * 0.5,
                        y: (1.0 - clip[1] / clip[3]) * 0.5,
                    };
                    count += 1;
                }
            }
        }
        point_in_convex_hull(&convex_hull(&corners[..count]), uv)
    }

    /// Render the next frame of the cube, at its current orientation, into a free buffer and return
    /// it — or re-hand the last frame while every buffer is still being sampled. `None` only before
    /// the first frame is drawn.
    pub fn frame(&mut self) -> Option<DmaBufHandle> {
        let Some(index) = self.free_slot() else {
            self.dropped += 1;
            return self.last.clone();
        };
        let mvp = self.mvp();
        match self.render_into(index, &mvp) {
            Ok(handle) => {
                self.rendered += 1;
                self.last = Some(handle.clone());
                Some(handle)
            }
            Err(error) => {
                log::error!("interop_cube: rendering frame failed: {error:#}");
                self.last.clone()
            }
        }
    }

    /// The first slot the compositor has released (or one whose release has timed out).
    fn free_slot(&self) -> Option<usize> {
        (0..self.slots.len()).find(|&index| self.slots[index].released())
    }

    /// Draw into `index`'s buffer and hand it over with an acquire fence and a release descriptor.
    fn render_into(&mut self, index: usize, mvp: &[f32; 16]) -> Result<DmaBufHandle> {
        let slot = &mut self.slots[index];
        // The last submission on this slot is done — the compositor waited on its acquire fence —
        // so the buffer, its descriptor, and its fence may all be replaced.
        slot.fence = None;
        slot.release = None;

        let device = &self.device;
        write_uniform(device, slot.uniform_memory, mvp)?;

        let fence = Fence::new(device)?;
        submit_cube(device, &self.graphics, slot, self.background, fence.fence())?;

        let acquire = fence.sync_file()?;
        let release =
            release_descriptor().context("making the release descriptor the compositor signals")?;

        slot.fence = Some(fence);
        slot.release = Some(release.clone());
        slot.handed = Some(Instant::now());

        let mut handle = slot.buffer.handle();
        handle.acquire_fence = Some(Arc::new(acquire));
        handle.release = Some(release);
        Ok(handle)
    }

    /// The cube's model-view-projection. The projection is the y-up, zero-to-one-depth one a `wgpu`
    /// shader is written against; `naga` flips Y for Vulkan, so the cube is upright. The eye looks at
    /// the origin from `distance` away, which is what the zoom moves.
    fn mvp(&self) -> [f32; 16] {
        let projection = perspective_rh(45f32.to_radians(), 1.0, 0.1, 100.0);
        let view = look_at_rh([0.0, 0.0, self.distance], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let model = mul(&rotation_y(self.yaw), &rotation_x(self.pitch));
        mul(&projection, &mul(&view, &model))
    }
}

/// One buffer of the ring and everything the producer draws into it with: the shareable image, the
/// depth image and framebuffer the pass needs, the uniform buffer the frame is written through, and
/// the fence and release descriptor that order its hand-over.
struct Slot {
    /// The shareable image. First, so it is dropped after the views and framebuffer that name it.
    buffer: SharedBuffer,
    device: ash::Device,
    view: vk::ImageView,
    depth: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    uniform_buffer: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    /// The descriptor set and command buffer are the pool's, freed when the pool is.
    descriptor_set: vk::DescriptorSet,
    command_buffer: vk::CommandBuffer,
    /// The edge of this slot's images, for the render pass's render area.
    size: u32,
    /// The acquire fence of the frame handed out on this slot; kept alive until the slot is reused.
    fence: Option<Fence>,
    /// The `eventfd` the compositor signals once it has finished sampling this slot's buffer.
    release: Option<Arc<OwnedFd>>,
    /// When the current buffer was handed out, so a release that never comes is reclaimed.
    handed: Option<Instant>,
}

impl Slot {
    fn new(
        device: &GpuDevice,
        graphics: &Graphics,
        size: u32,
        descriptor_set: vk::DescriptorSet,
        command_buffer: vk::CommandBuffer,
    ) -> Result<Self> {
        // Only `Tiled`: a rendered buffer carries `COLOR_ATTACHMENT`, which linear cannot. On the
        // renderer's own device a modifier is available, so this is the guaranteed-tiled path.
        let buffer = SharedBuffer::render_target(
            device,
            size,
            size,
            SurfaceFormatKind::bgra8(),
            Tiling::Tiled,
        )
        .context("allocating a shareable render target")?;

        let color_format = vk::Format::B8G8R8A8_UNORM;
        let view = create_view(
            device,
            buffer.image().context("a render target has an image")?,
            color_format,
            vk::ImageAspectFlags::COLOR,
        )?;
        let (depth, depth_memory) = create_image(
            device,
            DEPTH_FORMAT,
            size,
            size,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )?;
        let depth_view = create_view(device, depth, DEPTH_FORMAT, vk::ImageAspectFlags::DEPTH)?;
        let attachments = [view, depth_view];
        let framebuffer = unsafe {
            device.device().create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(graphics.render_pass)
                    .attachments(&attachments)
                    .width(size)
                    .height(size)
                    .layers(1),
                None,
            )
        }
        .context("creating the framebuffer")?;

        let (uniform_buffer, uniform_memory) = create_buffer(
            device,
            UNIFORM_SIZE,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        write_descriptor(device, descriptor_set, uniform_buffer)?;

        Ok(Self {
            buffer,
            device: device.device().clone(),
            view,
            depth,
            depth_memory,
            depth_view,
            framebuffer,
            uniform_buffer,
            uniform_memory,
            descriptor_set,
            command_buffer,
            size,
            fence: None,
            release: None,
            handed: None,
        })
    }

    /// Whether this slot's buffer may be drawn into again: nothing is handed out, or the compositor
    /// has released it, or its release has timed out.
    fn released(&self) -> bool {
        match &self.release {
            None => true,
            Some(release) => {
                release_signalled(release)
                    || self
                        .handed
                        .is_some_and(|handed| handed.elapsed() >= RELEASE_TIMEOUT)
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // SAFETY: every object was made by this value on this device and is destroyed once. The
        // descriptor set and command buffer belong to the pool and are freed with it.
        unsafe {
            self.device.destroy_framebuffer(self.framebuffer, None);
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth, None);
            self.device.free_memory(self.depth_memory, None);
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_buffer(self.uniform_buffer, None);
            self.device.free_memory(self.uniform_memory, None);
        }
    }
}

/// The producer's pipeline: the command pool and descriptor pool the slots draw their per-buffer
/// objects from, the render pass and layout they share, and the cube's own geometry and pipeline.
struct Graphics {
    device: ash::Device,
    command_pool: vk::CommandPool,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline_layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_count: u32,
}

impl Graphics {
    fn build(device: &GpuDevice, size: u32) -> Result<Self> {
        let raw = device.device();
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(device.queue_family_index());
        let command_pool = unsafe { raw.create_command_pool(&pool_info, None) }
            .context("creating the producer's command pool")?;

        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX)];
        let descriptor_set_layout = unsafe {
            raw.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .context("creating the uniform's set layout")?;

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(SLOTS as u32)];
        let descriptor_pool = unsafe {
            raw.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .pool_sizes(&pool_sizes)
                    .max_sets(SLOTS as u32),
                None,
            )
        }
        .context("creating the uniform's descriptor pool")?;

        let set_layouts = [descriptor_set_layout];
        let pipeline_layout = unsafe {
            raw.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )
        }
        .context("creating the pipeline layout")?;

        let render_pass = create_render_pass(device)?;
        let pipeline = create_pipeline(device, render_pass, pipeline_layout, size)?;
        let mesh = Mesh::new(device)?;

        Ok(Self {
            device: raw.clone(),
            command_pool,
            descriptor_set_layout,
            descriptor_pool,
            pipeline_layout,
            render_pass,
            pipeline,
            vertex_buffer: mesh.vertex_buffer,
            vertex_memory: mesh.vertex_memory,
            index_buffer: mesh.index_buffer,
            index_memory: mesh.index_memory,
            index_count: mesh.index_count,
        })
    }

    /// One descriptor set per slot, each pointed at that slot's uniform buffer.
    fn allocate_sets(&self, device: &GpuDevice, count: usize) -> Result<Vec<vk::DescriptorSet>> {
        let layouts = vec![self.descriptor_set_layout; count];
        let sets = unsafe {
            device.device().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("allocating the uniform descriptor sets")?;
        Ok(sets)
    }

    /// One primary command buffer per slot, recorded afresh each frame.
    fn allocate_commands(
        &self,
        device: &GpuDevice,
        count: usize,
    ) -> Result<Vec<vk::CommandBuffer>> {
        let commands = unsafe {
            device.device().allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(count as u32),
            )
        }
        .context("allocating the frame command buffers")?;
        Ok(commands)
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        // SAFETY: every object was made by this value on this device and is destroyed once, in an
        // order Vulkan needs (the pipeline before its layout and render pass; the pool after the
        // sets it owns). The slots' framebuffers are dropped before this, with the slots.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
            self.device.destroy_buffer(self.index_buffer, None);
            self.device.free_memory(self.index_memory, None);
            self.device.destroy_command_pool(self.command_pool, None);
        }
    }
}

/// The number of bytes a single `mat4x4<f32>` uniform takes.
const UNIFORM_SIZE: u64 = 64;

/// The cube's geometry: 24 vertices (four per face, so each face carries its own normal) and 36
/// indices, in a host-visible buffer.
struct Mesh {
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_count: u32,
}

impl Mesh {
    fn new(device: &GpuDevice) -> Result<Self> {
        /// A vertex: a position and the normal of the face it belongs to.
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Vertex {
            position: [f32; 3],
            normal: [f32; 3],
        }

        // A unit cube's six faces, four corners each. Winding is irrelevant — the pass tests depth
        // and culls nothing — so each face only has to agree with its normal.
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            (
                [1.0, 0.0, 0.0],
                [[1., -1., -1.], [1., -1., 1.], [1., 1., 1.], [1., 1., -1.]],
            ),
            (
                [-1.0, 0.0, 0.0],
                [
                    [-1., -1., 1.],
                    [-1., -1., -1.],
                    [-1., 1., -1.],
                    [-1., 1., 1.],
                ],
            ),
            (
                [0.0, 1.0, 0.0],
                [[-1., 1., -1.], [1., 1., -1.], [1., 1., 1.], [-1., 1., 1.]],
            ),
            (
                [0.0, -1.0, 0.0],
                [
                    [-1., -1., 1.],
                    [1., -1., 1.],
                    [1., -1., -1.],
                    [-1., -1., -1.],
                ],
            ),
            (
                [0.0, 0.0, 1.0],
                [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
            ),
            (
                [0.0, 0.0, -1.0],
                [
                    [1., -1., -1.],
                    [-1., -1., -1.],
                    [-1., 1., -1.],
                    [1., 1., -1.],
                ],
            ),
        ];

        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        for (normal, corners) in faces {
            let base = vertices.len() as u16;
            for corner in corners {
                vertices.push(Vertex {
                    position: corner,
                    normal,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        let flags = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let (vertex_buffer, vertex_memory) = create_buffer(
            device,
            std::mem::size_of_val(vertices.as_slice()) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            flags,
        )?;
        upload(device, vertex_memory, &vertices)?;
        let (index_buffer, index_memory) = create_buffer(
            device,
            std::mem::size_of_val(indices.as_slice()) as u64,
            vk::BufferUsageFlags::INDEX_BUFFER,
            flags,
        )?;
        upload(device, index_memory, &indices)?;

        Ok(Self {
            vertex_buffer,
            vertex_memory,
            index_buffer,
            index_memory,
            index_count: indices.len() as u32,
        })
    }
}

/// Record the cube's draw into `slot`'s command buffer and submit it, signalling `fence` when the
/// GPU is done. The render pass clears and ends in `GENERAL`, the layout the compositor imports the
/// dma-buf on regardless — a transition is what `UNDEFINED` buys.
fn submit_cube(
    device: &GpuDevice,
    graphics: &Graphics,
    slot: &mut Slot,
    background: [f32; 4],
    fence: vk::Fence,
) -> Result<()> {
    let raw = device.device();
    unsafe {
        raw.reset_command_buffer(slot.command_buffer, vk::CommandBufferResetFlags::empty())
            .context("resetting the frame command buffer")?;
        raw.begin_command_buffer(slot.command_buffer, &vk::CommandBufferBeginInfo::default())
            .context("beginning the frame command buffer")?;

        let clears = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: background,
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(graphics.render_pass)
            .framebuffer(slot.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: slot.size,
                    height: slot.size,
                },
            })
            .clear_values(&clears);
        raw.cmd_begin_render_pass(slot.command_buffer, &begin, vk::SubpassContents::INLINE);
        raw.cmd_bind_pipeline(
            slot.command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            graphics.pipeline,
        );
        raw.cmd_bind_descriptor_sets(
            slot.command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            graphics.pipeline_layout,
            0,
            &[slot.descriptor_set],
            &[],
        );
        raw.cmd_bind_vertex_buffers(slot.command_buffer, 0, &[graphics.vertex_buffer], &[0]);
        raw.cmd_bind_index_buffer(
            slot.command_buffer,
            graphics.index_buffer,
            0,
            vk::IndexType::UINT16,
        );
        raw.cmd_draw_indexed(slot.command_buffer, graphics.index_count, 1, 0, 0, 0);
        raw.cmd_end_render_pass(slot.command_buffer);
        raw.end_command_buffer(slot.command_buffer)
            .context("ending the frame command buffer")?;

        let buffers = [slot.command_buffer];
        let submit = vk::SubmitInfo::default().command_buffers(&buffers);
        raw.queue_submit(device.queue(), &[submit], fence)
            .context("submitting the cube's frame")?;
    }
    Ok(())
}

/// The render pass the cube draws through: the shareable colour image and a depth attachment.
fn create_render_pass(device: &GpuDevice) -> Result<vk::RenderPass> {
    let attachments = [
        vk::AttachmentDescription::default()
            .format(vk::Format::B8G8R8A8_UNORM)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            // General: the dma-buf is imported as its own image, so no consumer layout is expected,
            // and `GENERAL` is what the hatch's own `clear` leaves a handed-over buffer in.
            .final_layout(vk::ImageLayout::GENERAL),
        vk::AttachmentDescription::default()
            .format(DEPTH_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
    ];
    let color = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let depth = vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color))
        .depth_stencil_attachment(&depth)];
    unsafe {
        device.device().create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(&subpasses),
            None,
        )
    }
    .context("creating the cube's render pass")
}

/// The cube's graphics pipeline. No culling — the depth test resolves the cube — and no blending:
/// the pass clears and the cube covers what it covers.
fn create_pipeline(
    device: &GpuDevice,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    size: u32,
) -> Result<vk::Pipeline> {
    let raw = device.device();
    let vertex = shader_module(device, SHADER, naga::ShaderStage::Vertex, "vs_main")?;
    let fragment = shader_module(device, SHADER, naga::ShaderStage::Fragment, "fs_main")?;

    let vs_name = CString::new("vs_main").expect("a literal without a NUL");
    let fs_name = CString::new("fs_main").expect("a literal without a NUL");
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(&vs_name),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(&fs_name),
    ];

    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(std::mem::size_of::<[f32; 6]>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(std::mem::size_of::<[f32; 3]>() as u32),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = [vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: size as f32,
        height: size as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }];
    let scissor = [vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: size,
            height: size,
        },
    }];
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewports(&viewport)
        .scissors(&scissor);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let color_blend =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let pipeline =
        unsafe { raw.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }
            .map_err(|(_, error)| anyhow!("creating the cube's pipeline: {error:?}"))?;
    let pipeline = pipeline[0];

    // SAFETY: the modules were only ever needed to build the pipeline, which is now built.
    unsafe {
        raw.destroy_shader_module(vertex, None);
        raw.destroy_shader_module(fragment, None);
    }
    Ok(pipeline)
}

/// Compile `wgsl`'s `entry` point to a `vk::ShaderModule` with `naga`.
fn shader_module(
    device: &GpuDevice,
    wgsl: &str,
    stage: naga::ShaderStage,
    entry: &str,
) -> Result<vk::ShaderModule> {
    let module = naga::front::wgsl::parse_str(wgsl)
        .map_err(|error| anyhow!("parsing the cube's WGSL: {}", error.emit_to_string(wgsl)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| anyhow!("validating the cube's WGSL: {error:?}"))?;
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: stage,
        entry_point: entry.to_owned(),
    };
    let words = naga::back::spv::write_vec(
        &module,
        &info,
        &naga::back::spv::Options::default(),
        Some(&pipeline),
    )
    .map_err(|error| anyhow!("writing the cube's SPIR-V: {error:?}"))?;

    // SAFETY: `words` holds the module's SPIR-V and outlives the call; the module is owned here.
    unsafe {
        device
            .device()
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
    }
    .map_err(|error| anyhow!("creating the {entry} shader module: {error:?}"))
}

/// Allocate a buffer and bind memory whose properties carry every flag in `flags`.
fn create_buffer(
    device: &GpuDevice,
    size: u64,
    usage: vk::BufferUsageFlags,
    flags: vk::MemoryPropertyFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let raw = device.device();
    // SAFETY: every error path frees what this made; the returns are owned by the caller.
    unsafe {
        let buffer = raw
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .context("creating a buffer")?;
        let requirements = raw.get_buffer_memory_requirements(buffer);
        let memory = match device.memory_type(requirements.memory_type_bits, flags) {
            Ok(index) => raw
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(index),
                    None,
                )
                .context("allocating the buffer's memory")?,
            Err(error) => {
                raw.destroy_buffer(buffer, None);
                return Err(error);
            }
        };
        if let Err(error) = raw.bind_buffer_memory(buffer, memory, 0) {
            raw.free_memory(memory, None);
            raw.destroy_buffer(buffer, None);
            return Err(anyhow!("binding the buffer's memory: {error:?}"));
        }
        Ok((buffer, memory))
    }
}

/// Map `memory` and copy `data` into it — the upload a host-visible buffer takes.
fn upload<T: Copy>(device: &GpuDevice, memory: vk::DeviceMemory, data: &[T]) -> Result<()> {
    let bytes = std::mem::size_of_val(data);
    // SAFETY: `memory` is host-visible and at least `bytes` long; the mapping is released below.
    unsafe {
        let mapped = device
            .device()
            .map_memory(memory, 0, bytes as u64, vk::MemoryMapFlags::empty())
            .context("mapping a host-visible buffer")? as *mut u8;
        std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), mapped, bytes);
        device.device().unmap_memory(memory);
    }
    Ok(())
}

/// Write the frame's MVP into the slot's uniform buffer. The memory is coherent, so the write is
/// visible to the GPU without a flush.
fn write_uniform(device: &GpuDevice, memory: vk::DeviceMemory, mvp: &[f32; 16]) -> Result<()> {
    upload(device, memory, mvp.as_slice())
}

/// Point a slot's descriptor set at its uniform buffer.
fn write_descriptor(device: &GpuDevice, set: vk::DescriptorSet, buffer: vk::Buffer) -> Result<()> {
    let buffer_info = [vk::DescriptorBufferInfo::default()
        .buffer(buffer)
        .offset(0)
        .range(UNIFORM_SIZE)];
    let writes = [vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(&buffer_info)];
    // SAFETY: the set belongs to this device's pool, and the write describes a live buffer.
    unsafe { device.device().update_descriptor_sets(&writes, &[]) };
    Ok(())
}

/// Create an image on the device's local memory, for the producer's own (non-exported) use — the
/// depth attachment.
fn create_image(
    device: &GpuDevice,
    format: vk::Format,
    width: u32,
    height: u32,
    usage: vk::ImageUsageFlags,
) -> Result<(vk::Image, vk::DeviceMemory)> {
    let raw = device.device();
    // SAFETY: every error path frees what this made; the returns are owned by the caller.
    unsafe {
        let image = raw
            .create_image(
                &vk::ImageCreateInfo::default()
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
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .context("creating an image")?;
        let requirements = raw.get_image_memory_requirements(image);
        let memory = match device.memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        ) {
            Ok(index) => raw
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(index),
                    None,
                )
                .context("allocating the image's memory")?,
            Err(error) => {
                raw.destroy_image(image, None);
                return Err(error);
            }
        };
        if let Err(error) = raw.bind_image_memory(image, memory, 0) {
            raw.free_memory(memory, None);
            raw.destroy_image(image, None);
            return Err(anyhow!("binding the image's memory: {error:?}"));
        }
        Ok((image, memory))
    }
}

/// A 2D view over `image`, for the one aspect named.
fn create_view(
    device: &GpuDevice,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> Result<vk::ImageView> {
    // SAFETY: `image` is live and owned by the caller; the view is owned by the caller.
    unsafe {
        device.device().create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
            None,
        )
    }
    .context("creating an image view")
}

/// A fresh `eventfd` the compositor signals once it has finished with a buffer. Non-blocking, so a
/// signal can never stall the compositor, and `CLOEXEC`, so it does not leak into a child.
fn release_descriptor() -> Result<Arc<OwnedFd>> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(anyhow!(
            "creating a release descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `eventfd` returned a fresh descriptor this process owns.
    Ok(Arc::new(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Whether the compositor has signalled `release` — a non-blocking `poll`.
fn release_signalled(release: &OwnedFd) -> bool {
    let mut descriptor = libc::pollfd {
        fd: release.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    (unsafe { libc::poll(&mut descriptor, 1, 0) }) > 0
}

/// The projection the cube is drawn through: right-handed, looking down `-Z`, NDC `y` up and depth
/// in `[0, 1]` — the convention a `wgpu` shader is written against. `naga`'s SPIR-V writer flips Y
/// for Vulkan, so no flip belongs here.
fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let h = 1.0 / (0.5 * fov_y).tan();
    let w = h / aspect;
    let r = far / (near - far);
    [
        w,
        0.0,
        0.0,
        0.0, //
        0.0,
        h,
        0.0,
        0.0, //
        0.0,
        0.0,
        r,
        -1.0, //
        0.0,
        0.0,
        r * near,
        0.0,
    ]
}

/// A right-handed look-at view matrix, column-major.
fn look_at_rh(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let f = normalize(sub(center, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0, //
        s[1],
        u[1],
        -f[1],
        0.0, //
        s[2],
        u[2],
        -f[2],
        0.0, //
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

/// A rotation about `Y`, column-major.
fn rotation_y(angle: f32) -> [f32; 16] {
    let (s, c) = angle.sin_cos();
    [
        c, 0.0, -s, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        s, 0.0, c, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// A rotation about `X`, column-major.
fn rotation_x(angle: f32) -> [f32; 16] {
    let (s, c) = angle.sin_cos();
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, c, s, 0.0, //
        0.0, -s, c, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// The product of two column-major 4×4 matrices.
fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

/// A column-major 4×4 matrix applied to a column vector.
fn transform(m: &[f32; 16], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        out[row] = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    out
}

/// The convex hull of `points`, by Andrew's monotone chain. Fewer than three points come back
/// unchanged; the winding follows the frame's handedness, which [`point_in_convex_hull`] does not care
/// about.
fn convex_hull(points: &[Point<f32>]) -> Vec<Point<f32>> {
    let mut points = points.to_vec();
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let turn = |o: &Point<f32>, a: &Point<f32>, b: &Point<f32>| {
        (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
    };
    let mut hull: Vec<Point<f32>> = Vec::with_capacity(points.len() + 1);
    for point in &points {
        while hull.len() >= 2 && turn(&hull[hull.len() - 2], &hull[hull.len() - 1], point) <= 0.0 {
            hull.pop();
        }
        hull.push(*point);
    }
    // The upper chain must not unpick the lower one, so it stops popping before the lower chain and
    // skips the point the lower chain already ended on.
    let lower = hull.len() + 1;
    for point in points.iter().rev().skip(1) {
        while hull.len() >= lower
            && turn(&hull[hull.len() - 2], &hull[hull.len() - 1], point) <= 0.0
        {
            hull.pop();
        }
        hull.push(*point);
    }
    hull.pop();
    hull
}

/// Whether `p` is inside (or on the edge of) the convex polygon `hull`, by the winding test: a point of
/// a convex polygon lies on the same side of every edge. Fewer than three points bound no area.
fn point_in_convex_hull(hull: &[Point<f32>], p: Point<f32>) -> bool {
    if hull.len() < 3 {
        return false;
    }
    let (mut left, mut right) = (false, false);
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        let side = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        left |= side > 0.0;
        right |= side < 0.0;
        if left && right {
            return false;
        }
    }
    true
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}
