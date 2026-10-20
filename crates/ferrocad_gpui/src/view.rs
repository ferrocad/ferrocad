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
}
