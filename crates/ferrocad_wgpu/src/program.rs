//! Drawing an authored shader program: group 2, the pipeline cache and the placeholder.
//!
//! A [`DrawPacket`](ferrocad_graphics::DrawBatch) whose state carries a
//! [`ShaderProgram`] is drawn with **that** program instead of the scene pipeline. The
//! authored module is built with `create_shader_module` from the program's inline WGSL
//! and the pipeline is cached on a [`ProgramKey`]:
//!
//! ```text
//! (program fingerprint, geometry mode, topology, depth compare, depth write, blend,
//!  sample count, colour format)
//! ```
//!
//! A **plain** program does not draw the packet's authored geometry: it draws a
//! **4-vertex placeholder** (a `TriangleList`, six indices) whose only meaning is the
//! sign of each corner, which is what the fixed ABI's module derives screen corners
//! from. A **geometry** program draws its packet's own vertices and must therefore
//! declare the scene attribute layout (position/normal/uv/color); its topology is the
//! packet's. Culling is off either way, because a caller's winding is not part of the
//! ABI.
//!
//! ## Group 2 — the fixed ABI
//!
//! The **shared** scene pipeline layout is groups 0 (`Globals`), 1 (texture) and 2
//! (this module). The program shader is only required to declare group 2:
//!
//! | binding | block | size | contents |
//! | --- | --- | ---: | --- |
//! | 0 | `Frame` | 256 B | `view_proj`, `inv_view_proj`, world-space `eye`, `flags = [perspective, width, height, 0]`, then the host workplane: `workplane_origin = [x, y, z, patch_radius]`, `workplane_u`, `workplane_v`, `workplane_n`, `workplane_steps = [minor, major, 0, 0]`, `grid_stroke = [minor_lines, major_width, minor_width, 0]` |
//! | 1 | `Program` | 1392 B | `grid_color.rgb` (+ spare), `values = [base_spacing, minor_alpha, major_alpha, shadow_opacity]`, `counts = [shadow_count, 0, 0, 0]`, 80 zero `vec4`s, then the axis extension: `axis_y`, `axis_z`, `axis_values = [length, width, opacity, ghost_alpha]`, `axis_flags = [origin, labels, 0, 0]` |
//!
//! Binding 0 is a dynamic-offset uniform (one slot per view, so several cameras share
//! one buffer); binding 1 is a per-program buffer keyed on the fingerprint. Every scene
//! draw binds group 2 — ordinary draws bind [`ProgramPipeline::default_group`], a
//! zeroed `Frame`/`Program` pair — so the bind-group index never shifts between a
//! program pipeline and the scene pipeline.
//!
//! The workplane tail is **host-supplied**, not a program parameter: the viewport owns
//! the active construction plane, so a mod cannot claim a plane the view is not on. The
//! shader builds its grid in `workplane_u`/`workplane_v` around `workplane_origin` and,
//! when `workplane_origin.w` is non-zero, fades it out radially at that radius (the
//! native grid's patch). A zero radius means "no patch": the unbounded ground, which
//! with the default frame (origin 0, `u = +X`, `v = +Y`, `n = +Z`) is exactly the world
//! `XY` grid the shader drew before the field existed.
//!
//! Shadows are populated by the scene producer (`ferrocad_graphics::apply_shadows`):
//! the `Program` block carries up to 64 slots of `(centre, half-extent, blur)` and
//! `counts.x` is the slot count, so `grid_overlay.wgsl`'s loop runs only when the scene
//! supplied footprints. The four trailing axis `vec4`s are a backward-compatible
//! extension: the grid shader reads only the prefix, while `axis_overlay.wgsl` declares
//! the whole block.

use std::collections::HashMap;
use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use ferrocad_graphics::{
    Camera, DepthCompare, DepthState, ProgramParameters, ShaderProgram, Topology, WorkplaneFrame,
};
use ferrocad_types::{Matrix4, Vector3};

use crate::pipeline::DEPTH_FORMAT;

/// The `Frame` block's size (the WGSL layout: two matrices, seven `vec4`s).
pub const FRAME_SIZE: u64 = 256;
/// The `Program` block's size (three `vec4`s, 80 shadow `vec4`s, and four axis `vec4`s).
pub const PROGRAM_SIZE: u64 = 1_392;

/// The per-view `Frame` uniform (group 2, binding 0).
///
/// Byte layout (all members are 16-byte aligned, so the block is a plain `repr(C)`
/// prefix with no implicit padding):
///
/// | offset | bytes | field |
/// | ---: | ---: | --- |
/// | 0 | 64 | `view_proj` |
/// | 64 | 64 | `inv_view_proj` |
/// | 128 | 16 | `eye` |
/// | 144 | 16 | `flags` |
/// | 160 | 16 | `workplane_origin` (`xyz` world origin, `w` patch radius) |
/// | 176 | 16 | `workplane_u` |
/// | 192 | 16 | `workplane_v` |
/// | 208 | 16 | `workplane_n` |
/// | 224 | 16 | `workplane_steps` (`x` minor, `y` major, world units; `z`/`w` the levels' base ink) |
/// | 240 | 16 | `grid_stroke` (`x` 1 = minor lines, `y` major width, `z` minor width) |
/// | 256 | — | end (16-byte aligned) |
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
    /// `[origin.x, origin.y, origin.z, patch_radius]` of the active plane. A radius of
    /// `0` means "no patch": the shader draws the unbounded ground.
    pub workplane_origin: [f32; 4],
    /// The plane's in-plane `u` axis (the grid's `x`), `w` reserved.
    pub workplane_u: [f32; 4],
    /// The plane's in-plane `v` axis (the grid's `y`), `w` reserved.
    pub workplane_v: [f32; 4],
    /// The plane's unit normal (`u × v`), `w` reserved.
    pub workplane_n: [f32; 4],
    /// `[minor, major, minor_alpha, major_alpha]`: the plane's steps in world units and
    /// the **base ink** of its two levels, as the host resolved them (the native grid's
    /// effective steps and alphas). The grid shader draws these instead of deriving
    /// decades from screen density and using its own alpha, so both backends show the
    /// same lattice at the same weight. An alpha of `0` means "no claim": the shader
    /// falls back to the program's own `values.y`/`values.z`.
    pub workplane_steps: [f32; 4],
    /// `[minor_lines ? 1 : 0, major_width, minor_width, 0]`: the minor level's form and
    /// the two stroke widths in **world units**, as the native generator draws them. A
    /// width of `0` is the shader's own 1 px hairline, so an unset host keeps the old
    /// look. The strokes lie in the plane, which is what the native quads do.
    pub grid_stroke: [f32; 4],
}

/// The active construction plane a program draw builds its grid in (host-supplied).
///
/// This is **not** part of the program parameter ABI: the viewport owns the active
/// plane, so it fills [`Frame`]'s workplane tail from `ActiveWorkplane` instead of a
/// mod's `SoShaderParameter*` nodes. A mod therefore cannot claim a plane the view is
/// not on. The default is world `XY` with radius `0` (the unbounded ground).
///
/// The carrier itself is [`WorkplaneFrame`] from `ferrocad_graphics`, because a view
/// and a backend share it; this module only owns how it is packed.

/// A host's ink claim: a positive finite alpha, else `0` ("no claim").
fn claim(alpha: f32) -> f32 {
    if alpha.is_finite() && alpha > 0.0 {
        alpha
    } else {
        0.0
    }
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
        Self::from_camera_and_workplane(camera, size, WorkplaneFrame::world())
    }

    /// [`from_camera`](Self::from_camera) with the host's active construction plane.
    ///
    /// The plane is **not** part of the program parameter ABI: the viewport owns it, so
    /// it arrives on the view and is written here with the frame. A mod therefore cannot
    /// claim a plane the view is not on.
    pub fn from_camera_and_workplane(
        camera: &Camera,
        size: [u32; 2],
        workplane: WorkplaneFrame,
    ) -> Self {
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
            workplane_origin: [
                workplane.origin[0],
                workplane.origin[1],
                workplane.origin[2],
                workplane.patch_radius(),
            ],
            workplane_u: [workplane.u[0], workplane.u[1], workplane.u[2], 0.0],
            workplane_v: [workplane.v[0], workplane.v[1], workplane.v[2], 0.0],
            workplane_n: [workplane.n[0], workplane.n[1], workplane.n[2], 0.0],
            workplane_steps: [
                workplane.minor,
                workplane.major,
                // A host that does not claim the ink leaves `0` here (and a negative or
                // non-finite claim is treated the same way), which the shader reads as
                // "use the program's parameter" — the pre-ink ABI.
                claim(workplane.minor_alpha),
                claim(workplane.major_alpha),
            ],
            grid_stroke: [
                if workplane.minor_lines { 1.0 } else { 0.0 },
                workplane.major_width,
                workplane.minor_width,
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
    /// The Y axis stem's linear RGB ink (`w` spare).
    pub axis_y: [f32; 4],
    /// The Z axis stem's linear RGB ink (`w` spare).
    pub axis_z: [f32; 4],
    /// `[axis_length, axis_width, axis_opacity, axis_ghost_alpha]`.
    pub axis_values: [f32; 4],
    /// `[axis_origin, axis_labels, 0, 0]`.
    pub axis_flags: [f32; 4],
}

impl ProgramBlock {
    /// Pack the fixed-ABI parameters, including the producer's shadow block.
    pub fn new(parameters: &ProgramParameters) -> Self {
        let [r, g, b] = parameters.color;
        let [yr, yg, yb] = parameters.axis_y;
        let [zr, zg, zb] = parameters.axis_z;
        Self {
            grid_color: [r, g, b, 0.0],
            values: [
                parameters.base_spacing,
                parameters.minor_alpha,
                parameters.major_alpha,
                parameters.shadow_opacity,
            ],
            counts: [parameters.shadow_count, 0.0, 0.0, 0.0],
            shadows: parameters.shadow_data,
            axis_y: [yr, yg, yb, 0.0],
            axis_z: [zr, zg, zb, 0.0],
            axis_values: [
                parameters.axis_length,
                parameters.axis_width,
                parameters.axis_opacity,
                parameters.axis_ghost_alpha,
            ],
            axis_flags: [
                parameters.axis_origin,
                parameters.axis_labels,
                0.0,
                0.0,
            ],
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
///
/// `geometry` is part of the key because the two modes need different vertex state: a
/// plain program consumes only the placeholder's `position`, while a geometry program
/// consumes the scene layout. The same source used both ways therefore produces two
/// pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramKey {
    /// The program's content fingerprint ([`ShaderProgram::hash`]), which already
    /// includes the geometry mode.
    pub program: u64,
    /// Whether the program draws the packet's mesh (`true`) or the ABI placeholder.
    pub geometry: bool,
    /// The primitive topology: the packet's for a geometry program, `TriangleList` for
    /// the placeholder.
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
            geometry: program.geometry(),
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

    /// Write the `Frame` block for the view in slot `index` (world `XY`, no patch).
    pub fn write_frame(
        &self,
        queue: &wgpu::Queue,
        index: u32,
        camera: &Camera,
        size: [u32; 2],
    ) {
        self.write_frame_with_workplane(
            queue,
            index,
            camera,
            size,
            WorkplaneFrame::world(),
        );
    }

    /// Write the `Frame` block for the view in slot `index`, with the view's active
    /// construction plane.
    ///
    /// The workplane is host state, so it is written here with the frame rather than
    /// through the program's parameter block. Views drawn without one keep
    /// [`write_frame`](Self::write_frame)'s world `XY` default.
    pub fn write_frame_with_workplane(
        &self,
        queue: &wgpu::Queue,
        index: u32,
        camera: &Camera,
        size: [u32; 2],
        workplane: WorkplaneFrame,
    ) {
        let offset = u64::from(index) * u64::from(self.frame_stride);
        queue.write_buffer(
            &self.frames,
            offset,
            bytemuck::bytes_of(&Frame::from_camera_and_workplane(camera, size, workplane)),
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
            crate::trace::program_buffer(hash);
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

    /// The pipeline drawing `program` with `depth` and `topology`, creating it on first
    /// use. A plain program always draws the placeholder `TriangleList`, so callers pass
    /// `Topology::Triangles` for it; a geometry program passes the packet's topology.
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
    crate::trace::pipeline_created(
        key.program,
        key.topology,
        key.depth_compare,
        key.depth_write,
        key.sample_count,
        key.color_format,
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(program.name()),
        source: wgpu::ShaderSource::Wgsl(program.source().into()),
    });
    // The fixed ABI guarantees only `@location(0) position: vec3<f32>`; the placeholder
    // buffer carries nothing else. A geometry program instead reads the packet's mesh,
    // whose vertices are laid out like the scene's.
    let placeholder_attributes = wgpu::vertex_attr_array![0 => Float32x3];
    let placeholder_buffers = [wgpu::VertexBufferLayout {
        array_stride: size_of::<[f32; 3]>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &placeholder_attributes,
    }];
    let scene_buffers = crate::pipeline::scene_vertex_layout();
    let vertex_buffers: &[wgpu::VertexBufferLayout<'_>] = if key.geometry {
        &scene_buffers
    } else {
        &placeholder_buffers
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(program.name()),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: vertex_buffers,
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
    use ferrocad_graphics::{ShadowFootprint, apply_shadows};

    #[test]
    fn the_uniform_blocks_match_the_wgsl_abi() {
        assert_eq!(size_of::<Frame>() as u64, FRAME_SIZE);
        assert_eq!(size_of::<ProgramBlock>() as u64, PROGRAM_SIZE);
        assert_eq!(FRAME_SIZE % 16, 0, "uniform blocks are 16-byte aligned");
        assert_eq!(PROGRAM_SIZE % 16, 0, "uniform blocks are 16-byte aligned");
    }

    /// The host's active plane reaches the exact bytes the WGSL `Frame` declares.
    #[test]
    fn the_frame_packs_the_active_workplane() {
        // A tilted face-like plane: origin off the world origin, axes swapped.
        let plane = WorkplaneFrame {
            origin: [1.5, -2.0, 3.25],
            u: [0.0, 1.0, 0.0],
            v: [0.0, 0.0, 1.0],
            n: [1.0, 0.0, 0.0],
            minor: 2.5,
            major: 25.0,
            radius: 200.0,
            minor_alpha: 0.15,
            major_alpha: 0.3,
            minor_lines: true,
            major_width: 0.4,
            minor_width: 0.2,
        };
        let frame = Frame::from_camera_and_workplane(&Camera::identity(), [640, 480], plane);
        assert_eq!(frame.workplane_origin, [1.5, -2.0, 3.25, 200.0]);
        assert_eq!(frame.workplane_u, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(frame.workplane_v, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(frame.workplane_n, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            frame.workplane_steps,
            [2.5, 25.0, 0.15, 0.3],
            "steps and the base ink travel together"
        );
        assert_eq!(frame.grid_stroke, [1.0, 0.4, 0.2, 0.0]);

        // The offsets are the documented ABI: the workplane tail starts after `flags`
        // (160 B) and each field is one 16-byte `vec4`.
        let bytes = bytemuck::bytes_of(&frame);
        assert_eq!(&bytes[160..176], bytemuck::bytes_of(&frame.workplane_origin));
        assert_eq!(&bytes[176..192], bytemuck::bytes_of(&frame.workplane_u));
        assert_eq!(&bytes[192..208], bytemuck::bytes_of(&frame.workplane_v));
        assert_eq!(&bytes[208..224], bytemuck::bytes_of(&frame.workplane_n));
        assert_eq!(&bytes[224..240], bytemuck::bytes_of(&frame.workplane_steps));
        assert_eq!(&bytes[240..256], bytemuck::bytes_of(&frame.grid_stroke));

        // The pre-workplane fields keep their offsets: the extension is a suffix.
        assert_eq!(&bytes[128..144], bytemuck::bytes_of(&frame.eye));
        assert_eq!(&bytes[144..160], bytemuck::bytes_of(&frame.flags));
    }

    /// No host plane means world `XY` and no patch: the shader's original behaviour.
    #[test]
    fn a_world_workplane_is_the_unbounded_ground() {
        let frame = Frame::from_camera(&Camera::identity(), [640, 480]);
        assert_eq!(frame.workplane_origin, [0.0, 0.0, 0.0, 0.0], "radius 0 = no patch");
        assert_eq!(frame.workplane_u, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(frame.workplane_v, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(frame.workplane_n, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(frame.workplane_steps, [10.0, 100.0, 0.0, 0.0]);
        assert_eq!(frame.grid_stroke, [0.0, 0.0, 0.0, 0.0], "dots, hairlines");
        assert_eq!(WorkplaneFrame::world().radius, 0.0);
    }

    /// A degenerate host radius must draw the ground, not a zero-width patch.
    #[test]
    fn a_degenerate_patch_radius_is_no_patch() {
        for radius in [0.0, -5.0, f32::NAN, f32::INFINITY] {
            let plane = WorkplaneFrame {
                radius,
                ..WorkplaneFrame::world()
            };
            let frame = Frame::from_camera_and_workplane(&Camera::identity(), [640, 480], plane);
            assert_eq!(
                frame.workplane_origin[3], 0.0,
                "radius {radius} should mean no patch"
            );
        }
        let plane = WorkplaneFrame {
            radius: 12.5,
            ..WorkplaneFrame::world()
        };
        let frame = Frame::from_camera_and_workplane(&Camera::identity(), [640, 480], plane);
        assert_eq!(frame.workplane_origin[3], 12.5);
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
            ..Default::default()
        };
        let block = ProgramBlock::new(&parameters);
        assert_eq!(block.grid_color, [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(block.values, [2.5, 0.4, 0.6, 0.0]);
        assert_eq!(block.counts, [0.0, 0.0, 0.0, 0.0]);
        assert!(block.shadows.iter().flatten().all(|value| *value == 0.0));
    }

    /// The producer's slots reach the uniform verbatim, so the shader's flat
    /// `shadow_at(o)` indexing sees exactly what `ferrocad_graphics::apply_shadows`
    /// wrote.
    #[test]
    fn the_program_block_packs_the_shadow_producer() {
        let mut parameters = ProgramParameters {
            shadow_opacity: 0.3,
            ..Default::default()
        };
        assert_eq!(
            apply_shadows(
                &mut parameters,
                [
                    ShadowFootprint::from_bounds([-5.0, -5.0], [-1.0, -1.0], 0.5),
                    ShadowFootprint::from_bounds([2.0, 3.0], [4.0, 7.0], 0.75),
                ],
            ),
            2
        );

        let block = ProgramBlock::new(&parameters);
        assert_eq!(block.counts, [2.0, 0.0, 0.0, 0.0], "counts.x is the slot count");
        assert_eq!(block.values[3], 0.3, "the shared strength still rides values.w");

        // Slot 0: [centre.x, centre.y, half.x, half.y] then blur in the next vec4.
        assert_eq!(block.shadows[0], [-3.0, -3.0, 2.0, 2.0]);
        assert_eq!(block.shadows[1][0], 0.5);
        // Slot 1 crosses another vec4 boundary (flat float 5).
        assert_eq!(block.shadows[1][1], 3.0);
        assert_eq!(block.shadows[1][2], 5.0);
        assert_eq!(block.shadows[1][3], 1.0);
        assert_eq!(block.shadows[2][0], 2.0);
        assert_eq!(block.shadows[2][1], 0.75);

        // The shadow region starts after the three header vec4s (3 * 16 bytes), so the
        // grid shader's prefix and the axis tail keep their offsets.
        let bytes = bytemuck::bytes_of(&block);
        assert_eq!(&bytes[48..64], bytemuck::bytes_of(&block.shadows[0]));
        assert_eq!(&bytes[64..68], bytemuck::bytes_of(&block.shadows[1][0]));
    }

    /// The extended tail is packed after the grid prefix, so the grid shader's
    /// 1,328-byte view of the block is still a valid prefix.
    #[test]
    fn the_program_block_packs_the_axis_extension() {
        let parameters = ProgramParameters {
            axis_y: [0.1, 0.2, 0.3],
            axis_z: [0.4, 0.5, 0.6],
            axis_length: 120.0,
            axis_width: 2.5,
            axis_opacity: 0.5,
            axis_ghost_alpha: 0.2,
            axis_origin: 0.0,
            axis_labels: 1.0,
            ..Default::default()
        };
        let block = ProgramBlock::new(&parameters);
        assert_eq!(block.axis_y, [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(block.axis_z, [0.4, 0.5, 0.6, 0.0]);
        assert_eq!(block.axis_values, [120.0, 2.5, 0.5, 0.2]);
        assert_eq!(block.axis_flags, [0.0, 1.0, 0.0, 0.0]);

        // The axis tail starts after the grid prefix (three vec4s + 80 shadow vec4s).
        let bytes = bytemuck::bytes_of(&block);
        assert_eq!(&bytes[1_328..1_344], bytemuck::bytes_of(&block.axis_y));
        assert_eq!(&bytes[1_376..], bytemuck::bytes_of(&block.axis_flags));
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
        assert!(!default.geometry, "a plain program keeps the placeholder");
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

        // The target's sample count and colour format are part of the key: a resize
        // that changes either must not reuse a pipeline built for the old attachment.
        assert_ne!(
            ProgramKey::new(
                &program,
                &DepthState::default(),
                wgpu::PrimitiveTopology::TriangleList,
                1,
                format,
            ),
            default,
            "a different sample count is a different pipeline"
        );
        assert_ne!(
            ProgramKey::new(
                &program,
                &DepthState::default(),
                wgpu::PrimitiveTopology::TriangleList,
                4,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ),
            default,
            "a different colour format is a different pipeline"
        );

        // A retuned program is a different pipeline.
        let retuned = program.clone().with_parameters(ProgramParameters {
            base_spacing: 10.0,
            ..Default::default()
        });
        let retuned_key =
            ProgramKey::new(&retuned, &DepthState::default(), wgpu::PrimitiveTopology::TriangleList, 4, format);
        assert_ne!(retuned_key.program, default.program);
    }

    /// The same source used as a plain and as a geometry program must not share a
    /// pipeline: the two modes need different vertex state.
    #[test]
    fn the_pipeline_key_includes_the_geometry_mode() {
        let plain = ShaderProgram::new("gizmo", "@vertex fn vs_main() {}");
        let geometry = plain.clone().with_geometry();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let key = |program: &ShaderProgram| {
            ProgramKey::new(
                program,
                &DepthState::PROGRAM,
                wgpu::PrimitiveTopology::TriangleList,
                1,
                format,
            )
        };

        let plain_key = key(&plain);
        let geometry_key = key(&geometry);
        assert!(!plain_key.geometry);
        assert!(geometry_key.geometry);
        assert_ne!(plain_key, geometry_key);
        assert_ne!(
            plain_key.program, geometry_key.program,
            "the fingerprints already differ; the key carries the mode additionally"
        );

        // The pipeline cache is a `HashMap<ProgramKey, _>`: one source used both ways is
        // two resident pipelines.
        let mut cache = HashMap::new();
        cache.insert(plain_key, "placeholder");
        cache.insert(geometry_key, "scene layout");
        assert_eq!(cache.len(), 2, "one source, two vertex layouts");

        // A geometry program's topology is the packet's, so a line-list variant is a
        // third pipeline.
        let lines = ProgramKey::new(
            &geometry,
            &DepthState::PROGRAM,
            wgpu::PrimitiveTopology::LineList,
            1,
            format,
        );
        assert_ne!(lines, geometry_key);
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
