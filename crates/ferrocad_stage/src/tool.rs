//! Interactive tools: the replacement for `SoEventCallback`.
//!
//! A [`Tool`] reacts to [`PointerEvent`]s and produces **transient** visuals — a rubber
//! band, a snap marker, a preview — that the host draws alongside the document geometry
//! without touching it. Transient items are rebuilt as events arrive, so a tool never
//! mutates the scene graph (there is none) and never allocates a document object.

use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::shape::{DrawMode, ShapeItem};
use crate::snap::Snap;

/// An interactive tool.
///
/// `on_event` is called for every pointer event; [`items`](Self::items) returns the
/// transient visuals to draw this frame. The items reuse stable mesh identities across
/// events (only their geometry changes), so a renderer's upload cache is not flooded.
pub trait Tool {
    /// React to a pointer event. `snap` is the resolved snap (geometry or grid), if any.
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>);

    /// The current transient visuals, in draw order.
    fn items(&self) -> &[ShapeItem];
}

/// The default colour of the committed polyline.
const COMMITTED_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The default colour of the in-progress band and the snap marker.
const CURSOR_COLOR: [f32; 4] = [1.0, 0.2, 1.0, 1.0];

/// A polyline tool (the `DraftWireTool` analogue): left-click to place vertices, with a
/// rubber band and a snap marker following the cursor.
#[derive(Clone, Debug)]
pub struct PolylineTool {
    points: Vec<[f32; 3]>,
    cursor: Option<[f32; 3]>,
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

    /// Reset: drop every committed vertex, the band and the marker.
    pub fn clear(&mut self) {
        self.points.clear();
        self.cursor = None;
        self.rebuild();
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

        // Rubber band from the last placed vertex to the cursor.
        let band = match (self.points.last(), self.cursor) {
            (Some(&last), Some(cursor)) => vec![last, cursor],
            _ => Vec::new(),
        };
        set_polyline(&mut self.items[1], &band);

        // Snap marker at the cursor.
        set_marker(&mut self.items[2], self.cursor);
    }
}

impl Tool for PolylineTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        // Prefer the snap, then the raw hit point.
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        match event.kind {
            PointerKind::Move => self.cursor = target,
            PointerKind::Down if event.button == Some(MouseButton::Left) => {
                if let Some(point) = target {
                    self.points.push(point);
                }
                self.cursor = target;
            }
            PointerKind::Down | PointerKind::Up => {}
        }
        self.rebuild();
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }
}

/// A non-selectable wireframe item (a transient visual never takes part in picking).
fn wire_item(color: [f32; 4]) -> ShapeItem {
    ShapeItem::new(0)
        .with_draw_mode(DrawMode::Wireframe)
        .with_edge_color(color)
        .with_selectable(false)
}

/// Rewrite `item` as polyline `points` (`edges = [0,1,1,2,…]`). Fewer than two points
/// clears it.
fn set_polyline(item: &mut ShapeItem, points: &[[f32; 3]]) {
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
}
