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

use ferrocad_graphics::{
    Camera, DrawBatch, DrawPacket, Generation, Geometry, Hit, MeshId, PickId, Rect, RenderQueue,
    Topology,
};
use ferrocad_inventor::{ImageData, InventorView, ViewSpec};
use ferrocad_types::{Matrix4, Vector3};

use crate::batch::GpuMesh;
use crate::context::GpuContext;
use crate::pick::{self, PickPipelines, PickTargets};
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
    /// Uploaded meshes, keyed on `mesh_id`. A newer generation **replaces** (and drops)
    /// the older GPU buffers, so transient rewrites (tool cursor moves, gizmo drags) do
    /// not leak entries across frames.
    mesh_cache: HashMap<MeshId, GpuMesh>,
    /// The single-sampled pick pipelines (the offscreen G-buffer pass).
    pick_pipelines: PickPipelines,
    /// The pick targets, allocated lazily on the first [`pick_at`](Self::pick_at) and
    /// dropped on resize (they track the target size).
    pick_targets: Option<PickTargets>,
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
        let pick_pipelines = PickPipelines::new(&context.device, &uniform_layout);
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
            pick_pipelines,
            pick_targets: None,
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
        self.pick_targets = None; // re-allocated at the new size on the next pick
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
    /// with its [`StateSnapshot`](ferrocad_graphics::StateSnapshot) bound and
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
            if let Some(geometry) = batch.geometry() {
                self.upload_geometry(batch.mesh_id(), batch.generation(), geometry);
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
                pick: snapshot.id.0,
                topology: topology_ordinal(batch.topology()),
                _padding: [0; 2],
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
                let Some(mesh) = self.mesh_cache.get(&batch.mesh_id()) else {
                    continue;
                };
                if mesh.generation != batch.generation() {
                    continue;
                }
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
                        pick: snapshot.id.0,
                        topology: topology_ordinal(batch.topology),
                        _padding: [0; 2],
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
                    let Some(mesh) = self.mesh_cache.get(&batch.mesh_id) else {
                        slot += 1;
                        continue;
                    };
                    if mesh.generation == batch.generation {
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

    /// Pick the object under a **device** pixel (bottom-left origin) with an offscreen
    /// G-buffer pass.
    ///
    /// The queue (the main scene view) is drawn once more into the pick targets, encoding
    /// each packet's id, its vertices' topological sub-element index and the surface
    /// normal; one texel of each target is then read back and the world point unprojected
    /// through `camera`. `None` outside the target, when nothing is under the pixel, or
    /// when the view-projection is singular.
    ///
    /// Unlike the CPU ray cast, this resolves the exact rasterised pixel — so it agrees
    /// with what the user sees for overlapping or concave geometry. Because the pick pass
    /// is single-sampled and offscreen, it is independent of the scene's MSAA target.
    pub fn pick_at(
        &mut self,
        queue: &RenderQueue,
        camera: &Camera,
        device: [i32; 2],
    ) -> Option<Hit> {
        let [width, height] = self.size;
        let [x, y] = device;
        if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height || queue.is_empty() {
            return None;
        }

        // Re-draw the queue into the pick targets: fresh geometry (a no-op cache hit for
        // the frame that just drew it) and one uniform slot per packet.
        self.ensure_uniform_capacity(queue.packets.len() as u32);
        self.upload_packets(&queue.packets);
        let view_proj = mat4_to_wgsl(&camera.view_projection());
        for (index, packet) in queue.packets.iter().enumerate() {
            self.uniforms.write(
                &self.context.queue,
                index as u32,
                &UniformData {
                    view_proj,
                    model: mat4_to_wgsl(&packet.state.world_transform),
                    color: packet.state.color,
                    pick: packet.state.id.0,
                    topology: topology_ordinal(packet.topology),
                    _padding: [0; 2],
                },
            );
        }

        if self.pick_targets.is_none() {
            self.pick_targets = Some(PickTargets::new(&self.context.device, self.size));
        }
        let targets = self.pick_targets.as_ref()?;

        let mut encoder = self.context.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("ferrocad pick"),
            },
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ferrocad pick pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.id_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Id 0 = "empty": a hit overwrites it with the packet's id.
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.normal_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.depth_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_buffer_view,
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
            for (index, packet) in queue.packets.iter().enumerate() {
                let Some(mesh) = self.mesh_cache.get(&packet.mesh_id) else {
                    continue;
                };
                if mesh.generation != packet.generation {
                    continue;
                }
                pass.set_pipeline(self.pick_pipelines.for_topology(packet.topology));
                pass.set_bind_group(
                    0,
                    &self.uniforms.bind_group,
                    &[self.uniforms.offset(index as u32)],
                );
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
        self.context.queue.submit(Some(encoder.finish()));

        // `device` is bottom-left origin; the pick targets are top-left origin, so the
        // copy row flips. The unprojection below uses the (unflipped) device pixel centre.
        let origin = wgpu::Origin3d {
            x: x as u32,
            y: height - 1 - y as u32,
            z: 0,
        };
        let (gpu_device, gpu_queue) = (&self.context.device, &self.context.queue);
        let ids = pick::read_texel(
            gpu_device,
            gpu_queue,
            &targets.id,
            wgpu::TextureAspect::All,
            8,
            origin,
        )
        .ok()?;
        let normals = pick::read_texel(
            gpu_device,
            gpu_queue,
            &targets.normal,
            wgpu::TextureAspect::All,
            16,
            origin,
        )
        .ok()?;
        let depths = pick::read_texel(
            gpu_device,
            gpu_queue,
            &targets.depth,
            wgpu::TextureAspect::All,
            4,
            origin,
        )
        .ok()?;

        let entity_raw = read_u32(&ids)?;
        if entity_raw == 0 {
            // Empty space: the target was cleared to 0, and a hit always writes id + 1,
            // so `PickId(0)` (the compiler's default) remains a valid hit.
            return None;
        }
        let entity = entity_raw - 1;
        let element = read_u32(&ids[4..])?;
        let normal = [
            read_f32(&normals)?,
            read_f32(&normals[4..])?,
            read_f32(&normals[8..])?,
        ];
        let topology = topology_from_ordinal(read_f32(&normals[12..])? as u32);
        let depth = read_f32(&depths)?;

        let [width_f, height_f] = [f64::from(width), f64::from(height)];
        let ndc = [
            2.0 * (f64::from(x) + 0.5) / width_f - 1.0,
            2.0 * (f64::from(y) + 0.5) / height_f - 1.0,
            f64::from(depth),
        ];
        let inverse = camera.view_projection().inverse()?;
        let point = unproject(&inverse, ndc);

        // The G-buffer carries no ray parameter; the eye is the view-space origin.
        let eye = camera.view.inverse()?.transform(&Vector3::zero());
        let distance = Vector3::new(
            f64::from(point[0]),
            f64::from(point[1]),
            f64::from(point[2]),
        )
        .sub(&eye)
        .length();

        Some(Hit {
            id: PickId(entity),
            point,
            normal,
            element,
            topology,
            distance,
        })
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

    /// Upload the fresh geometry of each packet, evicting a stale generation for the
    /// same `mesh_id`.
    fn upload_packets(&mut self, packets: &[DrawPacket]) {
        for packet in packets {
            if let Some(geometry) = packet.geometry.as_ref() {
                self.upload_geometry(packet.mesh_id, packet.generation, geometry);
            }
        }
    }

    /// Upload `geometry` for `mesh_id`, unless that exact generation is already resident.
    ///
    /// A changed generation **replaces** the cache entry, so the previous GPU buffers are
    /// dropped immediately (generational eviction) rather than accumulating per edit.
    fn upload_geometry(&mut self, mesh_id: MeshId, generation: Generation, geometry: &Geometry) {
        if self.mesh_cache.get(&mesh_id).map(|mesh| mesh.generation) == Some(generation) {
            return;
        }
        let uploaded = GpuMesh::upload(&self.context.device, geometry, generation);
        self.mesh_cache.insert(mesh_id, uploaded);
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

    fn resize(&mut self, size: [u32; 2]) {
        WgpuRenderer::resize(self, size);
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

    fn supports_pick(&self) -> bool {
        true
    }

    fn pick_at(&mut self, queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
        // The inherent method (the G-buffer pass), not this trait method.
        WgpuRenderer::pick_at(self, queue, camera, device)
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

/// The topology ordinal the pick shader writes to the normal target's `w` channel.
fn topology_ordinal(topology: Topology) -> u32 {
    match topology {
        Topology::Triangles => 0,
        Topology::Lines => 1,
        Topology::Points => 2,
    }
}

/// Decode [`topology_ordinal`]; anything unexpected is treated as triangles.
fn topology_from_ordinal(ordinal: u32) -> Topology {
    match ordinal {
        1 => Topology::Lines,
        2 => Topology::Points,
        _ => Topology::Triangles,
    }
}

/// Read a little-endian `u32` from the first four bytes.
fn read_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// Read a little-endian `f32` from the first four bytes.
fn read_f32(bytes: &[u8]) -> Option<f32> {
    Some(f32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// Unproject a clip-space NDC point with the inverse (row-major) view-projection.
fn unproject(inverse: &Matrix4, ndc: [f64; 3]) -> [f32; 3] {
    let v = [ndc[0], ndc[1], ndc[2], 1.0];
    let mut out = [0.0f64; 4];
    for row in 0..4 {
        out[row] = (0..4).map(|col| inverse.m[row * 4 + col] * v[col]).sum();
    }
    let w = if out[3] != 0.0 { out[3] } else { 1.0 };
    [(out[0] / w) as f32, (out[1] / w) as f32, (out[2] / w) as f32]
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
    use ferrocad_graphics::{PickId, Topology, Viewport};
    use ferrocad_inventor::{Node, NodeKind, Scene, compile, compile_with};

    /// Serializes the GPU tests. The suite runs in one process with one thread per test,
    /// and the (software) adapter can fault when several `wgpu` devices are driven at
    /// once, so every GPU test holds this for its duration.
    static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Acquire [`GPU_LOCK`], ignoring poisoning (a failed test already released it).
    fn gpu_guard() -> std::sync::MutexGuard<'static, ()> {
        GPU_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

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
        let _gpu = gpu_guard();
        let (queue, _) = compile(&clip_space_triangle());
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

    /// A quad covering the overlay's ortho view, for [`overlay_draws_into_its_rect`].
    fn overlay_quad() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    Node {
                        kind: NodeKind::Material(vec![[0.8, 0.2, 0.2, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(vec![
                            [-1.0, -1.0, 0.0],
                            [1.0, -1.0, 0.0],
                            [1.0, 1.0, 0.0],
                            [-1.0, 1.0, 0.0],
                        ]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Mesh {
                            indices: vec![0, 1, 2, 0, 2, 3],
                            normal_indices: vec![],
                            topology: Topology::Triangles,
                        },
                        children: vec![],
                    },
                ],
            },
        }
    }

    /// The fixed orthographic overlay camera for `rect` (the navicube's overlay camera,
    /// inlined here so the backend does not depend on the viewport).
    fn overlay_camera(rect: Rect) -> Camera {
        Camera {
            view: ferrocad_types::Placement::new(
                ferrocad_types::Vector3::new(0.0, 0.0, -3.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            projection: ferrocad_graphics::Projection::Orthographic {
                extent: 1.1,
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new([rect.width.max(1), rect.height.max(1)], 1.0),
        }
    }

    /// An overlay draws into its rectangle only, over the main view; the readback shows
    /// its colour in the corner. Skips without a GPU.
    #[test]
    fn overlay_draws_into_its_rect() {
        let _gpu = gpu_guard();
        let (main, _) = compile(&clip_space_triangle());
        let (cube, _) = compile(&overlay_quad());
        let Ok(mut renderer) = WgpuRenderer::new_default([128, 128]) else {
            eprintln!("no GPU adapter available; skipping overlay test");
            return;
        };
        // A 50x50 overlay in the top-right corner (device rect, bottom-left origin).
        let rect = Rect::new(78, 78, 50, 50);
        let cube_camera = overlay_camera(rect);
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

    /// A legacy inventor scene and a modern [`ferrocad_stage`] item render together in
    /// **one** pass over one depth buffer, and share the `(mesh_id, generation)` upload
    /// cache across frames. The two producers are spatially separated (the coplanar-edge
    /// depth bias is deferred to T4). Skips (rather than fails) without a GPU adapter.
    #[test]
    fn a_stage_and_an_inventor_scene_coexist_in_one_pass() {
        use ferrocad_stage::{DrawMode, ShapeItem, Stage};

        let _gpu = gpu_guard();
        let Ok(mut renderer) = WgpuRenderer::new_default([64, 64]) else {
            eprintln!("no GPU adapter available; skipping coexistence test");
            return;
        };

        // Legacy: a red surface triangle on the left half (clip space, identity camera).
        let scene = Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    Node {
                        kind: NodeKind::Material(vec![[1.0, 0.0, 0.0, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(vec![
                            [-0.9, -0.5, 0.5],
                            [-0.1, -0.5, 0.5],
                            [-0.5, 0.5, 0.5],
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
        };

        // Modern: a blue `Shaded` surface on the right half, and a separate `Wireframe`
        // triangle along the bottom — the two `DrawMode`s as distinct items.
        let stage = Stage::new()
            .with_item(
                ShapeItem::new(1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_surface_color([0.0, 0.0, 1.0, 1.0])
                    .with_shape(
                        vec![[0.1, -0.5, 0.5], [0.9, -0.5, 0.5], [0.5, 0.5, 0.5]],
                        vec![],
                        vec![0, 1, 2],
                        vec![],
                    ),
            )
            .with_item(
                ShapeItem::new(2)
                    .with_draw_mode(DrawMode::Wireframe)
                    .with_edge_color([0.0, 1.0, 0.0, 1.0])
                    .with_shape(
                        vec![[-0.5, -0.9, 0.5], [0.5, -0.9, 0.5], [0.0, -0.6, 0.5]],
                        vec![],
                        vec![],
                        vec![0, 1, 1, 2, 2, 0],
                    ),
            );

        let viewport = Viewport::new([64, 64], 1.0);
        let camera = Camera::identity();

        // One unified queue: the legacy compile, then the stage's batches appended.
        let merge = || {
            let (mut queue, _) = compile_with(&scene, &viewport);
            stage.emit_batches(&mut queue);
            queue
        };

        let queue = merge();
        assert_eq!(queue.len(), 3, "one legacy surface + one stage surface + one stage wire");
        // Both producers contributed, and the stage's line packets are present.
        assert!(queue.packets.iter().any(|p| p.state.id == PickId(0)), "legacy packet");
        assert!(queue.packets.iter().any(|p| p.state.id == PickId(1)), "stage surface");
        assert!(
            queue
                .packets
                .iter()
                .any(|p| p.state.id == PickId(2) && p.topology == Topology::Lines),
            "stage wireframe"
        );

        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .expect("frame 1");
        let uploaded = renderer.mesh_cache.len();
        assert_eq!(uploaded, 3, "the first frame uploads one mesh per packet");

        // Frame 2 rebuilds the same queue: the mesh ids are stable across compiles, so
        // every upload is a cache hit and nothing new is allocated on the GPU.
        let queue = merge();
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .expect("frame 2");
        assert_eq!(
            renderer.mesh_cache.len(),
            uploaded,
            "unchanged items must reuse the upload cache"
        );

        let image = renderer.image().expect("readback");
        let texel = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        let background = texel(2, 2);
        // NDC x = -0.5 -> pixel x = 16; NDC x = +0.5 -> pixel x = 48 (both at y = 32).
        let legacy = texel(16, 32);
        let modern = texel(48, 32);
        assert_ne!(legacy, background, "the inventor triangle should rasterize");
        assert_ne!(modern, background, "the stage triangle should rasterize");
        assert_ne!(legacy, modern, "the two producers are distinct objects");
        assert!(legacy[0] > legacy[2], "the inventor triangle is red-dominant: {legacy:?}");
        assert!(modern[2] > modern[0], "the stage triangle is blue-dominant: {modern:?}");
    }

    /// A coplanar edge wireframe (CAD shaded-with-edges) must win over its surface: the
    /// surface pipeline carries a depth bias, so the unbiased lines render on top with no
    /// z-fighting. Skips (rather than fails) without a GPU adapter.
    #[test]
    fn coplanar_edges_render_over_the_shaded_surface() {
        use ferrocad_stage::{DrawMode, ShapeItem, Stage};

        let _gpu = gpu_guard();
        let Ok(mut renderer) = WgpuRenderer::new_default([64, 64]) else {
            eprintln!("no GPU adapter available; skipping depth-bias test");
            return;
        };

        // A quad at z = 0.5 (clip space, identity camera) with coplanar hatch lines across
        // it, each aligned to a pixel-row centre so the 1px lines are fully covered.
        let mut positions = vec![
            [-0.8, -0.8, 0.5],
            [0.8, -0.8, 0.5],
            [0.8, 0.8, 0.5],
            [-0.8, 0.8, 0.5],
        ];
        let mut edges = Vec::new();
        for row in [10u32, 16, 22, 28, 34, 40] {
            let y = 2.0 * (row as f32 + 0.5) / 64.0 - 1.0;
            let base = positions.len() as u32;
            positions.push([-0.8, y, 0.5]);
            positions.push([0.8, y, 0.5]);
            edges.extend_from_slice(&[base, base + 1]);
        }
        let item = ShapeItem::new(1)
            .with_draw_mode(DrawMode::ShadedWithEdges)
            .with_surface_color([0.2, 0.3, 0.95, 1.0])
            .with_edge_color([0.9, 0.2, 0.2, 1.0])
            .with_shape(positions, vec![], vec![0, 1, 2, 0, 2, 3], edges);
        let queue = Stage::new().with_item(item).to_queue();

        let camera = Camera::identity();
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .expect("draw");
        let image = renderer.image().expect("readback");

        // Red-dominant pixels are the edge wireframe (blue-dominant ones are the surface,
        // and the clear colour is near-neutral).
        let edge_pixels = image
            .rgba
            .chunks_exact(4)
            .filter(|pixel| i32::from(pixel[0]) - i32::from(pixel[2]) > 30)
            .count();
        assert!(
            edge_pixels > 100,
            "the coplanar edge wireframe should win over the surface ({edge_pixels} edge pixels)"
        );
    }

    /// Generational eviction: repeated geometry rewrites of one item keep the cache a
    /// constant size, with no leak across tool cursor moves or gizmo drags. Skips without
    /// a GPU adapter.
    #[test]
    fn repeated_shape_rewrites_do_not_grow_the_mesh_cache() {
        use ferrocad_stage::{DrawMode, ShapeItem};

        let _gpu = gpu_guard();
        let Ok(mut renderer) = WgpuRenderer::new_default([32, 32]) else {
            eprintln!("no GPU adapter available; skipping cache-eviction test");
            return;
        };

        let mut item = ShapeItem::new(1)
            .with_draw_mode(DrawMode::Shaded)
            .with_shape(
                vec![[0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [0.0, 1.0, 0.5]],
                vec![],
                vec![0, 1, 2],
                vec![],
            );

        let camera = Camera::identity();
        for step in 0..25u32 {
            let offset = step as f32 * 0.01;
            item.set_shape(
                vec![[0.0, 0.0, 0.5], [1.0 + offset, 0.0, 0.5], [0.0, 1.0, 0.5]],
                vec![],
                vec![0, 1, 2],
                vec![],
            );
            let mut queue = RenderQueue::default();
            item.emit(&mut queue);
            renderer
                .draw_frame(&[ViewSpec {
                    queue: &queue,
                    camera: &camera,
                    rect: None,
                    overlay: false,
                }])
                .expect("draw");
            assert_eq!(
                renderer.mesh_cache.len(),
                1,
                "the cache holds one mesh after {step} rewrites (no leak)"
            );
        }
    }

    /// A draw packet with an explicit pick id, per-vertex sub-element index and topology,
    /// for driving the G-buffer pass directly (no producer). Identity camera, so the
    /// positions are clip-space *and* world coordinates at the sampled depth.
    fn pick_packet(
        mesh: u64,
        id: u32,
        part: u32,
        topology: Topology,
        positions: Vec<[f32; 3]>,
        indices: Vec<u32>,
    ) -> DrawPacket {
        use ferrocad_graphics::{Generation, StateSnapshot, Vertex};
        DrawPacket {
            mesh_id: MeshId(mesh),
            generation: Generation(1),
            topology,
            geometry: Some(Geometry {
                vertices: positions
                    .iter()
                    .map(|position| Vertex {
                        position: *position,
                        normal: [0.0, 0.0, 1.0],
                        uv: [0.0, 0.0],
                        part_idx: part,
                    })
                    .collect(),
                indices,
            }),
            state: StateSnapshot {
                world_transform: Matrix4::identity(),
                color: [1.0, 1.0, 1.0, 1.0],
                depth_bias: 0.0,
                texture: None,
                id: PickId(id),
            },
        }
    }

    /// The offscreen G-buffer pass resolves the object under a pixel, its world point and
    /// its surface normal — the hardware replacement for the CPU ray cast. Skips (rather
    /// than fails) without a GPU adapter.
    #[test]
    fn pick_at_resolves_the_object_and_its_world_point() {
        let _gpu = gpu_guard();
        let Ok(mut renderer) = WgpuRenderer::new_default([64, 64]) else {
            eprintln!("no GPU adapter available; skipping pick test");
            return;
        };
        let camera = Camera::identity();

        // Two clip-space triangles at z = 0.5: id 1 on the left, id 2 on the right.
        let mut queue = RenderQueue::default();
        queue.packets.push(pick_packet(
            1,
            1,
            0,
            Topology::Triangles,
            vec![[-0.9, -0.9, 0.5], [-0.1, -0.9, 0.5], [-0.5, 0.9, 0.5]],
            vec![0, 1, 2],
        ));
        queue.packets.push(pick_packet(
            2,
            2,
            0,
            Topology::Triangles,
            vec![[0.1, -0.9, 0.5], [0.9, -0.9, 0.5], [0.5, 0.9, 0.5]],
            vec![0, 1, 2],
        ));
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .expect("draw");

        let left = renderer.pick_at(&queue, &camera, [16, 32]).expect("left hit");
        assert_eq!(left.id, PickId(1));
        assert_eq!(left.topology, Topology::Triangles);
        // Identity camera: clip == world, so the world point is the pixel-centre NDC at
        // the sampled depth (0.5).
        let expected: [f32; 3] = [2.0 * 16.5 / 64.0 - 1.0, 2.0 * 32.5 / 64.0 - 1.0, 0.5];
        for (got, want) in left.point.iter().zip(expected) {
            assert!((got - want).abs() < 0.02, "world point {got} !~ {want}");
        }
        // The triangle lies in the XY plane, so its normal is +Z.
        assert!(
            (left.normal[2] - 1.0).abs() < 0.01,
            "normal {:?} should be +Z",
            left.normal
        );
        assert!(left.distance > 0.0);

        let right = renderer.pick_at(&queue, &camera, [48, 32]).expect("right hit");
        assert_eq!(right.id, PickId(2));

        assert_eq!(renderer.pick_at(&queue, &camera, [2, 2]), None, "empty space");
        assert_eq!(
            renderer.pick_at(&queue, &camera, [-1, 0]),
            None,
            "outside the target"
        );
    }

    /// The G-buffer carries each vertex's topological sub-element index and the primitive
    /// topology, so a hit resolves to a `Face1`/`Edge2` (and the CPU and GPU paths agree on
    /// the index). `PickId(0)` — the compiler's default — is a *real* id, not "empty".
    /// Skips (rather than fails) without a GPU adapter.
    #[test]
    fn pick_at_reports_the_sub_element_index_and_topology() {
        let _gpu = gpu_guard();
        let Ok(mut renderer) = WgpuRenderer::new_default([64, 64]) else {
            eprintln!("no GPU adapter available; skipping pick detail test");
            return;
        };
        let camera = Camera::identity();

        // A triangle (id 0, part 5) on the left, and a wireframe line (id 4, part 7) across
        // the right at the row-32 pixel centre.
        let y = 2.0 * 32.5 / 64.0 - 1.0;
        let mut queue = RenderQueue::default();
        queue.packets.push(pick_packet(
            1,
            0,
            5,
            Topology::Triangles,
            vec![[-0.9, -0.9, 0.5], [-0.1, -0.9, 0.5], [-0.5, 0.9, 0.5]],
            vec![0, 1, 2],
        ));
        queue.packets.push(pick_packet(
            2,
            4,
            7,
            Topology::Lines,
            vec![[0.2, y, 0.5], [0.9, y, 0.5]],
            vec![0, 1],
        ));
        renderer
            .draw_frame(&[ViewSpec {
                queue: &queue,
                camera: &camera,
                rect: None,
                overlay: false,
            }])
            .expect("draw");

        let face = renderer.pick_at(&queue, &camera, [16, 32]).expect("triangle hit");
        assert_eq!(face.id, PickId(0), "PickId(0) is a valid hit, not empty space");
        assert_eq!(face.element, 5, "the face's sub-element index");
        assert_eq!(face.topology, Topology::Triangles);

        let edge = renderer.pick_at(&queue, &camera, [48, 32]).expect("line hit");
        assert_eq!(edge.id, PickId(4));
        assert_eq!(edge.element, 7, "the edge's sub-element index");
        assert_eq!(edge.topology, Topology::Lines);
    }
}
