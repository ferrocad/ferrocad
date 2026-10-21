//! The mesh compiler: turn a [`Scene`] into a render queue.
//!
//! Compilation is the traversal Coin performs in its render action: walk the scene
//! accumulating inherited state (transform, colour, …) and emit a [`DrawPacket`] per
//! drawable, with that state frozen into a [`StateSnapshot`](crate::StateSnapshot)
//! and the geometry flattened to GPU-ready vertices.
//!
//! Fields follow Coin's scoping rules: a `Material`, `Coordinate3` or `Transform`
//! affects the traversal that *follows* it within its `Separator` (i.e. later
//! siblings), and a `Separator` isolates its subtree from the state around it.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrocad_graphics::{DepthCompare, DepthState, ProgramParameters, ShaderProgram};

use crate::batch::{DrawPacket, Generation, MeshId, PickId, RenderQueue};
use crate::directive::{Directive, Directives};
use crate::geometry::{Geometry, Topology, Vertex};
use crate::node::{
    Node, NodeKind, SO_SWITCH_ALL, SO_SWITCH_INHERIT, Scene, ShaderParameter, ShaderStage,
    ShaderStageKind, depth_function, pick_style,
};
use crate::pixels::Viewport;
use crate::state::State;

/// Compile `scene` into a `(RenderQueue, Directives)` pair at a device-pixel ratio of 1.
///
/// A convenience for callers that have no viewport (tests, headless tools); the
/// host uses [`compile_with`].
pub fn compile(scene: &Scene) -> (RenderQueue, Directives) {
    compile_with(scene, &Viewport::default())
}

/// Compile `scene` into a `(RenderQueue, Directives)` pair for `viewport`.
///
/// Each [`NodeKind::Mesh`] becomes one packet. Geometry is attached **on every
/// compile** (a fresh upload from the compiler's point of view); a renderer keys its
/// upload cache on `(mesh_id, generation)`. `mesh_id` is allocated in traversal order,
/// which can change when a producer reorders or omits packets (the navigation cube
/// sorts back-to-front, and hides some labels), so the generation is a **fingerprint
/// of the geometry bytes**: a reused id whose geometry changed no longer matches the
/// cached upload, while identical geometry still hits.
///
/// Semantic nodes (a [`NodeKind::NaviCube`]) emit **no** packet: they are lifted into
/// the [`Directives`] the viewport consumes (see `docs/directives.md`). A
/// [`NodeKind::ShaderProgram`] and its stage/parameter values emit no packet either;
/// the program binds to the sibling shapes that follow it.
///
/// `viewport`'s device-pixel ratio seeds [`State::device_pixel_ratio`], which
/// screen-space nodes read while building geometry. Because that scaling is baked in
/// at compile time, a change of `device_pixel_ratio` (e.g. moving the window to a
/// display with a different scale factor) requires recompiling the scene.
///
/// [`Scene::root`] is treated as a group: its children are traversed at the top level.
///
/// This wrapper has no base directory for Coin's `sourceProgram` paths, so a stage that
/// uses only `sourceProgram` fails loudly (it panics). Use [`compile_with_base`] when
/// the caller can name the directory.
///
/// # Panics
///
/// Panics if a shader program cannot be assembled. Headless tools that must handle
/// the error should call [`compile_with_base`].
pub fn compile_with(scene: &Scene, viewport: &Viewport) -> (RenderQueue, Directives) {
    compile_with_base(scene, viewport, None)
        .unwrap_or_else(|error| panic!("shader program extraction failed: {error}"))
}

/// Compile `scene`, resolving `sourceProgram` paths against `base`.
///
/// Inline `source` is always authoritative; `base` is consulted only for a stage
/// that supplies no inline source. A path that is absolute ignores `base`. A stage
/// with neither is treated as empty (the other stage may hold the whole module).
///
/// Unlike [`compile_with`] this reports a malformed program (missing or duplicate
/// stage, unknown parameter name, unreadable file) instead of panicking. A malformed
/// program produces no draw queue; the error names the stage or parameter.
pub fn compile_with_base(
    scene: &Scene,
    viewport: &Viewport,
    base: Option<&Path>,
) -> Result<(RenderQueue, Directives), ShaderError> {
    let mut state = State::default();
    state.device_pixel_ratio = viewport.device_pixel_ratio();

    let mut ctx = Compiler {
        next_mesh: 0,
        packets: Vec::new(),
        directives: Directives::new(),
        base: base.map(Path::to_path_buf),
        error: None,
    };
    let mut coordinates = Vec::new();
    let mut tex_coords = Vec::new();
    walk_children(
        &scene.root.children,
        &state,
        &mut coordinates,
        &mut tex_coords,
        &mut ctx,
        SO_SWITCH_ALL,
    );
    if let Some(error) = ctx.error {
        return Err(error);
    }
    Ok((
        RenderQueue {
            packets: ctx.packets,
        },
        ctx.directives,
    ))
}

struct Compiler {
    next_mesh: u64,
    packets: Vec<DrawPacket>,
    directives: Directives,
    /// The base directory for Coin `sourceProgram` paths, supplied explicitly by the
    /// caller; `None` makes a path-only stage an error.
    base: Option<PathBuf>,
    /// The first program-assembly error. Traversal continues (so diagnostics are not
    /// truncated) and [`compile_with_base`] reports it at the end.
    error: Option<ShaderError>,
}

/// Walk a list of siblings in order, threading `state` and the coordinate/texture
/// buffers forward across them (so a field node affects the siblings that follow it).
///
/// `switch_state` is the nearest enclosing `SoSwitch` selection: a non-negative
/// child index, [`SO_SWITCH_NONE`](crate::node::SO_SWITCH_NONE) or
/// [`SO_SWITCH_ALL`]. A nested switch whose `which` is
/// [`SO_SWITCH_INHERIT`](crate::node::SO_SWITCH_INHERIT) resolves against it. At
/// the top level there is no enclosing selection, so it starts as
/// [`SO_SWITCH_ALL`] (Coin's default switch element).
fn walk_children(
    children: &[Node],
    state: &State,
    coordinates: &mut Vec<[f32; 3]>,
    tex_coords: &mut Vec<[f32; 2]>,
    ctx: &mut Compiler,
    switch_state: i32,
) {
    let mut state = state.clone();
    for child in children {
        match &child.kind {
            // A separator isolates fields (and the coordinate fields) to its subtree.
            NodeKind::Separator => {
                let saved_coords = std::mem::take(coordinates);
                let saved_uv = std::mem::take(tex_coords);
                walk_children(
                    &child.children,
                    &state,
                    coordinates,
                    tex_coords,
                    ctx,
                    switch_state,
                );
                *coordinates = saved_coords;
                *tex_coords = saved_uv;
            }
            NodeKind::Transform(placement) => {
                state.transform = state.transform.mul(&placement.to_matrix());
            }
            // A material list sets the draw colour to its first (diffuse) entry;
            // an empty list leaves the inherited colour in place.
            NodeKind::Material(colors) => {
                if let Some(color) = colors.first() {
                    state.color = *color;
                }
            }
            NodeKind::Coordinate3(points) => {
                *coordinates = points.clone();
            }
            NodeKind::TextureCoordinate2(uv) => {
                *tex_coords = uv.clone();
            }
            NodeKind::Texture2(texture) => {
                state.texture = Some(texture.clone());
            }
            // A selection id applies to the shapes that follow it.
            NodeKind::PickId(id) => {
                state.id = PickId(*id);
            }
            // Normals round-trip through the model but are not consumed yet
            // (flat shading); Phase 3 will thread them into the mesh compiler.
            NodeKind::Normal(_) => {}
            // `SoSwitch` derives from `SoGroup`, so it does not isolate state: the
            // selected child(ren) inherit (and may leak) the current state.
            // `SO_SWITCH_INHERIT` takes the enclosing switch's selection verbatim,
            // including a sibling index; with no enclosing switch the traversal
            // starts at `SO_SWITCH_ALL`, so a root `INHERIT` draws every child.
            NodeKind::Switch { which } => {
                let selection = if *which == SO_SWITCH_INHERIT {
                    switch_state
                } else {
                    *which
                };
                match selection {
                    SO_SWITCH_ALL => walk_children(
                        &child.children,
                        &state,
                        coordinates,
                        tex_coords,
                        ctx,
                        SO_SWITCH_ALL,
                    ),
                    n if n < 0 => {}
                    n => {
                        if let Some(active) = child.children.get(n as usize) {
                            walk_children(
                                std::slice::from_ref(active),
                                &state,
                                coordinates,
                                tex_coords,
                                ctx,
                                n,
                            );
                        }
                    }
                }
            }
            NodeKind::Mesh {
                indices, topology, ..
            } => {
                emit_mesh(&state, coordinates, tex_coords, indices, *topology, ctx);
            }
            // `SoFaceSet` consumes `num_vertices[i]` consecutive coordinates per
            // face; the compiler fan-triangulates each polygon (correct for the
            // convex faces in scope; a general n-gon needs the later triangulator).
            NodeKind::FaceSet { num_vertices } => {
                let indices = triangulate_faces(num_vertices, coordinates.len());
                emit_mesh(
                    &state,
                    coordinates,
                    tex_coords,
                    &indices,
                    Topology::Triangles,
                    ctx,
                );
            }
            // Recorded state nodes with no renderer effect. `SoLightModel` is
            // deliberately inert: under a program the shader owns shading, and without
            // one the renderer has a single fixed light model (`scene.wgsl`), so a
            // `BASE_COLOR` pipeline would be dead code (plan §4.4).
            NodeKind::LightModel { .. } => {}
            // `SoPickStyle`: only `UNPICKABLE` acts. `SHAPE` (and `BOUNDING_BOX`, which
            // is not modelled) restore pickability, so a nested `UNPICKABLE` does not
            // leak into the rest of the file.
            NodeKind::PickStyle { style } => {
                state.pickable = *style != pick_style::UNPICKABLE;
            }
            // `SoDepthBuffer` becomes the depth test/write state the renderer's
            // pipeline takes. `test == false` compares `Always` (see `DepthState`).
            NodeKind::DepthBuffer {
                test,
                write,
                function,
            } => {
                state.depth = DepthState {
                    test: *test,
                    write: *write,
                    compare: depth_compare(*function),
                };
            }
            // A navigation-cube node draws nothing: it becomes a host-facing directive.
            NodeKind::NaviCube(directive) => {
                ctx.directives.push(Directive::NaviCube(*directive));
            }
            // A program node binds to the siblings that follow it. It draws nothing
            // itself, and its children (the `shaderObject` stages) are values, so the
            // traversal never descends into them.
            NodeKind::ShaderProgram => {
                if ctx.error.is_none() {
                    match build_program(child, ctx.base.as_deref()) {
                        Ok(program) => {
                            state.program = Some(Arc::new(program));
                            // A program covers its shape with a full-screen
                            // placeholder instead of the shape's own geometry, so the
                            // packet must not write depth (it would hide everything
                            // behind it) and must not be picked. A later
                            // `SoDepthBuffer`/`SoPickStyle` may grow back the depth
                            // write; the pick paths skip a program packet regardless.
                            state.depth = DepthState::PROGRAM;
                            state.pickable = false;
                        }
                        Err(error) => ctx.error = Some(error),
                    }
                }
            }
            // Stage and parameter nodes are values: never packets. They are normally
            // reached through a program node, which does not descend; a stray one at
            // sibling level is accepted and ignored.
            NodeKind::ShaderStage(_) | NodeKind::ShaderParameter(_) => {}
            // Custom node classes carry no geometry the core understands yet.
            NodeKind::Custom(_) => {}
            // `SoEventCallback` draws nothing; it becomes a host-facing directive so the
            // viewport can route input to the callables keyed by its id (same traversal,
            // so `SoSwitch` state is honoured without a rescan).
            NodeKind::EventCallback(id) => {
                ctx.directives.push(Directive::EventCallback(*id));
            }
        }
    }
}

/// Assemble the [`ShaderProgram`] a [`NodeKind::ShaderProgram`] node links.
///
/// The node's children are Coin's `shaderObject` (`SoMFNode`): exactly one vertex and
/// one fragment stage, in any order. The module is the concatenation of the stages'
/// non-empty WGSL sources in (vertex, fragment) order, because WGSL modules are whole
/// programs with no per-stage compilation. Parameters come from each stage's
/// `parameter` list and are mapped by name onto [`ProgramParameters`].
fn build_program(node: &Node, base: Option<&Path>) -> Result<ShaderProgram, ShaderError> {
    let mut stages: [Option<&Node>; 2] = [None, None];
    for child in &node.children {
        let NodeKind::ShaderStage(stage) = &child.kind else {
            return Err(ShaderError::NotAStage(class_name(&child.kind).to_owned()));
        };
        let slot = match stage.kind {
            ShaderStageKind::Vertex => 0,
            ShaderStageKind::Fragment => 1,
        };
        if stages[slot].is_some() {
            return Err(ShaderError::DuplicateStage(stage.kind.class_name()));
        }
        stages[slot] = Some(child);
    }
    let vertex = stages[0].ok_or(ShaderError::MissingStage("SoVertexShader"))?;
    let fragment = stages[1].ok_or(ShaderError::MissingStage("SoFragmentShader"))?;

    let mut source = String::new();
    for stage in [vertex, fragment] {
        let NodeKind::ShaderStage(value) = &stage.kind else {
            unreachable!("stages were filtered to ShaderStage above");
        };
        let text = stage_source(value, base)?;
        if text.trim().is_empty() {
            continue;
        }
        if !source.is_empty() {
            source.push('\n');
        }
        source.push_str(&text);
    }
    if source.trim().is_empty() {
        return Err(ShaderError::MissingSource);
    }

    let parameters = collect_parameters(vertex, fragment)?;
    Ok(ShaderProgram::from_source(source).with_parameters(parameters))
}

/// The WGSL text one stage contributes: inline `source` wins, otherwise Coin's
/// `sourceProgram` path is resolved against the explicit `base` directory.
fn stage_source<'a>(
    stage: &'a ShaderStage,
    base: Option<&Path>,
) -> Result<Cow<'a, str>, ShaderError> {
    if let Some(source) = stage.source.as_deref() {
        if !source.trim().is_empty() {
            return Ok(Cow::Borrowed(source));
        }
    }
    let Some(path) = stage.source_program.as_deref() else {
        return Ok(Cow::Borrowed(""));
    };
    let base = base.ok_or_else(|| ShaderError::MissingBaseDirectory {
        path: path.to_owned(),
    })?;
    let full = base.join(path);
    std::fs::read_to_string(&full)
        .map(Cow::Owned)
        .map_err(|error| ShaderError::UnreadableSource {
            path: full,
            error: error.to_string(),
        })
}

/// Collect both stages' `parameter` lists into the fixed ABI block.
fn collect_parameters(vertex: &Node, fragment: &Node) -> Result<ProgramParameters, ShaderError> {
    let mut parameters = ProgramParameters::default();
    let mut seen: Vec<&str> = Vec::new();
    for stage in [vertex, fragment] {
        for child in &stage.children {
            let NodeKind::ShaderParameter(parameter) = &child.kind else {
                return Err(ShaderError::NotAParameter(
                    class_name(&child.kind).to_owned(),
                ));
            };
            if seen.contains(&parameter.name()) {
                return Err(ShaderError::DuplicateParameter(parameter.name().to_owned()));
            }
            seen.push(parameter.name());
            apply_parameter(&mut parameters, parameter)?;
        }
    }
    Ok(parameters)
}

/// Map one parameter onto its ABI slot, or name the slot it asked for.
fn apply_parameter(
    parameters: &mut ProgramParameters,
    parameter: &ShaderParameter,
) -> Result<(), ShaderError> {
    match parameter {
        ShaderParameter::Float { name, value } => match name.as_str() {
            "base_spacing" => parameters.base_spacing = *value,
            "minor_alpha" => parameters.minor_alpha = *value,
            "major_alpha" => parameters.major_alpha = *value,
            "shadow_opacity" => parameters.shadow_opacity = *value,
            "shadow_count" => parameters.shadow_count = *value,
            _ => return Err(ShaderError::UnknownParameter(name.clone())),
        },
        ShaderParameter::Vec3 { name, value } => match name.as_str() {
            "grid_color" => parameters.color = *value,
            _ => return Err(ShaderError::UnknownParameter(name.clone())),
        },
        ShaderParameter::Floats { name, values } => match name.as_str() {
            // The shadow block (`SHADOW_MAX * SHADOW_STRIDE` floats) is part of the
            // fixed ABI but deferred: no document-bounds producer is wired yet
            // (`docs/grid-overlay-plan.md` §3.5). Accept the slot so an upstream tree
            // extracts, and do not pretend to apply the data.
            "shadow_data" => {
                let _ = values;
            }
            _ => return Err(ShaderError::UnknownParameter(name.clone())),
        },
    }
    Ok(())
}

/// The Coin class name for a node kind (used by program errors only).
fn class_name(kind: &NodeKind) -> &str {
    match kind {
        NodeKind::ShaderProgram => "SoShaderProgram",
        NodeKind::ShaderStage(stage) => stage.kind.class_name(),
        NodeKind::ShaderParameter(parameter) => parameter.class_name(),
        NodeKind::Separator => "SoSeparator",
        NodeKind::Switch { .. } => "SoSwitch",
        NodeKind::Coordinate3(_) => "SoCoordinate3",
        NodeKind::TextureCoordinate2(_) => "SoTextureCoordinate2",
        NodeKind::Texture2(_) => "SoTexture2",
        NodeKind::Normal(_) => "SoNormal",
        NodeKind::Mesh { .. } => "SoIndexedShape",
        NodeKind::FaceSet { .. } => "SoFaceSet",
        NodeKind::PickStyle { .. } => "SoPickStyle",
        NodeKind::LightModel { .. } => "SoLightModel",
        NodeKind::DepthBuffer { .. } => "SoDepthBuffer",
        NodeKind::Material(_) => "SoMaterial",
        NodeKind::PickId(_) => "SoPickId",
        NodeKind::Transform(_) => "SoTransform",
        NodeKind::EventCallback(_) => "SoEventCallback",
        NodeKind::NaviCube(_) => "SoNaviCube",
        NodeKind::Custom(name) => name,
    }
}

/// Why a shader program could not be assembled. Reported by [`compile_with_base`].
#[derive(Clone, Debug, PartialEq)]
pub enum ShaderError {
    /// The program has no stage of this Coin class.
    MissingStage(&'static str),
    /// The program links two stages of the same kind.
    DuplicateStage(&'static str),
    /// A child of `shaderObject` is not a stage node.
    NotAStage(String),
    /// A child of a stage's `parameter` list is not a parameter node.
    NotAParameter(String),
    /// Neither stage supplied any WGSL.
    MissingSource,
    /// A stage uses `sourceProgram` but the caller supplied no base directory.
    MissingBaseDirectory {
        /// The unresolved path the stage asked for.
        path: String,
    },
    /// A `sourceProgram` file could not be read.
    UnreadableSource {
        /// The path that was tried.
        path: PathBuf,
        /// The operating-system error.
        error: String,
    },
    /// A parameter name is not in the fixed ABI.
    UnknownParameter(String),
    /// The same parameter name appears twice.
    DuplicateParameter(String),
}

impl fmt::Display for ShaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingStage(stage) => write!(formatter, "SoShaderProgram needs one {stage}"),
            Self::DuplicateStage(stage) => write!(formatter, "SoShaderProgram has two {stage}s"),
            Self::NotAStage(kind) => {
                write!(
                    formatter,
                    "SoShaderProgram.shaderObject: {kind} is not a shader stage"
                )
            }
            Self::NotAParameter(kind) => {
                write!(
                    formatter,
                    "shader parameter list: {kind} is not a SoShaderParameter"
                )
            }
            Self::MissingSource => write!(formatter, "SoShaderProgram has no WGSL source"),
            Self::MissingBaseDirectory { path } => write!(
                formatter,
                "sourceProgram {path:?} needs an explicit base directory; set the inline \
                 `source` field or compile with `compile_with_base`"
            ),
            Self::UnreadableSource { path, error } => {
                write!(
                    formatter,
                    "cannot read sourceProgram {}: {error}",
                    path.display()
                )
            }
            Self::UnknownParameter(name) => {
                write!(formatter, "unknown shader parameter {name:?} (fixed ABI)")
            }
            Self::DuplicateParameter(name) => {
                write!(formatter, "shader parameter {name:?} appears twice")
            }
        }
    }
}

impl std::error::Error for ShaderError {}

/// Map Coin's `SoDepthBuffer.function` ordinal onto the neutral [`DepthCompare`].
///
/// An ordinal outside the enum falls back to `Less`, the renderer's default, rather than
/// silently disabling the test.
fn depth_compare(function: i32) -> DepthCompare {
    match function {
        depth_function::NEVER => DepthCompare::Never,
        depth_function::EQUAL => DepthCompare::Equal,
        depth_function::LEQUAL => DepthCompare::LessEqual,
        depth_function::GREATER => DepthCompare::Greater,
        depth_function::NOTEQUAL => DepthCompare::NotEqual,
        depth_function::GEQUAL => DepthCompare::GreaterEqual,
        depth_function::ALWAYS => DepthCompare::Always,
        // `LESS` and anything outside the enum: the renderer's default.
        _ => DepthCompare::Less,
    }
}

/// Emit one packet for `indices`, attaching geometry on every compile.
fn emit_mesh(
    state: &State,
    coordinates: &[[f32; 3]],
    tex_coords: &[[f32; 2]],
    indices: &[u32],
    topology: Topology,
    ctx: &mut Compiler,
) {
    let Some(geometry) = build_geometry(coordinates, tex_coords, indices, topology) else {
        return;
    };
    let mesh_id = MeshId(ctx.next_mesh);
    ctx.next_mesh += 1;
    let generation = Generation(fingerprint(&geometry));
    ctx.packets.push(DrawPacket {
        mesh_id,
        generation,
        topology,
        geometry: Some(geometry),
        state: state.snapshot(),
    });
}

/// Fan-triangulate an `SoFaceSet`: each `num_vertices[i]` consumes that many
/// consecutive coordinates. Counts below 3 are skipped (they still advance the
/// cursor, as Coin's per-face consumption does); a face whose coordinates are
/// missing is skipped as well.
fn triangulate_faces(num_vertices: &[u32], coordinate_count: usize) -> Vec<u32> {
    let mut indices = Vec::new();
    let mut start = 0usize;
    for &count in num_vertices {
        let count = count as usize;
        let end = start.saturating_add(count);
        if count >= 3 && end <= coordinate_count {
            for corner in 1..count - 1 {
                indices.push(start as u32);
                indices.push((start + corner) as u32);
                indices.push((start + corner + 1) as u32);
            }
        }
        start = end;
    }
    indices
}

/// A content fingerprint of `geometry`, used as its [`Generation`].
///
/// The renderer's upload cache is keyed on `(mesh_id, generation)` and treats an equal
/// pair as "the buffers are still valid". Mesh ids are positional, so a producer that
/// changes packet order or count (a depth-sorted overlay, a hidden label) would
/// otherwise hand a new mesh an old upload. Hashing the bytes makes the pair honest
/// without comparing the buffers on the render thread.
fn fingerprint(geometry: &Geometry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for vertex in &geometry.vertices {
        for value in vertex
            .position
            .iter()
            .chain(&vertex.normal)
            .chain(&vertex.uv)
        {
            value.to_bits().hash(&mut hasher);
        }
        vertex.part_idx.hash(&mut hasher);
    }
    geometry.indices.hash(&mut hasher);
    hasher.finish()
}

/// Flatten coordinates + indices into flat-shaded vertices (one normal per face).
fn build_geometry(
    coordinates: &[[f32; 3]],
    tex_coords: &[[f32; 2]],
    indices: &[u32],
    topology: Topology,
) -> Option<Geometry> {
    if indices.is_empty() {
        return None;
    }
    let mut vertices = Vec::new();
    let mut indices_out = Vec::new();

    let step = match topology {
        Topology::Triangles => 3,
        Topology::Lines => 2,
        Topology::Points => 1,
    };
    for face in indices.chunks_exact(step) {
        let normal = if step >= 2 {
            let a = coordinates.get(face[0] as usize)?;
            let b = coordinates.get(face[1] as usize)?;
            let c = if step == 3 {
                coordinates.get(face[2] as usize)?
            } else {
                b
            };
            face_normal(a, b, c)
        } else {
            [0.0, 0.0, 1.0]
        };
        for &index in face {
            let position = *coordinates.get(index as usize)?;
            let uv = tex_coords
                .get(index as usize)
                .copied()
                .unwrap_or([0.0, 0.0]);
            indices_out.push(vertices.len() as u32);
            vertices.push(Vertex {
                position,
                normal,
                uv,
                part_idx: 0,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
    }

    Some(Geometry {
        vertices,
        indices: indices_out,
    })
}

/// The unit normal of triangle `abc` (the cross product, normalised).
fn face_normal(a: &[f32; 3], b: &[f32; 3], c: &[f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > f32::EPSILON {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINTS: [[f32; 3]; 3] = [[0.0, 0.5, 0.0], [-0.5, -0.5, 0.0], [0.5, -0.5, 0.0]];

    fn mesh() -> Node {
        Node {
            kind: NodeKind::Mesh {
                indices: vec![0, 1, 2],
                normal_indices: vec![],
                topology: Topology::Triangles,
            },
            children: vec![],
        }
    }

    fn separator(children: Vec<Node>) -> Node {
        Node {
            kind: NodeKind::Separator,
            children,
        }
    }

    #[test]
    fn compiles_a_triangle_to_one_packet() {
        let scene = Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1);
        let packet = &queue.packets[0];
        assert_eq!(packet.topology, Topology::Triangles);
        assert_eq!(packet.state.color, [1.0, 1.0, 1.0, 1.0]);
        let geometry = packet.geometry.as_ref().unwrap();
        assert_eq!(geometry.vertices.len(), 3);
        assert_eq!(geometry.indices, vec![0, 1, 2]);
        assert_eq!(geometry.vertices[0].normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn the_generation_fingerprints_the_geometry() {
        let scene = |points: Vec<[f32; 3]>| Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(points),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let (first, _) = compile(&scene(POINTS.to_vec()));
        let (again, _) = compile(&scene(POINTS.to_vec()));
        assert_eq!(
            first.packets[0].generation, again.packets[0].generation,
            "identical geometry reuses the upload"
        );

        // Mesh ids are positional, so a producer that reorders or hides packets can
        // reuse an id for different geometry; the generation must change with the bytes.
        let mut moved = POINTS;
        moved[0][1] = 0.25;
        let (changed, _) = compile(&scene(moved.to_vec()));
        assert_eq!(first.packets[0].mesh_id, changed.packets[0].mesh_id);
        assert_ne!(
            first.packets[0].generation, changed.packets[0].generation,
            "different geometry must miss the upload cache"
        );
    }

    #[test]
    fn material_scopes_to_following_siblings_in_its_separator() {
        let scene = Scene {
            root: separator(vec![
                // A red material applies to the mesh that follows it...
                separator(vec![
                    Node {
                        kind: NodeKind::Material(vec![[1.0, 0.0, 0.0, 1.0]]),
                        children: vec![],
                    },
                    Node {
                        kind: NodeKind::Coordinate3(POINTS.to_vec()),
                        children: vec![],
                    },
                    mesh(),
                ]),
                // ...but not across the separator boundary.
                separator(vec![
                    Node {
                        kind: NodeKind::Coordinate3(POINTS.to_vec()),
                        children: vec![],
                    },
                    mesh(),
                ]),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.packets[0].state.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(queue.packets[1].state.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn material_list_binds_its_first_entry() {
        let scene = Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Material(vec![[0.1, 0.2, 0.3, 1.0], [1.0, 0.0, 0.0, 1.0]]),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.packets[0].state.color, [0.1, 0.2, 0.3, 1.0]);
    }

    /// A `SoSwitch` draws only the child(ren) its `whichChild` selects: an index
    /// picks one, `SO_SWITCH_NONE` draws nothing, `SO_SWITCH_ALL` draws all. It
    /// inherits the sibling coordinates (it is a group, not a separator).
    fn switch_scene(which: i32) -> Scene {
        Scene {
            root: separator(vec![
                Node {
                    kind: NodeKind::Coordinate3(POINTS.to_vec()),
                    children: vec![],
                },
                Node {
                    kind: NodeKind::Switch { which },
                    children: vec![mesh(), mesh()],
                },
            ]),
        }
    }

    #[test]
    fn switch_sentinels_match_coin() {
        assert_eq!(crate::node::SO_SWITCH_NONE, -1);
        assert_eq!(crate::node::SO_SWITCH_INHERIT, -2);
        assert_eq!(crate::node::SO_SWITCH_ALL, -3);
    }

    #[test]
    fn switch_honours_which_child_sentinels() {
        // A non-negative index selects exactly one child.
        assert_eq!(compile(&switch_scene(0)).0.len(), 1);
        assert_eq!(compile(&switch_scene(1)).0.len(), 1);
        // `SO_SWITCH_NONE` draws nothing; `SO_SWITCH_ALL` (Coin's -3) draws every
        // child. The addon hard-codes -3 twice, so this is the regression guard.
        assert_eq!(
            compile(&switch_scene(crate::node::SO_SWITCH_NONE)).0.len(),
            0
        );
        assert_eq!(compile(&switch_scene(-3)).0.len(), 2);
        assert_eq!(
            compile(&switch_scene(crate::node::SO_SWITCH_ALL)).0.len(),
            2
        );
    }

    fn node(kind: NodeKind, children: Vec<Node>) -> Node {
        Node { kind, children }
    }

    /// A parent switch at `parent_which` whose second child is a nested switch at
    /// `inner_which` with two drawable children.
    fn nested_switch_scene(parent_which: i32, inner_which: i32) -> Scene {
        Scene {
            root: separator(vec![
                node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                node(
                    NodeKind::Switch {
                        which: parent_which,
                    },
                    vec![
                        mesh(),
                        node(
                            NodeKind::Switch { which: inner_which },
                            vec![mesh(), mesh()],
                        ),
                    ],
                ),
            ]),
        }
    }

    #[test]
    fn nested_switch_inherit_follows_the_parent_selection() {
        use crate::node::SO_SWITCH_INHERIT;

        // The parent selects its index 1 (the nested switch); the nested switch
        // inherits that selection, so it draws its own index 1 — one packet.
        assert_eq!(
            compile(&nested_switch_scene(1, SO_SWITCH_INHERIT)).0.len(),
            1
        );
        // The parent draws all children; the nested switch inherits `ALL` and so
        // contributes both of its children: 1 (the mesh) + 2 = 3 packets.
        assert_eq!(
            compile(&nested_switch_scene(
                crate::node::SO_SWITCH_ALL,
                SO_SWITCH_INHERIT
            ))
            .0
            .len(),
            3
        );
        // The parent selects index 0 with two children, so only the first parent
        // child (a mesh) is drawn and the nested switch is never reached.
        assert_eq!(
            compile(&nested_switch_scene(0, SO_SWITCH_INHERIT)).0.len(),
            1
        );
    }

    #[test]
    fn a_root_switch_with_inherit_behaves_as_all() {
        // No enclosing switch: the traversal starts at `SO_SWITCH_ALL`, matching
        // Coin's default switch element (plan §4.1: "no parent → all").
        let scene = Scene {
            root: separator(vec![
                node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                node(
                    NodeKind::Switch {
                        which: crate::node::SO_SWITCH_INHERIT,
                    },
                    vec![mesh(), mesh()],
                ),
            ]),
        };
        assert_eq!(compile(&scene).0.len(), 2);
    }

    const QUAD: [[f32; 3]; 4] = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];

    fn face_set_scene(points: Vec<[f32; 3]>, num_vertices: Vec<u32>) -> Scene {
        Scene {
            root: separator(vec![
                node(NodeKind::Coordinate3(points), vec![]),
                node(NodeKind::FaceSet { num_vertices }, vec![]),
            ]),
        }
    }

    #[test]
    fn face_set_fan_triangulates_a_quad() {
        let (queue, _) = compile(&face_set_scene(QUAD.to_vec(), vec![4]));
        assert_eq!(queue.len(), 1);
        let packet = &queue.packets[0];
        assert_eq!(packet.topology, Topology::Triangles);
        let geometry = packet.geometry.as_ref().expect("geometry");
        // A quad fan is (0,1,2) + (0,2,3). Flat shading unrolls each triangle into
        // fresh vertices, so the positions spell out the two fans.
        assert_eq!(geometry.vertices.len(), 6);
        let positions: Vec<_> = geometry.vertices.iter().map(|v| v.position).collect();
        assert_eq!(
            positions,
            vec![QUAD[0], QUAD[1], QUAD[2], QUAD[0], QUAD[2], QUAD[3]]
        );
        assert_eq!(geometry.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(geometry.vertices[0].normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn face_set_consumes_consecutive_polygons() {
        // A pentagon fans to three triangles (9 indices).
        let mut pentagon = QUAD.to_vec();
        pentagon.push([0.0, 1.5, 0.0]);
        let (queue, _) = compile(&face_set_scene(pentagon, vec![5]));
        assert_eq!(queue.packets[0].geometry.as_ref().unwrap().indices.len(), 9);

        // Two faces consume consecutive coordinate runs: a quad then a triangle.
        let mut points = QUAD.to_vec();
        points.extend_from_slice(&POINTS);
        let (queue, _) = compile(&face_set_scene(points, vec![4, 3]));
        let geometry = queue.packets[0].geometry.as_ref().unwrap();
        assert_eq!(geometry.indices.len(), 9);
        assert_eq!(geometry.vertices.len(), 9);
    }

    #[test]
    fn face_set_skips_counts_below_three_but_still_advances() {
        // A 2-vertex "face" is skipped; the following quad starts at coordinate 2.
        let mut points = vec![[9.0, 9.0, 9.0], [8.0, 8.0, 8.0]];
        points.extend_from_slice(&QUAD);
        let (queue, _) = compile(&face_set_scene(points, vec![2, 4]));
        let geometry = queue.packets[0].geometry.as_ref().unwrap();
        assert_eq!(geometry.indices.len(), 6);
        assert_eq!(geometry.vertices[0].position, QUAD[0]);
    }

    #[test]
    fn state_nodes_apply_without_dropping_the_subtree() {
        use crate::node::{depth_function, light_model, pick_style};
        let scene = Scene {
            root: separator(vec![
                node(
                    NodeKind::PickStyle {
                        style: pick_style::UNPICKABLE,
                    },
                    vec![],
                ),
                node(
                    NodeKind::LightModel {
                        model: light_model::BASE_COLOR,
                    },
                    vec![],
                ),
                node(
                    NodeKind::DepthBuffer {
                        test: true,
                        write: false,
                        function: depth_function::LESS,
                    },
                    vec![],
                ),
                node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1, "state nodes must not drop the subtree");

        let state = &queue.packets[0].state;
        assert!(!state.pickable, "SoPickStyle.UNPICKABLE");
        assert_eq!(
            state.depth,
            DepthState {
                test: true,
                write: false,
                compare: DepthCompare::Less,
            }
        );
        // SoLightModel is deliberately inert under the fixed light model.
        assert!(state.program.is_none());
    }

    #[test]
    fn the_depth_state_maps_every_compare_mode_and_disabling_the_test_compares_always() {
        use crate::node::{depth_function, pick_style};
        let modes = [
            (depth_function::NEVER, DepthCompare::Never),
            (depth_function::LESS, DepthCompare::Less),
            (depth_function::EQUAL, DepthCompare::Equal),
            (depth_function::LEQUAL, DepthCompare::LessEqual),
            (depth_function::GREATER, DepthCompare::Greater),
            (depth_function::NOTEQUAL, DepthCompare::NotEqual),
            (depth_function::GEQUAL, DepthCompare::GreaterEqual),
            (depth_function::ALWAYS, DepthCompare::Always),
        ];
        for (ordinal, expected) in modes {
            let scene = Scene {
                root: separator(vec![
                    node(
                        NodeKind::DepthBuffer {
                            test: true,
                            write: true,
                            function: ordinal,
                        },
                        vec![],
                    ),
                    node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                    mesh(),
                ]),
            };
            let (queue, _) = compile(&scene);
            let depth = queue.packets[0].state.depth;
            assert_eq!(depth.compare, expected, "function {ordinal}");
            assert_eq!(depth.effective_compare(), expected);
            assert!(depth.effective_write());
        }

        // `test FALSE` forces `Always`; `write` is independent.
        let scene = Scene {
            root: separator(vec![
                node(
                    NodeKind::DepthBuffer {
                        test: false,
                        write: false,
                        function: depth_function::GREATER,
                    },
                    vec![],
                ),
                node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        let depth = queue.packets[0].state.depth;
        assert_eq!(depth.compare, DepthCompare::Greater);
        assert_eq!(depth.effective_compare(), DepthCompare::Always);
        assert!(!depth.effective_write());

        // A later `SoPickStyle.SHAPE` restores pickability inside the separator.
        let scene = Scene {
            root: separator(vec![
                node(
                    NodeKind::PickStyle {
                        style: pick_style::UNPICKABLE,
                    },
                    vec![],
                ),
                node(
                    NodeKind::PickStyle {
                        style: pick_style::SHAPE,
                    },
                    vec![],
                ),
                node(NodeKind::Coordinate3(POINTS.to_vec()), vec![]),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert!(queue.packets[0].state.pickable);
    }

    #[test]
    fn a_program_packet_defaults_to_depth_write_off_and_no_pick() {
        let program = program_node(vec![
            stage_node(ShaderStageKind::Vertex, Some(WGSL), vec![]),
            stage_node(
                ShaderStageKind::Fragment,
                Some(""),
                vec![float_parameter("base_spacing", 2.0)],
            ),
        ]);
        let scene = Scene {
            root: separator(vec![
                coords(),
                program,
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        let state = &queue.packets[0].state;
        assert!(state.program.is_some());
        assert_eq!(state.depth, DepthState::PROGRAM);
        assert!(!state.pickable);
    }

    #[test]
    fn a_navicube_node_becomes_a_directive_not_a_packet() {
        use crate::{NaviCubeDirective, Rect};
        let directive = NaviCubeDirective {
            rect: Rect::new(10, 10, 50, 50),
            orientation: None,
            hilite: None,
            orthographic: true,
        };
        let scene = Scene {
            root: separator(vec![Node {
                kind: NodeKind::NaviCube(directive),
                children: vec![],
            }]),
        };
        let (queue, directives) = compile(&scene);
        assert!(queue.is_empty(), "a navicube node draws nothing");
        assert_eq!(
            directives.navicubes().copied().collect::<Vec<_>>(),
            vec![directive]
        );
    }

    #[test]
    fn event_callback_nodes_become_directives_and_honour_switch_state() {
        let callback = |id| Node {
            kind: NodeKind::EventCallback(id),
            children: vec![],
        };
        let switch = |which, children| Node {
            kind: NodeKind::Switch { which },
            children,
        };
        let scene = Scene {
            root: separator(vec![
                callback(1),
                switch(1, vec![callback(2), callback(3)]),
                switch(crate::node::SO_SWITCH_NONE, vec![callback(4)]),
            ]),
        };
        let (queue, directives) = compile(&scene);
        assert!(queue.is_empty(), "event callbacks draw nothing");
        // Only the callback outside the switch and the active (index 1) child.
        assert_eq!(directives.event_callbacks().collect::<Vec<_>>(), vec![1, 3]);
    }

    // ------------------------------------------------------------------
    // Shader programs (G3)
    // ------------------------------------------------------------------

    const WGSL: &str = "@fragment fn fs_main() {}";

    fn stage_node(kind: ShaderStageKind, source: Option<&str>, parameters: Vec<Node>) -> Node {
        Node {
            kind: NodeKind::ShaderStage(ShaderStage {
                kind,
                source: source.map(str::to_owned),
                source_program: None,
            }),
            children: parameters,
        }
    }

    fn float_parameter(name: &str, value: f32) -> Node {
        Node {
            kind: NodeKind::ShaderParameter(ShaderParameter::Float {
                name: name.to_owned(),
                value,
            }),
            children: vec![],
        }
    }

    fn program_node(children: Vec<Node>) -> Node {
        Node {
            kind: NodeKind::ShaderProgram,
            children,
        }
    }

    fn coords() -> Node {
        Node {
            kind: NodeKind::Coordinate3(POINTS.to_vec()),
            children: vec![],
        }
    }

    #[test]
    fn a_program_binds_to_the_following_siblings_only() {
        let scene = Scene {
            root: separator(vec![
                coords(),
                mesh(),
                program_node(vec![
                    stage_node(ShaderStageKind::Fragment, Some(WGSL), vec![]),
                    stage_node(
                        ShaderStageKind::Vertex,
                        Some(""),
                        vec![float_parameter("base_spacing", 2.5)],
                    ),
                ]),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 2, "a program node draws nothing");
        assert!(
            queue.packets[0].state.program.is_none(),
            "shapes before the program keep no program"
        );
        let program = queue.packets[1]
            .state
            .program
            .as_ref()
            .expect("the following shape carries the program");
        assert_eq!(program.source(), WGSL);
        assert_eq!(program.parameters().base_spacing, 2.5);
    }

    #[test]
    fn stage_and_parameter_values_never_emit_packets() {
        let scene = Scene {
            root: separator(vec![
                coords(),
                program_node(vec![
                    stage_node(
                        ShaderStageKind::Vertex,
                        Some(WGSL),
                        vec![float_parameter("minor_alpha", 0.2)],
                    ),
                    stage_node(ShaderStageKind::Fragment, Some(""), vec![]),
                ]),
                // Stray values at sibling level are accepted and ignored.
                stage_node(ShaderStageKind::Fragment, None, vec![]),
                float_parameter("major_alpha", 0.5),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1, "only the mesh draws");
        let parameters = queue.packets[0]
            .state
            .program
            .as_ref()
            .unwrap()
            .parameters();
        assert_eq!(parameters.minor_alpha, 0.2);
    }

    #[test]
    fn a_separator_scopes_a_program() {
        let shader_program = || {
            program_node(vec![
                stage_node(ShaderStageKind::Vertex, Some(WGSL), vec![]),
                stage_node(ShaderStageKind::Fragment, Some(""), vec![]),
            ])
        };
        // Outer program, then an inner separator whose own program must not leak out
        // of it; the inner shape binds the inner program, and the shape after the
        // separator still sees the outer one.
        let scene = Scene {
            root: separator(vec![
                shader_program(),
                coords(),
                separator(vec![
                    program_node(vec![
                        stage_node(ShaderStageKind::Vertex, Some("@fragment fn f() {}"), vec![]),
                        stage_node(ShaderStageKind::Fragment, Some(""), vec![]),
                    ]),
                    coords(),
                    mesh(),
                ]),
                mesh(),
            ]),
        };
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue.packets[0].state.program.as_ref().unwrap().source(),
            "@fragment fn f() {}",
            "the inner program shadows the outer one inside its list"
        );
        assert_eq!(
            queue.packets[1].state.program.as_ref().unwrap().source(),
            WGSL,
            "the inner separator does not leak its program out"
        );
    }

    #[test]
    fn duplicate_and_missing_stages_are_reported() {
        let stages = vec![
            stage_node(ShaderStageKind::Vertex, Some(WGSL), vec![]),
            stage_node(ShaderStageKind::Vertex, Some(WGSL), vec![]),
        ];
        let scene = Scene {
            root: separator(vec![program_node(stages)]),
        };
        let error = compile_with_base(&scene, &Viewport::default(), None).unwrap_err();
        assert_eq!(error, ShaderError::DuplicateStage("SoVertexShader"));

        let scene = Scene {
            root: separator(vec![program_node(vec![stage_node(
                ShaderStageKind::Fragment,
                Some(WGSL),
                vec![],
            )])]),
        };
        let error = compile_with_base(&scene, &Viewport::default(), None).unwrap_err();
        assert_eq!(error, ShaderError::MissingStage("SoVertexShader"));
    }

    #[test]
    fn unknown_and_duplicate_parameters_are_reported() {
        let scene_with = |name: &str| Scene {
            root: separator(vec![program_node(vec![
                stage_node(ShaderStageKind::Vertex, Some(WGSL), vec![]),
                stage_node(
                    ShaderStageKind::Fragment,
                    Some(""),
                    vec![float_parameter(name, 1.0)],
                ),
            ])]),
        };
        assert_eq!(
            compile_with_base(&scene_with("fog_density"), &Viewport::default(), None).unwrap_err(),
            ShaderError::UnknownParameter("fog_density".to_owned())
        );

        let scene = Scene {
            root: separator(vec![program_node(vec![
                stage_node(
                    ShaderStageKind::Vertex,
                    Some(WGSL),
                    vec![float_parameter("minor_alpha", 0.1)],
                ),
                stage_node(
                    ShaderStageKind::Fragment,
                    Some(""),
                    vec![float_parameter("minor_alpha", 0.2)],
                ),
            ])]),
        };
        assert_eq!(
            compile_with_base(&scene, &Viewport::default(), None).unwrap_err(),
            ShaderError::DuplicateParameter("minor_alpha".to_owned())
        );
    }

    #[test]
    fn source_program_needs_an_explicit_base_and_resolves_against_it() {
        let dir = std::env::temp_dir().join(format!("ferrocad-shader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("grid.wgsl"), WGSL).unwrap();

        let path_only_stage = || Node {
            kind: NodeKind::ShaderStage(ShaderStage {
                kind: ShaderStageKind::Fragment,
                source: None,
                source_program: Some("grid.wgsl".to_owned()),
            }),
            children: vec![],
        };
        let scene = Scene {
            root: separator(vec![
                coords(),
                program_node(vec![
                    stage_node(ShaderStageKind::Vertex, Some(""), vec![]),
                    path_only_stage(),
                ]),
                mesh(),
            ]),
        };

        let error = compile_with_base(&scene, &Viewport::default(), None).unwrap_err();
        assert_eq!(
            error,
            ShaderError::MissingBaseDirectory {
                path: "grid.wgsl".to_owned()
            }
        );

        let (queue, _) = compile_with_base(&scene, &Viewport::default(), Some(&dir)).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.packets[0].state.program.as_ref().unwrap().source(),
            WGSL
        );

        // An inline source is authoritative and never touches the path.
        let inline = Node {
            kind: NodeKind::ShaderStage(ShaderStage {
                kind: ShaderStageKind::Fragment,
                source: Some(WGSL.to_owned()),
                source_program: Some("missing.wgsl".to_owned()),
            }),
            children: vec![],
        };
        let scene = Scene {
            root: separator(vec![
                coords(),
                program_node(vec![
                    stage_node(ShaderStageKind::Vertex, Some(""), vec![]),
                    inline,
                ]),
                mesh(),
            ]),
        };
        let (queue, _) = compile_with_base(&scene, &Viewport::default(), None).unwrap();
        assert_eq!(
            queue.packets[0].state.program.as_ref().unwrap().source(),
            WGSL
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
