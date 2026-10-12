//! The batch boundary: what the inventor state machine emits and a renderer draws.
//!
//! The pipeline is:
//!
//! ```text
//! Scene ──compile──▶ RenderQueue { DrawPacket … } ──DrawBatch trait──▶ renderer
//! ```
//!
//! A [`DrawPacket`] names a mesh (`mesh_id` + `generation`), how to assemble it
//! ([`Topology`]), the state to bind ([`StateSnapshot`]) and — **only on a cache
//! miss** — fresh [`Geometry`]. The renderer keys its upload cache on
//! `(mesh_id, generation)`, so a hit carries no geometry and reuses the upload.
//!
//! [`DrawBatch`] is the renderer-facing trait; [`DrawPacket`] is the core
//! implementation. Keeping the boundary a trait (rather than a concrete struct)
//! lets each backend accept any batch type while the node model and compiler stay
//! independent of the graphics API.

use crate::geometry::{Geometry, Topology};
use crate::state::StateSnapshot;

/// Identity of a mesh within the scene. Stable across frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MeshId(pub u64);

/// Version of a mesh's geometry.
///
/// Bumped whenever the geometry changes, so a renderer's upload cache is keyed on
/// `(MeshId, Generation)` and a stale upload is replaced, never reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Generation(pub u64);

/// Per-draw selection identity, written to the picking buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PickId(pub u32);

/// One draw packet produced by the inventor state machine.
#[derive(Clone, Debug)]
pub struct DrawPacket {
    /// Which mesh this draws.
    pub mesh_id: MeshId,
    /// Which version of that mesh's geometry.
    pub generation: Generation,
    /// How to assemble the vertices.
    pub topology: Topology,
    /// Fresh geometry, present **only on a cache miss**. `None` means the renderer
    /// already holds `(mesh_id, generation)` and must reuse it.
    pub geometry: Option<Geometry>,
    /// The resolved traversal state to bind for this draw.
    pub state: StateSnapshot,
}

impl DrawPacket {
    /// Whether this packet carries geometry the renderer must upload.
    pub fn is_cache_miss(&self) -> bool {
        self.geometry.is_some()
    }
}

/// The renderer-facing boundary: everything a backend needs to draw one packet.
///
/// A backend takes `&[impl DrawBatch]` and, for each, checks
/// [`geometry`](Self::geometry) to decide upload-vs-reuse, then binds
/// [`state`](Self::state) and draws [`topology`](Self::topology) up to
/// [`mesh_id`](Self::mesh_id)/[`generation`](Self::generation)'s cached sizes.
pub trait DrawBatch {
    /// Which mesh this draws.
    fn mesh_id(&self) -> MeshId;
    /// Which version of that mesh's geometry.
    fn generation(&self) -> Generation;
    /// How to assemble the vertices.
    fn topology(&self) -> Topology;
    /// Fresh geometry on a cache miss; `None` when the renderer can reuse its upload.
    fn geometry(&self) -> Option<&Geometry>;
    /// The traversal state to bind for this draw.
    fn state(&self) -> &StateSnapshot;
}

impl DrawBatch for DrawPacket {
    fn mesh_id(&self) -> MeshId {
        self.mesh_id
    }
    fn generation(&self) -> Generation {
        self.generation
    }
    fn topology(&self) -> Topology {
        self.topology
    }
    fn geometry(&self) -> Option<&Geometry> {
        self.geometry.as_ref()
    }
    fn state(&self) -> &StateSnapshot {
        &self.state
    }
}

/// The packet stream a scene compiles into.
#[derive(Clone, Debug, Default)]
pub struct RenderQueue {
    /// The packets to draw, in scene-graph order.
    pub packets: Vec<DrawPacket>,
}

impl RenderQueue {
    /// Whether the queue has nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// The number of packets in the queue.
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Iterate the packets in draw order.
    pub fn iter(&self) -> impl Iterator<Item = &DrawPacket> {
        self.packets.iter()
    }
}
