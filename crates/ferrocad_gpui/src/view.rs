//! View requests and the shell's presentation state (A5, extended in G5).
//!
//! A command that changes only presentation returns `{"view": {...}}` instead of
//! editing the document. The shell owns the state, applies what the viewport supports
//! (the projection, the grid and its steps), and mirrors the state to
//! `FerroCADGui.ViewState` so a checkable command's `IsChecked` can read it.
//!
//! A `workplane` request is an **action**, not state: it activates, clears or looks down
//! the active workplane. The shell performs it on the viewport and reports the outcome.

use ferrocad_stage::grid;
use ferrocad_stage::{
    AxisStyle, GizmoStyle, MAX_GHOST_ALPHA, MIN_HIT_RADIUS_PX, StandardPlane,
};
use serde::Deserialize;

/// A presentation change a command returns. Never undoable, never a document edit.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ViewRequest {
    /// Show or hide the reference grid.
    #[serde(default)]
    pub grid: Option<bool>,
    /// Show or hide the axis cross (the world triad, its origin node and axis tags).
    /// **Deprecated alias** for `axis.enabled`: it applies only when the nested request
    /// omits `enabled`.
    #[serde(default)]
    pub axis_cross: Option<bool>,
    /// The axis cross's style and visibility (`view.axis`).
    #[serde(default)]
    pub axis: Option<AxisRequest>,
    /// The transform gizmo's style and visibility (`view.gizmo`).
    #[serde(default)]
    pub gizmo: Option<GizmoRequest>,
    /// `"persp"`/`"perspective"` or `"ortho"`/`"orthographic"`.
    #[serde(default)]
    pub projection: Option<String>,
    /// The world grid's minor step, in millimetres.
    #[serde(default)]
    pub grid_minor: Option<f64>,
    /// The world grid's major step, in millimetres.
    #[serde(default)]
    pub grid_major: Option<f64>,
    /// A fixed world-grid extent (mm). Omitted keeps it camera-derived.
    #[serde(default)]
    pub grid_radius: Option<f64>,
    /// The world grid's ink as a `"#rrggbb"` (or `"#rgb"`) string.
    #[serde(default)]
    pub grid_color: Option<String>,
    /// A multiplier over the grid tokens' alphas, in `0.0..=1.0`.
    #[serde(default)]
    pub grid_alpha: Option<f32>,
    /// Hide the native ground layer in favour of an authored (shader) grid. Registered
    /// overlays draw regardless; this flag is about the native ground.
    #[serde(default)]
    pub grid_overlay: Option<bool>,
    /// A workplane action: `"activate"`, `"clear"`, `"view"`, a standard plane
    /// (`"world:xy"`, `"world:xz"`, `"world:yz"`), `"camera"` or `"selection"`.
    #[serde(default)]
    pub workplane: Option<String>,
}

/// A parameterized axis-cross change (`view.axis`). Every field is optional; an
/// omitted field keeps the current value.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AxisRequest {
    /// Show or hide the axis cross.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// A provider claim: hide the native triad, origin node and labels because a
    /// registered backend (the Coin overlay) draws the cross. Not a user setting, and
    /// not geometry.
    #[serde(default)]
    pub overlay: Option<bool>,
    /// The X stem's `"#rrggbb"` (or `"#rgb"`) ink.
    #[serde(default)]
    pub color_x: Option<String>,
    /// The Y stem's ink.
    #[serde(default)]
    pub color_y: Option<String>,
    /// The Z stem's ink.
    #[serde(default)]
    pub color_z: Option<String>,
    /// A multiplier over every stem's alpha, clamped to `0.0..=1.0`.
    #[serde(default)]
    pub opacity: Option<f32>,
    /// The stem length, in device pixels (finite and positive).
    #[serde(default)]
    pub length: Option<f64>,
    /// The stem width, in device pixels (finite and positive).
    #[serde(default)]
    pub width: Option<f64>,
    /// Draw the origin node.
    #[serde(default)]
    pub origin: Option<bool>,
    /// Draw the `X`/`Y`/`Z` (or `U`/`V`/`N`) tags.
    #[serde(default)]
    pub labels: Option<bool>,
    /// The occluded stem pass's alpha multiplier, clamped to `0.0..=1.0`.
    #[serde(default)]
    pub ghost_alpha: Option<f32>,
}

/// A parameterized gizmo change (`view.gizmo`). Every field is optional; an omitted
/// field keeps the current value.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GizmoRequest {
    /// Show or hide the transform gizmo.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// A provider claim: the id of the backend that owns the gizmo's visuals (the native
    /// default is `"ferrocad"`), not a user setting, and not geometry.
    #[serde(default)]
    pub backend: Option<String>,
    /// The hovered/grabbed handle's `"#rrggbb"` (or `"#rgb"`) ink. The hex is RGB-only;
    /// the current alpha is kept.
    #[serde(default)]
    pub active_color: Option<String>,
    /// The idle handle's ink. The hex is RGB-only; the current alpha is kept.
    #[serde(default)]
    pub inactive_color: Option<String>,
    /// The outline/halo ink behind the handles. RGB-only; the alpha is kept.
    #[serde(default)]
    pub outline_color: Option<String>,
    /// The outline width around a handle, in device pixels (`0` disables it).
    #[serde(default)]
    pub halo_width: Option<f64>,
    /// The stroke-width multiplier of the hovered/grabbed handle (`1.0` disables).
    #[serde(default)]
    pub hover_scale: Option<f64>,
    /// The analytic grab radius, in device pixels (clamped up to the 24 px target).
    #[serde(default)]
    pub hit_radius_px: Option<f64>,
    /// The triad's target radius, in device pixels (finite and positive).
    #[serde(default)]
    pub size: Option<f64>,
    /// The handle stroke width, in device pixels (finite and positive).
    #[serde(default)]
    pub width: Option<f64>,
    /// A multiplier over every handle's alpha, clamped to `0.0..=1.0`.
    #[serde(default)]
    pub opacity: Option<f32>,
    /// The occluded ghost pass's alpha multiplier, clamped to `0.0..=1.0`.
    #[serde(default)]
    pub ghost_alpha: Option<f32>,
    /// The rotation-ring detent, in degrees, clamped to `0.0..=180.0`; `0` means
    /// continuous rotation.
    #[serde(default)]
    pub ring_snap_deg: Option<f64>,
    /// Show the numeric readout beside the handle.
    #[serde(default)]
    pub show_readout: Option<bool>,
}

/// A workplane action carried by a [`ViewRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkplaneAction {
    /// Activate the face under the pointer.
    Activate,
    /// Clear the active workplane.
    Clear,
    /// Look down the active workplane's normal.
    View,
    /// Activate one of the three world-standard axis planes.
    Standard(StandardPlane),
    /// Adopt the camera's view plane.
    Camera,
    /// Activate the selected planar face.
    Selection,
}

impl WorkplaneAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "activate" => Ok(Self::Activate),
            "clear" => Ok(Self::Clear),
            "view" => Ok(Self::View),
            "world:xy" | "xy" => Ok(Self::Standard(StandardPlane::Top)),
            "world:xz" | "xz" => Ok(Self::Standard(StandardPlane::Front)),
            "world:yz" | "yz" => Ok(Self::Standard(StandardPlane::Right)),
            "camera" | "view-plane" | "viewplane" => Ok(Self::Camera),
            "selection" | "selected" => Ok(Self::Selection),
            other => Err(format!("unknown workplane action {other:?}")),
        }
    }
}

impl ViewRequest {
    /// The validated workplane action, when the request carries one.
    pub fn workplane_action(&self) -> Result<Option<WorkplaneAction>, String> {
        self.workplane
            .as_deref()
            .map(WorkplaneAction::parse)
            .transpose()
    }
}

/// The projection as the shell names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionName {
    Perspective,
    Orthographic,
}

impl ProjectionName {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "persp" | "perspective" => Ok(Self::Perspective),
            "ortho" | "orthographic" => Ok(Self::Orthographic),
            other => Err(format!("unknown projection {other:?}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Perspective => "persp",
            Self::Orthographic => "ortho",
        }
    }
}

/// The shell's presentation state, mirrored to `FerroCADGui.ViewState`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub grid: bool,
    /// Whether the axis cross is shown (on by default, as upstream's is). This mirrors
    /// [`axis.enabled`](AxisStyle::enabled); the Python facade reads it.
    pub axis_cross: bool,
    /// The axis cross's parameterized style.
    pub axis: AxisStyle,
    /// The transform gizmo's parameterized style.
    pub gizmo: GizmoStyle,
    pub projection: ProjectionName,
    /// The world grid's minor step (mm).
    pub grid_minor: f64,
    /// The world grid's major step (mm).
    pub grid_major: f64,
    /// A fixed world-grid extent (mm); `None` keeps it camera-derived.
    pub grid_radius: Option<f64>,
    /// The world grid's linear ink (alpha is carried by the per-vertex fade).
    pub grid_color: [f32; 4],
    /// A multiplier over the grid tokens' alphas.
    pub grid_alpha: f32,
    /// Whether the authored (shader) grid replaces the native ground (`false`: the native
    /// grid is). When on, the viewport hides the native ground; registered program
    /// overlays draw either way. `grid_radius` is ignored (an infinite shader grid has no
    /// extent).
    pub grid_overlay: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            grid: false,
            axis_cross: true,
            axis: AxisStyle::default(),
            gizmo: GizmoStyle::default(),
            projection: ProjectionName::Perspective,
            grid_minor: grid::MINOR_STEP,
            grid_major: grid::MAJOR_STEP,
            grid_radius: None,
            grid_color: grid::GRID_COLOR,
            grid_alpha: 1.0,
            grid_overlay: false,
        }
    }
}

impl ViewState {
    /// Apply a request and summarise what changed (`"grid on"`, `"projection ortho"`).
    ///
    /// Everything is validated **before** anything is mutated, so an unknown
    /// projection or an invalid step leaves the state untouched. A `workplane` action
    /// is validated here but performed by the shell (it is not state).
    pub fn apply(&mut self, request: &ViewRequest) -> Result<String, String> {
        let projection = request
            .projection
            .as_deref()
            .map(ProjectionName::parse)
            .transpose()?;
        request.workplane_action()?;
        if let Some(minor) = request.grid_minor {
            if !(minor.is_finite() && minor > 0.0) {
                return Err(format!("grid minor step must be positive, got {minor}"));
            }
        }
        if let Some(major) = request.grid_major {
            if !(major.is_finite() && major > 0.0) {
                return Err(format!("grid major step must be positive, got {major}"));
            }
        }
        if let Some(radius) = request.grid_radius {
            if !(radius.is_finite() && radius >= 0.0) {
                return Err(format!(
                    "grid radius must be zero (auto) or positive, got {radius}"
                ));
            }
        }
        let grid_color = match request.grid_color.as_deref() {
            Some(text) => Some(grid::parse_srgb_hex(text)?),
            None => None,
        };
        if let Some(alpha) = request.grid_alpha {
            if !(alpha.is_finite() && (0.0..=1.0).contains(&alpha)) {
                return Err(format!("grid alpha must be between 0 and 1, got {alpha}"));
            }
        }
        // The axis style is validated up front too, so a bad colour (or a degenerate
        // stem) rejects the whole request before any field is mutated.
        let axis_colors = match request.axis.as_ref() {
            Some(axis) => [
                axis.color_x
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
                axis.color_y
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
                axis.color_z
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
            ],
            None => [None, None, None],
        };
        if let Some(axis) = request.axis.as_ref() {
            for (name, value) in [
                ("opacity", axis.opacity),
                ("ghost alpha", axis.ghost_alpha),
            ] {
                if let Some(value) = value {
                    if !value.is_finite() {
                        return Err(format!("axis {name} must be finite, got {value}"));
                    }
                }
            }
            for (name, value) in [("length", axis.length), ("width", axis.width)] {
                if let Some(value) = value {
                    if !(value.is_finite() && value > 0.0) {
                        return Err(format!(
                            "axis {name} must be positive and finite, got {value}"
                        ));
                    }
                }
            }
        }
        // The gizmo style is validated up front too, so a bad colour (or a degenerate
        // handle) rejects the whole request before any field is mutated.
        let gizmo_colors = match request.gizmo.as_ref() {
            Some(gizmo) => [
                gizmo
                    .active_color
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
                gizmo
                    .inactive_color
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
                gizmo
                    .outline_color
                    .as_deref()
                    .map(grid::parse_srgb_hex)
                    .transpose()?,
            ],
            None => [None, None, None],
        };
        let gizmo_backend = match request.gizmo.as_ref().and_then(|gizmo| gizmo.backend.as_deref())
        {
            Some(text) => {
                let backend = text.trim();
                if backend.is_empty() {
                    return Err("gizmo backend cannot be empty".to_string());
                }
                Some(backend.to_string())
            }
            None => None,
        };
        if let Some(gizmo) = request.gizmo.as_ref() {
            for (name, value) in [
                ("opacity", gizmo.opacity),
                ("ghost alpha", gizmo.ghost_alpha),
            ] {
                if let Some(value) = value {
                    if !value.is_finite() {
                        return Err(format!("gizmo {name} must be finite, got {value}"));
                    }
                }
            }
            for (name, value) in [("size", gizmo.size), ("width", gizmo.width)] {
                if let Some(value) = value {
                    if !(value.is_finite() && value > 0.0) {
                        return Err(format!(
                            "gizmo {name} must be positive and finite, got {value}"
                        ));
                    }
                }
            }
            for (name, value) in [
                ("halo width", gizmo.halo_width),
                ("hover scale", gizmo.hover_scale),
                ("hit radius", gizmo.hit_radius_px),
            ] {
                if let Some(value) = value {
                    if !(value.is_finite() && value >= 0.0) {
                        return Err(format!(
                            "gizmo {name} must be non-negative and finite, got {value}"
                        ));
                    }
                }
            }
            if let Some(scale) = gizmo.hover_scale {
                if scale < 1.0 {
                    return Err(format!(
                        "gizmo hover scale must be at least 1.0, got {scale}"
                    ));
                }
            }
            if let Some(snap) = gizmo.ring_snap_deg {
                if !snap.is_finite() {
                    return Err(format!("gizmo ring snap must be finite, got {snap}"));
                }
            }
        }
        let minor = request.grid_minor.unwrap_or(self.grid_minor);
        let major = request.grid_major.unwrap_or(self.grid_major);
        if major < minor - 1e-9 {
            return Err(format!(
                "the grid major step ({major}) cannot be below the minor one ({minor})"
            ));
        }

        let mut changes = Vec::new();
        if let Some(grid) = request.grid {
            if self.grid != grid {
                self.grid = grid;
                changes.push(format!("grid {}", if grid { "on" } else { "off" }));
            }
        }
        if let Some(axis) = request.axis.as_ref() {
            let mut summary: Option<&'static str> = None;
            let mut styled = false;
            if let Some(enabled) = axis.enabled {
                if self.axis.enabled != enabled {
                    self.axis.enabled = enabled;
                    summary = Some(if enabled { "axis on" } else { "axis off" });
                }
            } else if let Some(alias) = request.axis_cross {
                // The deprecated alias applies only when `axis.enabled` is absent.
                if self.axis.enabled != alias {
                    self.axis.enabled = alias;
                    summary = Some(if alias { "axis cross on" } else { "axis cross off" });
                }
            }
            for (index, color) in axis_colors.into_iter().enumerate() {
                if let Some(color) = color {
                    // Compare in the 8-bit sRGB space, as the grid ink does: the default
                    // tokens are authored at three decimals.
                    if ink_hex(self.axis.colors[index]) != ink_hex(color) {
                        self.axis.colors[index] = color;
                        styled = true;
                    }
                }
            }
            if let Some(opacity) = axis.opacity {
                let opacity = opacity.clamp(0.0, 1.0);
                if (self.axis.opacity - opacity).abs() > f32::EPSILON {
                    self.axis.opacity = opacity;
                    styled = true;
                }
            }
            if let Some(length) = axis.length {
                if (self.axis.length - length).abs() > 1e-12 {
                    self.axis.length = length;
                    styled = true;
                }
            }
            if let Some(width) = axis.width {
                if (self.axis.width - width).abs() > 1e-12 {
                    self.axis.width = width;
                    styled = true;
                }
            }
            if let Some(origin) = axis.origin {
                if self.axis.origin != origin {
                    self.axis.origin = origin;
                    styled = true;
                }
            }
            if let Some(labels) = axis.labels {
                if self.axis.labels != labels {
                    self.axis.labels = labels;
                    styled = true;
                }
            }
            if let Some(ghost_alpha) = axis.ghost_alpha {
                let ghost_alpha = ghost_alpha.clamp(0.0, 1.0);
                if (self.axis.ghost_alpha - ghost_alpha).abs() > f32::EPSILON {
                    self.axis.ghost_alpha = ghost_alpha;
                    styled = true;
                }
            }
            if summary.is_none() && styled {
                summary = Some("axis style");
            }
            if let Some(summary) = summary {
                changes.push(summary.to_string());
            }
            if let Some(overlay) = axis.overlay {
                if self.axis.overlay != overlay {
                    self.axis.overlay = overlay;
                    changes.push(format!(
                        "axis overlay {} ({})",
                        if overlay { "on" } else { "off" },
                        if overlay {
                            "native triad hidden"
                        } else {
                            "native triad"
                        }
                    ));
                }
            }
            // `axis_cross` stays a bool at the top level, mirroring the style.
            self.axis_cross = self.axis.enabled;
        } else if let Some(axis_cross) = request.axis_cross {
            if self.axis_cross != axis_cross {
                self.axis_cross = axis_cross;
                self.axis.enabled = axis_cross;
                changes.push(format!(
                    "axis cross {}",
                    if axis_cross { "on" } else { "off" }
                ));
            }
        }
        if let Some(gizmo) = request.gizmo.as_ref() {
            let mut summary: Option<&'static str> = None;
            let mut styled = false;
            if let Some(enabled) = gizmo.enabled {
                if self.gizmo.enabled != enabled {
                    self.gizmo.enabled = enabled;
                    summary = Some(if enabled { "gizmo on" } else { "gizmo off" });
                }
            }
            let [active, inactive, outline] = gizmo_colors;
            if let Some(color) = active {
                // Compare in the 8-bit sRGB space, as the axis colours do: the default
                // tokens are authored at three decimals. The hex carries no alpha, so the
                // token's existing alpha is kept.
                if ink_hex(self.gizmo.active) != ink_hex(color) {
                    self.gizmo.active = [color[0], color[1], color[2], self.gizmo.active[3]];
                    styled = true;
                }
            }
            if let Some(color) = inactive {
                if ink_hex(self.gizmo.inactive) != ink_hex(color) {
                    self.gizmo.inactive =
                        [color[0], color[1], color[2], self.gizmo.inactive[3]];
                    styled = true;
                }
            }
            if let Some(color) = outline {
                if ink_hex(self.gizmo.outline) != ink_hex(color) {
                    self.gizmo.outline = [color[0], color[1], color[2], self.gizmo.outline[3]];
                    styled = true;
                }
            }
            if let Some(width) = gizmo.halo_width {
                let width = width.clamp(0.0, 8.0);
                if (self.gizmo.halo_width - width).abs() > 1e-12 {
                    self.gizmo.halo_width = width;
                    styled = true;
                }
            }
            if let Some(scale) = gizmo.hover_scale {
                let scale = scale.clamp(1.0, 4.0);
                if (self.gizmo.hover_scale - scale).abs() > 1e-12 {
                    self.gizmo.hover_scale = scale;
                    styled = true;
                }
            }
            if let Some(radius) = gizmo.hit_radius_px {
                // A grab target below 24 px fails WCAG 2.5.8, so the floor is the spec.
                let radius = radius.clamp(MIN_HIT_RADIUS_PX, 48.0);
                if (self.gizmo.hit_radius_px - radius).abs() > 1e-12 {
                    self.gizmo.hit_radius_px = radius;
                    styled = true;
                }
            }
            if let Some(opacity) = gizmo.opacity {
                let opacity = opacity.clamp(0.0, 1.0);
                if (self.gizmo.opacity - opacity).abs() > f32::EPSILON {
                    self.gizmo.opacity = opacity;
                    styled = true;
                }
            }
            if let Some(size) = gizmo.size {
                if (self.gizmo.size - size).abs() > 1e-12 {
                    self.gizmo.size = size;
                    styled = true;
                }
            }
            if let Some(width) = gizmo.width {
                if (self.gizmo.width - width).abs() > 1e-12 {
                    self.gizmo.width = width;
                    styled = true;
                }
            }
            if let Some(ghost_alpha) = gizmo.ghost_alpha {
                // Above half, an occluded handle reads as a front handle; the WCAG
                // setting clamps rather than trusting the caller.
                let ghost_alpha = ghost_alpha.clamp(0.0, MAX_GHOST_ALPHA);
                if (self.gizmo.ghost_alpha - ghost_alpha).abs() > f32::EPSILON {
                    self.gizmo.ghost_alpha = ghost_alpha;
                    styled = true;
                }
            }
            if let Some(snap) = gizmo.ring_snap_deg {
                // Zero is continuous rotation; otherwise the detent is a 5° multiple so
                // a motor-impaired drag never has to hit an arbitrary fraction.
                let snap = snap.clamp(0.0, 180.0);
                let snap = (snap / 5.0).round() * 5.0;
                if (self.gizmo.ring_snap_deg - snap).abs() > 1e-12 {
                    self.gizmo.ring_snap_deg = snap;
                    styled = true;
                }
            }
            if let Some(show_readout) = gizmo.show_readout {
                if self.gizmo.show_readout != show_readout {
                    self.gizmo.show_readout = show_readout;
                    styled = true;
                }
            }
            if summary.is_none() && styled {
                summary = Some("gizmo style");
            }
            if let Some(summary) = summary {
                changes.push(summary.to_string());
            }
            if let Some(backend) = gizmo_backend {
                if self.gizmo.backend != backend {
                    changes.push(format!("gizmo backend {backend}"));
                    self.gizmo.backend = backend;
                }
            }
        }
        if let Some(projection) = projection {
            if self.projection != projection {
                self.projection = projection;
                changes.push(format!("projection {}", projection.as_str()));
            }
        }
        if let Some(minor) = request.grid_minor {
            if (self.grid_minor - minor).abs() > 1e-12 {
                self.grid_minor = minor;
                changes.push(format!("grid minor {}", trim(minor)));
            }
        }
        if let Some(major) = request.grid_major {
            if (self.grid_major - major).abs() > 1e-12 {
                self.grid_major = major;
                changes.push(format!("grid major {}", trim(major)));
            }
        }
        if let Some(radius) = request.grid_radius {
            // Zero means "back to the camera-derived extent".
            let next = (radius > 0.0).then_some(radius);
            if self.grid_radius != next {
                self.grid_radius = next;
                changes.push(match next {
                    Some(radius) => format!("grid radius {}", trim(radius)),
                    None => "grid radius auto".to_string(),
                });
            }
        }
        if let Some(color) = grid_color {
            // Compare in the user's (8-bit sRGB) space: the ink constants are authored at
            // three decimals, so linear equality would report `#94a3b8` as a change.
            let hex = ink_hex(color);
            if ink_hex(self.grid_color) != hex {
                self.grid_color = color;
                changes.push(format!("grid colour {hex}"));
            }
        }
        if let Some(alpha) = request.grid_alpha {
            if (self.grid_alpha - alpha).abs() > f32::EPSILON {
                self.grid_alpha = alpha;
                changes.push(format!("grid alpha {}", trim(f64::from(alpha))));
            }
        }
        if let Some(grid_overlay) = request.grid_overlay {
            if self.grid_overlay != grid_overlay {
                self.grid_overlay = grid_overlay;
                changes.push(format!(
                    "grid overlay {} ({})",
                    if grid_overlay { "on" } else { "off" },
                    if grid_overlay {
                        "native ground hidden"
                    } else {
                        "native grid"
                    }
                ));
            }
        }
        // The shader grid is infinite, so a radius is meaningless while it is on. Say so
        // rather than silently accepting a value the viewport will not use.
        if self.grid_overlay && request.grid_radius.is_some() {
            changes.push("grid radius ignored (shader grid)".to_string());
        }
        Ok(if changes.is_empty() {
            "view unchanged".to_string()
        } else {
            changes.join(", ")
        })
    }
}

/// A compact number for a status line (`10`, not `10.0`).
fn trim(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The ink tokens are authored at three decimals, so spelling a token in hex can land
/// in the last digit away from its constant (`#94a3b8` parses to `0.4794…`, the token
/// is `0.480`). Comparisons therefore happen in the canonical `#rrggbb` space, where
/// the token round-trips exactly and every 8-bit step stays distinct.
fn ink_hex(color: [f32; 4]) -> String {
    let channel = |value: f32| {
        let value = value.clamp(0.0, 1.0);
        let srgb = if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color[0]),
        channel(color[1]),
        channel(color[2])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_a_perspective_view_without_a_grid() {
        let state = ViewState::default();
        assert!(!state.grid);
        assert!(state.axis_cross, "the axis cross is on by default");
        assert_eq!(state.projection, ProjectionName::Perspective);
        assert_eq!(state.projection.as_str(), "persp");
        assert_eq!(state.grid_minor, grid::MINOR_STEP);
        assert_eq!(state.grid_major, grid::MAJOR_STEP);
        assert_eq!(state.grid_radius, None);
        assert_eq!(state.grid_color, grid::GRID_COLOR);
        assert_eq!(state.grid_alpha, 1.0);
        assert!(!state.grid_overlay, "the native grid is authoritative");

        // The axis cross defaults reproduce today's look.
        assert!(state.axis.enabled);
        assert!(!state.axis.overlay, "the native backend is authoritative");
        assert_eq!(state.axis_cross, state.axis.enabled);
        assert_eq!(state.axis.colors, AxisStyle::default().colors);
        assert_eq!(state.axis.opacity, 0.65);
        assert_eq!(state.axis.length, 80.0);
        assert_eq!(state.axis.width, 1.5);
        assert!(state.axis.origin);
        assert!(state.axis.labels);
        assert_eq!(state.axis.ghost_alpha, 0.10);
    }

    #[test]
    fn a_request_applies_grid_and_projection() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid: Some(true),
            projection: Some("orthographic".to_string()),
            ..Default::default()
        };
        assert_eq!(state.apply(&request).unwrap(), "grid on, projection ortho");
        assert!(state.grid);
        assert_eq!(state.projection, ProjectionName::Orthographic);

        // Re-applying the same values is a no-op, not an error.
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
    }

    #[test]
    fn an_unknown_projection_changes_nothing() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid: Some(true),
            projection: Some("isometric".to_string()),
            ..Default::default()
        };
        let error = state.apply(&request).expect_err("unknown projection");
        assert!(error.contains("isometric"), "{error}");
        assert!(!state.grid, "grid was not applied either");
        assert_eq!(state.projection, ProjectionName::Perspective);
    }

    #[test]
    fn the_axis_cross_is_a_separate_flag() {
        let mut state = ViewState::default();
        let off = ViewRequest {
            axis_cross: Some(false),
            ..Default::default()
        };
        assert_eq!(state.apply(&off).unwrap(), "axis cross off");
        assert!(!state.axis_cross);
        assert!(!state.grid, "the grid is untouched");

        // The grid does not imply the cross.
        let grid = ViewRequest {
            grid: Some(true),
            ..Default::default()
        };
        assert_eq!(state.apply(&grid).unwrap(), "grid on");
        assert!(!state.axis_cross);
    }

    #[test]
    fn every_axis_field_applies() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r##"{"axis": {
                "color_x": "#010203", "color_y": "#0f0", "color_z": "#fff",
                "opacity": 0.25, "length": 40.0, "width": 3.0,
                "origin": false, "labels": false, "ghost_alpha": 0.3
            }}"##,
        )
        .expect("the contract's request shape");
        assert_eq!(state.apply(&request).unwrap(), "axis style");

        let axis = &state.axis;
        assert_eq!(axis.colors[0], grid::parse_srgb_hex("#010203").unwrap());
        assert_eq!(axis.colors[1], grid::parse_srgb_hex("#00ff00").unwrap());
        assert_eq!(axis.colors[2], grid::parse_srgb_hex("#ffffff").unwrap());
        assert_eq!(axis.opacity, 0.25);
        assert_eq!(axis.length, 40.0);
        assert_eq!(axis.width, 3.0);
        assert!(!axis.origin);
        assert!(!axis.labels);
        assert_eq!(axis.ghost_alpha, 0.3);
        assert_eq!(state.axis_cross, axis.enabled, "the bool mirrors the style");

        // The same request again is a no-op.
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");

        // Opacity and ghost alpha clamp rather than error.
        let clamped: ViewRequest = serde_json::from_str(
            r#"{"axis": {"opacity": 2.0, "ghost_alpha": -1.0}}"#,
        )
        .unwrap();
        assert_eq!(state.apply(&clamped).unwrap(), "axis style");
        assert_eq!(state.axis.opacity, 1.0);
        assert_eq!(state.axis.ghost_alpha, 0.0);
    }

    #[test]
    fn a_bad_axis_field_is_rejected_atomically() {
        let bad_colour = ViewRequest {
            grid: Some(true),
            axis: Some(AxisRequest {
                enabled: Some(false),
                color_y: Some("not a colour".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut state = ViewState::default();
        assert!(state.apply(&bad_colour).is_err());
        assert!(!state.grid, "nothing was applied");
        assert!(state.axis.enabled, "the flag was not applied");
        assert!(state.axis_cross, "the mirror is untouched");
        assert_eq!(state.axis.colors, AxisStyle::default().colors);

        for (length, width) in [(0.0, 1.5), (-1.0, 1.5), (f64::NAN, 1.5), (80.0, 0.0)] {
            let request = ViewRequest {
                grid: Some(true),
                axis: Some(AxisRequest {
                    length: Some(length),
                    width: Some(width),
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(state.apply(&request).is_err(), "{length} × {width}");
            assert!(!state.grid, "nothing was applied");
            assert_eq!(state.axis.length, 80.0);
        }
    }

    #[test]
    fn the_axis_overlay_is_a_provider_claim_not_geometry() {
        let mut state = ViewState::default();
        let request: ViewRequest =
            serde_json::from_str(r#"{"axis": {"overlay": true}}"#).expect("the provider claim");
        assert_eq!(
            state.apply(&request).unwrap(),
            "axis overlay on (native triad hidden)"
        );
        assert!(state.axis.overlay);
        assert!(state.axis.enabled, "the user's switch is untouched");
        assert!(state.axis_cross, "and so is its mirror");

        // Geometry and the baked colours are untouched: the flag only hides the
        // native drawing.
        let geometry = AxisStyle::default();
        assert_eq!(state.axis.colors, geometry.colors);
        assert_eq!(state.axis.opacity, geometry.opacity);
        assert_eq!(state.axis.length, geometry.length);
        assert_eq!(state.axis.width, geometry.width);
        assert_eq!(state.axis.origin, geometry.origin);
        assert_eq!(state.axis.labels, geometry.labels);
        assert_eq!(state.axis.ghost_alpha, geometry.ghost_alpha);

        // Clearing the claim restores the native drawing.
        let cleared: ViewRequest =
            serde_json::from_str(r#"{"axis": {"overlay": false}}"#).unwrap();
        assert_eq!(state.apply(&cleared).unwrap(), "axis overlay off (native triad)");
        assert!(!state.axis.overlay);

        // A bad field still rejects the whole request atomically, overlay included.
        let bad: ViewRequest = serde_json::from_str(
            r##"{"axis": {"overlay": true, "color_x": "not a colour"}}"##,
        )
        .unwrap();
        assert!(state.apply(&bad).is_err());
        assert!(!state.axis.overlay, "nothing was applied");
    }

    #[test]
    fn the_axis_cross_alias_still_works() {
        let mut state = ViewState::default();
        let alias = ViewRequest {
            axis_cross: Some(false),
            ..Default::default()
        };
        assert_eq!(state.apply(&alias).unwrap(), "axis cross off");
        assert!(!state.axis_cross);
        assert!(!state.axis.enabled, "the alias drives the style");

        // The alias is ignored when the nested request states `enabled`.
        let mut state = ViewState::default();
        let both = ViewRequest {
            axis_cross: Some(true),
            axis: Some(AxisRequest {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(state.apply(&both).unwrap(), "axis off");
        assert!(!state.axis_cross);
        assert!(!state.axis.enabled, "`axis.enabled` wins");
    }

    #[test]
    fn the_axis_cross_mirrors_the_nested_enabled_flag() {
        let mut state = ViewState::default();
        for enabled in [false, true, false] {
            let request = ViewRequest {
                axis: Some(AxisRequest {
                    enabled: Some(enabled),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let summary = state.apply(&request).unwrap();
            assert_eq!(summary, if enabled { "axis on" } else { "axis off" });
            assert_eq!(state.axis_cross, enabled);
            assert_eq!(state.axis_cross, state.axis.enabled);
        }
    }

    #[test]
    fn the_axis_contract_example_deserializes() {
        let request: ViewRequest = serde_json::from_str(
            r##"{"axis": {
                "enabled": true,
                "color_x": "#f87171", "color_y": "#4ade80", "color_z": "#38bdf8",
                "opacity": 0.65,
                "length": 80.0, "width": 1.5,
                "origin": true, "labels": true,
                "ghost_alpha": 0.10
            }}"##,
        )
        .unwrap();
        assert_eq!(request.axis, Some(AxisRequest {
            enabled: Some(true),
            overlay: None,
            color_x: Some("#f87171".to_string()),
            color_y: Some("#4ade80".to_string()),
            color_z: Some("#38bdf8".to_string()),
            opacity: Some(0.65),
            length: Some(80.0),
            width: Some(1.5),
            origin: Some(true),
            labels: Some(true),
            ghost_alpha: Some(0.10),
        }));
        assert_eq!(request.axis_cross, None);
    }

    #[test]
    fn the_gizmo_defaults_carry_the_accessibility_policy() {
        let state = ViewState::default();
        assert_eq!(state.gizmo, GizmoStyle::default());
        assert!(state.gizmo.enabled);
        assert_eq!(state.gizmo.backend, "ferrocad");
        assert_eq!(state.gizmo.size, 110.0);
        assert_eq!(state.gizmo.width, 2.0);
        assert_eq!(state.gizmo.opacity, 1.0);
        // The WCAG 2.2 AA defaults: a contrast halo, a non-colour hover cue, a 24 px
        // target and an occluded alpha that stays visibly behind a front handle.
        assert_eq!(state.gizmo.ring_snap_deg, 15.0);
        assert!(state.gizmo.show_readout);
        assert_eq!(state.gizmo.inactive[3], 0.9, "the legacy inactive alpha");
    }

    #[test]
    fn the_gizmo_contract_example_is_a_no_op() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r##"{"gizmo": {
                "enabled": true, "backend": "ferrocad",
                "active_color": "#38bdf8", "inactive_color": "#94a3b8",
                "outline_color": "#0f172a",
                "size": 110.0, "width": 2.0, "halo_width": 1.0,
                "hover_scale": 1.5, "hit_radius_px": 12.0,
                "opacity": 1.0,
                "ghost_alpha": 0.2, "ring_snap_deg": 15.0, "show_readout": true
            }}"##,
        )
        .expect("the contract's request shape");
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
        assert_eq!(state.gizmo, GizmoStyle::default());
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
    }

    #[test]
    fn every_gizmo_visual_field_applies() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r##"{"gizmo": {
                "active_color": "#010203", "inactive_color": "#0f0",
                "size": 40.0, "width": 3.0, "opacity": 0.25,
                "ghost_alpha": 0.3, "ring_snap_deg": 45.0, "show_readout": false
            }}"##,
        )
        .expect("the contract's request shape");
        assert_eq!(state.apply(&request).unwrap(), "gizmo style");

        let gizmo = &state.gizmo;
        let mut active = grid::parse_srgb_hex("#010203").unwrap();
        active[3] = 1.0;
        assert_eq!(gizmo.active, active);
        let mut inactive = grid::parse_srgb_hex("#00ff00").unwrap();
        inactive[3] = 0.9;
        assert_eq!(gizmo.inactive, inactive, "the token's alpha is kept");
        assert_eq!(gizmo.size, 40.0);
        assert_eq!(gizmo.width, 3.0);
        assert_eq!(gizmo.opacity, 0.25);
        assert_eq!(gizmo.ghost_alpha, 0.3);
        assert_eq!(gizmo.ring_snap_deg, 45.0);
        assert!(!gizmo.show_readout);

        // The same request again is a no-op.
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
    }

    #[test]
    fn the_gizmo_switch_and_backend_summarise() {
        let mut state = ViewState::default();
        let off: ViewRequest = serde_json::from_str(r#"{"gizmo": {"enabled": false}}"#).unwrap();
        assert_eq!(state.apply(&off).unwrap(), "gizmo off");
        assert!(!state.gizmo.enabled);

        let on: ViewRequest = serde_json::from_str(r#"{"gizmo": {"enabled": true}}"#).unwrap();
        assert_eq!(state.apply(&on).unwrap(), "gizmo on");
        assert!(state.gizmo.enabled);

        let backend: ViewRequest =
            serde_json::from_str(r#"{"gizmo": {"backend": "  shader  "}}"#).unwrap();
        assert_eq!(state.apply(&backend).unwrap(), "gizmo backend shader");
        assert_eq!(state.gizmo.backend, "shader", "the backend is trimmed");
        assert_eq!(state.apply(&backend).unwrap(), "view unchanged");
    }

    #[test]
    fn a_bad_gizmo_field_is_rejected_atomically() {
        let mut state = ViewState::default();
        let bad_colour: ViewRequest = serde_json::from_str(
            r##"{"grid": true, "gizmo": {"enabled": false, "active_color": "not a colour"}}"##,
        )
        .unwrap();
        assert!(state.apply(&bad_colour).is_err());
        assert!(!state.grid, "nothing was applied");
        assert!(state.gizmo.enabled, "the flag was not applied");
        assert_eq!(state.gizmo, GizmoStyle::default());

        for request in [
            r#"{"grid": true, "gizmo": {"size": 0.0}}"#,
            r#"{"grid": true, "gizmo": {"size": -1.0}}"#,
            r#"{"grid": true, "gizmo": {"width": 0.0}}"#,
            r#"{"grid": true, "gizmo": {"backend": "   "}}"#,
        ] {
            let request: ViewRequest = serde_json::from_str(request).unwrap();
            assert!(state.apply(&request).is_err(), "{request:?}");
            assert!(!state.grid, "nothing was applied");
            assert_eq!(state.gizmo, GizmoStyle::default());
        }

        // JSON has no NaN, so the degenerate width is built directly.
        let nan = ViewRequest {
            grid: Some(true),
            gizmo: Some(GizmoRequest {
                width: Some(f64::NAN),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(state.apply(&nan).is_err());
        assert!(!state.grid, "nothing was applied");
        assert_eq!(state.gizmo, GizmoStyle::default());
    }

    #[test]
    fn gizmo_alphas_and_the_ring_snap_clamp() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r#"{"gizmo": {"opacity": 2.0, "ghost_alpha": -1.0, "ring_snap_deg": 400.0}}"#,
        )
        .unwrap();
        assert_eq!(state.apply(&request).unwrap(), "gizmo style");
        assert_eq!(state.gizmo.opacity, 1.0);
        assert_eq!(state.gizmo.ghost_alpha, 0.0);
        assert_eq!(state.gizmo.ring_snap_deg, 180.0);
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");

        // Above half, an occluded handle reads as a front handle: the ceiling is 0.5.
        let ghost: ViewRequest =
            serde_json::from_str(r#"{"gizmo": {"ghost_alpha": 0.9}}"#).unwrap();
        assert_eq!(state.apply(&ghost).unwrap(), "gizmo style");
        assert_eq!(state.gizmo.ghost_alpha, 0.5);

        // The detent is stepped to 5° multiples (and 0 stays continuous).
        for (input, applied) in [(7.0, 5.0), (12.4, 10.0), (180.0, 180.0), (0.0, 0.0)] {
            let snap: ViewRequest = ViewRequest {
                gizmo: Some(GizmoRequest {
                    ring_snap_deg: Some(input),
                    ..Default::default()
                }),
                ..Default::default()
            };
            state.apply(&snap).unwrap();
            assert_eq!(state.gizmo.ring_snap_deg, applied, "{input}°");
        }
    }

    #[test]
    fn the_accessibility_knobs_apply_and_clamp() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r##"{"gizmo": {
                "outline_color": "#010203",
                "halo_width": 3.0, "hover_scale": 2.0, "hit_radius_px": 20.0
            }}"##,
        )
        .unwrap();
        assert_eq!(state.apply(&request).unwrap(), "gizmo style");
        let mut outline = grid::parse_srgb_hex("#010203").unwrap();
        outline[3] = 1.0;
        assert_eq!(state.gizmo.outline, outline, "the outline alpha is kept");
        assert_eq!(state.gizmo.halo_width, 3.0);
        assert_eq!(state.gizmo.hover_scale, 2.0);
        assert_eq!(state.gizmo.hit_radius_px, 20.0);
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");

        // A grab radius below 24 px is clamped up to the WCAG 2.5.8 floor.
        let small: ViewRequest =
            serde_json::from_str(r#"{"gizmo": {"hit_radius_px": 2.0}}"#).unwrap();
        assert_eq!(state.apply(&small).unwrap(), "gizmo style");
        assert_eq!(state.gizmo.hit_radius_px, 12.0);

        // The halo and hover scale have ceilings so the gizmo cannot blot the canvas.
        let large: ViewRequest =
            serde_json::from_str(r#"{"gizmo": {"halo_width": 40.0, "hover_scale": 9.0}}"#).unwrap();
        state.apply(&large).unwrap();
        assert_eq!(state.gizmo.halo_width, 8.0);
        assert_eq!(state.gizmo.hover_scale, 4.0);
    }

    #[test]
    fn a_hover_scale_below_one_is_rejected() {
        let mut state = ViewState::default();
        let request: ViewRequest =
            serde_json::from_str(r#"{"grid": true, "gizmo": {"hover_scale": 0.5}}"#).unwrap();
        assert!(state.apply(&request).is_err());
        assert!(!state.grid, "nothing was applied");
        assert_eq!(state.gizmo, GizmoStyle::default());
    }

    #[test]
    fn a_request_can_change_the_gizmo_and_the_grid() {
        let mut state = ViewState::default();
        let request: ViewRequest = serde_json::from_str(
            r#"{"grid": true, "gizmo": {"enabled": false, "opacity": 0.5}}"#,
        )
        .unwrap();
        assert_eq!(state.apply(&request).unwrap(), "grid on, gizmo off");
        assert!(state.grid);
        assert!(!state.gizmo.enabled);
        assert_eq!(state.gizmo.opacity, 0.5);
    }

    #[test]
    fn projections_accept_both_spellings() {
        assert_eq!(
            ProjectionName::parse("ORTHO").unwrap(),
            ProjectionName::Orthographic
        );
        assert_eq!(
            ProjectionName::parse("perspective").unwrap(),
            ProjectionName::Perspective
        );
    }

    #[test]
    fn the_grid_steps_apply_and_summarise() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid_minor: Some(5.0),
            grid_major: Some(50.0),
            grid_radius: Some(250.0),
            ..Default::default()
        };
        assert_eq!(
            state.apply(&request).unwrap(),
            "grid minor 5, grid major 50, grid radius 250"
        );
        assert_eq!(state.grid_minor, 5.0);
        assert_eq!(state.grid_major, 50.0);
        assert_eq!(state.grid_radius, Some(250.0));

        // A zero radius restores the camera-derived extent.
        let auto = ViewRequest {
            grid_radius: Some(0.0),
            ..Default::default()
        };
        assert_eq!(state.apply(&auto).unwrap(), "grid radius auto");
        assert_eq!(state.grid_radius, None);
    }

    #[test]
    fn an_invalid_step_leaves_everything_untouched() {
        let mut state = ViewState::default();
        let zero = ViewRequest {
            grid: Some(true),
            grid_minor: Some(0.0),
            ..Default::default()
        };
        assert!(state.apply(&zero).is_err());
        assert!(!state.grid, "nothing was applied");

        // The major step may not sit below the minor one (or below the existing minor).
        let inverted = ViewRequest {
            grid_minor: Some(10.0),
            grid_major: Some(1.0),
            ..Default::default()
        };
        assert!(state.apply(&inverted).is_err());
        assert_eq!(state.grid_major, grid::MAJOR_STEP);
    }

    #[test]
    fn the_grid_ink_applies_and_summarises() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid_color: Some("#F87171".to_string()),
            grid_alpha: Some(0.5),
            ..Default::default()
        };
        assert_eq!(
            state.apply(&request).unwrap(),
            "grid colour #f87171, grid alpha 0.5"
        );
        assert_eq!(
            state.grid_color,
            grid::parse_srgb_hex("#f87171").unwrap(),
            "the state holds linear ink"
        );
        assert_eq!(state.grid_alpha, 0.5);

        // The same colour in another spelling is a no-op.
        let again = ViewRequest {
            grid_color: Some("#f87171".to_string()),
            ..Default::default()
        };
        assert_eq!(state.apply(&again).unwrap(), "view unchanged");
    }

    #[test]
    fn a_bad_ink_or_alpha_leaves_the_state_untouched() {
        let mut state = ViewState::default();
        let bad_colour = ViewRequest {
            grid: Some(true),
            grid_color: Some("not a colour".to_string()),
            ..Default::default()
        };
        assert!(state.apply(&bad_colour).is_err());
        assert!(!state.grid, "nothing was applied");
        assert_eq!(state.grid_color, grid::GRID_COLOR);

        for alpha in [1.5, -0.1, f32::NAN, f32::INFINITY] {
            let request = ViewRequest {
                grid: Some(true),
                grid_alpha: Some(alpha),
                ..Default::default()
            };
            assert!(state.apply(&request).is_err(), "alpha {alpha}");
            assert!(!state.grid, "nothing was applied");
            assert_eq!(state.grid_alpha, 1.0);
        }
    }

    #[test]
    fn the_default_ink_spelling_is_not_a_change() {
        // `GRID_COLOR` is the token rounded to three decimals, so the hex spelling must
        // not report a change on a fresh state.
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid_color: Some("#94a3b8".to_string()),
            ..Default::default()
        };
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
        assert_eq!(state.grid_color, grid::GRID_COLOR);

        // The comparison is exact at every 8-bit step, not a coarse tolerance.
        for hex in ["#010101", "#020202"] {
            let dark = ViewRequest {
                grid_color: Some(hex.to_string()),
                ..Default::default()
            };
            assert_eq!(state.apply(&dark).unwrap(), format!("grid colour {hex}"));
        }
    }

    #[test]
    fn a_workplane_request_is_validated_but_not_state() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            workplane: Some("Activate".to_string()),
            ..Default::default()
        };
        assert_eq!(state.apply(&request).unwrap(), "view unchanged");
        assert_eq!(
            request.workplane_action().unwrap(),
            Some(WorkplaneAction::Activate)
        );

        // The datum rows and the in-situ verbs parse from the same single string the
        // Python command returns, case- and whitespace-insensitively.
        for (text, action) in [
            ("world:xy", WorkplaneAction::Standard(StandardPlane::Top)),
            (" World:XZ ", WorkplaneAction::Standard(StandardPlane::Front)),
            ("world:yz", WorkplaneAction::Standard(StandardPlane::Right)),
            ("yz", WorkplaneAction::Standard(StandardPlane::Right)),
            ("camera", WorkplaneAction::Camera),
            ("view-plane", WorkplaneAction::Camera),
            ("selection", WorkplaneAction::Selection),
        ] {
            assert_eq!(WorkplaneAction::parse(text).unwrap(), action, "{text}");
        }

        let bad = ViewRequest {
            grid: Some(true),
            workplane: Some("sketch".to_string()),
            ..Default::default()
        };
        assert!(state.apply(&bad).is_err());
        assert!(!state.grid, "nothing was applied");
        assert!(bad.workplane_action().is_err());
    }

    #[test]
    fn the_grid_overlay_flag_applies_and_summarises() {
        let mut state = ViewState::default();
        let on = ViewRequest {
            grid_overlay: Some(true),
            ..Default::default()
        };
        assert_eq!(
            state.apply(&on).unwrap(),
            "grid overlay on (native ground hidden)"
        );
        assert!(state.grid_overlay);

        // Re-applying is a no-op; turning it off restores the native grid.
        assert_eq!(state.apply(&on).unwrap(), "view unchanged");
        let off = ViewRequest {
            grid_overlay: Some(false),
            ..Default::default()
        };
        assert_eq!(state.apply(&off).unwrap(), "grid overlay off (native grid)");
        assert!(!state.grid_overlay);
    }

    #[test]
    fn a_radius_is_ignored_while_the_shader_grid_is_on() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid_overlay: Some(true),
            grid_radius: Some(250.0),
            ..Default::default()
        };
        let summary = state.apply(&request).unwrap();
        assert!(summary.contains("grid overlay on"), "{summary}");
        assert!(
            summary.contains("grid radius ignored (shader grid)"),
            "{summary}"
        );
    }

    #[test]
    fn a_bad_request_does_not_switch_the_grid_authority() {
        let mut state = ViewState::default();
        let bad = ViewRequest {
            grid_overlay: Some(true),
            projection: Some("isometric".to_string()),
            ..Default::default()
        };
        assert!(state.apply(&bad).is_err());
        assert!(!state.grid_overlay, "validate before mutate");
    }

    #[test]
    fn the_grid_overlay_flag_deserializes() {
        let request: ViewRequest = serde_json::from_str(r#"{"grid_overlay": true}"#).unwrap();
        assert_eq!(request.grid_overlay, Some(true));
        let empty: ViewRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.grid_overlay, None);
    }
}
