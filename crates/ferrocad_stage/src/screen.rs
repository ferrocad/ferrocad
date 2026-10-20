//! Screen-space helpers shared by the stage's manipulators.
//!
//! Projection, pixel-scaled sizes and stroke geometry are the same for the
//! transform gizmo and for a workbench manipulator (the edge fillet arc in
//! `ferrocad_part_gui`). They live here so a workbench's presentation crate can
//! draw a screen-invariant overlay without reaching into the gizmo's internals.

use ferrocad_graphics::{Camera, Projection};
use ferrocad_types::{Matrix4, Vector3};

/// The visual stroke width, in device pixels.
pub const STROKE_PIXELS: f64 = 1.5;

/// The visual half-width of a handle stroke, in device pixels.
pub const HIT_TOLERANCE_PIXELS: f64 = 2.0;

/// Extra grab padding around a handle, in device pixels.
pub const HIT_PADDING_PIXELS: f64 = 10.0;

/// The total screen-space grab radius: stroke tolerance + padding.
pub const HIT_SLOP: f64 = HIT_TOLERANCE_PIXELS + HIT_PADDING_PIXELS;

/// The world direction the camera looks along (view space −Z in world space).
pub fn camera_forward(camera: &Camera) -> Vector3 {
    // The view matrix's third row is the world direction of view-space +Z (toward the
    // eye); the camera looks along its negation.
    Vector3::new(camera.view.m[8], camera.view.m[9], camera.view.m[10]).scale(-1.0)
}

/// The camera's screen-right direction in world space (the view matrix's first row).
pub fn camera_right(camera: &Camera) -> Vector3 {
    Vector3::new(camera.view.m[0], camera.view.m[1], camera.view.m[2])
}

/// The camera's screen-up direction in world space (the view matrix's second row).
pub fn camera_up(camera: &Camera) -> Vector3 {
    Vector3::new(camera.view.m[4], camera.view.m[5], camera.view.m[6])
}

/// Project a world point to device pixels (bottom-left origin).
///
/// `None` when the point is on (or behind) the camera's `w = 0` plane: a point behind a
/// perspective eye would otherwise project to a mirrored screen position.
pub fn project_device(camera: &Camera, point: Vector3) -> Option<[f64; 2]> {
    let m = camera.view_projection().m;
    let w = m[12] * point.x + m[13] * point.y + m[14] * point.z + m[15];
    if w <= 1e-9 {
        return None;
    }
    let x = (m[0] * point.x + m[1] * point.y + m[2] * point.z + m[3]) / w;
    let y = (m[4] * point.x + m[5] * point.y + m[6] * point.z + m[7]) / w;
    let [width, height] = camera.viewport.device_size();
    Some([
        (x * 0.5 + 0.5) * f64::from(width),
        (y * 0.5 + 0.5) * f64::from(height),
    ])
}

/// The distance from `point` to the segment `a`–`b`, plus the segment parameter of the
/// closest point.
pub fn point_segment_distance(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    if length_squared < 1e-12 {
        return ((point[0] - a[0]).hypot(point[1] - a[1]), 0.0);
    }
    let t = (((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / length_squared).clamp(0.0, 1.0);
    let closest = [a[0] + dx * t, a[1] + dy * t];
    ((point[0] - closest[0]).hypot(point[1] - closest[1]), t)
}

/// The world-space radius that keeps a `pixels`-radius overlay on screen, given the
/// camera.
///
/// `radius ∝ distance / (m5 * viewport_height)`, where `m5` is the projection's
/// vertical scale (`1 / tan(fov/2)`), so the apparent size is constant.
pub fn scale_for(camera: &Camera, origin: Vector3, pixels: f64) -> f64 {
    let height = f64::from(camera.viewport.device_size()[1].max(1));
    // Orthographic projection has no depth scaling: a pixel is a fixed world size.
    if let Projection::Orthographic { extent, .. } = camera.projection {
        return 2.0 * pixels * extent / height;
    }
    let eye = camera_position(&camera.view);
    let distance = eye.sub(&origin).length().max(1e-6);
    let m5 = camera.projection_matrix().m[5].abs().max(1e-6);
    2.0 * pixels * distance / (m5 * height)
}

/// The camera's world position from its view matrix (`view = [R | t]` → `-Rᵀ·t`).
pub fn camera_position(view: &Matrix4) -> Vector3 {
    let m = &view.m;
    let t = Vector3::new(m[3], m[7], m[11]);
    // Rᵀ·t
    Vector3::new(
        m[0] * t.x + m[4] * t.y + m[8] * t.z,
        m[1] * t.x + m[5] * t.y + m[9] * t.z,
        m[2] * t.x + m[6] * t.y + m[10] * t.z,
    )
    .scale(-1.0)
}

/// A `Vector3` in the draw contract's `f32` form.
pub fn to3(v: Vector3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

/// Append a ribbon quad (two triangles) when the segment is not view-parallel.
pub fn push_ribbon(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<u32>,
    quad: Option<[Vector3; 4]>,
) {
    let Some(quad) = quad else {
        return;
    };
    let base = positions.len() as u32;
    for point in quad {
        positions.push(to3(point));
    }
    triangles.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}
