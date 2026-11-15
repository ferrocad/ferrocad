//! The two-click regular polygon (`Polygon`).
//!
//! [`PolygonTool`] takes the centre with the first click and a vertex with the second;
//! the vertex is both the radius and the winding's first corner, so the polygon turns
//! the way the pointer did. The second click completes the session.
//!
//! The side count is a tool setting with upstream's default of six; a digit `3`–`9`
//! while the tool is armed sets it and rebuilds the preview, which is the one thing a
//! tool can offer before the shell grows numeric fields.
//!
//! The commit carries the **centre, the exact first vertex, the plane's normal and the
//! side count**.
//!
//! Anchored on a **planar face** the polygon is drawn in that face's plane (the
//! [`HoverTarget`] the first press bound) and the commit names the face as its
//! [`crate::interaction::CreationHost`], so the workbench can imprint it into the host.

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
pub const POLYGON_TOOL_ID: &str = "polygon";
/// The manipulator id a finished polygon commits under; the edition's `interaction`
/// hook creates the object for it.
pub const POLYGON_MANIPULATOR_ID: &str = "polygon";
/// The operation within the manipulator.
pub const POLYGON_ACTION_CREATE: &str = "create";
/// The label the tool strip shows.
pub const POLYGON_TOOL_LABEL: &str = "Polygon";
/// The status hint before the centre is placed.
pub const POLYGON_HINT_START: &str = "click the centre · 3–9 sets the sides · Esc to cancel";
/// The status hint once the centre is placed.
pub const POLYGON_HINT_VERTEX: &str =
    "click a vertex · 3–9 sets the sides · Backspace to move the centre · Esc to cancel";
/// The shortest radius that still names a polygon, in millimetres.
pub const POLYGON_MIN_RADIUS: f64 = MIN_DRAG;
/// Upstream's default side count (`polygon_sides = 6`).
pub const DEFAULT_POLYGON_SIDES: usize = 6;
/// The fewest sides that close a polygon.
pub const MIN_POLYGON_SIDES: usize = 3;

/// A two-click regular polygon.
#[derive(Clone, Debug)]
pub struct PolygonTool {
    center: Option<Vector3>,
    cursor: Option<[f32; 3]>,
    /// The centre and the exact first vertex, once the session is complete.
    placed: Option<(Vector3, Vector3)>,
    sides: usize,
    /// The placement under the pointer while nothing is placed.
    hover: Option<HoverTarget>,
    /// The face placement the first press bound, if any.
    session: Option<HoverTarget>,
    /// Three fixed slots — `[committed, preview, ghost]` — whose mesh identities are
    /// stable.
    items: Vec<ShapeItem>,
}

impl Default for PolygonTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PolygonTool {
    /// A new tool with [`DEFAULT_POLYGON_SIDES`] and the default colours.
    pub fn new() -> Self {
        Self {
            center: None,
            cursor: None,
            placed: None,
            sides: DEFAULT_POLYGON_SIDES,
            hover: None,
            session: None,
            items: vec![
                wire_item(COMMITTED_COLOR),
                wire_item(CURSOR_COLOR),
                ghost_item(CURSOR_COLOR),
            ],
        }
    }

    /// The side count the next polygon gets.
    pub fn sides(&self) -> usize {
        self.sides
    }

    /// Set the side count (`3`–`100`, upstream's ceiling). Returns whether it changed.
    pub fn set_sides(&mut self, sides: usize) -> bool {
        if sides < MIN_POLYGON_SIDES || sides == self.sides {
            return false;
        }
        self.sides = sides;
        self.rebuild();
        true
    }

    /// The frame the current state draws: `None` until the centre and a cursor exist.
    pub fn frame(&self) -> Option<DragFrame> {
        self.frame_at(self.center?, vector(self.cursor?))
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

    /// Reset every piece of session state (the side count survives).
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
            Some((center, vertex)) => self
                .frame_at(center, vertex)
                .map(|frame| to_points(&frame.polygon(self.sides))),
            None => self
                .frame()
                .map(|frame| to_points(&frame.polygon(self.sides))),
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

impl Tool for PolygonTool {
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
            if self.center.take().is_some() {
                self.rebuild();
                return true;
            }
            return false;
        }
        if let Some(sides) = key.parse::<usize>().ok().filter(|sides| *sides >= MIN_POLYGON_SIDES) {
            return self.set_sides(sides);
        }
        false
    }

    fn status(&self) -> ToolStatus {
        if self.center.is_some() {
            return ToolStatus::new(
                POLYGON_TOOL_ID,
                POLYGON_TOOL_LABEL,
                1,
                format!("{} sides · {POLYGON_HINT_VERTEX}", self.sides),
            );
        }
        ToolStatus::new(POLYGON_TOOL_ID, POLYGON_TOOL_LABEL, 0, POLYGON_HINT_START)
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
        let (center, vertex) = self.placed?;
        let frame = self.frame_at(center, vertex)?;
        if frame.radius < POLYGON_MIN_RADIUS {
            return None;
        }
        let normal = frame.normal();
        let mut commit = InteractionCommit::new(POLYGON_MANIPULATOR_ID, 0, POLYGON_ACTION_CREATE)
            .with_values(vec![
                center.x,
                center.y,
                center.z,
                vertex.x,
                vertex.y,
                vertex.z,
                normal.x,
                normal.y,
                normal.z,
                self.sides as f64,
            ])
            .with_readout(format!("{} sides · R {:.1} mm", self.sides, frame.radius))
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
    use crate::interaction::Tool;

    fn press(tool: &mut PolygonTool, point: [f32; 3]) {
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

    fn move_to(tool: &mut PolygonTool, point: [f32; 3]) {
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
    fn the_default_is_six_sides_and_the_commit_names_them() {
        let mut tool = PolygonTool::new();
        assert_eq!(tool.sides(), DEFAULT_POLYGON_SIDES);
        press(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [10.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        assert!(tool.completed());

        let commit = tool.finish().expect("a commit");
        assert_eq!(commit.manipulator, POLYGON_MANIPULATOR_ID);
        assert_eq!(commit.values.len(), 10, "centre, vertex, normal, sides");
        assert_eq!(&commit.values[0..3], &[0.0, 0.0, 0.0]);
        assert_eq!(&commit.values[3..6], &[10.0, 0.0, 0.0]);
        assert_eq!(&commit.values[6..9], &[0.0, 0.0, 1.0]);
        assert_eq!(commit.values[9], 6.0);
        assert_eq!(commit.readout, "6 sides · R 10.0 mm");
    }

    #[test]
    fn a_digit_changes_the_sides_and_rebuilds_the_preview() {
        let mut tool = PolygonTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [10.0, 0.0, 0.0]);
        assert!(tool.on_key("4", false), "4 sets four sides");
        assert_eq!(tool.sides(), 4);
        assert!(tool.status().hint.starts_with("4 sides"), "{}", tool.status().hint);
        assert!(!tool.on_key("2", false), "two sides cannot close");
        assert_eq!(tool.sides(), 4, "a refused digit changes nothing");
        assert!(!tool.on_key("z", false), "a letter is left to the host");
    }

    #[test]
    fn the_square_lands_on_the_drawn_plane_and_commits_once() {
        let mut tool = PolygonTool::new();
        tool.set_sides(4);
        press(&mut tool, [0.0, 0.0, 0.0]);
        move_to(&mut tool, [10.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        let commit = tool.finish().expect("a commit");
        assert_eq!(commit.values[9], 4.0);
        assert!(tool.finish().is_none(), "the commit owns the geometry");
        assert!(
            !tool.on_key("backspace", false),
            "nothing is left to drop"
        );
    }

    #[test]
    fn a_second_click_in_place_commits_nothing() {
        let mut tool = PolygonTool::new();
        press(&mut tool, [3.0, 3.0, 3.0]);
        press(&mut tool, [3.0, 3.0, 3.0]);
        assert!(!tool.completed());
        assert!(tool.finish().is_none());
    }

    #[test]
    fn cancel_drops_the_placed_polygon_and_keeps_the_side_count() {
        let mut tool = PolygonTool::new();
        tool.set_sides(5);
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [4.0, 0.0, 0.0]);
        assert!(tool.cancel());
        assert!(!tool.completed());
        assert_eq!(tool.sides(), 5, "the setting is the user's, not the session's");
    }
}
