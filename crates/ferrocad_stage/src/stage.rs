//! The stage: an ordered collection of declarative visual items.

use ferrocad_graphics::{RenderQueue, ShadowFootprint};
use ferrocad_types::Vector3;

use crate::shape::ShapeItem;
use crate::workplane::Workplane;

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

/// How soft an item's ground shadow is, as a fraction of its larger half-extent.
const SHADOW_BLUR_FRACTION: f32 = 0.25;
/// The smallest world-space shadow blur, so a tiny item still has a soft edge.
const SHADOW_MIN_BLUR: f32 = 1e-3;

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

    /// The ground-shadow footprint of each item, in world `(x, y)`, for the fixed
    /// program ABI's shadow block.
    ///
    /// Shorthand for [`shadow_footprints_on`](Self::shadow_footprints_on) with the world
    /// `XY` plane. A host that draws a bound workplane should pass that plane instead, so
    /// the shadow lands on the plane the grid is drawn on.
    pub fn shadow_footprints(&self) -> Vec<ShadowFootprint> {
        self.shadow_footprints_on(&Workplane::world())
    }

    /// The ground-shadow footprint of each item **in `plane`'s `(u, v)` frame**, for the fixed
    /// program ABI's shadow block.
    ///
    /// One slot per item that has positions; an empty stage yields an empty list (the
    /// shader's loop then does not run). Each vertex of the item's displayed geometry (its
    /// placement composed with any temporary preview transform) is projected onto the
    /// plane, and the footprint is the `(u, v)` AABB: on the world `XY` ground that is the
    /// familiar `(x, y)` box, and on a vertical plane it is that plane's own extent. The
    /// shader therefore shades whatever plane the grid is on, not only the ground.
    ///
    /// `blur` is derived from the footprint, not supplied: a quarter of the larger
    /// half-extent (at least [`SHADOW_MIN_BLUR`] world units), so a small part gets a
    /// proportionally tight shadow and a large one does not smear across the grid.
    ///
    /// Feed the result to [`ferrocad_graphics::apply_shadows`], which enforces the
    /// 64-slot cap and writes `shadow_count`.
    pub fn shadow_footprints_on(&self, plane: &Workplane) -> Vec<ShadowFootprint> {
        let mut footprints = Vec::with_capacity(self.items.len());
        for item in &self.items {
            let positions = item.positions();
            if positions.is_empty() {
                continue;
            }
            let mut world = item.placement().to_matrix();
            if let Some(temporary) = item.temporary_transform() {
                world = world.mul(&temporary);
            }
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            for position in positions {
                let point = world.transform(&Vector3::new(
                    f64::from(position[0]),
                    f64::from(position[1]),
                    f64::from(position[2]),
                ));
                let relative = point.sub(&plane.origin);
                let u = relative.dot(&plane.u);
                let v = relative.dot(&plane.v);
                min[0] = min[0].min(u);
                min[1] = min[1].min(v);
                max[0] = max[0].max(u);
                max[1] = max[1].max(v);
            }
            let half = [
                ((max[0] - min[0]) * 0.5) as f32,
                ((max[1] - min[1]) * 0.5) as f32,
            ];
            let blur = (SHADOW_BLUR_FRACTION * half[0].max(half[1])).max(SHADOW_MIN_BLUR);
            footprints.push(ShadowFootprint::from_bounds(
                [min[0] as f32, min[1] as f32],
                [max[0] as f32, max[1] as f32],
                blur,
            ));
        }
        footprints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::DrawMode;
    use ferrocad_graphics::{PickId, RenderQueue, Topology};
    use ferrocad_types::{Placement, Rotation};

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

    /// A flat box in the ground plane, spanning `x .. x + width` and `y .. y + depth`.
    fn box_at(x: f32, y: f32, width: f32, depth: f32) -> ShapeItem {
        ShapeItem::new(1).with_shape(
            vec![
                [x, y, 0.0],
                [x + width, y, 0.0],
                [x + width, y + depth, 0.0],
                [x, y + depth, 0.0],
            ],
            vec![],
            vec![],
            vec![],
        )
    }

    #[test]
    fn an_empty_stage_has_no_shadow_footprints() {
        assert!(Stage::new().shadow_footprints().is_empty());
        assert!(
            Stage::new()
                .with_item(ShapeItem::new(1))
                .shadow_footprints()
                .is_empty(),
            "an item with no tessellation casts no shadow"
        );
    }

    /// A footprint is measured in the plane's own frame, so a vertical workplane shades
    /// with its `(u, v)` extents rather than the world `(x, y)` box.
    #[test]
    fn a_footprint_is_measured_in_the_planes_frame() {
        use crate::StandardPlane;

        // A 2 × 4 × 6 box from the origin.
        let item = ShapeItem::new(1).with_shape(
            vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
                [0.0, 0.0, 6.0],
                [2.0, 0.0, 6.0],
                [2.0, 4.0, 6.0],
                [0.0, 4.0, 6.0],
            ],
            vec![],
            vec![],
            vec![],
        );
        let stage = Stage::new().with_item(item);

        let ground = stage.shadow_footprints();
        assert_eq!(ground[0].center, [1.0, 2.0], "world XY: the (x, y) box");

        let front = stage.shadow_footprints_on(&Workplane::standard(StandardPlane::Front));
        assert_eq!(front[0].center, [1.0, 3.0], "XZ: u = x, v = z");
        assert_eq!(front[0].half_extent, [1.0, 3.0]);

        let side = stage.shadow_footprints_on(&Workplane::standard(StandardPlane::Right));
        assert_eq!(side[0].center, [2.0, 3.0], "YZ: u = y, v = z");
        assert_eq!(side[0].half_extent, [2.0, 3.0]);
    }

    #[test]
    fn shadow_footprints_are_one_world_xy_box_per_item() {
        let stage = Stage::new()
            .with_item(box_at(-5.0, -5.0, 4.0, 2.0))
            .with_item(box_at(2.0, 3.0, 2.0, 4.0));

        let footprints = stage.shadow_footprints();
        assert_eq!(footprints.len(), 2);
        assert_eq!(footprints[0].center, [-3.0, -4.0]);
        assert_eq!(footprints[0].half_extent, [2.0, 1.0]);
        assert_eq!(footprints[1].center, [3.0, 5.0]);
        assert_eq!(footprints[1].half_extent, [1.0, 2.0]);

        // A quarter of the larger half-extent, so the softness follows the size.
        assert_eq!(footprints[0].blur, 0.5);
        assert_eq!(footprints[1].blur, 0.5);
    }

    #[test]
    fn shadow_footprints_follow_the_displayed_placement() {
        let item = box_at(0.0, 0.0, 2.0, 2.0).with_placement(Placement::new(
            Vector3::new(10.0, 20.0, 0.0),
            Rotation::identity(),
        ));
        let footprints = Stage::new().with_item(item).shadow_footprints();
        assert_eq!(footprints.len(), 1);
        assert_eq!(footprints[0].center, [11.0, 21.0]);
        assert_eq!(footprints[0].half_extent, [1.0, 1.0]);

        // A temporary preview transform is part of what is displayed, so it counts too.
        let preview = box_at(0.0, 0.0, 2.0, 2.0)
            .with_temporary_transform(Some(Placement::new(
                Vector3::new(-4.0, 0.0, 0.0),
                Rotation::identity(),
            ).to_matrix()));
        let footprints = Stage::new().with_item(preview).shadow_footprints();
        assert_eq!(footprints[0].center, [-3.0, 1.0]);
    }

    #[test]
    fn stage_footprints_fill_the_program_shadow_block() {
        use ferrocad_graphics::{ProgramParameters, apply_shadows};

        let stage = Stage::new()
            .with_item(box_at(-5.0, -5.0, 4.0, 4.0))
            .with_item(box_at(2.0, 2.0, 2.0, 2.0));
        let mut parameters = ProgramParameters::default();

        assert_eq!(apply_shadows(&mut parameters, stage.shadow_footprints()), 2);
        assert_eq!(parameters.shadow_count, 2.0);
        assert!(parameters.shadow_data[..3].iter().flatten().any(|value| *value != 0.0));
    }
}
