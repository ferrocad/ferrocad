//! The transform session's 3-D measuring tape.
//!
//! A stage producer, like [`feedback`](crate::feedback): it returns raw vertices for the
//! viewport's line queue and knows nothing about gpui, a document or a transaction.
//! The geometry is rebuilt from the session snapshot each paint, because the dash, tick and
//! ring sizes are screen-invariant and therefore depend on the camera.
//!
//! The path is the orthogonal stair-step from the session datum to the previewed origin,
//! in the **active transform frame** (the same basis the gizmo and the fields use):
//!
//! ```text
//! O_start ──ΔX──► P_x ──ΔY──► P_xy ──ΔZ──► O_current
//! ```
//!
//! A `LOC` move therefore shows one leg along the body's own axis and its signed local
//! displacement, instead of the world-axis decomposition of the same world delta. Only a
//! non-zero frame component draws a leg, each leg carries its axis token, and every corner
//! gets a perpendicular tick. The ghost ring marks the datum. The tape is depth-off and
//! never pickable; it is a measurement, not geometry.

use std::f64::consts::TAU;

use ferrocad_graphics::theme::linear_from_srgb_hex;
use ferrocad_graphics::{Camera, Vertex};
use ferrocad_types::Vector3;

use crate::gizmo::INACTIVE_COLOR;
use crate::screen::{camera_forward, camera_right, camera_up, scale_for};
use crate::session::TransformTape;

/// A leg's dash length, in device pixels (the projection-ray technique).
pub const DASH_PIXELS: f64 = 4.0;
/// A corner tick's total length, in device pixels.
pub const TICK_PIXELS: f64 = 6.0;
/// The ghost origin ring's radius, in device pixels.
pub const RING_PIXELS: f64 = 10.0;
/// The ghost ring's alpha: `#94A3B8` at half opacity.
const RING_ALPHA: f32 = 0.50;
/// A guard against a pathologically long leg.
const MAX_DASHES: usize = 512;
/// How many segments approximate the ghost ring.
const RING_SEGMENTS: usize = 32;
/// The axis tokens as authored sRGB: `#F87171`, `#4ADE80`, `#38BDF8`.
const AXIS_HEX: [u32; 3] = [0xf87171, 0x4ade80, 0x38bdf8];

/// The three axis inks, in world-axis order (X/Y/Z).
fn axis_color(axis: usize) -> [f32; 4] {
    linear_from_srgb_hex(AXIS_HEX[axis.min(2)])
}

/// The ghost origin ink: the inactive gizmo token at half opacity.
fn ring_color() -> [f32; 4] {
    [
        INACTIVE_COLOR[0],
        INACTIVE_COLOR[1],
        INACTIVE_COLOR[2],
        RING_ALPHA,
    ]
}

/// The tape's line geometry, split so the renderer can draw it as three line packets and
/// the tests can inspect the path, its ticks and the ring separately.
#[derive(Default)]
pub struct TapeGeometry {
    /// The ghost ring at the datum (line pairs).
    pub ring: Vec<Vertex>,
    /// The dashed stair-step legs (line pairs).
    pub legs: Vec<Vertex>,
    /// The perpendicular six-pixel ticks at each corner (line pairs).
    pub ticks: Vec<Vertex>,
}

impl TapeGeometry {
    /// Whether nothing would be drawn.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty() && self.legs.is_empty() && self.ticks.is_empty()
    }
}

/// Build the tape for `tape` against `camera`. A clean tape returns empty geometry.
pub fn build(tape: &TransformTape, camera: &Camera) -> TapeGeometry {
    if !tape.is_visible() {
        return TapeGeometry::default();
    }
    let mut geometry = TapeGeometry {
        ring: ring(tape.datum(), camera),
        ..TapeGeometry::default()
    };
    let mut current = tape.datum();
    for axis in 0..3 {
        let amount = tape.axis_delta(axis);
        // A pure frame-axis drag leaves tiny floating-point residue on the others.
        if amount.abs() <= 1e-9 {
            continue;
        }
        let direction = tape.axis(axis);
        let next = current.add(&direction.scale(amount));
        let color = axis_color(axis);
        let mid = current.add(&direction.scale(amount * 0.5));
        let dash = scale_for(camera, mid, DASH_PIXELS).max(1e-6);
        push_dashed(&mut geometry.legs, current, next, dash, color);
        push_tick(&mut geometry.ticks, current, direction, camera, color);
        push_tick(&mut geometry.ticks, next, direction, camera, color);
        current = next;
    }
    geometry
}

/// A screen-facing ring of [`RING_PIXELS`] radius at `at`, as line segments.
fn ring(at: Vector3, camera: &Camera) -> Vec<Vertex> {
    let radius = scale_for(camera, at, RING_PIXELS);
    let right = camera_right(camera);
    let up = camera_up(camera);
    let color = ring_color();
    let mut vertices = Vec::with_capacity(RING_SEGMENTS * 2);
    for index in 0..RING_SEGMENTS {
        let a = index as f64 / RING_SEGMENTS as f64 * TAU;
        let b = (index + 1) as f64 / RING_SEGMENTS as f64 * TAU;
        let first = at
            .add(&right.scale(a.cos() * radius))
            .add(&up.scale(a.sin() * radius));
        let second = at
            .add(&right.scale(b.cos() * radius))
            .add(&up.scale(b.sin() * radius));
        vertices.push(unlit(first, color));
        vertices.push(unlit(second, color));
    }
    vertices
}

/// A six-pixel tick through `at`, perpendicular to `along` and facing the camera.
fn push_tick(
    vertices: &mut Vec<Vertex>,
    at: Vector3,
    along: Vector3,
    camera: &Camera,
    color: [f32; 4],
) {
    let forward = camera_forward(camera);
    let mut normal = along.cross(&forward);
    if normal.length() < 1e-9 {
        normal = camera_right(camera);
    } else {
        normal = normal.normalize();
    }
    let half = scale_for(camera, at, TICK_PIXELS * 0.5);
    vertices.push(unlit(at.sub(&normal.scale(half)), color));
    vertices.push(unlit(at.add(&normal.scale(half)), color));
}

/// Append alternating `dash`-length on/off segments from `from` to `to`.
///
/// The same technique as `ferrocad_stage::feedback`'s projection rays: WebGPU lines are
/// solid, so a dash is a separate short segment.
fn push_dashed(
    vertices: &mut Vec<Vertex>,
    from: Vector3,
    to: Vector3,
    dash: f64,
    color: [f32; 4],
) {
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
        // The line pipeline is unlit; the normal is unused but must be a unit-ish value.
        normal: [0.0, 0.0, 1.0],
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
    fn a_tape_draws_one_leg_along_the_frame_axis() {
        let camera = camera();
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let basis = [
            Vector3::new(half, half, 0.0),
            Vector3::new(-half, half, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ];
        // Ten along the rotated frame's X. A world decomposition would stair-step through
        // world X and Y and never keep `x == y`.
        let delta = Vector3::new(10.0 * half, 10.0 * half, 0.0);
        let tape = TransformTape::in_frame(Vector3::zero(), delta, basis);
        let geometry = build(&tape, &camera);
        assert!(!geometry.legs.is_empty());
        for vertex in &geometry.legs {
            let [x, y, _] = vertex.position;
            assert!(
                (f64::from(x) - f64::from(y)).abs() < 1e-3,
                "the single leg follows the frame X, not a world stair-step: {vertex:?}"
            );
        }
    }

    fn tape(delta: [f64; 3]) -> TransformTape {
        TransformTape::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(delta[0], delta[1], delta[2]))
    }

    #[test]
    fn a_clean_tape_draws_nothing() {
        assert!(build(&tape([0.0, 0.0, 0.0]), &camera()).is_empty());
    }

    #[test]
    fn the_legs_follow_only_the_nonzero_axes_in_order() {
        let geometry = build(&tape([5.0, 0.0, 3.0]), &camera());
        assert!(!geometry.legs.is_empty());
        assert_eq!(geometry.legs.len() % 2, 0, "pairs per dash");
        // Every leg vertex lies on the X leg (y, z fixed) or the Z leg (x, y fixed).
        for vertex in &geometry.legs {
            let [x, y, z] = vertex.position;
            assert!((1.0..=6.0).contains(&(x as f64)), "off the X leg: {vertex:?}");
            assert!((y as f64 - 2.0).abs() < 1e-5, "off the X leg: {vertex:?}");
            assert!(
                (z as f64 - 3.0).abs() < 1e-5 || (x as f64 - 6.0).abs() < 1e-5,
                "off both legs: {vertex:?}"
            );
        }
        // The first leg starts at the datum and the path never takes a Y leg.
        assert!((geometry.legs[0].position[0] - 1.0).abs() < 1e-5);
        assert!((geometry.legs[0].position[1] - 2.0).abs() < 1e-5);
        assert!((geometry.legs[0].position[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn each_leg_and_tick_carries_its_axis_token() {
        let geometry = build(&tape([5.0, 0.0, 3.0]), &camera());
        let x = axis_color(0);
        let z = axis_color(2);
        assert!(geometry.legs.iter().any(|v| v.color == x));
        assert!(geometry.legs.iter().any(|v| v.color == z));
        assert!(!geometry.legs.iter().any(|v| v.color == axis_color(1)));
        assert!(geometry.ticks.iter().all(|v| v.color == x || v.color == z));
    }

    #[test]
    fn every_corner_gets_a_perpendicular_tick() {
        let geometry = build(&tape([5.0, 0.0, 3.0]), &camera());
        // Two legs, each with a start and an end tick.
        assert_eq!(geometry.ticks.len(), 8);
        let length = |pair: &[Vertex]| {
            let dx = pair[0].position[0] - pair[1].position[0];
            let dy = pair[0].position[1] - pair[1].position[1];
            let dz = pair[0].position[2] - pair[1].position[2];
            (f64::from(dx * dx + dy * dy + dz * dz)).sqrt()
        };
        let pixel = |pair: &[Vertex]| {
            let mid = Vector3::new(
                (f64::from(pair[0].position[0]) + f64::from(pair[1].position[0])) * 0.5,
                (f64::from(pair[0].position[1]) + f64::from(pair[1].position[1])) * 0.5,
                (f64::from(pair[0].position[2]) + f64::from(pair[1].position[2])) * 0.5,
            );
            scale_for(&camera(), mid, 1.0)
        };
        for pair in geometry.ticks.chunks(2) {
            let expected = TICK_PIXELS * pixel(pair);
            assert!(
                (length(pair) - expected).abs() < 1e-4,
                "a tick is {TICK_PIXELS} px: {} vs {expected}",
                length(pair)
            );
        }
    }

    #[test]
    fn the_ghost_ring_sits_at_the_datum() {
        let geometry = build(&tape([5.0, 0.0, 0.0]), &camera());
        assert_eq!(geometry.ring.len(), RING_SEGMENTS * 2);
        let radius = scale_for(&camera(), Vector3::new(1.0, 2.0, 3.0), RING_PIXELS);
        for vertex in &geometry.ring {
            let dx = f64::from(vertex.position[0]) - 1.0;
            let dy = f64::from(vertex.position[1]) - 2.0;
            let dz = f64::from(vertex.position[2]) - 3.0;
            assert!(
                (dx * dx + dy * dy + dz * dz).sqrt() - radius < 1e-5,
                "off the ring: {vertex:?}"
            );
            assert_eq!(vertex.color[3], RING_ALPHA);
        }
    }
}
