//! Snapping: resolving the pointer onto a geometric feature or a workplane's grid.
//!
//! A snap is resolved against a [`SnapContext`] (a workplane plus its minor/major
//! steps). The world `XY` plane is the default context, so the old `z = 0` behaviour is
//! `SnapContext::world(step)`. Feature snaps (vertex/edge midpoint/centre) still arrive
//! with the richer pick; [`SnapKind::Point`] is the fallback over geometry.

use ferrocad_graphics::Camera;
use ferrocad_types::Vector3;

use crate::input::PointerEvent;
use crate::workplane::Workplane;

/// What the pointer snapped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// A point on the picked geometry.
    Point,
    /// A node of the active workplane's grid.
    Grid,
}

/// A resolved snap: where the pointer effectively is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    /// The snapped world-space point.
    pub point: [f32; 3],
    /// What it snapped to.
    pub kind: SnapKind,
    /// The local `(u, v)` on the active workplane, when one is known.
    pub local: Option<[f64; 2]>,
    /// Whether a grid snap landed on a **major** intersection rather than a minor node.
    pub major: bool,
}

/// The plane and steps a snap is resolved against.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapContext {
    pub workplane: Workplane,
    pub minor: f64,
    pub major: f64,
}

impl SnapContext {
    /// The world `XY` plane with `minor` and a decade-major step.
    pub fn world(minor: f64) -> Self {
        Self {
            workplane: Workplane::world(),
            minor,
            major: minor * 10.0,
        }
    }
}

/// Resolve a snap for `event`.
///
/// Over geometry, snaps to the picked world point ([`SnapKind::Point`]), reporting its
/// local coordinates when a workplane is known. Otherwise, when a `context` is given,
/// unprojects the pointer onto its plane and snaps to the nearest **major** or **minor**
/// node ([`SnapKind::Grid`]). `None` when neither applies.
pub fn snap_to_geometry(
    event: &PointerEvent,
    camera: &Camera,
    context: Option<&SnapContext>,
) -> Option<Snap> {
    if let Some(hit) = &event.hit {
        let local = context.map(|context| {
            context.workplane.project(Vector3::new(
                f64::from(hit.point[0]),
                f64::from(hit.point[1]),
                f64::from(hit.point[2]),
            ))
        });
        return Some(Snap {
            point: hit.point,
            kind: SnapKind::Point,
            local,
            major: false,
        });
    }

    let context = context?;
    if context.minor <= 0.0 {
        return None;
    }
    let ray = camera.ray_through(event.position)?;
    let origin = Vector3::new(ray.origin[0], ray.origin[1], ray.origin[2]);
    let direction = Vector3::new(ray.direction[0], ray.direction[1], ray.direction[2]);
    let local = context.workplane.ray_local(origin, direction)?;
    let (a, b, major) = snap_local(local, context.minor, context.major);
    let point = context.workplane.point(a, b);
    Some(Snap {
        point: [point.x as f32, point.y as f32, point.z as f32],
        kind: SnapKind::Grid,
        local: Some([a, b]),
        major,
    })
}

/// Round local coordinates to the minor step, and report whether the node is also a
/// **major** intersection.
fn snap_local(local: [f64; 2], minor: f64, major: f64) -> (f64, f64, bool) {
    let a = round_to(local[0], minor);
    let b = round_to(local[1], minor);
    let on_major = major > 0.0
        && (a / major).round().mul_add(major, -a).abs() < 1e-6
        && (b / major).round().mul_add(major, -b).abs() < 1e-6;
    (a, b, on_major)
}

/// Round `value` to the nearest multiple of `step`.
fn round_to(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{PointerEvent, PointerKind};
    use ferrocad_graphics::{Hit, PickId, Topology, Viewport};

    fn event(kind: PointerKind, position: [i32; 2], hit: Option<Hit>) -> PointerEvent {
        PointerEvent {
            kind,
            position,
            button: None,
            hit,
        }
    }

    fn hit_at(point: [f32; 3]) -> Hit {
        Hit {
            id: PickId(1),
            point,
            normal: [0.0, 0.0, 1.0],
            part: 0,
            element: Some(0),
            topology: Topology::Triangles,
            distance: 1.0,
        }
    }

    fn camera() -> Camera {
        Camera {
            viewport: Viewport::new([200, 200], 1.0),
            ..Camera::identity()
        }
    }

    #[test]
    fn a_hit_snaps_to_the_picked_point_and_reports_local_coordinates() {
        let context = SnapContext::world(0.5);
        let snap = snap_to_geometry(
            &event(PointerKind::Move, [10, 10], Some(hit_at([1.0, 2.0, 3.0]))),
            &camera(),
            Some(&context),
        )
        .unwrap();
        assert_eq!(snap.kind, SnapKind::Point);
        assert_eq!(snap.point, [1.0, 2.0, 3.0]);
        assert_eq!(snap.local, Some([1.0, 2.0]));
        assert!(!snap.major);
    }

    #[test]
    fn without_a_hit_or_context_there_is_no_snap() {
        assert!(
            snap_to_geometry(&event(PointerKind::Move, [10, 10], None), &camera(), None).is_none()
        );
    }

    #[test]
    fn an_empty_pointer_snaps_to_the_world_grid_on_the_z_zero_plane() {
        let context = SnapContext::world(0.5);
        let snap = snap_to_geometry(
            &event(PointerKind::Move, [123, 45], None),
            &camera(),
            Some(&context),
        )
        .unwrap();
        assert_eq!(snap.kind, SnapKind::Grid);
        assert_eq!(snap.point[2], 0.0);
        let local = snap.local.expect("local coordinates");
        assert!((local[0] / 0.5).fract().abs() < 1e-6, "{local:?}");
        assert!((local[1] / 0.5).fract().abs() < 1e-6, "{local:?}");
    }

    #[test]
    fn a_major_intersection_is_reported() {
        // The identity camera's centre ray hits the origin, which is major for both
        // steps; nudging the pointer keeps it on that node.
        let mut context = SnapContext::world(0.5);
        context.major = 5.0;
        let snap = snap_to_geometry(
            &event(PointerKind::Move, [100, 100], None),
            &camera(),
            Some(&context),
        )
        .unwrap();
        assert!(snap.major, "{snap:?}");
        assert_eq!(snap.local, Some([0.0, 0.0]));
    }

    #[test]
    fn a_tilted_plane_snaps_in_its_own_coordinates() {
        let plane = Workplane::new(
            Vector3::new(0.0, 5.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            crate::workplane::WorkplaneKind::Datum("XZ".into()),
        )
        .unwrap();
        let context = SnapContext {
            workplane: plane,
            minor: 1.0,
            major: 10.0,
        };
        // A camera straight above the origin looking down -Y would need a custom view;
        // instead resolve the plane intersection directly, then snap the local point.
        let local = context
            .workplane
            .ray_local(Vector3::new(2.2, 10.0, 3.4), Vector3::new(0.0, -1.0, 0.0))
            .expect("a hit");
        let (a, b, _) = snap_local(local, 1.0, 10.0);
        assert_eq!((a, b), (2.0, 3.0));
        let world = context.workplane.point(a, b);
        assert!((world.y - 5.0).abs() < 1e-9, "{world:?}");
    }
}
