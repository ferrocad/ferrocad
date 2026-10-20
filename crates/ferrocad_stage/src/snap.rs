//! Snapping: resolving the pointer onto a geometric feature.
//!
//! The first cut snaps to the picked world point when the pointer is over geometry, and
//! otherwise to a grid node on the `z = 0` plane. Feature snaps (vertex, edge midpoint,
//! centre) need the hit mesh's vertices and land with the richer pick in T4.

use ferrocad_graphics::Camera;

use crate::input::PointerEvent;

/// What the pointer snapped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// A point on the picked geometry.
    Point,
    /// A node of the reference grid.
    Grid,
}

/// A resolved snap: where the pointer effectively is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    /// The snapped world-space point.
    pub point: [f32; 3],
    /// What it snapped to.
    pub kind: SnapKind,
}

/// Resolve a snap for `event`.
///
/// Over geometry, snaps to the picked world point ([`SnapKind::Point`]). Otherwise, when
/// `grid` is given, unprojects the pointer onto the `z = 0` plane and snaps to the
/// nearest grid node ([`SnapKind::Grid`]). `None` when neither applies.
pub fn snap_to_geometry(event: &PointerEvent, camera: &Camera, grid: Option<f64>) -> Option<Snap> {
    if let Some(hit) = &event.hit {
        return Some(Snap {
            point: hit.point,
            kind: SnapKind::Point,
        });
    }

    let step = grid?;
    if step <= 0.0 {
        return None;
    }
    let ray = camera.ray_through(event.position)?;
    // Intersect the ray with the z = 0 plane: origin.z + t * direction.z = 0.
    if ray.direction[2].abs() < 1e-9 {
        return None;
    }
    let t = -ray.origin[2] / ray.direction[2];
    if t < 0.0 {
        return None;
    }
    let x = ray.origin[0] + t * ray.direction[0];
    let y = ray.origin[1] + t * ray.direction[1];
    Some(Snap {
        point: [round_to(x, step) as f32, round_to(y, step) as f32, 0.0],
        kind: SnapKind::Grid,
    })
}

/// Round `value` to the nearest multiple of `step`.
fn round_to(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{PointerKind, PointerEvent};
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
    fn a_hit_snaps_to_the_picked_point() {
        let snap = snap_to_geometry(
            &event(PointerKind::Move, [10, 10], Some(hit_at([1.0, 2.0, 3.0]))),
            &camera(),
            None,
        )
        .unwrap();
        assert_eq!(snap.kind, SnapKind::Point);
        assert_eq!(snap.point, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn without_a_hit_or_grid_there_is_no_snap() {
        assert!(snap_to_geometry(&event(PointerKind::Move, [10, 10], None), &camera(), None).is_none());
    }

    #[test]
    fn an_empty_pointer_snaps_to_the_grid_on_the_z_zero_plane() {
        let snap = snap_to_geometry(&event(PointerKind::Move, [123, 45], None), &camera(), Some(0.5))
            .unwrap();
        assert_eq!(snap.kind, SnapKind::Grid);
        assert_eq!(snap.point[2], 0.0);
        // Both coordinates land on the grid.
        assert!((f64::from(snap.point[0]) / 0.5).fract().abs() < 1e-6, "{:?}", snap.point);
        assert!((f64::from(snap.point[1]) / 0.5).fract().abs() < 1e-6, "{:?}", snap.point);
    }
}
