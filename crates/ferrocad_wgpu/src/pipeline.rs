//! The render pipelines — the WGSL shaders and their vertex layouts.
//!
//! For now a single opaque triangle-list pipeline covers every
//! [`Topology`](ferrocad_inventor::Topology); line/point pipelines and the pick
//! pass slot in alongside it.

use std::mem::size_of;

use crate::batch::GpuVertex;

/// The depth format the scene pass uses.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The render pipelines the scene renderer draws with.
pub struct ScenePipelines {
    /// Opaque triangles (the common case).
    pub opaque: wgpu::RenderPipeline,
}

impl ScenePipelines {
    /// Build the pipelines for `color_format`, using `uniforms` as the only bind
    /// group (binding 0, dynamic offset). `sample_count` must match the render
    /// target's (multisampling for anti-aliasing).
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        uniforms: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ferrocad scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ferrocad scene"),
            bind_group_layouts: &[Some(uniforms)],
            immediate_size: 0,
        });

        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let opaque = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ferrocad opaque"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self { opaque }
    }
}
