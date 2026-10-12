//! The frozen per-draw state a renderer binds: transform, colour, texture, identity.
//!
//! This is the neutral half of the state accumulator: the working [`State`] that a
//! traversal advances lives with the traversal (in `ferrocad_inventor`), but the
//! *snapshot* it resolves to — the thing a [`DrawPacket`](crate::DrawPacket) carries —
//! is shared by every producer.

use std::sync::Arc;

use ferrocad_types::Matrix4;

use crate::batch::PickId;
use crate::geometry::Texture2;

/// The resolved state bound to a draw, independent of the node model.
#[derive(Clone, Debug)]
pub struct StateSnapshot {
    /// The world transform to apply to the mesh.
    pub world_transform: Matrix4,
    /// Linear RGBA colour.
    pub color: [f32; 4],
    /// Depth bias applied before rasterisation.
    pub depth_bias: f32,
    /// The texture to sample, if any (multiplied into the colour).
    pub texture: Option<Arc<Texture2>>,
    /// Selection identity, written to the picking buffer.
    pub id: PickId,
}
