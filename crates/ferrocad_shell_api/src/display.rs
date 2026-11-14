//! The tessellated document shape an [`AppBackend`](crate::app::AppBackend) returns.
//!
//! The mesh is the seam's own [`Tessellation`] held behind an [`Arc`]; the shell's
//! [`DisplaySync`](https://docs.rs/ferrocad_shell_core) turns these into stage items and
//! keeps the entity → object map a pick needs. It lives in the API crate because it
//! crosses the backend seam.
//!
//! The [`Arc`] is not decoration. A document backend memoises each object's display
//! tessellation, and a sync asks every object for its mesh on every commit; handing the
//! memo's `Arc` over as-is is what keeps a commit from copying the document's vertices,
//! and it gives the synchroniser a zero-cost "did this object's mesh change?" — the
//! pointer. A script half that answers with JSON rows allocates a fresh mesh per row, so
//! its rows compare by content instead (see `DisplaySync::build`).

use std::sync::Arc;

/// The mesh a [`StageMesh`] carries; re-exported so a consumer of the seam can name the
/// type of its `mesh` field without depending on the geometry crate directly.
pub use ferrocad_geom::Tessellation;
use ferrocad_types::Placement;
use serde::{Deserialize, Deserializer};

/// One tessellated document shape, with the entity id a pick reports.
///
/// `entity` is assigned by the document and is stable for the lifetime of a sync.
///
/// Deserialization goes through [`StageMeshRow`]: a JSON row carries the mesh as flat
/// arrays, this holds it as a [`Tessellation`].
#[derive(Debug, Clone)]
pub struct StageMesh {
    /// The picking identity the viewport reports for this shape.
    pub entity: u32,
    /// The document object's internal name (`Box`).
    pub object: String,
    /// The user-visible label.
    pub label: String,
    /// The tessellated mesh: positions, per-vertex normals and the triangle, edge and
    /// vertex index runs.
    ///
    /// Shared with the producer's own memo when it has one, so a sync of an unchanged
    /// object is a pointer copy rather than a pass over every vertex.
    pub mesh: Arc<Tessellation>,
    /// The object's own origin in world space (the placement's base point), when the
    /// provider knows it. The transform triad hugs it — which also makes it an origin
    /// indicator — and a rotate gizmo pivots about it. Absent falls back to the
    /// bounding box's base corner.
    pub origin: Option<[f32; 3]>,
    /// The rigid move the stored geometry carries, when the host keeps `positions` in the
    /// frame they were tessellated in instead of baking the object's placement into them.
    ///
    /// `positions` and `placement` together are what `origin` alone describes for the
    /// gizmo: a host that reports both lets the viewport draw the body where it belongs
    /// while its rows stay byte-identical across a move — which is what lets a commit
    /// patch packet transforms instead of re-emitting every vertex. Absent (the display
    /// hook's answered rows) means the positions are already world space.
    pub placement: Option<Placement>,
    /// The object's RGBA surface colour (linear, 0..1), if the provider supplies one.
    pub color: Option<[f32; 4]>,
    /// How to draw it: `"shaded"`, `"wireframe"`, `"shaded_edges"` (the default)
    /// or the opt-in `"glass"` card (a datum plane).
    pub display: Option<String>,
}

impl StageMesh {
    /// Parse the JSON array a script half answers `stage_meshes` with.
    ///
    /// A script that has nothing to add answers `[]` (or `null`); both are the empty
    /// set, not an error. Field defaults make a minimal row legal: `{entity, object,
    /// positions, triangles}` draws, and everything else falls back.
    pub fn from_json_array(json: &str) -> Result<Vec<StageMesh>, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| format!("stage meshes: {error}"))?;
        match value {
            serde_json::Value::Null => Ok(Vec::new()),
            serde_json::Value::Array(rows) => rows
                .into_iter()
                .map(|row| {
                    serde_json::from_value(row).map_err(|error| format!("stage mesh: {error}"))
                })
                .collect(),
            _ => Err("stage meshes: expected an array".to_string()),
        }
    }

    /// The mesh in the seam's vocabulary, shared.
    pub fn tessellation(&self) -> Arc<Tessellation> {
        Arc::clone(&self.mesh)
    }
}

/// The row a script answers with: the mesh flattened into one field per array.
///
/// The seam's own shape holds the mesh as a [`Tessellation`], but a JSON row carries its
/// arrays inline (`positions`, `triangles`, …) and both script halves already emit that
/// form. This is the one place the two shapes meet.
#[derive(Deserialize)]
struct StageMeshRow {
    entity: u32,
    object: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    positions: Vec<[f32; 3]>,
    #[serde(default)]
    normals: Vec<[f32; 3]>,
    #[serde(default)]
    triangles: Vec<u32>,
    #[serde(default)]
    triangle_parts: Vec<u32>,
    #[serde(default)]
    edges: Vec<u32>,
    #[serde(default)]
    segment_parts: Vec<u32>,
    #[serde(default)]
    vertices: Vec<u32>,
    #[serde(default)]
    vertex_parts: Vec<u32>,
    #[serde(default)]
    parts: Vec<String>,
    #[serde(default)]
    origin: Option<[f32; 3]>,
    #[serde(default)]
    placement: Option<Placement>,
    #[serde(default)]
    color: Option<[f32; 4]>,
    #[serde(default)]
    display: Option<String>,
}

impl From<StageMeshRow> for StageMesh {
    fn from(row: StageMeshRow) -> Self {
        StageMesh {
            entity: row.entity,
            object: row.object,
            label: row.label,
            mesh: Arc::new(Tessellation {
                positions: row.positions,
                normals: row.normals,
                triangles: row.triangles,
                triangle_parts: row.triangle_parts,
                edges: row.edges,
                segment_parts: row.segment_parts,
                vertices: row.vertices,
                vertex_parts: row.vertex_parts,
                parts: row.parts,
            }),
            origin: row.origin,
            placement: row.placement,
            color: row.color,
            display: row.display,
        }
    }
}

impl<'de> Deserialize<'de> for StageMesh {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        StageMeshRow::deserialize(deserializer).map(StageMesh::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script can answer with the bare minimum: the id, the object and the arrays.
    #[test]
    fn a_minimal_row_parses() {
        let json = r#"[{"entity": 3, "object": "Toy",
                        "positions": [[0,0,0],[1,0,0],[0,1,0]],
                        "triangles": [0,1,2]}]"#;
        let meshes = StageMesh::from_json_array(json).expect("a row");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].entity, 3);
        assert_eq!(meshes[0].object, "Toy");
        assert_eq!(meshes[0].mesh.triangles, vec![0, 1, 2]);
        // Everything omitted falls back, so a hook does not have to spell out edges,
        // parts or a colour to draw a shape.
        assert!(meshes[0].label.is_empty());
        assert!(meshes[0].mesh.edges.is_empty());
        assert_eq!(meshes[0].display, None);
    }

    #[test]
    fn an_empty_answer_is_an_empty_set() {
        assert!(StageMesh::from_json_array("[]").unwrap().is_empty());
        assert!(StageMesh::from_json_array("null").unwrap().is_empty());
    }

    #[test]
    fn a_wrong_shape_is_an_error() {
        assert!(StageMesh::from_json_array("{}").is_err());
        assert!(StageMesh::from_json_array("not json").is_err());
    }

    /// A producer that already holds the mesh as a [`Tessellation`] keeps its `Arc`: the
    /// row is the mesh, not a copy of it (that is what makes a document sync cheap).
    #[test]
    fn a_document_row_shares_the_mesh_it_was_built_with() {
        let mesh = Arc::new(Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![0, 1, 2],
            ..Tessellation::default()
        });
        let row = StageMesh {
            entity: 7,
            object: "Box".to_string(),
            label: "Box".to_string(),
            mesh: Arc::clone(&mesh),
            origin: None,
            placement: None,
            color: None,
            display: None,
        };
        assert!(Arc::ptr_eq(&row.tessellation(), &mesh));
    }
}
