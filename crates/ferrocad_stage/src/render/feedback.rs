//! Snap feedback: the aurora, the projection rays, and the workplane origin node.
//!
//! These are **stage producers**, like the grid: pure arithmetic returning vertices
//! for the viewport's queues. Nothing here knows about gpui, a kernel, or Python.
//!
//! - The **aurora** is a screen-facing diamond at the snapped point. It flashes when a
//!   new snap point is acquired and then settles to a steady marker.
//! - The **rays** are dashed segments from a grid snap to the workplane's `U` and `V`
//!   axes, showing the alignment lineage. WebGPU lines are 1 px solid, so a dash is a
//!   short segment (the ring polylines use the same trick).
//! - The **origin node** is a small hollow diamond at the workplane origin that blooms
//!   when the pointer comes close.

use std::time::Duration;

use ferrocad_graphics::{Camera, Projection, Vertex};
use ferrocad_types::Vector3;

use crate::grid::{LIGHT, hash};
use crate::screen::{camera_right, camera_up, scale_for};
use crate::workplane::Workplane;

/// The aurora's radius, in device pixels.
pub const AURORA_PIXELS: f64 = 7.0;
/// The aurora's ink: linear `#38BDF8` at 85 %.
pub const AURORA_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 0.85];
/// The aurora's settled alpha after the flash.
pub const AURORA_REST_ALPHA: f32 = 0.55;
/// How long the acquisition flash lasts.
pub const FLASH: Duration = Duration::from_millis(120);

/// A projection ray's dash length, in device pixels.
pub const RAY_DASH_PIXELS: f64 = 4.0;
/// A projection ray's colour (the active accent, a little quieter than the aurora).
pub const RAY_COLOR: [f32; 4] = [0.040, 0.509, 0.939, 0.55];
/// A guard against a pathologically long ray.
const MAX_DASHES: usize = 512;

/// The origin node's radius, in device pixels.
pub const ORIGIN_PIXELS: f64 = 3.0;
/// The origin node's resting alpha; it blooms to `1.0` near the pointer.
pub const ORIGIN_ALPHA: f32 = 0.65;
/// The pointer proximity (device pixels) that blooms the origin node.
pub const ORIGIN_PROXIMITY_PIXELS: f64 = 5.0;

/// The aurora's alpha: brightest when the snap is acquired, settling after [`FLASH`].
pub fn aurora_alpha(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f64() / FLASH.as_secs_f64()).clamp(0.0, 1.0) as f32;
    AURORA_COLOR[3] + (AURORA_REST_ALPHA - AURORA_COLOR[3]) * t
}

/// A screen-facing diamond of `AURORA_PIXELS` radius at `point`.
pub fn aurora(point: Vector3, camera: &Camera, alpha: f32) -> (Vec<Vertex>, Vec<u32>) {
    let radius = scale_for(camera, point, AURORA_PIXELS);
    let right = camera_right(camera).scale(radius);
    let up = camera_up(camera).scale(radius);
    let color = [AURORA_COLOR[0], AURORA_COLOR[1], AURORA_COLOR[2], alpha];
    let vertices = vec![
        unlit(point.add(&up), color),
        unlit(point.add(&right), color),
        unlit(point.sub(&up), color),
        unlit(point.sub(&right), color),
    ];
    (vertices, vec![0, 1, 2, 0, 2, 3])
}

/// Dashed rays from a snapped point to the workplane's `U` and `V` axes.
///
/// `local` is the snap's position in the plane: the point is `(a, b)`, so the rays run
/// from it to `(0, b)` (along `−u`) and to `(a, 0)` (along `−v`).
pub fn rays(workplane: &Workplane, local: [f64; 2], camera: &Camera) -> Vec<Vertex> {
    let point = workplane.point(local[0], local[1]);
    let dash = scale_for(camera, point, RAY_DASH_PIXELS).max(1e-6);
    let mut vertices = Vec::new();
    push_dashed(
        &mut vertices,
        point,
        workplane.point(0.0, local[1]),
        dash,
        RAY_COLOR,
    );
    push_dashed(
        &mut vertices,
        point,
        workplane.point(local[0], 0.0),
        dash,
        RAY_COLOR,
    );
    vertices
}

/// The workplane origin's node: a hollow, screen-facing diamond.
pub fn origin_node(workplane: &Workplane, camera: &Camera, bloomed: bool) -> Vec<Vertex> {
    let point = workplane.origin;
    let radius = scale_for(camera, point, ORIGIN_PIXELS);
    let right = camera_right(camera).scale(radius);
    let up = camera_up(camera).scale(radius);
    let alpha = if bloomed { 1.0 } else { ORIGIN_ALPHA };
    let color = [AURORA_COLOR[0], AURORA_COLOR[1], AURORA_COLOR[2], alpha];
    let corners = [
        point.add(&up),
        point.add(&right),
        point.sub(&up),
        point.sub(&right),
    ];
    let mut vertices = Vec::with_capacity(8);
    for index in 0..4 {
        vertices.push(unlit(corners[index], color));
        vertices.push(unlit(corners[(index + 1) % 4], color));
    }
    vertices
}

/// The cache key for an origin node: the plane, the camera and the proximity state.
pub fn origin_key(workplane: &Workplane, camera: &Camera, bloomed: bool) -> u64 {
    let mut parts = Vec::with_capacity(24);
    for v in [workplane.origin, workplane.u, workplane.v, workplane.n] {
        parts.extend_from_slice(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]);
    }
    for value in camera.view.m {
        parts.push(value.to_bits());
    }
    match camera.projection {
        Projection::Perspective { fov_y, .. } => {
            parts.push(0);
            parts.push(fov_y.to_bits());
        }
        Projection::Orthographic { extent, .. } => {
            parts.push(1);
            parts.push(extent.to_bits());
        }
        Projection::Matrix(matrix) => {
            parts.push(2);
            parts.extend(matrix.m.iter().map(|value| value.to_bits()));
        }
    }
    parts.push(u64::from(camera.viewport.device_size()[1]));
    parts.push(u64::from(bloomed));
    hash(&parts)
}

/// Append alternating `dash`-length on/off segments from `from` to `to`.
fn push_dashed(vertices: &mut Vec<Vertex>, from: Vector3, to: Vector3, dash: f64, color: [f32; 4]) {
    let length = to.sub(&from).length();
    if length < dash {
        // Too short to dash: draw the whole run, so a small delta is still readable.
        vertices.push(unlit(from, color));
        vertices.push(unlit(to, color));
        return;
    }
    let count = ((length / (2.0 * dash)).ceil() as usize).min(MAX_DASHES);
    for index in 0..count {
        let start = (index as f64 * 2.0 * dash / length).min(1.0);
        let end = (start + dash / length).min(1.0);
        vertices.push(unlit(lerp(from, to, start), color));
        vertices.push(unlit(lerp(from, to, end), color));
    }
}

fn lerp(from: Vector3, to: Vector3, t: f64) -> Vector3 {
    from.add(&to.sub(&from).scale(t.clamp(0.0, 1.0)))
}

fn unlit(position: Vector3, color: [f32; 4]) -> Vertex {
    Vertex {
        position: [position.x as f32, position.y as f32, position.z as f32],
        normal: LIGHT,
        uv: [0.0, 0.0],
        part_idx: 0,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_graphics::Viewport;

    fn camera() -> Camera {
        Camera {
            viewport: Viewport::new([200, 200], 1.0),
            ..Camera::identity()
        }
    }

    #[test]
    fn the_aurora_flashes_then_settles() {
        assert!((aurora_alpha(Duration::ZERO) - 0.85).abs() < 1e-6);
        let mid = aurora_alpha(FLASH / 2);
        assert!(mid < 0.85 && mid > AURORA_REST_ALPHA, "{mid}");
        assert!((aurora_alpha(FLASH) - AURORA_REST_ALPHA).abs() < 1e-6);
        assert!((aurora_alpha(FLASH * 4) - AURORA_REST_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn the_aurora_is_a_screen_facing_diamond_at_the_snap() {
        let point = Vector3::new(1.0, 2.0, 3.0);
        let (vertices, indices) = aurora(point, &camera(), 0.85);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(vertices.len(), 4);
        assert!(vertices.iter().all(|v| v.color[3] == 0.85));
        // The identity camera has right = +X and up = +Y: the diamond spans both.
        let xs: Vec<f32> = vertices.iter().map(|v| v.position[0]).collect();
        let ys: Vec<f32> = vertices.iter().map(|v| v.position[1]).collect();
        assert!(xs.iter().any(|x| *x > point.x as f32 + 1e-6));
        assert!(xs.iter().any(|x| *x < point.x as f32 - 1e-6));
        assert!(ys.iter().any(|y| *y > point.y as f32 + 1e-6));
        assert!(ys.iter().any(|y| *y < point.y as f32 - 1e-6));
    }

    #[test]
    fn the_rays_dash_toward_both_local_axes() {
        let plane = Workplane::world();
        let vertices = rays(&plane, [2.0, 3.0], &camera());
        assert!(!vertices.is_empty());
        assert_eq!(vertices.len() % 2, 0, "pairs per dash");
        // Every dash lies on one of the two rays: x = 2 or y = 3 (z = 0).
        for vertex in &vertices {
            let [x, y, z] = vertex.position;
            assert!(z.abs() < 1e-6);
            assert!(
                (x - 2.0).abs() < 1e-5 || (y - 3.0).abs() < 1e-5,
                "off both rays: {vertex:?}"
            );
        }
        // The first ray starts at the snap and ends on the V axis.
        assert!((vertices[0].position[0] - 2.0).abs() < 1e-5);
        assert!((vertices[0].position[1] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn a_short_run_is_a_single_segment() {
        // A run shorter than one dash is drawn whole, so a small delta is still visible.
        let mut vertices = Vec::new();
        push_dashed(
            &mut vertices,
            Vector3::zero(),
            Vector3::new(0.5, 0.0, 0.0),
            1.0,
            RAY_COLOR,
        );
        assert_eq!(vertices.len(), 2);
    }

    #[test]
    fn the_origin_node_blooms_with_proximity() {
        let plane = Workplane::world();
        let resting = origin_node(&plane, &camera(), false);
        let bloomed = origin_node(&plane, &camera(), true);
        assert_eq!(resting.len(), 8, "a hollow diamond: four segments");
        assert!((resting[0].color[3] - ORIGIN_ALPHA).abs() < 1e-6);
        assert_eq!(bloomed[0].color[3], 1.0);
        assert_ne!(
            origin_key(&plane, &camera(), false),
            origin_key(&plane, &camera(), true)
        );
    }
}
