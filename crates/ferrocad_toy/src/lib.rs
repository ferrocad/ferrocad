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
//! See `docs/toy-workbench.md` and `docs/architecture.md` §10.

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::Camera;
use ferrocad_stage::gizmo::{ACTIVE_COLOR, INACTIVE_COLOR};
use ferrocad_stage::screen::{HIT_SLOP, project_device, scale_for, to3};
use ferrocad_stage::{DrawMode, InteractionCommit, Manipulator, ShapeItem, Stage};
use ferrocad_types::{Matrix4, Vector3};

/// The manipulator's registry id (`InteractionCommit::manipulator`).
pub const TOY_HEIGHT_ID: &str = "toy_height";

/// The grip's picking id. It is drawn as an overlay, so it only has to stay clear of
/// document and gizmo ids.
pub const TOY_GRIP_PICK_ID: u32 = 3_000_000;

/// The smallest height the grip will produce.
pub const MIN_HEIGHT: f64 = 0.1;

/// A vertical grip that drags the slab's `Height` property.
///
/// The anchor is the top centre of the displayed item; the handle is a vertical
/// dimension line down to the item's base plus a cross at the top. Dragging up grows
/// the height by the pointer's **vertical travel**, in screen-invariant world units.
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
    item: ShapeItem,
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
            item: ShapeItem::new(TOY_GRIP_PICK_ID)
                .with_draw_mode(DrawMode::Wireframe)
                .with_selectable(false),
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

    /// The overlay item.
    pub fn items(&self) -> &[ShapeItem] {
        std::slice::from_ref(&self.item)
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
        self.item.set_edge_color(self.stroke_color());
    }

    /// Whether `device` is on the grip (within the shared grab slop).
    pub fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool {
        let Some(anchor) = project_device(camera, self.top) else {
            return false;
        };
        (f64::from(device[0]) - anchor[0]).hypot(f64::from(device[1]) - anchor[1]) <= HIT_SLOP
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

    /// Rebuild the geometry: a dimension line from the base to the top, plus a cross
    /// at the top. The cross is screen-invariant, so the target stays grabbable at any
    /// zoom.
    fn build(&mut self, camera: &Camera) {
        let bottom = Vector3::new(self.top.x, self.top.y, self.top.z - self.height);
        let half = scale_for(camera, self.top, 5.0);
        let positions = vec![
            to3(bottom),
            to3(self.top),
            to3(Vector3::new(self.top.x - half, self.top.y, self.top.z)),
            to3(Vector3::new(self.top.x + half, self.top.y, self.top.z)),
        ];
        self.item
            .set_shape(positions, Vec::new(), Vec::new(), vec![0, 1, 2, 3]);
        self.apply_color();
    }

    /// Where the readout sits, in device pixels.
    pub fn readout_anchor(&self, camera: &Camera) -> Option<[f64; 2]> {
        project_device(camera, self.top)
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
    let positions = item.positions();
    if positions.is_empty() {
        return None;
    }
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for [x, y, z] in positions {
        min_x = min_x.min(*x);
        max_x = max_x.max(*x);
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
        min_z = min_z.min(*z);
        max_z = max_z.max(*z);
    }
    let top = Vector3::new(
        f64::from((min_x + max_x) * 0.5),
        f64::from((min_y + max_y) * 0.5),
        f64::from(max_z),
    );
    let height = f64::from(max_z - min_z);
    Some(Box::new(HeightGrip::new(selection.object, top, height)))
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
        let camera = camera(200.0);
        assert_eq!(
            grip.readout_anchor(&camera),
            project_device(&camera, Vector3::new(0.0, 0.0, 20.0))
        );
        assert!(grip.readout().expect("a readout").starts_with("H "));
    }

    #[test]
    fn a_sub_element_or_unknown_selection_arms_nothing() {
        let stage = slab(7);
        assert!(height_grip(&SelectionRef::element(7, "Edge1"), &stage).is_none());
        assert!(height_grip(&SelectionRef::whole(9), &stage).is_none());
        assert!(height_grip(&SelectionRef::whole(7), &Stage::new()).is_none());
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
