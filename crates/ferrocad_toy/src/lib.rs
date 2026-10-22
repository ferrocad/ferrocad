//! # FerroCAD Toy (`ferrocad_toy`)
//!
//! A **toy workbench** that exercises the generic interaction model without a kernel.
//! It exists to prove that a manipulator is a workbench contribution, not a shell or
//! stage feature:
//!
//! ```text
//! ferrocad_stage      Manipulator · InteractionCommit                     (generic)
//!       ▲
//! ferrocad_toy        HeightGrip · height_grip                            (this crate)
//!       ▲
//! its app script      display_mesh (a cube sized by Height) · interaction (writes it)
//! ```
//!
//! The document object is a plain `App::FeaturePython` with a `Height` length
//! property. The app script generates the display mesh and implements the
//! `toy_height` interaction; this crate supplies the draggable grip. The edition
//! binary registers it exactly as the Part edition registers its fillet arc, so the
//! shell routes it without naming either workbench.
//!
//! The toy is also the cheapest full-loop test: no OCCT, no shape kernel, just the
//! interpreter, a mesh generator and a property.
//!
//! See `docs/workbenches/toy-workbench.md` and `docs/architecture.md` §10.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::{Camera, screen_ribbon};
use ferrocad_stage::gizmo::{ACTIVE_COLOR, INACTIVE_COLOR};
use ferrocad_stage::screen::{
    HIT_SLOP, STROKE_PIXELS, camera_forward, point_segment_distance, project_device, push_ribbon,
    scale_for,
};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, ShapeItem, Stage};
use ferrocad_stage::{GizmoMode, TransformGizmo};
use ferrocad_types::{Matrix4, Vector3};

/// The manipulator's registry id (`InteractionCommit::manipulator`).
pub const TOY_HEIGHT_ID: &str = "toy_height";

/// The grip's picking id. It is drawn as an overlay, so it only has to stay clear of
/// document and gizmo ids.
pub const TOY_GRIP_PICK_ID: u32 = 3_000_000;

/// The smallest height the grip will produce.
pub const MIN_HEIGHT: f64 = 0.1;

/// The grip stem's screen-invariant length, in device pixels.
const STEM_PIXELS: f64 = 44.0;
/// The dark casing around the grip stroke, in device pixels (the stroke itself is
/// [`STROKE_PIXELS`] wide).
const CASING_PIXELS: f64 = 3.0;
/// The dark casing ink: the canvas void, so the grip reads against a bright selected
/// slab as well as against the dark canvas.
const CASING_COLOR: [f32; 4] = [0.004777, 0.006049, 0.009134, 1.0];

/// A vertical grip that drags the slab's `Height` property.
///
/// The anchor is the top centre of the displayed item; the handle is a screen-invariant
/// stem projecting **up** from that anchor into empty space plus a cross at its tip.
/// Dragging up grows the height by the pointer's **vertical travel**, in screen-invariant
/// world units.
#[derive(Clone, Debug)]
pub struct HeightGrip {
    target: u32,
    /// Top centre of the item, in world space.
    top: Vector3,
    height: f64,
    /// `(press device y, height at press)` while a drag is active.
    press: Option<(f64, f64)>,
    hovered: bool,
    /// The camera view and height the cached stroke was built for.
    view: Option<Matrix4>,
    built: Option<f64>,
    /// The draw order: the dark casing first, the coloured stroke on top.
    items: [ShapeItem; 2],
}

impl HeightGrip {
    /// A grip for the item whose top centre is `top`, starting at `height`.
    pub fn new(target: u32, top: Vector3, height: f64) -> Self {
        Self {
            target,
            top,
            height: height.max(MIN_HEIGHT),
            press: None,
            hovered: false,
            view: None,
            built: None,
            items: [
                ShapeItem::new(TOY_GRIP_PICK_ID)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false)
                    .with_surface_color(CASING_COLOR)
                    .with_edge_color(CASING_COLOR),
                ShapeItem::new(TOY_GRIP_PICK_ID + 1)
                    .with_draw_mode(DrawMode::Shaded)
                    .with_selectable(false),
            ],
        }
    }

    /// The entity the grip edits.
    pub fn target_id(&self) -> u32 {
        self.target
    }

    /// The current height.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// The grip's world anchor (the item's top centre).
    pub fn top(&self) -> Vector3 {
        self.top
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.press.is_some()
    }

    /// Whether the grip is highlighted.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// The overlay items, casing first so the stroke sits on top of it.
    pub fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    /// The readout text (`H 20.00 mm`).
    pub fn readout(&self) -> String {
        format!("H {:.2} mm", self.height)
    }

    /// Highlight (or un-highlight) the grip. Returns whether the visuals changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.apply_color();
        true
    }

    fn stroke_color(&self) -> [f32; 4] {
        if self.hovered || self.is_dragging() {
            ACTIVE_COLOR
        } else {
            INACTIVE_COLOR
        }
    }

    fn apply_color(&mut self) {
        let color = self.stroke_color();
        self.items[1].set_surface_color(color);
        self.items[1].set_edge_color(color);
    }

    /// Whether `device` is on the grip (the top anchor or the stem, within the shared
    /// grab slop).
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(anchor) = project_device(camera, self.top) else {
            return false;
        };
        let Some(tip) = project_device(camera, self.stem_end(camera)) else {
            return false;
        };
        let point = [f64::from(device[0]), f64::from(device[1])];
        point_segment_distance(point, anchor, tip).0 <= HIT_SLOP
    }

    /// Begin a drag at `device`.
    pub fn begin_drag(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        if !self.hit(camera, device) {
            return false;
        }
        self.press = Some((f64::from(device[1]), self.height));
        self.apply_color();
        true
    }

    /// Advance the drag. The device origin is bottom-left, so a larger `y` is **up**.
    pub fn drag_to(&mut self, camera: &Camera, device: [i32; 2]) -> Option<f64> {
        let (press_y, press_height) = self.press?;
        let world_per_pixel = scale_for(camera, self.top, 1.0).max(1e-12);
        let height =
            (press_height + (f64::from(device[1]) - press_y) * world_per_pixel).max(MIN_HEIGHT);
        if (height - self.height).abs() <= 1e-12 {
            return None;
        }
        self.height = height;
        Some(height)
    }

    /// End the drag, returning the value to commit.
    pub fn end_drag(&mut self) -> Option<InteractionCommit> {
        self.press.take()?;
        Some(
            InteractionCommit::new(TOY_HEIGHT_ID, self.target, "set")
                .with_values([self.height])
                .with_readout(self.readout()),
        )
    }

    /// Cancel the drag, restoring the press value. Returns whether a drag was active.
    pub fn cancel_drag(&mut self) -> bool {
        let Some((_, height)) = self.press.take() else {
            return false;
        };
        self.height = height;
        self.apply_color();
        true
    }

    /// Rebuild the stroke when the view or the value changed.
    pub fn update(&mut self, camera: &Camera) -> bool {
        if self.view == Some(camera.view) && self.built == Some(self.height) {
            return false;
        }
        self.view = Some(camera.view);
        self.built = Some(self.height);
        self.build(camera);
        true
    }

    /// The stem's endpoint: `STEM_PIXELS` of screen-invariant travel straight up from
    /// the top face, into empty space.
    fn stem_end(&self, camera: &Camera) -> Vector3 {
        let length = scale_for(camera, self.top, STEM_PIXELS);
        Vector3::new(self.top.x, self.top.y, self.top.z + length)
    }

    /// Rebuild the geometry: a 1.5 px ribbon from the top centre up into empty space,
    /// plus a crossbar at its tip, each over a wider dark casing. The stem, crossbar and
    /// casing are screen-invariant, so the target stays grabbable at any zoom.
    fn build(&mut self, camera: &Camera) {
        let stem_end = self.stem_end(camera);
        let crossbar_half = scale_for(camera, self.top, 5.0);
        let stroke_half = scale_for(camera, self.top, STROKE_PIXELS / 2.0);
        let casing_half = scale_for(camera, self.top, (STROKE_PIXELS + CASING_PIXELS) / 2.0);
        let forward = camera_forward(camera);
        let left = Vector3::new(
            stem_end.x - crossbar_half,
            stem_end.y,
            stem_end.z,
        );
        let right = Vector3::new(
            stem_end.x + crossbar_half,
            stem_end.y,
            stem_end.z,
        );

        let ribbons = |half: f64| {
            let mut positions = Vec::new();
            let mut triangles = Vec::new();
            push_ribbon(
                &mut positions,
                &mut triangles,
                screen_ribbon(self.top, stem_end, half, forward),
            );
            push_ribbon(
                &mut positions,
                &mut triangles,
                screen_ribbon(left, right, half, forward),
            );
            (positions, triangles)
        };

        let (stroke_positions, stroke_triangles) = ribbons(stroke_half);
        let (casing_positions, casing_triangles) = ribbons(casing_half);
        // The stroke is coplanar with the casing; nudge it half a pixel toward the eye so
        // the depth test lets the coloured ribbon win deterministically.
        let toward_eye = camera_forward(camera).scale(-scale_for(camera, self.top, 0.5));
        let stroke_positions: Vec<[f32; 3]> = stroke_positions
            .into_iter()
            .map(|point| {
                [
                    point[0] + toward_eye.x as f32,
                    point[1] + toward_eye.y as f32,
                    point[2] + toward_eye.z as f32,
                ]
            })
            .collect();
        self.items[0].set_shape(
            casing_positions,
            Vec::new(),
            casing_triangles,
            Vec::new(),
        );
        self.items[1].set_shape(
            stroke_positions,
            Vec::new(),
            stroke_triangles,
            Vec::new(),
        );
        self.apply_color();
    }

    /// Where the readout sits, in device pixels: above the stem's tip.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.stem_end(camera))
    }
}

impl Manipulator for HeightGrip {
    fn id(&self) -> &'static str {
        TOY_HEIGHT_ID
    }

    fn items(&self) -> &[ShapeItem] {
        HeightGrip::items(self)
    }

    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::hit(self, camera, device)
    }

    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::begin_drag(self, camera, device)
    }

    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool {
        HeightGrip::drag_to(self, camera, device).is_some()
    }

    fn drag_end(&mut self) -> Option<InteractionCommit> {
        HeightGrip::end_drag(self)
    }

    fn drag_cancel(&mut self) -> bool {
        HeightGrip::cancel_drag(self)
    }

    fn is_dragging(&self) -> bool {
        HeightGrip::is_dragging(self)
    }

    fn set_hovered(&mut self, hovered: bool) -> bool {
        HeightGrip::set_hovered(self, hovered)
    }

    fn refresh(&mut self, camera: &Camera) -> bool {
        self.update(camera)
    }

    fn readout(&self) -> Option<String> {
        Some(HeightGrip::readout(self))
    }

    fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        HeightGrip::readout_anchor(self, camera)
    }
}

/// The world-space bounds `(min, max)` of a stage item's positions.
fn bounds(item: &ShapeItem) -> Option<(Vector3, Vector3)> {
    let positions = item.positions();
    if positions.is_empty() {
        return None;
    }
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for [x, y, z] in positions {
        min.x = min.x.min(f64::from(*x));
        min.y = min.y.min(f64::from(*y));
        min.z = min.z.min(f64::from(*z));
        max.x = max.x.max(f64::from(*x));
        max.y = max.y.max(f64::from(*y));
        max.z = max.z.max(f64::from(*z));
    }
    Some((min, max))
}

/// The toy workbench's manipulator factory: a **whole-object** selection arms a height
/// grip over the item's top centre.
///
/// Unlike the Part factory (which wants an `EdgeN`), the toy has no sub-elements, so it
/// claims whole-object selections only. The registry does not care which rule a
/// workbench uses.
pub fn height_grip(selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>> {
    if !selection.is_whole() {
        return None;
    }
    let item = stage.item(selection.object)?;
    let (min, max) = bounds(item)?;
    let top = Vector3::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, max.z);
    Some(Box::new(HeightGrip::new(
        selection.object,
        top,
        max.z - min.z,
    )))
}

/// The toy workbench's gizmo factory: a **whole-object** selection gets a translate
/// gizmo at the item's centre.
///
/// The gizmo is the generic move/rotate handle; whether an object responds is the
/// workbench's decision. The toy offers translation (its document provider composes the
/// committed matrix with the object's placement).
pub fn move_gizmo(selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
    if !selection.is_whole() {
        return None;
    }
    let item = stage.item(selection.object)?;
    let (min, max) = bounds(item)?;
    let centre = min.add(&max).scale(0.5);
    Some(TransformGizmo::new(
        selection.object,
        centre,
        GizmoMode::Translate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::{Projection, Viewport};
    use ferrocad_stage::ShapeItem;
    use ferrocad_types::{Placement, Rotation};

    fn camera(eye_z: f64) -> Camera {
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0f64.to_radians(),
                near: 0.1,
                far: 1000.0,
            },
            viewport: Viewport::new([400, 400], 1.0),
        }
    }

    /// A 30° pitched view, so the vertical stem projects into a visible screen segment
    /// instead of collapsing onto the view axis, and world +Z reads as screen-up.
    fn tilted_camera(eye_z: f64) -> Camera {
        let rotation = Rotation::from_axis_angle(
            &Vector3::new(1.0, 0.0, 0.0),
            (-30.0_f64).to_radians(),
        );
        Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -eye_z), rotation).to_matrix(),
            ..camera(eye_z)
        }
    }

    /// A 40 × 40 × 20 slab from z = 0, centred on the origin in x/y.
    fn slab(entity: u32) -> Stage {
        use ferrocad_geom::Tessellation;

        let mesh = Tessellation {
            positions: vec![
                [-20.0, -20.0, 0.0],
                [20.0, -20.0, 0.0],
                [20.0, 20.0, 0.0],
                [-20.0, 20.0, 0.0],
                [-20.0, -20.0, 20.0],
                [20.0, -20.0, 20.0],
                [20.0, 20.0, 20.0],
                [-20.0, 20.0, 20.0],
            ],
            normals: Vec::new(),
            triangles: vec![0, 1, 2, 0, 2, 3],
            triangle_parts: vec![0, 0],
            edges: vec![0, 1],
            segment_parts: vec![0],
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: Vec::new(),
        };
        Stage::new().with_item(ShapeItem::from_tessellation(entity, &mesh))
    }

    /// The grip at `(0, 0, 20)` over the slab.
    fn grip() -> HeightGrip {
        HeightGrip::new(7, Vector3::new(0.0, 0.0, 20.0), 20.0)
    }

    #[test]
    fn the_factory_arms_on_a_whole_selection_and_reads_the_top_centre() {
        let stage = slab(7);
        let grip = height_grip(&SelectionRef::whole(7), &stage).expect("a grip");
        assert_eq!(grip.id(), TOY_HEIGHT_ID);
        let camera = tilted_camera(200.0);
        let top = Vector3::new(0.0, 0.0, 20.0);
        let stem_end = Vector3::new(0.0, 0.0, top.z + scale_for(&camera, top, STEM_PIXELS));
        assert_eq!(
            grip.readout_anchor(&camera),
            project_device(&camera, stem_end)
        );
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let face = project_device(&camera, top).expect("the top face");
        assert!(anchor[1] > face[1], "the readout floats above the stem");
        assert!(grip.readout().expect("a readout").starts_with("H "));
    }

    #[test]
    fn the_stem_projects_up_from_the_top_into_empty_space() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        assert!(grip.update(&camera));

        // Casing first, then the 1.5 px stroke; each is a stem ribbon and a crossbar
        // ribbon, so two quads (8 vertices, 12 indices) per item.
        assert_eq!(grip.items().len(), 2, "casing + stroke");
        let casing = &grip.items()[0];
        let stroke = &grip.items()[1];
        for item in [casing, stroke] {
            assert_eq!(item.positions().len(), 8, "two ribbon quads");
            assert_eq!(item.triangles().len(), 12, "six indices per quad");
        }
        assert_eq!(casing.surface_color(), CASING_COLOR);
        assert_eq!(stroke.surface_color(), INACTIVE_COLOR);

        // The geometry spans the top-face centroid up to the stem tip; neither ribbon
        // reaches down to the slab's base.
        let top = grip.top();
        let expected = scale_for(&camera, top, STEM_PIXELS);
        let min_z = stroke
            .positions()
            .iter()
            .map(|point| f64::from(point[2]))
            .fold(f64::INFINITY, f64::min);
        let max_z = stroke
            .positions()
            .iter()
            .map(|point| f64::from(point[2]))
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            min_z <= top.z + 1.0,
            "the stem starts at the top face ({min_z} > {})",
            top.z
        );
        assert!(
            max_z >= top.z + expected - 1.0,
            "the stem reaches the screen-invariant tip ({max_z})"
        );
        assert!(
            min_z > top.z - 1.0,
            "the ribbon never reaches the slab base ({min_z})"
        );
    }

    #[test]
    fn a_click_along_the_stem_begins_a_drag() {
        let mut grip = grip();
        let camera = tilted_camera(200.0);
        let top = project_device(&camera, grip.top()).expect("the top");
        let tip = project_device(&camera, grip.stem_end(&camera)).expect("the stem tip");
        assert!(tip[1] > top[1], "the stem rises on screen");

        // Halfway up the stem, away from both the top-face anchor and the crossbar.
        let click = [
            ((top[0] + tip[0]) * 0.5).round() as i32,
            ((top[1] + tip[1]) * 0.5).round() as i32,
        ];
        assert!(grip.hit(&camera, click));
        assert!(grip.begin_drag(&camera, click));
        assert!(grip.is_dragging());
    }

    #[test]
    fn a_sub_element_or_unknown_selection_arms_nothing() {
        let stage = slab(7);
        assert!(height_grip(&SelectionRef::element(7, "Edge1"), &stage).is_none());
        assert!(height_grip(&SelectionRef::whole(9), &stage).is_none());
        assert!(height_grip(&SelectionRef::whole(7), &Stage::new()).is_none());
    }

    #[test]
    fn the_gizmo_sits_at_the_item_centre() {
        let stage = slab(7);
        let gizmo = move_gizmo(&SelectionRef::whole(7), &stage).expect("a gizmo");
        assert_eq!(gizmo.target_id(), 7);
        let origin = gizmo.origin();
        assert!((origin.x).abs() < 1e-9);
        assert!((origin.y).abs() < 1e-9);
        assert!((origin.z - 10.0).abs() < 1e-9);
        assert!(move_gizmo(&SelectionRef::element(7, "Face1"), &stage).is_none());
    }

    #[test]
    fn dragging_up_grows_the_height_and_commits_it() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(grip.is_dragging());
        assert!(grip.drag_to(&camera, [grab[0], grab[1] + 25]).is_some());
        let commit = grip.end_drag().expect("a commit");

        assert_eq!(commit.manipulator, TOY_HEIGHT_ID);
        assert_eq!(commit.target, 7);
        assert_eq!(commit.action, "set");
        assert!(commit.values[0] > 20.0, "dragging up grew the height");
        assert_eq!(commit.readout, format!("H {:.2} mm", commit.values[0]));
        assert!(!grip.is_dragging());
    }

    #[test]
    fn cancelling_restores_the_press_height() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(grip.drag_to(&camera, [grab[0], grab[1] + 40]).is_some());
        assert!(grip.cancel_drag());
        assert!(!grip.is_dragging());
        assert_eq!(grip.height(), 20.0);
        assert_eq!(grip.end_drag(), None);
    }

    #[test]
    fn the_height_never_goes_below_the_minimum() {
        let mut grip = grip();
        let camera = camera(200.0);
        let anchor = grip.readout_anchor(&camera).expect("an anchor");
        let grab = [anchor[0].round() as i32, anchor[1].round() as i32];

        assert!(grip.begin_drag(&camera, grab));
        assert!(
            grip.drag_to(&camera, [grab[0], grab[1] - 100_000])
                .is_some()
        );
        let commit = grip.end_drag().expect("a commit");
        assert!(commit.values[0] >= MIN_HEIGHT);
    }

    #[test]
    fn a_click_off_the_grip_does_not_start_a_drag() {
        let mut grip = grip();
        let camera = camera(200.0);
        assert!(!grip.begin_drag(&camera, [0, 0]));
        assert!(!grip.is_dragging());
    }
}
