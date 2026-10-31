//! Snapping: resolving the pointer onto a geometric feature or a workplane's grid.
//!
//! A snap is resolved against a [`SnapContext`] (a workplane plus its minor/major
//! steps). The world `XY` plane is the default context, so the old `z = 0` behaviour is
//! `SnapContext::world(step)`. Feature snaps (a topological vertex, an edge midpoint, a
//! face centre or the workplane origin) are supplied by the caller as
//! [`SnapCandidate`]s — the viewport reads them from the stage's display tessellation —
//! and win over the picked point and the grid.

use ferrocad_graphics::Camera;
use ferrocad_types::Vector3;

use crate::input::PointerEvent;
use crate::screen::project_device;
use crate::workplane::Workplane;

/// The screen-space tolerance for a feature snap, in device pixels.
pub const FEATURE_PIXELS: f64 = 10.0;

/// What the pointer snapped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// A point on the picked geometry.
    Point,
    /// A node of the active workplane's grid.
    Grid,
    /// A topological vertex of the hovered object.
    Vertex,
    /// The midpoint of a topological edge of the hovered object.
    EdgeMid,
    /// The centroid of the hovered planar face.
    Centre,
    /// The active workplane's origin.
    Origin,
}

impl SnapKind {
    /// Whether this is a feature snap (a named topological point) rather than a point on
    /// the surface or the grid.
    pub fn is_feature(self) -> bool {
        matches!(
            self,
            SnapKind::Vertex | SnapKind::EdgeMid | SnapKind::Centre | SnapKind::Origin
        )
    }
}

/// A world-space point the pointer may snap to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapCandidate {
    pub point: Vector3,
    pub kind: SnapKind,
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

    /// Resolve a snap for a **world-space** point (the headless counterpart of
    /// [`snap_to_geometry_with`], which needs a camera).
    ///
    /// A candidate within `tolerance` (Euclidean world distance) wins; ties go to the
    /// earlier candidate, matching [`nearest_feature`]. Otherwise the point is projected
    /// onto the workplane and its local `(u, v)` rounded to [`minor`](Self::minor),
    /// reporting whether the node is also a `major` intersection. With `tolerance <= 0`
    /// feature snapshots are disabled. With `minor <= 0` the projected point is returned
    /// as [`SnapKind::Point`].
    pub fn snap_point(&self, point: Vector3, candidates: &[SnapCandidate], tolerance: f64) -> Snap {
        if tolerance > 0.0 {
            // Candidates are tested in order and only a **strictly** closer point replaces
            // the current best, so the caller's order breaks ties (as in
            // `nearest_feature`). A candidate exactly at the tolerance is still inside it.
            let mut best: Option<(f64, SnapCandidate)> = None;
            for candidate in candidates {
                let distance = candidate.point.distance(&point);
                if distance > tolerance {
                    continue;
                }
                if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
                    best = Some((distance, *candidate));
                }
            }
            if let Some((_, feature)) = best {
                return Snap {
                    point: [
                        feature.point.x as f32,
                        feature.point.y as f32,
                        feature.point.z as f32,
                    ],
                    kind: feature.kind,
                    local: Some(self.workplane.project(feature.point)),
                    major: false,
                };
            }
        }

        let local = self.workplane.project(point);
        if self.minor <= 0.0 {
            let projected = self.workplane.point(local[0], local[1]);
            return Snap {
                point: [
                    projected.x as f32,
                    projected.y as f32,
                    projected.z as f32,
                ],
                kind: SnapKind::Point,
                local: Some(local),
                major: false,
            };
        }

        let (a, b, major) = snap_local(local, self.minor, self.major);
        let projected = self.workplane.point(a, b);
        Snap {
            point: [
                projected.x as f32,
                projected.y as f32,
                projected.z as f32,
            ],
            kind: SnapKind::Grid,
            local: Some([a, b]),
            major,
        }
    }
}

/// Resolve a snap for `event`, preferring a feature over the picked point and the grid.
///
/// Feature snaps are resolved first (see [`nearest_feature`]); then the picked point;
/// then, when a `context` is given, the nearest **major** or **minor** grid node on its
/// plane. `None` when none applies.
pub fn snap_to_geometry_with(
    event: &PointerEvent,
    camera: &Camera,
    context: Option<&SnapContext>,
    candidates: &[SnapCandidate],
) -> Option<Snap> {
    if let Some(feature) = nearest_feature(camera, event.position, candidates, FEATURE_PIXELS) {
        return Some(Snap {
            point: [
                feature.point.x as f32,
                feature.point.y as f32,
                feature.point.z as f32,
            ],
            kind: feature.kind,
            local: context.map(|context| context.workplane.project(feature.point)),
            major: false,
        });
    }
    snap_to_geometry(event, camera, context)
}

/// The nearest candidate within `max_pixels` of `device`, projected through `camera`.
///
/// Candidates are tested in order and only a **strictly** closer point replaces the
/// current best, so the caller's order breaks ties (most specific first).
pub fn nearest_feature(
    camera: &Camera,
    device: [i32; 2],
    candidates: &[SnapCandidate],
    max_pixels: f64,
) -> Option<SnapCandidate> {
    let target = [f64::from(device[0]), f64::from(device[1])];
    let mut best: Option<(f64, SnapCandidate)> = None;
    for candidate in candidates {
        let Some(projected) = project_device(camera, candidate.point) else {
            continue;
        };
        let distance = (projected[0] - target[0]).hypot(projected[1] - target[1]);
        if distance > max_pixels {
            continue;
        }
        if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
            best = Some((distance, *candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
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
            projection: None,
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

    /// A rotated frame at `(10, 0, 5)` with `u = (1, 1, 0)/√2`, `v = (-1, 1, 0)/√2`
    /// and `n = +Z`.
    fn rotated() -> Workplane {
        let s = 0.5f64.sqrt();
        Workplane::new(
            Vector3::new(10.0, 0.0, 5.0),
            Vector3::new(s, s, 0.0),
            Vector3::new(-s, s, 0.0),
            crate::workplane::WorkplaneKind::Datum("Rotated".into()),
        )
        .expect("a frame")
    }

    fn assert_point(actual: [f32; 3], expected: Vector3) {
        assert!((f64::from(actual[0]) - expected.x).abs() < 1e-4, "{actual:?}");
        assert!((f64::from(actual[1]) - expected.y).abs() < 1e-4, "{actual:?}");
        assert!((f64::from(actual[2]) - expected.z).abs() < 1e-4, "{actual:?}");
    }

    #[test]
    fn a_world_point_snaps_to_the_world_grid() {
        let context = SnapContext::world(0.5);
        let snap = context.snap_point(Vector3::new(1.2, -0.6, 3.0), &[], 0.0);
        assert_eq!(snap.kind, SnapKind::Grid);
        assert_eq!(snap.local, Some([1.0, -0.5]));
        assert_point(snap.point, Vector3::new(1.0, -0.5, 0.0));
        assert!(!snap.major);
    }

    #[test]
    fn a_rotated_workplane_snaps_in_its_own_coordinates() {
        let context = SnapContext {
            workplane: rotated(),
            minor: 1.0,
            major: 10.0,
        };
        let on_plane = context.workplane.point(2.0, 3.0);
        // A point off the plane still projects onto it: the normal component is dropped.
        let off_plane = on_plane.add(&context.workplane.n.scale(4.0));
        let snap = context.snap_point(off_plane, &[], 0.0);
        assert_eq!(snap.kind, SnapKind::Grid);
        let local = snap.local.expect("local coordinates");
        assert!((local[0] - 2.0).abs() < 1e-9, "{local:?}");
        assert!((local[1] - 3.0).abs() < 1e-9, "{local:?}");
        assert_point(snap.point, on_plane);
        assert!(!snap.major);
    }

    #[test]
    fn a_feature_within_tolerance_wins_over_the_grid() {
        let context = SnapContext::world(1.0);
        let candidate = SnapCandidate {
            point: Vector3::new(2.4, 2.4, 0.0),
            kind: SnapKind::Vertex,
        };
        let snap = context.snap_point(Vector3::new(2.1, 2.1, 0.0), &[candidate], 0.5);
        assert_eq!(snap.kind, SnapKind::Vertex);
        assert_point(snap.point, candidate.point);
        assert_eq!(snap.local, Some([2.4, 2.4]));
        assert!(!snap.major);

        // Outside the tolerance the grid node (2, 2) wins instead.
        let snap = context.snap_point(Vector3::new(2.1, 2.1, 0.0), &[candidate], 0.1);
        assert_eq!(snap.kind, SnapKind::Grid);
        assert_eq!(snap.local, Some([2.0, 2.0]));
    }

    #[test]
    fn the_tolerance_is_measured_in_world_distance() {
        let context = SnapContext::world(1.0);
        // 3-4-5: the candidate is 5 away from the point.
        let candidate = SnapCandidate {
            point: Vector3::new(3.0, 4.0, 0.0),
            kind: SnapKind::Vertex,
        };
        let point = Vector3::new(0.0, 0.0, 0.0);
        assert_eq!(
            context.snap_point(point, &[candidate], 4.9).kind,
            SnapKind::Grid
        );
        assert_eq!(
            context.snap_point(point, &[candidate], 5.0).kind,
            SnapKind::Vertex,
            "a candidate exactly at the tolerance is inside it"
        );
        assert_eq!(
            context.snap_point(point, &[candidate], 5.1).kind,
            SnapKind::Vertex
        );
        // `tolerance <= 0` disables feature snapshots entirely.
        assert_eq!(
            context.snap_point(point, &[candidate], 0.0).kind,
            SnapKind::Grid
        );
    }

    #[test]
    fn a_tie_goes_to_the_earlier_candidate() {
        let context = SnapContext::world(1.0);
        let point = Vector3::new(0.0, 0.0, 0.0);
        let candidates = [
            SnapCandidate {
                point: Vector3::new(1.0, 0.0, 0.0),
                kind: SnapKind::Vertex,
            },
            SnapCandidate {
                point: Vector3::new(-1.0, 0.0, 0.0),
                kind: SnapKind::EdgeMid,
            },
        ];
        let snap = context.snap_point(point, &candidates, 5.0);
        assert_eq!(snap.kind, SnapKind::Vertex);
        assert_point(snap.point, Vector3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn the_origin_edge_mid_and_centre_kinds_are_reported() {
        let context = SnapContext::world(1.0);
        for (kind, point) in [
            (SnapKind::Origin, Vector3::new(0.0, 0.0, 0.0)),
            (SnapKind::EdgeMid, Vector3::new(5.0, 1.0, 0.0)),
            (SnapKind::Centre, Vector3::new(-2.0, 3.0, 0.0)),
        ] {
            let candidate = SnapCandidate { point, kind };
            let snap = context.snap_point(point, &[candidate], 0.25);
            assert_eq!(snap.kind, kind);
            assert_point(snap.point, point);
            assert_eq!(snap.local, Some([point.x, point.y]));
            assert!(!snap.major);
        }
    }

    #[test]
    fn a_non_positive_minor_returns_the_projected_point() {
        let context = SnapContext {
            workplane: rotated(),
            minor: 0.0,
            major: 0.0,
        };
        let on_plane = context.workplane.point(2.4, 3.6);
        let off_plane = on_plane.add(&context.workplane.n.scale(2.0));
        let snap = context.snap_point(off_plane, &[], 0.0);
        assert_eq!(snap.kind, SnapKind::Point);
        assert_point(snap.point, on_plane);
        let local = snap.local.expect("local coordinates");
        assert!((local[0] - 2.4).abs() < 1e-9, "{local:?}");
        assert!((local[1] - 3.6).abs() < 1e-9, "{local:?}");
        assert!(!snap.major);

        // A negative step behaves the same and also disables the grid.
        let context = SnapContext {
            workplane: Workplane::world(),
            minor: -1.0,
            major: 10.0,
        };
        let snap = context.snap_point(Vector3::new(1.3, 2.7, 0.0), &[], 0.0);
        assert_eq!(snap.kind, SnapKind::Point);
        assert_eq!(snap.local, Some([1.3, 2.7]));
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
