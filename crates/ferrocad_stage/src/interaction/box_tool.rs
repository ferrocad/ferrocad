//! The **Box** tool: the zero-mouse primitive and C2's canonical numeric strip.
//!
//! [`BoxTool`] is armed and does nothing else: it previews a box at the active
//! workplane (or on the face the pointer is over) and commits it on `Enter`/`✓`, so
//! `⊕ Box` then `Enter` creates a 20 × 20 × 20 box without touching the pointer — the
//! zero-mouse fast path the old static `Create Box` dock button had, now inside the
//! creation pill.
//!
//! It is also the tool the **field strip** exists for: `W`, `L` and `H` are
//! [`ToolField`]s, and [`Tool::set_field`] writes a typed value back into the tool, which
//! resizes the preview and the commit. A click anchors the corner (the same press binds a
//! face's plane, exactly as the drawing tools do); without a click the box sits at the
//! placement frame's origin.

use ferrocad_types::Vector3;

use crate::frame::vector;
use crate::input::{MouseButton, PointerEvent, PointerKind};
use crate::interaction::{HoverTarget, InteractionCommit};
use crate::shape::ShapeItem;
use crate::snap::Snap;
use crate::tool::{
    CURSOR_COLOR, Tool, ToolField, ToolStatus, ghost_item, set_ghost_loop, set_polyline,
};

/// The tool's id in its status (the host's registry id is separate).
pub const BOX_TOOL_ID: &str = "box";
/// The manipulator id a committed box uses; the edition's `interaction` hook creates it.
pub const BOX_MANIPULATOR_ID: &str = "box";
/// The operation within the manipulator.
pub const BOX_ACTION_CREATE: &str = "create";
/// The label the tool strip shows.
pub const BOX_TOOL_LABEL: &str = "Box";
/// The status hint: the box is ready immediately, and the numbers are the input.
pub const BOX_HINT: &str =
    "type a size · click to place the corner · Enter to create · Esc to cancel";
/// The default edge length of each side, in millimetres (upstream's Box default).
pub const BOX_DEFAULT_SIZE: f64 = 20.0;
/// The smallest acceptable side, in millimetres.
pub const BOX_MIN_SIZE: f64 = 1e-3;
/// The largest acceptable side, in millimetres.
pub const BOX_MAX_SIZE: f64 = 1e6;

/// The three field ids, in strip order.
pub const BOX_FIELDS: [&str; 3] = ["w", "l", "h"];

/// A box with a width, a length and a height, committed on `Enter`.
#[derive(Clone, Debug)]
pub struct BoxTool {
    width: f64,
    length: f64,
    height: f64,
    /// The placement under the pointer (its frame decides the plane and the corner).
    hover: Option<HoverTarget>,
    /// The corner a press anchored, in world space.
    anchor: Option<Vector3>,
    /// Two fixed slots — `[ghost, anchor cue]` — whose mesh identities are stable.
    items: Vec<ShapeItem>,
}

impl Default for BoxTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxTool {
    /// A new tool with a 20 mm cube.
    pub fn new() -> Self {
        let mut tool = Self {
            width: BOX_DEFAULT_SIZE,
            length: BOX_DEFAULT_SIZE,
            height: BOX_DEFAULT_SIZE,
            hover: None,
            anchor: None,
            items: vec![ghost_item(CURSOR_COLOR), ghost_item(CURSOR_COLOR)],
        };
        // The box is ready before any pointer event: the preview is the strip's numbers.
        tool.rebuild();
        tool
    }

    /// The box's dimensions, in millimetres.
    pub fn size(&self) -> [f64; 3] {
        [self.width, self.length, self.height]
    }

    /// The corner the box grows from: the press, else the placement frame's origin,
    /// else the world origin.
    pub fn origin(&self) -> Vector3 {
        if let Some(anchor) = self.anchor {
            return anchor;
        }
        self.hover
            .as_ref()
            .map(|target| target.frame.origin)
            .unwrap_or_else(Vector3::zero)
    }

    /// The placement basis: the hovered frame's, or the world's.
    fn basis(&self) -> [Vector3; 3] {
        self.hover.as_ref().map(|target| target.frame.basis).unwrap_or([
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ])
    }

    /// The box's eight corners: `origin + u·w + v·l + n·h`.
    fn corners(&self) -> Vec<Vector3> {
        let [u, v, n] = self.basis();
        let origin = self.origin();
        let mut corners = Vec::with_capacity(8);
        for (a, b, c) in [
            (0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (1.0, 0.0, 1.0),
            (1.0, 1.0, 1.0),
            (0.0, 1.0, 1.0),
        ] {
            corners.push(
                origin
                    .add(&u.scale(a * self.width))
                    .add(&v.scale(b * self.length))
                    .add(&n.scale(c * self.height)),
            );
        }
        corners
    }

    /// The ghost mesh: twelve triangles and the twelve outline edges.
    fn rebuild(&mut self) {
        let corners = self.corners();
        let points: Vec<[f32; 3]> = corners
            .iter()
            .map(|corner| [corner.x as f32, corner.y as f32, corner.z as f32])
            .collect();
        // Six quads, two triangles each; the winding is outward for a right-handed
        // frame, so the ghost shades rather than showing its back faces.
        let quads = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut triangles = Vec::with_capacity(36);
        for [a, b, c, d] in quads {
            triangles.extend_from_slice(&[a, b, c, a, c, d]);
        }
        let mut edges = Vec::with_capacity(24);
        for quad in quads {
            for index in 0..4 {
                edges.push(quad[index] as u32);
                edges.push(quad[(index + 1) % 4] as u32);
            }
        }
        self.items[0].set_shape(points, Vec::new(), triangles, edges);
        // The anchor cue: a small square at the base corner, so the committed corner is
        // visible even when the ghost is edge-on.
        let cue = self
            .hover
            .as_ref()
            .map(|target| target.marker(super::tool::HOVER_MARKER_HALF))
            .unwrap_or_default();
        set_ghost_loop(&mut self.items[1], &cue);
        if self.anchor.is_some() {
            set_polyline(&mut self.items[1], &[]);
        }
    }

    /// The strip's fields, from the current size.
    fn fields(&self) -> Vec<ToolField> {
        [
            ("w", "W", self.width),
            ("l", "L", self.length),
            ("h", "H", self.height),
        ]
        .into_iter()
        .map(|(id, label, value)| ToolField {
            min: Some(BOX_MIN_SIZE),
            max: Some(BOX_MAX_SIZE),
            step: 1.0,
            ..ToolField::number(id, label, value, "mm")
        })
        .collect()
    }

    /// Set one side from typed text, refusing what is not a usable length.
    fn edit(&mut self, id: &str, text: &str) -> bool {
        let Ok(value) = text.trim().parse::<f64>() else {
            return false;
        };
        if !value.is_finite() || !(BOX_MIN_SIZE..=BOX_MAX_SIZE).contains(&value) {
            return false;
        }
        match id {
            "w" => self.width = value,
            "l" => self.length = value,
            "h" => self.height = value,
            _ => return false,
        }
        self.rebuild();
        true
    }
}

impl Tool for BoxTool {
    fn on_event(&mut self, event: &PointerEvent, snap: Option<Snap>) {
        let target = snap.map(|snap| snap.point).or_else(|| event.world());
        if event.kind == PointerKind::Down
            && event.button == Some(MouseButton::Left)
            && let Some(point) = target
        {
            // A press anchors the corner. It is not a completion: `Enter`/`✓` commits,
            // so the numbers can be typed after the placement (and a second press moves
            // the corner rather than creating a second box).
            self.anchor = Some(vector(point));
            self.rebuild();
        }
    }

    fn items(&self) -> &[ShapeItem] {
        &self.items
    }

    fn hover(&mut self, target: Option<&HoverTarget>) {
        if self.anchor.is_some() {
            return;
        }
        self.hover = target.cloned();
        self.rebuild();
    }

    fn anchor(&self) -> Option<Vector3> {
        // The corner the box grows from: the press once it is placed, else the placement
        // the ghost is sitting on.
        self.anchor
            .or_else(|| self.hover.as_ref().map(|target| target.frame.origin))
    }

    fn status(&self) -> ToolStatus {
        ToolStatus::new(
            BOX_TOOL_ID,
            BOX_TOOL_LABEL,
            usize::from(self.anchor.is_some()),
            BOX_HINT,
        )
        .with_fields(self.fields())
    }

    fn set_field(&mut self, id: &str, text: &str) -> bool {
        self.edit(id, text)
    }

    fn cancel(&mut self) -> bool {
        let had = self.hover.is_some() || self.anchor.is_some();
        self.hover = None;
        self.anchor = None;
        self.rebuild();
        had
    }

    fn finish(&mut self) -> Option<InteractionCommit> {
        let [w, l, h] = self.size();
        if w < BOX_MIN_SIZE || l < BOX_MIN_SIZE || h < BOX_MIN_SIZE {
            return None;
        }
        let origin = self.origin();
        let commit = InteractionCommit::new(BOX_MANIPULATOR_ID, 0, BOX_ACTION_CREATE)
            .with_values(vec![
                w, l, h, origin.x, origin.y, origin.z,
            ])
            .with_readout(format!("{w:.1} × {l:.1} × {h:.1} mm"))
            .creating();
        self.hover = None;
        self.anchor = None;
        self.rebuild();
        Some(commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Workplane;
    use crate::gizmo::ManipulatorFrame;
    use crate::interaction::Tool;

    #[test]
    fn the_default_is_a_twenty_millimetre_cube_with_three_fields() {
        let tool = BoxTool::new();
        assert_eq!(tool.size(), [20.0, 20.0, 20.0]);
        let status = tool.status();
        assert!(status.active);
        assert_eq!(
            status.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["w", "l", "h"]
        );
        assert_eq!(status.fields[0].value, "20.00");
        assert_eq!(status.fields[0].unit, "mm");
        // The ghost is already drawn: the box is ready before any click.
        assert_eq!(tool.items()[0].positions().len(), 8);
    }

    #[test]
    fn a_field_write_back_resizes_the_ghost_and_the_commit() {
        let mut tool = BoxTool::new();
        assert!(tool.set_field("w", "35.5"));
        assert_eq!(tool.size(), [35.5, 20.0, 20.0]);
        assert_eq!(tool.status().fields[0].value, "35.50");
        // The corner moved: the ghost's far corner is `origin + (35.5, 0, 0)`.
        let positions = tool.items()[0].positions().to_vec();
        assert_eq!(positions[1], [35.5, 0.0, 0.0]);

        // A refusal leaves the tool as it was.
        assert!(!tool.set_field("w", "wide"));
        assert!(!tool.set_field("w", "0"));
        assert!(!tool.set_field("q", "3"));
        assert_eq!(tool.size(), [35.5, 20.0, 20.0]);

        let commit = tool.finish().expect("a commit");
        assert!(commit.creates);
        assert_eq!(commit.manipulator, BOX_MANIPULATOR_ID);
        assert_eq!(&commit.values[0..3], &[35.5, 20.0, 20.0]);
        assert_eq!(&commit.values[3..6], &[0.0, 0.0, 0.0], "the world origin");
        assert_eq!(commit.readout, "35.5 × 20.0 × 20.0 mm");
    }

    #[test]
    fn a_hovering_face_places_the_box_on_that_plane() {
        let target = HoverTarget {
            frame: ManipulatorFrame {
                origin: Vector3::new(5.0, 5.0, 10.0),
                basis: [
                    Vector3::new(1.0, 0.0, 0.0),
                    Vector3::new(0.0, 1.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                ],
            },
            host: Some(crate::interaction::CreationHost::element(1, "Face6")),
            kind: crate::interaction::TargetKind::Face,
        };
        let mut tool = BoxTool::new();
        tool.hover(Some(&target));
        assert_eq!(tool.origin(), Vector3::new(5.0, 5.0, 10.0));
        let commit = tool.finish().expect("a commit");
        assert_eq!(&commit.values[3..6], &[5.0, 5.0, 10.0]);
        // A box is a new body, so it names no host.
        assert!(commit.host.is_none());
    }

    /// The tool publishes the corner it is working at, so a host can put the session's
    /// `✗`/`✓` pair **beside the box** instead of under the pointer (where the next click
    /// lands).
    #[test]
    fn the_anchor_is_the_corner_the_box_grows_from() {
        let mut tool = BoxTool::new();
        assert_eq!(tool.anchor(), None, "nothing placed and no hover: no anchor");

        // The hovered placement: the ghost's corner, before any click.
        let target = HoverTarget::of_workplane(&Workplane::world());
        tool.hover(Some(&target));
        assert_eq!(tool.anchor(), Some(target.frame.origin));

        // A press replaces it with the corner it anchored.
        tool.on_event(
            &PointerEvent {
                kind: PointerKind::Down,
                position: [0, 0],
                button: Some(MouseButton::Left),
                hit: None,
                projection: None,
            },
            Some(Snap {
                point: [2.0, 3.0, 4.0],
                kind: crate::snap::SnapKind::Point,
                local: None,
                major: false,
            }),
        );
        assert_eq!(tool.anchor(), Some(Vector3::new(2.0, 3.0, 4.0)));
    }

    #[test]
    fn a_click_anchors_the_corner_and_enter_still_finishes() {
        use crate::input::PointerEvent;

        let mut tool = BoxTool::new();
        assert_eq!(tool.status().points, 0);
        tool.on_event(
            &PointerEvent {
                kind: PointerKind::Down,
                position: [0, 0],
                button: Some(MouseButton::Left),
                hit: None,
                projection: None,
            },
            Some(Snap {
                point: [2.0, 3.0, 4.0],
                kind: crate::snap::SnapKind::Point,
                local: None,
                major: false,
            }),
        );
        assert_eq!(tool.status().points, 1);
        assert_eq!(tool.origin(), Vector3::new(2.0, 3.0, 4.0));
        // The click did not complete the tool: `Enter` is what creates the box.
        assert!(!tool.completed());
        let commit = tool.finish().expect("a commit");
        assert_eq!(&commit.values[3..6], &[2.0, 3.0, 4.0]);
        assert!(tool.hover.is_none() && tool.anchor.is_none(), "the commit owns the box");
    }
}
