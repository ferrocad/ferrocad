//! The ground grid and the world origin triad (G2).
//!
//! A grid is a **stage producer**, not a second renderer: [`build`] returns vertices
//! for three packets (dots, major lines, the triad) that the viewport appends to the
//! same queue as the document. Each vertex carries its own colour, which is what makes
//! the radial alpha fade possible.
//!
//! The producer is pure arithmetic: no kernel, no gpui, no Python. The viewport owns
//! the mesh identities, the projection culling and the depth ordering.

use ferrocad_graphics::{Camera, Vertex};
use ferrocad_types::Vector3;

use crate::axis::{self, AxisStyle};
use crate::screen::project_device;
use crate::workplane::Workplane;

/// The default minor spacing (10 mm).
pub const MINOR_STEP: f64 = 10.0;
/// The default major spacing (100 mm).
pub const MAJOR_STEP: f64 = 100.0;
/// A local workplane's minor spacing (1 mm). The viewport derives a local grid's steps
/// from its extent now; this remains for hosts that draw a fixed-step local grid.
pub const LOCAL_MINOR_STEP: f64 = 1.0;
/// A local workplane's major spacing (10 mm); also the fallback step for a degenerate
/// extent.
pub const LOCAL_MAJOR_STEP: f64 = 10.0;
/// The radius clamp (`1 mm .. 5 m`, in millimetres).
///
/// The lower bound used to be **500 mm**, which for a small model was pure off-screen
/// geometry: a Toy-sized part sits in a view a few tens of millimetres across, so the
/// disc spent its whole density budget (`MINOR_LINE_COLUMNS` / the host's
/// `DOT_COLUMNS`) on a patch nobody could see. Through that budget the floor also
/// coarsened the finest drawable minor step by up to 12× — a configured `minor = 1 mm`
/// was silently drawn at 5 mm. The visible frame reaches about `1.2 ×` the eye distance
/// and the radial fade starts at `0.55 × radius`, so `radius = 2.5 × eye` already keeps
/// the fade off-screen; no floor is needed for that.
pub const RADIUS_RANGE: (f64, f64) = (1.0, 5_000.0);
/// How many minor columns the **line** form may draw across its radius.
///
/// A line lattice costs one row per column (plus a bounded number of samples per row, see
/// [`FADE_SEGMENTS`]), so a fine configured step is affordable; the lattice is **clipped**
/// to this many columns rather than refused or coarsened. 512 covers the visible frame for
/// any aspect ratio while a step is still resolvable (`MINOR_PIXELS`), so in practice the
/// clip is never visible: it only keeps a zoomed-out fine step from becoming a wall.
const MINOR_LINE_COLUMNS: f64 = 512.0;

/// How many minor columns the **dot** form may draw across its radius.
///
/// A dot field costs one vertex per intersection, so it is capped hard: about 40 k dots is
/// what `grid::ground` will emit. Same rule as the line form — the level is clipped, never
/// coarsened, so the configured pitch is always what is drawn.
pub const DOT_COLUMNS: f64 = 112.0;

/// How many samples one line may take across its length.
///
/// The alpha along a line is a smooth radial fade, so a handful of samples per line is
/// plenty; emitting one vertex per grid cell instead made the generator quadratic in the
/// column count, which starved the minor level's density budget.
const FADE_SEGMENTS: f64 = 24.0;

/// Linear `#94A3B8` (the visual language's structural ink).
pub const GRID_COLOR: [f32; 4] = [0.296, 0.366, 0.480, 1.0];
/// The world grid's alpha at the origin.
pub const GRID_ALPHA: f32 = 0.15;
/// The fraction of the grid radius where the radial fade begins. Inside it the grid
/// stays at full alpha; beyond it a smoothstep falloff reaches zero at the radius.
pub const FADE_START: f64 = 0.55;

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

    /// The radius the spec suggests for `camera`: `2.5 ×` the eye distance, capped at
    /// [`RADIUS_RANGE`].
    ///
    /// `2.5 ×` keeps the radial fade (`FADE_START × radius`) outside a 16:9 frame, whose
    /// corners reach about `1.2 ×` the eye distance. The floor is deliberately tiny: a
    /// disc much larger than the view costs geometry and, through the density budget on
    /// the minor level, coarsens the finest step that can be drawn.
    pub fn radius_for(camera: &Camera) -> f64 {
        let eye = crate::screen::camera_position(&camera.view).length();
        (2.5 * eye).clamp(RADIUS_RANGE.0, RADIUS_RANGE.1)
    }
}

/// Which form the minor level takes.
///
/// The native grid and the Coin shader both draw **dots** at the minor intersections by
/// default: a dot is the product of the two axes' line coverage, and it reads as the fine
/// level under the major lattice instead of a dense cage of lines. [`Lines`](Self::Lines)
/// draws a full lattice at the minor step, which is the graph-paper look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MinorStyle {
    /// A dot at each minor intersection (the default).
    #[default]
    Dots,
    /// A line along each minor row and column.
    Lines,
}

impl MinorStyle {
    /// The wire form: `"dots"` or `"lines"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dots => "dots",
            Self::Lines => "lines",
        }
    }

    /// Parse the wire form (case-insensitive); anything else is an error.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "dots" => Ok(Self::Dots),
            "lines" => Ok(Self::Lines),
            other => Err(format!(
                "a minor grid style must be \"dots\" or \"lines\", got {other:?}"
            )),
        }
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
    /// The minor level's form (`FerroCAD.Grid.minor_style`).
    pub minor_style: MinorStyle,
    /// The major lines' stroke width **in the plane's units**; `0` is a hairline (the
    /// renderer's 1 px line).
    ///
    /// The stroke lies in the plane rather than facing the camera: that is what keeps the
    /// ground's geometry independent of the camera's *orientation*, so an orbit does not
    /// rebuild it. It foreshortens with the plane, exactly like the cells it grids — and
    /// the Coin shader measures the same in-plane distance, so the two backends agree.
    pub major_width: f64,
    /// The minor lattice's stroke width in the plane's units; `0` is a hairline. Used
    /// when [`minor_style`](Self::minor_style) is [`MinorStyle::Lines`].
    pub minor_width: f64,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            color: GRID_COLOR,
            minor_alpha: GRID_ALPHA,
            major_alpha: GRID_ALPHA,
            dim: 1.0,
            triad_dim: 1.0,
            minor_style: MinorStyle::Dots,
            major_width: 0.0,
            minor_width: 0.0,
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
            minor_style: MinorStyle::Dots,
            major_width: 0.0,
            minor_width: 0.0,
        }
    }
}

/// The world grid's alpha multiplier under an active local workplane.
pub const WORLD_DIM: f32 = 0.25;
/// The world triad's alpha multiplier under an active local workplane.
pub const WORLD_TRIAD_DIM: f32 = 0.30;

/// The thinnest grid stroke, in device pixels: the renderer's own hairline. A width at
/// this value draws the cheap `LineList` path and no quads at all.
pub const MIN_STROKE_PIXELS: f64 = 1.0;
/// The thickest grid stroke, in device pixels. Past this a line is wider than a cell on a
/// small patch and the grid reads as a fill.
pub const MAX_STROKE_PIXELS: f64 = 8.0;

/// The steps that survive the screen-space cull.
///
/// Below `MINOR_PIXELS` of projected spacing a level cannot be resolved: the **minor**
/// (dot) level is coarsened, and the **major** (line) level moves a decade, so a far view
/// does not turn into moiré.
pub const MINOR_PIXELS: f64 = 4.0;

/// How many decades the major level may climb before the cull gives up.
///
/// One decade is enough when the step comes from the same scale as the view (the
/// configured world steps). An **adaptive** step comes from the extent a layer covers —
/// the ground disc or a face — which can be several decades smaller than the view when it
/// is zoomed out, so the lattice climbs until its lines are resolvable rather than
/// assuming one decade fixes it.
const MAX_PROMOTIONS: usize = 12;

/// The effective `(minor, major)` steps for `camera` on `workplane`.
///
/// The major level is the visible lattice, so its step only changes when the major
/// *itself* cannot be resolved. The **minor** level is coarsened up the 1-2-5 ladder until
/// it draws, and collapses onto the major only when even the coarsest step below it is
/// unreadable. Promoting *both* levels made the whole lattice jump a decade as the camera
/// tilted, because tilting compresses the plane and crosses the threshold; coarsening the
/// minor alone never moves a major line, so the visible lattice stays put.
///
/// Collapsing the minor onto the major immediately (the older rule) is what made
/// `minor_style = lines` appear to do nothing: a lattice at the major step draws no minor
/// rows at all, because every one of them *is* a major row. Adaptive spacing puts the
/// minor near the cull threshold by construction (~a tenth of the major, which is ~a tenth
/// of the patch), so that collapse used to fire on any face seen at a normal zoom.
///
/// The major climbs as many decades as it needs (see [`MAX_PROMOTIONS`]). It is a
/// property of the view, not of the plane: a step that is fine relative to a 10 mm part
/// but sub-pixel from a metre away is promoted until it draws, and a step that is
/// already resolved never moves.
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
    let resolved = |step: f64| spacing(step).is_some_and(|pixels| pixels >= MINOR_PIXELS);
    let mut major = major.max(minor);
    for _ in 0..MAX_PROMOTIONS {
        if resolved(major) {
            break;
        }
        major *= 10.0;
    }
    let mut minor = minor.max(1e-9).min(major);
    while minor < major {
        if resolved(minor) {
            break;
        }
        minor = coarser_step(minor).min(major);
    }
    (minor, major)
}

/// The next step up the 1-2-5 ladder: the smallest `1`, `2` or `5` × a decade that is
/// strictly larger than `step`.
///
/// Coarsening walks this ladder rather than doubling, so the minor level stays on the same
/// kind of grid the rest of the generator uses (and `major/10 → major/5 → major/2 →
/// major` is reachable in three steps).
fn coarser_step(step: f64) -> f64 {
    if !(step.is_finite() && step > 0.0) {
        return step;
    }
    let decade = 10f64.powf(step.log10().floor());
    if !(decade.is_finite() && decade > 0.0) {
        return step;
    }
    let scaled = step / decade;
    let mantissa = if scaled < 2.0 - 1e-9 {
        2.0
    } else if scaled < 5.0 - 1e-9 {
        5.0
    } else {
        10.0
    };
    let next = mantissa * decade;
    if next > step {
        next
    } else {
        step * 2.0
    }
}

/// The vertices for the grid's packets, in draw order.
///
/// The minor level is either **dots** (`Topology::Points`) or a lattice of lines; a line
/// level is a `LineList` at a hairline width, or an in-plane triangle stripe when the style
/// asks for a wide stroke (then `*_indices` is non-empty and the packet asks for the
/// renderer's blended scene-triangle variant).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridGeometry {
    /// `Topology::Points` — the minor level with [`MinorStyle::Dots`].
    pub dots: Vec<Vertex>,
    /// The dots as triangles (a wide dot); empty for a 1 px point.
    pub dot_indices: Vec<u32>,
    /// The minor lattice as a hairline `LineList` ([`MinorStyle::Lines`]).
    pub minor_lines: Vec<Vertex>,
    /// The minor lattice as triangles (a wide stroke); empty for a hairline.
    pub minor_indices: Vec<u32>,
    /// The major lines as a hairline `LineList`; empty when the stroke is wide.
    pub lines: Vec<Vertex>,
    /// The major lines as triangles (a wide stroke); empty for a hairline.
    pub line_indices: Vec<u32>,
    /// `Topology::Triangles` (the triad ribbons).
    pub triangles: Vec<Vertex>,
    pub triangle_indices: Vec<u32>,
}

/// The ground's two levels, as [geometry](GridGeometry) without the triad.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ground {
    pub dots: Vec<Vertex>,
    pub dot_indices: Vec<u32>,
    pub minor_lines: Vec<Vertex>,
    pub minor_indices: Vec<u32>,
    pub lines: Vec<Vertex>,
    pub line_indices: Vec<u32>,
}

/// The radius the **minor** level covers: the disc, clipped to what its form can afford.
///
/// A dot field costs one vertex per intersection and a line lattice one row per column, so
/// a fine step cannot cover an arbitrarily large disc. Clipping the *extent* keeps the
/// configured **pitch** exact: the level fades out where it would get too dense, instead
/// of coarsening the step (which made the subdivisions change as the camera zoomed out) or
/// dropping the level (which made `minor_style` look broken). In practice the clip is
/// never visible — it only bites once the step is far smaller than `MINOR_PIXELS` — it just
/// keeps a zoomed-out fine step from becoming a wall of geometry.
pub fn minor_radius(minor: f64, radius: f64, style: MinorStyle) -> f64 {
    let columns = match style {
        MinorStyle::Lines => MINOR_LINE_COLUMNS,
        MinorStyle::Dots => DOT_COLUMNS,
    };
    radius.min(minor.max(1e-9) * columns)
}

/// Build the grid for `spec`, projected with `camera`.
///
/// The dots and lines carry the radial fade in their alpha; the triad uses the default
/// [`AxisStyle`] scaled by the grid style's `triad_dim` (the world triad recedes under a
/// local frame). A host that styles the axis draws it with [`axis::triad`] instead.
pub fn build(spec: &GridSpec, style: &GridStyle, camera: &Camera) -> GridGeometry {
    let ground = ground(spec, style);
    let axis = AxisStyle::default().scaled_opacity(style.triad_dim);
    let (triangles, triangle_indices) = axis::triad(&spec.workplane, camera, &axis);
    GridGeometry {
        dots: ground.dots,
        dot_indices: ground.dot_indices,
        minor_lines: ground.minor_lines,
        minor_indices: ground.minor_indices,
        lines: ground.lines,
        line_indices: ground.line_indices,
        triangles,
        triangle_indices,
    }
}

/// The ground vertices (the minor level, then the major lattice) for `spec`.
///
/// This half depends only on the spec and the style, so a camera orbit does not
/// rebuild it; the viewport keys its cache on the spec. The stroke widths are therefore
/// **in-plane** ([`GridStyle::major_width`]): a camera-facing ribbon would have to be
/// rebuilt on every orbit.
pub fn ground(spec: &GridSpec, style: &GridStyle) -> Ground {
    let radius = spec.radius.max(1e-6);
    let minor = spec.minor.max(1e-9);
    let major = spec.major.max(minor);
    let mut ground = Ground::default();

    // The subdivision for the fade band. `add_line` caps its samples per line, so this
    // only has to keep a very fine configured lattice from asking for more cells than the
    // fade can resolve.
    let step = if radius / minor > 2_000.0 { major } else { minor };

    // The minor level is **clipped** to what its form can afford — never coarsened and
    // never dropped — so the configured pitch is always what is drawn, over as much of the
    // disc as a dot field or a line lattice can cover. See [`minor_radius`].
    let rm = minor_radius(minor, radius, style.minor_style);
    let columns = rm / minor;
    if style.minor_style != MinorStyle::Lines {
        // Dots, skipping the major lines (the lines own those pixels). A wide dot is an
        // in-plane square (four vertices, six indices) instead of a 1 px point, so the
        // Coin shader's covered dot and the native one are the same size.
        let count = columns.ceil() as i64;
        let half = if style.minor_width.is_finite() && style.minor_width > 0.0 {
            style.minor_width * 0.5
        } else {
            0.0
        };
        for ix in -count..=count {
            let a = ix as f64 * minor;
            for iy in -count..=count {
                let b = iy as f64 * minor;
                if on_major_line(a, b, major) {
                    continue;
                }
                let distance = (a * a + b * b).sqrt();
                if distance > rm {
                    continue;
                }
                let alpha = fade(style.minor_alpha * style.dim, distance, rm);
                if alpha <= 0.0 {
                    continue;
                }
                if half > 0.0 {
                    let base = ground.dots.len() as u32;
                    for (u, v) in [
                        (a - half, b - half),
                        (a + half, b - half),
                        (a + half, b + half),
                        (a - half, b + half),
                    ] {
                        ground
                            .dots
                            .push(vertex(spec.workplane.point(u, v), ink(style, alpha)));
                    }
                    ground.dot_indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                } else {
                    ground
                        .dots
                        .push(vertex(spec.workplane.point(a, b), ink(style, alpha)));
                }
            }
        }
    } else {
        // The lattice at the minor step, skipping the rows and columns the major lattice
        // already draws (a doubled line would read twice as dark).
        let count = columns.ceil() as i64;
        for k in -count..=count {
            let offset = k as f64 * minor;
            if on_major(offset, major) {
                continue;
            }
            let half = (rm * rm - offset * offset).sqrt();
            if half <= 0.0 {
                continue;
            }
            add_line(
                &mut ground.minor_lines,
                &mut ground.minor_indices,
                spec,
                style,
                style.minor_alpha * style.dim,
                offset,
                true,
                half,
                step,
                rm,
                style.minor_width,
            );
            add_line(
                &mut ground.minor_lines,
                &mut ground.minor_indices,
                spec,
                style,
                style.minor_alpha * style.dim,
                offset,
                false,
                half,
                step,
                rm,
                style.minor_width,
            );
        }
    }

    // Major lines, in both directions, subdivided so the smoothstep fade band is
    // sampled rather than interpolated across one long segment.
    let lines_count = (radius / major).ceil() as i64;
    for k in -lines_count..=lines_count {
        let offset = k as f64 * major;
        let half = (radius * radius - offset * offset).sqrt();
        if half <= 0.0 {
            continue;
        }
        add_line(
            &mut ground.lines,
            &mut ground.line_indices,
            spec,
            style,
            style.major_alpha * style.dim,
            offset,
            true,
            half,
            step,
            radius,
            style.major_width,
        );
        add_line(
            &mut ground.lines,
            &mut ground.line_indices,
            spec,
            style,
            style.major_alpha * style.dim,
            offset,
            false,
            half,
            step,
            radius,
            style.major_width,
        );
    }

    ground
}

/// A key for the ground cache: the spec's numbers, its plane, and the ink.
///
/// The ink is part of the key because the vertices bake alpha per dot and per line
/// segment: a dimmed world grid is different geometry, not a different packet colour. The
/// minor form and the stroke widths are geometry too, so they are hashed with it.
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
        u64::from(style.minor_style == MinorStyle::Lines),
        style.major_width.to_bits(),
        style.minor_width.to_bits(),
    ];
    for v in [plane.origin, plane.u, plane.v, plane.n] {
        parts.extend_from_slice(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]);
    }
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

/// Add one line along `u` (when `along_u`) or `v`, from `-half` to `+half`.
///
/// The line is a `LineList` at a hairline width (`width <= 0`), or a strip of quads lying
/// **in the plane** when the style asks for a wide stroke: the stripe is `width` across and
/// the per-vertex ink carries the radial fade as usual.
#[allow(clippy::too_many_arguments)]
fn add_line(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    spec: &GridSpec,
    style: &GridStyle,
    alpha: f32,
    offset: f64,
    along_u: bool,
    half: f64,
    step: f64,
    radius: f64,
    width: f64,
) {
    let wide = width.is_finite() && width > 0.0;
    let half_width = width * 0.5;
    // The ink is a **radial** fade, so a line only needs enough samples for the fade band
    // to read as a smooth gradient: one per grid cell is exact but quadratic in the number
    // of columns, which is what used to force a coarse minor step over a large disc. Capping
    // the samples per line makes the generator cost O(columns) instead of O(columns²) — a
    // 1024-column lattice drops from ~47 ms to a few, so the configured step can be
    // honoured much further out before the density budget has to coarsen it.
    let step = step.max(2.0 * half / FADE_SEGMENTS);
    let mut t = -half;
    while t < half - 1e-9 {
        let next = (t + step).min(half);
        if next <= t + 1e-12 {
            break;
        }
        let alpha0 = fade(alpha, (t * t + offset * offset).sqrt(), radius);
        let alpha1 = fade(alpha, (next * next + offset * offset).sqrt(), radius);
        let ink0 = ink(style, alpha0);
        let ink1 = ink(style, alpha1);
        if !wide {
            let (a0, b0) = if along_u { (t, offset) } else { (offset, t) };
            let (a1, b1) = if along_u { (next, offset) } else { (offset, next) };
            vertices.push(vertex(spec.workplane.point(a0, b0), ink0));
            vertices.push(vertex(spec.workplane.point(a1, b1), ink1));
        } else {
            let base = vertices.len() as u32;
            // `RIBBON_UVS` order: one lateral side first (`uv.x = 1`), then the other
            // (`uv.x = 0`), each from `t` to `next`.
            for (along, lateral, uv_x) in [
                (t, -half_width, 1.0),
                (next, -half_width, 1.0),
                (next, half_width, 0.0),
                (t, half_width, 0.0),
            ] {
                let (a, b) = if along_u {
                    (along, offset + lateral)
                } else {
                    (offset + lateral, along)
                };
                let mut point = vertex(
                    spec.workplane.point(a, b),
                    if along == t { ink0 } else { ink1 },
                );
                point.uv = [uv_x, 0.0];
                vertices.push(point);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        t = next;
    }
}

/// The edge-on fade for `plane` as seen by `camera`: `1` face-on, `0` when the view is
/// parallel to the plane.
///
/// It is one value per **frame** (the camera's forward direction), never per pixel: a
/// per-pixel equivalent crosses zero along the plane's own horizon, which paints a soft
/// band straight through the model on a vertical workplane. The Coin shader applies the
/// same law with the same threshold, so the two backends dim alike.
pub fn grazing(camera: &Camera, plane: &Workplane) -> f32 {
    let alignment = crate::screen::camera_forward(camera)
        .dot(&plane.n)
        .abs();
    let t = (alignment / GRAZING_THRESHOLD).clamp(0.0, 1.0);
    (t * t * (3.0 - 2.0 * t)) as f32
}

/// The cosine below which an edge-on plane starts to fade (`grid_overlay.wgsl` uses the
/// same 0.08).
pub const GRAZING_THRESHOLD: f64 = 0.08;

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
    on_major(a, major) || on_major(b, major)
}

/// Whether `value` sits on a major line (a multiple of `major`).
fn on_major(value: f64, major: f64) -> bool {
    if major <= 0.0 {
        return false;
    }
    (value / major).round().mul_add(major, -value).abs() < 1e-6
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
    fn grazing_dims_a_plane_seen_along_its_own_surface() {
        let plane = Workplane::world();
        assert_eq!(grazing(&Camera::identity(), &plane), 1.0, "face-on");

        // The view whose third row is world `+X`: the camera looks along `−X`, which
        // lies in the world `XY` plane.
        let edge_on = Camera {
            view: ferrocad_types::Matrix4 {
                m: [
                    0.0, 1.0, 0.0, 0.0, // right = +Y
                    0.0, 0.0, 1.0, 0.0, // up    = +Z
                    1.0, 0.0, 0.0, 0.0, // back  = +X
                    0.0, 0.0, 0.0, 1.0,
                ],
            },
            ..camera()
        };
        assert_eq!(grazing(&edge_on, &plane), 0.0);
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
    fn the_minor_style_wire_form_round_trips() {
        for style in [MinorStyle::Dots, MinorStyle::Lines] {
            assert_eq!(MinorStyle::parse(style.as_str()), Ok(style));
        }
        assert_eq!(MinorStyle::parse(" LineS "), Ok(MinorStyle::Lines));
        assert!(MinorStyle::parse("dashes").is_err());
        assert_eq!(MinorStyle::default(), MinorStyle::Dots);
        assert_eq!(GridStyle::default().minor_style, MinorStyle::Dots);
    }

    /// `minor_style = lines` draws a lattice at the minor step instead of dots, and it
    /// skips the rows the major lattice already draws (a doubled line reads twice as
    /// dark).
    #[test]
    fn the_minor_level_is_dots_or_a_lattice() {
        let mut spec = spec(10.0);
        spec.minor = 1.0;
        spec.major = 5.0;

        let dots = ground(&spec, &GridStyle::default());
        assert!(!dots.dots.is_empty(), "the default form is dots");
        assert!(dots.minor_lines.is_empty());

        let lines = ground(
            &spec,
            &GridStyle {
                minor_style: MinorStyle::Lines,
                ..GridStyle::default()
            },
        );
        assert!(lines.dots.is_empty(), "the lattice replaces the dots");
        assert!(!lines.minor_lines.is_empty());
        assert!(
            lines.minor_indices.is_empty(),
            "a hairline lattice is a LineList, not triangles"
        );

        // No minor line lies on a major row or column: `major = 5`, `minor = 1`. Every
        // lattice vertex is on a minor-only offset in at least one axis (the line it
        // belongs to), which a major line's vertex never is.
        let minor_only = |value: f64| (value / 5.0).round().mul_add(5.0, -value).abs() >= 1e-6;
        for vertex in &lines.minor_lines {
            let x = f64::from(vertex.position[0]);
            let y = f64::from(vertex.position[1]);
            assert!(
                minor_only(x) || minor_only(y),
                "the minor lattice runs between the major lines: {x}, {y}"
            );
        }
    }

    /// A wide stroke is a triangle quad lying in the plane, with the lateral UV the
    /// ribbon stage feathers from. A hairline stays a `LineList`, so the default geometry
    /// and its cost are unchanged.
    #[test]
    fn a_wide_stroke_is_an_in_plane_quad() {
        let mut spec = spec(10.0);
        spec.minor = 1.0;
        spec.major = 5.0;
        let hairline = ground(&spec, &GridStyle::default());
        assert!(hairline.line_indices.is_empty());
        assert_eq!(hairline.lines.len() % 2, 0, "two vertices per segment");

        let wide = ground(
            &spec,
            &GridStyle {
                major_width: 0.4,
                minor_style: MinorStyle::Lines,
                minor_width: 0.2,
                ..GridStyle::default()
            },
        );
        assert!(!wide.line_indices.is_empty(), "the stroke is triangles");
        assert_eq!(wide.lines.len() % 4, 0, "four corners per segment");
        assert_eq!(wide.line_indices.len(), wide.lines.len() / 4 * 6);
        // The quad straddles its line by `width / 2`, and its UVs are the two sides.
        let quad = &wide.lines[0..4];
        let xs: Vec<f64> = quad.iter().map(|v| f64::from(v.position[0])).collect();
        let ys: Vec<f64> = quad.iter().map(|v| f64::from(v.position[1])).collect();
        let uvs: Vec<f32> = quad.iter().map(|v| v.uv[0]).collect();
        assert_eq!(uvs, vec![1.0, 1.0, 0.0, 0.0]);
        let spread_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let spread_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let across = spread_y.min(spread_x);
        assert!((across - 0.4).abs() < 1e-6, "the stroke width: {across}");

        // The minor lattice takes its own width.
        assert!(!wide.minor_indices.is_empty());
        let minor_quad = &wide.minor_lines[0..4];
        let my: Vec<f64> = minor_quad.iter().map(|v| f64::from(v.position[1])).collect();
        let spread = my.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - my.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx: Vec<f64> = minor_quad.iter().map(|v| f64::from(v.position[0])).collect();
        let spread_x = mx.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - mx.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!((spread.min(spread_x) - 0.2).abs() < 1e-6);
    }

    /// A wide dot is an in-plane square, so the minor width sizes the dots too — the Coin
    /// shader's dot is the product of two stroked axes and grows with the same setting.
    #[test]
    fn a_wide_dot_is_an_in_plane_square() {
        let mut spec = spec(10.0);
        spec.minor = 1.0;
        spec.major = 5.0;
        let points = ground(&spec, &GridStyle::default());
        assert!(points.dot_indices.is_empty(), "a hairline dot is a point");

        let wide = ground(
            &spec,
            &GridStyle {
                minor_width: 0.4,
                ..GridStyle::default()
            },
        );
        assert!(!wide.dots.is_empty());
        assert_eq!(wide.dots.len() % 4, 0, "four corners per dot");
        assert_eq!(wide.dot_indices.len(), wide.dots.len() / 4 * 6);
        let square = &wide.dots[0..4];
        let xs: Vec<f64> = square.iter().map(|v| f64::from(v.position[0])).collect();
        let ys: Vec<f64> = square.iter().map(|v| f64::from(v.position[1])).collect();
        let span = |values: &[f64]| {
            values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
                - values.iter().cloned().fold(f64::INFINITY, f64::min)
        };
        assert!((span(&xs) - 0.4).abs() < 1e-6, "0.4 across: {}", span(&xs));
        assert!((span(&ys) - 0.4).abs() < 1e-6);
        // It is centred on a lattice point (the minor step is 1).
        let centre = (
            xs.iter().sum::<f64>() / 4.0,
            ys.iter().sum::<f64>() / 4.0,
        );
        assert!(centre.0.round().mul_add(1.0, -centre.0).abs() < 1e-6);
        assert!(centre.1.round().mul_add(1.0, -centre.1).abs() < 1e-6);
    }

    /// Both forms **clip** a level that would be too dense, and neither is refused: the
    /// configured pitch is always what is drawn, over as much of the disc as the form can
    /// cover. This is what stops the subdivisions changing as the camera zooms out.
    #[test]
    fn a_dense_minor_level_is_clipped_not_refused() {
        let furthest = |vertices: &[Vertex]| {
            vertices
                .iter()
                .map(|vertex| {
                    (f64::from(vertex.position[0]).powi(2)
                        + f64::from(vertex.position[1]).powi(2))
                    .sqrt()
                })
                .fold(0.0, f64::max)
        };
        let lines = GridStyle {
            minor_style: MinorStyle::Lines,
            ..GridStyle::default()
        };

        // A 1 mm line lattice over a 100 mm disc is 100 columns: the whole disc.
        let mut spec = spec(100.0);
        spec.minor = 1.0;
        let emitted = ground(&spec, &lines);
        assert!(!emitted.minor_lines.is_empty());
        let reach = furthest(&emitted.minor_lines);
        assert!((reach - 100.0).abs() < 1e-3, "the whole disc: {reach}");

        // 1000 columns: clipped to 512 mm, still drawn at 1 mm.
        spec.radius = 1_000.0;
        let emitted = ground(&spec, &lines);
        assert!(!emitted.minor_lines.is_empty(), "still drawn");
        let reach = furthest(&emitted.minor_lines);
        assert!(
            (reach - 1.0 * MINOR_LINE_COLUMNS).abs() < 1e-3,
            "clipped at the line budget: {reach}"
        );

        // The dot form is capped much harder (one vertex per intersection), and it too
        // clips instead of vanishing.
        spec.radius = 100.0;
        spec.minor = 0.25;
        let emitted = ground(&spec, &GridStyle::default());
        assert!(!emitted.dots.is_empty(), "still drawn");
        assert!(emitted.dots.len() < 40_000, "inside the point budget");
        let reach = furthest(&emitted.dots);
        assert!(
            reach <= 0.25 * DOT_COLUMNS + 1e-3 && reach > 0.25 * DOT_COLUMNS - 0.5,
            "clipped at the dot budget: {reach}"
        );
    }

    #[test]
    fn a_dimmed_style_scales_the_ground_and_the_triad() {
        let full = GridStyle::default();
        let dimmed = full.clone().dimmed(WORLD_TRIAD_DIM);
        let bright = ground(&spec(100.0), &full);
        let dim = ground(&spec(100.0), &dimmed);
        let peak = |dots: &[Vertex]| dots.iter().map(|dot| dot.color[3]).fold(0.0, f32::max);
        assert!((peak(&dim.dots) - peak(&bright.dots) * WORLD_TRIAD_DIM).abs() < 1e-6);

        // `build` uses the default axis style, with `triad_dim` scaling its opacity.
        let full = build(&spec(100.0), &GridStyle::default(), &tilted_camera());
        let dim = build(&spec(100.0), &GridStyle::default().dimmed(WORLD_TRIAD_DIM), &tilted_camera());
        let alpha = |geometry: &GridGeometry| geometry.triangles[0].color[3];
        assert!((alpha(&full) - axis::AXIS_COLORS[0][3] * axis::TRIAD_OPACITY).abs() < 1e-6);
        assert!((alpha(&dim) - alpha(&full) * WORLD_TRIAD_DIM).abs() < 1e-6);
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
    fn the_build_triad_is_three_axis_coloured_ribbons() {
        let geometry = build(&spec(100.0), &GridStyle::default(), &tilted_camera());
        assert_eq!(
            geometry.triangles.len(),
            12,
            "three ribbons of four vertices"
        );
        assert_eq!(geometry.triangle_indices.len(), 18, "two triangles each");
        let colors: Vec<[f32; 4]> = geometry.triangles.chunks(4).map(|q| q[0].color).collect();
        for (color, token) in colors.iter().zip(axis::AXIS_COLORS) {
            assert!((color[0] - token[0]).abs() < 1e-9);
            assert!((color[1] - token[1]).abs() < 1e-9);
            assert!((color[2] - token[2]).abs() < 1e-9);
            assert!((color[3] - token[3] * axis::TRIAD_OPACITY).abs() < 1e-9);
        }
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

    /// The minor level is a **level of detail**: when its step gets too small to read it
    /// coarsens up the ladder, while the **major** lattice keeps its step (a compressed or
    /// distant plane must not re-space the lines). Collapsing the minor onto the major
    /// instead used to leave `minor_style = lines` with nothing to draw, because a lattice
    /// at the major step has no minor rows.
    #[test]
    fn a_zoomed_out_view_coarsens_the_minor_level() {
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
        // Between the two: 10 mm is unresolvable but 100 mm is not. The minor climbs the
        // ladder (10 → 20 → 50) until it reads, and the major does not move.
        let middle = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -3_000.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera.clone()
        };
        let (minor, major) = effective_steps(&middle, &Workplane::world(), 10.0, 100.0);
        assert_eq!(major, 100.0, "the lattice keeps its step");
        assert!(minor < major, "a minor level survives: {minor}");
        assert!(
            minor > 10.0,
            "it coarsened to something that draws: {minor}"
        );
        let a = crate::screen::project_device(&middle, Workplane::world().point(0.0, 0.0))
            .expect("the origin projects");
        let b = crate::screen::project_device(&middle, Workplane::world().point(minor, 0.0))
            .expect("the step projects");
        assert!(
            (a[0] - b[0]).hypot(a[1] - b[1]) >= MINOR_PIXELS,
            "the coarsened minor draws"
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
            (500.0, 1000.0),
            "the lattice moved a decade and the minor coarsened to the last step that \
             still draws (the 1-2-5 ladder, not a blind collapse onto the major)"
        );
    }

    #[test]
    fn an_adaptive_step_climbs_until_it_is_resolved() {
        // A scene-derived step is sized for the model, not the view: a 0.5 mm step from a
        // 20 mm part stays sub-pixel from 30 m away. One decade does not fix that, so the
        // lattice keeps climbing until its lines draw.
        let far = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -30_000.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera()
        };
        let (minor, major) = effective_steps(&far, &Workplane::world(), 0.5, 5.0);
        assert!(major > 5.0, "one decade is not enough: {major}");
        assert_eq!(minor, major, "the dot level is hidden with it");

        let a = crate::screen::project_device(&far, Workplane::world().point(0.0, 0.0))
            .expect("the origin projects");
        let b = crate::screen::project_device(&far, Workplane::world().point(major, 0.0))
            .expect("the step projects");
        let pixels = (a[0] - b[0]).hypot(a[1] - b[1]);
        assert!(pixels >= MINOR_PIXELS, "the promoted step draws: {pixels} px");

        // A step that is already coarse enough never moves.
        assert_eq!(
            effective_steps(&far, &Workplane::world(), 1000.0, 10_000.0),
            (1000.0, 10_000.0)
        );
    }

    #[test]
    fn the_dot_field_fits_its_point_budget() {
        // A disc of radius `r` holds about `π · (r/minor)²` dots; the clip keeps it at
        // `DOT_COLUMNS` columns whatever the radius, so the point budget always holds.
        let mut dense = spec(100.0);
        dense.minor = 1.0;
        let emitted = ground(&dense, &GridStyle::default());
        assert!(!emitted.dots.is_empty());
        assert!(emitted.dots.len() < 40_000);

        for radius in [100.0, 1_000.0, 5_000.0] {
            let mut wide = spec(radius);
            wide.minor = 0.5;
            let emitted = ground(&wide, &GridStyle::default());
            assert!(
                emitted.dots.len() < 40_000,
                "{radius}: {} dots",
                emitted.dots.len()
            );
        }
    }

    /// The levels are a **subdivision** apart: at equal steps every lattice row is a major
    /// row, so the minor level is empty whatever `minor_style` says. The facade keeps the
    /// pair a decade apart for exactly this reason.
    #[test]
    fn a_minor_level_equal_to_the_major_draws_nothing() {
        let mut spec = spec(100.0);
        spec.minor = 10.0;
        spec.major = 10.0;
        for style in [MinorStyle::Dots, MinorStyle::Lines] {
            let emitted = ground(
                &spec,
                &GridStyle {
                    minor_style: style,
                    ..GridStyle::default()
                },
            );
            assert!(
                emitted.dots.is_empty() && emitted.minor_lines.is_empty(),
                "{style:?}: a minor row at the major step is a major row"
            );
            assert!(!emitted.lines.is_empty(), "{style:?}: the major lattice draws");
        }
    }

    #[test]
    fn the_radius_follows_the_camera_and_is_only_capped_far_away() {
        // A small model: the view is a few tens of millimetres across, so the disc is
        // too. It used to floor at 500 mm, which was all off-screen geometry — and, via
        // the density budget, it coarsened the finest drawable minor step by up to 12×.
        let near = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -40.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera()
        };
        assert_eq!(GridSpec::radius_for(&near), 100.0, "2.5 × the eye distance");

        let far = Camera {
            view: ferrocad_types::Placement::new(
                Vector3::new(0.0, 0.0, -10_000.0),
                ferrocad_types::Rotation::identity(),
            )
            .to_matrix(),
            ..camera()
        };
        assert_eq!(GridSpec::radius_for(&far), RADIUS_RANGE.1);

        let radius = GridSpec::radius_for(&camera());
        assert!((RADIUS_RANGE.0..=RADIUS_RANGE.1).contains(&radius));
    }
}
