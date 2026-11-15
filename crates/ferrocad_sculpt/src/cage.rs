//! The cage point grip: grab a control point of a Sculpt cage and drag it.
//!
//! The Sculpt display hook publishes a cage's control points as stage **vertices**
//! whose parts are named `Point0`, `Point1`, … (see
//! `python/ferrocad_sculpt`'s display provider). This manipulator is the other half
//! of that contract: it arms on a cage selection, draws a screen-invariant marker at
//! every control point, and a drag of one marker commits the point's new world
//! position.
//!
//! ```text
//! press on a marker ──▶ drag in the camera plane ──▶ release
//!                                                    │
//!                       cage_point/move, Point3, [x, y, z]
//!                                                    ▼
//!                         the app script's `interaction` provider:
//!                         one transaction → CagePoints[3] = (x, y, z) → recompute,
//!                         which rebuilds the preview mesh and the linked skin
//! ```
//!
//! The manipulator names no document: it reads the points from the stage and hands
//! its commit to whoever registered the `cage_point` verb. That keeps the edition's
//! geometry knowledge in one place (the bundle) and the interaction seam generic.
//!
//! The drag is **screen-parallel**: the point moves in the plane through its press
//! position that faces the camera. That is what a vertex drag should do, and it needs
//! no axis to pick. The typed twin ([`Manipulator::numeric_fields`]) is X/Y/Z, so a
//! value can be typed or nudged instead of dragged.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::Camera;
use ferrocad_stage::gizmo::GizmoStyle;
use ferrocad_stage::screen::{camera_right, camera_up, project_device, scale_for};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, NumericField, ShapeItem, Stage};
use ferrocad_types::Vector3;

/// The interaction verb the app script handles (its `manipulator` name).
pub const CAGE_POINT_ID: &str = "cage_point";

/// The operation a move reports (the app script's `action`).
pub const CAGE_POINT_MOVE: &str = "move";

/// The prefix the display hook gives each control point's part name.
const POINT_PREFIX: &str = "Point";

/// The marker's radius, in device pixels.
const MARKER_PIXELS: f64 = 5.0;

/// The smallest grab radius, in device pixels.
const MIN_HIT_RADIUS_PX: f64 = 8.0;

/// The pick id of the marker overlay. It is never selectable, so it only has to stay
/// clear of document entity ids and the other overlays' ranges.
const MARKER_PICK_ID: u32 = 3_900_000;

/// One control point, as the stage publishes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlPoint {
    /// The index in the cage's own `CagePoints` order.
    pub index: usize,
    /// The world position the marker sits on.
    pub position: Vector3,
}

/// An active drag.
#[derive(Clone, Copy, Debug)]
struct CagePress {
    index: usize,
    /// The point's position when the drag began.
    origin: Vector3,
    /// The pointer position at the press, in device pixels.
    device: [f64; 2],
    /// The current position.
    current: Vector3,
}

/// A grip over one cage's control points.
#[derive(Clone, Debug)]
pub struct CagePointGrip {
    /// The stage entity the cage is drawn as.
    target: u32,
    points: Vec<ControlPoint>,
    press: Option<CagePress>,
    hovered: Option<usize>,
    style: GizmoStyle,
    markers: ShapeItem,
    preview: ShapeItem,
    /// The view and the positions the cached geometry was built for.
    view: Option<ferrocad_types::Matrix4>,
    built: Option<Vec<[f32; 3]>>,
}

impl CagePointGrip {
    /// A grip over `points` on stage entity `target`.
    pub fn new(target: u32, points: Vec<ControlPoint>) -> Self {
        let style = GizmoStyle::default();
        let ghost_alpha = style.ghost_alpha;
        let markers = ShapeItem::new(MARKER_PICK_ID)
            .with_draw_mode(DrawMode::Shaded)
            .with_selectable(false)
            .with_ribbon_aa();
        let preview = ShapeItem::new(MARKER_PICK_ID + 1)
            .with_draw_mode(DrawMode::Shaded)
            .with_selectable(false)
            .with_blended_surface()
            .with_ghost_surface()
            .with_ghost_alpha(ghost_alpha)
            .with_ribbon_aa();
        let mut grip = Self {
            target,
            points,
            press: None,
            hovered: None,
            style,
            markers,
            preview,
            view: None,
            built: None,
        };
        grip.apply_color();
        grip
    }

    /// The entity the grip edits.
    pub fn target_id(&self) -> u32 {
        self.target
    }

    /// The control points, in the cage's order.
    pub fn points(&self) -> &[ControlPoint] {
        &self.points
    }

    /// The point being dragged, if any.
    pub fn dragged(&self) -> Option<usize> {
        self.press.map(|press| press.index)
    }

    /// The marker item (drawn always) and the preview item (only while dragging).
    fn preview_item(&self) -> &ShapeItem {
        &self.preview
    }

    fn stroke_color(&self) -> [f32; 4] {
        let token = if self.press.is_some() || self.hovered.is_some() {
            self.style.active
        } else {
            self.style.inactive
        };
        [token[0], token[1], token[2], token[3] * self.style.opacity]
    }

    fn apply_color(&mut self) {
        let color = self.stroke_color();
        self.markers.set_surface_color(color);
        self.markers.set_edge_color(color);
        self.markers.set_ghost_alpha(self.style.ghost_alpha);
        let ghost = [
            self.style.active[0],
            self.style.active[1],
            self.style.active[2],
            self.style.active[3] * self.style.opacity * self.style.ghost_alpha,
        ];
        self.preview.set_surface_color(ghost);
        self.preview.set_edge_color(color);
        self.preview.set_ghost_alpha(self.style.ghost_alpha);
    }

    /// Apply the workspace gizmo style. Returns whether the style changed.
    pub fn set_style(&mut self, style: &GizmoStyle) -> bool {
        if self.style == *style {
            return false;
        }
        self.style = style.clone();
        self.built = None;
        self.apply_color();
        true
    }

    /// The world position of point `index`, while it is being dragged.
    fn position(&self, index: usize) -> Vector3 {
        match self.press {
            Some(press) if press.index == index => press.current,
            _ => self.points[index].position,
        }
    }

    /// The nearest control point to `device` within the grab radius.
    fn nearest(&self, camera: &Camera, device: [i32; 2]) -> Option<usize> {
        let pointer = [f64::from(device[0]), f64::from(device[1])];
        let slop = self.style.hit_radius_px.max(MIN_HIT_RADIUS_PX);
        let mut best: Option<(f64, usize)> = None;
        for point in &self.points {
            let Some(projected) = project_device(camera, point.position) else {
                continue;
            };
            let dx = projected[0] - pointer[0];
            let dy = projected[1] - pointer[1];
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= slop && best.is_none_or(|(best, _)| distance < best) {
                best = Some((distance, point.index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// Whether `device` is on a marker.
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        self.nearest(camera, device).is_some()
    }

    /// Begin a drag of the marker under `device`.
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(index) = self.nearest(camera, device) else {
            return false;
        };
        let origin = self.points[index].position;
        self.press = Some(CagePress {
            index,
            origin,
            device: [f64::from(device[0]), f64::from(device[1])],
            current: origin,
        });
        self.hovered = Some(index);
        self.built = None;
        self.apply_color();
        true
    }

    /// Advance the drag in the camera plane. Returns whether the value changed.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(press) = self.press else {
            return false;
        };
        let world_per_pixel = scale_for(camera, press.origin, 1.0).max(1e-12);
        let dx = (f64::from(device[0]) - press.device[0]) * world_per_pixel;
        let dy = (f64::from(device[1]) - press.device[1]) * world_per_pixel;
        let moved = press
            .origin
            .add(&camera_right(camera).scale(dx))
            .add(&camera_up(camera).scale(dy));
        if moved.sub(&press.current).length() <= 1e-12 {
            return false;
        }
        self.press = Some(CagePress {
            current: moved,
            ..press
        });
        true
    }

    /// End the drag, returning the commit (if the point moved).
    pub fn end_drag(&mut self) -> Option<InteractionCommit> {
        let press = self.press.take()?;
        self.built = None;
        self.apply_color();
        if press.current.sub(&press.origin).length() <= 1e-12 {
            return None;
        }
        Some(self.commit(press.index, press.current))
    }

    /// Cancel the drag. Returns whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        let _ = press;
        self.built = None;
        self.apply_color();
        true
    }

    fn element(index: usize) -> String {
        format!("{POINT_PREFIX}{index}")
    }

    fn commit(&self, index: usize, position: Vector3) -> InteractionCommit {
        InteractionCommit::new(CAGE_POINT_ID, self.target, CAGE_POINT_MOVE)
            .with_element(Self::element(index))
            .with_values([position.x, position.y, position.z])
            .with_readout(self.readout_for(index, position))
    }

    fn readout_for(&self, index: usize, position: Vector3) -> String {
        format!(
            "P{index}  {:.2}, {:.2}, {:.2} mm",
            position.x, position.y, position.z
        )
    }

    /// Rebuild the markers (and the preview) after a camera or value change.
    fn rebuild(&mut self, camera: &Camera) {
        let positions: Vec<[f32; 3]> = self
            .points
            .iter()
            .map(|point| {
                let position = self.position(point.index);
                [position.x as f32, position.y as f32, position.z as f32]
            })
            .collect();
        let radius = scale_for(
            camera,
            self.points
                .first()
                .map_or(Vector3::zero(), |point| point.position),
            MARKER_PIXELS,
        );
        let mut positions_out = Vec::new();
        let mut triangles = Vec::new();
        let mut parts = Vec::new();
        for (slot, point) in positions.iter().enumerate() {
            push_marker(
                *point,
                radius,
                slot as u32,
                &mut positions_out,
                &mut triangles,
                &mut parts,
            );
        }
        self.markers
            .set_mesh(positions_out, Vec::new(), triangles, Vec::new(), parts);

        // The preview: a line from the press position to the dragged point, plus the
        // dragged point's own marker.
        let mut preview_positions = Vec::new();
        let mut preview_triangles = Vec::new();
        let mut preview_parts = Vec::new();
        if let Some(press) = self.press {
            let from = press.origin;
            let to = press.current;
            push_segment(
                [from.x as f32, from.y as f32, from.z as f32],
                [to.x as f32, to.y as f32, to.z as f32],
                radius * 0.25,
                &mut preview_positions,
                &mut preview_triangles,
                &mut preview_parts,
            );
            push_marker(
                [to.x as f32, to.y as f32, to.z as f32],
                radius * 1.5,
                1,
                &mut preview_positions,
                &mut preview_triangles,
                &mut preview_parts,
            );
        }
        self.preview.set_mesh(
            preview_positions,
            Vec::new(),
            preview_triangles,
            Vec::new(),
            preview_parts,
        );
        self.view = Some(camera.view);
        self.built = Some(positions);
    }
}

/// Push one screen-invariant octahedron marker (`part` tags its triangles).
fn push_marker(
    centre: [f32; 3],
    radius: f64,
    part: u32,
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<u32>,
    parts: &mut Vec<u32>,
) {
    let base = positions.len() as u32;
    let r = radius as f32;
    let [x, y, z] = centre;
    positions.extend([
        [x + r, y, z],
        [x - r, y, z],
        [x, y + r, z],
        [x, y - r, z],
        [x, y, z + r],
        [x, y, z - r],
    ]);
    for (a, b, c) in [
        (0, 2, 4),
        (2, 1, 4),
        (1, 3, 4),
        (3, 0, 4),
        (2, 0, 5),
        (1, 2, 5),
        (3, 1, 5),
        (0, 3, 5),
    ] {
        triangles.extend([base + a, base + b, base + c]);
        parts.push(part);
    }
}

/// Push one thin screen-facing quad as the drag's rubber line.
fn push_segment(
    from: [f32; 3],
    to: [f32; 3],
    half_width: f64,
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<u32>,
    parts: &mut Vec<u32>,
) {
    // A screen-parallel drag is drawn in a plane that faces the camera only
    // approximately; a flat quad between the two points, widened sideways in world X/Y
    // of the segment, is enough for a readout line and never picks.
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let length = (dx * dx + dy * dy).sqrt().max(1e-9);
    let nx = (-dy / length) * half_width as f32;
    let ny = (dx / length) * half_width as f32;
    let base = positions.len() as u32;
    positions.extend([
        [from[0] + nx, from[1] + ny, from[2]],
        [from[0] - nx, from[1] - ny, from[2]],
        [to[0] - nx, to[1] - ny, to[2]],
        [to[0] + nx, to[1] + ny, to[2]],
    ]);
    triangles.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    parts.extend([0, 0]);
}

impl Manipulator for CagePointGrip {
    fn id(&self) -> &str {
        CAGE_POINT_ID
    }

    fn items(&self) -> &[ShapeItem] {
        std::slice::from_ref(&self.markers)
    }

    fn preview_items(&self) -> &[ShapeItem] {
        std::slice::from_ref(self.preview_item())
    }

    fn is_dragging(&self) -> bool {
        self.press.is_some()
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        CagePointGrip::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        CagePointGrip::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        let moved = CagePointGrip::drag_to(self, camera, device);
        if moved {
            self.rebuild(camera);
        }
        moved
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        CagePointGrip::end_drag(self)
    }

    fn drag_cancel(&mut self) -> bool {
        CagePointGrip::cancel_drag(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        let next = if hovered { self.hovered } else { None };
        if self.hovered == next {
            return false;
        }
        self.hovered = next;
        self.apply_color();
        true
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.rebuild(camera);
        true
    }

    fn set_style(&mut self, style: &GizmoStyle) -> bool {
        CagePointGrip::set_style(self, style)
    }

    fn anchor(&self) -> Option<Vector3> {
        let index = self.dragged().or(self.hovered)?;
        Some(self.position(index))
    }

    fn readout(&self) -> Option<String> {
        let press = self.press?;
        Some(self.readout_for(press.index, press.current))
    }

    fn numeric_fields(&self) -> Vec<NumericField> {
        let Some(index) = self.dragged().or(self.hovered) else {
            return Vec::new();
        };
        let position = self.position(index);
        vec![
            NumericField::millimetres("X", position.x),
            NumericField::millimetres("Y", position.y),
            NumericField::millimetres("Z", position.z),
        ]
    }

    fn set_values(&mut self, values: &[f64]) -> bool {
        let (Some(&x), Some(&y), Some(&z)) = (values.first(), values.get(1), values.get(2)) else {
            return false;
        };
        let Some(index) = self.dragged().or(self.hovered) else {
            return false;
        };
        let origin = match self.press {
            Some(press) => press.origin,
            None => self.points[index].position,
        };
        let current = Vector3::new(x, y, z);
        if current.sub(&self.position(index)).length() <= 1e-12 {
            return false;
        }
        self.press = Some(CagePress {
            index,
            origin,
            device: [0.0, 0.0],
            current,
        });
        true
    }

    fn pending_commit(&self) -> Option<InteractionCommit> {
        let press = self.press?;
        if press.current.sub(&press.origin).length() <= 1e-12 {
            return None;
        }
        Some(self.commit(press.index, press.current))
    }
}

/// The Sculpt edition's manipulator factory.
///
/// Arms when the selection's item publishes cage control points (stage vertices whose
/// part names are `Point*`): the display hook is the only thing that produces those,
/// so a Part object, a mesh or the toy slab never arms this grip.
pub fn cage_manipulator(selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>> {
    let item = stage.item(selection.object)?;
    let points: Vec<ControlPoint> = item
        .vertices()
        .iter()
        .filter_map(|position| {
            let part = *item.parts().get(*position as usize)?;
            let name = item.part_name(part)?;
            let index = name.strip_prefix(POINT_PREFIX)?.parse::<usize>().ok()?;
            let [x, y, z] = *item.positions().get(*position as usize)?;
            Some(ControlPoint {
                index,
                position: Vector3::new(f64::from(x), f64::from(y), f64::from(z)),
            })
        })
        .collect();
    if points.is_empty() {
        return None;
    }
    Some(Box::new(CagePointGrip::new(selection.object, points)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_stage::ShapeItem;
    use ferrocad_types::{Placement, Rotation};

    fn camera() -> Camera {
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -100.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 1000.0,
            },
            viewport: Viewport::new([400, 400], 1.0),
        }
    }

    /// A stage item shaped the way the Sculpt display hook shapes a cage: the surface
    /// triangles in part 0 and the control points as `Point*` vertices.
    fn cage_item() -> ShapeItem {
        let mut item = ShapeItem::new(7);
        item.set_tessellation(&ferrocad_geom::Tessellation {
            positions: vec![
                // Two surface triangles at part 0… (a quad's corners)
                [-10.0, -10.0, 0.0],
                [10.0, -10.0, 0.0],
                [10.0, 10.0, 0.0],
                [-10.0, 10.0, 0.0],
                // …then two control points.
                [0.0, 0.0, 0.0],
                [5.0, 5.0, 10.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3],
            triangle_parts: vec![0, 0],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: vec![4, 5],
            vertex_parts: vec![1, 2],
            parts: vec!["".to_string(), "Point0".to_string(), "Point1".to_string()],
        });
        item
    }

    fn stage_with_cage() -> Stage {
        Stage::new().with_item(cage_item())
    }

    #[test]
    fn the_factory_reads_the_published_control_points() {
        let stage = stage_with_cage();
        let mut grip = cage_manipulator(&SelectionRef::whole(7), &stage).expect("a cage arms");
        assert_eq!(grip.id(), CAGE_POINT_ID);
        // The overlay is screen-invariant, so it is built on the first `refresh`.
        grip.refresh(&camera());
        // Two control points, each an eight-triangle marker: the surface itself is
        // not a control point, so it contributes nothing to the overlay.
        assert_eq!(grip.items()[0].triangles().len(), 2 * 8 * 3);
    }

    #[test]
    fn the_grip_keeps_the_cage_order_and_positions() {
        let grip = CagePointGrip::new(
            7,
            vec![
                ControlPoint {
                    index: 0,
                    position: Vector3::new(0.0, 0.0, 0.0),
                },
                ControlPoint {
                    index: 1,
                    position: Vector3::new(5.0, 5.0, 10.0),
                },
            ],
        );
        assert_eq!(grip.points().len(), 2);
        assert_eq!(grip.points()[1].index, 1);
        assert_eq!(grip.points()[1].position, Vector3::new(5.0, 5.0, 10.0));
    }

    #[test]
    fn a_part_shape_does_not_arm_the_grip() {
        let mut stage = Stage::new();
        let mut item = ShapeItem::new(9);
        item.set_tessellation(&ferrocad_geom::Tessellation {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Vec::new(),
            triangles: vec![0, 1, 2],
            triangle_parts: vec![0],
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: vec![0, 1, 2],
            vertex_parts: vec![0, 0, 0],
            parts: vec!["Vertex1".to_string()],
        });
        stage.add_item(item);
        assert!(cage_manipulator(&SelectionRef::whole(9), &stage).is_none());
    }

    #[test]
    fn a_drag_commits_the_point_in_world_space() {
        let camera = camera();
        let stage = stage_with_cage();
        let mut grip = cage_manipulator(&SelectionRef::whole(7), &stage).expect("a cage arms");
        let _ = grip.refresh(&camera);

        // The first control point projects to the camera's centre (400x400 device).
        let device = project_device(&camera, Vector3::new(0.0, 0.0, 0.0)).expect("visible");
        assert!(grip.hit(&camera, [device[0] as i32, device[1] as i32]));
        assert!(grip.drag_begin(&camera, [device[0] as i32, device[1] as i32]));

        // A right-ward drag of 20 px moves the point by 20 px worth of world units.
        let moved = grip.drag_update(&camera, [(device[0] + 20.0) as i32, device[1] as i32]);
        assert!(moved, "the drag moves the point");

        let commit = grip.drag_end().expect("the drag commits");
        assert_eq!(commit.manipulator, CAGE_POINT_ID);
        assert_eq!(commit.action, CAGE_POINT_MOVE);
        assert_eq!(commit.target, 7);
        assert_eq!(commit.element.as_deref(), Some("Point0"));
        assert_eq!(commit.values.len(), 3);
        // The x coordinate follows the drag; the point stays in the camera plane.
        assert!(commit.values[0] > 0.0, "{:?}", commit.values);
        assert!(commit.values[0] < 20.0, "{:?}", commit.values);
        assert!(commit.values[2].abs() < 1e-6, "{:?}", commit.values);
    }

    #[test]
    fn a_press_away_from_every_marker_misses() {
        let camera = camera();
        let stage = stage_with_cage();
        let mut grip = cage_manipulator(&SelectionRef::whole(7), &stage).expect("a cage arms");
        assert!(!grip.hit(&camera, [5, 5]));
        assert!(!grip.drag_begin(&camera, [5, 5]));
        assert!(grip.drag_end().is_none());
    }

    #[test]
    fn a_typed_value_is_the_numeric_twin_of_the_drag() {
        let camera = camera();
        let stage = stage_with_cage();
        let mut grip = cage_manipulator(&SelectionRef::whole(7), &stage).expect("a cage arms");
        let device = project_device(&camera, Vector3::new(0.0, 0.0, 0.0)).expect("visible");
        grip.drag_begin(&camera, [device[0] as i32, device[1] as i32]);

        let fields = grip.numeric_fields();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].label, "X");

        assert!(grip.set_values(&[4.0, -2.0, 6.0]));
        let commit = grip.pending_commit().expect("a typed value is a commit");
        assert_eq!(commit.values, vec![4.0, -2.0, 6.0]);
        assert_eq!(commit.element.as_deref(), Some("Point0"));
    }
}
