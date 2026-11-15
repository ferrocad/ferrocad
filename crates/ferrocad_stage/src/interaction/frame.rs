//! The world frame a pointer drag sweeps out.
//!
//! Every creation tool asks the same first question: which plane did the user draw
//! in? When no face is bound the answer is the axis least aligned with the drag — the
//! plane the pointer swept — and the tools share that rule here, so a rectangle's
//! sides stay square to the world axes while a circle's rim stays in the plane the
//! pointer travelled.
//!
//! [`DragFrame`] is that plane plus the centre and radius the drag named, and the
//! sampling a round tool previews with: a ring, a regular polygon, an arc through a
//! sweep. The tools hand the shell a commit of world points, so nothing downstream
//! re-derives the plane from a rule.

use std::f64::consts::{PI, TAU};

use ferrocad_types::Vector3;

/// The shortest drag, in millimetres, that names a plane or a radius.
pub const MIN_DRAG: f64 = 1e-6;

/// How many segments a full circle's preview uses (upstream's preview uses 96; 64 is
/// smooth at any zoom the shell draws and a third cheaper).
pub const RING_SEGMENTS: usize = 64;

/// An arc preview samples at least one point per this much sweep, in radians (5°).
const ARC_STEP: f64 = PI / 36.0;

/// The two world axes a drag in `delta` spans, `(u, v)` in right-handed order.
///
/// `u × v` is the plane's normal: the axis **least aligned** with the drag is
/// dropped. A flat drag therefore lands in world `XY`, a vertical one in `XZ`, and a
/// drag along `Y` in `YZ`.
pub fn drag_axes(delta: Vector3) -> (Vector3, Vector3) {
    let (x, y, z) = (delta.x.abs(), delta.y.abs(), delta.z.abs());
    if z <= x && z <= y {
        (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0))
    } else if y <= x {
        (Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0))
    } else {
        (Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0))
    }
}

/// A drawing frame: the centre, the plane's two axes and its normal.
///
/// `u` points at the drag's own in-plane direction and `v` is square to it, so a
/// polygon's first vertex lands where the pointer is and a circle's rim passes
/// through it. The normal is kept as the world axis [`drag_axes`] named rather than
/// recomputed from `u × v`, so a ground-plane circle commits exactly `(0, 0, 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragFrame {
    /// The point the drag started from.
    pub center: Vector3,
    /// The in-plane radius: the drag with the normal dropped.
    pub radius: f64,
    /// The in-plane axis the drag points along.
    pub u: Vector3,
    /// The in-plane axis square to `u`.
    pub v: Vector3,
    /// The plane's normal: the world axis the drag is least aligned with.
    pub normal: Vector3,
}

impl DragFrame {
    /// The frame `center → cursor` names, or `None` when the drag is too short to
    /// have a radius once its off-plane component is dropped.
    pub fn from_drag(center: Vector3, cursor: Vector3) -> Option<Self> {
        let delta = cursor.sub(&center);
        let (a, b) = drag_axes(delta);
        let in_plane = a.scale(delta.dot(&a)).add(&b.scale(delta.dot(&b)));
        let radius = in_plane.length();
        if !radius.is_finite() || radius < MIN_DRAG {
            return None;
        }
        let u = in_plane.scale(1.0 / radius);
        let normal = a.cross(&b);
        Some(Self {
            center,
            radius,
            u,
            v: normal.cross(&u),
            normal,
        })
    }

    /// The plane's normal (`u × v`, kept exact for a world-aligned drag).
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The rim point at `angle` radians, measured from `u` toward `v`.
    pub fn point(&self, angle: f64) -> Vector3 {
        self.center
            .add(&self.u.scale(self.radius * angle.cos()))
            .add(&self.v.scale(self.radius * angle.sin()))
    }

    /// The closed circle, [`RING_SEGMENTS`] segments, first point repeated last.
    pub fn ring(&self) -> Vec<Vector3> {
        let mut points: Vec<Vector3> = (0..RING_SEGMENTS)
            .map(|index| self.point(TAU * index as f64 / RING_SEGMENTS as f64))
            .collect();
        if let Some(first) = points.first().copied() {
            points.push(first);
        }
        points
    }

    /// The closed regular polygon with `sides` vertices, the first at `u`.
    pub fn polygon(&self, sides: usize) -> Vec<Vector3> {
        let mut points: Vec<Vector3> = (0..sides)
            .map(|index| self.point(TAU * index as f64 / sides as f64))
            .collect();
        if let Some(first) = points.first().copied() {
            points.push(first);
        }
        points
    }

    /// The open arc from `u` through `sweep` radians, sampled at [`ARC_STEP`].
    ///
    /// A negative sweep runs clockwise (from `u` toward `-v`), and a sweep past a
    /// half turn keeps going: the arc is sampled wherever the pointer travelled.
    pub fn arc(&self, sweep: f64) -> Vec<Vector3> {
        let steps = ((sweep.abs() / ARC_STEP).ceil() as usize).max(2);
        (0..=steps)
            .map(|index| self.point(sweep * index as f64 / steps as f64))
            .collect()
    }
}

/// The frame a preview draws with: `center → cursor`, or `None` before both exist.
pub fn preview_frame(center: Option<Vector3>, cursor: Option<[f32; 3]>) -> Option<DragFrame> {
    DragFrame::from_drag(center?, vector(cursor?))
}

/// An event's world point as a `Vector3`.
pub fn vector(point: [f32; 3]) -> Vector3 {
    Vector3::new(
        f64::from(point[0]),
        f64::from(point[1]),
        f64::from(point[2]),
    )
}

/// World points as the `[f32; 3]` list a transient wire is built from.
pub fn to_points(points: &[Vector3]) -> Vec<[f32; 3]> {
    points
        .iter()
        .map(|point| [point.x as f32, point.y as f32, point.z as f32])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point equality at the precision a drag's trigonometry carries.
    fn close(a: Vector3, b: Vector3) -> bool {
        a.distance(&b) < 1e-12
    }

    #[test]
    fn a_flat_drag_lands_on_the_ground_and_a_vertical_one_stands_up() {
        let (u, v) = drag_axes(Vector3::new(10.0, 4.0, 0.0));
        assert_eq!(u.cross(&v), Vector3::new(0.0, 0.0, 1.0), "world XY");
        let (u, v) = drag_axes(Vector3::new(10.0, 0.0, 6.0));
        assert_eq!(u.cross(&v), Vector3::new(0.0, -1.0, 0.0), "world XZ");
        let (u, v) = drag_axes(Vector3::new(0.0, 10.0, 6.0));
        assert_eq!(u.cross(&v), Vector3::new(1.0, 0.0, 0.0), "world YZ");
        // A drag along one axis only picks the plane that keeps it, never a zero side.
        let (u, v) = drag_axes(Vector3::new(0.0, 0.0, 5.0));
        assert_eq!(u.cross(&v), Vector3::new(0.0, -1.0, 0.0));
    }

    #[test]
    fn the_frame_drops_the_off_plane_component_and_puts_u_on_the_drag() {
        let frame = DragFrame::from_drag(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(6.0, 8.0, 5.0),
        )
        .expect("a frame");
        assert_eq!(frame.radius, 10.0, "the drag's in-plane length");
        assert!(close(frame.u, Vector3::new(0.6, 0.8, 0.0)), "{:?}", frame.u);
        assert_eq!(frame.normal(), Vector3::new(0.0, 0.0, 1.0));
        assert!(close(frame.point(0.0), Vector3::new(6.0, 8.0, 0.0)));
        assert!(close(frame.v, Vector3::new(-0.8, 0.6, 0.0)), "{:?}", frame.v);
    }

    #[test]
    fn a_drag_straight_off_the_plane_is_refused() {
        assert!(DragFrame::from_drag(Vector3::zero(), Vector3::zero()).is_none());
    }

    #[test]
    fn a_ring_closes_and_a_polygon_puts_its_first_vertex_on_u() {
        let frame = DragFrame::from_drag(Vector3::zero(), Vector3::new(10.0, 0.0, 0.0))
            .expect("a frame");
        let ring = frame.ring();
        assert_eq!(ring.len(), RING_SEGMENTS + 1);
        assert_eq!(ring.first(), ring.last(), "the ring closes");
        assert!((ring[0].length() - 10.0).abs() < 1e-9, "{:?}", ring[0]);

        let square = frame.polygon(4);
        assert_eq!(square.len(), 5);
        assert!(close(square[0], Vector3::new(10.0, 0.0, 0.0)));
        assert!(close(square[1], Vector3::new(0.0, 10.0, 0.0)), "{:?}", square[1]);
        assert_eq!(square[4], square[0], "the polygon closes");
    }

    #[test]
    fn an_arc_runs_from_u_through_the_sweep() {
        let frame = DragFrame::from_drag(Vector3::zero(), Vector3::new(10.0, 0.0, 0.0))
            .expect("a frame");
        let quarter = frame.arc(PI / 2.0);
        assert!(quarter
            .first()
            .is_some_and(|start| close(*start, Vector3::new(10.0, 0.0, 0.0))));
        assert!(
            quarter
                .last()
                .is_some_and(|end| end.distance(&Vector3::new(0.0, 10.0, 0.0)) < 1e-9),
            "{:?}",
            quarter.last()
        );
        // Past a half turn the arc keeps its direction instead of folding back.
        let three_quarters = frame.arc(1.5 * PI);
        assert!(
            three_quarters
                .last()
                .is_some_and(|end| end.y < -9.99 && end.x.abs() < 1e-6),
            "{:?}",
            three_quarters.last()
        );
    }
}
