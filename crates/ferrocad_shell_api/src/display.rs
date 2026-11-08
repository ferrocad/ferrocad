//! The tessellated document shape an [`AppBackend`](crate::app::AppBackend) returns.
//!
//! The mesh fields mirror [`ferrocad_geom::Tessellation`]; the shell's
//! [`DisplaySync`](https://docs.rs/ferrocad_shell_core) turns these into stage items and
//! keeps the entity → object map a pick needs. It lives in the API crate because it
//! crosses the backend seam.

use ferrocad_geom::Tessellation;
use ferrocad_types::Placement;
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
    /// The rigid move the stored geometry carries, when the host keeps `positions` in the
    /// frame they were tessellated in instead of baking the object's placement into them.
    ///
    /// `positions` and `placement` together are what `origin` alone describes for the
    /// gizmo: a host that reports both lets the viewport draw the body where it belongs
    /// while its rows stay byte-identical across a move — which is what lets a commit
    /// patch packet transforms instead of re-emitting every vertex. Absent (the display
    /// hook's answered rows) means the positions are already world space.
    #[serde(default)]
    pub placement: Option<Placement>,
    /// The object's RGBA surface colour (linear, 0..1), if the provider supplies one.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// How to draw it: `"shaded"`, `"wireframe"`, `"shaded_edges"` (the default)
    /// or the opt-in `"glass"` card (a datum plane).
    #[serde(default)]
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

    /// The mesh in the seam's vocabulary.
    ///
    /// Copies every array, so prefer [`into_tessellation`](Self::into_tessellation)
    /// when the caller owns the mesh — `DisplaySync` does.
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

    /// The mesh in the seam's vocabulary, moving the arrays instead of copying them.
    ///
    /// This is the form `DisplaySync` uses: it owns the `StageMesh` it was handed, and
    /// on a large document the copy [`tessellation`](Self::tessellation) makes is a
    /// second pass over every vertex.
    pub fn into_tessellation(self) -> Tessellation {
        let Self {
            positions,
            normals,
            triangles,
            triangle_parts,
            edges,
            segment_parts,
            vertices,
            vertex_parts,
            parts,
            ..
        } = self;
        Tessellation {
            positions,
            normals,
            triangles,
            triangle_parts,
            edges,
            segment_parts,
            vertices,
            vertex_parts,
            parts,
        }
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
        assert_eq!(meshes[0].triangles, vec![0, 1, 2]);
        // Everything omitted falls back, so a hook does not have to spell out edges,
        // parts or a colour to draw a shape.
        assert!(meshes[0].label.is_empty());
        assert!(meshes[0].edges.is_empty());
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
}
