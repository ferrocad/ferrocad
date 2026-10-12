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
    /// Texture coordinates (`[0, 0]` when the shape has no texture).
    pub uv: [f32; 2],
}

/// A CPU-side RGBA8 texture: the `SoTexture2` payload a shape samples.
///
/// The `id` is a stable identity assigned on creation, so a renderer can key its
/// upload cache on it (and a node can be cloned — and the scene rebuilt — without
/// re-uploading, as long as the same `Arc` is reused).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture2 {
    /// A process-unique identity for the renderer's upload cache.
    pub id: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8.
    pub rgba: Vec<u8>,
}

impl Texture2 {
    /// Wrap RGBA8 pixels, assigning a fresh identity.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            rgba,
        }
    }
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
