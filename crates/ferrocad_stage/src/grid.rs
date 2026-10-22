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
/// A local workplane's minor spacing (1 mm).
pub const LOCAL_MINOR_STEP: f64 = 1.0;
/// A local workplane's major spacing (10 mm).
pub const LOCAL_MAJOR_STEP: f64 = 10.0;
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
/// The fraction of the grid radius where the radial fade begins. Inside it the grid
/// stays at full alpha; beyond it a smoothstep falloff reaches zero at the radius.
pub const FADE_START: f64 = 0.55;
/// The triad stems' linear colours (`#F87171`, `#4ADE80`, `#38BDF8`) at 65 %.
pub const AXIS_COLORS: [[f32; 4]; 3] = [
    [0.939, 0.165, 0.165, 0.65],
    [0.069, 0.730, 0.216, 0.65],
    [0.040, 0.509, 0.939, 0.65],
];

/// The direction the fragment shader's light comes from, normalized. Emitting it as the
/// vertex normal makes the diffuse term `1`, so the grid, the triad and the snap
/// feedback render unlit — their per-vertex colour is exactly what reaches the screen.
pub(crate) const LIGHT: [f32; 3] = [0.3714, 0.7428, 0.5571];

/// Parse a `#rrggbb` or `#rgb` sRGB hex colour into **linear** RGBA with alpha `1`.
///
/// The producer bakes linear ink into its vertices, so a caller-supplied colour has to
/// cross the same sRGB transfer function the tokens were authored through: `#94a3b8`
/// parses to [`GRID_COLOR`]. Surrounding whitespace is ignored; the hex digits are
/// case-insensitive. A malformed colour is an error, never a silent fallback.
pub fn parse_srgb_hex(text: &str) -> Result<[f32; 4], String> {
    let text = text.trim();
    let digits = text
        .strip_prefix('#')
        .ok_or_else(|| format!("a grid colour must start with '#', got {text:?}"))?;
    let nibbles: Vec<u8> = digits
        .chars()
        .map(|c| {
            c.to_digit(16)
                .map(|digit| digit as u8)
                .ok_or_else(|| format!("a grid colour has a non-hex digit {c:?} in {text:?}"))
        })
        .collect::<Result<_, _>>()?;
    let [r, g, b] = match nibbles.as_slice() {
        [r, g, b] => [r * 17, g * 17, b * 17],
        [r1, r2, g1, g2, b1, b2] => [r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2],
        _ => {
            return Err(format!(
                "a grid colour must be #rgb or #rrggbb, got {text:?}"
            ));
        }
    };
    let linear = |byte: u8| {
        let value = f32::from(byte) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    Ok([linear(r), linear(g), linear(b), 1.0])
}

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
    /// A global alpha multiplier for the ground. The world grid recedes to
    /// [`WORLD_DIM`] while a local workplane is active.
    pub dim: f32,
    /// The triad's own alpha multiplier; the world triad recedes to
    /// [`WORLD_TRIAD_DIM`], a little stronger than the ground so the global anchor
    /// never vanishes.
    pub triad_dim: f32,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            color: GRID_COLOR,
            minor_alpha: GRID_ALPHA,
            major_alpha: GRID_ALPHA,
            dim: 1.0,
            triad_dim: 1.0,
        }
    }
}

impl GridStyle {
    /// The same ink, scaled back so a lower level of the hierarchy recedes.
    pub fn dimmed(mut self, factor: f32) -> Self {
        self.dim = factor;
        self.triad_dim = factor;
        self
    }

    /// A workplane grid: 1 mm minor dots, 10 mm major lines, the major level twice as
    /// strong as the minor one.
    pub fn workplane() -> Self {
        Self {
            color: GRID_COLOR,
            minor_alpha: GRID_ALPHA,
            major_alpha: GRID_ALPHA * 2.0,
            dim: 1.0,
            triad_dim: 1.0,
        }
    }
}

/// The world grid's alpha multiplier under an active local workplane.
pub const WORLD_DIM: f32 = 0.25;
/// The world triad's alpha multiplier under an active local workplane.
pub const WORLD_TRIAD_DIM: f32 = 0.30;

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
    let (triangles, triangle_indices) = triad(&spec.workplane, camera, style.triad_dim);
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
                let alpha = fade(style.minor_alpha * style.dim, distance, radius);
                if alpha <= 0.0 {
                    continue;
                }
                dots.push(vertex(spec.workplane.point(a, b), ink(style, alpha)));
            }
        }
    }

    // Major lines, in both directions, subdivided so the smoothstep fade band is
    // sampled rather than interpolated across one long segment.
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
///
/// `dim` scales every stem's alpha, so a lower level of the hierarchy can recede.
pub fn triad(workplane: &Workplane, camera: &Camera, dim: f32) -> (Vec<Vertex>, Vec<u32>) {
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
    for (axis, mut color) in axes {
        color[3] *= dim;
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

/// A key for the ground cache: the spec's numbers, its plane, and the ink.
///
/// The ink is part of the key because the vertices bake alpha per dot and per line
/// segment: a dimmed world grid is different geometry, not a different packet colour.
pub fn ground_key(spec: &GridSpec, style: &GridStyle) -> u64 {
    let plane = &spec.workplane;
    let mut parts = vec![
        spec.minor.to_bits(),
        spec.major.to_bits(),
        spec.radius.to_bits(),
        u64::from(style.color[0].to_bits()),
        u64::from(style.color[1].to_bits()),
        u64::from(style.color[2].to_bits()),
        u64::from(style.color[3].to_bits()),
        u64::from(style.minor_alpha.to_bits()),
        u64::from(style.major_alpha.to_bits()),
        u64::from(style.dim.to_bits()),
    ];
    for v in [plane.origin, plane.u, plane.v, plane.n] {
        parts.extend_from_slice(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]);
    }
    hash(&parts)
}

/// A key for the triad cache: the plane, the camera's view matrix, and the ink's alpha
/// multiplier (a dimmed triad bakes different colours into its vertices).
pub fn triad_key(workplane: &Workplane, camera: &Camera, dim: f32) -> u64 {
    let mut parts = Vec::with_capacity(24 + 16);
    parts.push(u64::from(dim.to_bits()));
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

pub(crate) fn hash(parts: &[u64]) -> u64 {
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
        let alpha0 = fade(
            style.major_alpha * style.dim,
            (a0 * a0 + b0 * b0).sqrt(),
            radius,
        );
        let alpha1 = fade(
            style.major_alpha * style.dim,
            (a1 * a1 + b1 * b1).sqrt(),
            radius,
        );
        lines.push(vertex(spec.workplane.point(a0, b0), ink(style, alpha0)));
        lines.push(vertex(spec.workplane.point(a1, b1), ink(style, alpha1)));
        t = next;
    }
}

/// The radial fade: full `alpha` inside `FADE_START · radius`, falling smoothly to zero
/// at `radius` (the `smoothstep(R_max, R_inner, r)` law).
fn fade(alpha: f32, distance: f64, radius: f64) -> f32 {
    let inner = FADE_START * radius;
    if distance <= inner {
        return alpha;
    }
    if distance >= radius {
        return 0.0;
    }
    let t = ((distance - inner) / (radius - inner)).clamp(0.0, 1.0);
    let smooth = t * t * (3.0 - 2.0 * t);
    (f64::from(alpha) * (1.0 - smooth)) as f32
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
    fn a_hex_colour_parses_to_the_structural_ink() {
        // The token is linear, so the parsed value has to be linear too.
        let ink = parse_srgb_hex("#94a3b8").expect("the token's hex");
        for (parsed, token) in ink.iter().zip(GRID_COLOR) {
            assert!((parsed - token).abs() < 1e-3, "{parsed} is not {token}");
        }
        assert_eq!(ink[3], 1.0, "a grid colour is opaque");

        // Case-insensitive, and the six-digit and three-digit forms agree.
        assert_eq!(parse_srgb_hex("#94A3B8").unwrap(), ink);
        assert_eq!(
            parse_srgb_hex("#f00").unwrap(),
            parse_srgb_hex("#FF0000").unwrap()
        );
        assert_eq!(parse_srgb_hex("#fff").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(parse_srgb_hex("#000").unwrap(), [0.0, 0.0, 0.0, 1.0]);
        // Leading and trailing whitespace is harmless.
        assert_eq!(parse_srgb_hex(" #00ff00 ").unwrap(), [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn a_malformed_colour_is_rejected() {
        for bad in [
            "",
            "#",
            "#12",
            "#1234",
            "#1234567",
            "123456",
            "#ggg",
            "#12345g",
            "#12 34 56",
            "#+12345",
        ] {
            assert!(parse_srgb_hex(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn the_fade_follows_the_smoothstep_law() {
        let radius = 100.0;
        let inner = FADE_START * radius;
        // Full alpha in the centre and out to the start of the band.
        assert_eq!(fade(0.15, 0.0, radius), 0.15);
        assert_eq!(fade(0.15, inner, radius), 0.15);
        // Zero at the rim and beyond.
        assert_eq!(fade(0.15, radius, radius), 0.0);
        assert_eq!(fade(0.15, radius + 20.0, radius), 0.0);
        // smoothstep(0.5) = 0.5, so the band's midpoint is half the alpha.
        let midpoint = (inner + radius) * 0.5;
        assert!((fade(0.15, midpoint, radius) - 0.075).abs() < 1e-6);
        // Monotonic non-increasing across the whole radius.
        let mut previous = f64::MAX;
        for step in 0..=120 {
            let alpha = f64::from(fade(0.15, f64::from(step), radius));
            assert!(alpha <= previous + 1e-9, "rose at d = {step}");
            previous = alpha;
        }
    }

    #[test]
    fn a_dimmed_style_scales_the_ground_and_the_triad() {
        let full = GridStyle::default();
        let dimmed = full.clone().dimmed(WORLD_DIM);
        let (bright, _) = ground(&spec(100.0), &full);
        let (dim, _) = ground(&spec(100.0), &dimmed);
        let peak = |dots: &[Vertex]| dots.iter().map(|dot| dot.color[3]).fold(0.0, f32::max);
        assert!((peak(&dim) - peak(&bright) * WORLD_DIM).abs() < 1e-6);

        let (triad_full, _) = triad(&Workplane::world(), &tilted_camera(), 1.0);
        let (triad_dim, _) = triad(&Workplane::world(), &tilted_camera(), WORLD_TRIAD_DIM);
        assert!((triad_full[0].color[3] - AXIS_COLORS[0][3]).abs() < 1e-9);
        assert!((triad_dim[0].color[3] - AXIS_COLORS[0][3] * WORLD_TRIAD_DIM).abs() < 1e-6);
    }

    #[test]
    fn a_workplane_grid_has_finer_steps_and_a_stronger_major() {
        let style = GridStyle::workplane();
        assert!(style.major_alpha > style.minor_alpha * 2.0 - 1e-6);
        assert_eq!(style.dim, 1.0);
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
