//! The render pipelines — the WGSL shaders and their vertex layouts.
//!
//! One shader serves every primitive; the pipelines differ by **topology**
//! (triangle/line/point) and by **depth behaviour**: the scene pipelines write depth
//! and compare `Less`, while the **overlay** pipelines (the navigation cube) write no
//! depth, compare `Always`, and alpha-blend, so they draw on top of the scene.
//!
//! The **surface** (triangle) pipeline additionally carries a slope/constant depth bias,
//! pushing shaded faces slightly away from the camera so a coplanar edge wireframe (CAD's
//! shaded-with-edges) renders cleanly on top instead of z-fighting. The line and point
//! pipelines keep a zero bias.
//!
//! A packet whose [`DepthState`] differs from the default (`SoDepthBuffer`) selects a
//! pipeline variant built on demand and cached on [`PipelineKey`]; the six base
//! pipelines above stay the fast path for the default state.
//!
//! The pipeline layout is **groups 0, 1 and 2**: the dynamic globals, the texture, and
//! the shader-program ABI ([`crate::program`]). The scene shader declares no binding in
//! group 2, but the render pass binds it for every draw so the index never shifts.

use std::collections::HashMap;
use std::mem::size_of;

use ferrocad_graphics::{DepthCompare, DepthState, Topology};

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
    /// The full-screen vertical canvas gradient (drawn before the scene views).
    pub background: wgpu::RenderPipeline,
    /// The shader module and layout shared by the base pipelines and every variant.
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
    /// Pipelines for a non-default depth state, built on first use.
    variants: HashMap<PipelineKey, wgpu::RenderPipeline>,
}

/// What makes a depth-state pipeline variant distinct.
///
/// The six base pipelines cover [`DepthState::default()`] (and the overlay's
/// no-write/`Always` pair); a packet that asks for anything else gets a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    /// Whether the draw is an overlay (depth off, blending) or scene geometry.
    pub overlay: bool,
    /// The primitive topology.
    pub topology: wgpu::PrimitiveTopology,
    /// The depth comparison (after `test == false` collapses to `Always`).
    pub depth_compare: wgpu::CompareFunction,
    /// Whether a passing fragment writes depth.
    pub depth_write: bool,
}

impl PipelineKey {
    /// The key for `depth` drawn as `topology` in a scene or overlay view.
    pub fn new(overlay: bool, topology: Topology, depth: &DepthState) -> Self {
        Self {
            overlay,
            topology: wgpu_topology(topology),
            depth_compare: compare_function(depth.effective_compare()),
            depth_write: depth.effective_write(),
        }
    }

    /// Whether the six base pipelines already draw this state (the default: compare
    /// `Less` and write). The overlay *view* additionally forces its own pair of
    /// pipelines, which is unchanged pre-existing behaviour.
    pub fn is_base(&self) -> bool {
        self.depth_compare == wgpu::CompareFunction::Less && self.depth_write
    }
}

/// The wgpu topology for a neutral one.
fn wgpu_topology(topology: Topology) -> wgpu::PrimitiveTopology {
    match topology {
        Topology::Triangles => wgpu::PrimitiveTopology::TriangleList,
        Topology::Lines => wgpu::PrimitiveTopology::LineList,
        Topology::Points => wgpu::PrimitiveTopology::PointList,
    }
}

/// Map a neutral depth comparison onto wgpu's.
fn compare_function(compare: DepthCompare) -> wgpu::CompareFunction {
    match compare {
        DepthCompare::Never => wgpu::CompareFunction::Never,
        DepthCompare::Less => wgpu::CompareFunction::Less,
        DepthCompare::Equal => wgpu::CompareFunction::Equal,
        DepthCompare::LessEqual => wgpu::CompareFunction::LessEqual,
        DepthCompare::Greater => wgpu::CompareFunction::Greater,
        DepthCompare::NotEqual => wgpu::CompareFunction::NotEqual,
        DepthCompare::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        DepthCompare::Always => wgpu::CompareFunction::Always,
    }
}

impl ScenePipelines {
    /// Build the pipelines for `color_format`, using `uniforms` (group 0), `textures`
    /// (group 1) and the program ABI's `program` (group 2) as bind groups.
    /// `sample_count` must match the render target's (multisampling for anti-aliasing).
    ///
    /// Group 2 is part of the shared layout even though the scene shader declares no
    /// binding there: a program pipeline built against the same layout can therefore
    /// replace these pipelines without the render pass rebinding groups 0/1 or shifting
    /// the group indices.
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        uniforms: &wgpu::BindGroupLayout,
        textures: &wgpu::BindGroupLayout,
        program: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ferrocad scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ferrocad scene"),
            bind_group_layouts: &[Some(uniforms), Some(textures), Some(program)],
            immediate_size: 0,
        });

        // Push shaded surfaces slightly away from the camera, so a coplanar edge
        // wireframe wins the depth test (CAD's shaded-with-edges, without z-fighting).
        let surface_bias = wgpu::DepthBiasState {
            constant: 1,
            slope_scale: 1.0,
            clamp: 0.0,
        };
        let make = |label: &str,
                    topology: wgpu::PrimitiveTopology,
                    overlay: bool,
                    blend: wgpu::BlendState,
                    bias: wgpu::DepthBiasState| {
            let (depth_write_enabled, depth_compare) = if overlay {
                (false, wgpu::CompareFunction::Always)
            } else {
                (true, wgpu::CompareFunction::Less)
            };
            create_pipeline(
                device,
                &layout,
                &shader,
                label,
                topology,
                blend,
                bias,
                depth_write_enabled,
                depth_compare,
                sample_count,
                color_format,
            )
        };

        let scene = wgpu::BlendState::REPLACE;
        let none = wgpu::DepthBiasState::default();

        // The canvas gradient is a full-screen triangle with no vertex buffer or bind
        // groups; it writes colour only (depth test `Always`, no write).
        let background_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ferrocad background"),
            source: wgpu::ShaderSource::Wgsl(include_str!("background.wgsl").into()),
        });
        let background_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ferrocad background"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });
        let background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ferrocad background"),
            layout: Some(&background_layout),
            vertex: wgpu::VertexState {
                module: &background_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &background_shader,
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

        Self {
            opaque: make(
                "ferrocad opaque",
                wgpu::PrimitiveTopology::TriangleList,
                false,
                scene,
                surface_bias,
            ),
            overlay: make(
                "ferrocad overlay",
                wgpu::PrimitiveTopology::TriangleList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
                none,
            ),
            lines: make(
                "ferrocad lines",
                wgpu::PrimitiveTopology::LineList,
                false,
                // Alpha blending (with depth still tested/written) so a producer can
                // fade a line per vertex, as the ground grid's falloff does.
                wgpu::BlendState::ALPHA_BLENDING,
                none,
            ),
            overlay_lines: make(
                "ferrocad overlay lines",
                wgpu::PrimitiveTopology::LineList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
                none,
            ),
            points: make(
                "ferrocad points",
                wgpu::PrimitiveTopology::PointList,
                false,
                // See `lines`: per-vertex alpha must blend, not replace.
                wgpu::BlendState::ALPHA_BLENDING,
                none,
            ),
            overlay_points: make(
                "ferrocad overlay points",
                wgpu::PrimitiveTopology::PointList,
                true,
                wgpu::BlendState::ALPHA_BLENDING,
                none,
            ),
            background,
            shader,
            layout,
            color_format,
            sample_count,
            variants: HashMap::new(),
        }
    }

    /// The full-screen canvas-gradient pipeline.
    pub fn background(&self) -> &wgpu::RenderPipeline {
        &self.background
    }

    /// The pipeline for drawing `topology` in a scene or overlay view with the default
    /// depth state.
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

    /// The pipeline for `topology` with `depth` bound.
    ///
    /// The default state returns one of the six base pipelines; any other state gets a
    /// variant built on first use and cached on [`PipelineKey`].
    pub fn for_state(
        &mut self,
        device: &wgpu::Device,
        overlay: bool,
        topology: Topology,
        depth: &DepthState,
    ) -> &wgpu::RenderPipeline {
        let key = PipelineKey::new(overlay, topology, depth);
        if key.is_base() {
            return self.for_draw(overlay, topology);
        }
        if !self.variants.contains_key(&key) {
            let pipeline = create_variant(
                device,
                &self.layout,
                &self.shader,
                key,
                self.sample_count,
                self.color_format,
            );
            self.variants.insert(key, pipeline);
        }
        &self.variants[&key]
    }

    /// The number of depth-state variants built so far (tests and diagnostics).
    pub fn variant_count(&self) -> usize {
        self.variants.len()
    }
}

/// Build one pipeline from the shared scene shader.
#[allow(clippy::too_many_arguments)]
fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    topology: wgpu::PrimitiveTopology,
    blend: wgpu::BlendState,
    bias: wgpu::DepthBiasState,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    sample_count: u32,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint32, 4 => Float32x4
    ];
    let vertex_buffers = [wgpu::VertexBufferLayout {
        array_stride: size_of::<GpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    }];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
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
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil: wgpu::StencilState::default(),
            bias,
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
}

/// Build the variant for `key`, matching its base pipeline's blend and bias.
fn create_variant(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    key: PipelineKey,
    sample_count: u32,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let (blend, bias) = match (key.overlay, key.topology) {
        (false, wgpu::PrimitiveTopology::TriangleList) => (
            // A non-default scene state is how the selection tint blends: the 12%
            // highlight surface asks for test-on/write-off, which is not the base
            // opaque pipeline. Opaque scene draws keep `REPLACE` through `opaque`.
            wgpu::BlendState::ALPHA_BLENDING,
            // The surface bias belongs to the shaded-surface pipeline; keep it so a
            // depth-state change does not revive z-fighting with a coplanar wireframe.
            wgpu::DepthBiasState {
                constant: 1,
                slope_scale: 1.0,
                clamp: 0.0,
            },
        ),
        (true, wgpu::PrimitiveTopology::TriangleList) => {
            (wgpu::BlendState::ALPHA_BLENDING, wgpu::DepthBiasState::default())
        }
        _ => (wgpu::BlendState::ALPHA_BLENDING, wgpu::DepthBiasState::default()),
    };
    create_pipeline(
        device,
        layout,
        shader,
        "ferrocad depth variant",
        key.topology,
        blend,
        bias,
        key.depth_write,
        key.depth_compare,
        sample_count,
        color_format,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_state_selects_the_base_pipelines() {
        let default = DepthState::default();
        for overlay in [false, true] {
            for topology in [Topology::Triangles, Topology::Lines, Topology::Points] {
                let key = PipelineKey::new(overlay, topology, &default);
                assert!(key.is_base(), "{overlay:?} {topology:?} is a base pipeline");
                assert_eq!(key.depth_compare, wgpu::CompareFunction::Less);
                assert!(key.depth_write);
            }
        }
        // The overlay's default *pipeline* state (no write, Always) is not the default
        // `DepthState`; it is the overlay's own fixed pair, not a variant of a packet.
        let overlay_default = PipelineKey::new(true, Topology::Triangles, &default);
        assert_ne!(
            overlay_default.depth_compare,
            wgpu::CompareFunction::Always,
            "the packet state, not the overlay flag, decides the compare"
        );
    }

    /// The pure key mapping `SoDepthBuffer` needs: `test FALSE` → `Always`, `write FALSE`
    /// → no depth write, and every compare mode round-trips.
    #[test]
    fn the_pipeline_key_follows_the_depth_state() {
        let no_test = PipelineKey::new(
            false,
            Topology::Triangles,
            &DepthState {
                test: false,
                ..DepthState::default()
            },
        );
        assert_eq!(no_test.depth_compare, wgpu::CompareFunction::Always);
        assert!(no_test.depth_write, "the test and the write are independent");
        assert!(!no_test.is_base());

        let no_write = PipelineKey::new(
            false,
            Topology::Triangles,
            &DepthState {
                write: false,
                ..DepthState::default()
            },
        );
        assert!(!no_write.depth_write);
        assert_eq!(no_write.depth_compare, wgpu::CompareFunction::Less);
        assert!(!no_write.is_base());

        let cases = [
            (DepthCompare::Never, wgpu::CompareFunction::Never),
            (DepthCompare::Less, wgpu::CompareFunction::Less),
            (DepthCompare::Equal, wgpu::CompareFunction::Equal),
            (DepthCompare::LessEqual, wgpu::CompareFunction::LessEqual),
            (DepthCompare::Greater, wgpu::CompareFunction::Greater),
            (DepthCompare::NotEqual, wgpu::CompareFunction::NotEqual),
            (DepthCompare::GreaterEqual, wgpu::CompareFunction::GreaterEqual),
            (DepthCompare::Always, wgpu::CompareFunction::Always),
        ];
        for (neutral, expected) in cases {
            let key = PipelineKey::new(
                false,
                Topology::Lines,
                &DepthState {
                    compare: neutral,
                    ..DepthState::default()
                },
            );
            assert_eq!(key.depth_compare, expected, "{neutral:?}");
        }
    }

    #[test]
    fn the_program_default_state_is_a_variant() {
        let key = PipelineKey::new(false, Topology::Triangles, &DepthState::PROGRAM);
        assert!(!key.is_base());
        assert!(!key.depth_write);
        assert_eq!(key.depth_compare, wgpu::CompareFunction::Less);
    }

    /// The selection tint asks for test-on/write-off; that non-base state is what
    /// selects the blended triangle variant instead of the opaque scene pipeline.
    #[test]
    fn a_no_write_scene_triangle_gets_a_non_base_variant() {
        let key = PipelineKey::new(
            false,
            Topology::Triangles,
            &DepthState {
                write: false,
                ..DepthState::default()
            },
        );
        assert!(!key.is_base());
        assert!(key.depth_compare == wgpu::CompareFunction::Less);
        assert!(!key.depth_write);
    }
}
