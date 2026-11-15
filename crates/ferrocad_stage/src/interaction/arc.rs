//! The three-click circular arc (`Arc`).
//!
//! [`ArcTool`] takes the centre, then the start of the arc, then its end — upstream's
//! order (`controller.py`: "Click center, then start, then arc end"). The first two
//! clicks fix the plane and the radius; the third sweeps to the pointer and completes
//! the session.
//!
//! The sweep **accumulates** as the pointer turns, so passing a half turn keeps going
//! instead of folding back to the short way round: a swept angle is what the user
//! drew, and `atan2` alone would forget which way the pointer went round.
//!
//! The commit carries the **three points on the arc** (start, halfway, end), which is
//! exactly what upstream's `Arc.edge()` hands the kernel (`Part.Arc(start, mid, end)`),
//! so the workbench needs no plane arithmetic of its own.

use std::f64::consts::{PI, TAU};

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
pub const ARC_TOOL_ID: &str = "arc";
/// The manipulator id a finished arc commits under; the edition's `interaction` hook
/// creates the object for it.
pub const ARC_MANIPULATOR_ID: &str = "arc";
/// The operation within the manipulator.
pub const ARC_ACTION_CREATE: &str = "create";
/// The label the tool strip shows.
pub const ARC_TOOL_LABEL: &str = "Arc";
/// The status hint before the centre is placed.
pub const ARC_HINT_START: &str = "click the centre · Esc to cancel";
/// The status hint once the centre is placed.
pub const ARC_HINT_RADIUS: &str = "click the arc's start · Esc to cancel";
/// The status hint once the radius is set.
pub const ARC_HINT_SWEEP: &str =
    "click the arc's end · Backspace to restart the radius · Esc to cancel";
/// The shortest radius that still names an arc, in millimetres.
pub const ARC_MIN_RADIUS: f64 = MIN_DRAG;
/// The smallest sweep a click accepts, in radians (about 0.006°): below it the three
/// points are too close to collinear to name a circle.
pub const ARC_MIN_SWEEP: f64 = 1e-4;
/// The largest sweep, just under a full turn — upstream refuses exactly 360°.
pub const ARC_MAX_SWEEP: f64 = TAU - 1e-3;

/// A three-click circular arc: centre, start, end.
#[derive(Clone, Debug)]
pub struct ArcTool {
    center: Option<Vector3>,
    /// The plane and radius the first two clicks fixed.
    frame: Option<DragFrame>,
    cursor: Option<[f32; 3]>,
    /// The sweep accumulated from the pointer's motion, in radians.
    sweep: f64,
    /// The pointer's last angle in the plane, for the accumulation's step.
    last_angle: Option<f64>,
    /// Three points on the arc — start, halfway, end — once the session is complete.
    placed: Option<[Vector3; 3]>,
    /// The placement under the pointer while no centre is placed.
    hover: Option<HoverTarget>,
    /// The face placement the first press bound, if any.
    session: Option<HoverTarget>,
    /// Three fixed slots — `[committed, preview, ghost]` — whose mesh identities are
    /// stable.
    items: Vec<ShapeItem>,
}

impl Default for ArcTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ArcTool {
    /// A new tool with the default colours.
    pub fn new() -> Self {
        Self {
            center: None,
            frame: None,
            cursor: None,
            sweep: 0.0,
            last_angle: None,
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

    /// The plane and radius, once the second click fixed them.
    pub fn frame(&self) -> Option<DragFrame> {
        self.frame
    }

    /// The sweep drawn so far, in radians.
    pub fn sweep(&self) -> f64 {
        self.sweep
    }

    /// Reset every piece of session state.
    pub fn clear(&mut self) {
        self.center = None;
        self.frame = None;
        self.cursor = None;
        self.sweep = 0.0;
        self.last_angle = None;
        self.placed = None;
        self.hover = None;
        self.session = None;
        self.rebuild();
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

    /// Turn the pointer's position into an accumulated sweep.
    ///
    /// Each step is wrapped to `(-π, π]` before it is added, so the accumulated angle
    /// follows the pointer across the half-turn seam instead of jumping the short way
    /// back.
    fn accumulate(&mut self, cursor: Vector3) {
        let Some(frame) = self.frame else {
            return;
        };
        let offset = cursor.sub(&frame.center);
        let (x, y) = (offset.dot(&frame.u), offset.dot(&frame.v));
        if x.hypot(y) < MIN_DRAG {
            return;
        }
        let angle = y.atan2(x);
        if let Some(last) = self.last_angle {
            let mut step = angle - last;
            while step > PI {
                step -= TAU;
            }
            while step < -PI {
                step += TAU;
            }
            self.sweep = (self.sweep + step).clamp(-ARC_MAX_SWEEP, ARC_MAX_SWEEP);
        }
        self.last_angle = Some(angle);
    }

    /// The outline the current state draws.
    fn outline(&self) -> Option<Vec<[f32; 3]>> {
        if let Some(points) = self.placed {
            return Some(to_points(&points));
        }
        if let Some(frame) = self.frame {
            // The radius is fixed: show the sweep the pointer has drawn, and from a
            // fresh start the spoke it is still setting.
            if self.sweep.abs() < ARC_MIN_SWEEP {
                return Some(to_points(&[frame.center, frame.point(0.0)]));
            }
            return Some(to_points(&frame.arc(self.sweep)));
        }
        match (self.center, self.cursor) {
            (Some(center), Some(cursor)) => self
                .frame_at(center, vector(cursor))
                .map(|frame| to_points(&frame.ring())),
            _ => None,
        }
    }

    /// Rewrite the slots from the current state.
    fn rebuild(&mut self) {
        let marker = match (&self.hover, self.center.is_none()) {
            (Some(target), true) => target.marker(HOVER_MARKER_HALF),
            _ => Vec::new(),
        };
        set_ghost_loop(&mut self.items[2], &marker);
        let outline = self.outline();
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

impl Tool for ArcTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        match event.kind {
            PointerKind::Move => {
                self.cursor = target;
                if let Some(point) = target {
                    self.accumulate(vector(point));
                }
                self.rebuild();
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
                    }
                    Some(center) if self.frame.is_none() => {
                        // The second click fixes the plane and the radius. A click
                        // that would make a hairline arc moves nothing.
                        let Some(frame) = self.frame_at(center, vector(point)) else {
                            return;
                        };
                        self.frame = Some(frame);
                        self.sweep = 0.0;
                        self.last_angle = Some(0.0);
                        self.cursor = Some(point);
                    }
                    Some(_) => {
                        if self.sweep.abs() < ARC_MIN_SWEEP {
                            return;
                        }
                        let Some(frame) = self.frame else {
                            return;
                        };
                        self.placed = Some([
                            frame.point(0.0),
                            frame.point(self.sweep / 2.0),
                            frame.point(self.sweep),
                        ]);
                    }
                }
                self.rebuild();
            }
            _ => {}
        }
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn hover(&mut self, target: Option<&HoverTarget>) {
        if self.center.is_some() {
            return;
        }
        self.hover = target.cloned();
        self.rebuild();
    }

    fn on_key(&mut self, key: &str, ctrl: bool) -> bool {
        if ctrl {
            return false;
        }
        match key {
            "backspace" | "delete" => {
                // One step back: the radius, then the centre.
                if self.frame.take().is_some() {
                    self.sweep = 0.0;
                    self.last_angle = None;
                    self.rebuild();
                    return true;
                }
                if self.center.take().is_some() {
                    self.rebuild();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn status(&self) -> ToolStatus {
        if let Some(frame) = self.frame {
            return ToolStatus::new(
                ARC_TOOL_ID,
                ARC_TOOL_LABEL,
                2,
                format!(
                    "{:.0}° of {:.1} mm · {ARC_HINT_SWEEP}",
                    self.sweep.to_degrees().abs(),
                    frame.radius
                ),
            );
        }
        if self.center.is_some() {
            return ToolStatus::new(ARC_TOOL_ID, ARC_TOOL_LABEL, 1, ARC_HINT_RADIUS);
        }
        ToolStatus::new(ARC_TOOL_ID, ARC_TOOL_LABEL, 0, ARC_HINT_START)
    }

    fn completed(&self) -> bool {
        self.placed.is_some()
    }

    fn cancel(&mut self) -> bool {
        let cancelled = self.center.is_some()
            || self.frame.is_some()
            || self.cursor.is_some()
            || self.placed.is_some();
        self.clear();
        cancelled
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        let [start, mid, end] = self.placed?;
        let frame = self.frame?;
        if frame.radius < ARC_MIN_RADIUS || self.sweep.abs() < ARC_MIN_SWEEP {
            return None;
        }
        let mut commit = InteractionCommit::new(ARC_MANIPULATOR_ID, 0, ARC_ACTION_CREATE)
            .with_values(vec![
                start.x, start.y, start.z, mid.x, mid.y, mid.z, end.x, end.y, end.z,
            ])
            .with_readout(format!(
                "{:.1}° · R {:.1} mm",
                self.sweep.to_degrees().abs(),
                frame.radius
            ))
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

    fn press(tool: &mut ArcTool, point: [f32; 3]) {
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

    fn move_to(tool: &mut ArcTool, point: [f32; 3]) {
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
    fn centre_start_and_end_make_a_quarter_circle() {
        let mut tool = ArcTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        assert!(!tool.completed(), "two clicks only fix the radius");
        assert_eq!(tool.status().points, 2);
        move_to(&mut tool, [0.0, 10.0, 0.0]);
        assert!((tool.sweep() - PI / 2.0).abs() < 1e-9, "{}", tool.sweep());
        press(&mut tool, [0.0, 10.0, 0.0]);
        assert!(tool.completed());

        let commit = tool.finish().expect("a commit");
        assert_eq!(commit.manipulator, ARC_MANIPULATOR_ID);
        assert_eq!(commit.values.len(), 9, "three points on the arc");
        // Start on `+X`, halfway at 45°, end on `+Y`.
        assert!((commit.values[0] - 10.0).abs() < 1e-6);
        assert!((commit.values[1]).abs() < 1e-6);
        let mid = 10.0 / 2.0_f64.sqrt();
        assert!((commit.values[3] - mid).abs() < 1e-6, "{:?}", commit.values);
        assert!((commit.values[4] - mid).abs() < 1e-6, "{:?}", commit.values);
        assert!((commit.values[6]).abs() < 1e-6);
        assert!((commit.values[7] - 10.0).abs() < 1e-6);
        assert_eq!(commit.readout, "90.0° · R 10.0 mm");
        assert!(tool.finish().is_none(), "the commit owns the geometry");
    }

    #[test]
    fn a_sweep_past_a_half_turn_keeps_going_the_way_the_pointer_went() {
        let mut tool = ArcTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        // Turn through +Y and -X to -Y: three quarters of a turn.
        move_to(&mut tool, [0.0, 10.0, 0.0]);
        move_to(&mut tool, [-10.0, 0.0, 0.0]);
        move_to(&mut tool, [0.0, -10.0, 0.0]);
        assert!((tool.sweep() - 1.5 * PI).abs() < 1e-9, "{}", tool.sweep());
        press(&mut tool, [0.0, -10.0, 0.0]);

        let commit = tool.finish().expect("a commit");
        // The end is on `-Y`, not folded back to the 90° complement.
        assert!((commit.values[7] + 10.0).abs() < 1e-6, "{:?}", commit.values);
        assert!(commit.readout.starts_with("270.0°"), "{}", commit.readout);
    }

    #[test]
    fn the_sweep_is_signed_by_the_direction_of_travel() {
        let mut tool = ArcTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        move_to(&mut tool, [0.0, -10.0, 0.0]);
        assert!((tool.sweep() + PI / 2.0).abs() < 1e-9, "{}", tool.sweep());
    }

    #[test]
    fn a_third_click_without_a_sweep_commits_nothing() {
        let mut tool = ArcTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        assert!(!tool.completed(), "no sweep, no arc");
        assert!(tool.finish().is_none());
    }

    #[test]
    fn backspace_steps_back_to_the_centre_and_cancel_clears_everything() {
        let mut tool = ArcTool::new();
        press(&mut tool, [0.0, 0.0, 0.0]);
        press(&mut tool, [10.0, 0.0, 0.0]);
        move_to(&mut tool, [0.0, 10.0, 0.0]);
        assert!(tool.on_key("backspace", false));
        assert!(tool.frame().is_none(), "the radius is gone");
        assert_eq!(tool.sweep(), 0.0);

        press(&mut tool, [10.0, 0.0, 0.0]);
        move_to(&mut tool, [0.0, 10.0, 0.0]);
        press(&mut tool, [0.0, 10.0, 0.0]);
        assert!(tool.completed());
        assert!(tool.cancel());
        assert!(!tool.completed());
        assert!(tool.finish().is_none());
    }
}
