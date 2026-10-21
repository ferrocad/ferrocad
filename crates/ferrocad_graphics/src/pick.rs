//! Ray picking: which draw packet is under a device-space pointer position.
//!
//! The counterpart of Coin's `SoRayPickAction`: it unprojects a device position
//! through the camera into a world-space ray and intersects it with the compiled
//! triangles. Because it works on a [`RenderQueue`] (world-space packets), it needs
//! neither a GPU nor a live scene graph — the same reason the g-buffer pass in
//! `ferrocad_wgpu` is optional.

use ferrocad_types::{Matrix4, Vector3};

use crate::batch::{PickId, RenderQueue};
use crate::camera::{Camera, Projection};
use crate::geometry::Topology;
use crate::pixels::Rect;

/// A world-space ray. `direction` is normalised.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// Where the ray starts (the near clip point).
    pub origin: [f64; 3],
    /// The (normalised) direction.
    pub direction: [f64; 3],
}

impl Ray {
    /// The distance from [`origin`](Self::origin) to triangle `abc`, if the ray
    /// crosses it (either face).
    ///
    /// Möller–Trumbore; only intersections in front of the origin (`t > 0`) count.
    pub fn triangle_distance(&self, a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<f64> {
        let edge1 = sub(b, a);
        let edge2 = sub(c, a);
        let pvec = cross(self.direction, edge2);
        let det = dot(edge1, pvec);
        if det.abs() < 1e-12 {
            return None; // ray parallel to the triangle
        }
        let inv_det = 1.0 / det;
        let tvec = sub(self.origin, a);
        let u = dot(tvec, pvec) * inv_det;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let qvec = cross(tvec, edge1);
        let v = dot(self.direction, qvec) * inv_det;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = dot(edge2, qvec) * inv_det;
        (t > 0.0).then_some(t)
    }

    /// The distance to the line segment `ab`, if the ray passes within `radius` of it
    /// (a "thick ray", for picking polylines like the navigation cube's edges).
    pub fn segment_distance(&self, a: [f64; 3], b: [f64; 3], radius: f64) -> Option<f64> {
        let d1 = self.direction;
        let d2 = sub(b, a);
        let r = sub(self.origin, a);
        let a11 = dot(d1, d1);
        let a12 = dot(d1, d2);
        let a22 = dot(d2, d2);
        let b1 = dot(d1, r);
        let b2 = dot(d2, r);
        let denom = a11 * a22 - a12 * a12;
        // Closest approach: t on the ray, s on the segment. Solve the two
        // perpendicularity conditions, then clamp to the segment and in front of the
        // eye, refining against the constraint we clamped.
        let (s, t) = if denom.abs() < 1e-12 {
            (0.0, (b1 / a11).max(0.0))
        } else {
            let t = (a12 * b2 - b1 * a22) / denom;
            let s = (b2 + t * a12) / a22;
            if (0.0..=1.0).contains(&s) {
                (s, t.max(0.0))
            } else {
                let s = s.clamp(0.0, 1.0);
                (s, ((s * a12 - b1) / a11).max(0.0))
            }
        };
        let point = add(self.origin, scale(d1, t));
        let closest = add(a, scale(d2, s));
        let gap = sub(point, closest);
        (dot(gap, gap).sqrt() <= radius).then_some(t)
    }
}

impl Camera {
    /// The world-space ray through a device position (bottom-left origin).
    ///
    /// `None` if the view-projection is singular.
    pub fn ray_through(&self, device: [i32; 2]) -> Option<Ray> {
        self.ray_through_rect(self.viewport.rect(), device)
    }

    /// The world-space ray through a device position **within `rect`** (bottom-left
    /// origin): the NDC is computed relative to the rectangle, so an overlay with its
    /// own rect picks through its own view.
    pub fn ray_through_rect(&self, rect: Rect, device: [i32; 2]) -> Option<Ray> {
        let width = f64::from(rect.width.max(1));
        let height = f64::from(rect.height.max(1));
        let ndc_x = 2.0 * f64::from(device[0] - rect.x) / width - 1.0;
        let ndc_y = 2.0 * f64::from(device[1] - rect.y) / height - 1.0;

        let inverse = invert(&self.view_projection())?;
        let near = unproject(&inverse, ndc_x, ndc_y, 0.0);
        let far = unproject(&inverse, ndc_x, ndc_y, 1.0);
        let direction = normalize(sub(far, near));
        Some(Ray {
            origin: near,
            direction,
        })
    }
}

/// A ray hit: which object, where on it, and which sub-element.
///
/// The richer form of [`pick`], carrying what `SoPickedPoint` and the detail classes
/// expose to Python (world point, surface normal, and the primitive/sub-element ids).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The object that was hit (the packet's [`PickId`]).
    pub id: PickId,
    /// The world-space intersection point.
    pub point: [f32; 3],
    /// The world-space normal (triangle normal; `+Z` for a line hit).
    pub normal: [f32; 3],
    /// The topological sub-element id carried by the hit primitive's vertices
    /// ([`Vertex::part_idx`](crate::geometry::Vertex::part_idx)). `0` when the
    /// producer does not tag sub-elements.
    pub part: u32,
    /// The primitive ordinal within its mesh: a triangle index for
    /// [`Topology::Triangles`], a segment index for [`Topology::Lines`].
    ///
    /// The G-buffer path resolves a pixel rather than a primitive, so it reports
    /// `None` there; it always reports [`part`](Self::part).
    pub element: Option<u32>,
    /// The topology of the mesh that was hit.
    pub topology: Topology,
    /// The distance along the ray from its origin.
    pub distance: f64,
}

/// A fixed world-space line radius for [`pick_hit`], which has no camera from which to
/// derive a screen-space tolerance. [`pick_at_hit`] uses [`LINE_PICK_PIXELS`] instead.
const DEFAULT_LINE_PICK_RADIUS: f64 = 0.04;

/// The screen-space line tolerance: a 1 px stroke plus one pixel of padding.
pub const LINE_PICK_PIXELS: f64 = 2.0;

/// The nearest packet under `ray`, with its hit distance.
pub fn pick(queue: &RenderQueue, ray: &Ray) -> Option<(PickId, f64)> {
    pick_hit(queue, ray).map(|hit| (hit.id, hit.distance))
}

/// The nearest triangle or line segment under `ray`, with its point and indices.
pub fn pick_hit(queue: &RenderQueue, ray: &Ray) -> Option<Hit> {
    pick_hit_impl(queue, ray, None)
}

/// [`pick_hit`], with a camera to derive a screen-space tolerance for line segments.
fn pick_hit_impl(queue: &RenderQueue, ray: &Ray, camera: Option<&Camera>) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for packet in &queue.packets {
        // A shader program draws a placeholder quad, not its authored geometry, and an
        // `SoPickStyle.UNPICKABLE` packet opts out outright: neither can be a hit.
        if packet.state.program.is_some() || !packet.state.pickable {
            continue;
        }
        if packet.topology == Topology::Points {
            continue;
        }
        let Some(geometry) = &packet.geometry else {
            continue; // a cache-hit packet has no geometry to test against
        };
        let world: Vec<[f64; 3]> = geometry
            .vertices
            .iter()
            .map(|vertex| transform_point(&packet.state.world_transform, vertex.position))
            .collect();
        let chunk = match packet.topology {
            Topology::Triangles => 3,
            Topology::Lines => 2,
            Topology::Points => continue,
        };
        for (element, indices) in geometry.indices.chunks_exact(chunk).enumerate() {
            let a = world[indices[0] as usize];
            let b = world[indices[1] as usize];
            let (distance, normal) = if chunk == 3 {
                let c = world[indices[2] as usize];
                let Some(distance) = ray.triangle_distance(a, b, c) else {
                    continue;
                };
                (distance, to_f32(normalize(cross(sub(b, a), sub(c, a)))))
            } else {
                // A polyline is picked through a "thick ray". With a camera the
                // thickness is derived from the segment's distance (screen-space);
                // without one it falls back to a fixed world radius.
                let tolerance = camera.map_or(DEFAULT_LINE_PICK_RADIUS, |camera| {
                    world_per_pixel(camera, scale(add(a, b), 0.5)) * LINE_PICK_PIXELS
                });
                let Some(distance) = ray.segment_distance(a, b, tolerance) else {
                    continue;
                };
                (distance, [0.0, 0.0, 1.0])
            };
            if best.is_none_or(|hit| distance < hit.distance) {
                let point = add(ray.origin, scale(ray.direction, distance));
                let part = geometry
                    .vertices
                    .get(indices[0] as usize)
                    .map_or(0, |vertex| vertex.part_idx);
                best = Some(Hit {
                    id: packet.state.id,
                    point: to_f32(point),
                    normal,
                    part,
                    element: Some(element as u32),
                    topology: packet.topology,
                    distance,
                });
            }
        }
    }
    best
}

/// The world size of one device pixel at `point`.
fn world_per_pixel(camera: &Camera, point: [f64; 3]) -> f64 {
    let height = f64::from(camera.viewport.device_size()[1].max(1));
    if let Projection::Orthographic { extent, .. } = camera.projection {
        return 2.0 * extent / height;
    }
    let Some(inverse) = camera.view.inverse() else {
        return DEFAULT_LINE_PICK_RADIUS;
    };
    let eye = inverse.transform(&Vector3::zero());
    let distance = Vector3::new(point[0], point[1], point[2])
        .sub(&eye)
        .length()
        .max(1e-6);
    let m5 = camera.projection_matrix().m[5].abs().max(1e-6);
    distance * 2.0 / (height * m5)
}

/// The [`PickId`] under a device position, if any.
pub fn pick_at(queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<PickId> {
    pick_at_hit(queue, camera, device).map(|hit| hit.id)
}

/// The [`Hit`] under a device position, if any.
pub fn pick_at_hit(queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
    let ray = camera.ray_through(device)?;
    pick_hit_impl(queue, &ray, Some(camera))
}

/// The [`PickId`] under a device position **within `rect`** (an overlay), if any.
pub fn pick_at_rect(queue: &RenderQueue, camera: &Camera, rect: Rect, device: [i32; 2]) -> Option<PickId> {
    pick_at_rect_hit(queue, camera, rect, device).map(|hit| hit.id)
}

/// The [`Hit`] under a device position **within `rect`** (an overlay), if any.
pub fn pick_at_rect_hit(
    queue: &RenderQueue,
    camera: &Camera,
    rect: Rect,
    device: [i32; 2],
) -> Option<Hit> {
    let ray = camera.ray_through_rect(rect, device)?;
    pick_hit_impl(queue, &ray, Some(camera))
}

// --- small vector/matrix helpers (f64, row-major) --------------------------------

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(v: [f64; 3], s: f64) -> [f64; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn to_f32(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    if length > 0.0 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        v
    }
}

/// Transform a point by a row-major matrix, dividing by `w`.
fn transform_point(m: &Matrix4, p: [f32; 3]) -> [f64; 3] {
    let v = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2]), 1.0];
    let out = mul_vec(m, v);
    let w = if out[3] != 0.0 { out[3] } else { 1.0 };
    [out[0] / w, out[1] / w, out[2] / w]
}

/// Unproject an NDC point with the inverse view-projection.
fn unproject(inverse: &Matrix4, x: f64, y: f64, z: f64) -> [f64; 3] {
    let out = mul_vec(inverse, [x, y, z, 1.0]);
    let w = if out[3] != 0.0 { out[3] } else { 1.0 };
    [out[0] / w, out[1] / w, out[2] / w]
}

/// Row-major matrix-vector product.
fn mul_vec(m: &Matrix4, v: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0; 4];
    for row in 0..4 {
        out[row] = (0..4).map(|col| m.m[row * 4 + col] * v[col]).sum();
    }
    out
}

/// Inverse of a row-major 4×4 matrix (the standard cofactor expansion).
fn invert(m: &Matrix4) -> Option<Matrix4> {
    let a = &m.m;
    let mut inv = [0.0f64; 16];

    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14]
        + a[13] * a[6] * a[11]
        - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14]
        - a[12] * a[6] * a[11]
        + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13]
        + a[12] * a[5] * a[11]
        - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13]
        - a[12] * a[5] * a[10]
        + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14]
        - a[13] * a[2] * a[11]
        + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14]
        + a[12] * a[2] * a[11]
        - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13]
        - a[12] * a[1] * a[11]
        + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13]
        + a[12] * a[1] * a[10]
        - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14]
        + a[13] * a[2] * a[7]
        - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14]
        - a[12] * a[2] * a[7]
        + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13]
        + a[12] * a[1] * a[7]
        - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13]
        - a[12] * a[1] * a[6]
        + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10]
        - a[9] * a[2] * a[7]
        + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10]
        + a[8] * a[2] * a[7]
        - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9]
        - a[8] * a[1] * a[7]
        + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9]
        + a[8] * a[1] * a[6]
        - a[8] * a[2] * a[5];

    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    for value in &mut inv {
        *value *= inv_det;
    }
    Some(Matrix4::from_values(inv))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{DrawPacket, Generation, MeshId, PickId, RenderQueue};
    use crate::geometry::{Geometry, Vertex};
    use crate::state::{ShaderProgram, StateSnapshot};
    use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};
    use std::sync::Arc;

    fn packet_with_part(
        topology: Topology,
        positions: &[[f32; 3]],
        indices: Vec<u32>,
        part: u32,
    ) -> DrawPacket {
        let vertices = positions
            .iter()
            .map(|position| Vertex {
                position: *position,
                normal: [0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                part_idx: part,
                color: [1.0, 1.0, 1.0, 1.0],
            })
            .collect();
        DrawPacket {
            mesh_id: MeshId(0),
            generation: Generation(0),
            topology,
            geometry: Some(Geometry { vertices, indices }),
            state: StateSnapshot {
                world_transform: Matrix4::identity(),
                color: [1.0, 1.0, 1.0, 1.0],
                depth_bias: 0.0,
                texture: None,
                id: PickId(0),
                depth: Default::default(),
                pickable: true,
                program: None,
            },
        }
    }

    fn packet(topology: Topology, positions: &[[f32; 3]], indices: Vec<u32>) -> DrawPacket {
        packet_with_part(topology, positions, indices, 0)
    }

    /// A triangle at z = 0.5 spanning [-0.5, 0.5] in x and y.
    fn triangle_queue() -> RenderQueue {
        RenderQueue {
            packets: vec![packet(
                Topology::Triangles,
                &[[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.0, 0.5, 0.5]],
                vec![0, 1, 2],
            )],
        }
    }

    /// A single horizontal segment at z = 0.
    fn line_queue() -> RenderQueue {
        RenderQueue {
            packets: vec![packet(Topology::Lines, &[[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]], vec![0, 1])],
        }
    }

    #[test]
    fn identity_camera_ray_through_center_hits_the_triangle() {
        let queue = triangle_queue();
        // Identity view/projection: view_projection is identity, so device (50,50)
        // of a 100x100 viewport is NDC (0, 0) and the ray runs along +z.
        let camera = Camera {
            viewport: crate::Viewport::new([100, 100], 1.0),
            ..Camera::identity()
        };
        assert_eq!(pick_at(&queue, &camera, [50, 50]), Some(PickId(0)));
        assert_eq!(pick_at(&queue, &camera, [5, 5]), None);
    }

    #[test]
    fn triangle_distance_respects_winding_and_range() {
        let ray = Ray {
            origin: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        };
        let a = [-0.5, -0.5, 0.5];
        let b = [0.5, -0.5, 0.5];
        let c = [0.0, 0.5, 0.5];
        assert!((ray.triangle_distance(a, b, c).unwrap() - 0.5).abs() < 1e-9);
        // Behind the origin.
        let behind = Ray {
            origin: [0.0, 0.0, 1.0],
            direction: [0.0, 0.0, 1.0],
        };
        assert_eq!(behind.triangle_distance(a, b, c), None);
    }

    #[test]
    fn segment_distance_picks_within_the_radius() {
        let ray = Ray {
            origin: [0.0, 0.0, -5.0],
            direction: [0.0, 0.0, 1.0],
        };
        // A segment crossing the view axis at z = 0.
        assert!((ray.segment_distance([-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.1).unwrap() - 5.0).abs() < 1e-9);
        // Offset beyond the radius.
        assert_eq!(ray.segment_distance([-1.0, 0.5, 0.0], [1.0, 0.5, 0.0], 0.1), None);
        // Behind the ray origin.
        assert_eq!(ray.segment_distance([-1.0, 0.0, -10.0], [1.0, 0.0, -10.0], 0.1), None);
    }

    #[test]
    fn pick_hit_reports_line_topology() {
        let queue = line_queue();
        let ray = Ray {
            origin: [0.0, 0.0, -5.0],
            direction: [0.0, 0.0, 1.0],
        };
        let hit = pick_hit(&queue, &ray).expect("line hit");
        assert_eq!(hit.topology, Topology::Lines);
        assert_eq!(hit.element, Some(0));
        assert_eq!(hit.part, 0);
    }

    #[test]
    fn a_cpu_pick_carries_the_vertex_part_id() {
        let queue = RenderQueue {
            packets: vec![packet_with_part(
                Topology::Triangles,
                &[[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.0, 0.5, 0.5]],
                vec![0, 1, 2],
                7,
            )],
        };
        let ray = Ray {
            origin: [0.0, 0.0, -5.0],
            direction: [0.0, 0.0, 1.0],
        };
        let hit = pick_hit(&queue, &ray).expect("triangle hit");
        assert_eq!(hit.part, 7, "the triangle's part id");
        assert_eq!(hit.element, Some(0), "the triangle ordinal");
    }

    #[test]
    fn a_line_pick_is_screen_space_when_the_camera_is_known() {
        // The eye ends up at world z = +100 looking down -Z.
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -100.0), Rotation::identity()).to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 1000.0,
            },
            viewport: crate::Viewport::new([100, 100], 1.0),
        };
        // One world unit off the view axis at ~100 units: a little over one pixel at
        // this depth. The screen-space tolerance picks it; the fixed world radius
        // used by the camera-less API does not.
        let queue = RenderQueue {
            packets: vec![packet(
                Topology::Lines,
                &[[-1.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
                vec![0, 1],
            )],
        };
        let device = [50, 50];
        let hit = pick_at_hit(&queue, &camera, device).expect("screen-space line hit");
        assert_eq!(hit.topology, Topology::Lines);

        let ray = camera.ray_through(device).expect("a ray");
        assert!(
            pick_hit(&queue, &ray).is_none(),
            "the fixed world radius (0.04) misses a one-unit offset"
        );
    }

    #[test]
    fn a_program_packet_is_never_a_hit() {
        let ray = Ray {
            origin: [0.0, 0.0, -5.0],
            direction: [0.0, 0.0, 1.0],
        };
        let mut queue = triangle_queue();
        // The triangle would be hit, so only the program skip can make this miss.
        assert!(pick_hit(&queue, &ray).is_some());
        queue.packets[0].state.program = Some(Arc::new(ShaderProgram::from_source(
            "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }",
        )));
        assert!(
            pick_hit(&queue, &ray).is_none(),
            "a program draws a placeholder, never the authored geometry"
        );
        assert!(pick_at(&queue, &Camera::identity(), [50, 50]).is_none());
    }

    #[test]
    fn an_unpickable_packet_is_skipped_but_a_pickable_one_is_not() {
        let ray = Ray {
            origin: [0.0, 0.0, -5.0],
            direction: [0.0, 0.0, 1.0],
        };
        let mut queue = triangle_queue();
        queue.packets[0].state.pickable = false;
        assert!(pick_hit(&queue, &ray).is_none(), "SoPickStyle.UNPICKABLE");
        queue.packets[0].state.pickable = true;
        assert!(pick_hit(&queue, &ray).is_some());
    }
}
