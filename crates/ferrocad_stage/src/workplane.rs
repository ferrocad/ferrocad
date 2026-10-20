//! A planar frame: the presentation-side coordinate system a grid and a sketch live on
//!
//! `n = u × v` is right-handed. The world frame is `origin = 0`, `u = +X`, `v = +Y`,
//! `n = +Z` (the engine is Z-up, consistent with `snap_to_geometry`'s ground plane and
//! OCCT). A datum or a planar face produces another frame with the same operations.

use ferrocad_types::Vector3;

/// Where a frame came from (for the UI and the grid's extent rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkplaneKind {
    /// The world `XY` plane at the origin.
    World,
    /// A named datum plane from the document.
    Datum(String),
    /// A planar face of a displayed object.
    Face { entity: u32, element: String },
}

/// A right-handed planar frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Workplane {
    pub origin: Vector3,
    pub u: Vector3,
    pub v: Vector3,
    pub n: Vector3,
    pub kind: WorkplaneKind,
}

impl Workplane {
    /// The world `XY` plane: `origin = 0`, `u = +X`, `v = +Y`, `n = +Z`.
    pub fn world() -> Self {
        Self {
            origin: Vector3::zero(),
            u: Vector3::new(1.0, 0.0, 0.0),
            v: Vector3::new(0.0, 1.0, 0.0),
            n: Vector3::new(0.0, 0.0, 1.0),
            kind: WorkplaneKind::World,
        }
    }

    /// A frame from an origin and an in-plane pair. The axes are normalized and the
    /// normal is their cross product; `None` when either axis is degenerate.
    pub fn new(origin: Vector3, u: Vector3, v: Vector3, kind: WorkplaneKind) -> Option<Self> {
        if u.length() < 1e-12 || v.length() < 1e-12 {
            return None;
        }
        let u = u.normalize();
        let v = v.normalize();
        let n = u.cross(&v);
        if n.length() < 1e-12 {
            return None;
        }
        Some(Self {
            origin,
            u,
            v,
            n: n.normalize(),
            kind,
        })
    }

    /// The world point at local `(a, b)`.
    pub fn point(&self, a: f64, b: f64) -> Vector3 {
        self.origin.add(&self.u.scale(a)).add(&self.v.scale(b))
    }

    /// The local `(u, v)` coordinates of a world point: its offset from the origin
    /// projected onto the two axes.
    pub fn project(&self, world: Vector3) -> [f64; 2] {
        let delta = world.sub(&self.origin);
        [delta.dot(&self.u), delta.dot(&self.v)]
    }

    /// Intersect a world ray with the plane, returning the local `(u, v)` hit.
    ///
    /// `None` when the ray is parallel to the plane or the hit lies behind the ray
    /// origin.
    pub fn ray_local(&self, origin: Vector3, direction: Vector3) -> Option<[f64; 2]> {
        let denominator = direction.dot(&self.n);
        if denominator.abs() < 1e-9 {
            return None;
        }
        let t = self.origin.sub(&origin).dot(&self.n) / denominator;
        if t < 0.0 {
            return None;
        }
        Some(self.project(origin.add(&direction.scale(t))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_world_frame_projects_and_round_trips() {
        let world = Workplane::world();
        assert_eq!(world.kind, WorkplaneKind::World);
        assert_eq!(world.n, Vector3::new(0.0, 0.0, 1.0));

        let p = world.point(12.0, -4.0);
        assert_eq!(p, Vector3::new(12.0, -4.0, 0.0));
        assert_eq!(world.project(p), [12.0, -4.0]);
    }

    #[test]
    fn a_tilted_frame_is_right_handed() {
        // The XZ plane with `u = X`, `v = Z` has `n = -Y`.
        let plane = Workplane::new(
            Vector3::new(0.0, 5.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 3.0),
            WorkplaneKind::Datum("Tilted".into()),
        )
        .expect("a frame");
        assert_eq!(plane.u, Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(plane.v, Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(plane.n, Vector3::new(0.0, -1.0, 0.0));

        let p = plane.point(2.0, 3.0);
        assert_eq!(p, Vector3::new(2.0, 5.0, 3.0));
        assert_eq!(plane.project(p), [2.0, 3.0]);
    }

    #[test]
    fn a_degenerate_frame_is_rejected() {
        let origin = Vector3::zero();
        assert!(
            Workplane::new(
                origin,
                Vector3::zero(),
                Vector3::new(0.0, 1.0, 0.0),
                WorkplaneKind::World
            )
            .is_none()
        );
        // Parallel axes → a zero normal.
        assert!(
            Workplane::new(
                origin,
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
                WorkplaneKind::World
            )
            .is_none()
        );
    }

    #[test]
    fn a_ray_hits_the_tilted_plane() {
        let plane = Workplane::new(
            Vector3::new(0.0, 5.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            WorkplaneKind::World,
        )
        .unwrap();
        // From above (negative Y) straight down at (2, ?, 3).
        let local = plane
            .ray_local(Vector3::new(2.0, 10.0, 3.0), Vector3::new(0.0, -1.0, 0.0))
            .expect("a hit");
        assert!((local[0] - 2.0).abs() < 1e-9);
        assert!((local[1] - 3.0).abs() < 1e-9);

        // A ray parallel to the plane misses.
        assert!(
            plane
                .ray_local(Vector3::new(0.0, 10.0, 0.0), Vector3::new(1.0, 0.0, 0.0))
                .is_none()
        );
    }
}
