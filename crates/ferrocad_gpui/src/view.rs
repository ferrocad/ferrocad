//! View requests and the shell's presentation state (A5, extended in G5).
//!
//! A command that changes only presentation returns `{"view": {...}}` instead of
//! editing the document. The shell owns the state, applies what the viewport supports
//! (the projection, the grid and its steps), and mirrors the state to
//! `FreeCADGui.ViewState` so a checkable command's `IsChecked` can read it.
//!
//! A `workplane` request is an **action**, not state: it activates, clears or looks down
//! the active workplane. The shell performs it on the viewport and reports the outcome.

use ferrocad_stage::grid;
use serde::Deserialize;

/// A presentation change a command returns. Never undoable, never a document edit.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ViewRequest {
    /// Show or hide the reference grid.
    #[serde(default)]
    pub grid: Option<bool>,
    /// Show or hide the axis cross (the world triad, its origin node and axis tags).
    #[serde(default)]
    pub axis_cross: Option<bool>,
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
    /// A workplane action: `"activate"`, `"clear"` or `"view"`.
    #[serde(default)]
    pub workplane: Option<String>,
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
}

impl WorkplaneAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "activate" => Ok(Self::Activate),
            "clear" => Ok(Self::Clear),
            "view" => Ok(Self::View),
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

/// The shell's presentation state, mirrored to `FreeCADGui.ViewState`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub grid: bool,
    /// Whether the axis cross is shown (on by default, as upstream's is).
    pub axis_cross: bool,
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
        if let Some(axis_cross) = request.axis_cross {
            if self.axis_cross != axis_cross {
                self.axis_cross = axis_cross;
                changes.push(format!(
                    "axis cross {}",
                    if axis_cross { "on" } else { "off" }
                ));
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
