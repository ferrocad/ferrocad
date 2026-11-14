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
use std::sync::Arc;

use ferrocad_stage::shape::BLENDED_DEPTH;
use ferrocad_stage::{DepthState, DrawMode, ShapeItem, Stage, theme};

/// The mesh a [`StageMesh`] carries (the seam's tessellation vocabulary).
pub use ferrocad_geom::Tessellation;
/// The seam's shape payload: documented in [`ferrocad_shell_api::display`].
pub use ferrocad_shell_api::display::StageMesh;

/// The alpha of the datum card's glass fill.
///
/// The fill ink is the theme's glass-top token; only its alpha is lowered, so a
/// datum plane reads as a bounded card rather than a solid face. The rim uses
/// the front-edge token so the boundary stays readable where the fill fades.
const GLASS_FILL_ALPHA: f32 = 0.18;

/// The provider's opt-in name for a translucent card (`Part::DatumPlane`).
///
/// It reuses the existing `display` field, so a provider that omits it (every
/// solid in the document) keeps the opaque path unchanged.
const GLASS_CARD: &str = "glass";

/// The translucent fill of a datum card.
fn glass_fill() -> [f32; 4] {
    [
        theme::GLASS_TOP[0],
        theme::GLASS_TOP[1],
        theme::GLASS_TOP[2],
        GLASS_FILL_ALPHA,
    ]
}

/// The entity → document-object map, kept in step with the displayed stage.
///
/// It also keeps the item each entity was last displayed with, so a sync that leaves a
/// shape alone keeps its stage item — and with it the `MeshId` the renderer caches the
/// upload under and the emitted vertex buffers the queues share. Rebuilding the item
/// instead (which is what this used to do) allocated a fresh mesh identity per object per
/// sync, so a commit re-uploaded the whole document to the GPU and re-emitted every
/// vertex, twice.
#[derive(Debug, Default, Clone)]
pub struct DisplaySync {
    objects: HashMap<u32, String>,
    /// The item each displayed entity was built as, with the mesh it was built from.
    /// The mesh is compared by pointer first: a document backend hands over its
    /// tessellation memo, so an unchanged object is `Arc::ptr_eq` and costs nothing.
    items: HashMap<u32, Displayed>,
}

/// One entity's last displayed item and the mesh it was built from.
#[derive(Debug, Clone)]
struct Displayed {
    mesh: Arc<Tessellation>,
    item: ShapeItem,
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
    ///
    /// An entity whose mesh is unchanged keeps the item it had: same [`ShapeItem`], so the
    /// same mesh identities and the same emitted geometry. Only its placement, origin and
    /// display attributes are taken from the new row. That is what makes a commit cost
    /// follow the object that changed instead of the scene: the viewport's queue is still
    /// re-emitted (a packet list is cheap when every pass is shared), but nothing but the
    /// edited body is re-uploaded, and a rigid move still takes the transform-only path.
    pub fn build(&mut self, meshes: impl IntoIterator<Item = StageMesh>) -> Stage {
        let mut stage = Stage::new();
        let mut objects = HashMap::with_capacity(self.objects.len());
        let mut items = HashMap::with_capacity(self.items.len());
        for mesh in meshes {
            if mesh.mesh.positions.is_empty() {
                continue;
            }
            let entity = mesh.entity;
            // The rigid move the rows carry, when the host kept them in the frame they
            // were tessellated in. `positions` with it applied is world space, which is
            // what a row with no placement already is.
            let placement = mesh
                .placement
                .unwrap_or_else(ferrocad_types::Placement::identity);
            let mut item = match self.items.remove(&entity) {
                // The mesh is the same object the last sync drew: keep the item that
                // already owns the renderer's upload. A script half answers with fresh
                // rows, so its equal-but-distinct meshes are compared by content — once,
                // for the rows a script supplies, not for a document's own objects.
                Some(displayed)
                    if Arc::ptr_eq(&displayed.mesh, &mesh.mesh)
                        || *displayed.mesh == *mesh.mesh =>
                {
                    displayed.item
                }
                _ => ShapeItem::from_tessellation_owned(entity, (*mesh.mesh).clone()),
            };
            item.set_placement(placement);
            item.set_origin(mesh.origin);
            // The highlight is view state (what the pointer is over), not the document's:
            // the shell paints it again after a sync, so a reused item must not bring the
            // previous selection's parts back with it.
            item.set_highlight(std::iter::empty());
            style(&mut item, mesh.display.as_deref(), mesh.color);
            objects.insert(entity, mesh.object);
            items.insert(
                entity,
                Displayed {
                    mesh: mesh.mesh,
                    item: item.clone(),
                },
            );
            stage.add_item(item);
        }
        self.objects = objects;
        self.items = items;
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

/// Apply a row's display attributes to `item`.
///
/// Every field the row controls is written, not just the ones the row sets: an unchanged
/// item is reused, and a row that switches representation — a solid becoming a glass
/// datum card, or gaining a workbench colour — must not inherit a flag from the item it
/// reused.
fn style(item: &mut ShapeItem, display: Option<&str>, color: Option<[f32; 4]>) {
    item.set_draw_mode(display_mode(display));
    if display == Some(GLASS_CARD) {
        // A datum card: a low-alpha glass fill plus the front-edge rim, in the blended
        // pass. No occluded ghost edges — the card is meant to be read through, and its
        // rim is the whole boundary cue.
        item.set_surface_depth(BLENDED_DEPTH);
        item.set_wire_depth(DepthState::default());
        item.set_ghost_edges(false);
        item.set_ghost_surface(false);
        item.set_surface_color(glass_fill());
        item.set_shade_color(Some(glass_fill()));
        item.set_edge_color(theme::EDGE_FRONT);
        return;
    }
    // Document solids get the V1 occluded-edge ghost; overlays do not.
    item.set_surface_depth(DepthState::default());
    item.set_wire_depth(DepthState::default());
    item.set_ghost_edges(true);
    item.set_ghost_surface(false);
    match color {
        Some(color) => {
            // The edge colour follows the surface, so a wireframe shows the same colour
            // as the shaded item. A workbench colour stays single-tone.
            item.set_surface_color(color);
            item.set_shade_color(None);
            item.set_edge_color(color);
        }
        None => {
            // V2b: up-facing facets use the glass-top ink, the rest the shade.
            item.set_surface_color(theme::GLASS_TOP);
            item.set_shade_color(Some(theme::GLASS_SHADE));
            item.set_edge_color(theme::EDGE_FRONT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slab whose `Edge1` is the segment from (0,0,0) to (10,0,0), tagged `Face1`.
    fn slab(entity: u32, object: &str) -> StageMesh {
        slab_with(
            entity,
            object,
            vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
        )
    }

    /// The same row with an explicit mesh, so a test can change the geometry of one
    /// entity while another stays put.
    fn slab_with(entity: u32, object: &str, positions: Vec<[f32; 3]>) -> StageMesh {
        StageMesh {
            entity,
            object: object.to_string(),
            label: object.to_string(),
            mesh: Arc::new(Tessellation {
                positions,
                triangles: vec![0, 1, 2],
                triangle_parts: vec![0],
                edges: vec![0, 1],
                segment_parts: vec![1],
                parts: vec!["Face1".to_string(), "Edge1".to_string()],
                ..Tessellation::default()
            }),
            origin: Some([0.0, 0.0, 0.0]),
            placement: None,
            color: None,
            display: None,
        }
    }

    /// The mesh ids `entity` draws with, in packet order.
    fn mesh_ids(stage: &Stage, entity: u32) -> Vec<u64> {
        stage
            .to_queue()
            .packets
            .iter()
            .filter(|packet| packet.state.id.0 == entity)
            .map(|packet| packet.mesh_id.0)
            .collect()
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

    /// A document item carries **world-space** geometry and an identity placement.
    ///
    /// The provider bakes the object's `Placement` into the mesh and reports its base as
    /// `origin` (`python/ferrocad_parametric/__init__.py:86-93`), so `placement` is not
    /// the object's transform — `origin` is the datum a gizmo (and the future transform
    /// session) hugs. Anything that reads `placement()` as "where the object is" would be
    /// wrong, which is why this is pinned.
    #[test]
    fn a_document_item_is_world_space_with_an_identity_placement() {
        let mut display = DisplaySync::new();
        let mut mesh = slab_with(
            7,
            "Box",
            vec![[5.0, 0.0, 0.0], [15.0, 0.0, 0.0], [5.0, 10.0, 0.0]],
        );
        mesh.origin = Some([5.0, 0.0, 0.0]);
        let stage = display.build(vec![mesh]);
        let item = &stage.items()[0];
        assert_eq!(
            item.placement(),
            ferrocad_types::Placement::identity(),
            "a document item's placement is not the object's transform"
        );
        assert_eq!(
            item.anchor(),
            Some(ferrocad_types::Vector3::new(5.0, 0.0, 0.0)),
            "the datum is the world-space origin, not the placement's base"
        );
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
    fn a_blended_mesh_is_a_glass_card_with_a_rim() {
        let mut display = DisplaySync::new();
        let mut datum = slab(7, "DatumPlane");
        datum.display = Some(GLASS_CARD.to_string());
        let stage = display.build(vec![datum]);
        let item = stage.item(7).expect("the item");

        // The fill is the glass-top ink at the card's low alpha, in the blended
        // pass; the rim is the front-edge token, and no occluded ghost is emitted.
        assert_eq!(item.surface_color(), glass_fill());
        assert_eq!(item.surface_color()[3], GLASS_FILL_ALPHA);
        assert_eq!(item.edge_color(), theme::EDGE_FRONT);
        assert!(!item.ghost_edges(), "a card has one crisp rim, not a ghost");
        assert_eq!(item.draw_mode(), DrawMode::ShadedWithEdges);

        // The same mesh without the opt-in keeps the opaque solid path.
        let stage = display.build(vec![slab(8, "Box")]);
        let solid = stage.item(8).expect("the solid");
        assert!(solid.ghost_edges());
        assert_eq!(solid.surface_color(), theme::GLASS_TOP);
        assert_ne!(solid.surface_color()[3], GLASS_FILL_ALPHA);
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
        let empty = slab_with(8, "Empty", Vec::new());
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

    /// A commit that leaves a shape alone must not re-upload it: the item it already drew
    /// with is kept, mesh identity and all, so the renderer's `(mesh_id, generation)`
    /// cache hits. Before this, every sync allocated a fresh identity per object and the
    /// GPU re-uploaded the whole document.
    #[test]
    fn an_unchanged_mesh_keeps_its_mesh_identity() {
        let mut display = DisplaySync::new();
        let shared = Arc::new(Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
            ..Tessellation::default()
        });
        let row = |entity: u32, mesh: &Arc<Tessellation>| StageMesh {
            entity,
            object: "Box".to_string(),
            label: "Box".to_string(),
            mesh: Arc::clone(mesh),
            origin: Some([0.0, 0.0, 0.0]),
            placement: None,
            color: None,
            display: None,
        };

        let first = display.build([row(7, &shared), row(8, &shared)]);
        let before = (mesh_ids(&first, 7), mesh_ids(&first, 8));
        assert_eq!(
            before.0.len(),
            3,
            "the surface, the front wire and its occluded ghost"
        );

        // The same document twice: the document backend hands over the memo's `Arc`, so
        // this is exactly what a commit that changed nothing looks like.
        let again = display.build([row(7, &shared), row(8, &shared)]);
        assert_eq!(mesh_ids(&again, 7), before.0, "entity 7 keeps its upload");
        assert_eq!(mesh_ids(&again, 8), before.1, "entity 8 keeps its upload");

        // Reordering the rows is not a geometry change either.
        let reordered = display.build([row(8, &shared), row(7, &shared)]);
        assert_eq!(mesh_ids(&reordered, 7), before.0);
        assert_eq!(mesh_ids(&reordered, 8), before.1);

        // One object edited: only that one gets a new identity.
        let edited = Arc::new(Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
            ..Tessellation::default()
        });
        let after = display.build([row(7, &edited), row(8, &shared)]);
        assert_ne!(mesh_ids(&after, 7), before.0, "the edited body re-uploads");
        assert_eq!(mesh_ids(&after, 8), before.1, "its neighbour does not");
    }

    /// A script half answers with fresh rows, so its meshes are equal but distinct. They
    /// still keep the item: the comparison falls back to the content.
    #[test]
    fn a_row_with_equal_content_keeps_its_mesh_identity() {
        let mut display = DisplaySync::new();
        let first = display.build([slab(7, "Box")]);
        let before = mesh_ids(&first, 7);
        let again = display.build([slab(7, "Box")]);
        assert_eq!(mesh_ids(&again, 7), before);
    }

    /// A reused item takes the new row's placement, origin and style — including a switch
    /// of representation, which must not leave the previous row's flags behind.
    #[test]
    fn a_reused_item_takes_the_rows_placement_and_style() {
        use ferrocad_types::{Placement, Rotation, Vector3};

        let mut display = DisplaySync::new();
        let shared = Arc::new(Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            ..Tessellation::default()
        });
        let row = |placement: Placement, origin: [f32; 3], display: Option<&str>| StageMesh {
            entity: 7,
            object: "Box".to_string(),
            label: "Box".to_string(),
            mesh: Arc::clone(&shared),
            origin: Some(origin),
            placement: Some(placement),
            color: None,
            display: display.map(str::to_string),
        };

        let resting = display.build([row(Placement::identity(), [0.0; 3], None)]);
        let mut moved = display.build([row(
            Placement::new(Vector3::new(3.0, 4.0, 0.0), Rotation::identity()),
            [3.0, 4.0, 0.0],
            None,
        )]);
        assert_eq!(
            moved.items()[0].placement(),
            Placement::new(Vector3::new(3.0, 4.0, 0.0), Rotation::identity())
        );
        assert_eq!(moved.items()[0].origin(), Some([3.0, 4.0, 0.0]));
        // The move is the only change, so the viewport's transform-only path applies.
        assert!(resting.same_rigid_items(&moved));
        moved.items_mut()[0].set_placement(Placement::identity());
        moved.items_mut()[0].set_origin(Some([0.0; 3]));
        assert!(resting.same_rigid_items(&moved));

        // A datum card reuses the same geometry but replaces every style flag.
        let card = display.build([row(Placement::identity(), [0.0; 3], Some(GLASS_CARD))]);
        let item = card.item(7).expect("the card");
        assert_eq!(item.surface_color(), glass_fill());
        assert!(!item.ghost_edges(), "the old solid's ghost pass is gone");
        assert_eq!(item.surface_depth(), ferrocad_stage::shape::BLENDED_DEPTH);

        // And back: the solid's flags come back with it.
        let solid = display.build([row(Placement::identity(), [0.0; 3], None)]);
        let item = solid.item(7).expect("the solid");
        assert!(item.ghost_edges());
        assert_eq!(item.surface_color(), theme::GLASS_TOP);
        assert_eq!(item.surface_depth(), DepthState::default());
    }

    /// The highlight is view state, not the document's: a sync must not paint the
    /// previous selection onto a shape the reused item brought along.
    #[test]
    fn a_reused_item_does_not_keep_the_selection() {
        let mut display = DisplaySync::new();
        let mut stage = display.build([slab(7, "Box")]);
        stage.items_mut()[0].set_highlight([1]);
        assert!(!stage.items()[0].highlight().is_empty());

        let stage = display.build([slab(7, "Box")]);
        assert!(stage.items()[0].highlight().is_empty());
    }
}
