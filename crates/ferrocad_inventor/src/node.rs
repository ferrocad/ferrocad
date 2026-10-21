//! The scene-graph node AST: the vocabulary a scene is built from.
//!
//! This is the small, closed set of node kinds the core understands today. It is
//! intentionally data-only — behaviour (traversal, state accumulation, compilation)
//! lives in [`state`](crate::state) and [`compile`](crate::compile), and the
//! Python-facing constructors live in `ferrocad_inventor_py`.

use std::sync::Arc;

use ferrocad_types::Placement;

use crate::directive::NaviCubeDirective;
use crate::geometry::{Texture2, Topology};

/// A scene: a root node. The compiler walks this to produce a render queue.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// The root of the scene graph.
    pub root: Node,
}

/// `SoSwitch.whichChild` sentinel: traverse no child (Coin `SO_SWITCH_NONE`).
pub const SO_SWITCH_NONE: i32 = -1;
/// `SoSwitch.whichChild` sentinel: use the enclosing switch's selection
/// (Coin `SO_SWITCH_INHERIT`). With no enclosing switch the switch behaves as
/// [`SO_SWITCH_ALL`].
pub const SO_SWITCH_INHERIT: i32 = -2;
/// `SoSwitch.whichChild` sentinel: traverse every child (Coin `SO_SWITCH_ALL`).
pub const SO_SWITCH_ALL: i32 = -3;

/// `SoPickStyle.style` values (Coin's `SoPickStyle::Style`).
///
/// `UNPICKABLE` is the only one with semantics: it clears the snapshot's `pickable`
/// bit. `BOUNDING_BOX` is accepted but picks like `SHAPE`. The numeric values follow
/// Coin's declaration order but are not verified against Coin's headers (none are
/// vendored here).
pub mod pick_style {
    /// Pick the shape itself (Coin `SHAPE`).
    pub const SHAPE: i32 = 0;
    /// Pick the bounding box (Coin `BOUNDING_BOX`); not modelled.
    pub const BOUNDING_BOX: i32 = 1;
    /// Skip the shape when picking (Coin `UNPICKABLE`).
    pub const UNPICKABLE: i32 = 2;
}

/// `SoLightModel.model` values (Coin's `SoLightModel::Model`).
///
/// Recorded only: under a shader program the shader owns shading, and without one
/// FerroCAD has a single light model.
pub mod light_model {
    /// Unlit, constant colour (Coin `BASE_COLOR`).
    pub const BASE_COLOR: i32 = 0;
    /// The default lit model (Coin `PHONG`).
    pub const PHONG: i32 = 1;
}

/// `SoDepthBuffer.function` compare modes (Coin's
/// `SoDepthBuffer::DepthWriteFunction`). The compiler maps them onto
/// [`ferrocad_graphics::DepthCompare`], which the renderer's pipeline selection uses.
pub mod depth_function {
    /// `NEVER`.
    pub const NEVER: i32 = 0;
    /// `LESS` (the renderer's current opaque comparison).
    pub const LESS: i32 = 1;
    /// `EQUAL`.
    pub const EQUAL: i32 = 2;
    /// `LEQUAL`.
    pub const LEQUAL: i32 = 3;
    /// `GREATER`.
    pub const GREATER: i32 = 4;
    /// `NOTEQUAL`.
    pub const NOTEQUAL: i32 = 5;
    /// `GEQUAL`.
    pub const GEQUAL: i32 = 6;
    /// `ALWAYS`.
    pub const ALWAYS: i32 = 7;
}

/// The stage a shader node declares (`SoVertexShader` vs. `SoFragmentShader`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShaderStageKind {
    /// `SoVertexShader`.
    #[default]
    Vertex,
    /// `SoFragmentShader`.
    Fragment,
}

impl ShaderStageKind {
    /// The Coin class name for this stage.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Vertex => "SoVertexShader",
            Self::Fragment => "SoFragmentShader",
        }
    }
}

/// A shader stage value (Coin's `SoVertexShader`/`SoFragmentShader`).
///
/// Coin's `sourceProgram` is a **file path**, resolved at extraction time against an
/// explicit base directory in [`compile_with_base`](crate::compile::compile_with_base).
/// `source` is a **FerroCAD extension**: inline WGSL, authoritative when it is set, and
/// the only form that is portable (a path is environment-dependent).
///
/// WGSL has no per-stage modules: the program's module is the concatenation of the
/// non-empty stage `source`s in (vertex, fragment) order, so a single-file shader may
/// live in either stage and leave the other empty. The Coin stage split is therefore
/// compatibility-only under the fixed ABI.
///
/// The stage's `parameter` (`SoMFNode`) lives in the node's [`Node::children`]; the
/// stage itself is a value and never emits a draw packet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderStage {
    /// Which stage this is.
    pub kind: ShaderStageKind,
    /// Inline WGSL (FerroCAD extension); authoritative when non-empty.
    pub source: Option<String>,
    /// Coin's `sourceProgram`: a file path, compatibility-only.
    pub source_program: Option<String>,
}

impl ShaderStage {
    /// An empty vertex stage.
    pub fn vertex() -> Self {
        Self {
            kind: ShaderStageKind::Vertex,
            ..Self::default()
        }
    }

    /// An empty fragment stage.
    pub fn fragment() -> Self {
        Self {
            kind: ShaderStageKind::Fragment,
            ..Self::default()
        }
    }
}

/// One `SoShaderParameter*` value, a leaf that never emits a draw packet.
///
/// The compiler maps these onto [`ProgramParameters`](ferrocad_graphics::ProgramParameters)
/// by `name`; an unknown name is an extraction error.
#[derive(Clone, Debug, PartialEq)]
pub enum ShaderParameter {
    /// `SoShaderParameter1f`.
    Float {
        /// The ABI slot name.
        name: String,
        /// The value.
        value: f32,
    },
    /// `SoShaderParameter3f`.
    Vec3 {
        /// The ABI slot name.
        name: String,
        /// The value.
        value: [f32; 3],
    },
    /// `SoShaderParameterArray1f` (`SoMFFloat`).
    Floats {
        /// The ABI slot name.
        name: String,
        /// The values.
        values: Vec<f32>,
    },
}

impl ShaderParameter {
    /// The ABI slot name.
    pub fn name(&self) -> &str {
        match self {
            Self::Float { name, .. } | Self::Vec3 { name, .. } | Self::Floats { name, .. } => name,
        }
    }

    /// The Coin class name for this parameter shape.
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::Float { .. } => "SoShaderParameter1f",
            Self::Vec3 { .. } => "SoShaderParameter3f",
            Self::Floats { .. } => "SoShaderParameterArray1f",
        }
    }
}

/// The kinds of node the core recognises.
///
/// This grows as nodes are ported; [`NodeKind::Custom`] carries the name of a
/// FreeCAD/Part/Mesh node class (e.g. `SoBrepEdgeSet`) whose behaviour is provided
/// by a registered extension.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum NodeKind {
    /// Groups children and isolates state changes (`SoSeparator`).
    #[default]
    Separator,
    /// Draws only the selected child (`SoSwitch`).
    Switch {
        /// The active child index, or a `SO_SWITCH_*` sentinel (negative).
        which: i32,
    },
    /// Vertex positions for the following shape (`SoCoordinate3`).
    Coordinate3(Vec<[f32; 3]>),
    /// Texture coordinates for the following shape (`SoTextureCoordinate2`).
    TextureCoordinate2(Vec<[f32; 2]>),
    /// The texture for the following shapes (`SoTexture2`).
    Texture2(Arc<Texture2>),
    /// Vertex normals for the following shape (`SoNormal`).
    ///
    /// Stored for round-tripping: the compiler flat-shades today and does not
    /// consume these yet; Phase 3 (split-vertex unrolling) will.
    Normal(Vec<[f32; 3]>),
    /// A triangle/line/point set drawn from the current `Coordinate3`.
    Mesh {
        /// Indices into the enclosing `Coordinate3` (`SoIndexedShape.coordIndex`).
        indices: Vec<u32>,
        /// Per-index normals (`SoIndexedFaceSet.normalIndex`); empty when absent.
        /// Unused by the compiler today (see [`NodeKind::Normal`]).
        normal_indices: Vec<u32>,
        /// How the indices are assembled.
        topology: Topology,
    },
    /// A non-indexed polygon set (`SoFaceSet`).
    ///
    /// Each entry of `num_vertices` is the vertex count of one polygon; the
    /// polygons consume consecutive entries of the enclosing `Coordinate3`, so
    /// face 0 uses coordinates `0..n0`, face 1 the next `n1`, and so on. The
    /// compiler fan-triangulates each polygon. `startIndex` is not modelled yet
    /// (always 0).
    FaceSet {
        /// Per-face vertex counts (`SoFaceSet.numVertices`).
        num_vertices: Vec<u32>,
    },
    /// A pickability hint (`SoPickStyle`).
    ///
    /// The compiler traverses past it without dropping the subtree. `UNPICKABLE` clears
    /// the snapshot's `pickable` bit so both pick paths miss the following shapes;
    /// `SHAPE` restores it. `BOUNDING_BOX` is not modelled (it picks like `SHAPE`),
    /// because nothing needs box picking (plan §4.4). `style` is one of [`pick_style`].
    PickStyle {
        /// The requested style ([`pick_style::SHAPE`] by default).
        style: i32,
    },
    /// A shading-model hint (`SoLightModel`) — **recorded, not applied**.
    ///
    /// Deliberately inert: under a shader program the shader owns shading, and without a
    /// program FerroCAD has one fixed light model (`scene.wgsl`), so `BASE_COLOR` would
    /// be a dead pipeline (plan §4.4). `model` is one of [`light_model`].
    LightModel {
        /// The requested model ([`light_model::PHONG`] by default).
        model: i32,
    },
    /// Depth-buffer state (`SoDepthBuffer`).
    ///
    /// The compiler freezes it into the snapshot's `depth` state, which a renderer's
    /// pipeline selection honours: `test == false` compares `Always`, and `write ==
    /// false` leaves the depth buffer untouched. `function` is one of
    /// [`depth_function`]. `test` defaults to true, `write` to true and `function` to
    /// [`depth_function::LESS`], matching the renderer's opaque pipeline. Coin's default
    /// `function` is not verified (Coin is not vendored); `LESS` is the renderer's
    /// historical choice.
    DepthBuffer {
        /// Whether the depth test is enabled.
        test: bool,
        /// Whether the depth buffer is written.
        write: bool,
        /// The compare mode ([`depth_function`]).
        function: i32,
    },
    /// The colours for the following shapes (`SoMaterial`/`SoBaseColor`).
    ///
    /// A list (Coin's `SoMFColor.diffuseColor`); the compiler binds the first
    /// entry as the draw colour.
    Material(Vec<[f32; 4]>),
    /// Sets the selection identity ([`PickId`](crate::PickId)) of the following
    /// shapes — the `SoFCSelection`/`SoPickStyle` role. Used to give overlay parts
    /// (e.g. the navigation cube's faces) distinguishable pick ids.
    PickId(u32),
    /// A transform applied to the following siblings (`SoTransform`).
    Transform(Placement),
    /// An event-handling node (`SoEventCallback`).
    ///
    /// Carries an opaque id; the Python callables registered on the corresponding
    /// `SoEventCallback` live on the facade side, keyed by this id (the core stays
    /// Python-free). The compiler treats it as a leaf.
    EventCallback(u64),
    /// A navigation-cube overlay node (`SoNaviCube`).
    ///
    /// It draws nothing into the scene: the compiler lifts it into a
    /// [`Directive::NaviCube`](crate::Directive) that the viewport hosts as an overlay
    /// (see `docs/viewport/directives.md`).
    NaviCube(NaviCubeDirective),
    /// A shader program (`SoShaderProgram`) — draws nothing itself.
    ///
    /// Its [`children`](Node::children) are Coin's `shaderObject` (`SoMFNode`): the
    /// stage nodes it links. The compiler resolves them into a
    /// [`ShaderProgram`](ferrocad_graphics::ShaderProgram) and binds it to the shapes
    /// that **follow** the program in the same sibling list (a `Separator` scopes it,
    /// like every other inherited state).
    ShaderProgram,
    /// A shader stage value (`SoVertexShader`/`SoFragmentShader`); never draws.
    ShaderStage(ShaderStage),
    /// A shader parameter value (`SoShaderParameter*`); never draws.
    ShaderParameter(ShaderParameter),
    /// A node class whose behaviour is registered from outside the core.
    Custom(String),
}

/// A node: a kind plus its ordered children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// What this node is.
    pub kind: NodeKind,
    /// Child nodes, in scene order.
    pub children: Vec<Node>,
}
