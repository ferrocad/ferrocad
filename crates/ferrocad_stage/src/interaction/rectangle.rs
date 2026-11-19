//! The two-corner rectangle (`Rectangle`).
//!
//! [`RectangleTool`] takes two left-clicks as opposite corners and previews the box
//! between them; the second click completes the session, so the host finishes it without
//! an `Enter`. The rectangle lies in the **axis-aligned plane that keeps both drawn sides
//! square to the world axes** (world `XY` for a flat drag, `XZ`/`YZ` for a vertical one) —
//! the same "draw on the ground unless the drag is vertical" rule SimpleCAD's own drawing
//! uses, and the reason the tool can preview honestly without knowing the active workplane.
//!
//! The commit carries the **four corners** in world space, so the workbench that creates
//! the face does not re-derive the plane: the geometry the user saw is the geometry that
//! is committed.
//!
//! Anchored on a **planar face** the rule changes once: the first press binds that face's
//! plane (the [`HoverTarget`] the viewport resolved), the box is drawn square to the
//! face's own axes, and the commit names the face as its [`CreationHost`], so the
//! workbench can imprint the outline into the host instead of making a second object.

use ferrocad_types::Vector3;

use crate::frame::{drag_axes, vector};
use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::{CreationHost, HoverTarget, InteractionCommit};
use crate::shape::ShapeItem;
use crate::snap::Snap;
use crate::tool::{
    COMMITTED_COLOR, CURSOR_COLOR, HOVER_MARKER_HALF, Tool, ToolStatus, ghost_item,
    set_ghost_loop, set_polyline, wire_item,
};

/// The tool's id in its status (the host's registry id is separate).
pub const RECTANGLE_TOOL_ID: &str = "rectangle";
/// The manipulator id a finished rectangle commits under; the edition's `interaction`
/// hook creates the object for it.
pub const RECTANGLE_MANIPULATOR_ID: &str = "rectangle";
/// The operation within the manipulator.
pub const RECTANGLE_ACTION_CREATE: &str = "create";
/// The label the tool strip shows.
pub const RECTANGLE_TOOL_LABEL: &str = "Rectangle";
/// The status hint before the first corner is placed.
pub const RECTANGLE_HINT_START: &str = "click the first corner · Esc to cancel";
/// The status hint once the first corner is placed.
pub const RECTANGLE_HINT_SECOND: &str =
    "click the opposite corner · Backspace to move the first · Esc to cancel";

/// A side shorter than this is not a rectangle, in millimetres.
pub const RECTANGLE_MIN_SIDE: f64 = 1e-6;

/// A two-click rectangle: the first press places a corner, the second completes the box.
///
/// The preview is the box the second click would commit, rebuilt on every pointer move.
/// `Backspace` drops the first corner (a second chance at it); `Esc` cancels the session
/// without writing anything.
#[derive(Clone, Debug)]
pub struct RectangleTool {
    first: Option<[f32; 3]>,
    cursor: Option<[f32; 3]>,
    /// The placed corners once the session is complete; read by [`Tool::completed`].
    placed: Option<([f32; 3], [f32; 3])>,
    /// The placement under the pointer while nothing is placed.
    hover: Option<HoverTarget>,
    /// The placement the **first press** bound: its plane is the drawing plane and its
    /// host is what the commit names. `None` on the workplane, where the drag's own
    /// world plane decides.
    session: Option<HoverTarget>,
    /// Three fixed slots — `[committed, preview, ghost]` — whose mesh identities are
    /// stable.
    items: Vec<ShapeItem>,
}

impl Default for RectangleTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RectangleTool {
    /// A new tool with the default colours.
    pub fn new() -> Self {
        Self {
            first: None,
            cursor: None,
            placed: None,
            hover: None,
            session: None,
            items: vec![
                wire_item(COMMITTED_COLOR),
                wire_item(CURSOR_COLOR),
                ghost_item(CURSOR_COLOR),
            ],
        }
    }

    /// The first corner, once placed.
    pub fn first(&self) -> Option<[f32; 3]> {
        self.first
    }

    /// The corners the current state would commit: `first` and the live cursor.
    ///
    /// Anchored on a face the box is square to the **face's** axes; on the workplane it
    /// is square to the world axes of the plane the drag names.
    pub fn corners(&self) -> Option<[Vector3; 4]> {
        self.corners_at(self.first?, self.cursor?)
    }

    /// The corners two opposite points name, in this session's plane.
    fn corners_at(&self, first: [f32; 3], second: [f32; 3]) -> Option<[Vector3; 4]> {
        match &self.session {
            Some(session) => {
                let [u, v, _] = session.frame.basis;
                rectangle_corners_in(first, second, &u, &v)
            }
            None => rectangle_corners(first, second),
        }
    }

    /// The body (and element) the session is anchored on, once the first press bound one.
    pub fn host(&self) -> Option<&CreationHost> {
        self.session.as_ref().and_then(|session| session.host.as_ref())
    }

    /// Reset every piece of session state.
    pub fn clear(&mut self) {
        self.first = None;
        self.cursor = None;
        self.placed = None;
        self.hover = None;
        self.session = None;
        self.rebuild();
    }

    /// Rewrite the slots from the current state.
    fn rebuild(&mut self) {
        // The ghost is the anchor cue: a small square in the placement plane, shown
        // until the first corner fixes the placement.
        let marker = match (&self.hover, self.first.is_none()) {
            (Some(target), true) => target.marker(HOVER_MARKER_HALF),
            _ => Vec::new(),
        };
        set_ghost_loop(&mut self.items[2], &marker);
        match self.corners() {
            Some(corners) => {
                let outline = outline(&corners);
                if self.placed.is_some() {
                    set_polyline(&mut self.items[0], &outline);
                    set_polyline(&mut self.items[1], &[]);
                } else {
                    set_polyline(&mut self.items[0], &[]);
                    set_polyline(&mut self.items[1], &outline);
                }
            }
            None => {
                set_polyline(&mut self.items[0], &[]);
                set_polyline(&mut self.items[1], &[]);
            }
        }
    }
}

impl Tool for RectangleTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        match event.kind {
            PointerKind::Move => {
                if self.first.is_some() {
                    self.cursor = target;
                    self.rebuild();
                }
            }
            PointerKind::Down if event.button == Some(MouseButton::Left) => {
                let Some(point) = target else {
                    return;
                };
                match self.first {
                    None => {
                        // The first press fixes the placement: a face target binds its
                        // plane and names the body; the workplane leaves the drag's own
                        // world plane to decide, as it always has.
                        self.session = self.hover.take().filter(HoverTarget::is_face);
                        self.first = Some(point);
                        self.cursor = Some(point);
                        self.rebuild();
                    }
                    Some(first) => {
                        // A second click that would make a degenerate box moves nothing:
                        // the tool waits for a corner that gives it a width and a height.
                        if self.corners_at(first, point).is_none() {
                            return;
                        }
                        self.placed = Some((first, point));
                        self.cursor = Some(point);
                        self.rebuild();
                    }
                }
            }
            _ => {}
        }
    }

    fn hover(&mut self, target: Option<&HoverTarget>) {
        if self.first.is_some() {
            return;
        }
        self.hover = target.cloned();
        self.rebuild();
    }

    fn anchor(&self) -> Option<Vector3> {
        // The first corner, once it is placed; before that the placement under the pointer.
        self.first.map(vector).or_else(|| {
            self.hover
                .as_ref()
                .map(|target| target.frame.origin)
        })
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn on_key(&mut self, key: &str, ctrl: bool) -> bool {
        if ctrl {
            return false;
        }
        if key == "backspace" || key == "delete" {
            // One step back: the first corner is the only thing placed so far.
            if self.first.take().is_some() {
                self.rebuild();
                return true;
            }
        }
        false
    }

    fn status(&self) -> ToolStatus {
        if self.first.is_some() {
            return ToolStatus::new(RECTANGLE_TOOL_ID, RECTANGLE_TOOL_LABEL, 1, RECTANGLE_HINT_SECOND);
        }
        ToolStatus::new(RECTANGLE_TOOL_ID, RECTANGLE_TOOL_LABEL, 0, RECTANGLE_HINT_START)
    }

    fn completed(&self) -> bool {
        self.placed.is_some()
    }

    fn cancel(&mut self) -> bool {
        let cancelled = self.first.is_some() || self.cursor.is_some() || self.placed.is_some();
        self.clear();
        cancelled
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        self.placed?;
        let corners = self.corners()?;
        let values = corners
            .iter()
            .flat_map(|corner| [corner.x, corner.y, corner.z])
            .collect::<Vec<_>>();
        let width = corners[1].sub(&corners[0]).length();
        let height = corners[3].sub(&corners[0]).length();
        let mut commit = InteractionCommit::new(RECTANGLE_MANIPULATOR_ID, 0, RECTANGLE_ACTION_CREATE)
            .with_values(values)
            .with_readout(format!("{width:.1} × {height:.1} mm"))
            .creating();
        if let Some(host) = self.host().cloned() {
            commit = commit.with_host(host);
        }
        // The commit owns the geometry: a second finish must not create a twin.
        self.clear();
        Some(commit)
    }
}

/// The rectangle's four corners, in order, from two opposite corners of the drag.
///
/// The plane is the axis-aligned one that keeps both sides square to the world axes —
/// [`drag_axes`], the rule every creation tool shares: the axis least aligned with the
/// diagonal is dropped. `None` when that plane would make a side shorter than
/// [`RECTANGLE_MIN_SIDE`] (a click, or a drag along the dropped axis only).
pub fn rectangle_corners(first: [f32; 3], second: [f32; 3]) -> Option<[Vector3; 4]> {
    let first = as_vector(first);
    let second = as_vector(second);
    let delta = second.sub(&first);
    // The two world axes the box spans; the third is the plane's normal and is ignored, so
    // the rectangle is axis-square in the plane it was drawn in.
    let (u, v) = drag_axes(delta);
    let width = delta.dot(&u);
    let height = delta.dot(&v);
    assemble(first, first.add(&u.scale(width)), width, &v, height)
}

/// The four corners of the box `first → second` drawn in the plane `(u, v)`.
///
/// This is the **face-anchored** rule: `u` and `v` are the bound face's own in-plane
/// axes, so the box stays square to the face instead of to the world. `None` when a
/// side would be shorter than [`RECTANGLE_MIN_SIDE`].
pub fn rectangle_corners_in(
    first: [f32; 3],
    second: [f32; 3],
    u: &Vector3,
    v: &Vector3,
) -> Option<[Vector3; 4]> {
    let first = as_vector(first);
    let delta = as_vector(second).sub(&first);
    let width = delta.dot(u);
    let height = delta.dot(v);
    assemble(first, first.add(&u.scale(width)), width, v, height)
}

/// The four corners from the first corner, the first side's far point and the second
/// in-plane axis, or `None` for a degenerate box.
fn assemble(
    first: Vector3,
    along_end: Vector3,
    width: f64,
    v: &Vector3,
    height: f64,
) -> Option<[Vector3; 4]> {
    if width.abs() < RECTANGLE_MIN_SIDE || height.abs() < RECTANGLE_MIN_SIDE {
        return None;
    }
    let far = along_end.add(&v.scale(height));
    Some([first, along_end, far, first.add(&v.scale(height))])
}

fn as_vector(point: [f32; 3]) -> Vector3 {
    Vector3::new(
        f64::from(point[0]),
        f64::from(point[1]),
        f64::from(point[2]),
    )
}

/// The closed outline of four corners, as the polyline the preview and the ghost draw.
fn outline(corners: &[Vector3; 4]) -> Vec<[f32; 3]> {
    let mut points = corners
        .iter()
        .map(|corner| [corner.x as f32, corner.y as f32, corner.z as f32])
        .collect::<Vec<_>>();
    points.push(points[0]);
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::ManipulatorFrame;
    use crate::interaction::{CreationHost, TargetKind, Tool};

    /// A planar face target in the world `XZ` plane (`n = -Y`), hosted on `Face6`.
    fn face_target() -> HoverTarget {
        HoverTarget {
            frame: ManipulatorFrame {
                origin: Vector3::zero(),
                basis: [
                    Vector3::new(1.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    Vector3::new(0.0, -1.0, 0.0),
                ],
            },
            host: Some(CreationHost::element(7, "Face6")),
            kind: TargetKind::Face,
        }
    }

    fn click(tool: &mut RectangleTool, point: [f32; 3]) {
        tool.on_event(
            &PointerEvent {
                kind: PointerKind::Down,
                position: [0, 0],
                button: Some(MouseButton::Left),
                hit: None,
                projection: None,
            },
            Some(Snap {
                point,
                kind: crate::snap::SnapKind::Point,
                local: None,
                major: false,
            }),
        );
    }

    fn move_to(tool: &mut RectangleTool, point: [f32; 3]) {
        tool.on_event(
            &PointerEvent {
                kind: PointerKind::Move,
                position: [0, 0],
                button: None,
                hit: None,
                projection: None,
            },
            Some(Snap {
                point,
                kind: crate::snap::SnapKind::Point,
                local: None,
                major: false,
            }),
        );
    }

    #[test]
    fn two_corners_make_a_rectangle_on_the_world_plane() {
        let mut tool = RectangleTool::new();
        assert!(!tool.completed());
        click(&mut tool, [0.0, 0.0, 0.0]);
        assert!(!tool.completed(), "one corner is not a rectangle");
        assert_eq!(tool.status().points, 1);

        move_to(&mut tool, [10.0, 4.0, 0.0]);
        let corners = tool.corners().expect("a preview box");
        assert_eq!(corners[0], Vector3::new(0.0, 0.0, 0.0));
        assert_eq!(corners[1], Vector3::new(10.0, 0.0, 0.0));
        assert_eq!(corners[2], Vector3::new(10.0, 4.0, 0.0));
        assert_eq!(corners[3], Vector3::new(0.0, 4.0, 0.0));

        click(&mut tool, [10.0, 4.0, 0.0]);
        assert!(tool.completed(), "the second click completes the box");

        let commit = tool.finish().expect("a commit");
        assert!(commit.creates);
        assert_eq!(commit.manipulator, RECTANGLE_MANIPULATOR_ID);
        assert_eq!(commit.action, RECTANGLE_ACTION_CREATE);
        assert_eq!(commit.values.len(), 12, "four corners in world space");
        assert_eq!(&commit.values[6..9], &[10.0, 4.0, 0.0]);
        assert_eq!(commit.readout, "10.0 × 4.0 mm");
        // The commit owns the geometry: finishing again writes nothing.
        assert!(tool.finish().is_none());
    }

    #[test]
    fn a_vertical_drag_makes_a_rectangle_in_the_vertical_plane() {
        let corners = rectangle_corners([0.0, 0.0, 0.0], [10.0, 0.0, 6.0]).expect("a box");
        // The diagonal has no `y` component, so world `XZ` is the plane.
        for corner in corners {
            assert_eq!(corner.y, 0.0);
        }
        assert_eq!(corners[1], Vector3::new(10.0, 0.0, 0.0));
        assert_eq!(corners[2], Vector3::new(10.0, 0.0, 6.0));
        assert_eq!(corners[3], Vector3::new(0.0, 0.0, 6.0));
    }

    #[test]
    fn a_click_and_a_drag_along_the_normal_make_no_rectangle() {
        assert!(rectangle_corners([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]).is_none());
        // Along the plane normal only: every candidate plane makes a zero side.
        assert!(rectangle_corners([0.0, 0.0, 0.0], [0.0, 0.0, 5.0]).is_none());
    }

    #[test]
    fn backspace_drops_the_first_corner_and_cancel_clears_everything() {
        let mut tool = RectangleTool::new();
        click(&mut tool, [2.0, 2.0, 0.0]);
        move_to(&mut tool, [5.0, 5.0, 0.0]);
        assert!(tool.on_key("backspace", false));
        assert!(tool.first().is_none(), "the first corner is gone");
        assert!(tool.corners().is_none());
        assert!(!tool.on_key("backspace", false), "nothing left to drop");

        click(&mut tool, [2.0, 2.0, 0.0]);
        click(&mut tool, [6.0, 6.0, 0.0]);
        assert!(tool.completed());
        assert!(tool.cancel());
        assert!(!tool.completed(), "cancel drops the placed box too");
        assert!(tool.finish().is_none());
    }

    #[test]
    fn hovering_a_face_draws_the_anchor_cue_and_clears_it_on_the_first_click() {
        let mut tool = RectangleTool::new();
        assert!(tool.items()[2].positions().is_empty(), "no cue before a hover");

        tool.hover(Some(&face_target()));
        assert_eq!(
            tool.items()[2].positions().len(),
            4,
            "the ghost slot holds the marker's four corners"
        );

        click(&mut tool, [0.0, 0.0, 0.0]);
        assert!(
            tool.items()[2].positions().is_empty(),
            "the anchor replaces the cue"
        );
        assert_eq!(
            tool.host().and_then(|host| host.element_name()),
            Some("Face6"),
            "the first press binds the face"
        );
    }

    #[test]
    fn a_face_anchored_rectangle_is_square_to_the_face_not_the_world() {
        let mut tool = RectangleTool::new();
        tool.hover(Some(&face_target()));
        click(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [10.0, 5.0, 4.0]);
        // The drag has a `y` component, but the bound plane is `XZ`: the box has no
        // depth into the face's normal. The world rule would have picked `XY`.
        let corners = tool.corners().expect("a preview box");
        for corner in corners {
            assert_eq!(corner.y, 0.0, "{corner:?}");
        }
        assert_eq!(corners[2], Vector3::new(10.0, 0.0, 4.0));

        click(&mut tool, [10.0, 5.0, 4.0]);
        let commit = tool.finish().expect("a commit");
        let host = commit.host.as_ref().expect("the face rides on the commit");
        assert_eq!(host.entity, 7);
        assert_eq!(host.element_name(), Some("Face6"));
        assert_eq!(&commit.values[6..9], &[10.0, 0.0, 4.0]);
    }

    #[test]
    fn a_workplane_hover_leaves_the_world_plane_rule_alone() {
        let mut tool = RectangleTool::new();
        tool.hover(Some(&HoverTarget::of_workplane(&crate::workplane::Workplane::world())));
        click(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [10.0, 0.0, 6.0]);
        // A vertical drag: the drag's own `XZ` plane, exactly as before the hover frame.
        let corners = tool.corners().expect("a preview box");
        assert_eq!(corners[2], Vector3::new(10.0, 0.0, 6.0));
        click(&mut tool, [10.0, 0.0, 6.0]);
        let commit = tool.finish().expect("a commit");
        assert!(commit.host.is_none(), "the workplane is not a body");
    }
}
