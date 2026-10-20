//! The stage: an ordered collection of declarative visual items.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::RenderQueue;
use ferrocad_types::Vector3;

use crate::dress_up::{DressUpMode, DressUpScrubber};
use crate::shape::ShapeItem;

/// A collection of declarative visual items.
///
/// A stage holds no scene graph: it is a flat, ordered list of items, and
/// [`emit_batches`](Self::emit_batches) compiles each to draw packets. Rebuilding the
/// list each frame is cheap, so a host can reassemble a stage from live document state
/// without diffing a node tree.
#[derive(Clone, Debug, Default)]
pub struct Stage {
    items: Vec<ShapeItem>,
}

impl Stage {
    /// An empty stage.
    pub fn new() -> Self {
        Self::default()
    }

    /// The builder form of [`add_item`](Self::add_item).
    pub fn with_item(mut self, item: ShapeItem) -> Self {
        self.items.push(item);
        self
    }

    /// Append an item (drawn last).
    pub fn add_item(&mut self, item: ShapeItem) {
        self.items.push(item);
    }

    /// The items, in draw order.
    pub fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    /// The items, mutably — for previewing a transform (a gizmo drag) or recolouring
    /// without rebuilding the list.
    pub fn items_mut(&mut self) -> &mut [ShapeItem] {
        &mut self.items
    }

    /// The number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the stage has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item carrying `entity`, if the stage has one.
    pub fn item(&self, entity: u32) -> Option<&ShapeItem> {
        self.items.iter().find(|item| item.entity_id() == entity)
    }

    /// The dress-up scrubber for a selection, when it names an **edge** of an item.
    ///
    /// This is the presentation half of the edge manipulator: the viewport already
    /// knows the selection and the stage, so the mapping from `Edge3` to the scrubber
    /// lives here rather than in a host. A face or vertex selection, an unknown entity
    /// or a mesh without a wire run returns `None`.
    pub fn scrubber_for(
        &self,
        selection: &SelectionRef,
        mode: DressUpMode,
    ) -> Option<DressUpScrubber> {
        let edge = selection.elements.first()?;
        if !edge.starts_with("Edge") {
            return None;
        }
        let points = self.item(selection.object)?.edge_polyline(edge)?;
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
            mode,
            edge.clone(),
            points,
        ))
    }

    /// Compile every item into `queue`, appending after whatever is already there (so a
    /// caller can merge a stage with an inventor-compiled queue in one pass).
    pub fn emit_batches(&self, queue: &mut RenderQueue) {
        for item in &self.items {
            item.emit(queue);
        }
    }

    /// Compile into a fresh queue.
    pub fn to_queue(&self) -> RenderQueue {
        let mut queue = RenderQueue::default();
        self.emit_batches(&mut queue);
        queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::DrawMode;
    use ferrocad_graphics::{PickId, RenderQueue, Topology};

    fn triangle(entity_id: u32) -> ShapeItem {
        ShapeItem::new(entity_id)
            .with_draw_mode(DrawMode::Shaded)
            .with_shape(
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![],
                vec![0, 1, 2],
                vec![],
            )
    }

    #[test]
    fn an_empty_stage_emits_nothing() {
        assert!(Stage::new().to_queue().is_empty());
    }

    #[test]
    fn emit_batches_appends_all_items() {
        let stage = Stage::new().with_item(triangle(1)).with_item(triangle(2));
        let queue = stage.to_queue();
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.packets[0].topology, Topology::Triangles);
        assert_eq!(queue.packets[0].state.id, PickId(1));
        assert_eq!(queue.packets[1].state.id, PickId(2));
    }

    #[test]
    fn emit_batches_appends_after_existing_packets() {
        // A stage emits into a queue that may already hold another producer's packets
        // (e.g. a compiled inventor scene): it appends, never replaces.
        let mut queue = RenderQueue::default();
        Stage::new().with_item(triangle(9)).emit_batches(&mut queue);
        assert_eq!(queue.len(), 1);
        Stage::new().with_item(triangle(1)).emit_batches(&mut queue);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.packets[0].state.id, PickId(9));
        assert_eq!(queue.packets[1].state.id, PickId(1));
    }

    /// A slab whose `Edge1` runs from (0,0,0) to (10,0,0), tagged `Face1`/`Edge1`.
    fn slab(entity: u32) -> ShapeItem {
        use ferrocad_geom::Tessellation;

        let mesh = Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: vec![0, 1],
            segment_parts: vec![1],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".to_string(), "Edge1".to_string()],
        };
        ShapeItem::from_tessellation(entity, &mesh)
    }

    #[test]
    fn item_finds_a_scan_by_entity() {
        let stage = Stage::new().with_item(slab(7)).with_item(triangle(8));
        assert_eq!(stage.item(7).map(|item| item.entity_id()), Some(7));
        assert!(stage.item(9).is_none());
    }

    #[test]
    fn an_edge_selection_makes_a_scrubber_over_the_edge() {
        use crate::dress_up::DressUpMode;
        use ferrocad_geom::SelectionRef;

        let stage = Stage::new().with_item(slab(7));
        let scrubber = stage
            .scrubber_for(&SelectionRef::element(7, "Edge1"), DressUpMode::Fillet)
            .expect("a scrubber");
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
        use crate::dress_up::DressUpMode;
        use ferrocad_geom::SelectionRef;

        let stage = Stage::new().with_item(slab(7));
        assert!(
            stage
                .scrubber_for(&SelectionRef::element(7, "Face1"), DressUpMode::Fillet)
                .is_none()
        );
        assert!(
            stage
                .scrubber_for(&SelectionRef::whole(7), DressUpMode::Fillet)
                .is_none()
        );
        assert!(
            stage
                .scrubber_for(&SelectionRef::element(9, "Edge1"), DressUpMode::Fillet)
                .is_none()
        );
        assert!(
            stage
                .scrubber_for(&SelectionRef::element(7, "Edge9"), DressUpMode::Fillet)
                .is_none()
        );
    }
}
