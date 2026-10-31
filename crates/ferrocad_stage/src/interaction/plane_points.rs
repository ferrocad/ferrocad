//! The three-point workplane pick (`App_Workplane3Points`).
//!
//! [`PlanePointsTool`] plots three world points and resolves the frame they span. It
//! creates **no geometry**: the host reads [`Tool::workplane`] before the session ends
//! and applies it as the active workplane, so the pick leaves nothing in the document.

use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::InteractionCommit;
use crate::shape::ShapeItem;
use crate::snap::Snap;
use crate::tool::{COMMITTED_COLOR, CURSOR_COLOR, Tool, ToolStatus, wire_item};
use crate::workplane::{Workplane, WorkplaneKind};
use ferrocad_types::Vector3;

/// The tool's id in its status (the host's registry id is the same name).
pub const PLANE_POINTS_TOOL_ID: &str = "workplane_points";
/// The label the tool strip shows.
pub const PLANE_POINTS_TOOL_LABEL: &str = "Workplane";
/// How many points the frame needs.
pub const PLANE_POINTS_COUNT: usize = 3;
/// The status hint while fewer than three points are plotted.
pub const PLANE_POINTS_HINT: &str = "pick 3 points · Backspace to undo · Esc to cancel";
/// The status hint once the three points are plotted.
pub const PLANE_POINTS_HINT_DONE: &str = "Enter to use these points · Esc to cancel";

/// A workplane pick: three left-clicks span a plane, which the host applies on finish.
///
/// The points are ordinary transient state: nothing leaves the tool until the host
/// finishes the session, and `Esc` throws the pick away (see [`Tool::cancel`]).
#[derive(Clone, Debug)]
pub struct PlanePointsTool {
    points: Vec<[f32; 3]>,
    /// Two fixed slots — `[dots, triangle]` — whose mesh identities are stable; only
    /// their geometry is rewritten. An empty slot emits nothing.
    items: Vec<ShapeItem>,
}

impl Default for PlanePointsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanePointsTool {
    /// A new tool with the default colours.
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            items: vec![wire_item(COMMITTED_COLOR), wire_item(CURSOR_COLOR)],
        }
    }

    /// The plotted points, in pick order.
    pub fn points(&self) -> &[[f32; 3]] {
        &self.points
    }

    /// The plane the three plotted points span, if they are distinct and not collinear.
    /// `None` until the third point is placed.
    pub fn workplane(&self) -> Option<Workplane> {
        plane_of(&self.points)
    }

    /// Rewrite the two slots from the current points.
    fn rebuild(&mut self) {
        set_dots(&mut self.items[0], &self.points);
        set_triangle(&mut self.items[1], &self.points);
    }
}

impl Tool for PlanePointsTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        if event.kind != PointerKind::Down || event.button != Some(MouseButton::Left) {
            return;
        }
        // Three points define the frame; a further press is ignored until Backspace
        // takes one away, so a stray click cannot move the pick.
        if self.points.len() >= PLANE_POINTS_COUNT {
            return;
        }
        // Prefer the snap, then the raw hit point (exactly as `PolylineTool` does).
        let Some(point) = snap.map(|snap| snap.point).or_else(|| event.world()) else {
            return;
        };
        self.points.push(point);
        self.rebuild();
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn status(&self) -> ToolStatus {
        ToolStatus::new(
            PLANE_POINTS_TOOL_ID,
            PLANE_POINTS_TOOL_LABEL,
            self.points.len(),
            if self.points.len() < PLANE_POINTS_COUNT {
                PLANE_POINTS_HINT
            } else {
                PLANE_POINTS_HINT_DONE
            },
        )
    }

    fn on_key(&mut self, key: &str, _ctrl: bool) -> bool {
        match key {
            "backspace" | "delete" => {
                let removed = self.points.pop().is_some();
                if removed {
                    self.rebuild();
                }
                removed
            }
            _ => false,
        }
    }

    fn cancel(&mut self) -> bool {
        let cancelled = !self.points.is_empty();
        self.points.clear();
        self.rebuild();
        cancelled
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        // A plane pick creates nothing: the host read `workplane` before ending the
        // session and applies the frame itself.
        None
    }

    fn workplane(&self) -> Option<Workplane> {
        plane_of(&self.points)
    }

    fn picks_workplane(&self) -> bool {
        true
    }
}

/// The frame spanned by `p0`, with `p1 - p0` and `p2 - p0` as its in-plane axes.
///
/// `Workplane::new` rejects a degenerate axis pair, which is exactly the collinear (or
/// repeated-point) case — so a triple that does not span a plane yields `None`.
fn plane_of(points: &[[f32; 3]]) -> Option<Workplane> {
    let [p0, p1, p2] = match points {
        [p0, p1, p2, ..] => [p0, p1, p2],
        _ => return None,
    };
    let world = |point: &[f32; 3]| {
        Vector3::new(
            f64::from(point[0]),
            f64::from(point[1]),
            f64::from(point[2]),
        )
    };
    let origin = world(p0);
    Workplane::new(
        origin,
        world(p1).sub(&origin),
        world(p2).sub(&origin),
        WorkplaneKind::Custom,
    )
}

/// Rewrite `item` as a small cross (a dot) at each plotted point; an empty list clears
/// it. The cross is world-space, like the polyline tool's marker.
fn set_dots(item: &mut ShapeItem, points: &[[f32; 3]]) {
    const HALF: f32 = 0.05;
    let mut positions = Vec::with_capacity(points.len() * 6);
    let mut edges = Vec::with_capacity(points.len() * 6);
    for p in points {
        let base = positions.len() as u32;
        positions.extend([
            [p[0] - HALF, p[1], p[2]],
            [p[0] + HALF, p[1], p[2]],
            [p[0], p[1] - HALF, p[2]],
            [p[0], p[1] + HALF, p[2]],
            [p[0], p[1], p[2] - HALF],
            [p[0], p[1], p[2] + HALF],
        ]);
        edges.extend([base, base + 1, base + 2, base + 3, base + 4, base + 5]);
    }
    item.set_shape(positions, Vec::new(), Vec::new(), edges);
}

/// Rewrite `item` as the closed triangle through exactly three points; anything else
/// clears it, so the preview appears only once the pick is complete.
fn set_triangle(item: &mut ShapeItem, points: &[[f32; 3]]) {
    let (positions, edges) = match points {
        [a, b, c] => (vec![*a, *b, *c], vec![0, 1, 1, 2, 2, 0]),
        _ => (Vec::new(), Vec::new()),
    };
    item.set_shape(positions, Vec::new(), Vec::new(), edges);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::SnapKind;
    use ferrocad_graphics::{Hit, PickId, RenderQueue, Topology};

    /// A left press at `world`, as the viewport dispatches one.
    fn press(world: [f32; 3]) -> PointerEvent {
        PointerEvent {
            kind: PointerKind::Down,
            position: [0, 0],
            button: Some(MouseButton::Left),
            hit: Some(Hit {
                id: PickId(1),
                point: world,
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            }),
            projection: None,
        }
    }

    fn snap(point: [f32; 3]) -> Option<Snap> {
        Some(Snap {
            point,
            kind: SnapKind::Point,
            local: None,
            major: false,
        })
    }

    /// The tool's transient visuals compiled to draw packets.
    fn emit(tool: &dyn Tool) -> RenderQueue {
        let mut queue = RenderQueue::default();
        for item in tool.items() {
            item.emit(&mut queue);
        }
        queue
    }

    fn line_packets(queue: &RenderQueue) -> usize {
        queue
            .packets
            .iter()
            .filter(|p| p.topology == Topology::Lines)
            .count()
    }

    #[test]
    fn three_presses_resolve_the_frame() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), snap([0.0, 0.0, 0.0]));
        tool.on_event(&press([1.0, 0.0, 0.0]), snap([1.0, 0.0, 0.0]));
        assert!(tool.workplane().is_none(), "two points do not span a plane");

        tool.on_event(&press([0.0, 1.0, 0.0]), snap([0.0, 1.0, 0.0]));
        assert_eq!(tool.points().len(), 3);

        let plane = tool.workplane().expect("three points span a plane");
        assert_eq!(plane.origin, Vector3::new(0.0, 0.0, 0.0));
        assert_eq!(plane.kind, WorkplaneKind::Custom);
        // `u = +X` and `v = +Y`, so the triangle's normal is `+Z` — perpendicular to
        // both in-plane axes.
        assert_eq!(plane.n, Vector3::new(0.0, 0.0, 1.0));
        for axis in [plane.u, plane.v] {
            assert!(plane.n.dot(&axis).abs() < 1e-9);
        }
    }

    #[test]
    fn two_points_are_not_a_workplane() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), None);
        tool.on_event(&press([1.0, 0.0, 0.0]), None);
        assert_eq!(tool.points().len(), 2);
        assert!(tool.workplane().is_none());
    }

    #[test]
    fn collinear_points_are_not_a_workplane() {
        let mut tool = PlanePointsTool::new();
        for x in [0.0, 1.0, 2.0] {
            tool.on_event(&press([x, 0.0, 0.0]), None);
        }
        assert_eq!(tool.points().len(), 3);
        assert!(
            tool.workplane().is_none(),
            "a straight line spans no plane"
        );
    }

    #[test]
    fn a_pick_prefers_the_snap_over_the_raw_hit() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([5.0, 5.0, 5.0]), snap([0.5, 0.0, 0.0]));
        assert_eq!(tool.points(), &[[0.5, 0.0, 0.0]]);
    }

    #[test]
    fn a_fourth_press_adds_nothing() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), None);
        tool.on_event(&press([1.0, 0.0, 0.0]), None);
        tool.on_event(&press([0.0, 1.0, 0.0]), None);
        let picked = tool.points().to_vec();

        tool.on_event(&press([4.0, 4.0, 4.0]), None);
        assert_eq!(tool.points(), &picked[..]);
        assert_eq!(tool.status().points, 3);
    }

    #[test]
    fn backspace_drops_the_last_point() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), None);
        tool.on_event(&press([1.0, 0.0, 0.0]), None);
        tool.on_event(&press([0.0, 1.0, 0.0]), None);

        assert!(tool.on_key("backspace", false));
        assert_eq!(tool.points().len(), 2, "the third point is gone");
        assert!(tool.workplane().is_none());

        assert!(tool.on_key("delete", false));
        assert_eq!(tool.points().len(), 1);
        assert!(tool.on_key("backspace", false));
        assert!(tool.points().is_empty());
        assert!(!tool.on_key("backspace", false), "nothing is left to remove");
        assert!(!tool.on_key("z", false), "an unrelated key is left to the host");
    }

    #[test]
    fn cancel_clears_the_points() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), None);
        tool.on_event(&press([1.0, 0.0, 0.0]), None);

        assert!(tool.cancel());
        assert!(tool.points().is_empty());
        assert!(tool.workplane().is_none());
        assert!(emit(&tool).is_empty(), "no transient visual survives");
        assert_eq!(tool.status().points, 0);
    }

    #[test]
    fn the_dots_lead_and_the_triangle_closes_the_pick() {
        let mut tool = PlanePointsTool::new();
        tool.on_event(&press([0.0, 0.0, 0.0]), None);
        tool.on_event(&press([1.0, 0.0, 0.0]), None);
        // Two dots, no triangle yet.
        let queue = emit(&tool);
        assert_eq!(line_packets(&queue), 1);
        assert_eq!(queue.packets[0].geometry.as_ref().unwrap().vertices.len(), 12);

        tool.on_event(&press([0.0, 1.0, 0.0]), None);
        let queue = emit(&tool);
        assert_eq!(line_packets(&queue), 2, "the triangle joins the three dots");
        assert_eq!(queue.packets[0].geometry.as_ref().unwrap().vertices.len(), 18);
        assert_eq!(queue.packets[1].geometry.as_ref().unwrap().vertices.len(), 3);
    }
}
