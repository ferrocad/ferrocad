//! The stage: an ordered collection of declarative visual items.

use ferrocad_graphics::RenderQueue;

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
}
