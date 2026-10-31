//! The origin triad's parameterized style and geometry.
//!
//! The triad was originally baked into [`crate::grid`] as a fixed three-colour
//! cross. It is now driven by an [`AxisStyle`]: the colours, the screen-invariant
//! stem length and width, the alpha of the lit pass, the occluded "ghost" pass and
//! the origin/label toggles all live here, so a host can style the axis without
//! touching the ground grid.
//!
//! Like the grid, this is pure arithmetic: no kernel, no gpui, no Python.

use ferrocad_graphics::{Camera, Projection, Vertex, screen_ribbon};
use ferrocad_types::Vector3;

use crate::grid::hash;
use crate::screen::{STROKE_PIXELS, camera_forward, scale_for};
use crate::workplane::Workplane;

/// The default stem length, in device pixels.
pub const TRIAD_PIXELS: f64 = 80.0;

/// The default opacity of the lit stems.
pub const TRIAD_OPACITY: f32 = 0.65;

/// The default alpha multiplier of the occluded (ghost) pass.
pub const TRIAD_GHOST_ALPHA: f32 = 0.10;

/// The default stem colours in **linear** RGBA: X `#F87171`, Y `#4ADE80`,
/// Z `#38BDF8`, each at alpha `1.0` (the style's `opacity` carries the 65 %).
pub const AXIS_COLORS: [[f32; 4]; 3] = [
    [0.939, 0.165, 0.165, 1.0],
    [0.069, 0.730, 0.216, 1.0],
    [0.040, 0.509, 0.939, 1.0],
];

/// The origin triad's appearance.
///
/// The three colours are indexed X/Y/Z in the world frame and U/V/N in a local one.
/// Each colour's alpha multiplies [`opacity`](Self::opacity).
#[derive(Clone, Debug, PartialEq)]
pub struct AxisStyle {
    /// Whether the axis cross is shown at all (the triad, origin node and labels).
    pub enabled: bool,
    /// Whether an authored (shader) backend owns the axis cross (`view.axis.overlay`).
    ///
    /// This is a **visibility claim**, not geometry: when set, the native triad, the
    /// origin node and the labels step aside for the Coin/WGSL backend, exactly as
    /// `ViewState.grid_overlay` hides the native ground. It is not a user setting and
    /// does not enter [`triad_key`].
    pub overlay: bool,
    /// Linear RGBA per stem.
    pub colors: [[f32; 4]; 3],
    /// A multiplier over every stem's alpha, in `0.0..=1.0`.
    pub opacity: f32,
    /// The stem length, in device pixels (screen-invariant).
    pub length: f64,
    /// The stem width, in device pixels.
    pub width: f64,
    /// Whether the origin node is drawn.
    pub origin: bool,
    /// Whether the `X`/`Y`/`Z` (or `U`/`V`/`N`) tags are drawn.
    pub labels: bool,
    /// The alpha multiplier of the occluded stem pass.
    pub ghost_alpha: f32,
}

impl Default for AxisStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay: false,
            colors: AXIS_COLORS,
            opacity: TRIAD_OPACITY,
            length: TRIAD_PIXELS,
            width: STROKE_PIXELS,
            origin: true,
            labels: true,
            ghost_alpha: TRIAD_GHOST_ALPHA,
        }
    }
}

impl AxisStyle {
    /// The same style with [`opacity`](Self::opacity) scaled by `factor` (clamped to
    /// `0..=1`), so the world triad can recede under a local frame.
    pub fn scaled_opacity(mut self, factor: f32) -> Self {
        self.opacity = (self.opacity * factor).clamp(0.0, 1.0);
        self
    }
}

/// The triad's screen-facing ribbons at the workplane origin: three stems.
///
/// The stems are [`style.length`](AxisStyle::length) long and
/// [`style.width`](AxisStyle::width) wide, in device pixels, and each vertex's alpha
/// is `style.colors[i][3] * style.opacity`.
pub fn triad(workplane: &Workplane, camera: &Camera, style: &AxisStyle) -> (Vec<Vertex>, Vec<u32>) {
    triad_axes(workplane, camera, style, 3)
}

/// The in-plane bracket: the `U` and `V` stems only, with no `+N` stem.
///
/// This is the triad for a plane bound to a **face**. The solid already says which side
/// is outside, and the normal direction is the face manipulator's to show (the push/pull
/// stem sits at the same centroid and points the same way), so a third stem there would
/// draw two arrows on one line. A plane with no surface behind it keeps the full triad,
/// because nothing else says which side is `+N`.
pub fn triad_in_plane(
    workplane: &Workplane,
    camera: &Camera,
    style: &AxisStyle,
) -> (Vec<Vertex>, Vec<u32>) {
    triad_axes(workplane, camera, style, 2)
}

fn triad_axes(
    workplane: &Workplane,
    camera: &Camera,
    style: &AxisStyle,
    count: usize,
) -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(12);
    let mut indices = Vec::with_capacity(18);
    let forward = camera_forward(camera);
    let origin = workplane.origin;
    let length = scale_for(camera, origin, style.length);
    let half_width = scale_for(camera, origin, style.width / 2.0);
    let axes = [
        (workplane.u, style.colors[0]),
        (workplane.v, style.colors[1]),
        (workplane.n, style.colors[2]),
    ];
    for (axis, mut color) in axes.into_iter().take(count) {
        color[3] *= style.opacity;
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

/// The occluded pass of `vertices`: the same ribbons at `alpha` times their own alpha.
///
/// The viewport draws this pass behind solid geometry with the depth test inverted, so
/// a stem that runs into a solid reads at the ghost ink instead of disappearing.
pub fn ghost(vertices: &[Vertex], alpha: f32) -> Vec<Vertex> {
    let alpha = if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        0.0
    };
    vertices
        .iter()
        .map(|vertex| {
            let mut vertex = *vertex;
            vertex.color[3] *= alpha;
            vertex
        })
        .collect()
}

/// A key for the triad cache: the plane, the camera's view matrix, and every style
/// field that changes the geometry or the baked colours.
///
/// `enabled`/`overlay`/`origin`/`labels` are visibility flags, not geometry, so they
/// are not part of the key. `ghost_alpha` is, because the ghost pass is cached beside
/// the lit one.
/// A cache key for the triad: the frame, the camera, the ink and the stem count.
///
/// `in_plane` is part of the key because the same frame and ink draw different geometry
/// as a 2-stem bracket and as a 3-stem triad.
pub fn triad_key(
    workplane: &Workplane,
    camera: &Camera,
    style: &AxisStyle,
    in_plane: bool,
) -> u64 {
    let mut parts = Vec::with_capacity(24 + 16 + 16);
    parts.push(u64::from(in_plane));
    for color in style.colors {
        parts.extend(color.iter().map(|value| u64::from(value.to_bits())));
    }
    parts.push(u64::from(style.opacity.to_bits()));
    parts.push(style.length.to_bits());
    parts.push(style.width.to_bits());
    parts.push(u64::from(style.ghost_alpha.to_bits()));
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

fn vertex(position: Vector3, color: [f32; 4]) -> Vertex {
    Vertex {
        position: [position.x as f32, position.y as f32, position.z as f32],
        normal: crate::grid::LIGHT,
        uv: [0.0, 0.0],
        part_idx: 0,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A view with all three axes off the view direction, so every ribbon exists.
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
    fn the_defaults_reproduce_the_legacy_triad() {
        let style = AxisStyle::default();
        assert!(style.enabled);
        assert!(!style.overlay, "the native backend owns the axis by default");
        assert!(style.origin);
        assert!(style.labels);
        assert_eq!(style.opacity, 0.65);
        assert_eq!(style.length, 80.0);
        assert_eq!(style.width, 1.5);
        assert_eq!(style.ghost_alpha, 0.10);
        assert_eq!(style.colors, AXIS_COLORS);
        assert_eq!(style.colors[0][3], 1.0, "opacity carries the alpha");
    }

    #[test]
    fn the_triad_is_three_ribbons_in_the_style_colours() {
        let style = AxisStyle::default();
        let (vertices, indices) = triad(&Workplane::world(), &tilted_camera(), &style);
        assert_eq!(vertices.len(), 12, "three ribbons of four vertices");
        assert_eq!(indices.len(), 18, "two triangles each");
        let colors: Vec<[f32; 4]> = vertices.chunks(4).map(|quad| quad[0].color).collect();
        for (color, token) in colors.iter().zip(AXIS_COLORS) {
            assert!((color[0] - token[0]).abs() < 1e-9);
            assert!((color[1] - token[1]).abs() < 1e-9);
            assert!((color[2] - token[2]).abs() < 1e-9);
            assert!((color[3] - token[3] * 0.65).abs() < 1e-9, "opacity");
        }
    }

    #[test]
    fn a_custom_style_changes_the_colours_the_length_and_the_width() {
        let mut style = AxisStyle::default();
        style.colors[0] = [0.1, 0.2, 0.3, 0.5];
        style.opacity = 0.4;
        style.length = 40.0;
        style.width = 4.0;

        let (vertices, _) = triad(&Workplane::world(), &tilted_camera(), &style);
        assert!((vertices[0].color[0] - 0.1).abs() < 1e-6);
        assert!((vertices[0].color[1] - 0.2).abs() < 1e-6);
        assert!((vertices[0].color[2] - 0.3).abs() < 1e-6);
        assert!((vertices[0].color[3] - 0.2).abs() < 1e-6, "0.5 × 0.4");

        // A shorter stem moves the tip closer to the origin in device space.
        let plane = Workplane::world();
        let long = triad(&plane, &tilted_camera(), &AxisStyle::default());
        let short = triad(&plane, &tilted_camera(), &style);
        let reach = |vertices: &[Vertex]| -> f32 {
            vertices
                .iter()
                .map(|v| v.position[1].abs().max(v.position[0].abs()))
                .fold(0.0, f32::max)
        };
        assert!(reach(&short.0) < reach(&long.0), "40 px is shorter than 80 px");

        // A wider stem pushes the ribbon's side vertices further from the axis.
        let mut wide = AxisStyle::default();
        wide.width = 6.0;
        let (narrow_v, _) = triad(&plane, &tilted_camera(), &AxisStyle::default());
        let (wide_v, _) = triad(&plane, &tilted_camera(), &wide);
        let spread = |vertices: &[Vertex]| -> f32 {
            // The quad is [base+side, tip+side, tip-side, base-side], so 0 and 3 sit on
            // opposite sides of the stem at the same point.
            let a = vertices[0].position;
            let b = vertices[3].position;
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        assert!(spread(&wide_v) > spread(&narrow_v), "a wider ribbon");
    }

    #[test]
    fn the_ghost_scales_the_alpha() {
        let style = AxisStyle::default();
        let (vertices, _) = triad(&Workplane::world(), &tilted_camera(), &style);
        let ghosts = ghost(&vertices, style.ghost_alpha);
        assert_eq!(ghosts.len(), vertices.len());
        assert!((ghosts[0].color[3] - vertices[0].color[3] * 0.10).abs() < 1e-6);
        assert_eq!(ghost(&vertices, f32::NAN)[0].color[3], 0.0);
    }

    #[test]
    fn an_in_plane_bracket_drops_the_normal_stem() {
        let plane = Workplane::world();
        let camera = tilted_camera();
        let style = AxisStyle::default();

        let (full, full_indices) = triad(&plane, &camera, &style);
        let (bracket, bracket_indices) = triad_in_plane(&plane, &camera, &style);
        assert_eq!(full.len(), bracket.len() + 4, "one stem is four vertices");
        assert_eq!(full_indices.len(), bracket_indices.len() + 6);
        // The key must distinguish them: same frame, same ink, different geometry.
        assert_ne!(
            triad_key(&plane, &camera, &style, true),
            triad_key(&plane, &camera, &style, false)
        );
    }

    #[test]
    fn the_key_tracks_the_geometry_and_the_colour_but_not_the_flags() {
        let plane = Workplane::world();
        let camera = tilted_camera();
        let style = AxisStyle::default();
        let key = triad_key(&plane, &camera, &style, false);

        // Visibility flags are not geometry.
        let mut flags = style.clone();
        flags.enabled = false;
        flags.overlay = true;
        flags.origin = false;
        flags.labels = false;
        assert_eq!(triad_key(&plane, &camera, &flags, false), key);

        for mutate in [
            (|s: &mut AxisStyle| s.colors[1][0] += 0.01) as fn(&mut AxisStyle),
            |s: &mut AxisStyle| s.opacity = 0.5,
            |s: &mut AxisStyle| s.length = 40.0,
            |s: &mut AxisStyle| s.width = 3.0,
            |s: &mut AxisStyle| s.ghost_alpha = 0.2,
        ] {
            let mut changed = style.clone();
            mutate(&mut changed);
            assert_ne!(triad_key(&plane, &camera, &changed, false), key);
        }
    }

    #[test]
    fn a_scaled_opacity_clamps() {
        let style = AxisStyle::default().scaled_opacity(0.30);
        assert!((style.opacity - 0.195).abs() < 1e-6);
        assert_eq!(AxisStyle::default().scaled_opacity(100.0).opacity, 1.0);
        assert_eq!(AxisStyle::default().scaled_opacity(-1.0).opacity, 0.0);
    }
}
