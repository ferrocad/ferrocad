//! G-buffer picking: render pick ids offscreen, read them back per pixel.
//!
//! The scene is drawn once more with the pick pipelines, encoding each packet's
//! [`StateSnapshot::id`](ferrocad_graphics::StateSnapshot::id) and its per-vertex
//! topological sub-element index into offscreen targets. A click reads one texel to
//! learn which object (and which face/edge) is under the cursor — O(1), unlike the CPU
//! ray cast in [`ferrocad_graphics::pick`], which the host keeps as a fallback.
//!
//! Two colour targets are written:
//!
//! - [`ID_FORMAT`] `Rg32Uint` — `[entity_id, part_idx]`, the identity the caller needs.
//! - [`normal_format`](NORMAL_FORMAT) `Rgba16Float` — `[normal.xyz, topology]`. 16-bit
//!   float is renderable on Vulkan, GL and WebGPU; `Rgba32Float` is not everywhere
//!   (Intel's Mesa Vulkan driver rejects it), which silently disabled GPU picking.
//!   which primitive kind was hit.
//! - [`DEPTH_FORMAT`](crate::pipeline::DEPTH_FORMAT)-style depth as a **`R32Float` colour
//!   target**, so the host can copy a single texel: a depth *attachment* cannot be
//!   partially copied on every backend (the pick pass still binds a depth attachment for
//!   the depth test).
//!
//! The pass shares the scene's depth format and uniform layout, but is **single-sampled**
//! (the scene pass is multisampled; a pick resolves exactly one pixel), so it owns its
//! targets rather than reusing the scene's MSAA attachments.

use std::mem::size_of;

use ferrocad_graphics::Topology;

use crate::batch::GpuVertex;
use crate::pipeline::DEPTH_FORMAT;

/// The pick id/detail target format: `[entity_id, part_idx]`.
pub const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// The pick normal/topology target format: `[normal.xyz, topology]`.
///
/// 16-bit float rather than 32-bit: a 128-bit float colour attachment is optional and
/// Intel's Mesa Vulkan driver refuses it, which failed every pick pipeline at creation
/// and silently disabled GPU picking. Half floats are renderable everywhere wgpu runs
/// and are ample for a unit normal plus a topology ordinal.
pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The pick depth target format (a colour target the host can copy, not the attachment).
pub const DEPTH_VALUE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// The offscreen targets the pick pass renders into (all single-sampled).
pub struct PickTargets {
    /// The `[entity_id, part_idx]` colour target.
    pub id: wgpu::Texture,
    /// The view the pick pipeline writes ids into.
    pub id_view: wgpu::TextureView,
    /// The `[normal.xyz, topology]` colour target.
    pub normal: wgpu::Texture,
    /// The view the pick pipeline writes normals into.
    pub normal_view: wgpu::TextureView,
    /// The `R32Float` depth target (a colour target, so it can be copied a texel at a
    /// time).
    pub depth: wgpu::Texture,
    /// The view the pick pipeline writes depth into.
    pub depth_view: wgpu::TextureView,
    /// The depth *attachment*, bound only for the depth test (never read back).
    pub depth_buffer: wgpu::Texture,
    /// The view bound as the pick pass's depth attachment.
    pub depth_buffer_view: wgpu::TextureView,
}

impl PickTargets {
    /// Allocate the pick targets for `size` (each dimension at least one pixel).
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let extent = wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        };
        let make = |label, format, usage| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        };
        let color_usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let (id, id_view) = make("ferrocad pick ids", ID_FORMAT, color_usage);
        let (normal, normal_view) = make("ferrocad pick normals", NORMAL_FORMAT, color_usage);
        let (depth, depth_view) = make("ferrocad pick depth", DEPTH_VALUE_FORMAT, color_usage);
        let (depth_buffer, depth_buffer_view) = make(
            "ferrocad pick depth buffer",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        Self {
            id,
            id_view,
            normal,
            normal_view,
            depth,
            depth_view,
            depth_buffer,
            depth_buffer_view,
        }
    }
}

/// The pick pipelines: one per topology (triangle/line/point).
pub struct PickPipelines {
    triangles: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
}

impl PickPipelines {
    /// Build the pick pipelines, binding group 0 to the shared `uniforms` layout.
    pub fn new(device: &wgpu::Device, uniforms: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ferrocad pick"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pick.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ferrocad pick"),
            bind_group_layouts: &[Some(uniforms)],
            immediate_size: 0,
        });
        // The vertex buffer is the same interleaved layout the scene uses; the shader
        // reads locations 0/1/3 (position, normal, part index) and ignores the uv and
        // the per-vertex colour (declared so both passes share one layout).
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint32, 4 => Float32x4
        ];
        let vertex_buffers = [wgpu::VertexBufferLayout {
            array_stride: size_of::<GpuVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        }];
        let make = |label: &str, topology: wgpu::PrimitiveTopology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &vertex_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: ID_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: NORMAL_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: DEPTH_VALUE_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            triangles: make("ferrocad pick triangles", wgpu::PrimitiveTopology::TriangleList),
            lines: make("ferrocad pick lines", wgpu::PrimitiveTopology::LineList),
            points: make("ferrocad pick points", wgpu::PrimitiveTopology::PointList),
        }
    }

    /// The pipeline drawing `topology`.
    pub fn for_topology(&self, topology: Topology) -> &wgpu::RenderPipeline {
        match topology {
            Topology::Triangles => &self.triangles,
            Topology::Lines => &self.lines,
            Topology::Points => &self.points,
        }
    }
}

/// Decode a normal/topology texel in `format` into `[nx, ny, nz, topology]`.
pub fn read_normal(bytes: &[u8], format: wgpu::TextureFormat) -> Option<[f32; 4]> {
    match format {
        wgpu::TextureFormat::Rgba32Float => Some([
            read_f32(bytes, 0)?,
            read_f32(bytes, 4)?,
            read_f32(bytes, 8)?,
            read_f32(bytes, 12)?,
        ]),
        wgpu::TextureFormat::Rgba16Float => Some([
            f16_to_f32(read_u16(bytes, 0)?),
            f16_to_f32(read_u16(bytes, 2)?),
            f16_to_f32(read_u16(bytes, 4)?),
            f16_to_f32(read_u16(bytes, 6)?),
        ]),
        wgpu::TextureFormat::Rgba8Snorm => Some([
            snorm8(*bytes.get(0)?),
            snorm8(*bytes.get(1)?),
            snorm8(*bytes.get(2)?),
            snorm8(*bytes.get(3)?),
        ]),
        _ => None,
    }
}

fn read_f32(bytes: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn snorm8(byte: u8) -> f32 {
    (byte as i8) as f32 / 127.0
}

/// IEEE 754 binary16 to f32 (no external crate).
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x3ff;
    let magnitude = match exponent {
        0 => mantissa as f32 * 2f32.powi(-24),
        0x1f if mantissa == 0 => f32::INFINITY,
        0x1f => f32::NAN,
        _ => (1.0 + mantissa as f32 / 1024.0) * 2f32.powi(exponent as i32 - 15),
    };
    sign * magnitude
}

/// Copy a single texel of `texture` back to the CPU as its raw bytes.
///
/// `bytes_per_texel` is the format's texel size (4 or 8). wgpu requires
/// `bytes_per_row` to be a multiple of [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] even for a
/// 1×1 copy, so the staging buffer strided at 256 bytes and only the first texel is read.
pub fn read_texel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
    bytes_per_texel: u32,
    origin: wgpu::Origin3d,
) -> Result<Vec<u8>, &'static str> {
    debug_assert!(bytes_per_texel <= wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ferrocad pick readback"),
        size: u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ferrocad pick readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|_| "device poll failed")?;

    let view = slice.get_mapped_range();
    let bytes = view[..bytes_per_texel as usize].to_vec();
    drop(view);
    buffer.unmap();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_float_decodes() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert!((f16_to_f32(0x3555) - 0.33325).abs() < 1e-4);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    }

    #[test]
    fn the_normal_texel_decodes_in_each_format() {
        let half: Vec<u8> = [0x3c00u16, 0x0000, 0xbc00, 0x0000]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            read_normal(&half, wgpu::TextureFormat::Rgba16Float),
            Some([1.0, 0.0, -1.0, 0.0])
        );

        let full: Vec<u8> = [0.5f32, -0.5, 1.0, 2.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert_eq!(
            read_normal(&full, wgpu::TextureFormat::Rgba32Float),
            Some([0.5, -0.5, 1.0, 2.0])
        );

        let normals = read_normal(&[127, 0, 0x81, 0x7f], wgpu::TextureFormat::Rgba8Snorm)
            .expect("snorm decodes");
        assert!((normals[0] - 1.0).abs() < 1e-6);
        assert_eq!(normals[1], 0.0);
        assert!((normals[2] + 1.0).abs() < 1e-6);
    }
}
