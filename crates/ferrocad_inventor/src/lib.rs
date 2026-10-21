//! # FerroCAD Inventor core (`ferrocad_inventor`)
//!
//! A pure-Rust reimplementation of the part of Coin3D that workbench code actually
//! uses — the *semantics*, not the renderer. It is deliberately headless and free
//! of any graphics API: no GPU, no Python, no Qt. Three pieces, mirroring how Coin
//! is driven:
//!
//! - [`node`] — the **AST**: the scene-graph node vocabulary (`SoSeparator`,
//!   `SoCoordinate3`, `SoMaterial`, …).
//! - [`state`] — the **state accumulator**: the traversal that threads inherited
//!   state (transform, colour, depth bias, identity) down the tree, resolving to a
//!   [`StateSnapshot`] per draw.
//! - [`compile`] — the **mesh compiler**: it walks a [`Scene`] with the accumulator
//!   and emits a [`RenderQueue`] of [`DrawPacket`]s.
//!
//! The boundary a renderer consumes is the [`DrawBatch`] trait: a packet names its
//! mesh (`mesh_id` + `generation`), its [`Topology`], the [`StateSnapshot`] to bind,
//! and — **only on a cache miss** — fresh [`Geometry`]. A backend (`wgpu` today, D3D
//! or Metal later) keys its upload cache on `(mesh_id, generation)`, so the node
//! model and compiler never depend on the graphics API.
//!
//! [`registry`] reproduces the piece that makes Coin a cross-language ABI: the
//! name⇄type lookup (`SoType.fromName("SoBrepEdgeSet")`) Python workbenches use to
//! instantiate C++ node classes.
//!
//! [`view`] is the presentation boundary: [`InventorView`] lets a host present a
//! rendered scene as a shareable GPU surface or a readback [`ImageData`], expressed
//! renderer-agnostically so this crate stays free of `gpui`/`wgpu`.
//!
//! This crate depends only on [`ferrocad_types`] and stays a leaf.
//!
//! # Example
//!
//! ```
//! use ferrocad_inventor::{Scene, compile};
//!
//! // An empty scene compiles to an empty queue (nothing to draw yet).
//! let (queue, _directives) = compile(&Scene::default());
//! assert!(queue.is_empty());
//! ```

pub mod compile;
pub mod event;
pub mod node;
pub mod registry;
pub mod state;
pub mod view;

// The neutral draw contract, camera/viewport and picking live in `ferrocad_graphics`;
// they are re-exported (modules and types) so existing `ferrocad_inventor::…` paths
// keep working. New code should depend on `ferrocad_graphics` directly.
pub use ferrocad_graphics::{batch, camera, directive, geometry, pick, pixels};
pub use ferrocad_graphics::pick::{
    pick_at, pick_at_hit, pick_at_rect, pick_at_rect_hit, pick_hit,
};
pub use ferrocad_graphics::{
    Camera, Directive, Directives, DrawBatch, DrawPacket, Generation, Geometry, Hit, MeshId,
    NaviCubeDirective, PickId, Projection, Ray, Rect, RenderQueue, StateSnapshot, Texture2, Topology,
    Vertex, Viewport,
};

pub use compile::{ShaderError, compile, compile_diagnostic, compile_with, compile_with_base};
pub use event::{Event, EventListener};
pub use node::{
    Node, NodeKind, SO_SWITCH_ALL, SO_SWITCH_INHERIT, SO_SWITCH_NONE, Scene, ShaderParameter,
    ShaderStage, ShaderStageKind, depth_function, light_model, pick_style,
};
pub use registry::TypeRegistry;
pub use state::State;
pub use view::{ImageData, InventorView, ViewSpec};
