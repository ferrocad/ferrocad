//! Interactive tools: the replacement for `SoEventCallback`.
//!
//! A [`Tool`] reacts to [`PointerEvent`]s and produces **transient** visuals — a rubber
//! band, a snap marker, a preview — that the host draws alongside the document geometry
//! without touching it. Transient items are rebuilt as events arrive, so a tool never
//! mutates the scene graph (there is none) and never allocates a document object.

use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::InteractionCommit;
use crate::shape::{DrawMode, ShapeItem};
use crate::snap::Snap;
use crate::workplane::Workplane;

/// A tool's live status, for the host's tool strip.
///
/// A tool reports this while a session is active: the label and id name it, `points`
/// counts the placed vertices, and `hint` says what the next actions are. When
/// `active` is `false` the host hides the strip; a tool with no session state (the
/// default implementation) reports exactly that.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolStatus {
    /// The tool's own id (the registry id lives in the host, e.g. `toy_polyline`).
    pub id: String,
    /// The human label (`Polyline`).
    pub label: String,
    /// Whether a tool session is active and the strip should be shown.
    pub active: bool,
    /// How many vertices/points the session has placed so far.
    pub points: usize,
    /// One-line hint for the next actions.
    pub hint: String,
}

impl ToolStatus {
    /// A status with no session: the host draws no strip.
    pub fn inactive() -> Self {
        Self::default()
    }

    /// An active status.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        points: usize,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            active: true,
            points,
            hint: hint.into(),
        }
    }

    /// Whether a session is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The same tool marked inactive: the session finished or was cancelled. The id
    /// and label survive so the host can name it in a message.
    pub fn ended(mut self) -> Self {
        self.active = false;
        self.points = 0;
        self.hint.clear();
        self
    }
}

/// An interactive tool.
///
/// `on_event` is called for every pointer event; [`items`](Self::items) returns the
/// transient visuals to draw this frame. The items reuse stable mesh identities across
/// events (only their geometry changes), so a renderer's upload cache is not flooded.
///
/// A tool also owns its **lifecycle**: [`status`](Self::status) is the surface the
/// host draws (label, point count, hint) and [`cancel`](Self::cancel) drops every
/// transient visual without touching the document. The defaults describe a tool with
/// no session state: it is inactive and cancels nothing.
pub trait Tool {
    /// React to a pointer event. `snap` is the resolved snap (geometry or grid), if any.
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>);

    /// The current transient visuals, in draw order.
    fn items(&self) -> &[ShapeItem];

    /// React to an **editing key** the host did not consume itself. `key` is the host's
    /// key name (`"backspace"`, `"delete"`, `"z"`, …) and `ctrl` reports the control
    /// modifier. Returns whether the tool consumed the key; the default leaves every
    /// key to the host.
    ///
    /// This is deliberately narrow: it is the channel a tool-local edit (deleting a
    /// placed vertex, undoing that removal) needs, not a general keymap.
    fn on_key(&mut self, _key: &str, _ctrl: bool) -> bool {
        false
    }

    /// The tool's live status for the host's tool strip. A tool with no state reports
    /// [`ToolStatus::inactive`].
    fn status(&self) -> ToolStatus {
        ToolStatus::inactive()
    }

    /// Cancel the session: clear every transient visual (points, rubber band, marker,
    /// …) and leave the document untouched. Returns whether anything was cancelled.
    fn cancel(&mut self) -> bool {
        false
    }

    /// Finish the session, returning the geometry to commit (if any).
    ///
    /// The host calls this on `Enter` and on the active-action surface's **Finish**
    /// button. The default ends the session without writing anything; a tool that
    /// produces an object returns a commit marked
    /// [`creating`](InteractionCommit::creating), so the host routes it to the workbench
    /// without resolving an entity first.
    fn finish(&mut self) -> Option<InteractionCommit> {
        None
    }

    /// A workplane this tool resolved, if it resolved one instead of geometry. The host
    /// applies it when the session ends; `finish` still runs and typically returns
    /// nothing.
    fn workplane(&self) -> Option<Workplane> {
        None
    }

    /// Whether this tool **picks** a workplane rather than drawing on one.
    ///
    /// A creation tool that starts on a planar face binds that face for its session
    /// (the ephemeral workplane). A picking tool must not: its points are the input, so
    /// every point must land where the pointer hit, not be flattened onto the first
    /// face's plane.
    fn picks_workplane(&self) -> bool {
        false
    }
}

/// The polyline tool's id in its status (the host's registry id is separate).
pub const POLYLINE_TOOL_ID: &str = "polyline";
/// The manipulator id a finished polyline commits under; the edition's `interaction`
/// hook creates the object for it.
pub const POLYLINE_MANIPULATOR_ID: &str = "polyline";
/// The label the tool strip shows.
pub const POLYLINE_TOOL_LABEL: &str = "Polyline";
/// The status hint before the first vertex is placed.
pub const POLYLINE_HINT_START: &str = "click to add a point · Esc to cancel";
/// The status hint once the polyline has a vertex.
pub const POLYLINE_HINT: &str =
    "click to add · drag a point to adjust · Enter to finish · Esc to cancel";
/// The status hint while the pointer is over a placed vertex.
pub const POLYLINE_HINT_HOVER: &str =
    "drag to adjust · click to select · Enter to finish · Esc to cancel";
/// The status hint while a placed vertex is being dragged.
pub const POLYLINE_HINT_DRAG: &str = "release to place the point · Esc to cancel";
/// The status hint while a placed vertex is the tool's active point.
pub const POLYLINE_HINT_ACTIVE: &str =
    "Backspace to delete · drag to adjust · Enter to finish · Esc to cancel";

/// The screen-space grab radius for one of the tool's own placed vertices, in device
/// pixels.
pub const VERTEX_PIXELS: f64 = 8.0;

/// The press-to-drag travel threshold, in device pixels: a press that releases inside
/// it is a click (select), not a drag (adjust).
pub const DRAG_THRESHOLD_PIXELS: f64 = 5.0;

/// The default colour of the committed polyline (and of a plotted point).
pub(crate) const COMMITTED_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The default colour of the in-progress band, marker and plane preview.
pub(crate) const CURSOR_COLOR: [f32; 4] = [1.0, 0.2, 1.0, 1.0];

/// A left press on a placed vertex, remembered until the release that ends the gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    /// The grabbed vertex.
    index: usize,
    /// Where the press landed, in device pixels (the drag threshold's origin).
    position: [i32; 2],
    /// Whether the pointer has travelled past [`DRAG_THRESHOLD_PIXELS`].
    dragging: bool,
}

/// A polyline tool (the `DraftWireTool` analogue): left-click to place vertices, with a
/// rubber band and a snap marker following the cursor.
///
/// Placed vertices stay editable while the session is open: press and drag one to slide
/// it in plane (the adjacent segments follow from the shared point list), or click it to
/// make it the **active point** — [`active`](Self::active) — which `Backspace`/`Delete`
/// removes. This is a tool sub-state, not a document edit: nothing leaves the tool until
/// the host finishes the session, so the document, the selection and the `Move` form are
/// untouched.
#[derive(Clone, Debug)]
pub struct PolylineTool {
    points: Vec<[f32; 3]>,
    cursor: Option<[f32; 3]>,
    /// The placed vertex under the pointer (the hover row), if any.
    hover: Option<usize>,
    /// The placed vertex a click selected (the tool's active point), if any.
    active: Option<usize>,
    /// A left press on a placed vertex, in flight.
    press: Option<Press>,
    /// Vertices removed by `Backspace`/`Delete`, most recent last, for the tool-local
    /// undo. Cleared by an append and by [`cancel`](Tool::cancel).
    removed: Vec<(usize, [f32; 3])>,
    committed_color: [f32; 4],
    cursor_color: [f32; 4],
    /// Three fixed slots — `[committed, band, marker]` — whose mesh identities are
    /// stable; only their geometry is rewritten. An empty slot emits nothing.
    items: Vec<ShapeItem>,
}

impl Default for PolylineTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PolylineTool {
    /// A new tool with the default colours.
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            cursor: None,
            hover: None,
            active: None,
            press: None,
            removed: Vec::new(),
            committed_color: COMMITTED_COLOR,
            cursor_color: CURSOR_COLOR,
            items: vec![
                wire_item(COMMITTED_COLOR),
                wire_item(CURSOR_COLOR),
                wire_item(CURSOR_COLOR),
            ],
        }
    }

    /// Set the committed-polyline and cursor colours.
    pub fn with_colors(mut self, committed: [f32; 4], cursor: [f32; 4]) -> Self {
        self.committed_color = committed;
        self.cursor_color = cursor;
        self.items = vec![
            wire_item(committed),
            wire_item(cursor),
            wire_item(cursor),
        ];
        self
    }

    /// The committed vertices, in placement order.
    pub fn points(&self) -> &[[f32; 3]] {
        &self.points
    }

    /// The placed vertex the pointer is over, if any (the hole the hover row fills).
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// The placed vertex a click selected (the tool-local active point), if any.
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// The placed vertex being dragged, if a drag is live.
    pub fn dragging(&self) -> Option<usize> {
        self.press.filter(|press| press.dragging).map(|press| press.index)
    }

    /// Reset: drop every committed vertex, the band, the marker and the editing state.
    pub fn clear(&mut self) {
        self.points.clear();
        self.cursor = None;
        self.hover = None;
        self.active = None;
        self.press = None;
        self.removed.clear();
        self.rebuild();
    }

    /// The placed vertex within [`VERTEX_PIXELS`] of `event.position`, nearest first.
    ///
    /// This is a **screen-space** test through the event's projection, so it stays
    /// correct at every zoom. Without a projection (a headless event) nothing is hit.
    fn vertex_at(&self, event: &PointerEvent) -> Option<usize> {
        let projection = event.projection?;
        let mut best: Option<(f64, usize)> = None;
        for (index, point) in self.points.iter().enumerate() {
            let Some(distance) = projection.pixels_to(*point, event.position) else {
                continue;
            };
            if distance > VERTEX_PIXELS {
                continue;
            }
            // Strictly closer only, so the earlier vertex breaks a tie.
            if best.is_none_or(|(closest, _)| distance < closest) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// Remove the active vertex, or the last one when nothing is active. The removal is
    /// remembered for [`PolylineTool::undo_removal`]. Returns whether a point was removed.
    fn remove_point(&mut self) -> bool {
        let index = self
            .active
            .filter(|index| *index < self.points.len())
            .or_else(|| self.points.len().checked_sub(1));
        let Some(index) = index else {
            return false;
        };
        let point = self.points.remove(index);
        self.removed.push((index, point));
        self.active = None;
        self.hover = None;
        self.press = None;
        self.rebuild();
        true
    }

    /// Restore the most recently removed vertex (the tool-local undo). Returns whether a
    /// point was restored.
    fn undo_removal(&mut self) -> bool {
        let Some((index, point)) = self.removed.pop() else {
            return false;
        };
        let index = index.min(self.points.len());
        self.points.insert(index, point);
        self.active = Some(index);
        self.hover = None;
        self.rebuild();
        true
    }

    /// Rewrite the three slots from the current points and cursor.
    fn rebuild(&mut self) {
        // Committed polyline (two or more placed vertices).
        let committed = if self.points.len() >= 2 {
            self.points.clone()
        } else {
            Vec::new()
        };
        set_polyline(&mut self.items[0], &committed);

        // A live drag owns the pointer, so the next-point band would read as an extra
        // edge; hide it until the release. Otherwise band from the last vertex to the
        // cursor.
        let band = if self.press.is_some_and(|press| press.dragging) {
            Vec::new()
        } else {
            match (self.points.last(), self.cursor) {
                (Some(&last), Some(cursor)) => vec![last, cursor],
                _ => Vec::new(),
            }
        };
        set_polyline(&mut self.items[1], &band);

        // Snap marker: on the dragged vertex while dragging, else at the cursor.
        let marker = match self.dragging() {
            Some(index) => self.points.get(index).copied(),
            None => self.cursor,
        };
        set_marker(&mut self.items[2], marker);
    }
}

impl PolylineTool {
    /// The hint for the current editing state.
    fn hint(&self) -> String {
        if let Some(index) = self.dragging() {
            return format!("adjusting point {} · {POLYLINE_HINT_DRAG}", index + 1);
        }
        if let Some(index) = self.active {
            return format!("point {} selected · {POLYLINE_HINT_ACTIVE}", index + 1);
        }
        if let Some(index) = self.hover {
            return format!("point {} · {POLYLINE_HINT_HOVER}", index + 1);
        }
        if self.points.is_empty() {
            POLYLINE_HINT_START.to_string()
        } else {
            POLYLINE_HINT.to_string()
        }
    }
}

impl Tool for PolylineTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        // Prefer the snap, then the raw hit point.
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        match event.kind {
            PointerKind::Move => {
                self.cursor = target;
                self.hover = self.vertex_at(event);
                if let Some(mut press) = self.press {
                    if !press.dragging
                        && pixels_between(press.position, event.position) >= DRAG_THRESHOLD_PIXELS
                    {
                        press.dragging = true;
                        self.press = Some(press);
                    }
                    if press.dragging {
                        if let Some(point) = target {
                            self.set_point(press.index, point);
                        }
                    }
                }
            }
            PointerKind::Down if event.button == Some(MouseButton::Left) => {
                self.hover = self.vertex_at(event);
                self.active = None;
                match self.hover {
                    // A press on a placed vertex is never an append: it becomes a drag,
                    // or a plain click that selects the vertex on release.
                    Some(index) => {
                        self.press = Some(Press {
                            index,
                            position: event.position,
                            dragging: false,
                        });
                        self.cursor = target;
                    }
                    None => {
                        self.press = None;
                        if let Some(point) = target {
                            self.points.push(point);
                            self.removed.clear();
                        }
                        self.cursor = target;
                    }
                }
            }
            PointerKind::Up if event.button == Some(MouseButton::Left) => {
                if let Some(mut press) = self.press.take() {
                    // A release that travelled is a drag even when no Move arrived.
                    let travelled = pixels_between(press.position, event.position)
                        >= DRAG_THRESHOLD_PIXELS;
                    if press.dragging || travelled {
                        press.dragging = true;
                        if let Some(point) = target {
                            self.set_point(press.index, point);
                        }
                    }
                    // Click or drag, the vertex the user touched becomes the active
                    // point (so Delete removes exactly the one they meant).
                    self.active = Some(press.index);
                    self.hover = self.vertex_at(event);
                    self.cursor = target;
                }
            }
            PointerKind::Down | PointerKind::Up => {}
        }
        self.rebuild();
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn status(&self) -> ToolStatus {
        ToolStatus::new(
            POLYLINE_TOOL_ID,
            POLYLINE_TOOL_LABEL,
            self.points.len(),
            self.hint(),
        )
    }

    fn on_key(&mut self, key: &str, ctrl: bool) -> bool {
        match key {
            "backspace" | "delete" => self.remove_point(),
            "z" if ctrl => self.undo_removal(),
            _ => false,
        }
    }

    fn cancel(&mut self) -> bool {
        let cancelled = !self.points.is_empty()
            || self.cursor.is_some()
            || self.hover.is_some()
            || self.active.is_some()
            || self.press.is_some()
            || !self.removed.is_empty();
        self.clear();
        cancelled
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        // Two points are the minimum that draws anything; a single click is not a
        // polyline and must not create an object.
        if self.points.len() < 2 {
            return None;
        }
        let values = self
            .points
            .iter()
            .flat_map(|point| {
                [
                    f64::from(point[0]),
                    f64::from(point[1]),
                    f64::from(point[2]),
                ]
            })
            .collect::<Vec<_>>();
        let commit = InteractionCommit::new(POLYLINE_MANIPULATOR_ID, 0, "create")
            .with_values(values)
            .with_readout(format!("{} points", self.points.len()))
            .creating();
        // The commit now owns the geometry: a second finish must not create a twin.
        self.clear();
        Some(commit)
    }
}

impl PolylineTool {
    /// Move one placed vertex, ignoring an index that no longer exists.
    fn set_point(&mut self, index: usize, point: [f32; 3]) {
        if let Some(vertex) = self.points.get_mut(index) {
            *vertex = point;
        }
    }
}

/// The distance between two device-pixel positions.
fn pixels_between(a: [i32; 2], b: [i32; 2]) -> f64 {
    (f64::from(a[0]) - f64::from(b[0])).hypot(f64::from(a[1]) - f64::from(b[1]))
}

/// A non-selectable wireframe item (a transient visual never takes part in picking).
pub(crate) fn wire_item(color: [f32; 4]) -> ShapeItem {
    ShapeItem::new(0)
        .with_draw_mode(DrawMode::Wireframe)
        .with_edge_color(color)
        .with_selectable(false)
}

/// Rewrite `item` as polyline `points` (`edges = [0,1,1,2,…]`). Fewer than two points
/// clears it.
pub(crate) fn set_polyline(item: &mut ShapeItem, points: &[[f32; 3]]) {
    let mut edges = Vec::new();
    for i in 0..points.len().saturating_sub(1) {
        edges.push(i as u32);
        edges.push(i as u32 + 1);
    }
    item.set_shape(points.to_vec(), Vec::new(), Vec::new(), edges);
}

/// Rewrite `item` as a small three-axis cross at `at` (or clear it when `None`).
fn set_marker(item: &mut ShapeItem, at: Option<[f32; 3]>) {
    const HALF: f32 = 0.05;
    let (positions, edges) = match at {
        Some(p) => (
            vec![
                [p[0] - HALF, p[1], p[2]],
                [p[0] + HALF, p[1], p[2]],
                [p[0], p[1] - HALF, p[2]],
                [p[0], p[1] + HALF, p[2]],
                [p[0], p[1], p[2] - HALF],
                [p[0], p[1], p[2] + HALF],
            ],
            vec![0, 1, 2, 3, 4, 5],
        ),
        None => (Vec::new(), Vec::new()),
    };
    item.set_shape(positions, Vec::new(), Vec::new(), edges);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::SnapKind;
    use ferrocad_graphics::{Hit, PickId, RenderQueue, Topology};

    fn event(kind: PointerKind, button: Option<MouseButton>, world: Option<[f32; 3]>) -> PointerEvent {
        PointerEvent {
            kind,
            position: [0, 0],
            button,
            hit: world.map(|point| Hit {
                id: PickId(1),
                point,
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            }),
            projection: None,
        }
    }

    /// A pointer event at `position`, carrying a projection that maps a world point to
    /// `(x * 0.5 + 0.5, y * 0.5 + 0.5)` × 1000 device pixels (identity view-projection).
    fn event_at(
        kind: PointerKind,
        button: Option<MouseButton>,
        world: Option<[f32; 3]>,
        position: [i32; 2],
    ) -> PointerEvent {
        PointerEvent {
            kind,
            position,
            button,
            hit: world.map(|point| Hit {
                id: PickId(1),
                point,
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            }),
            projection: Some(crate::input::ViewProjection::new(
                ferrocad_types::Matrix4::identity(),
                [1000, 1000],
            )),
        }
    }

    /// The identity projection's device pixel for a world `(x, y)`.
    fn pixel(world: [f32; 2]) -> [i32; 2] {
        [
            ((world[0] * 0.5 + 0.5) * 1000.0).round() as i32,
            ((world[1] * 0.5 + 0.5) * 1000.0).round() as i32,
        ]
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
    fn a_fresh_tool_emits_nothing() {
        assert!(emit(&PolylineTool::new()).is_empty());
    }

    #[test]
    fn clicks_accumulate_points_and_a_band_follows_the_cursor() {
        let mut tool = PolylineTool::new();

        // First click places a vertex; the marker appears at the cursor.
        tool.on_event(&event(PointerKind::Down, Some(MouseButton::Left), Some([0.0, 0.0, 0.0])), snap([0.0, 0.0, 0.0]));
        tool.on_event(&event(PointerKind::Move, None, Some([1.0, 0.0, 0.0])), snap([1.0, 0.0, 0.0]));
        assert_eq!(tool.points(), &[[0.0, 0.0, 0.0]]);
        // Only one committed point: a band (2 verts) + a marker. No committed polyline.
        assert_eq!(line_packets(&emit(&tool)), 2);

        // Second click commits a two-vertex polyline; the band starts afresh.
        tool.on_event(&event(PointerKind::Down, Some(MouseButton::Left), Some([1.0, 0.0, 0.0])), snap([1.0, 0.0, 0.0]));
        assert_eq!(tool.points().len(), 2);
        let queue = emit(&tool);
        // committed + band + marker = three line packets.
        assert_eq!(line_packets(&queue), 3);
        // The committed polyline is the first packet, with its two vertices.
        assert_eq!(queue.packets[0].state.id, PickId(0), "transient items are not pickable");

        // Clearing drops everything.
        tool.clear();
        assert!(emit(&tool).is_empty());
    }

    #[test]
    fn a_snap_is_preferred_over_the_raw_hit() {
        let mut tool = PolylineTool::new();
        let hit = [5.0, 5.0, 5.0];
        let snapped = [0.5, 0.0, 0.0];
        tool.on_event(
            &event(PointerKind::Down, Some(MouseButton::Left), Some(hit)),
            snap(snapped),
        );
        assert_eq!(tool.points(), &[snapped]);
    }

    #[test]
    fn a_non_left_button_does_not_place_a_point() {
        let mut tool = PolylineTool::new();
        tool.on_event(&event(PointerKind::Down, Some(MouseButton::Right), Some([0.0, 0.0, 0.0])), None);
        assert!(tool.points().is_empty());
    }

    #[test]
    fn the_status_names_the_tool_counts_the_points_and_hints_the_actions() {
        let mut tool = PolylineTool::new();
        let status = tool.status();
        assert!(status.active);
        assert_eq!(status.id, POLYLINE_TOOL_ID);
        assert_eq!(status.label, POLYLINE_TOOL_LABEL);
        assert_eq!(status.points, 0);
        assert!(status.hint.contains("Esc to cancel"), "{}", status.hint);

        tool.on_event(
            &event(PointerKind::Down, Some(MouseButton::Left), Some([1.0, 2.0, 0.0])),
            snap([1.0, 2.0, 0.0]),
        );
        let status = tool.status();
        assert_eq!(status.points, 1);
        assert!(status.hint.contains("Enter to finish"), "{}", status.hint);
        assert!(status.hint.contains("Esc to cancel"));
    }

    #[test]
    fn a_stateless_tool_is_inactive_and_cancels_nothing() {
        struct Nothing;
        impl Tool for Nothing {
            fn on_event(&mut self, _event: &PointerEvent, _snap: Option<Snap>) {}
            fn items(&self) -> &[ShapeItem] {
                &[]
            }
        }
        let mut tool = Nothing;
        let status = tool.status();
        assert!(!status.active);
        assert_eq!(status.points, 0);
        assert!(!tool.cancel());
    }

    #[test]
    fn cancelling_clears_every_transient_visual_and_reports_it() {
        let mut tool = PolylineTool::new();
        tool.on_event(
            &event(PointerKind::Down, Some(MouseButton::Left), Some([0.0, 0.0, 0.0])),
            snap([0.0, 0.0, 0.0]),
        );
        tool.on_event(
            &event(PointerKind::Move, None, Some([1.0, 0.0, 0.0])),
            snap([1.0, 0.0, 0.0]),
        );
        assert!(!tool.items()[1].positions().is_empty(), "a rubber band");

        assert!(tool.cancel(), "the session had geometry to drop");
        assert!(tool.points().is_empty());
        assert!(emit(&tool).is_empty(), "no transient visual survives");
        assert!(!tool.cancel(), "a second cancel has nothing to do");
    }

    /// Place one point at `world` through a left press at its projected pixel.
    fn place(tool: &mut PolylineTool, world: [f32; 3]) {
        let position = pixel([world[0], world[1]]);
        tool.on_event(
            &event_at(PointerKind::Down, Some(MouseButton::Left), Some(world), position),
            None,
        );
        tool.on_event(
            &event_at(PointerKind::Up, Some(MouseButton::Left), Some(world), position),
            None,
        );
    }

    #[test]
    fn hovering_a_placed_vertex_is_reported_in_the_tool_state() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [1.0, 0.0, 0.0]);

        // A move onto the first vertex reports it; a move off it clears the hover.
        tool.on_event(
            &event_at(PointerKind::Move, None, Some([0.0, 0.0, 0.0]), pixel([0.0, 0.0])),
            None,
        );
        assert_eq!(tool.hover(), Some(0));
        assert_eq!(tool.active(), None);
        assert!(tool.status().hint.contains("point 1"), "{}", tool.status().hint);

        tool.on_event(
            &event_at(PointerKind::Move, None, Some([0.0, 0.0, 0.0]), pixel([0.0, 0.5])),
            None,
        );
        assert_eq!(tool.hover(), None);
    }

    #[test]
    fn clicking_a_placed_vertex_selects_it_without_appending() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [1.0, 0.0, 0.0]);
        assert_eq!(tool.points().len(), 2);

        // A press inside the hit radius holds the gesture (nothing is appended)…
        let at = pixel([0.0, 0.0]);
        tool.on_event(
            &event_at(PointerKind::Down, Some(MouseButton::Left), Some([0.0, 0.0, 0.0]), at),
            None,
        );
        assert_eq!(tool.points().len(), 2, "a press on a vertex never appends");
        assert_eq!(tool.active(), None, "selection happens on release");

        // …and a release inside the drag threshold selects it instead.
        tool.on_event(
            &event_at(
                PointerKind::Up,
                Some(MouseButton::Left),
                Some([0.0, 0.0, 0.0]),
                [at[0] + 2, at[1] + 2],
            ),
            None,
        );
        assert_eq!(tool.points().len(), 2, "a click never appends");
        assert_eq!(tool.active(), Some(0), "the touched vertex becomes active");
        assert!(tool.status().hint.contains("Backspace to delete"), "{}", tool.status().hint);
    }

    #[test]
    fn dragging_a_placed_vertex_slides_it_and_keeps_the_point_count() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [1.0, 0.0, 0.0]);

        let start = pixel([0.0, 0.0]);
        tool.on_event(
            &event_at(PointerKind::Down, Some(MouseButton::Left), Some([0.0, 0.0, 0.0]), start),
            None,
        );

        // Past the threshold the grabbed vertex follows the pointer.
        let moved = [0.5, 0.5, 0.0];
        tool.on_event(
            &event_at(PointerKind::Move, None, Some(moved), pixel([0.5, 0.5])),
            snap(moved),
        );
        assert_eq!(tool.dragging(), Some(0));
        assert_eq!(tool.points(), &[moved, [1.0, 0.0, 0.0]]);
        assert_eq!(tool.points().len(), 2, "adjusting never changes the point count");

        // The adjacent segment is rubber-banded live: the committed wire carries the
        // moved vertex while the next-point band stays hidden during the drag.
        let committed = tool.items()[0].positions();
        assert_eq!(committed.len(), 2);
        assert!((committed[0][0] - 0.5).abs() < 1e-6, "{committed:?}");
        assert!(tool.items()[1].positions().is_empty(), "no trailing band mid-drag");

        // The release keeps the moved coordinate, ends the drag and selects the point.
        tool.on_event(
            &event_at(PointerKind::Up, Some(MouseButton::Left), Some(moved), pixel([0.5, 0.5])),
            snap(moved),
        );
        assert_eq!(tool.dragging(), None);
        assert_eq!(tool.active(), Some(0));
        assert_eq!(tool.points(), &[moved, [1.0, 0.0, 0.0]]);
    }

    #[test]
    fn a_click_on_empty_workplane_still_appends() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [0.5, 0.0, 0.0]);
        assert_eq!(tool.points(), &[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0]]);
        assert_eq!(tool.hover(), None);
        assert_eq!(tool.active(), None);
    }

    #[test]
    fn backspace_removes_the_active_or_last_vertex_and_ctrl_z_restores_it() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [0.5, 0.0, 0.0]);
        place(&mut tool, [1.0, 0.0, 0.0]);
        assert_eq!(tool.points().len(), 3);

        // No active point: the last vertex goes.
        assert!(tool.on_key("backspace", false));
        assert_eq!(tool.points(), &[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0]]);

        // The tool-local undo puts it back at its index.
        assert!(tool.on_key("z", true));
        assert_eq!(
            tool.points(),
            &[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [1.0, 0.0, 0.0]]
        );
        assert_eq!(tool.active(), Some(2), "the restored point is selected");

        // `Delete` removes the active point, not the last one.
        tool.on_event(
            &event_at(
                PointerKind::Down,
                Some(MouseButton::Left),
                Some([0.0, 0.0, 0.0]),
                pixel([0.0, 0.0]),
            ),
            None,
        );
        tool.on_event(
            &event_at(
                PointerKind::Up,
                Some(MouseButton::Left),
                Some([0.0, 0.0, 0.0]),
                pixel([0.0, 0.0]),
            ),
            None,
        );
        assert_eq!(tool.active(), Some(0));
        assert!(tool.on_key("delete", false));
        assert_eq!(
            tool.points(),
            &[[0.5, 0.0, 0.0], [1.0, 0.0, 0.0]],
            "the active vertex, not the last"
        );
        assert_eq!(tool.active(), None);

        // An unhandled key is left to the host.
        assert!(!tool.on_key("q", false));
    }

    #[test]
    fn cancelling_clears_the_editing_state_too() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [0.5, 0.0, 0.0]);
        assert!(tool.on_key("backspace", false));
        tool.on_event(
            &event_at(PointerKind::Move, None, None, pixel([0.0, 0.0])),
            None,
        );

        assert!(tool.cancel());
        assert!(tool.points().is_empty());
        assert_eq!(tool.hover(), None);
        assert_eq!(tool.active(), None);
        assert_eq!(tool.dragging(), None);
        assert!(emit(&tool).is_empty());
        assert!(!tool.on_key("z", true), "no removal survives the cancel");
    }

    #[test]
    fn every_transient_item_stays_non_selectable() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [0.5, 0.0, 0.0]);
        tool.on_event(
            &event_at(PointerKind::Move, None, Some([0.8, 0.8, 0.0]), pixel([0.8, 0.8])),
            None,
        );
        assert!(tool.items().iter().all(|item| !item.selectable()));
    }

    #[test]
    fn finishing_hands_over_the_points_as_a_creating_commit() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        place(&mut tool, [1.0, 0.0, 0.0]);
        place(&mut tool, [1.0, 1.0, 0.0]);

        let commit = tool.finish().expect("three points commit");
        assert!(commit.creates, "a finished tool creates, it does not edit");
        assert_eq!(commit.manipulator, POLYLINE_MANIPULATOR_ID);
        assert_eq!(commit.action, "create");
        assert_eq!(
            commit.values,
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0],
            "the world-space points, in order"
        );
        assert_eq!(commit.readout, "3 points");
        assert!(tool.points().is_empty(), "the commit owns the geometry now");
        assert!(tool.finish().is_none(), "a second finish cannot create a twin");
        assert!(emit(&tool).is_empty(), "nothing is left on screen");
    }

    #[test]
    fn a_single_point_finishes_without_a_commit() {
        let mut tool = PolylineTool::new();
        place(&mut tool, [0.0, 0.0, 0.0]);
        assert!(tool.finish().is_none(), "one point is not a polyline");
    }
}
