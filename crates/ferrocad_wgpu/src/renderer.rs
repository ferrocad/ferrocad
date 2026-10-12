//! The wgpu scene renderer: encodes passes, owns an offscreen target, and
//! implements [`InventorView`] so a host can composite it or read it back.
//!
//! Two export variants, matching [`InventorView`]:
//!
//! - [`borrow_surface`](InventorView::borrow_surface) — the offscreen
//!   [`wgpu::Texture`], for a host that can composite a shared GPU surface (zero-copy).
//! - [`image`](InventorView::image) — a readback [`ImageData`], the MVP path while
//!   `bite-gpui` has no wgpu surface primitive.
//!
//! The scene is drawn into a multisampled target (4× by default) that resolves into
//! the single-sampled `target` we read back or hand to a host, so edges are
//! anti-aliased.

use std::collections::HashMap;
use std::fmt;

use ferrocad_inventor::{
    Camera, DrawBatch, DrawPacket, Generation, ImageData, InventorView, MeshId, Rect, RenderQueue,
    ViewSpec,
};
use ferrocad_types::Matrix4;

use crate::batch::GpuMesh;
use crate::context::GpuContext;
use crate::pipeline::{self, ScenePipelines};
use crate::textures;
use crate::uniforms::{self, DynamicUniforms, UniformData};

/// What can go wrong while creating a device, encoding, or reading back a frame.
#[derive(Debug)]
pub enum RenderError {
    /// A packet referenced geometry that was neither fresh nor in the cache.
    MissingMesh {
        /// The mesh the packet referred to.
        mesh_id: MeshId,
        /// The generation the packet referred to.
        generation: Generation,
    },
    /// No GPU adapter could be found.
    Adapter,
    /// The adapter could not provide a device.
    Device,
    /// A frame could not be read back to CPU memory.
    Readback(&'static str),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::MissingMesh { mesh_id, generation } => {
                write!(
                    f,
                    "no geometry for mesh {} generation {}",
                    mesh_id.0, generation.0
                )
            }
            RenderError::Adapter => write!(f, "no suitable GPU adapter found"),
            RenderError::Device => write!(f, "could not create a GPU device"),
            RenderError::Readback(reason) => write!(f, "readback failed: {reason}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// The offscreen target format (matches gpui's `RenderImage` RGBA expectations).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The default multisample count (4× anti-aliasing).
const DEFAULT_SAMPLES: u32 = 4;

/// The offscreen targets: the multisampled colour (when AA is on), the resolved
/// single-sampled colour, and the depth attachment.
struct Targets {
    /// Single-sampled colour target: what is read back or shared.
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    /// Multisampled colour target (rendered into, resolved into `color`). `None`
    /// when anti-aliasing is off (`sample_count == 1`).
    msaa_view: Option<wgpu::TextureView>,
    /// Kept alive so [`Targets::msaa_view`] stays valid.
    _msaa: Option<wgpu::Texture>,
    /// Kept alive so [`Targets::depth_view`] stays valid.
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    sample_count: u32,
}

impl Targets {
    fn new(device: &wgpu::Device, size: [u32; 2], sample_count: u32) -> Self {
        let sample_count = sample_count.max(1);
        let (color, color_view) = create_color(device, size);
        let (msaa, msaa_view) = if sample_count > 1 {
            let (texture, view) = create_msaa_color(device, size, sample_count);
            (Some(texture), Some(view))
        } else {
            (None, None)
        };
        let (depth, depth_view) = create_depth(device, size, sample_count);
        Self {
            color,
            color_view,
            msaa_view,
            _msaa: msaa,
            _depth: depth,
            depth_view,
            sample_count,
        }
    }
}

/// Draws compiled scenes with wgpu and exposes the result to a host.
pub struct WgpuRenderer {
    context: GpuContext,
    targets: Targets,
    size: [u32; 2],
    uniform_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    uniforms: DynamicUniforms,
    /// Bound by shapes with no texture.
    default_texture: wgpu::BindGroup,
    /// Uploaded textures, keyed on the `Texture2` id.
    texture_cache: HashMap<u64, (wgpu::Texture, wgpu::BindGroup)>,
    pipelines: ScenePipelines,
    /// Uploaded meshes, keyed on `(mesh_id, generation)`.
    mesh_cache: HashMap<(MeshId, Generation), GpuMesh>,
}

impl WgpuRenderer {
    /// Create a renderer with an offscreen target `size`, at [`DEFAULT_SAMPLES`].
    pub fn new(context: GpuContext, size: [u32; 2]) -> Self {
        Self::with_sample_count(context, size, DEFAULT_SAMPLES)
    }

    /// Create a renderer at a given `sample_count` (1 disables anti-aliasing; 4 is
    /// the usual choice). The count must be supported by the device for both the
    /// colour and depth formats.
    pub fn with_sample_count(context: GpuContext, size: [u32; 2], sample_count: u32) -> Self {
        let size = clamp_size(size);
        let targets = Targets::new(&context.device, size, sample_count);
        let uniform_layout = uniforms::layout(&context.device);
        let textures_layout = textures::layout(&context.device);
        let (_, default_texture) =
            textures::default_texture(&context.device, &context.queue, &textures_layout);
        let uniforms = DynamicUniforms::new(
            &context.device,
            &uniform_layout,
            context.device.limits().min_uniform_buffer_offset_alignment,
            1,
        );
        let pipelines = ScenePipelines::new(
            &context.device,
            TARGET_FORMAT,
            &uniform_layout,
            &textures_layout,
            targets.sample_count,
        );
        Self {
            context,
            targets,
            size,
            uniform_layout,
            textures_layout,
            uniforms,
            default_texture,
            texture_cache: HashMap::new(),
            pipelines,
            mesh_cache: HashMap::new(),
        }
    }

    /// Create a renderer on its own freshly-requested device.
    ///
    /// Handy for the image path, where the host (gpui) does not hand out its device.
    /// Picks 4× MSAA when the adapter supports it, else 1×.
    pub fn new_default(size: [u32; 2]) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|_| RenderError::Adapter)?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ferrocad"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|_| RenderError::Device)?;

        let samples = if supports_samples(&adapter, DEFAULT_SAMPLES) {
            DEFAULT_SAMPLES
        } else {
            1
        };
        Ok(Self::with_sample_count(GpuContext::new(device, queue), size, samples))
    }

    /// The GPU context this renderer owns.
    pub fn context(&self) -> &GpuContext {
        &self.context
    }

    /// Resize the offscreen target. No-op when `size` is unchanged.
    pub fn resize(&mut self, size: [u32; 2]) {
        let size = clamp_size(size);
        if size == self.size {
            return;
        }
        self.targets = Targets::new(&self.context.device, size, self.targets.sample_count);
        self.size = size;
    }

    /// The offscreen (single-sampled, resolved) target as a texture.
    pub fn surface(&self) -> &wgpu::Texture {
        &self.targets.color
    }

    /// The multisample count in use.
    pub fn sample_count(&self) -> u32 {
        self.targets.sample_count
    }

    /// Encode and submit a frame drawing `batches` into `target`.
    ///
    /// Fresh geometry (cache misses) is uploaded first; each packet is then drawn
    /// with its [`StateSnapshot`](ferrocad_inventor::StateSnapshot) bound and
    /// `camera`'s view-projection.
    pub fn render<B: DrawBatch>(
        &mut self,
        target: &wgpu::TextureView,
        camera: &Camera,
        batches: &[B],
    ) -> Result<(), RenderError> {
        let count = batches.len() as u32;
        self.ensure_uniform_capacity(count);

        // Upload anything not already cached. A stable scene keeps the same
        // `(mesh_id, generation)`, so this uploads once and reuses thereafter.
        for batch in batches {
            if batch.geometry().is_some() {
                let key = (batch.mesh_id(), batch.generation());
                if !self.mesh_cache.contains_key(&key) {
                    let geometry = batch.geometry().expect("checked above");
                    self.mesh_cache
                        .insert(key, GpuMesh::upload(&self.context.device, geometry));
                }
            }
        }
        self.ensure_textures(batches);

        let view_proj = mat4_to_wgsl(&camera.view_projection());
        for (index, batch) in batches.iter().enumerate() {
            let snapshot = batch.state();
            let data = UniformData {
                view_proj,
                model: mat4_to_wgsl(&snapshot.world_transform),
                color: snapshot.color,
            };
            self.uniforms.write(&self.context.queue, index as u32, &data);
        }

        // With MSAA on, draw into the multisampled target and resolve into `target`.
        let (attachment_view, resolve_target) = match &self.targets.msaa_view {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };

        let mut encoder = self.context.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("ferrocad scene"),
            },
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ferrocad scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: attachment_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            for (index, batch) in batches.iter().enumerate() {
                let key = (batch.mesh_id(), batch.generation());
                let Some(mesh) = self.mesh_cache.get(&key) else {
                    continue;
                };
                pass.set_pipeline(self.pipelines.for_draw(false, batch.topology()));
                pass.set_bind_group(
                    0,
                    &self.uniforms.bind_group,
                    &[self.uniforms.offset(index as u32)],
                );
                pass.set_bind_group(1, self.texture_bind_group(batch), &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
        self.context.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Draw a frame of one or more [`ViewSpec`]s in one pass: the main view first
    /// (clearing), then each overlay confined to its rectangle (`set_viewport` +
    /// `set_scissor_rect`), with the overlay pipeline (depth off, blending).
    ///
    /// This is what a host uses to composite the navigation cube over the scene; the
    /// single-view [`render`](Self::render) is the special case of one full-target
    /// view.
    pub fn render_views(
        &mut self,
        target: &wgpu::TextureView,
        views: &[ViewSpec<'_>],
    ) -> Result<(), RenderError> {
        let total: u32 = views
            .iter()
            .map(|view| view.queue.packets.len() as u32)
            .sum();
        self.ensure_uniform_capacity(total);

        // Upload every view's fresh geometry, then write one uniform slot per packet
        // (in view order) with that view's view-projection.
        for view in views {
            self.upload_packets(&view.queue.packets);
            self.ensure_textures(&view.queue.packets);
        }
        let mut slot = 0u32;
        for view in views {
            let view_proj = mat4_to_wgsl(&view.camera.view_projection());
            for batch in &view.queue.packets {
                let snapshot = batch.state();
                self.uniforms.write(
                    &self.context.queue,
                    slot,
                    &UniformData {
                        view_proj,
                        model: mat4_to_wgsl(&snapshot.world_transform),
                        color: snapshot.color,
                    },
                );
                slot += 1;
            }
        }

        let (attachment_view, resolve_target) = match &self.targets.msaa_view {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        let [width, height] = self.size;

        let mut encoder = self.context.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("ferrocad frame"),
            },
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ferrocad frame pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: attachment_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let mut slot = 0u32;
            for view in views {
                // Overlays are confined to their rectangle; the main view fills the
                // target. The rect is bottom-left origin, wgpu's viewport/scissor are
                // top-left origin.
                let (x, y, w, h) = match view.rect {
                    Some(rect) => wgpu_rect(rect, height),
                    None => (0.0, 0.0, width as f32, height as f32),
                };
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
                for batch in &view.queue.packets {
                    let key = (batch.mesh_id, batch.generation);
                    if let Some(mesh) = self.mesh_cache.get(&key) {
                        pass.set_pipeline(self.pipelines.for_draw(view.overlay, batch.topology));
                        pass.set_bind_group(
                            0,
                            &self.uniforms.bind_group,
                            &[self.uniforms.offset(slot)],
                        );
                        pass.set_bind_group(1, self.texture_bind_group(batch), &[]);
                        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                    slot += 1;
                }
            }
        }
        self.context.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Grow the dynamic-uniform buffer so it holds at least `count` slots.
    fn ensure_uniform_capacity(&mut self, count: u32) {
        if count > self.uniforms.capacity {
            self.uniforms = DynamicUniforms::new(
                &self.context.device,
                &self.uniform_layout,
                self.context.device.limits().min_uniform_buffer_offset_alignment,
                count,
            );
        }
    }

    /// Upload the fresh geometry of each packet, keyed on `(mesh_id, generation)`.
    fn upload_packets(&mut self, packets: &[DrawPacket]) {
        for packet in packets {
            if let Some(geometry) = packet.geometry.as_ref() {
                let key = (packet.mesh_id, packet.generation);
                if !self.mesh_cache.contains_key(&key) {
                    self.mesh_cache
                        .insert(key, GpuMesh::upload(&self.context.device, geometry));
                }
            }
        }
    }

    /// Upload any texture a batch names that is not already cached (keyed on its id).
    fn ensure_textures<B: DrawBatch>(&mut self, batches: &[B]) {
        for batch in batches {
            if let Some(texture) = batch.state().texture.as_ref() {
                if !self.texture_cache.contains_key(&texture.id) {
                    let entry = textures::upload_texture2(
                        &self.context.device,
                        &self.context.queue,
                        &self.textures_layout,
                        texture,
                    );
                    self.texture_cache.insert(texture.id, entry);
                }
            }
        }
    }

    /// The texture bind group (group 1) for a batch: its texture, or the white default.
    fn texture_bind_group<B: DrawBatch>(&self, batch: &B) -> &wgpu::BindGroup {
        batch
            .state()
            .texture
            .as_ref()
            .and_then(|texture| self.texture_cache.get(&texture.id))
            .map(|(_, group)| group)
            .unwrap_or(&self.default_texture)
    }

    /// Read `texture` back into RGBA8, stripping the row padding wgpu requires.
    fn read_back(&self, texture: &wgpu::Texture) -> Result<ImageData, RenderError> {
        let [width, height] = self.size;
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;

        let buffer = self.context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self.context.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("ferrocad readback"),
            },
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.context.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.context
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|_| RenderError::Readback("device poll failed"))?;

        let view = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            rgba.extend_from_slice(&view[start..start + unpadded as usize]);
        }
        drop(view);
        buffer.unmap();

        Ok(ImageData {
            width,
            height,
            rgba,
        })
    }
}

impl InventorView for WgpuRenderer {
    type Surface = wgpu::Texture;
    type Error = RenderError;

    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn draw(&mut self, queue: &RenderQueue, camera: &Camera) -> Result<(), Self::Error> {
        // Clone the view handle so the borrow of the target view does not alias the
        // `&mut self` needed by `render` (a `TextureView` is a cheap handle).
        let target = self.targets.color_view.clone();
        self.render(&target, camera, &queue.packets)
    }

    fn draw_frame(&mut self, views: &[ViewSpec<'_>]) -> Result<(), Self::Error> {
        let target = self.targets.color_view.clone();
        self.render_views(&target, views)
    }

    fn borrow_surface(&self) -> Option<&Self::Surface> {
        Some(&self.targets.color)
    }

    fn image(&mut self) -> Result<ImageData, Self::Error> {
        self.read_back(&self.targets.color)
    }
}

/// Whether the adapter supports `count` samples for both the colour and depth formats.
fn supports_samples(adapter: &wgpu::Adapter, count: u32) -> bool {
    adapter
        .get_texture_format_features(TARGET_FORMAT)
        .flags
        .sample_count_supported(count)
        && adapter
            .get_texture_format_features(pipeline::DEPTH_FORMAT)
            .flags
            .sample_count_supported(count)
}

/// Convert a row-major [`Matrix4`] to the column-major layout WGSL uniforms use.
fn mat4_to_wgsl(m: &Matrix4) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for row in 0..4 {
        for col in 0..4 {
            out[col][row] = m.m[row * 4 + col] as f32;
        }
    }
    out
}

/// Convert a bottom-left-origin device [`Rect`] into a wgpu top-left-origin
/// `(x, y, width, height)`, clamped to the target.
fn wgpu_rect(rect: Rect, target_height: u32) -> (f32, f32, f32, f32) {
    let w = rect.width.max(1) as f32;
    let h = rect.height.max(1) as f32;
    let x = rect.x.max(0) as f32;
    let y = (target_height as f32 - (rect.y as f32 + h)).clamp(0.0, target_height as f32);
    (x, y, w, h)
}

/// A target size with each dimension at least one pixel (textures cannot be empty).
fn clamp_size([width, height]: [u32; 2]) -> [u32; 2] {
    [width.max(1), height.max(1)]
}

/// Create the single-sampled colour target (the resolve/readback target).
fn create_color(device: &wgpu::Device, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
    let [width, height] = clamp_size(size);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ferrocad target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Create the multisampled colour target that resolves into [`create_color`]'s.
fn create_msaa_color(
    device: &wgpu::Device,
    size: [u32; 2],
    sample_count: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let [width, height] = clamp_size(size);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ferrocad msaa"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Create the depth target and its view.
fn create_depth(
    device: &wgpu::Device,
    size: [u32; 2],
    sample_count: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let [width, height] = clamp_size(size);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ferrocad depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: sample_count.max(1),
        dimension: wgpu::TextureDimension::D2,
        format: pipeline::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Drive a future to completion on the current thread (device init only).
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::{Node, NodeKind, Scene, Topology, compile};

    /// A triangle already in clip space, for use with [`Camera::identity`].
    fn clip_space_triangle() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    Node {
                        kind: NodeKind::Coordinate3(vec![
                            [-0.5, -0.5, 0.5],
                            [0.5, -0.5, 0.5],
                            [0.0, 0.5, 0.5],
                        ]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Mesh {
                            indices: vec![0, 1, 2],
                            normal_indices: vec![],
                            topology: Topology::Triangles,
                        },
                        children: vec![],
                    },
                ],
            },
        }
    }

    /// End-to-end: compile a scene, render it, read the frame back. Skips (rather
    /// than fails) when the machine has no GPU adapter, so CI without a GPU passes.
    #[test]
    fn renders_and_reads_back() {
        let queue = compile(&clip_space_triangle());
        let Ok(mut renderer) = WgpuRenderer::new_default([64, 64]) else {
            eprintln!("no GPU adapter available; skipping offscreen render test");
            return;
        };
        renderer.draw(&queue, &Camera::identity()).expect("draw");
        let image = renderer.image().expect("readback");
        assert_eq!([image.width, image.height], [64, 64]);
        assert_eq!(image.rgba.len(), 64 * 64 * 4);

        let texel = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        assert_ne!(
            texel(32, 32),
            texel(0, 0),
            "the triangle should cover the centre but not the corner"
        );
    }

    /// An overlay (the navigation cube) draws into its rectangle only, over the main
    /// view; the readback shows its colour in the corner. Skips without a GPU.
    #[test]
    fn overlay_draws_into_its_rect() {
        let main = compile(&clip_space_triangle());
        let cube = compile(&ferrocad_inventor::navicube::scene(None));
        let Ok(mut renderer) = WgpuRenderer::new_default([128, 128]) else {
            eprintln!("no GPU adapter available; skipping overlay test");
            return;
        };
        // A 50x50 overlay in the top-right corner (device rect, bottom-left origin).
        let rect = Rect::new(78, 78, 50, 50);
        let cube_camera = ferrocad_inventor::navicube::camera(rect);
        let main_camera = Camera::identity();
        let views = [
            ViewSpec {
                queue: &main,
                camera: &main_camera,
                rect: None,
                overlay: false,
            },
            ViewSpec {
                queue: &cube,
                camera: &cube_camera,
                rect: Some(rect),
                overlay: true,
            },
        ];
        renderer.draw_frame(&views).expect("draw_frame");
        let image = renderer.image().expect("readback");
        let texel = |x: u32, y: u32| {
            let i = ((y * 128 + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        // Image is top-left origin, the rect bottom-left origin: the overlay is in the
        // image's top-right. Its centre differs from the clear-coloured far corner.
        assert_ne!(
            texel(103, 24),
            texel(5, 5),
            "the navigation cube should paint its corner over the main view"
        );
    }
}
