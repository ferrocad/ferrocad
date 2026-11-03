//! The browser's engine: a small Rust [`AppBackend`] over an in-memory sample.
//!
//! There is no `ferrocad_core::Document` yet (M4 wires the real one through
//! `ferrocad_part`/OCCT). This backend exists to make the shell real: a document with
//! two boxes, editable property rows, and the header commands the chrome expects. It
//! also proves the seam — the shell cannot tell it from the CPython one.

use std::cell::RefCell;
use std::collections::HashMap;

use ferrocad_shell_core::app::AppBackend;
use ferrocad_shell_core::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_core::types::{
    Bootstrap, DocumentNode, EvalResult, ObjectNode, PropRow, SetResult,
};
use ferrocad_shell_core::view::{GizmoRequest, ViewRequest, ViewState};

/// Document and command ids shared with the native registry, so the chrome's controls
/// and the app menu resolve exactly as they do natively.
const GRID: &str = "App_ToggleGrid";
const AXIS: &str = "App_ToggleAxisCross";
const WORKPLANE: &str = "App_ViewWorkplane";
const GIZMO: &str = "App_ToggleGizmo";
const SNAP: &str = "Draft_Snap_Lock";
const DEV_TOOLS: &str = "Dev_ToggleTools";
const FIT: &str = "App_Fit";

/// The sample document's name.
const DOCUMENT: &str = "Sample";

/// One sample object: its label and its three dimensions.
#[derive(Debug, Clone, Copy)]
struct SampleObject {
    label: &'static str,
    size: [f64; 3],
}

/// The mutable half of the backend: the view state the commands toggle, the snap
/// switch, and any property values the user edited.
#[derive(Default)]
struct State {
    view: ViewState,
    snap: bool,
    values: HashMap<(String, String), String>,
}

impl State {
    fn new() -> Self {
        Self {
            // The shell mirrors its boot state back through `set_view_state`; start
            // from the same defaults so the first command list agrees.
            view: ViewState::default(),
            snap: false,
            values: HashMap::new(),
        }
    }

    fn value(&self, object: &str, property: &str, fallback: impl Into<String>) -> String {
        self.values
            .get(&(object.to_string(), property.to_string()))
            .cloned()
            .unwrap_or_else(|| fallback.into())
    }
}

/// The browser's `AppBackend`.
pub struct SampleBackend {
    state: RefCell<State>,
}

impl SampleBackend {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(State::new()),
        }
    }

    /// The sample objects, by internal name.
    fn objects() -> &'static [(&'static str, SampleObject)] {
        &[
            (
                "Box",
                SampleObject {
                    label: "Box",
                    size: [10.0, 10.0, 10.0],
                },
            ),
            (
                "Box001",
                SampleObject {
                    label: "Box001",
                    size: [20.0, 12.0, 6.0],
                },
            ),
        ]
    }

    fn object(name: &str) -> Option<SampleObject> {
        Self::objects()
            .iter()
            .find(|(id, _)| *id == name)
            .map(|(_, object)| *object)
    }
}

impl Default for SampleBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AppBackend for SampleBackend {
    fn bootstrap(&mut self) -> Result<Bootstrap, String> {
        Ok(Bootstrap {
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "rust".to_string(),
            document: DOCUMENT.to_string(),
        })
    }

    fn hello(&self) -> Result<String, String> {
        Ok("FerroCAD browser shell · Rust backend (no CPython)".to_string())
    }

    fn model_tree(&self) -> Result<Vec<DocumentNode>, String> {
        Ok(vec![DocumentNode {
            name: DOCUMENT.to_string(),
            label: DOCUMENT.to_string(),
            objects: Self::objects()
                .iter()
                .map(|(name, object)| ObjectNode {
                    name: name.to_string(),
                    label: object.label.to_string(),
                    type_id: "Part::Box".to_string(),
                })
                .collect(),
        }])
    }

    fn properties(&self, object: &str) -> Result<Vec<PropRow>, String> {
        let Some(sample) = Self::object(object) else {
            return Ok(Vec::new());
        };
        let state = self.state.borrow();
        let rows = [
            ("Label", "App::PropertyString", "text", "mm", None, None),
            (
                "Length",
                "App::PropertyLength",
                "quantity",
                "mm",
                Some(0.1),
                Some(1000.0),
            ),
            (
                "Width",
                "App::PropertyLength",
                "quantity",
                "mm",
                Some(0.1),
                Some(1000.0),
            ),
            (
                "Height",
                "App::PropertyLength",
                "quantity",
                "mm",
                Some(0.1),
                Some(1000.0),
            ),
        ];
        Ok(rows
            .iter()
            .map(|(name, type_id, kind, unit, min, max)| {
                let fallback = match *name {
                    "Label" => sample.label.to_string(),
                    "Length" => format!("{:.1} mm", sample.size[0]),
                    "Width" => format!("{:.1} mm", sample.size[1]),
                    _ => format!("{:.1} mm", sample.size[2]),
                };
                PropRow {
                    name: (*name).to_string(),
                    label: String::new(),
                    type_id: (*type_id).to_string(),
                    value: state.value(object, name, fallback),
                    status: String::new(),
                    editable: true,
                    command: None,
                    kind: (*kind).to_string(),
                    min: *min,
                    max: *max,
                    unit: (!unit.is_empty()).then(|| (*unit).to_string()),
                    choices: Vec::new(),
                }
            })
            .collect())
    }

    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        let state = self.state.borrow();
        let mut commands = vec![
            command(GRID, "Grid", "grid", true, state.view.grid),
            command(AXIS, "Axes", "axis", true, state.view.axis_cross),
            command(WORKPLANE, "WP", "workplane", false, false),
            command(GIZMO, "Gizmo", "gizmo", true, state.view.gizmo.enabled),
            command(SNAP, "Snap", "snap", true, state.snap),
            command(DEV_TOOLS, "Dev Tools", "hidden", false, false),
            command(FIT, "Fit", "dock", false, false),
        ];
        // The app menu lists the first entry by id; give Fit a friendly tooltip too.
        for command in &mut commands {
            command.cmd_type = "ForView".to_string();
            command.group = "View".to_string();
        }
        if let Some(fit) = commands.iter_mut().find(|command| command.id == FIT) {
            fit.tooltip = "Frame the document (no viewport yet)".to_string();
        }
        Ok(commands)
    }

    fn set_view_state(&self, state: &ViewState) -> Result<(), String> {
        self.state.borrow_mut().view = state.clone();
        Ok(())
    }

    fn set_selection(&self, _items: &[ferrocad_shell_core::SelectionItem]) -> Result<(), String> {
        // The browser sample has no selection facade; the shell owns the selection.
        Ok(())
    }

    fn evaluate(&self, _code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: false,
            output: "the browser build has no Python interpreter".to_string(),
        })
    }

    fn set_property(
        &mut self,
        object: &str,
        property: &str,
        value: &str,
    ) -> Result<SetResult, String> {
        if Self::object(object).is_none() {
            return Ok(SetResult {
                ok: false,
                error: format!("no object {object}"),
                view: None,
            });
        }
        if !matches!(property, "Label" | "Length" | "Width" | "Height") {
            return Ok(SetResult {
                ok: false,
                error: format!("{property} is read-only"),
                view: None,
            });
        }
        self.state
            .borrow_mut()
            .values
            .insert((object.to_string(), property.to_string()), value.to_string());
        Ok(SetResult {
            ok: true,
            error: String::new(),
            view: None,
        })
    }

    fn run_command(&mut self, id: &str) -> Result<CommandOutcome, String> {
        let mut state = self.state.borrow_mut();
        let (status, view) = match id {
            GRID => {
                state.view.grid = !state.view.grid;
                (
                    format!("grid {}", if state.view.grid { "on" } else { "off" }),
                    Some(ViewRequest {
                        grid: Some(state.view.grid),
                        ..Default::default()
                    }),
                )
            }
            AXIS => {
                let on = !state.view.axis_cross;
                state.view.axis.enabled = on;
                (
                    format!("axes {}", if on { "on" } else { "off" }),
                    Some(ViewRequest {
                        axis_cross: Some(on),
                        ..Default::default()
                    }),
                )
            }
            GIZMO => {
                let on = !state.view.gizmo.enabled;
                (
                    format!("gizmo {}", if on { "on" } else { "off" }),
                    Some(ViewRequest {
                        gizmo: Some(GizmoRequest {
                            enabled: Some(on),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                )
            }
            SNAP => {
                state.snap = !state.snap;
                (
                    format!("snap {}", if state.snap { "on" } else { "off" }),
                    None,
                )
            }
            WORKPLANE => ("workplane: no viewport yet".to_string(), None),
            FIT => ("fit: no viewport yet".to_string(), None),
            other => {
                return Ok(CommandOutcome {
                    ok: false,
                    error: format!("{other} is not available in the browser build"),
                    ..Default::default()
                });
            }
        };
        Ok(CommandOutcome {
            ok: true,
            status,
            view,
            ..Default::default()
        })
    }

    fn refresh_providers(&mut self, _feature: &str) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }
}

/// One header command. Checkable controls report their state through `checked`.
fn command(id: &str, menu: &str, surface: &str, checkable: bool, checked: bool) -> CommandInfo {
    CommandInfo {
        id: id.to_string(),
        menu: menu.to_string(),
        tooltip: menu.to_string(),
        accel: String::new(),
        group: "View".to_string(),
        cmd_type: "ForView".to_string(),
        surface: surface.to_string(),
        short: menu.to_string(),
        tone: String::new(),
        parent: String::new(),
        wheel: false,
        enabled: true,
        checked,
        checkable,
        dev_only: false,
    }
}
