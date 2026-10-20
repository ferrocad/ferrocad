//! Dynamic uniforms: per-frame globals and per-packet data in one buffer.
//!
//! Each draw's data (view-projection, model, colour) is packed with a fixed stride
//! into a single uniform buffer and selected with a dynamic offset, so one bind
//! group layout serves every draw without a bind group per packet. The stride is
//! padded to the adapter's `min_uniform_buffer_offset_alignment`.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};

/// The per-draw uniform block. Matrices are **column-major** (as WGSL expects).
///
/// The last four words are what the G-buffer pick shader reads; the scene shader
/// declares the same struct but ignores them, so one layout serves both passes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct UniformData {
    /// The combined world → clip matrix (`projection * view`).
    pub view_proj: [[f32; 4]; 4],
    /// The draw's world transform.
    pub model: [[f32; 4]; 4],
    /// The draw's linear RGBA colour.
    pub color: [f32; 4],
    /// The pick id written to the G-buffer's red channel (`0` = not pickable).
    pub pick: u32,
    /// The topology ordinal written to the G-buffer's normal target's `w` channel.
    pub topology: u32,
    /// A clip-space depth offset (positive = towards the camera) for coplanar overlays
    /// such as a selection highlight.
    pub depth_bias: f32,
    /// Keeps the block a multiple of 16 bytes.
    pub _padding: u32,
}

/// The size of one [`UniformData`] in bytes.
pub const UNIFORM_SIZE: u64 = size_of::<UniformData>() as u64;

/// The bind group layout for the uniform buffer (binding 0, dynamic offset).
pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ferrocad uniforms"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
            },
            count: None,
        }],
    })
}

/// A uniform buffer holding `capacity` [`UniformData`] slots, plus its bind group.
pub struct DynamicUniforms {
    /// The uniform buffer (one padded slot per draw).
    pub buffer: wgpu::Buffer,
    /// The bind group binding [`buffer`](Self::buffer).
    pub bind_group: wgpu::BindGroup,
    /// Bytes between consecutive slots (aligned for dynamic offsets).
    pub stride: u32,
    /// Number of slots the buffer holds.
    pub capacity: u32,
}

impl DynamicUniforms {
    /// Allocate `capacity` slots, aligned to `min_alignment`.
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        min_alignment: u32,
        capacity: u32,
    ) -> Self {
        let stride = align_up(UNIFORM_SIZE as u32, min_alignment.max(1));
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad uniforms"),
            size: u64::from(stride) * u64::from(capacity.max(1)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ferrocad uniforms"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(UNIFORM_SIZE),
                }),
            }],
        });
        Self {
            buffer,
            bind_group,
            stride,
            capacity: capacity.max(1),
        }
    }

    /// The dynamic offset, in bytes, of slot `index`.
    pub fn offset(&self, index: u32) -> u32 {
        index * self.stride
    }

    /// Write slot `index`.
    pub fn write(&self, queue: &wgpu::Queue, index: u32, data: &UniformData) {
        queue.write_buffer(&self.buffer, u64::from(self.offset(index)), bytemuck::bytes_of(data));
    }
}

fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
