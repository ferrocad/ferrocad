//! The two-click circle (`Circle`).
//!
//! [`CircleTool`] takes the centre with the first click and a rim point with the
//! second; the second click completes the session, so the host finishes it without an
//! `Enter`. The rim point is what names the plane — the drag's own, from
//! [`DragFrame`] — so the circle lies where the pointer swept and no active-workplane
//! state has to exist for the tool to preview honestly.
//!
//! The commit carries the **centre, an exact rim point and the plane's normal**, so the
//! workbench does not re-derive the plane from a rule: the circle the user saw is the
//! circle that is committed.
//!
//! Anchored on a **planar face** the circle is drawn in that face's plane (the
//! [`HoverTarget`] the first press bound) and the commit names the face as its
//! [`crate::interaction::CreationHost`], so the workbench can imprint the rim into the
//! host instead of making a second object.

use ferrocad_types::Vector3;

use crate::frame::{DragFrame, MIN_DRAG, to_points, vector};
use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::{CreationHost, HoverTarget, InteractionCommit};
use crate::shape::ShapeItem;
use crate::snap::Snap;
use crate::tool::{
    COMMITTED_COLOR, CURSOR_COLOR, HOVER_MARKER_HALF, Tool, ToolStatus, ghost_item,
    set_ghost_loop, set_polyline, wire_item,
};

/// The tool's id in its status (the host's registry id is separate).
pub const CIRCLE_TOOL_ID: &str = "circle";
/// The manipulator id a finished circle commits under; the edition's `interaction`
/// hook creates the object for it.
pub const CIRCLE_MANIPULATOR_ID: &str = "circle";
/// The operation within the manipulator.
pub const CIRCLE_ACTION_CREATE: &str = "create";
/// The label the tool strip shows.
pub const CIRCLE_TOOL_LABEL: &str = "Circle";
/// The status hint before the centre is placed.
pub const CIRCLE_HINT_START: &str = "click the centre · Esc to cancel";
/// The status hint once the centre is placed.
pub const CIRCLE_HINT_RADIUS: &str =
    "click to set the radius · Backspace to move the centre · Esc to cancel";
/// A radius shorter than this is not a circle, in millimetres.
pub const CIRCLE_MIN_RADIUS: f64 = MIN_DRAG;

/// A two-click circle: the first press is the centre, the second the radius.
#[derive(Clone, Debug)]
pub struct CircleTool {
    center: Option<Vector3>,
    cursor: Option<[f32; 3]>,
    /// The centre and an exact in-plane rim point, once the session is complete.
    placed: Option<(Vector3, Vector3)>,
    /// The placement under the pointer while nothing is placed.
    hover: Option<HoverTarget>,
    /// The face placement the first press bound, if any.
    session: Option<HoverTarget>,
    /// Three fixed slots — `[committed, preview, ghost]` — whose mesh identities are
    /// stable.
    items: Vec<ShapeItem>,
}

impl Default for CircleTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CircleTool {
    /// A new tool with the default colours.
    pub fn new() -> Self {
        Self {
            center: None,
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

    /// The centre, once placed.
    pub fn center(&self) -> Option<Vector3> {
        self.center
    }

    /// The frame the current state draws: `None` until the centre and a cursor exist.
    pub fn frame(&self) -> Option<DragFrame> {
        let center = self.center?;
        let cursor = self.cursor?;
        self.frame_at(center, vector(cursor))
    }

    /// The frame the drag `center → cursor` names in this session's plane.
    fn frame_at(&self, center: Vector3, cursor: Vector3) -> Option<DragFrame> {
        match &self.session {
            Some(session) => DragFrame::in_plane(center, cursor, &session.frame),
            None => DragFrame::from_drag(center, cursor),
        }
    }

    /// The body (and element) the session is anchored on, once the first press bound one.
    pub fn host(&self) -> Option<&CreationHost> {
        self.session.as_ref().and_then(|session| session.host.as_ref())
    }

    /// Reset every piece of session state.
    pub fn clear(&mut self) {
        self.center = None;
        self.cursor = None;
        self.placed = None;
        self.hover = None;
        self.session = None;
        self.rebuild();
    }

    /// Rewrite the slots from the current state.
    fn rebuild(&mut self) {
        let marker = match (&self.hover, self.center.is_none()) {
            (Some(target), true) => target.marker(HOVER_MARKER_HALF),
            _ => Vec::new(),
        };
        set_ghost_loop(&mut self.items[2], &marker);
        let outline = match self.placed {
            Some((center, rim)) => self
                .frame_at(center, rim)
                .map(|frame| to_points(&frame.ring())),
            None => self.frame().map(|frame| to_points(&frame.ring())),
        };
        match (self.placed.is_some(), outline) {
            (true, Some(outline)) => {
                set_polyline(&mut self.items[0], &outline);
                set_polyline(&mut self.items[1], &[]);
            }
            (false, Some(outline)) => {
                set_polyline(&mut self.items[0], &[]);
                set_polyline(&mut self.items[1], &outline);
            }
            (_, None) => {
                set_polyline(&mut self.items[0], &[]);
                set_polyline(&mut self.items[1], &[]);
            }
        }
    }
}

impl Tool for CircleTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        match event.kind {
            PointerKind::Move => {
                if self.center.is_some() {
                    self.cursor = target;
                    self.rebuild();
                }
            }
            PointerKind::Down if event.button == Some(MouseButton::Left) => {
                let Some(point) = target else {
                    return;
                };
                match self.center {
                    None => {
                        self.session = self.hover.take().filter(HoverTarget::is_face);
                        self.center = Some(vector(point));
                        self.cursor = Some(point);
                        self.rebuild();
                    }
                    Some(center) => {
                        // A click that would make a hairline circle moves nothing: the
                        // tool waits for a rim point that gives it a radius.
                        let Some(frame) = self.frame_at(center, vector(point)) else {
                            return;
                        };
                        self.placed = Some((frame.center, frame.point(0.0)));
                        self.rebuild();
                    }
                }
            }
            _ => {}
        }
    }

    fn hover(&mut self, target: Option<&HoverTarget>) {
        if self.center.is_some() {
            return;
        }
        self.hover = target.cloned();
        self.rebuild();
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn on_key(&mut self, key: &str, ctrl: bool) -> bool {
        if ctrl {
            return false;
        }
        if key == "backspace" || key == "delete" {
            // One step back: the centre is the only thing placed so far.
            if self.center.take().is_some() {
                self.rebuild();
                return true;
            }
        }
        false
    }

    fn status(&self) -> ToolStatus {
        if self.center.is_some() {
            return ToolStatus::new(
                CIRCLE_TOOL_ID,
                CIRCLE_TOOL_LABEL,
                1,
                CIRCLE_HINT_RADIUS,
            );
        }
        ToolStatus::new(CIRCLE_TOOL_ID, CIRCLE_TOOL_LABEL, 0, CIRCLE_HINT_START)
    }

    fn completed(&self) -> bool {
        self.placed.is_some()
    }

    fn cancel(&mut self) -> bool {
        let cancelled = self.center.is_some() || self.cursor.is_some() || self.placed.is_some();
        self.clear();
        cancelled
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        let (center, rim) = self.placed?;
        let frame = self.frame_at(center, rim)?;
        if frame.radius < CIRCLE_MIN_RADIUS {
            return None;
        }
        let normal = frame.normal();
        let mut commit = InteractionCommit::new(CIRCLE_MANIPULATOR_ID, 0, CIRCLE_ACTION_CREATE)
            .with_values(vec![
                center.x, center.y, center.z, rim.x, rim.y, rim.z, normal.x, normal.y, normal.z,
            ])
            .with_readout(format!("Ø {:.1} mm", 2.0 * frame.radius))
            .creating();
        if let Some(host) = self.host().cloned() {
            commit = commit.with_host(host);
        }
        // The commit owns the geometry: a second finish must not create a twin.
        self.clear();
        Some(commit)
    }
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

    fn press(tool: &mut CircleTool, point: [f32; 3]) {
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

    fn move_to(tool: &mut CircleTool, point: [f32; 3]) {
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
    fn two_clicks_make_a_circle_on_the_world_plane() {
        let mut tool = CircleTool::new();
        assert!(!tool.completed());
        press(&mut tool, [0.0, 0.0, 0.0]);
        assert!(!tool.completed(), "one click has no radius yet");
        assert_eq!(tool.status().points, 1);

        move_to(&mut tool, [6.0, 8.0, 0.0]);
        assert_eq!(tool.frame().expect("a preview").radius, 10.0);

        press(&mut tool, [6.0, 8.0, 0.0]);
        assert!(tool.completed(), "the second click completes the circle");

        let commit = tool.finish().expect("a commit");
        assert!(commit.creates);
        assert_eq!(commit.manipulator, CIRCLE_MANIPULATOR_ID);
        assert_eq!(commit.action, CIRCLE_ACTION_CREATE);
        assert_eq!(commit.values.len(), 9, "centre, rim, normal");
        assert_eq!(&commit.values[0..3], &[0.0, 0.0, 0.0]);
        assert!((commit.values[3] - 6.0).abs() < 1e-6);
        assert!((commit.values[4] - 8.0).abs() < 1e-6);
        assert_eq!(&commit.values[6..9], &[0.0, 0.0, 1.0], "the ground plane");
        assert_eq!(commit.readout, "Ø 20.0 mm");
        assert!(tool.finish().is_none(), "the commit owns the geometry");
    }

    #[test]
    fn a_vertical_radius_stands_the_circle_up() {
        let mut tool = CircleTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [0.0, 0.0, 5.0]);
        press(&mut tool, [0.0, 0.0, 5.0]);
        let commit = tool.finish().expect("a commit");
        // The radius had no `y` component, so the circle stands in world `XZ`.
        assert_eq!(&commit.values[6..9], &[0.0, -1.0, 0.0]);
        assert!((commit.values[5] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn a_second_click_in_place_commits_nothing() {
        let mut tool = CircleTool::new();
        press(&mut tool, [2.0, 2.0, 2.0]);
        press(&mut tool, [2.0, 2.0, 2.0]);
        assert!(!tool.completed(), "a click is not a radius");
        assert!(tool.finish().is_none());
    }

    #[test]
    fn backspace_drops_the_centre_and_cancel_clears_everything() {
        let mut tool = CircleTool::new();
        press(&mut tool, [1.0, 1.0, 0.0]);
        assert!(tool.on_key("backspace", false));
        assert!(tool.center().is_none(), "the centre is gone");
        assert!(tool.frame().is_none());
        assert!(!tool.on_key("backspace", false), "nothing left to drop");

        press(&mut tool, [1.0, 1.0, 0.0]);
        press(&mut tool, [4.0, 1.0, 0.0]);
        assert!(tool.completed());
        assert!(tool.cancel());
        assert!(!tool.completed(), "cancel drops the placed circle too");
        assert!(tool.finish().is_none());
    }

    #[test]
    fn a_face_anchored_circle_is_drawn_in_the_faces_plane_and_names_it() {
        let mut tool = CircleTool::new();
        tool.hover(Some(&face_target()));
        assert_eq!(tool.items()[2].positions().len(), 4, "the anchor cue");
        press(&mut tool, [0.0, 0.0, 0.0]);
        assert_eq!(tool.items()[2].positions().len(), 0, "the cue is spent");

        // A rim point 3 across and 4 up the face, 9 out along its normal: the radius is
        // the in-plane 5, and the other 9 is not part of the circle.
        move_to(&mut tool, [3.0, 9.0, 4.0]);
        assert_eq!(tool.frame().expect("a preview").radius, 5.0);
        press(&mut tool, [3.0, 9.0, 4.0]);

        let commit = tool.finish().expect("a commit");
        assert_eq!(commit.values.len(), 9, "centre, rim, normal");
        assert_eq!(&commit.values[6..9], &[0.0, -1.0, 0.0], "the face's normal");
        assert_eq!(commit.readout, "Ø 10.0 mm");
        let host = commit.host.as_ref().expect("the face rides on the commit");
        assert_eq!(host.entity, 7);
        assert_eq!(host.element_name(), Some("Face6"));
    }
}
