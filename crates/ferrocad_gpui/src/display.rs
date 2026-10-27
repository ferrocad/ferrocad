//! Document shapes → the viewport's stage: the host's display half.
//!
//! The shell names no workbench: it asks the app script for the tessellated shapes
//! (`stage_meshes`, which asks each module through `FerroCAD.display_mesh`) and this
//! module turns them into stage items and keeps the **entity → object** map a pick
//! needs. A workbench contributes meshes through the `FreeCAD` display hook; nothing
//! here knows what a `Part::Feature` is.
//!
//! This is the generic half of what used to be `HostDressUp`: the manipulator half
//! moved to `ferrocad_part_gui`, and the edit half is the app script's `interaction`
//! hook (see [`crate::shell`] and `docs/developer/architecture.md` §10).

use std::collections::HashMap;

use ferrocad_geom::Tessellation;
use ferrocad_stage::{DrawMode, ShapeItem, Stage, theme};
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
    /// How to draw it: `"shaded"`, `"wireframe"` or `"shaded_edges"` (the default).
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

/// The entity → document-object map, kept in step with the displayed stage.
///
/// A pick reports an entity id; the inspector and the interaction handler need the
/// document object's name. This is the only state the display path keeps, and it is
/// rebuilt wholesale on every sync (a document has hundreds of objects, not millions).
#[derive(Debug, Default, Clone)]
pub struct DisplaySync {
    objects: HashMap<u32, String>,
}

impl DisplaySync {
    /// An empty sync (no shapes displayed).
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the stage from `meshes`, replacing the entity map.
    ///
    /// A mesh with no positions is skipped (a shape the workbench could not
    /// tessellate), and an empty result means "nothing to show" — the caller keeps the
    /// sample scene.
    pub fn build(&mut self, meshes: impl IntoIterator<Item = StageMesh>) -> Stage {
        let mut stage = Stage::new();
        self.objects.clear();
        for mesh in meshes {
            if mesh.positions.is_empty() {
                continue;
            }
            let mut item = ShapeItem::from_tessellation(mesh.entity, &mesh.tessellation())
                .with_draw_mode(display_mode(mesh.display.as_deref()))
                .with_origin(mesh.origin)
                // Document solids get the V1 occluded-edge ghost; overlays do not.
                .with_ghost_edges();
            if mesh.color.is_none() {
                // V2b: up-facing facets use the glass-top ink, the rest the shade.
                item = item
                    .with_surface_color(theme::GLASS_TOP)
                    .with_shade_color(theme::GLASS_SHADE);
            }
            if let Some(color) = mesh.color {
                // The edge colour follows the surface, so a wireframe shows the same
                // colour as the shaded item. A workbench colour stays single-tone.
                item = item.with_surface_color(color).with_edge_color(color);
            }
            self.objects.insert(mesh.entity, mesh.object);
            stage.add_item(item);
        }
        stage
    }

    /// The document object a picked entity belongs to.
    pub fn object_of(&self, entity: u32) -> Option<&str> {
        self.objects.get(&entity).map(String::as_str)
    }

    /// The displayed entity for a document object (the inverse of
    /// [`object_of`](Self::object_of)).
    ///
    /// A tree selection carries a name, not a pick id; arming a transform gizmo from
    /// it needs the entity the stage knows. The map is small (one entry per displayed
    /// shape), so a scan is cheaper than a second index to keep in step.
    pub fn entity_of(&self, object: &str) -> Option<u32> {
        self.objects
            .iter()
            .find(|(_, name)| name.as_str() == object)
            .map(|(entity, _)| *entity)
    }

    /// The number of displayed entities.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether nothing is displayed.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// The draw mode a provider's `display` string names.
///
/// Unknown or absent values mean "shaded with edges", the CAD default; a provider
/// controls representation without the host knowing its vocabulary.
fn display_mode(display: Option<&str>) -> DrawMode {
    match display {
        Some("wireframe") => DrawMode::Wireframe,
        Some("shaded") => DrawMode::Shaded,
        _ => DrawMode::ShadedWithEdges,
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
            origin: Some([0.0, 0.0, 0.0]),
            color: None,
            display: None,
        }
    }

    #[test]
    fn build_creates_a_stage_and_maps_entities_to_objects() {
        let mut display = DisplaySync::new();
        let stage = display.build(vec![slab(7, "Box")]);
        assert_eq!(stage.len(), 1);
        assert_eq!(stage.items()[0].entity_id(), 7);
        assert_eq!(display.object_of(7), Some("Box"));
        assert_eq!(display.object_of(8), None);
        assert_eq!(display.len(), 1);
    }

    #[test]
    fn a_default_surface_is_the_two_tone_glass() {
        let mut display = DisplaySync::new();
        let stage = display.build(vec![slab(7, "Box")]);
        let item = stage.item(7).expect("the item");
        assert_eq!(item.surface_color(), theme::GLASS_TOP);
        assert!(item.ghost_edges(), "document solids opt into the ghost pass");
        let queue = stage.to_queue();
        assert_eq!(
            queue.packets[0].state.shade,
            theme::GLASS_SHADE,
            "the side facets carry the glass shade"
        );
    }

    #[test]
    fn an_empty_document_builds_an_empty_stage() {
        let mut display = DisplaySync::new();
        let stage = display.build(Vec::new());
        assert!(stage.is_empty());
        assert!(display.is_empty());
        assert_eq!(display.object_of(7), None);
    }

    #[test]
    fn a_mesh_without_positions_is_skipped() {
        let mut display = DisplaySync::new();
        let mut empty = slab(8, "Empty");
        empty.positions.clear();
        let stage = display.build(vec![slab(7, "Box"), empty]);
        assert_eq!(stage.len(), 1);
        assert_eq!(display.object_of(7), Some("Box"));
        assert_eq!(display.object_of(8), None);
    }

    #[test]
    fn a_second_sync_replaces_the_entity_map() {
        let mut display = DisplaySync::new();
        display.build(vec![slab(7, "Box")]);
        display.build(vec![slab(9, "Cylinder")]);
        assert_eq!(display.object_of(7), None);
        assert_eq!(display.object_of(9), Some("Cylinder"));
    }

    #[test]
    fn a_provider_controls_the_colour_and_the_draw_mode() {
        let mut display = DisplaySync::new();
        let mut shaded = slab(7, "Box");
        shaded.color = Some([0.2, 0.4, 0.8, 1.0]);
        shaded.display = Some("shaded".to_string());
        let stage = display.build(vec![shaded]);
        let item = stage.item(7).expect("the item");
        assert_eq!(item.draw_mode(), DrawMode::Shaded);
        assert_eq!(item.surface_color(), [0.2, 0.4, 0.8, 1.0]);
        assert_eq!(item.edge_color(), [0.2, 0.4, 0.8, 1.0]);

        let mut wire = slab(8, "Cage");
        wire.display = Some("wireframe".to_string());
        let stage = display.build(vec![wire]);
        assert_eq!(
            stage.item(8).expect("the item").draw_mode(),
            DrawMode::Wireframe
        );

        // An absent or unknown mode falls back to shaded-with-edges.
        let stage = display.build(vec![slab(9, "Plain")]);
        assert_eq!(
            stage.item(9).expect("the item").draw_mode(),
            DrawMode::ShadedWithEdges
        );
    }
}
