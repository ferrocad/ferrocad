//! # FerroCAD graphics (`ferrocad_graphics`)
//!
//! The **neutral graphics primitives** shared by every producer and consumer of the
//! draw contract: the batch vocabulary, the camera and viewport, and CPU picking. It
//! knows nothing about Open Inventor, `wgpu`, Python or the UI — both the legacy
//! `ferrocad_inventor` stack and the modern `ferrocad_stage` stack compile down to it,
//! and both `ferrocad_wgpu` and the viewport consume it.
//!
//! ```text
//! ferrocad_inventor ─┐
//!                    ├─▶ ferrocad_graphics ─▶ ferrocad_wgpu
//! ferrocad_stage ────┘
//! ```
//!
//! The boundary a renderer consumes is the [`DrawBatch`] trait: a [`DrawPacket`] names
//! its mesh (`mesh_id` + `generation`), its [`Topology`], the [`StateSnapshot`] to bind,
//! and — only on a cache miss — fresh [`Geometry`]. A backend keys its upload cache on
//! `(mesh_id, generation)`, so no producer's node model leaks into the graphics API.

pub mod batch;
pub mod camera;
pub mod geometry;
pub mod pick;
pub mod pixels;
pub mod state;

pub use batch::{DrawBatch, DrawPacket, Generation, MeshId, PickId, RenderQueue};
pub use camera::{Camera, Projection};
pub use geometry::{Geometry, Texture2, Topology, Vertex};
pub use pick::{Hit, Ray, pick_at, pick_at_hit, pick_at_rect, pick_at_rect_hit, pick_hit};
pub use pixels::{Rect, Viewport};
pub use state::StateSnapshot;
