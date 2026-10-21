//! Drawing an authored shader program: group 2, the pipeline cache and the placeholder.
//!
//! A [`DrawPacket`](ferrocad_graphics::DrawBatch) whose state carries a
//! [`ShaderProgram`] is drawn with **that** program instead of the scene pipeline. The
//! authored module is built with `create_shader_module` from the program's inline WGSL
//! and the pipeline is cached on a [`ProgramKey`]:
//!
//! ```text
//! (program fingerprint, topology, depth compare, depth write, blend,
//!  sample count, colour format)
//! ```
//!
//! The program does not draw the packet's authored geometry: it draws a **4-vertex
//! placeholder** (a `TriangleList`, six indices) whose only meaning is the sign of each
//! corner, which is what the fixed ABI's module derives screen corners from. Culling is
//! off, because a caller's winding is not part of the ABI.
//!
//! ## Group 2 — the fixed ABI
//!
//! The **shared** scene pipeline layout is groups 0 (`Globals`), 1 (texture) and 2
//! (this module). The program shader is only required to declare group 2:
//!
//! | binding | block | size | contents |
//! | --- | --- | ---: | --- |
//! | 0 | `Frame` | 160 B | `view_proj`, `inv_view_proj`, world-space `eye`, `flags = [perspective, width, height, 0]` |
//! | 1 | `Program` | 1328 B | `grid_color.rgb` (+ spare), `values = [base_spacing, minor_alpha, major_alpha, shadow_opacity]`, `counts = [shadow_count, 0, 0, 0]`, 80 zero `vec4`s |
//!
//! Binding 0 is a dynamic-offset uniform (one slot per view, so several cameras share
//! one buffer); binding 1 is a per-program buffer keyed on the fingerprint. Every scene
//! draw binds group 2 — ordinary draws bind [`ProgramPipeline::default_group`], a
//! zeroed `Frame`/`Program` pair — so the bind-group index never shifts between a
//! program pipeline and the scene pipeline.
//!
//! Shadows are deferred (`docs/grid-overlay-plan.md` §3.5): the `Program` block carries
//! the zeroed 80-`vec4` shadow array and `counts.x` is 0, so `grid_overlay.wgsl`'s
//! loop is compiled but never entered.

use std::collections::HashMap;
use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use ferrocad_graphics::{
    Camera, DepthCompare, DepthState, ProgramParameters, ShaderProgram, Topology,
};
use ferrocad_types::{Matrix4, Vector3};

use crate::pipeline::DEPTH_FORMAT;

/// The `Frame` block's size (the WGSL layout: two matrices, then two `vec4`s).
pub const FRAME_SIZE: u64 = 160;
/// The `Program` block's size (three `vec4`s plus 80 shadow `vec4`s).
pub const PROGRAM_SIZE: u64 = 1_328;

/// The per-view `Frame` uniform (group 2, binding 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Frame {
    /// The world → clip matrix, column-major.
    pub view_proj: [[f32; 4]; 4],
    /// Its inverse, column-major.
    pub inv_view_proj: [[f32; 4]; 4],
    /// The world-space eye (`w` is 1).
    pub eye: [f32; 4],
    /// `[perspective? 1 : 0, target width, target height, 0]`.
    pub flags: [f32; 4],
}

impl Frame {
    /// Build the frame block for `camera` rendering into a `size` target.
    ///
    /// An explicit [`Projection::Matrix`](ferrocad_graphics::Projection) is treated as
    /// perspective unless it is orthographic: the shader uses the flag to choose between
    /// the eye and the near point as the ray origin, and a host matrix is far more likely
    /// to be a real projection than a parallel one. A singular view-projection gets an
    /// identity inverse, which makes the shader's rays degenerate rather than NaN.
    pub fn from_camera(camera: &Camera, size: [u32; 2]) -> Self {
        let view_proj = camera.view_projection();
        let inverse = view_proj.inverse().unwrap_or_else(Matrix4::identity);
        let eye = camera
            .view
            .inverse()
            .map(|view| view.transform(&Vector3::zero()))
            .unwrap_or_else(Vector3::zero);
        Self {
            view_proj: mat4_to_wgsl(&view_proj),
            inv_view_proj: mat4_to_wgsl(&inverse),
            eye: [eye.x as f32, eye.y as f32, eye.z as f32, 1.0],
            flags: [
                if camera.is_orthographic() { 0.0 } else { 1.0 },
                size[0] as f32,
                size[1] as f32,
                0.0,
            ],
        }
    }
}

/// The per-program `Program` uniform (group 2, binding 1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ProgramBlock {
    /// `grid_color.rgb` (linear) plus room for the spare component.
    pub grid_color: [f32; 4],
    /// `[base_spacing, minor_alpha, major_alpha, shadow_opacity]`.
    pub values: [f32; 4],
    /// `[shadow_count, 0, 0, 0]`.
    pub counts: [f32; 4],
    /// 80 `vec4`s: 64 shadow slots of 5 floats re-packed on 16-byte boundaries.
    pub shadows: [[f32; 4]; 80],
}

impl ProgramBlock {
    /// Pack the fixed-ABI parameters, with the deferred shadow block zeroed.
    pub fn new(parameters: &ProgramParameters) -> Self {
        let [r, g, b] = parameters.color;
        Self {
            grid_color: [r, g, b, 0.0],
            values: [
                parameters.base_spacing,
                parameters.minor_alpha,
                parameters.major_alpha,
                parameters.shadow_opacity,
            ],
            counts: [parameters.shadow_count, 0.0, 0.0, 0.0],
            shadows: [[0.0; 4]; 80],
        }
    }
}

impl Default for ProgramBlock {
    fn default() -> Self {
        Self::new(&ProgramParameters::default())
    }
}

/// The bind-group layout of group 2 (the fixed ABI).
pub fn group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding: u32, size: u64, dynamic: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ferrocad program"),
        entries: &[entry(0, FRAME_SIZE, true), entry(1, PROGRAM_SIZE, false)],
    })
}

/// What makes a program pipeline distinct.
///
/// Two draws share a pipeline exactly when this key matches. The depth fields come from
/// the packet's [`DepthState`] (`SoDepthBuffer`), so one program can be drawn with
/// different depth behaviour without invalidating a cache entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramKey {
    /// The program's content fingerprint ([`ShaderProgram::hash`]).
    pub program: u64,
    /// The primitive topology (always `TriangleList` today).
    pub topology: wgpu::PrimitiveTopology,
    /// The depth comparison (after `test == false` collapses to `Always`).
    pub depth_compare: wgpu::CompareFunction,
    /// Whether a passing fragment writes depth.
    pub depth_write: bool,
    /// The colour blend state.
    pub blend: wgpu::BlendState,
    /// The render target's sample count.
    pub sample_count: u32,
    /// The render target's colour format.
    pub color_format: wgpu::TextureFormat,
}

impl ProgramKey {
    /// The key for `program` drawn with `depth` and `topology` into a target of
    /// `color_format` at `sample_count` samples.
    ///
    /// A program always alpha-blends: the fixed ABI's fragment output carries coverage
    /// (`grid_overlay.wgsl` returns a possibly-transparent colour), so replacing the
    /// target would paint black outside the lines.
    pub fn new(
        program: &ShaderProgram,
        depth: &DepthState,
        topology: wgpu::PrimitiveTopology,
        sample_count: u32,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            program: program.hash(),
            topology,
            depth_compare: compare_function(depth.effective_compare()),
            depth_write: depth.effective_write(),
            blend: wgpu::BlendState::ALPHA_BLENDING,
            sample_count,
            color_format,
        }
    }
}

/// Map a neutral depth comparison onto wgpu's.
pub fn compare_function(compare: DepthCompare) -> wgpu::CompareFunction {
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

/// Convert a row-major [`Matrix4`] to the column-major layout WGSL uniforms use.
pub fn mat4_to_wgsl(m: &Matrix4) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for row in 0..4 {
        for col in 0..4 {
            out[col][row] = m.m[row * 4 + col] as f32;
        }
    }
    out
}

/// The placeholder quad's corners. Only the sign of `x`/`y` is part of the ABI.
const PLACEHOLDER: [[f32; 3]; 4] = [
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
];

/// The placeholder's two triangles, as a `TriangleList`.
const PLACEHOLDER_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// The group-2 resources plus the program pipeline cache.
pub struct ProgramPipeline {
    /// Group 2 (the fixed ABI).
    group_layout: wgpu::BindGroupLayout,
    /// The shared scene layout: groups 0, 1 and 2.
    layout: wgpu::PipelineLayout,
    /// The colour format every pipeline targets.
    color_format: wgpu::TextureFormat,
    /// The scene's sample count.
    sample_count: u32,
    pipelines: HashMap<ProgramKey, wgpu::RenderPipeline>,
    /// One `Frame` slot per view, selected by a dynamic offset.
    frames: wgpu::Buffer,
    frame_stride: u32,
    frame_capacity: u32,
    /// The zeroed group 2 bound by every ordinary scene draw.
    default_group: wgpu::BindGroup,
    /// A program buffer and its group, keyed on the program fingerprint. Invalidated
    /// when the frame buffer is reallocated (the group holds the old buffer).
    groups: HashMap<u64, wgpu::BindGroup>,
    program_buffers: HashMap<u64, wgpu::Buffer>,
    /// The uniform buffers the default group binds.
    _zero_frame: wgpu::Buffer,
    _zero_program: wgpu::Buffer,
    placeholder_vertices: wgpu::Buffer,
    placeholder_indices: wgpu::Buffer,
}

impl ProgramPipeline {
    /// Allocate group 2 and an initial frame capacity of eight views.
    pub fn new(
        device: &wgpu::Device,
        uniforms: &wgpu::BindGroupLayout,
        textures: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let group_layout = group_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ferrocad scene"),
            bind_group_layouts: &[Some(uniforms), Some(textures), Some(&group_layout)],
            immediate_size: 0,
        });

        let zero_frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad frame (zero)"),
            size: FRAME_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let zero_program = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad program (zero)"),
            size: PROGRAM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let default_group =
            bind_group(device, &group_layout, &zero_frame, &zero_program, "ferrocad default");

        let placeholder_vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad program placeholder"),
            size: (size_of::<[f32; 3]>() * PLACEHOLDER.len()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let placeholder_indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrocad program placeholder indices"),
            size: (size_of::<u32>() * PLACEHOLDER_INDICES.len()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment.max(1);
        let frame_stride = align_up(FRAME_SIZE as u32, alignment);
        let frame_capacity = 8;
        let frames = frame_buffer(device, u64::from(frame_stride), frame_capacity);

        Self {
            group_layout,
            layout,
            color_format,
            sample_count,
            pipelines: HashMap::new(),
            frames,
            frame_stride,
            frame_capacity,
            default_group,
            groups: HashMap::new(),
            program_buffers: HashMap::new(),
            _zero_frame: zero_frame,
            _zero_program: zero_program,
            placeholder_vertices,
            placeholder_indices,
        }
    }

    /// The group-2 bind-group layout (a renderer's scene pipelines include it).
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.group_layout
    }

    /// The zeroed group 2 bound by every non-program draw.
    pub fn default_group(&self) -> &wgpu::BindGroup {
        &self.default_group
    }

    /// The dynamic offset `default_group` must be bound with (`0`).
    pub fn default_offset(&self) -> u32 {
        0
    }

    /// Make room for `count` frame slots, growing (and invalidating the program groups)
    /// when needed.
    pub fn ensure_frame_capacity(&mut self, device: &wgpu::Device, count: u32) {
        let count = count.max(1);
        if count <= self.frame_capacity {
            return;
        }
        self.frame_capacity = count.next_power_of_two();
        self.frames = frame_buffer(device, u64::from(self.frame_stride), self.frame_capacity);
        // A bind group holds the buffer it was created with, so the old frame buffer
        // cannot be swapped under it.
        self.groups.clear();
    }

    /// Write the `Frame` block for the view in slot `index`.
    pub fn write_frame(
        &self,
        queue: &wgpu::Queue,
        index: u32,
        camera: &Camera,
        size: [u32; 2],
    ) {
        let offset = u64::from(index) * u64::from(self.frame_stride);
        queue.write_buffer(
            &self.frames,
            offset,
            bytemuck::bytes_of(&Frame::from_camera(camera, size)),
        );
    }

    /// The dynamic offset of frame slot `index`.
    pub fn frame_offset(&self, index: u32) -> u32 {
        index * self.frame_stride
    }

    /// The `Program` buffer and bind group for `program`, creating them on first use.
    pub fn program_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: &ShaderProgram,
    ) -> &wgpu::BindGroup {
        let hash = program.hash();
        if !self.program_buffers.contains_key(&hash) {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ferrocad program"),
                size: PROGRAM_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(
                &buffer,
                0,
                bytemuck::bytes_of(&ProgramBlock::new(program.parameters())),
            );
            self.program_buffers.insert(hash, buffer);
        }
        if !self.groups.contains_key(&hash) {
            let group = bind_group(
                device,
                &self.group_layout,
                &self.frames,
                &self.program_buffers[&hash],
                "ferrocad program",
            );
            self.groups.insert(hash, group);
        }
        &self.groups[&hash]
    }

    /// The pipeline drawing `program` with `depth`, creating it on first use.
    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        program: &ShaderProgram,
        depth: &DepthState,
        topology: Topology,
    ) -> &wgpu::RenderPipeline {
        let key = ProgramKey::new(
            program,
            depth,
            topology_of(topology),
            self.sample_count,
            self.color_format,
        );
        self.pipelines.entry(key).or_insert_with(|| {
            create_pipeline(device, &self.layout, program, &key)
        })
    }

    /// The placeholder vertex buffer (4 corners; only their sign matters).
    pub fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.placeholder_vertices
    }

    /// The placeholder index buffer (6 indices, a `TriangleList`).
    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.placeholder_indices
    }

    /// The number of placeholder indices to draw.
    pub const fn index_count(&self) -> u32 {
        PLACEHOLDER_INDICES.len() as u32
    }

    /// Upload the placeholder geometry. Called once after construction; separate so the
    /// constructor does not need the queue.
    pub fn upload_placeholder(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.placeholder_vertices, 0, bytemuck::cast_slice(&PLACEHOLDER));
        queue.write_buffer(
            &self.placeholder_indices,
            0,
            bytemuck::cast_slice(&PLACEHOLDER_INDICES),
        );
    }
}

/// The wgpu topology for a neutral one.
fn topology_of(topology: Topology) -> wgpu::PrimitiveTopology {
    match topology {
        Topology::Triangles => wgpu::PrimitiveTopology::TriangleList,
        Topology::Lines => wgpu::PrimitiveTopology::LineList,
        Topology::Points => wgpu::PrimitiveTopology::PointList,
    }
}

/// Build one program pipeline.
fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    program: &ShaderProgram,
    key: &ProgramKey,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(program.name()),
        source: wgpu::ShaderSource::Wgsl(program.source().into()),
    });
    // The fixed ABI guarantees only `@location(0) position: vec3<f32>`; the placeholder
    // buffer carries nothing else.
    let attributes = wgpu::vertex_attr_array![0 => Float32x3];
    let vertex_buffers = [wgpu::VertexBufferLayout {
        array_stride: size_of::<[f32; 3]>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    }];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(program.name()),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &vertex_buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: key.topology,
            // A caller's winding is not part of the ABI.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(key.depth_write),
            depth_compare: Some(key.depth_compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: key.sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: key.color_format,
                blend: Some(key.blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A group 2 binding one frame buffer (dynamic offset) and one program buffer.
fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    frames: &wgpu::Buffer,
    program: &wgpu::Buffer,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: frames,
                    offset: 0,
                    size: wgpu::BufferSize::new(FRAME_SIZE),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: program,
                    offset: 0,
                    size: wgpu::BufferSize::new(PROGRAM_SIZE),
                }),
            },
        ],
    })
}

/// Allocate a uniform buffer of `capacity` slots of `stride` bytes.
fn frame_buffer(device: &wgpu::Device, stride: u64, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ferrocad frames"),
        size: stride * u64::from(capacity.max(1)),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_blocks_match_the_wgsl_abi() {
        assert_eq!(size_of::<Frame>() as u64, FRAME_SIZE);
        assert_eq!(size_of::<ProgramBlock>() as u64, PROGRAM_SIZE);
        assert_eq!(PROGRAM_SIZE % 16, 0, "uniform blocks are 16-byte aligned");
    }

    #[test]
    fn the_program_block_packs_the_fixed_abi() {
        let parameters = ProgramParameters {
            color: [0.1, 0.2, 0.3],
            base_spacing: 2.5,
            minor_alpha: 0.4,
            major_alpha: 0.6,
            shadow_opacity: 0.0,
            shadow_count: 0.0,
        };
        let block = ProgramBlock::new(&parameters);
        assert_eq!(block.grid_color, [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(block.values, [2.5, 0.4, 0.6, 0.0]);
        assert_eq!(block.counts, [0.0, 0.0, 0.0, 0.0]);
        assert!(block.shadows.iter().flatten().all(|value| *value == 0.0));
    }

    /// The pipeline key is a pure function of the program and the depth state.
    #[test]
    fn the_pipeline_key_follows_the_depth_state() {
        let program = ShaderProgram::new("grid", "@fragment fn fs_main() {}");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let key = |depth: &DepthState| {
            ProgramKey::new(
                &program,
                depth,
                wgpu::PrimitiveTopology::TriangleList,
                4,
                format,
            )
        };

        let default = key(&DepthState::default());
        assert_eq!(default.program, program.hash());
        assert_eq!(default.topology, wgpu::PrimitiveTopology::TriangleList);
        assert_eq!(default.depth_compare, wgpu::CompareFunction::Less);
        assert!(default.depth_write);
        assert_eq!(default.blend, wgpu::BlendState::ALPHA_BLENDING);
        assert_eq!(default.sample_count, 4);
        assert_eq!(default.color_format, format);

        // A disabled test collapses to Always; the write flag is independent.
        let no_test = key(&DepthState {
            test: false,
            ..DepthState::default()
        });
        assert_eq!(no_test.depth_compare, wgpu::CompareFunction::Always);
        assert_ne!(no_test, default, "a different depth state is a different pipeline");

        // A program's default state writes no depth.
        let program_default = key(&DepthState::PROGRAM);
        assert!(!program_default.depth_write);
        assert_eq!(program_default.depth_compare, wgpu::CompareFunction::Less);

        // A retuned program is a different pipeline.
        let retuned = program.clone().with_parameters(ProgramParameters {
            base_spacing: 10.0,
            ..Default::default()
        });
        let retuned_key =
            ProgramKey::new(&retuned, &DepthState::default(), wgpu::PrimitiveTopology::TriangleList, 4, format);
        assert_ne!(retuned_key.program, default.program);
    }

    #[test]
    fn every_depth_compare_maps_onto_wgpu() {
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
        for (neutral, wgpu_value) in cases {
            assert_eq!(compare_function(neutral), wgpu_value);
        }
    }
}
