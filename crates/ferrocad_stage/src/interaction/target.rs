//! What the pointer is over, resolved as a placement frame.
//!
//! A creation tool that is armed but has not placed its anchor asks one question of the
//! scene: *where would this go?* The answer is a [`HoverTarget`] — the placement frame
//! plus, when the pointer is over a body, the [`CreationHost`] that frame came from.
//!
//! The three cases are the two-hand design's, in `ferrocad_stage`'s vocabulary:
//!
//! | Hover | Frame | Host |
//! | --- | --- | --- |
//! | empty space | the active workplane's `u`/`v`/`n` | `None` |
//! | planar `FaceN` | the face's centroid, tangent plane, outward normal | `FaceN` |
//! | `EdgeN` | the edge's midpoint, tangent as `z` | `EdgeN` |
//!
//! The tool does not have to *use* it — a polyline anchors on its own clicks — but a
//! tool that draws a ghost can place it here before the first press, and the viewport
//! binds the same frame as the session workplane when the press lands, which is what
//! makes the preview and the committed geometry agree.

use ferrocad_types::Vector3;

use crate::gizmo::ManipulatorFrame;
use crate::interaction::CreationHost;
use crate::workplane::{FaceFrame, Workplane};

/// Where a [`HoverTarget`]'s frame came from.
///
/// The frame is the same `origin + basis` either way; the kind is what a tool reads to
/// decide whether it may *anchor* there. A drawing tool draws in a **face's** plane and
/// can imprint on it, while an **edge** frame names a direction (a sweep path, a datum
/// axis) rather than a plane, so a profile tool binds the face kind only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    /// Empty space: the active workplane's frame, and no body under the pointer.
    Workplane,
    /// A planar face of a body.
    Face,
    /// An edge of a body.
    Edge,
}

/// The placement a creation tool would use at the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverTarget {
    /// The frame the tool previews in.
    pub frame: ManipulatorFrame,
    /// The body it came from, or `None` for the active workplane.
    pub host: Option<CreationHost>,
    /// What the frame was resolved from.
    pub kind: TargetKind,
}

impl HoverTarget {
    /// The active workplane: no body under the pointer.
    pub fn of_workplane(plane: &Workplane) -> Self {
        Self {
            frame: ManipulatorFrame::of_workplane(plane),
            host: None,
            kind: TargetKind::Workplane,
        }
    }

    /// A planar face of body `entity`: the face's own frame, bound as the host.
    pub fn of_face(entity: u32, element: impl Into<String>, face: &FaceFrame) -> Self {
        Self {
            frame: ManipulatorFrame::of_workplane(&face.plane),
            host: Some(CreationHost::element(entity, element)),
            kind: TargetKind::Face,
        }
    }

    /// An edge of body `entity`: its first polyline segment sets the frame.
    ///
    /// `z` is the segment's tangent and the origin is the midpoint, so a tool that
    /// sweeps along the edge (or aligns a datum to it) has the direction the pointer
    /// found. `None` when the edge has no usable segment.
    pub fn of_edge(entity: u32, element: impl Into<String>, points: &[[f32; 3]]) -> Option<Self> {
        let (origin, tangent) = edge_axis(points)?;
        Some(Self {
            frame: ManipulatorFrame {
                origin,
                basis: [
                    perpendicular(tangent),
                    tangent.cross(&perpendicular(tangent)),
                    tangent,
                ],
            },
            host: Some(CreationHost::element(entity, element)),
            kind: TargetKind::Edge,
        })
    }

    /// The same placement with its origin moved to the point under the pointer.
    ///
    /// The basis is the face's (or the workplane's); only the origin follows the
    /// pointer, which is what makes the ghost sit where the press will land rather
    /// than at the face's centroid.
    pub fn with_origin(mut self, origin: Vector3) -> Self {
        self.frame.origin = origin;
        self
    }

    /// Whether the frame is a body's (the pointer is over geometry).
    pub fn is_hosted(&self) -> bool {
        self.host.is_some()
    }

    /// Whether the frame is a planar face's (a drawing tool may anchor here).
    pub fn is_face(&self) -> bool {
        self.kind == TargetKind::Face
    }

    /// The body (and element) the shell must name on the commit.
    pub fn host(&self) -> Option<&CreationHost> {
        self.host.as_ref()
    }

    /// A small closed square in this frame, centred on the origin.
    ///
    /// This is the anchor cue a tool shows before its first press: the square is in the
    /// placement plane, so it reads as "this is where the anchor lands, on this plane".
    /// `half` is its half-width in millimetres; the loop repeats its first point, as the
    /// tools' ghost setter (`set_ghost_loop`) expects.
    pub fn marker(&self, half: f32) -> Vec<[f32; 3]> {
        let [x, y, _] = self.frame.basis;
        let corner = |a: f64, b: f64| -> [f32; 3] {
            let point = self
                .frame
                .origin
                .add(&x.scale(a))
                .add(&y.scale(b));
            [point.x as f32, point.y as f32, point.z as f32]
        };
        let size = f64::from(half);
        let mut points = vec![
            corner(-size, -size),
            corner(size, -size),
            corner(size, size),
            corner(-size, size),
        ];
        points.push(points[0]);
        points
    }
}

/// The midpoint and unit tangent of an edge's polyline, longest segment first.
fn edge_axis(points: &[[f32; 3]]) -> Option<(Vector3, Vector3)> {
    let mut best: Option<(f64, Vector3, Vector3)> = None;
    for pair in points.windows(2) {
        let start = Vector3::new(
            f64::from(pair[0][0]),
            f64::from(pair[0][1]),
            f64::from(pair[0][2]),
        );
        let end = Vector3::new(
            f64::from(pair[1][0]),
            f64::from(pair[1][1]),
            f64::from(pair[1][2]),
        );
        let delta = end.sub(&start);
        let length = delta.length();
        if length <= f64::EPSILON {
            continue;
        }
        if best.as_ref().is_none_or(|(longest, ..)| length > *longest) {
            let midpoint = start.add(&end).scale(0.5);
            best = Some((length, midpoint, delta.scale(1.0 / length)));
        }
    }
    best.map(|(_, origin, tangent)| (origin, tangent))
}

/// Any unit vector square to `axis`, deterministic in the axis.
fn perpendicular(axis: Vector3) -> Vector3 {
    let seed = if axis.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let cross = seed.cross(&axis);
    cross.scale(1.0 / cross.length().max(f64::EPSILON))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workplane::Workplane;

    #[test]
    fn the_workplane_target_is_the_active_plane_and_unhosted() {
        let plane = Workplane::world();
        let target = HoverTarget::of_workplane(&plane);
        assert!(!target.is_hosted());
        assert!(!target.is_face());
        assert_eq!(target.kind, TargetKind::Workplane);
        assert_eq!(target.frame.origin, plane.origin);
    }

    #[test]
    fn a_face_target_carries_its_face_and_the_faces_frame() {
        // A unit square in the z = 3 plane, wound so the normal is +Z.
        let positions = [
            [0.0_f32, 0.0, 3.0],
            [2.0, 0.0, 3.0],
            [2.0, 2.0, 3.0],
            [0.0, 2.0, 3.0],
        ];
        let triangles = [0_u32, 1, 2, 0, 2, 3];
        let parts = [0_u32; 4];
        let face = FaceFrame::resolve(7, "Face6", &positions, &triangles, &parts, 0)
            .expect("a planar face");
        let target = HoverTarget::of_face(7, "Face6", &face);
        assert!(target.is_hosted());
        assert!(target.is_face());
        assert_eq!(target.kind, TargetKind::Face);
        let host = target.host.expect("a host");
        assert_eq!(host.entity, 7);
        assert_eq!(host.element.as_deref(), Some("Face6"));
        assert!(
            (target.frame.origin.z - 3.0).abs() < 1e-9,
            "{:?}",
            target.frame.origin
        );
    }

    #[test]
    fn an_edge_target_takes_the_longest_segments_tangent_as_z() {
        // Two segments; the second is longer, so it sets the axis.
        let points = [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 4.0, 0.0]];
        let target = HoverTarget::of_edge(9, "Edge3", &points).expect("an edge frame");
        assert!(target.is_hosted());
        assert!(
            (target.frame.basis[2].y - 1.0).abs() < 1e-9,
            "{:?}",
            target.frame.basis[2]
        );
        assert!(
            (target.frame.origin.y - 2.0).abs() < 1e-9,
            "{:?}",
            target.frame.origin
        );
        // The basis is right-handed and orthonormal.
        let [x, y, z] = target.frame.basis;
        assert!((x.dot(&y)).abs() < 1e-9);
        assert!((x.length() - 1.0).abs() < 1e-9);
        assert!((x.cross(&y).sub(&z).length()).abs() < 1e-9);
    }

    #[test]
    fn an_edge_with_no_segment_has_no_target() {
        assert!(HoverTarget::of_edge(1, "Edge1", &[[1.0, 2.0, 3.0]]).is_none());
        assert!(HoverTarget::of_edge(1, "Edge1", &[]).is_none());
        // A zero-length segment is not a direction.
        let degenerate = [[1.0_f32, 1.0, 1.0], [1.0, 1.0, 1.0]];
        assert!(HoverTarget::of_edge(1, "Edge1", &degenerate).is_none());
    }

    #[test]
    fn an_edge_is_hosted_but_is_not_a_drawing_face() {
        let points = [[0.0_f32, 0.0, 0.0], [4.0, 0.0, 0.0]];
        let target = HoverTarget::of_edge(9, "Edge3", &points).expect("an edge frame");
        assert!(target.is_hosted());
        assert!(!target.is_face(), "a drawing tool must not anchor on an edge");
        assert_eq!(target.kind, TargetKind::Edge);
    }

    #[test]
    fn the_origin_follows_the_pointer_and_the_basis_does_not() {
        let plane = Workplane::world();
        let target = HoverTarget::of_workplane(&plane).with_origin(Vector3::new(3.0, 4.0, 5.0));
        assert_eq!(target.frame.origin, Vector3::new(3.0, 4.0, 5.0));
        assert_eq!(target.frame.basis, ManipulatorFrame::of_workplane(&plane).basis);
    }

    #[test]
    fn the_marker_is_a_closed_square_in_the_frames_plane() {
        // A ground frame: the square lies at z = 0, centred on the origin.
        let target = HoverTarget::of_workplane(&Workplane::world())
            .with_origin(Vector3::new(1.0, 2.0, 0.0));
        let marker = target.marker(2.0);
        assert_eq!(marker.len(), 5, "four corners and the repeat");
        assert_eq!(marker[0], marker[4], "the loop closes");
        for point in &marker {
            assert_eq!(point[2], 0.0, "in the plane: {point:?}");
        }
        // Two millimetres out along each in-plane axis from the origin.
        assert_eq!(marker[0], [-1.0, 0.0, 0.0], "the near corner at (-2, -2)");
        assert_eq!(marker[2], [3.0, 4.0, 0.0], "the far corner at (+2, +2)");
    }
}
