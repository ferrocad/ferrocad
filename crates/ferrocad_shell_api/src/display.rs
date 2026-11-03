//! The tessellated document shape an [`AppBackend`](crate::app::AppBackend) returns.
//!
//! The mesh fields mirror [`ferrocad_geom::Tessellation`]; the shell's
//! [`DisplaySync`](https://docs.rs/ferrocad_shell_core) turns these into stage items and
//! keeps the entity → object map a pick needs. It lives in the API crate because it
//! crosses the backend seam.

use ferrocad_geom::Tessellation;
use serde::Deserialize;

/// One tessellated document shape, with the entity id a pick reports.
///
/// The mesh fields mirror [`Tessellation`]; `entity` is assigned by the document and is
/// stable for the lifetime of a sync.
#[derive(Debug, Clone, Deserialize)]
pub struct StageMesh {
    /// The picking identity the viewport reports for this shape.
    pub entity: u32,
    /// The document object's internal name (`Box`).
    pub object: String,
    /// The user-visible label.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub positions: Vec<[f32; 3]>,
    #[serde(default)]
    pub normals: Vec<[f32; 3]>,
    #[serde(default)]
    pub triangles: Vec<u32>,
    #[serde(default)]
    pub triangle_parts: Vec<u32>,
    #[serde(default)]
    pub edges: Vec<u32>,
    #[serde(default)]
    pub segment_parts: Vec<u32>,
    #[serde(default)]
    pub vertices: Vec<u32>,
    #[serde(default)]
    pub vertex_parts: Vec<u32>,
    #[serde(default)]
    pub parts: Vec<String>,
    /// The object's own origin in world space (the placement's base point), when the
    /// provider knows it. The transform triad hugs it — which also makes it an origin
    /// indicator — and a rotate gizmo pivots about it. Absent falls back to the
    /// bounding box's base corner.
    #[serde(default)]
    pub origin: Option<[f32; 3]>,
    /// The object's RGBA surface colour (linear, 0..1), if the provider supplies one.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// How to draw it: `"shaded"`, `"wireframe"`, `"shaded_edges"` (the default)
    /// or the opt-in `"glass"` card (a datum plane).
    #[serde(default)]
    pub display: Option<String>,
}

impl StageMesh {
    /// The mesh in the seam's vocabulary.
    pub fn tessellation(&self) -> Tessellation {
        Tessellation {
            positions: self.positions.clone(),
            normals: self.normals.clone(),
            triangles: self.triangles.clone(),
            triangle_parts: self.triangle_parts.clone(),
            edges: self.edges.clone(),
            segment_parts: self.segment_parts.clone(),
            vertices: self.vertices.clone(),
            vertex_parts: self.vertex_parts.clone(),
            parts: self.parts.clone(),
        }
    }
}
