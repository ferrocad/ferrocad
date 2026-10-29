//! A planar frame: the presentation-side coordinate system a grid and a sketch live on
//!
//! `n = u × v` is right-handed. The world frame is `origin = 0`, `u = +X`, `v = +Y`,
//! `n = +Z` (the engine is Z-up, consistent with `snap_to_geometry`'s ground plane and
//! OCCT). A datum or a planar face produces another frame with the same operations.

use ferrocad_types::Vector3;

/// How far a face grid may extend beyond its face, as a multiplier on the face's
/// in-plane radius.
pub const FACE_MARGIN: f64 = 1.1;

/// The cosine below which a triangle is not considered coplanar with the face
/// (about 2.6 degrees).
pub const PLANAR_TOLERANCE: f32 = 0.999;

/// Where a frame came from (for the UI and the grid's extent rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkplaneKind {
    /// A world-standard axis plane at the origin: `XY` (the default), `XZ` or `YZ`.
    World,
    /// A named datum plane from the document.
    Datum(String),
    /// A planar face of a displayed object.
    Face { entity: u32, element: String },
    /// A transient custom frame with no document object behind it: the camera's view
    /// plane, an offset of another plane, or a 3-point pick.
    Custom,
}

/// One of the three world-standard axis planes.
///
/// The engine is Z-up, so `Top` is `XY`, `Front` is `XZ` and `Right` is `YZ`. Each is
/// anchored at the world origin; unlike a datum or a face, nothing in the document owns
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardPlane {
    /// World `XY`: `u = +X`, `v = +Y`, `n = +Z` (the default plane).
    Top,
    /// World `XZ`: `u = +X`, `v = +Z`, `n = -Y` (looking along `+Y`).
    Front,
    /// World `YZ`: `u = +Y`, `v = +Z`, `n = +X` (looking along `-X`).
    Right,
}

impl StandardPlane {
    /// The pill label: `XY`, `XZ` or `YZ`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "XY",
            Self::Front => "XZ",
            Self::Right => "YZ",
        }
    }
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
        Self::standard(StandardPlane::Top)
    }

    /// One of the three world-standard axis planes at the origin.
    pub fn standard(plane: StandardPlane) -> Self {
        let x = Vector3::new(1.0, 0.0, 0.0);
        let y = Vector3::new(0.0, 1.0, 0.0);
        let z = Vector3::new(0.0, 0.0, 1.0);
        let (u, v) = match plane {
            StandardPlane::Top => (x, y),
            StandardPlane::Front => (x, z),
            StandardPlane::Right => (y, z),
        };
        Self {
            origin: Vector3::zero(),
            u,
            v,
            // Each pair is a right-handed world basis, so this is exact.
            n: match plane {
                StandardPlane::Top => z,
                StandardPlane::Front => y.scale(-1.0),
                StandardPlane::Right => x,
            },
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

/// A **planar** face resolved into a workplane, plus the extent of its grid.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceFrame {
    /// The frame: origin at the face centroid, `u`/`v` in the face plane.
    pub plane: Workplane,
    /// The in-plane radius of the face, already grown by [`FACE_MARGIN`].
    pub radius: f64,
}

impl FaceFrame {
    /// Resolve the face `part` of a display tessellation into a frame.
    ///
    /// `positions`/`triangles`/`parts` are the stage item's buffers: triangles index
    /// world-space positions and `parts[i]` is position `i`'s sub-element id (empty
    /// means every position is part `0`). Only a **planar** part with at least one
    /// non-degenerate triangle resolves; `None` is a curved face, an edge, or an empty
    /// part.
    ///
    /// The frame is the area-weighted centroid and normal of the part's triangles, with
    /// `u` the projection of world `+X` (or `+Y` when the face is perpendicular to
    /// `X`) and `v = n × u`.
    pub fn resolve(
        entity: u32,
        element: &str,
        positions: &[[f32; 3]],
        triangles: &[u32],
        parts: &[u32],
        part: u32,
    ) -> Option<Self> {
        let part_of = |index: u32| -> u32 { parts.get(index as usize).copied().unwrap_or(0) };
        let point = |index: u32| -> Option<Vector3> {
            let [x, y, z] = *positions.get(index as usize)?;
            Some(Vector3::new(f64::from(x), f64::from(y), f64::from(z)))
        };

        // The part's triangles: all three corners tagged with it.
        let mut area_sum = 0.0f64;
        let mut normal_sum = Vector3::zero();
        let mut centroid_sum = Vector3::zero();
        let mut face_indices: Vec<u32> = Vec::new();
        let mut triangle_normals: Vec<Vector3> = Vec::new();

        for corner in triangles.chunks_exact(3) {
            let [a, b, c] = [corner[0], corner[1], corner[2]];
            if [a, b, c].iter().any(|index| part_of(*index) != part) {
                continue;
            }
            let (Some(pa), Some(pb), Some(pc)) = (point(a), point(b), point(c)) else {
                continue;
            };
            let cross = pb.sub(&pa).cross(&pc.sub(&pa));
            let area = cross.length() * 0.5;
            if area < 1e-12 {
                continue;
            }
            let normal = cross.normalize();
            area_sum += area;
            normal_sum = normal_sum.add(&cross.scale(0.5));
            centroid_sum = centroid_sum.add(&pa.add(&pb).add(&pc).scale(area / 3.0));
            triangle_normals.push(normal);
            face_indices.extend_from_slice(&[a, b, c]);
        }

        if area_sum <= 1e-12 || normal_sum.length() < 1e-12 {
            return None;
        }
        let normal = normal_sum.normalize();
        // A curved part (two facets at an angle) is not a workplane.
        if triangle_normals
            .iter()
            .any(|facet| facet.dot(&normal) < f64::from(PLANAR_TOLERANCE))
        {
            return None;
        }

        let origin = centroid_sum.scale(1.0 / area_sum);
        // `u` is world +X projected into the plane; `+Y` when the face is normal to X.
        let mut u = project_onto_plane(Vector3::new(1.0, 0.0, 0.0), &normal);
        if u.length() < 1e-6 {
            u = project_onto_plane(Vector3::new(0.0, 1.0, 0.0), &normal);
        }
        let v = normal.cross(&u);
        let kind = WorkplaneKind::Face {
            entity,
            element: element.to_string(),
        };
        let plane = Workplane::new(origin, u, v, kind)?;

        // The extent covers the part's own vertices, not just the triangle corners.
        let mut radius = 0.0f64;
        for (index, position) in positions.iter().enumerate() {
            if part_of(index as u32) != part {
                continue;
            }
            let local = plane.project(Vector3::new(
                f64::from(position[0]),
                f64::from(position[1]),
                f64::from(position[2]),
            ));
            radius = radius.max(local[0].hypot(local[1]));
        }
        if radius < 1e-9 {
            return None;
        }
        Some(Self {
            plane,
            radius: radius * FACE_MARGIN,
        })
    }
}

/// `direction` with its component along `normal` removed (not normalised).
fn project_onto_plane(direction: Vector3, normal: &Vector3) -> Vector3 {
    direction.sub(&normal.scale(direction.dot(normal)))
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
    fn the_standard_planes_are_the_three_world_axes() {
        // Z-up, so `Top` is XY, `Front` is XZ (normal -Y) and `Right` is YZ (normal +X).
        for (plane, label, normal) in [
            (StandardPlane::Top, "XY", Vector3::new(0.0, 0.0, 1.0)),
            (StandardPlane::Front, "XZ", Vector3::new(0.0, -1.0, 0.0)),
            (StandardPlane::Right, "YZ", Vector3::new(1.0, 0.0, 0.0)),
        ] {
            let frame = Workplane::standard(plane);
            assert_eq!(plane.label(), label);
            assert_eq!(frame.origin, Vector3::zero(), "standards sit at the origin");
            assert_eq!(frame.n, normal, "{label}");
            assert_eq!(frame.kind, WorkplaneKind::World);
            // Right-handed: `u × v` is the stored normal.
            assert_eq!(frame.u.cross(&frame.v), normal, "{label}");
        }
        assert_eq!(Workplane::standard(StandardPlane::Top), Workplane::world());
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

    /// The top face of a 10 mm box: two triangles, all part `0`.
    fn box_top() -> (Vec<[f32; 3]>, Vec<u32>, Vec<u32>) {
        (
            vec![
                [0.0, 0.0, 10.0],
                [10.0, 0.0, 10.0],
                [10.0, 10.0, 10.0],
                [0.0, 10.0, 10.0],
            ],
            vec![0, 1, 2, 0, 2, 3],
            vec![0, 0, 0, 0],
        )
    }

    fn close(a: Vector3, b: Vector3) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
    }

    #[test]
    fn a_planar_face_resolves_to_a_frame_at_its_centroid() {
        let (positions, triangles, parts) = box_top();
        let frame = FaceFrame::resolve(7, "Face6", &positions, &triangles, &parts, 0)
            .expect("a planar face");

        assert_eq!(
            frame.plane.kind,
            WorkplaneKind::Face {
                entity: 7,
                element: "Face6".into()
            }
        );
        assert!(close(frame.plane.origin, Vector3::new(5.0, 5.0, 10.0)));
        assert_eq!(frame.plane.n, Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(frame.plane.u, Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(frame.plane.v, Vector3::new(0.0, 1.0, 0.0));
        // In-plane corner radius with the 10 % margin.
        assert!((frame.radius - 50.0f64.sqrt() * FACE_MARGIN).abs() < 1e-9);
    }

    #[test]
    fn a_tilted_face_uses_its_own_normal() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 1.0, 0.0]];
        let frame = FaceFrame::resolve(1, "Face1", &positions, &[0, 1, 2], &[], 0).unwrap();
        let s = 0.5f64.sqrt();
        assert!(
            (frame.plane.n.x + s).abs() < 1e-9,
            "n = {:?}",
            frame.plane.n
        );
        assert!(
            (frame.plane.n.z - s).abs() < 1e-9,
            "n = {:?}",
            frame.plane.n
        );
        // Right-handed: n = u × v.
        let cross = frame.plane.u.cross(&frame.plane.v);
        assert!((cross.dot(&frame.plane.n) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_face_normal_to_x_falls_back_to_world_y_for_u() {
        let positions = vec![[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]];
        let frame = FaceFrame::resolve(1, "Face3", &positions, &[0, 1, 2], &[], 0).unwrap();
        assert_eq!(frame.plane.n, Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(frame.plane.u, Vector3::new(0.0, 1.0, 0.0));
        assert_eq!(frame.plane.v, Vector3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn a_curved_or_mixed_part_is_not_a_workplane() {
        // Two facets at a right angle, tagged as one part.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        assert!(FaceFrame::resolve(1, "Face1", &positions, &[0, 1, 2, 0, 3, 2], &[], 0).is_none());
        // An edge (no triangles) has no frame either.
        let (positions, _, parts) = box_top();
        assert!(FaceFrame::resolve(1, "Edge2", &positions, &[], &parts, 0).is_none());
    }

    #[test]
    fn only_the_requested_part_is_resolved() {
        // Part 0 is the z = 10 cap, part 1 the z = 0 cap.
        let positions = vec![
            [0.0, 0.0, 10.0],
            [10.0, 0.0, 10.0],
            [10.0, 10.0, 10.0],
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
        ];
        let triangles = vec![0, 1, 2, 3, 4, 5];
        let parts = vec![0, 0, 0, 1, 1, 1];
        let top = FaceFrame::resolve(9, "Face1", &positions, &triangles, &parts, 0).unwrap();
        let bottom = FaceFrame::resolve(9, "Face2", &positions, &triangles, &parts, 1).unwrap();
        assert!(close(
            top.plane.origin,
            Vector3::new(20.0 / 3.0, 10.0 / 3.0, 10.0)
        ));
        assert!(close(
            bottom.plane.origin,
            Vector3::new(20.0 / 3.0, 10.0 / 3.0, 0.0)
        ));
    }

    #[test]
    fn a_workplane_converts_to_a_manipulator_frame() {
        let plane = Workplane::new(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            WorkplaneKind::Datum("Local".into()),
        )
        .expect("a frame");
        let frame = crate::gizmo::ManipulatorFrame::of_workplane(&plane);
        assert_eq!(frame.origin, plane.origin);
        assert_eq!(frame.basis, [plane.u, plane.v, plane.n]);
    }
}
