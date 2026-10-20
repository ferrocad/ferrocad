//! View requests and the shell's presentation state (A5).
//!
//! A command that changes only presentation returns `{"view": {...}}` instead of
//! editing the document. The shell owns the state, applies what the viewport already
//! supports (the projection), and mirrors the state to `FreeCADGui.ViewState` so a
//! checkable command's `IsChecked` can read it. The `grid` flag is carried for the
//! grid slice (G5); rendering it is that slice's job.

use serde::Deserialize;

/// A presentation change a command returns. Never undoable, never a document edit.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ViewRequest {
    /// Show or hide the reference grid.
    #[serde(default)]
    pub grid: Option<bool>,
    /// `"persp"`/`"perspective"` or `"ortho"`/`"orthographic"`.
    #[serde(default)]
    pub projection: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewState {
    pub grid: bool,
    pub projection: ProjectionName,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            grid: false,
            projection: ProjectionName::Perspective,
        }
    }
}

impl ViewState {
    /// Apply a request and summarise what changed (`"grid on"`, `"projection ortho"`).
    ///
    /// Everything is validated **before** anything is mutated, so an unknown
    /// projection leaves the state untouched.
    pub fn apply(&mut self, request: &ViewRequest) -> Result<String, String> {
        let projection = request
            .projection
            .as_deref()
            .map(ProjectionName::parse)
            .transpose()?;
        let mut changes = Vec::new();
        if let Some(grid) = request.grid {
            if self.grid != grid {
                self.grid = grid;
                changes.push(format!("grid {}", if grid { "on" } else { "off" }));
            }
        }
        if let Some(projection) = projection {
            if self.projection != projection {
                self.projection = projection;
                changes.push(format!("projection {}", projection.as_str()));
            }
        }
        Ok(if changes.is_empty() {
            "view unchanged".to_string()
        } else {
            changes.join(", ")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_a_perspective_view_without_a_grid() {
        let state = ViewState::default();
        assert!(!state.grid);
        assert_eq!(state.projection, ProjectionName::Perspective);
        assert_eq!(state.projection.as_str(), "persp");
    }

    #[test]
    fn a_request_applies_grid_and_projection() {
        let mut state = ViewState::default();
        let request = ViewRequest {
            grid: Some(true),
            projection: Some("orthographic".to_string()),
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
        };
        let error = state.apply(&request).expect_err("unknown projection");
        assert!(error.contains("isometric"), "{error}");
        assert!(!state.grid, "grid was not applied either");
        assert_eq!(state.projection, ProjectionName::Perspective);
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
}
