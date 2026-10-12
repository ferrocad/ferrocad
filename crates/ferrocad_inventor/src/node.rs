//! The scene-graph node AST: the vocabulary a scene is built from.
//!
//! This is the small, closed set of node kinds the core understands today. It is
//! intentionally data-only — behaviour (traversal, state accumulation, compilation)
//! lives in [`state`](crate::state) and [`compile`](crate::compile), and the
//! Python-facing constructors live in `ferrocad_inventor_py`.

use ferrocad_types::Placement;

use crate::geometry::Topology;

/// A scene: a root node. The compiler walks this to produce a render queue.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// The root of the scene graph.
    pub root: Node,
}

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
        /// The index of the active child.
        which: usize,
    },
    /// Vertex positions for the following shape.
    Coordinate3(Vec<[f32; 3]>),
    /// A triangle/line/point set drawn from the current `Coordinate3`.
    Mesh {
        /// Indices into the enclosing `Coordinate3`.
        indices: Vec<u32>,
        /// How the indices are assembled.
        topology: Topology,
    },
    /// The colour for the following shapes (`SoMaterial`/`SoBaseColor`).
    Material([f32; 4]),
    /// A transform applied to the following siblings (`SoTransform`).
    Transform(Placement),
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
