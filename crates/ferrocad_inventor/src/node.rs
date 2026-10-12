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
/// `SoSwitch.whichChild` sentinel: traverse every child (Coin `SO_SWITCH_ALL`).
pub const SO_SWITCH_ALL: i32 = -2;

/// The kinds of node the core recognises.
///
/// This grows as nodes are ported; [`NodeKind::Custom`] carries the name of a
/// FreeCAD/Part/Mesh node class (e.g. `SoBrepEdgeSet`) whose behaviour is provided
/// by a registered extension.
#[derive(Clone, Debug, Default)]
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
    /// (see `docs/directives.md`).
    NaviCube(NaviCubeDirective),
    /// A node class whose behaviour is registered from outside the core.
    Custom(String),
}

/// A node: a kind plus its ordered children.
#[derive(Clone, Debug, Default)]
pub struct Node {
    /// What this node is.
    pub kind: NodeKind,
    /// Child nodes, in scene order.
    pub children: Vec<Node>,
}
