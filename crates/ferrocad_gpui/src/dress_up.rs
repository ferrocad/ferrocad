//! Host-side wiring for the edge dress-up arc (F1).
//!
//! The viewport reports **what happened** — a [`SelectionRef`] on a click, a
//! [`DressUpChange`] when a scrub ends — and knows nothing about documents. This module
//! is the other half:
//!
//! ```text
//! selection (Edge3) ──▶ HostDressUp::scrubber_for ──▶ DressUpScrubber (viewport)
//!                                                          │ drag
//! DressUpChange ──▶ HostDressUp::apply ──▶ document ──▶ Part::Fillet ──▶ recompute
//!        ▲                                                      │
//!        └──────────────── sync() ◀── tessellated shapes ◀──────┘
//! ```
//!
//! The document sits behind [`DressUpDocument`], so the interaction logic is testable
//! without an interpreter or a kernel. The application supplies [`PythonDocument`],
//! which calls the shell's Python helpers (the host never links `ferrocad_part`).
//!
//! The stage builder owns the entity ids: the document reports one per shape, the host
//! maps it to the object name, and a pick therefore resolves back to the document.

use std::collections::HashMap;

use ferrocad_geom::{SelectionRef, Tessellation};
use ferrocad_stage::{
    DrawMode, DressUpChange, DressUpMode, DressUpScrubber, DressUpValue, ShapeItem, Stage,
};
use ferrocad_types::Vector3;
use serde::Deserialize;

use crate::python;

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

/// What the host needs from the document to run the dress-up interaction.
pub trait DressUpDocument: Send + Sync {
    /// The shapes to display, each with a stable entity id.
    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String>;

    /// Create or update the dress-up feature over `object` and recompute the document.
    fn apply_dress_up(
        &self,
        object: &str,
        mode: DressUpMode,
        edge: &str,
        value: DressUpValue,
    ) -> Result<(), String>;
}

/// The production document: the `FreeCAD` Python API through the shell's helpers.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonDocument;

impl DressUpDocument for PythonDocument {
    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        python::stage_meshes()
    }

    fn apply_dress_up(
        &self,
        object: &str,
        mode: DressUpMode,
        edge: &str,
        value: DressUpValue,
    ) -> Result<(), String> {
        let mode = match mode {
            DressUpMode::Fillet => "Fillet",
            DressUpMode::Chamfer => "Chamfer",
        };
        let (first, second) = match value {
            DressUpValue::Fillet { radius } => (radius, 0.0),
            DressUpValue::Chamfer { first, second } => (first, second),
        };
        let result = python::apply_dress_up(object, mode, edge, first, second)?;
        if result.ok {
            Ok(())
        } else if result.error.is_empty() {
            Err("the dress-up feature failed".to_string())
        } else {
            Err(result.error)
        }
    }
}

/// The host half of the dress-up interaction.
///
/// Ties a [`SelectionRef`] to the stage item it names (for the edge's points) and a
/// [`DressUpChange`] to the document object. The stage it builds carries the same
/// entity ids, so a pick and a scrub refer to the same shape.
pub struct HostDressUp<D: DressUpDocument> {
    document: D,
    mode: DressUpMode,
    /// Entity id → document object name.
    objects: HashMap<u32, String>,
    /// Entity id → the stage item (for `edge_polyline`).
    items: HashMap<u32, ShapeItem>,
}

impl<D: DressUpDocument> HostDressUp<D> {
    /// A host over `document` that creates fillets by default.
    pub fn new(document: D) -> Self {
        Self {
            document,
            mode: DressUpMode::Fillet,
            objects: HashMap::new(),
            items: HashMap::new(),
        }
    }

    /// The mode a new scrubber starts in.
    pub fn mode(&self) -> DressUpMode {
        self.mode
    }

    /// Set the mode a new scrubber starts in (`Fillet` or `Chamfer`).
    pub fn set_mode(&mut self, mode: DressUpMode) {
        self.mode = mode;
    }

    /// Re-read the document's shapes and build the stage to display.
    ///
    /// An empty stage means there is nothing to show (no Part workbench, an empty
    /// document), which the caller treats as "keep the sample scene".
    pub fn sync(&mut self) -> Result<Stage, String> {
        let meshes = self.document.stage_meshes()?;
        let mut stage = Stage::new();
        self.objects.clear();
        self.items.clear();
        for mesh in meshes {
            if mesh.positions.is_empty() {
                continue;
            }
            let item = ShapeItem::from_tessellation(mesh.entity, &mesh.tessellation())
                .with_draw_mode(DrawMode::ShadedWithEdges);
            self.objects.insert(mesh.entity, mesh.object);
            self.items.insert(mesh.entity, item.clone());
            stage.add_item(item);
        }
        Ok(stage)
    }

    /// The document object a picked entity belongs to.
    pub fn object_of(&self, entity: u32) -> Option<&str> {
        self.objects.get(&entity).map(String::as_str)
    }

    /// The number of shapes in the last sync.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the last sync found no shapes.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The scrubber for a selection, when it names an **edge** of a displayed shape.
    ///
    /// A face or vertex selection, an unknown entity or a shape whose mesh carries no
    /// wire run returns `None`: there is nothing to drag a radius against.
    pub fn scrubber_for(&self, selection: &SelectionRef) -> Option<DressUpScrubber> {
        let edge = selection.elements.first()?;
        if !edge.starts_with("Edge") {
            return None;
        }
        let points = self.items.get(&selection.object)?.edge_polyline(edge)?;
        if points.len() < 2 {
            return None;
        }
        let points = points
            .into_iter()
            .map(|point| {
                Vector3::new(
                    f64::from(point[0]),
                    f64::from(point[1]),
                    f64::from(point[2]),
                )
            })
            .collect();
        Some(DressUpScrubber::new(
            selection.object,
            self.mode,
            edge.clone(),
            points,
        ))
    }

    /// Apply a committed scrub to the document.
    pub fn apply(&mut self, change: &DressUpChange) -> Result<(), String> {
        let object = self
            .objects
            .get(&change.target_id)
            .cloned()
            .ok_or_else(|| format!("no document object for entity {}", change.target_id))?;
        let mode = match change.value {
            DressUpValue::Fillet { .. } => DressUpMode::Fillet,
            DressUpValue::Chamfer { .. } => DressUpMode::Chamfer,
        };
        self.document
            .apply_dress_up(&object, mode, &change.edge, change.value)
    }
}

/// A fake document for the host tests: fixed meshes and a recorded edit log.
#[cfg(test)]
#[derive(Clone)]
struct FakeDocument {
    meshes: Vec<StageMesh>,
    applied: std::sync::Arc<std::sync::Mutex<Vec<(String, DressUpMode, String, DressUpValue)>>>,
}

#[cfg(test)]
impl FakeDocument {
    fn new(meshes: Vec<StageMesh>) -> Self {
        Self {
            meshes,
            applied: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn applied(&self) -> Vec<(String, DressUpMode, String, DressUpValue)> {
        self.applied.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl DressUpDocument for FakeDocument {
    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        Ok(self.meshes.clone())
    }

    fn apply_dress_up(
        &self,
        object: &str,
        mode: DressUpMode,
        edge: &str,
        value: DressUpValue,
    ) -> Result<(), String> {
        self.applied
            .lock()
            .unwrap()
            .push((object.to_string(), mode, edge.to_string(), value));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slab whose `Edge1` is the segment from (0,0,0) to (10,0,0), tagged `Face1`.
    fn slab(entity: u32, object: &str) -> StageMesh {
        StageMesh {
            entity,
            object: object.to_string(),
            label: object.to_string(),
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
        }
    }

    fn host() -> HostDressUp<FakeDocument> {
        HostDressUp::new(FakeDocument::new(vec![slab(7, "Box")]))
    }

    #[test]
    fn sync_builds_a_stage_and_maps_entities_to_objects() {
        let mut host = host();
        let stage = host.sync().expect("a stage");
        assert_eq!(stage.len(), 1);
        assert_eq!(stage.items()[0].entity_id(), 7);
        assert_eq!(host.object_of(7), Some("Box"));
        assert_eq!(host.object_of(8), None);
        assert_eq!(host.len(), 1);
        assert!(!host.is_empty());
    }

    #[test]
    fn an_empty_document_syncs_to_an_empty_stage() {
        let mut host = HostDressUp::new(FakeDocument::new(Vec::new()));
        let stage = host.sync().unwrap();
        assert!(stage.is_empty());
        assert!(host.is_empty());
    }

    #[test]
    fn an_edge_selection_makes_a_scrubber_over_the_edge() {
        let mut host = host();
        host.sync().unwrap();
        let selection = SelectionRef::element(7, "Edge1");
        let scrubber = host.scrubber_for(&selection).expect("a scrubber");
        assert_eq!(scrubber.target_id(), 7);
        assert_eq!(scrubber.edge_name(), "Edge1");
        assert_eq!(scrubber.mode(), DressUpMode::Fillet);

        // The edge runs from (0,0,0) to (10,0,0); the anchor is its midpoint.
        let anchor = scrubber.anchor().expect("an anchor");
        assert!((anchor.x - 5.0).abs() < 1e-9);
        assert!(anchor.y.abs() < 1e-9);
    }

    #[test]
    fn a_face_or_unknown_selection_makes_no_scrubber() {
        let mut host = host();
        host.sync().unwrap();
        assert!(
            host.scrubber_for(&SelectionRef::element(7, "Face1"))
                .is_none()
        );
        assert!(host.scrubber_for(&SelectionRef::whole(7)).is_none());
        assert!(
            host.scrubber_for(&SelectionRef::element(9, "Edge1"))
                .is_none()
        );
        assert!(
            host.scrubber_for(&SelectionRef::element(7, "Edge9"))
                .is_none()
        );
    }

    #[test]
    fn a_chamfer_mode_selects_the_chamfer_feature() {
        let document = FakeDocument::new(vec![slab(7, "Box")]);
        let applied = document.applied.clone();
        let mut host = HostDressUp::new(document);
        host.sync().unwrap();
        host.set_mode(DressUpMode::Chamfer);

        let selection = SelectionRef::element(7, "Edge1");
        let scrubber = host.scrubber_for(&selection).expect("a scrubber");
        assert_eq!(scrubber.mode(), DressUpMode::Chamfer);

        host.apply(&DressUpChange {
            target_id: 7,
            edge: "Edge1".to_string(),
            value: DressUpValue::Chamfer {
                first: 2.0,
                second: 0.0,
            },
        })
        .unwrap();

        let applied = applied.lock().unwrap().clone();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "Box");
        assert_eq!(applied[0].1, DressUpMode::Chamfer);
        assert_eq!(applied[0].2, "Edge1");
        assert_eq!(
            applied[0].3,
            DressUpValue::Chamfer {
                first: 2.0,
                second: 0.0
            }
        );
    }

    #[test]
    fn a_commit_applies_the_object_and_value() {
        let mut host = host();
        host.sync().unwrap();
        host.apply(&DressUpChange {
            target_id: 7,
            edge: "Edge1".to_string(),
            value: DressUpValue::Fillet { radius: 2.5 },
        })
        .unwrap();

        let applied = host.document.applied();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "Box");
        assert_eq!(applied[0].1, DressUpMode::Fillet);
        assert_eq!(applied[0].2, "Edge1");
        assert_eq!(applied[0].3, DressUpValue::Fillet { radius: 2.5 });
    }

    #[test]
    fn committing_for_an_unknown_entity_is_an_error() {
        let mut host = host();
        host.sync().unwrap();
        let error = host
            .apply(&DressUpChange {
                target_id: 99,
                edge: "Edge1".to_string(),
                value: DressUpValue::Fillet { radius: 1.0 },
            })
            .unwrap_err();
        assert!(error.contains("99"), "the entity is named: {error}");
    }
}
