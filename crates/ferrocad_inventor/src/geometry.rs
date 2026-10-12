//! Geometry payloads: the vertex/index data a batch carries to a renderer.
//!
//! Kept free of any graphics API so the same payload feeds a `wgpu`, D3D or Metal
//! backend. Colour, transform and identity are *not* here — they belong to the
//! [`StateSnapshot`](crate::StateSnapshot) bound alongside the draw.

/// A single vertex.
///
/// `#[repr(C)]` so a backend can view a slice of these as bytes; a backend defines
/// its own mirror type with the `bytemuck` impls it needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vertex {
    /// Model-space position.
    pub position: [f32; 3],
    /// Model-space normal.
    pub normal: [f32; 3],
}

/// How a primitive's vertices are assembled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Topology {
    /// Independent triangles (three indices each).
    #[default]
    Triangles,
    /// Line segments (two indices each).
    Lines,
    /// Unconnected points.
    Points,
}

/// The CPU-side geometry of one mesh: vertices plus indices into them.
///
/// Carried by a [`DrawPacket`](crate::DrawPacket) **only on a cache miss**; on a
/// cache hit the packet omits it and the renderer reuses what it already uploaded
/// for the same `(mesh_id, generation)`.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    /// The mesh vertices.
    pub vertices: Vec<Vertex>,
    /// Indices into [`vertices`](Self::vertices).
    pub indices: Vec<u32>,
}
