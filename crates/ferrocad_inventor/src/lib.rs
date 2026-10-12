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
//! let queue = compile(&Scene::default());
//! assert!(queue.is_empty());
//! ```

pub mod batch;
pub mod camera;
pub mod compile;
pub mod event;
pub mod geometry;
pub mod navicube;
pub mod node;
pub mod pick;
pub mod pixels;
pub mod registry;
pub mod state;
pub mod view;

pub use batch::{DrawBatch, DrawPacket, Generation, MeshId, PickId, RenderQueue};
pub use camera::Camera;
pub use compile::{compile, compile_with};
pub use event::{Event, EventListener, event_callbacks};
pub use geometry::{Geometry, Texture2, Topology, Vertex};
pub use node::{Node, NodeKind, Scene, SO_SWITCH_ALL, SO_SWITCH_NONE};
pub use pick::{Hit, Ray, pick, pick_at, pick_at_hit, pick_at_rect, pick_at_rect_hit, pick_hit};
pub use pixels::{Rect, Viewport};
pub use registry::TypeRegistry;
pub use state::{State, StateSnapshot};
pub use view::{ImageData, InventorView, ViewSpec};
