//! The render pipelines — the WGSL shaders and their vertex layouts.
//!
//! One shader serves every primitive; the pipelines differ by **topology**
//! (triangle/line/point) and by **depth behaviour**: the scene pipelines write depth
//! and compare `Less`, while the **overlay** pipelines (the navigation cube) write no
//! depth, compare `Always`, and alpha-blend, so they draw on top of the scene.

use std::mem::size_of;

use ferrocad_graphics::Topology;

use crate::batch::GpuVertex;

/// The depth format the scene pass uses.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The render pipelines the scene renderer draws with: a scene and an overlay variant
/// for each topology.
pub struct ScenePipelines {
    /// Opaque triangles (the common case).
    pub opaque: wgpu::RenderPipeline,
    /// Overlay triangles (the navigation-cube faces).
    pub overlay: wgpu::RenderPipeline,
    /// Scene lines (`SoIndexedLineSet`, wireframes).
    pub lines: wgpu::RenderPipeline,
    /// Overlay lines (the navigation-cube edges).
    pub overlay_lines: wgpu::RenderPipeline,
    /// Scene points.
    pub points: wgpu::RenderPipeline,
    /// Overlay points.
    pub overlay_points: wgpu::RenderPipeline,
}

impl ScenePipelines {
    /// Build the pipelines for `color_format`, using `uniforms` (group 0) and
    /// `textures` (group 1) as bind groups. `sample_count` must match the render
    /// target's (multisampling for anti-aliasing).
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        uniforms: &wgpu::BindGroupLayout,
        textures: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ferrocad scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ferrocad scene"),
            bind_group_layouts: &[Some(uniforms), Some(textures)],
            immediate_size: 0,
        });

        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let vertex_buffers = [wgpu::VertexBufferLayout {
            array_stride: size_of::<GpuVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        }];

        let make = |label: &str,
                    topology: wgpu::PrimitiveTopology,
                    overlay: bool,
                    blend: wgpu::BlendState| {
            let (depth_write_enabled, depth_compare) = if overlay {
                (false, wgpu::CompareFunction::Always)
            } else {
                (true, wgpu::CompareFunction::Less)
            };
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
                    depth_write_enabled: Some(depth_write_enabled),
                    depth_compare: Some(depth_compare),
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
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let scene = wgpu::BlendState::REPLACE;
        Self {
            opaque: make(
                "ferrocad opaque",
                wgpu::PrimitiveTopology::TriangleList,
                false,
                scene,
            ),
            overlay: make(
                "ferrocad overlay",
                wgpu::PrimitiveTopology::TriangleList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
            ),
            lines: make(
                "ferrocad lines",
                wgpu::PrimitiveTopology::LineList,
                false,
                scene,
            ),
            overlay_lines: make(
                "ferrocad overlay lines",
                wgpu::PrimitiveTopology::LineList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
            ),
            points: make(
                "ferrocad points",
                wgpu::PrimitiveTopology::PointList,
                false,
                scene,
            ),
            overlay_points: make(
                "ferrocad overlay points",
                wgpu::PrimitiveTopology::PointList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
            ),
        }
    }

    /// The pipeline for drawing `topology` in a scene or overlay view.
    pub fn for_draw(&self, overlay: bool, topology: Topology) -> &wgpu::RenderPipeline {
        match (overlay, topology) {
            (false, Topology::Triangles) => &self.opaque,
            (true, Topology::Triangles) => &self.overlay,
            (false, Topology::Lines) => &self.lines,
            (true, Topology::Lines) => &self.overlay_lines,
            (false, Topology::Points) => &self.points,
            (true, Topology::Points) => &self.overlay_points,
        }
    }
}
