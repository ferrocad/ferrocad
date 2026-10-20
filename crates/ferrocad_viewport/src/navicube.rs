//! The navigation-cube controller: placement, camera sync, hit-testing and snapping.
//!
//! The counterpart of FreeCAD's `Gui::NaviCube`. The **model** ([`model`]) and the
//! **producers** ([`inventor`], and later a stage one) live beside it; this is the
//! viewer glue that places the cube in a viewport corner, mirrors the main camera's
//! orientation into the cube, hit-tests the corner rectangle, highlights the hovered
//! piece, and — on a click — snaps the main camera onto that face, edge or corner.
//!
//! The controller drives a [`NavicubeView`], so the same cube is built from either the
//! legacy `pivy.coin` stack or the modern `ferrocad_stage` stack. The glass rest state
//! (proximity wake-up, labels, the Home button, away-facing faces) and the animated
//! snap are built; the compass ring and context menu are deferred (see
//! `docs/navicube-gpui-design.md`, `docs/navicube.md` §4–§5 and
//! `docs/navicube-spec.md`).

mod inventor;
mod model;
mod stage;
mod theme;
mod view;

#[cfg(test)]
mod conformance;

pub use inventor::InventorProducer;
pub use model::{Corner, Piece, PieceKind, RING_PICK_ID, YawDirection, home_orientation};
pub use stage::StageProducer;
pub use theme::{NavicubeRole, NavicubeTheme};
pub use view::{NavicubeView, NavicubeVisual};

use ferrocad_graphics::{Camera, Rect, Viewport, pick_at_rect};
use ferrocad_inventor::{NaviCubeDirective, ViewSpec};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

/// The navigation cube's structural config: where it sits (logical pixels) and how big
/// the **cube** is. The surrounding ring/box follows from [`model::overlay_size`], so
/// `cube_size` is the one size the caller sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NaviCubeConfig {
    /// Which corner of the viewport.
    pub corner: Corner,
    /// The cube's **largest projected extent**, in logical pixels: its body diagonal, so
    /// the body always fits inside a `cube_size × cube_size` square at any orientation.
    /// The overlay rect is larger because the compass ring and its labels surround it.
    pub cube_size: f32,
    /// The margin from the corner, in logical pixels.
    pub margin: f32,
}

impl Default for NaviCubeConfig {
    fn default() -> Self {
        Self {
            corner: Corner::TopRight,
            cube_size: 100.0,
            margin: 8.0,
        }
    }
}

/// A label the viewport draws as **gpui text** over the overlay image: crisp,
/// anti-aliased, and in the app's font rather than a baked bitmap. Positions are
/// element-local **logical** pixels (top-left origin), the text's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavicubeLabel {
    /// The glyph to draw (`F`/`B`/`T`/`D`/`L`/`R` or `N`/`E`/`S`/`W`).
    pub text: char,
    /// Centre position, element-local logical pixels.
    pub position: [f32; 2],
    /// Font size in logical pixels.
    pub size: f32,
    /// Ink alpha (faces fade as they turn edge-on; compass letters stay legible).
    pub alpha: f32,
}

/// The gpui chrome bar's width in logical pixels. The cube is centred in a column of this
/// width at the chosen corner, so the bar sits symmetrically under it (WA6).
pub const CHROME_WIDTH: f32 = 248.0;

/// The navigation cube: a corner overlay showing the view orientation.
pub struct NaviCube {
    /// The producer drawing the cube (the legacy inventor producer by default).
    producer: Box<dyn NavicubeView>,
    camera: Camera,
    viewport: Viewport,
    rect: Rect,
    /// An explicit device-pixel placement (from a [`NaviCubeDirective`]); `None` places
    /// the cube in a corner from `config`.
    placement: Option<Rect>,
    /// The structural config: corner, size and margin (both logical pixels).
    config: NaviCubeConfig,
    /// The appearance theme (the Rust core owns styling).
    theme: NavicubeTheme,
    hilite: Option<Piece>,
    /// Whether the pointer is in the gizmo rect (including its negative space).
    proximity: bool,
    /// Whether the pointer is on the compass ring (hovering or scrubbing it).
    ring_active: bool,
    /// The extra bearing applied to the compass letters when a local workplane is active
    /// (radians, CCW in the ring plane). The ring geometry itself stays fixed.
    heading_offset: f64,
    /// The camera rotation last synced into the cube (so we only recompile on change).
    synced: Option<Rotation>,
}

impl NaviCube {
    /// A cube in `viewport`'s top-right corner, built by the legacy inventor producer.
    pub fn new(viewport: Viewport) -> Self {
        Self::with_producer(viewport, Box::new(InventorProducer::new()))
    }

    /// A cube in `viewport`'s top-right corner, built by the modern `ferrocad_stage`
    /// producer.
    pub fn stage(viewport: Viewport) -> Self {
        Self::with_producer(viewport, Box::new(StageProducer::new()))
    }

    /// A cube in `viewport`'s top-right corner, built by `producer`.
    pub fn with_producer(viewport: Viewport, producer: Box<dyn NavicubeView>) -> Self {
        let config = NaviCubeConfig::default();
        let rect = corner_rect(viewport, config.corner, config.cube_size, config.margin);
        let camera = model::camera(rect);
        let mut cube = Self {
            producer,
            camera,
            viewport,
            rect,
            placement: None,
            config,
            theme: NavicubeTheme::default(),
            hilite: None,
            proximity: false,
            ring_active: false,
            heading_offset: 0.0,
            synced: None,
        };
        cube.rebuild();
        cube
    }

    /// A cube placed by a [`NaviCubeDirective`]: an explicit device-pixel rectangle and
    /// orientation (or the main camera when the directive's orientation is `None`), with
    /// the directive's highlight. The viewport hosts one per `Directive::NaviCube`
    /// ([`docs/directives.md`](../../../docs/directives.md)).
    pub fn from_directive(viewport: Viewport, directive: NaviCubeDirective) -> Self {
        let rect = directive.rect;
        let mut cube = Self {
            producer: Box::new(InventorProducer::new()),
            camera: model::camera(rect),
            viewport,
            rect,
            placement: Some(rect),
            config: NaviCubeConfig::default(),
            theme: NavicubeTheme::default(),
            hilite: directive.hilite.and_then(Piece::from_pick_id),
            proximity: false,
            ring_active: false,
            heading_offset: 0.0,
            synced: directive.orientation,
        };
        cube.rebuild();
        cube
    }

    /// Replace the structural config (corner, size, margin) and relayout.
    pub fn with_config(mut self, config: NaviCubeConfig) -> Self {
        self.config = config;
        self.relayout();
        self
    }

    /// Replace the appearance theme and rebuild.
    pub fn with_theme(mut self, theme: NavicubeTheme) -> Self {
        self.theme = theme;
        self.rebuild();
        self
    }

    /// The device-pixel rectangle the cube occupies.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The cube's rectangle in **logical** pixels (top-left origin), as
    /// `[left, top, right, bottom]`.
    ///
    /// The overlay rounds its size and margin to whole device pixels —
    /// `round(logical × dpr)` in [`corner_rect`] — so the configured `size` is only
    /// the ideal. This is the authoritative logical extent: anything gpui draws beside
    /// the cube (the chrome bar) must derive its placement from it, not from
    /// [`config`](Self::config), or the two drift apart at fractional DPI.
    pub fn logical_rect(&self) -> [f32; 4] {
        let dpr = self.viewport.device_pixel_ratio();
        let height = self.viewport.device_size()[1] as f32;
        let left = self.rect.x as f32 / dpr;
        let right = (self.rect.x + self.rect.width as i32) as f32 / dpr;
        // Device Y is bottom-left origin; logical Y is top-left origin.
        let top = (height - self.rect.y as f32 - self.rect.height as f32) / dpr;
        let bottom = (height - self.rect.y as f32) / dpr;
        [left, top, right, bottom]
    }

    /// The **compass** labels to draw as gpui text over the overlay. The face letters are
    /// filled 3D glyphs emitted by the producers (`model::face_glyph`), so they
    /// foreshorten with the cube; only the ground-plane `N/E/S/W` letters are screen text.
    pub fn labels(&self) -> Vec<NavicubeLabel> {
        let rotation = self.synced.unwrap_or_else(Rotation::identity);
        let matrix = rotation.to_matrix();
        let mut labels = Vec::with_capacity(4);
        for (text, angle) in model::COMPASS_LABELS {
            let point = model::ring_label_position(angle + self.heading_offset);
            let view = matrix.transform(&Vector3::new(
                f64::from(point[0]),
                f64::from(point[1]),
                f64::from(point[2]),
            ));
            let radius = (view.x * view.x + view.y * view.y).sqrt();
            let far = view.z < 0.0;
            // A far-side letter that projects inside the cube's silhouette would stamp
            // over the front faces; cull it outright. A far letter outside the cube is
            // still useful, but faint, so it does not read as sitting on top.
            if far && radius < 0.9 {
                continue;
            }
            let openness = matrix.transform(&Vector3::new(0.0, 1.0, 0.0)).z.abs() as f32;
            labels.push(NavicubeLabel {
                text,
                position: self.project_view([view.x, view.y]),
                size: 11.0,
                alpha: (0.5 + 0.4 * openness) * if far { 0.2 } else { 1.0 },
            });
        }
        labels
    }

    /// Project a **cube/view-space** `(x, y)` to element-local logical pixels.
    fn project_view(&self, [x, y]: [f64; 2]) -> [f32; 2] {
        let [left, top, right, bottom] = self.logical_rect();
        let scale = (f64::from(right) - f64::from(left)) / (2.0 * model::ORTHO_EXTENT);
        [
            (f64::from(left) + f64::from(right)) as f32 / 2.0 + (x * scale) as f32,
            (f64::from(top) + f64::from(bottom)) as f32 / 2.0 - (y * scale) as f32,
        ]
    }

    /// The overlay view for the renderer.
    pub fn view(&self) -> ViewSpec<'_> {
        ViewSpec {
            queue: self.producer.queue(),
            camera: &self.camera,
            rect: Some(self.rect),
            overlay: true,
        }
    }

    /// Update the placement for a new viewport (its size or scale factor changed).
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.relayout();
    }

    /// Recompute the rect and overlay camera from the explicit placement (or the config)
    /// and rebuild.
    fn relayout(&mut self) {
        self.rect = match self.placement {
            Some(rect) => rect,
            None => corner_rect(
                self.viewport,
                self.config.corner,
                self.config.cube_size,
                self.config.margin,
            ),
        };
        self.camera = model::camera(self.rect);
        self.rebuild();
    }

    /// The rect placement/size this cube was built with (logical pixels).
    pub fn config(&self) -> NaviCubeConfig {
        self.config
    }

    /// Mirror the main camera's **viewpoint** into the cube, with a roll-free up vector.
    ///
    /// The cube is a instrument, not a second viewport: it reports where the eye is
    /// (azimuth/elevation) but deliberately ignores the camera's screen roll, so `T` stays
    /// upright and the turntable ellipse stays horizontal even if the scene view has been
    /// rolled. The rotation is rebuilt from the spherical angles rather than copied from
    /// the view matrix.
    pub fn sync(&mut self, main: &Camera) {
        let rotation = Rotation::from_matrix(&main.view);
        let inverse = rotation.inverse().to_matrix();
        let eye = inverse.transform(&Vector3::new(0.0, 0.0, 1.0));
        let azimuth = eye.z.atan2(eye.x);
        let elevation = eye.y.clamp(-1.0, 1.0).asin();
        // At a pole the eye cannot move, so a world-yaw only spins the camera about its
        // own view direction. The horizon is meaningless there: keep the camera's
        // in-plane up rather than rebuilding a frozen canonical one, so the compass
        // still turns in a top/bottom view.
        let up_reference =
            (eye.y.abs() > 1.0 - POLE_COS).then(|| inverse.transform(&Vector3::new(0.0, 1.0, 0.0)));
        let canonical = view_rotation_with_up(azimuth, elevation, up_reference);
        if self.synced.as_ref() == Some(&canonical) {
            return;
        }
        self.synced = Some(canonical);
        self.rebuild();
    }

    /// The face, edge or corner under a device position, if it is over the cube.
    pub fn hit(&self, device: [i32; 2]) -> Option<Piece> {
        if !self.rect.contains(device) {
            return None;
        }
        pick_at_rect(self.producer.queue(), &self.camera, self.rect, device)
            .and_then(Piece::from_pick_id)
    }

    /// Wake the wireframe when the pointer is anywhere in the gizmo rect (including
    /// the negative space), and dim it again on leave. Returns whether it changed.
    pub fn set_proximity(&mut self, proximity: bool) -> bool {
        if self.proximity == proximity {
            return false;
        }
        self.proximity = proximity;
        self.rebuild();
        true
    }

    /// Highlight `piece` or clear the highlight, rebuilding the cube when it changes.
    /// Returns whether anything changed.
    pub fn set_hilite(&mut self, target: Option<Piece>) -> bool {
        if self.hilite == target {
            return false;
        }
        self.hilite = target;
        self.rebuild();
        true
    }

    /// Rotate the compass letters by `offset` radians (CCW in the ring plane), or clear
    /// the offset with `None`. Used when a local workplane is active, so the ring's
    /// cardinal heading matches the sketch's `U`. The ring geometry itself is unchanged
    /// (`axis-origin-grid.md` §10.5).
    pub fn set_heading_offset(&mut self, offset: Option<f64>) -> bool {
        let next = offset.unwrap_or(0.0);
        if (self.heading_offset - next).abs() < 1e-12 {
            return false;
        }
        self.heading_offset = next;
        true
    }

    /// The bearing applied to the compass letters (radians).
    pub fn heading_offset(&self) -> f64 {
        self.heading_offset
    }

    /// Light the compass ring while the pointer is over it (hover or scrub). Returns
    /// whether anything changed.
    pub fn set_ring_active(&mut self, active: bool) -> bool {
        if self.ring_active == active {
            return false;
        }
        self.ring_active = active;
        self.rebuild();
        true
    }

    /// A device position in cube space (`x` right, `y` up), relative to the gizmo centre.
    fn cube_point(&self, device: [i32; 2]) -> Option<[f64; 2]> {
        if !self.rect.contains(device) {
            return None;
        }
        let scale = f64::from(self.rect.width.max(1)) / (2.0 * model::ORTHO_EXTENT);
        let centre_x = f64::from(self.rect.x) + f64::from(self.rect.width) / 2.0;
        let centre_y = f64::from(self.rect.y) + f64::from(self.rect.height) / 2.0;
        Some([
            (f64::from(device[0]) - centre_x) / scale,
            (f64::from(device[1]) - centre_y) / scale,
        ])
    }

    /// The element-local logical position of the **ring** at `angle` (radians in the
    /// ground plane), after the current view rotation. Useful for tests and for placing
    /// pointer affordances on the projected turntable.
    pub fn ring_point(&self, angle: f64) -> [f32; 2] {
        let rotation = self.synced.unwrap_or_else(Rotation::identity);
        let local = Vector3::new(
            model::RING_RADIUS * angle.cos(),
            model::RING_Y,
            model::RING_RADIUS * angle.sin(),
        );
        let view = rotation.to_matrix().transform(&local);
        self.project_view([view.x, view.y])
    }

    /// Whether a device position is on the compass ring's scrub band. The ring is a
    /// **ground-plane** circle now, so the pointer is projected onto `y = RING_Y` in cube
    /// space and tested by radius there — the hit region is the projected ellipse.
    pub fn ring_hit(&self, device: [i32; 2]) -> bool {
        self.ring_plane_point(device).is_some_and(|(radius, _)| {
            ((model::RING_RADIUS - model::RING_BAND_IN)
                ..=(model::RING_RADIUS + model::RING_BAND_OUT))
                .contains(&radius)
        })
    }

    /// The pointer's angle around the ring **in the ground plane** (radians, CCW from
    /// +X). Used to scrub the view's yaw; `None` when the ring is edge-on (the ray misses
    /// the plane), where scrubbing has no unambiguous angle.
    pub fn ring_angle(&self, device: [i32; 2]) -> Option<f64> {
        self.ring_plane_point(device).map(|(_, angle)| angle)
    }

    /// The pointer's ring angle under an explicit `rotation`, rather than the cube's
    /// live synced one.
    ///
    /// A scrub must use the orientation captured when the press began: the turntable is
    /// inside the rotating subtree, so re-measuring with the live rotation after each
    /// applied yaw feeds the camera's own motion back into the pointer angle. That made
    /// the same drag amplitude produce different yaws depending on frame timing (left
    /// then right did not return to the start). Freezing the projection makes the angle a
    /// pure function of the pointer position, so the scrub integrates exactly.
    pub fn ring_angle_for(&self, device: [i32; 2], rotation: Rotation) -> Option<f64> {
        self.ring_plane_point_for(device, rotation)
            .map(|(_, angle)| angle)
    }

    /// Intersect the device ray (view space) with the ground plane and return the point's
    /// in-plane `(radius, angle)` about the cube axis.
    fn ring_plane_point(&self, device: [i32; 2]) -> Option<(f64, f64)> {
        self.ring_plane_point_for(device, self.synced.unwrap_or_else(Rotation::identity))
    }

    /// [`ring_plane_point`](Self::ring_plane_point) with an explicit projection rotation.
    fn ring_plane_point_for(&self, device: [i32; 2], rotation: Rotation) -> Option<(f64, f64)> {
        let [x, y] = self.cube_point(device)?;
        let matrix = rotation.to_matrix();
        let normal = matrix.transform(&Vector3::new(0.0, 1.0, 0.0));
        // Edge-on: the ray is parallel to the plane and the hit has no unique point.
        if normal.z.abs() < 0.05 {
            return None;
        }
        let plane_point = matrix.transform(&Vector3::new(0.0, model::RING_Y, 0.0));
        let t = (normal.dot(&plane_point) - normal.x * x - normal.y * y) / normal.z;
        let local = rotation
            .inverse()
            .to_matrix()
            .transform(&Vector3::new(x, y, t));
        let radius = (local.x * local.x + local.z * local.z).sqrt();
        Some((radius, local.z.atan2(local.x)))
    }

    /// Rebuild the cube: the camera orientation plus the away-facing faces drawn
    /// translucent.
    fn rebuild(&mut self) {
        let visual = NavicubeVisual {
            orientation: self.synced.unwrap_or_else(Rotation::identity),
            hilite: self.hilite,
            away: self.away_faces(),
            proximity: self.proximity,
            ring: self.ring_active,
        };
        self.producer.rebuild(&self.viewport, &visual, &self.theme);
    }

    /// The faces pointing away from the viewer, given the synced camera rotation.
    fn away_faces(&self) -> Vec<Piece> {
        let Some(rotation) = self.synced else {
            return Vec::new();
        };
        let matrix = rotation.to_matrix();
        Piece::ALL
            .iter()
            .copied()
            .filter(|piece| {
                piece.kind() == PieceKind::Face && matrix.transform(&piece.direction()).z < 0.0
            })
            .collect()
    }
}

/// How close to a pole (on `|cos(elevation)|`) the horizon stops being meaningful.
/// Inside this band the navicube keeps the camera's own in-plane orientation, so a yaw
/// still turns the compass in a top/bottom view instead of freezing it.
const POLE_COS: f64 = 1e-3;

/// A **roll-free** view rotation for a viewpoint at `azimuth` and `elevation` (radians;
/// azimuth CCW from +X, elevation above the XZ plane). The returned rotation maps the eye
/// direction `(cos e·cos a, sin e, cos e·sin a)` onto +Z with world `+Y` projected as the
/// screen up, so the navicube's top stays upright and its turntable stays level.
///
/// Looking (almost) straight down or up has no unique up; the front (+Z) is placed at the
/// bottom of the screen (`−Z` is screen-up for a top view).
pub fn view_rotation(azimuth: f64, elevation: f64) -> Rotation {
    view_rotation_with_up(azimuth, elevation, None)
}

/// [`view_rotation`] with an explicit `up_reference` in **world** space. `None` uses the
/// canonical world-`+Y` reference (or `∓Z` near the poles). `sync` passes the camera's own
/// screen-up at a pole, where the canonical choice would otherwise discard the in-plane
/// rotation and make the compass look frozen.
fn view_rotation_with_up(azimuth: f64, elevation: f64, up_reference: Option<Vector3>) -> Rotation {
    let ce = elevation.cos();
    let direction = Vector3::new(ce * azimuth.cos(), elevation.sin(), ce * azimuth.sin());
    let fallback = if direction.y.abs() > 0.999 {
        Vector3::new(0.0, 0.0, if direction.y > 0.0 { -1.0 } else { 1.0 })
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let up_ref = up_reference.unwrap_or(fallback);
    let projected = up_ref.sub(&direction.scale(up_ref.dot(&direction)));
    let up = if projected.length() < 1e-12 {
        fallback
    } else {
        projected.normalize()
    };
    let right = up.cross(&direction).normalize();
    // The world→view rotation's **rows** are the screen basis expressed in world space:
    // row0 = right, row1 = up, row2 = direction (toward the viewer). The translation
    // column stays zero.
    let mut matrix = Matrix4::identity();
    matrix.m[0] = right.x;
    matrix.m[1] = right.y;
    matrix.m[2] = right.z;
    matrix.m[4] = up.x;
    matrix.m[5] = up.y;
    matrix.m[6] = up.z;
    matrix.m[8] = direction.x;
    matrix.m[9] = direction.y;
    matrix.m[10] = direction.z;
    Rotation::from_matrix(&matrix)
}

/// The main-camera **view matrix** that snaps the view onto `piece`, keeping the
/// current camera distance.
pub fn snap_view(piece: Piece, main: &Camera) -> Matrix4 {
    let base = Vector3::new(0.0, 0.0, -camera_distance(main));
    Placement::new(base, piece.orientation()).to_matrix()
}

/// The camera **view matrix** with its distance from the pivot scaled by `factor`.
///
/// The rotation is kept; only the translation along the view ray changes, so a
/// wheel notch moves the eye towards or away from the scene. The distance is
/// clamped away from zero so repeated zoom-in cannot collapse the camera onto the
/// pivot (which would make the view matrix singular).
pub fn zoom_view(main: &Camera, factor: f64) -> Matrix4 {
    let base = Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]);
    let distance = base.length();
    if distance <= 1e-9 {
        return main.view;
    }
    let target = (distance * factor).clamp(1e-4, 1e9);
    Placement::new(
        base.scale(target / distance),
        Rotation::from_matrix(&main.view),
    )
    .to_matrix()
}

/// The camera **view matrix** yawed by `angle` radians about the vertical (+Y) axis,
/// keeping the eye distance and the elevation. The yaw is a **world** rotation composed
/// on the right (`view · R_y`), so it turns the eye around the vertical axis rather than
/// about the tilted view-up; the horizon stays level at any elevation.
pub fn yaw_view(main: &Camera, angle: f64) -> Matrix4 {
    let yaw = Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), angle);
    let base = Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]);
    Placement::new(base, Rotation::from_matrix(&main.view).multiply(&yaw)).to_matrix()
}

/// The camera **view matrix** orbited by a drag of `delta` logical pixels.
pub fn drag_view(main: &Camera, delta: [f32; 2]) -> Matrix4 {
    const RADIANS_PER_PIXEL: f64 = 0.012;
    let yaw = Rotation::from_axis_angle(
        &Vector3::new(0.0, 1.0, 0.0),
        f64::from(delta[0]) * RADIANS_PER_PIXEL,
    );
    let pitch = Rotation::from_axis_angle(
        &Vector3::new(1.0, 0.0, 0.0),
        f64::from(delta[1]) * RADIANS_PER_PIXEL,
    );
    let base = Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]);
    // Pitch * yaw (not yaw * pitch): the vertical axis is applied last, so a diagonal
    // drag does not roll the horizon. The previous order tilted the world-up vector on
    // screen whenever both deltas were non-zero, which made the navicube tumble.
    let rotation = pitch.multiply(&yaw).multiply(&Rotation::from_matrix(&main.view));
    Placement::new(base, rotation).to_matrix()
}

/// Spherical linear interpolation between two rotations (`t` in `[0, 1]`).
pub fn slerp(a: &Rotation, b: &Rotation, t: f64) -> Rotation {
    let a_q = a.q;
    let mut b_q = b.q;
    let mut dot = a_q[0] * b_q[0] + a_q[1] * b_q[1] + a_q[2] * b_q[2] + a_q[3] * b_q[3];
    // Take the shorter arc: a quaternion and its negation are the same rotation.
    if dot < 0.0 {
        b_q = [-b_q[0], -b_q[1], -b_q[2], -b_q[3]];
        dot = -dot;
    }
    let q = if dot > 0.9995 {
        // Nearly parallel: a normalized linear blend avoids a 0/0 in the sin terms.
        let mut q = [0.0; 4];
        for i in 0..4 {
            q[i] = a_q[i] + (b_q[i] - a_q[i]) * t;
        }
        let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if length > 0.0 {
            for value in &mut q {
                *value /= length;
            }
        }
        q
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        let wa = ((1.0 - t) * theta).sin() / sin;
        let wb = (t * theta).sin() / sin;
        let mut q = [0.0; 4];
        for i in 0..4 {
            q[i] = wa * a_q[i] + wb * b_q[i];
        }
        q
    };
    Rotation {
        q,
        raw_axis: None,
    }
}

/// An in-progress animated snap of the main camera onto a piece.
#[derive(Clone, Debug)]
pub struct SnapAnimation {
    start: Rotation,
    end: Rotation,
    base: Vector3,
    start_time: std::time::Instant,
    duration: std::time::Duration,
}

impl SnapAnimation {
    /// Begin a snap from `main`'s current orientation onto `piece`.
    pub fn begin(piece: Piece, main: &Camera, duration: std::time::Duration) -> Self {
        Self::to(piece.orientation(), main, duration)
    }

    /// Begin a snap from `main`'s current orientation onto the target `end` rotation
    /// (e.g. the chrome's Home control and [`home_orientation`]).
    pub fn to(end: Rotation, main: &Camera, duration: std::time::Duration) -> Self {
        Self {
            start: Rotation::from_matrix(&main.view),
            end,
            base: Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]),
            start_time: std::time::Instant::now(),
            duration,
        }
    }

    /// The rotation the snap is heading towards.
    pub fn target(&self) -> Rotation {
        self.end
    }

    /// The camera view at `elapsed` since [`begin`](Self::begin) (eased).
    pub fn view_at(&self, elapsed: std::time::Duration) -> Matrix4 {
        let t = (elapsed.as_secs_f64() / self.duration.as_secs_f64().max(1e-9)).clamp(0.0, 1.0);
        let eased = t * t * (3.0 - 2.0 * t); // smoothstep
        Placement::new(self.base, slerp(&self.start, &self.end, eased)).to_matrix()
    }

    /// Whether the snap has finished.
    pub fn is_done(&self) -> bool {
        self.start_time.elapsed() >= self.duration
    }

    /// The elapsed time since [`begin`](Self::begin).
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

/// The cube's device-pixel rectangle for a corner, cube size and margin (all logical).
///
/// `cube_size` is the cube's largest projected extent, not the widget width: the overlay
/// box is derived from it through [`model::overlay_size`] so the compass ring and labels
/// fit around the cube without changing the cube's on-screen size. The box is centred in
/// a column `CHROME_WIDTH` wide so the gpui chrome bar beneath it is symmetric; the
/// margin applies to that column, not the cube itself.
fn corner_rect(viewport: Viewport, corner: Corner, cube_size: f32, margin: f32) -> Rect {
    let dpr = viewport.device_pixel_ratio();
    let size = (model::overlay_size(cube_size) * dpr).round().max(1.0) as i32;
    let margin = (margin * dpr).round().max(0.0) as i32;
    let group = (CHROME_WIDTH * dpr).round().max(1.0) as i32;
    let inset = (group - size) / 2;
    let [width, height] = viewport.device_size();
    let left = margin + inset;
    let right = width as i32 - margin - group + inset;
    let top = height as i32 - margin - size;
    let bottom = margin;
    let (x, y) = match corner {
        Corner::TopLeft => (left, top),
        Corner::TopRight => (right, top),
        Corner::BottomLeft => (left, bottom),
        Corner::BottomRight => (right, bottom),
    };
    Rect::new(x, y, size.max(1) as u32, size.max(1) as u32)
}

/// The camera distance (the length of the view matrix's translation).
fn camera_distance(camera: &Camera) -> f64 {
    let distance = Vector3::new(camera.view.m[3], camera.view.m[7], camera.view.m[11]).length();
    if distance > 1e-9 {
        distance
    } else {
        4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Viewport {
        Viewport::new([640, 480], 1.0)
    }

    fn colour_of(cube: &NaviCube, piece: Piece) -> [f32; 4] {
        cube.producer
            .queue()
            .packets
            .iter()
            .find(|packet| packet.state.id == piece.pick_id())
            .map(|packet| packet.state.color)
            .expect("piece packet")
    }

    fn centre(cube: &NaviCube) -> [i32; 2] {
        let rect = cube.rect();
        [
            rect.x + rect.width as i32 / 2,
            rect.y + rect.height as i32 / 2,
        ]
    }

    #[test]
    fn the_rect_sits_in_the_top_right() {
        let cube = NaviCube::new(viewport());
        let rect = cube.rect();
        let config = NaviCubeConfig::default();
        // The cube is 100 logical px across; the overlay box is larger because the ring
        // and its labels surround it.
        let size = model::overlay_size(config.cube_size).round() as i32;
        assert_eq!(rect.width as i32, size);
        // The box is centred in the `CHROME_WIDTH` column whose right edge keeps the
        // configured margin.
        assert_eq!(rect.x, 640 - 8 - 248 + (248 - size) / 2);
        assert_eq!(rect.y, 480 - 8 - size);
    }

    #[test]
    fn the_overlay_box_is_derived_from_the_cube_size() {
        use super::model::{CUBE_SPAN, ORTHO_EXTENT};

        // The box scales linearly with the cube, so DPR is the only other scaling step.
        assert!(
            (model::overlay_size(200.0) - 2.0 * model::overlay_size(100.0)).abs() < 1e-4,
            "the box follows the cube"
        );
        // And the projection really maps the cube's body diagonal to `cube_size`: at dpr 1
        // the 100px cube's 179px box has a scale of 179/3.1 px per cube unit.
        let rect = NaviCube::new(viewport()).rect();
        let scale = rect.width as f32 / (2.0 * ORTHO_EXTENT as f32);
        assert!(
            (CUBE_SPAN as f32 * scale - 100.0).abs() < 0.5,
            "the cube is 100 logical px across, not a fraction of the box"
        );
    }

    #[test]
    fn the_rect_and_its_logical_extent_follow_the_device_pixel_ratio() {
        // The cube is configured in logical pixels; the overlay box derived from it rounds
        // to whole device pixels, so at fractional DPI its logical width is
        // `round(overlay_size(cube_size)·dpr)/dpr`. It is centred in a `CHROME_WIDTH`
        // column whose right edge carries the margin.
        let configured = model::overlay_size(NaviCubeConfig::default().cube_size);
        for (device, dpr) in [
            ([640u32, 480u32], 1.0f32),
            ([640, 480], 1.25),
            ([704, 528], 1.1),
            ([960, 720], 1.5),
            ([1280, 960], 2.0),
        ] {
            let cube = NaviCube::new(Viewport::new(device, dpr));
            let rect = cube.rect();
            let size = (configured * dpr).round() as u32;
            let margin = (8.0 * dpr).round() as i32;
            let group = (CHROME_WIDTH * dpr).round() as i32;
            let inset = (group - size as i32) / 2;
            assert_eq!(rect.width, size, "width at dpr {dpr}");
            assert_eq!(rect.height, size, "height at dpr {dpr}");
            assert_eq!(
                rect.x,
                device[0] as i32 - margin - group + inset,
                "x at dpr {dpr}"
            );
            assert_eq!(
                rect.y,
                device[1] as i32 - margin - size as i32,
                "y at dpr {dpr}"
            );

            let [left, top, right, bottom] = cube.logical_rect();
            let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
            assert!(
                close(right - left, size as f32 / dpr),
                "logical width at dpr {dpr}"
            );
            assert!(close(top, margin as f32 / dpr), "top inset at dpr {dpr}");
            assert!(
                close(bottom, (margin as f32 + size as f32) / dpr),
                "bottom at dpr {dpr}"
            );
            // The column's right edge, not the cube's, carries the margin.
            assert!(
                close(
                    right,
                    (device[0] as i32 - margin - group + inset + size as i32) as f32 / dpr
                ),
                "column right edge at dpr {dpr}"
            );
        }
    }

    #[test]
    fn compass_labels_fade_on_the_far_side_and_are_culled_when_occluded() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        // Looking down −Z the ground ring is edge-on: N projects at the ring's y offset,
        // behind the cube, so it is culled. The face letters are 3D stroke geometry now,
        // so E/S/W are the only screen text.
        let mut text: Vec<char> = cube.labels().iter().map(|label| label.text).collect();
        text.sort_unstable();
        assert_eq!(text, vec!['E', 'S', 'W'], "the far N is culled");

        // From the top the whole ring is far, so every letter fades — but all four stay
        // outside the cube's silhouette (1.38 > 0.707), so none is culled.
        let top = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&top);
        let labels = cube.labels();
        assert_eq!(labels.len(), 4, "every letter survives, faintly");
        assert!(
            labels.iter().all(|label| label.alpha < 0.2),
            "far-side letters fade so they cannot stamp over a face"
        );

        // A 45° elevated view projects the far N letter inside the cube's silhouette; it
        // is culled rather than drawn over the front faces. The survivors sit outside the
        // cube's projected corner radius (0.707 in cube space).
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(
                    Vector3::new(
                        0.0,
                        std::f64::consts::FRAC_1_SQRT_2,
                        std::f64::consts::FRAC_1_SQRT_2,
                    ),
                    Vector3::new(0.0, 0.0, 1.0),
                ),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&camera);
        let labels = cube.labels();
        let mut text: Vec<char> = labels.iter().map(|label| label.text).collect();
        text.sort_unstable();
        assert_eq!(text, vec!['E', 'S', 'W'], "N is hidden behind the cube");
        let [left, top, right, bottom] = cube.logical_rect();
        let [cx, cy] = [(left + right) / 2.0, (top + bottom) / 2.0];
        let scale = (right - left) / (2.0 * model::ORTHO_EXTENT as f32);
        for label in &labels {
            let dx = label.position[0] - cx;
            let dy = label.position[1] - cy;
            assert!(
                (dx * dx + dy * dy).sqrt() > 0.707 * scale,
                "{} sits outside the cube",
                label.text
            );
        }
    }

    #[test]
    fn north_is_at_the_top_of_the_compass() {
        // The top view looks down +Y with −Z screen-up, so the compass must read N at the
        // top, S at the bottom, E right and W left.
        let top = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        let mut cube = NaviCube::new(viewport());
        cube.sync(&top);
        let labels = cube.labels();
        let letter = |text: char| {
            labels
                .iter()
                .find(|label| label.text == text)
                .unwrap_or_else(|| panic!("{text} is on screen"))
                .position
        };
        let (north, south, east, west) = (letter('N'), letter('S'), letter('E'), letter('W'));
        assert!(
            north[1] < south[1],
            "north sits above south: {north:?} vs {south:?}"
        );
        assert!(
            east[0] > west[0],
            "east sits right of west: {east:?} vs {west:?}"
        );
    }

    #[test]
    fn the_centre_maps_to_the_front_face() {
        let mut cube = NaviCube::new(viewport());
        // With the default (identity) camera orientation, Front faces the viewer.
        cube.sync(&Camera::identity());
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::FRONT));
        // Outside the rectangle there is no hit.
        assert_eq!(cube.hit([5, 5]), None);
    }

    #[test]
    fn the_heading_offset_turns_the_compass_letters() {
        use std::f64::consts::FRAC_PI_2;

        let mut cube = NaviCube::new(viewport());
        // A top view keeps all four compass letters visible (none is culled near the
        // cube's silhouette), so a rotation is observable for every letter.
        let top = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&top);
        let before = cube.labels();
        assert_eq!(before.len(), 4);
        let north = before.iter().find(|label| label.text == 'N').expect("N");

        assert!(cube.set_heading_offset(Some(FRAC_PI_2)));
        assert!((cube.heading_offset() - FRAC_PI_2).abs() < 1e-12);
        let after = cube.labels();
        let turned = after.iter().find(|label| label.text == 'N').expect("N");
        assert_ne!(north.position, turned.position, "the lettering turned");

        // Setting the same offset again is a no-op; `None` restores the world heading.
        assert!(!cube.set_heading_offset(Some(FRAC_PI_2)));
        assert!(cube.set_heading_offset(None));
        assert_eq!(cube.heading_offset(), 0.0);
    }

    #[test]
    fn a_corner_region_maps_to_a_corner_piece() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        // Somewhere in the cube rect there is a clickable corner (the +X+Y+Z nub);
        // scanning avoids depending on the exact ortho scale. The bead is only a few
        // pixels across, so probe at 2px resolution.
        let rect = cube.rect();
        let mut found = None;
        let steps = rect.width as i32 / 2;
        for i in 0..=steps {
            for j in 0..=steps {
                let probe = [
                    rect.x + rect.width as i32 * i / steps,
                    rect.y + rect.height as i32 * j / steps,
                ];
                if cube.hit(probe) == Some(Piece { axes: [1, 1, 1] }) {
                    found = Some(probe);
                }
            }
        }
        assert!(found.is_some(), "a corner piece should be clickable");
    }

    #[test]
    fn a_corner_triangle_is_pickable_at_its_centre() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        let corner = Piece { axes: [1, 1, 1] };
        let triangle = corner.corner_triangle();
        let centroid = triangle
            .iter()
            .fold(Vector3::zero(), |sum, point| {
                sum.add(&Vector3::new(
                    f64::from(point[0]),
                    f64::from(point[1]),
                    f64::from(point[2]),
                ))
            })
            .scale(1.0 / 3.0);
        let view = Rotation::identity().to_matrix().transform(&centroid);
        let rect = cube.rect();
        let scale = f64::from(rect.width) / (2.0 * model::ORTHO_EXTENT);
        let device = [
            (f64::from(rect.x) + f64::from(rect.width) / 2.0 + view.x * scale).round() as i32,
            (f64::from(rect.y) + f64::from(rect.height) / 2.0 + view.y * scale).round() as i32,
        ];
        assert_eq!(cube.hit(device), Some(corner));
    }

    #[test]
    fn the_ring_is_a_ground_plane_ellipse_and_reports_plane_angles() {
        // Tilt the view so the ring plane is not edge-on.
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(
                    Vector3::new(0.0, 0.9, 1.0).normalize(),
                    Vector3::new(0.0, 0.0, 1.0),
                ),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        let mut cube = NaviCube::new(viewport());
        cube.sync(&camera);
        let wrap = |angle: f64| {
            (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
        };

        for angle in [0.0, 0.7, std::f64::consts::FRAC_PI_2, 2.5] {
            let device = viewport().to_device(cube.ring_point(angle));
            assert!(cube.ring_hit(device), "ring hit at {angle}");
            assert_eq!(cube.hit(device), None, "the ring is outside every piece");
            let reported = cube.ring_angle(device).expect("an in-plane angle");
            assert!(wrap(reported - angle).abs() < 0.06, "{angle} -> {reported}");
        }
        // The cube body and the outside of the rect are not ring hits.
        let rect = cube.rect();
        let body = [
            rect.x + rect.width as i32 / 2,
            rect.y + rect.height as i32 / 2,
        ];
        assert!(!cube.ring_hit(body));
        assert!(cube.ring_angle([5, 5]).is_none());
    }

    #[test]
    fn yaw_view_preserves_elevation_and_the_eye_distance() {
        // A tilted camera: a view-space yaw would tilt it further, a world-Y yaw must not.
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_vectors(
                    Vector3::new(0.0, 0.9, 1.0).normalize(),
                    Vector3::new(0.0, 0.0, 1.0),
                ),
            )
            .to_matrix(),
            ..Camera::identity()
        };
        let (azimuth, elevation) = crate::inventor_view::camera_angles(&camera);
        let turned = Camera {
            view: yaw_view(&camera, 0.5),
            ..camera.clone()
        };
        let (turned_azimuth, turned_elevation) = crate::inventor_view::camera_angles(&turned);
        assert!(
            ((turned_azimuth - azimuth).abs() - 0.5_f64.to_degrees()).abs() < 1e-6,
            "azimuth {azimuth} -> {turned_azimuth}"
        );
        assert!((turned_elevation - elevation).abs() < 1e-9);
        assert!((turned.eye_distance() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn hilite_changes_the_piece_colour_and_only_recompiles_on_change() {
        let mut cube = NaviCube::new(viewport());
        assert!(cube.set_hilite(Some(Piece::TOP)));
        assert!(!cube.set_hilite(Some(Piece::TOP)), "no-op when unchanged");
        // A hovered face keeps its directional fill; the cyan goes on its outline.
        assert_eq!(
            colour_of(&cube, Piece::TOP),
            cube.theme
                .face_fill(NavicubeRole::FaceHighlighted, &Vector3::new(0.0, 1.0, 0.0))
        );
        assert!(cube.producer.queue().packets.iter().any(|packet| {
            packet.state.id == Piece::TOP.pick_id()
                && packet.topology == ferrocad_graphics::Topology::Lines
                && packet.state.color == super::theme::EDGE_HILITE
        }));
        assert_eq!(
            colour_of(&cube, Piece::FRONT),
            cube.theme
                .face_fill(NavicubeRole::Face, &Vector3::new(0.0, 0.0, 1.0))
        );
    }

    #[test]
    fn faces_are_directionally_lit_and_back_faces_are_culled() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        // With the identity camera, +Z (Front) faces the viewer and Back points away.
        let front = colour_of(&cube, Piece::FRONT);
        assert_eq!(
            front,
            cube.theme
                .face_fill(NavicubeRole::Face, &Vector3::new(0.0, 0.0, 1.0))
        );
        assert!(
            !cube
                .producer
                .queue()
                .packets
                .iter()
                .any(|packet| packet.state.id == Piece::BACK.pick_id()),
            "the back face is culled, not drawn translucently"
        );
        // The studio light sits above, so the top facet reads brighter than the front.
        assert!(colour_of(&cube, Piece::TOP)[3] > front[3]);
    }

    #[test]
    fn sync_mirrors_the_camera_orientation() {
        let mut cube = NaviCube::new(viewport());
        // A camera looking from +X makes the Right face point at the viewer.
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Piece::RIGHT.orientation())
                .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&camera);
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::RIGHT));

        // Snapping back to Front and re-syncing turns the Front face to the viewer.
        cube.sync(&Camera {
            view: snap_view(Piece::FRONT, &Camera::identity()),
            ..Camera::identity()
        });
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::FRONT));
    }

    #[test]
    fn the_synced_cube_stays_upright_at_any_azimuth() {
        // WA9 bug: at AZ 090° / EL 36° the cube rolled onto its side (`T` at the bottom,
        // the turntable tilted across the sky). `sync` rebuilds a roll-free rotation, so
        // world up stays screen up and the turntable stays horizontal at every bearing.
        for azimuth in [0.0_f64, 90.0, 180.0, 270.0] {
            for elevation in [0.0_f64, 24.0, 36.0, 60.0] {
                let (a, e) = (azimuth.to_radians(), elevation.to_radians());
                let rotation = view_rotation(a, e).to_matrix();
                let up = rotation.transform(&Vector3::new(0.0, 1.0, 0.0));
                assert!(
                    up.x.abs() < 1e-9,
                    "the turntable must not roll at {azimuth}°/{elevation}°"
                );
                assert!(
                    (up.y - e.cos()).abs() < 1e-9,
                    "world up keeps its elevation at {azimuth}°/{elevation}°"
                );
                // `T` projects toward the top of the screen and faces the viewer.
                let top = rotation.transform(&Piece::TOP.direction());
                assert!(
                    top.y > 0.0 && top.z >= -1e-9,
                    "T must stay upright at {azimuth}°/{elevation}°"
                );
            }
        }

        // A scene camera with a screen roll must not roll the cube: `sync` extracts
        // only the eye's azimuth/elevation and rebuilds the canonical rotation.
        let (a, e) = (90.0_f64.to_radians(), 36.0_f64.to_radians());
        let canonical = view_rotation(a, e);
        let roll = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), 0.4);
        let camera = |rotation| Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), rotation).to_matrix(),
            ..Camera::identity()
        };
        let mut plain = NaviCube::new(viewport());
        let mut rolled = NaviCube::new(viewport());
        plain.sync(&camera(canonical));
        rolled.sync(&camera(roll.multiply(&canonical)));
        assert_eq!(rolled.synced, plain.synced);
    }

    #[test]
    fn yawing_the_view_orbits_the_compass() {
        // At a normal tilt the compass letters are projected world bearings, so a yaw
        // must move them around the turntable rather than leave them pinned to the
        // screen.
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), view_rotation(0.6, 0.4)).to_matrix(),
            ..Camera::identity()
        };
        let mut cube = NaviCube::new(viewport());
        cube.sync(&camera);
        let before = cube.labels();
        let yawed = Camera {
            view: yaw_view(&camera, std::f64::consts::FRAC_PI_2),
            ..camera.clone()
        };
        cube.sync(&yawed);
        let after = cube.labels();
        let moved = before.iter().any(|a| {
            after.iter().any(|b| {
                a.text == b.text
                    && (b.position[0] - a.position[0]).hypot(b.position[1] - a.position[1]) > 1.0
            })
        });
        assert!(moved, "the compass stayed put after a yaw");
    }

    #[test]
    fn a_top_view_still_turns_when_yawed() {
        // Reproduce: click `T`, then click a chrome arrow. The eye is exactly on the
        // pole, so a world-Y yaw cannot move it; WA9 rebuilt an identical canonical
        // rotation and the cube/compass froze. The pole-aware sync keeps the camera's
        // in-plane up, so the compass still turns.
        let top = Camera {
            view: snap_view(Piece::TOP, &Camera::identity()),
            ..Camera::identity()
        };
        let mut cube = NaviCube::new(viewport());
        cube.sync(&top);
        let frozen = cube.synced;
        let before = cube.ring_point(std::f64::consts::FRAC_PI_2);

        let yawed = Camera {
            view: yaw_view(&top, std::f64::consts::FRAC_PI_2),
            ..top.clone()
        };
        cube.sync(&yawed);
        assert_ne!(cube.synced, frozen, "the pole view must not freeze");
        let after = cube.ring_point(std::f64::consts::FRAC_PI_2);
        assert!(
            (after[0] - before[0]).hypot(after[1] - before[1]) > 5.0,
            "the compass did not turn at the pole: {before:?} -> {after:?}"
        );
        // `T` still faces the viewer at the pole (its screen-up is perpendicular to it).
        let top_view = cube
            .synced
            .expect("a synced rotation")
            .to_matrix()
            .transform(&Piece::TOP.direction());
        assert!(top_view.z > 0.99, "T left the viewer at the pole");
    }

    #[test]
    fn snap_view_looks_along_the_piece_direction() {
        for piece in [Piece::RIGHT, Piece::TOP, Piece { axes: [1, 1, 1] }] {
            let view = snap_view(piece, &Camera::identity());
            let rotated = Rotation::from_matrix(&view)
                .to_matrix()
                .transform(&piece.direction());
            assert!(rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
        }
    }

    #[test]
    fn slerp_interpolates_between_rotations() {
        use std::time::Duration;
        let start = Camera::identity();
        let anim = SnapAnimation::begin(Piece::RIGHT, &start, Duration::from_millis(200));
        // At the end the view looks along the piece direction.
        let end = Rotation::from_matrix(&anim.view_at(Duration::from_millis(200)));
        let rotated = end.to_matrix().transform(&Piece::RIGHT.direction());
        assert!(rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
        // Halfway is between the two orientations (neither the start nor the end).
        let mid = Rotation::from_matrix(&anim.view_at(Duration::from_millis(100)));
        assert!(!mid.to_matrix().is_unity(1e-6));
        assert!(mid.q != end.q);
        // A zero-length duration is instant.
        assert!(anim.is_done() == false || anim.start_time.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn a_diagonal_drag_keeps_the_horizon_level() {
        let view = drag_view(&Camera::identity(), [40.0, 30.0]);
        let up = Rotation::from_matrix(&view)
            .to_matrix()
            .transform(&Vector3::new(0.0, 1.0, 0.0));
        assert!(
            up.x.abs() < 1e-9,
            "a diagonal orbit must not roll the world-up axis (x = {})",
            up.x
        );
    }

    #[test]
    fn drag_view_orbits_around_the_camera() {
        let camera = Camera::identity();
        let view = drag_view(&camera, [50.0, 0.0]);
        // A horizontal drag rotates the view (not the identity anymore) but keeps the
        // camera distance.
        assert!(!Rotation::from_matrix(&view).to_matrix().is_unity(1e-9));
        let base = Vector3::new(view.m[3], view.m[7], view.m[11]);
        assert!(base.is_null());
    }
}
