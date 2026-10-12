//! Uploading fresh geometry into GPU meshes.
//!
//! A [`Geometry`] arrives on the CPU (only on a cache miss — see
//! [`DrawBatch::geometry`](ferrocad_graphics::DrawBatch::geometry)); [`GpuMesh`] is
//! its uploaded form, cached by `(mesh_id, generation)`.

use bytemuck::{Pod, Zeroable};
use ferrocad_graphics::Geometry;
use wgpu::util::DeviceExt;

/// A vertex as laid out in a GPU buffer.
///
/// Mirrors [`ferrocad_graphics::Vertex`] and derives `bytemuck::Pod`, so a slice of
/// these can be uploaded directly. `#[repr(C)]` with two `vec3`s has no padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuVertex {
    /// Model-space position.
    pub position: [f32; 3],
    /// Model-space normal.
    pub normal: [f32; 3],
    /// Texture coordinates.
    pub uv: [f32; 2],
}

impl From<&ferrocad_graphics::Vertex> for GpuVertex {
    fn from(v: &ferrocad_graphics::Vertex) -> Self {
        Self {
            position: v.position,
            normal: v.normal,
            uv: v.uv,
        }
    }
}

/// Placeholder bytes for an empty mesh's dummy buffer.
const DUMMY_BYTES: &[u8] = &[0u8; 16];

/// Geometry resident on the GPU for one `(mesh_id, generation)`.
pub struct GpuMesh {
    /// The uploaded vertex buffer.
    pub vertex_buffer: wgpu::Buffer,
    /// The uploaded index buffer.
    pub index_buffer: wgpu::Buffer,
    /// Number of indices to draw.
    pub index_count: u32,
}

impl GpuMesh {
    /// Upload `geometry` into GPU buffers.
    pub fn upload(device: &wgpu::Device, geometry: &Geometry) -> Self {
        let vertices: Vec<GpuVertex> = geometry.vertices.iter().map(GpuVertex::from).collect();
        let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(&geometry.indices);

        // `create_buffer_init` rejects empty contents, so fall back to a tiny dummy
        // buffer when there is nothing to upload.
        let vertex_buffer = upload_bytes(
            device,
            "ferrocad vertices",
            vertex_bytes,
            wgpu::BufferUsages::VERTEX,
        );
        let index_buffer = upload_bytes(
            device,
            "ferrocad indices",
            index_bytes,
            wgpu::BufferUsages::INDEX,
        );

        Self {
            vertex_buffer,
            index_buffer,
            index_count: geometry.indices.len() as u32,
        }
    }
}

fn upload_bytes(
    device: &wgpu::Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let usage = usage | wgpu::BufferUsages::COPY_DST;
    if bytes.is_empty() {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: DUMMY_BYTES.len() as u64,
            usage,
            mapped_at_creation: false,
        })
    } else {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
    }
}
