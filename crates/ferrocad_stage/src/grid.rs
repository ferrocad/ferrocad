//! The ground grid and the world origin triad (G2).
//!
//! A grid is a **stage producer**, not a second renderer: [`build`] returns vertices
//! for three packets (dots, major lines, the triad) that the viewport appends to the
//! same queue as the document. Each vertex carries its own colour, which is what makes
//! the radial alpha fade possible.
//!
//! The producer is pure arithmetic: no kernel, no gpui, no Python. The viewport owns
//! the mesh identities, the projection culling and the depth ordering.

use ferrocad_graphics::{Camera, Projection, Vertex, screen_ribbon};
use ferrocad_types::Vector3;

use crate::screen::{STROKE_PIXELS, camera_forward, project_device, scale_for};
use crate::workplane::Workplane;

/// The default minor spacing (10 mm).
pub const MINOR_STEP: f64 = 10.0;
/// The default major spacing (100 mm).
pub const MAJOR_STEP: f64 = 100.0;
/// The triad stem length, in device pixels.
pub const TRIAD_PIXELS: f64 = 80.0;
/// The radius clamp (`0.5 m .. 5 m`, in millimetres).
pub const RADIUS_RANGE: (f64, f64) = (500.0, 5_000.0);
/// A guard against a zoomed-out dot field; the viewport promotes the step first.
const MAX_DOTS: usize = 40_000;

/// Linear `#94A3B8` (the visual language's structural ink).
pub const GRID_COLOR: [f32; 4] = [0.296, 0.366, 0.480, 1.0];
/// The world grid's alpha at the origin.
pub const GRID_ALPHA: f32 = 0.15;
/// The triad stems' linear colours (`#F87171`, `#4ADE80`, `#38BDF8`) at 65 %.
pub const AXIS_COLORS: [[f32; 4]; 3] = [
    [0.939, 0.165, 0.165, 0.65],
    [0.069, 0.730, 0.216, 0.65],
    [0.040, 0.509, 0.939, 0.65],
];

/// The direction the fragment shader's light comes from, normalized. Emitting it as the
/// vertex normal makes the diffuse term `1`, so the grid and the triad render unlit —
/// their per-vertex colour is exactly what reaches the screen.
const LIGHT: [f32; 3] = [0.3714, 0.7428, 0.5571];

/// The plane, steps and extent of one grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSpec {
    pub workplane: Workplane,
    /// The dot spacing (10 mm for the world).
    pub minor: f64,
    /// The line spacing (100 mm for the world).
    pub major: f64,
    /// The disc radius around the workplane origin.
    pub radius: f64,
}

impl GridSpec {
    /// The world grid at `radius`.
    pub fn world(radius: f64) -> Self {
        Self {
            workplane: Workplane::world(),
            minor: MINOR_STEP,
            major: MAJOR_STEP,
            radius: radius.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1),
        }
    }

    /// The radius the spec suggests for `camera`: `2.5 ×` the eye distance, clamped.
    pub fn radius_for(camera: &Camera) -> f64 {
        let eye = crate::screen::camera_position(&camera.view).length();
        (2.5 * eye).clamp(RADIUS_RANGE.0, RADIUS_RANGE.1)
    }
}

/// The grid's appearance.
#[derive(Clone, Debug, PartialEq)]
pub struct GridStyle {
    /// Linear RGB; alpha is taken from the per-vertex fade.
    pub color: [f32; 4],
    pub minor_alpha: f32,
    pub major_alpha: f32,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            color: GRID_COLOR,
            minor_alpha: GRID_ALPHA,
            major_alpha: GRID_ALPHA,
        }
    }
}

/// The steps that survive the screen-space cull.
///
/// Below `MINOR_PIXELS` of projected spacing the minor level is dropped and the step
/// moves a decade, so a zoomed-out view does not turn into moiré.
pub const MINOR_PIXELS: f64 = 4.0;

/// The effective `(minor, major)` steps for `camera` on `workplane`.
pub fn effective_steps(
    camera: &Camera,
    workplane: &Workplane,
    minor: f64,
    major: f64,
) -> (f64, f64) {
    let spacing = |step: f64| -> Option<f64> {
        let a = project_device(camera, workplane.point(0.0, 0.0))?;
        let b = project_device(camera, workplane.point(step, 0.0))?;
        Some((a[0] - b[0]).hypot(a[1] - b[1]))
    };
    match spacing(minor) {
        Some(pixels) if pixels >= MINOR_PIXELS => (minor, major),
        _ => (major, major * 10.0),
    }
}

/// The vertices for the three packets, in draw order (dots, lines, triad).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridGeometry {
    /// `Topology::Points`.
    pub dots: Vec<Vertex>,
    /// `Topology::Lines` (two vertices per segment).
    pub lines: Vec<Vertex>,
    /// `Topology::Triangles` (the triad ribbons).
    pub triangles: Vec<Vertex>,
    pub triangle_indices: Vec<u32>,
}

/// Build the grid for `spec`, projected with `camera`.
///
/// The dots and lines carry the radial fade in their alpha; the triad is screen-
/// invariant (`TRIAD_PIXELS` long, `STROKE_PIXELS` wide) and uses the axis hues.
pub fn build(spec: &GridSpec, style: &GridStyle, camera: &Camera) -> GridGeometry {
    let (dots, lines) = ground(spec, style);
    let (triangles, triangle_indices) = triad(&spec.workplane, camera);
    GridGeometry {
        dots,
        lines,
        triangles,
        triangle_indices,
    }
}

/// The ground vertices (dots, then major lines) for `spec`.
///
/// This half depends only on the spec and the style, so a camera orbit does not
/// rebuild it; the viewport keys its cache on the spec.
pub fn ground(spec: &GridSpec, style: &GridStyle) -> (Vec<Vertex>, Vec<Vertex>) {
    let radius = spec.radius.max(1e-6);
    let minor = spec.minor.max(1e-9);
    let mut dots = Vec::new();
    let mut lines = Vec::new();

    // Dots, skipping the major lines (the lines own those pixels).
    let count = (radius / minor).ceil() as i64;
    if ((2 * count + 1) * (2 * count + 1)) as usize <= MAX_DOTS {
        for ix in -count..=count {
            let a = ix as f64 * minor;
            for iy in -count..=count {
                let b = iy as f64 * minor;
                if on_major_line(a, b, spec.major) {
                    continue;
                }
                let distance = (a * a + b * b).sqrt();
                if distance > radius {
                    continue;
                }
                let alpha = fade(style.minor_alpha, distance, radius);
                if alpha <= 0.0 {
                    continue;
                }
                dots.push(vertex(spec.workplane.point(a, b), ink(style, alpha)));
            }
        }
    }

    // Major lines, in both directions, subdivided so the fade peaks in the middle
    // rather than interpolating from two zero-alpha ends.
    let major = spec.major.max(minor);
    let step = if radius / minor > 2_000.0 {
        major
    } else {
        minor
    };
    let lines_count = (radius / major).ceil() as i64;
    for k in -lines_count..=lines_count {
        let offset = k as f64 * major;
        let half = (radius * radius - offset * offset).sqrt();
        if half <= 0.0 {
            continue;
        }
        add_line(&mut lines, spec, style, offset, true, half, step, radius);
        add_line(&mut lines, spec, style, offset, false, half, step, radius);
    }

    (dots, lines)
}

/// The triad's screen-facing ribbons at the workplane origin: three stems.
pub fn triad(workplane: &Workplane, camera: &Camera) -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(12);
    let mut indices = Vec::with_capacity(18);
    let forward = camera_forward(camera);
    let origin = workplane.origin;
    let length = scale_for(camera, origin, TRIAD_PIXELS);
    let half_width = scale_for(camera, origin, STROKE_PIXELS / 2.0);
    let axes = [
        (workplane.u, AXIS_COLORS[0]),
        (workplane.v, AXIS_COLORS[1]),
        (workplane.n, AXIS_COLORS[2]),
    ];
    for (axis, color) in axes {
        let tip = origin.add(&axis.scale(length));
        let Some(quad) = screen_ribbon(origin, tip, half_width, forward) else {
            continue;
        };
        let base = vertices.len() as u32;
        for point in quad {
            vertices.push(vertex(point, color));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}

/// A key for the ground cache: the spec's numbers and plane.
pub fn ground_key(spec: &GridSpec) -> u64 {
    let plane = &spec.workplane;
    let mut parts = vec![
        spec.minor.to_bits(),
        spec.major.to_bits(),
        spec.radius.to_bits(),
    ];
    for v in [plane.origin, plane.u, plane.v, plane.n] {
        parts.extend_from_slice(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]);
    }
    hash(&parts)
}

/// A key for the triad cache: the plane and the camera's view matrix.
pub fn triad_key(workplane: &Workplane, camera: &Camera) -> u64 {
    let mut parts = Vec::with_capacity(24 + 16);
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
    hash(&parts)
}

fn hash(parts: &[u64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        hash ^= part;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Add one major line along `u` (when `along_u`) or `v`, from `-half` to `+half`.
#[allow(clippy::too_many_arguments)]
fn add_line(
    lines: &mut Vec<Vertex>,
    spec: &GridSpec,
    style: &GridStyle,
    offset: f64,
    along_u: bool,
    half: f64,
    step: f64,
    radius: f64,
) {
    let mut t = -half;
    while t < half - 1e-9 {
        let next = (t + step).min(half);
        if next <= t + 1e-12 {
            break;
        }
        let (a0, b0) = if along_u { (t, offset) } else { (offset, t) };
        let (a1, b1) = if along_u {
            (next, offset)
        } else {
            (offset, next)
        };
        let alpha0 = fade(style.major_alpha, (a0 * a0 + b0 * b0).sqrt(), radius);
        let alpha1 = fade(style.major_alpha, (a1 * a1 + b1 * b1).sqrt(), radius);
        lines.push(vertex(spec.workplane.point(a0, b0), ink(style, alpha0)));
        lines.push(vertex(spec.workplane.point(a1, b1), ink(style, alpha1)));
        t = next;
    }
}

/// `α(d) = max(0, α0 · (1 − d / R))`.
fn fade(alpha: f32, distance: f64, radius: f64) -> f32 {
    (f64::from(alpha) * (1.0 - distance / radius)).max(0.0) as f32
}

fn ink(style: &GridStyle, alpha: f32) -> [f32; 4] {
    [style.color[0], style.color[1], style.color[2], alpha]
}

fn vertex(position: Vector3, color: [f32; 4]) -> Vertex {
    Vertex {
        position: [position.x as f32, position.y as f32, position.z as f32],
        normal: LIGHT,
        uv: [0.0, 0.0],
        part_idx: 0,
        color,
    }
}

fn on_major_line(a: f64, b: f64, major: f64) -> bool {
    if major <= 0.0 {
        return false;
    }
    let near = |value: f64| (value / major).round().mul_add(major, -value).abs() < 1e-6;
    near(a) || near(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workplane::WorkplaneKind;
    use ferrocad_graphics::{Projection, Viewport};

    fn camera() -> Camera {
        Camera {
            viewport: Viewport::new([640, 480], 1.0),
            projection: Projection::Perspective {
                fov_y: std::f64::consts::FRAC_PI_3,
                near: 0.1,
                far: 10_000.0,
            },
            ..Camera::identity()
        }
    }

    fn spec(radius: f64) -> GridSpec {
        GridSpec {
            workplane: Workplane::world(),
            minor: 10.0,
            major: 100.0,
            radius,
        }
    }

    /// A view with all three axes off the view direction, so every triad ribbon exists.
    fn tilted_camera() -> Camera {
        let direction = Vector3::new(1.0, 1.0, 1.0).normalize();
        Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                ferrocad_types::Rotation::from_vectors(direction, Vector3::new(0.0, 0.0, 1.0)),
            )
            .to_matrix(),
            ..camera()
        }
    }

    #[test]
    fn the_fade_follows_the_radial_law() {
        assert!((fade(0.15, 0.0, 100.0) - 0.15).abs() < 1e-9);
        assert!((fade(0.15, 50.0, 100.0) - 0.075).abs() < 1e-6);
        assert_eq!(fade(0.15, 100.0, 100.0), 0.0);
        assert_eq!(fade(0.15, 120.0, 100.0), 0.0);
    }

    #[test]
    fn the_world_grid_has_dots_and_subdivided_major_lines() {
        let geometry = build(&spec(100.0), &GridStyle::default(), &camera());
        assert!(!geometry.dots.is_empty(), "the minor dots are generated");
        assert!(!geometry.lines.is_empty(), "the major lines are generated");
        assert_eq!(geometry.lines.len() % 2, 0, "two vertices per segment");
        assert!(
            geometry.lines.len() > 2 * 21,
            "the lines are subdivided, not one segment each: {}",
            geometry.lines.len()
        );

        // Every dot stays inside the disc and fades towards its edge.
        for dot in &geometry.dots {
            let r = (dot.position[0].powi(2) + dot.position[1].powi(2)).sqrt();
            assert!(r <= 100.0 + 1e-3, "a dot escaped the disc: {r}");
            assert!(dot.color[3] > 0.0 && dot.color[3] <= 0.15 + 1e-6);
        }
        // A dot near the origin is stronger than one near the edge.
        let near = geometry
            .dots
            .iter()
            .min_by(|a, b| {
                let da = a.position[0].powi(2) + a.position[1].powi(2);
                let db = b.position[0].powi(2) + b.position[1].powi(2);
                da.total_cmp(&db)
            })
            .unwrap();
        let far = geometry
            .dots
            .iter()
            .max_by(|a, b| {
                let da = a.position[0].powi(2) + a.position[1].powi(2);
                let db = b.position[0].powi(2) + b.position[1].powi(2);
                da.total_cmp(&db)
            })
            .unwrap();
        assert!(near.color[3] > far.color[3], "{near:?} vs {far:?}");
    }

    #[test]
    fn the_triad_is_three_axis_coloured_ribbons() {
        let geometry = build(&spec(100.0), &GridStyle::default(), &tilted_camera());
        assert_eq!(
            geometry.triangles.len(),
            12,
            "three ribbons of four vertices"
        );
        assert_eq!(geometry.triangle_indices.len(), 18, "two triangles each");
        let colors: Vec<[f32; 4]> = geometry.triangles.chunks(4).map(|q| q[0].color).collect();
        assert_eq!(colors, AXIS_COLORS.to_vec());
    }

    #[test]
    fn a_tilted_plane_builds_in_place() {
        let plane = Workplane::new(
            Vector3::new(0.0, 20.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            WorkplaneKind::Datum("XZ".into()),
        )
        .unwrap();
        let geometry = build(
            &GridSpec {
                workplane: plane,
                minor: 10.0,
                major: 100.0,
                radius: 100.0,
            },
            &GridStyle::default(),
            &camera(),
        );
        for dot in &geometry.dots {
            assert!((dot.position[1] - 20.0).abs() < 1e-3, "{:?}", dot.position);
        }
    }

    #[test]
    fn a_zoomed_out_view_drops_the_minor_level() {
        let camera = camera();
        // Identity camera, orthographic? This camera is perspective at the origin; use a
        // near view where 10 mm is plenty of pixels, then a far one where it is not.
        let near = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -300.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera.clone()
        };
        assert_eq!(
            effective_steps(&near, &Workplane::world(), 10.0, 100.0),
            (10.0, 100.0)
        );
        let far = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -30_000.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera
        };
        assert_eq!(
            effective_steps(&far, &Workplane::world(), 10.0, 100.0),
            (100.0, 1000.0)
        );
    }

    #[test]
    fn the_radius_is_clamped_and_follows_the_camera() {
        let radius = GridSpec::radius_for(&camera());
        assert!((RADIUS_RANGE.0..=RADIUS_RANGE.1).contains(&radius));
        let far = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -10_000.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera()
        };
        assert_eq!(GridSpec::radius_for(&far), RADIUS_RANGE.1);
    }
}
