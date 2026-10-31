//! The rotate session's 3-D protractor.
//!
//! A stage producer, like [`measure_tape`](crate::measure_tape): it returns raw vertices
//! for the viewport's overlay queues and knows nothing about gpui or a document. It is the
//! angular twin of the linear tape:
//!
//! ```text
//!          current ── 15°
//!            ╱
//!   origin  ╱  filled glass sector
//!      │   ╱   between the press spoke and the live spoke
//!      │  ╱
//!    start
//! ```
//!
//! The press spoke is dashed, the live spoke solid, the swept sector is a 11 % tint of
//! the axis token, and a detent tick sits every `detent_deg` around the ring so the
//! snapping positions are visible before the value reaches them (`Ctrl`/free rotation
//! hides them). All packets are depth-off and never pickable.

use std::f64::consts::TAU;

use ferrocad_graphics::theme::linear_from_srgb_hex;
use ferrocad_graphics::{Camera, Vertex};
use ferrocad_types::Vector3;

use crate::gizmo::RingSweep;
use crate::screen::scale_for;

/// A dashed press spoke's dash length, in device pixels.
pub const DASH_PIXELS: f64 = 4.0;
/// A detent tick's length, in device pixels.
pub const TICK_PIXELS: f64 = 7.0;
/// The swept fan's per-vertex alpha. The fan emits both windings so it survives culling
/// from either side; the two co-located triangles compose to about `11 %` on screen.
pub const FAN_ALPHA: f32 = 0.06;
/// The rim/current-spoke alpha.
const RIM_ALPHA: f32 = 0.85;
/// The dashed press spoke's alpha.
const START_ALPHA: f32 = 0.62;
/// The detent ticks' alpha.
const TICK_ALPHA: f32 = 0.32;
/// The arc's angular resolution: one fan triangle per this many degrees.
const SEGMENT_DEGREES: f64 = 5.0;
/// A guard against a many-revolution sweep.
const MAX_SEGMENTS: usize = 192;
/// The axis tokens as authored sRGB: `#F87171`, `#4ADE80`, `#38BDF8`.
const AXIS_HEX: [u32; 3] = [0xf87171, 0x4ade80, 0x38bdf8];

/// The protractor's geometry, split so the renderer draws the translucent fan as
/// triangles and the ink as lines.
#[derive(Default)]
pub struct ProtractorGeometry {
    /// The swept sector (triangle list).
    pub fan: Vec<Vertex>,
    /// The press/current spokes and the swept rim (line pairs).
    pub spokes: Vec<Vertex>,
    /// The detent ticks around the ring (line pairs).
    pub ticks: Vec<Vertex>,
}

impl ProtractorGeometry {
    /// Whether nothing would be drawn.
    pub fn is_empty(&self) -> bool {
        self.fan.is_empty() && self.spokes.is_empty() && self.ticks.is_empty()
    }
}

/// Build the protractor for every sweep. A zero sweep draws nothing.
pub fn build(sweeps: &[RingSweep], camera: &Camera) -> ProtractorGeometry {
    let mut geometry = ProtractorGeometry::default();
    for sweep in sweeps {
        if sweep.degrees.abs() < 1e-6 || sweep.radius <= 0.0 {
            continue;
        }
        let color = axis_color(sweep.axis);
        let swept = sweep.degrees.to_radians();
        let segments =
            ((sweep.degrees.abs() / SEGMENT_DEGREES).ceil() as usize).clamp(2, MAX_SEGMENTS);

        // The fan: triangles from the centre to consecutive arc points, both windings so
        // the glass reads from either side of the plane.
        let mut previous = sweep.start;
        for index in 1..=segments {
            let t = index as f64 / segments as f64;
            let next = rotate_about(&sweep.start, &sweep.axis, swept * t);
            push_fan_triangle(&mut geometry.fan, sweep, previous, next, color);
            previous = next;
        }

        // The spokes: the press direction dashed, the live one solid.
        let start_end = sweep.origin.add(&sweep.start.scale(sweep.radius));
        let dash = scale_for(camera, start_end, DASH_PIXELS).max(1e-6);
        push_dashed(
            &mut geometry.spokes,
            sweep.origin,
            start_end,
            dash,
            with_alpha(color, START_ALPHA),
        );
        push_line(
            &mut geometry.spokes,
            sweep.origin,
            sweep.origin.add(&sweep.current.scale(sweep.radius)),
            with_alpha(color, RIM_ALPHA),
        );

        // The swept rim, so the sector has a readable edge at a grazing angle.
        let mut previous = sweep.start;
        for index in 1..=segments {
            let t = index as f64 / segments as f64;
            let next = rotate_about(&sweep.start, &sweep.axis, swept * t);
            push_line(
                &mut geometry.spokes,
                sweep.origin.add(&previous.scale(sweep.radius)),
                sweep.origin.add(&next.scale(sweep.radius)),
                with_alpha(color, RIM_ALPHA),
            );
            previous = next;
        }

        // Detent ticks around the whole ring. Free rotation has no detent and no ticks.
        if sweep.detent_deg > 1e-9 {
            let step = sweep.detent_deg.to_radians();
            let count = (TAU / step).round().clamp(1.0, 72.0) as usize;
            let inner = sweep.radius * 0.94;
            let outer = sweep.radius * 1.02;
            for index in 0..count {
                let direction = rotate_about(&sweep.start, &sweep.axis, step * index as f64);
                push_line(
                    &mut geometry.ticks,
                    sweep.origin.add(&direction.scale(inner)),
                    sweep.origin.add(&direction.scale(outer)),
                    with_alpha(color, TICK_ALPHA),
                );
            }
        }
    }
    geometry
}

/// The axis token for a ring's normal: the dominant world component.
fn axis_color(axis: Vector3) -> [f32; 4] {
    let absolute = [axis.x.abs(), axis.y.abs(), axis.z.abs()];
    let index = absolute
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(index, _)| index)
        .unwrap_or(2);
    linear_from_srgb_hex(AXIS_HEX[index.min(2)])
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha;
    color
}

fn push_fan_triangle(
    vertices: &mut Vec<Vertex>,
    sweep: &RingSweep,
    from: Vector3,
    to: Vector3,
    color: [f32; 4],
) {
    let a = sweep.origin.add(&from.scale(sweep.radius));
    let b = sweep.origin.add(&to.scale(sweep.radius));
    let color = with_alpha(color, FAN_ALPHA);
    // Both windings: the fan stays visible from either side of the ring plane.
    for (first, second) in [(a, b), (b, a)] {
        vertices.push(surface(sweep.origin, sweep.axis, color));
        vertices.push(surface(first, sweep.axis, color));
        vertices.push(surface(second, sweep.axis, color));
    }
}

fn push_line(vertices: &mut Vec<Vertex>, from: Vector3, to: Vector3, color: [f32; 4]) {
    vertices.push(unlit(from, color));
    vertices.push(unlit(to, color));
}

fn push_dashed(vertices: &mut Vec<Vertex>, from: Vector3, to: Vector3, dash: f64, color: [f32; 4]) {
    let length = to.sub(&from).length();
    if length < dash {
        push_line(vertices, from, to, color);
        return;
    }
    let count = ((length / (2.0 * dash)).ceil() as usize).min(512);
    for index in 0..count {
        let start = (index as f64 * 2.0 * dash / length).min(1.0);
        let end = (start + dash / length).min(1.0);
        push_line(vertices, lerp(from, to, start), lerp(from, to, end), color);
    }
}

fn rotate_about(v: &Vector3, axis: &Vector3, angle: f64) -> Vector3 {
    let (sin, cos) = angle.sin_cos();
    v.scale(cos)
        .add(&axis.cross(v).scale(sin))
        .add(&axis.scale(axis.dot(v) * (1.0 - cos)))
}

fn lerp(from: Vector3, to: Vector3, t: f64) -> Vector3 {
    from.add(&to.sub(&from).scale(t.clamp(0.0, 1.0)))
}

fn unlit(position: Vector3, color: [f32; 4]) -> Vertex {
    Vertex {
        position: [position.x as f32, position.y as f32, position.z as f32],
        normal: [0.0, 0.0, 1.0],
        uv: [0.0, 0.0],
        part_idx: 0,
        color,
    }
}

/// A fan vertex: unlit-normal convention from [`unlit`], with the ring normal so the
/// lit overlay shader shades it like the ring plane.
fn surface(position: Vector3, axis: Vector3, color: [f32; 4]) -> Vertex {
    let mut vertex = unlit(position, color);
    vertex.normal = [axis.x as f32, axis.y as f32, axis.z as f32];
    vertex
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

    fn sweep(degrees: f64, detent: f64) -> RingSweep {
        let axis = Vector3::new(0.0, 0.0, 1.0);
        let start = Vector3::new(1.0, 0.0, 0.0);
        let current = rotate_about(&start, &axis, degrees.to_radians());
        RingSweep {
            origin: Vector3::zero(),
            axis,
            start,
            current,
            radius: 4.0,
            degrees,
            detent_deg: detent,
        }
    }

    #[test]
    fn a_zero_sweep_draws_nothing() {
        assert!(build(&[sweep(0.0, 15.0)], &camera()).is_empty());
    }

    #[test]
    fn a_sweep_draws_a_fan_two_spokes_and_detent_ticks() {
        let geometry = build(&[sweep(45.0, 15.0)], &camera());
        assert!(!geometry.fan.is_empty());
        assert_eq!(geometry.fan.len() % 3, 0, "triangle list");
        assert!(!geometry.spokes.is_empty());
        // 24 detents at 15° is a full turn.
        assert_eq!(geometry.ticks.len() / 2, 24);
    }

    #[test]
    fn free_rotation_has_no_detent_ticks() {
        let geometry = build(&[sweep(45.0, 0.0)], &camera());
        assert!(geometry.ticks.is_empty());
        assert!(!geometry.fan.is_empty());
    }
}
