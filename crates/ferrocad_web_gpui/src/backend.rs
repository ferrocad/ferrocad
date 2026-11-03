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
use ferrocad_shell_core::display::StageMesh;
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

/// One sample object: its label, its three dimensions and where it sits.
#[derive(Debug, Clone, Copy)]
struct SampleObject {
    label: &'static str,
    size: [f64; 3],
    /// The world-space position of the box's centre.
    centre: [f64; 3],
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
                    centre: [0.0, 0.0, 0.0],
                },
            ),
            (
                "Box001",
                SampleObject {
                    label: "Box001",
                    size: [20.0, 12.0, 6.0],
                    // Beside `Box`, so both are in frame and a pick can tell them apart.
                    centre: [20.0, 0.0, 0.0],
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

    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        Ok(Self::objects()
            .iter()
            .enumerate()
            .map(|(index, (name, object))| box_mesh(index as u32 + 1, name, *object))
            .collect())
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
            fit.tooltip = "Frame the document".to_string();
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
        self.state.borrow_mut().values.insert(
            (object.to_string(), property.to_string()),
            value.to_string(),
        );
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
            WORKPLANE => (
                "workplane: not wired in the browser build".to_string(),
                None,
            ),
            FIT => ("fit: the sample is framed on load".to_string(), None),
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

/// One box as the display seam's mesh: eight shared corners, twelve triangles and the
/// twelve edges the shaded-with-edges mode draws.
///
/// There is no kernel in this build, so the sample's boxes are tessellated here. The
/// shape is the same one a `Part::Box` would produce: axis-aligned, world-space, with
/// one `parts` entry per face and per edge so a pick can name the sub-element.
fn box_mesh(entity: u32, name: &str, object: SampleObject) -> StageMesh {
    let [hx, hy, hz] = [
        (object.size[0] / 2.0) as f32,
        (object.size[1] / 2.0) as f32,
        (object.size[2] / 2.0) as f32,
    ];
    let [cx, cy, cz] = [
        object.centre[0] as f32,
        object.centre[1] as f32,
        object.centre[2] as f32,
    ];
    // Corner index bits: bit 0 = +x, bit 1 = +y, bit 2 = +z.
    let positions: Vec<[f32; 3]> = (0..8)
        .map(|corner: u8| {
            let x = if corner & 1 != 0 { hx } else { -hx };
            let y = if corner & 2 != 0 { hy } else { -hy };
            let z = if corner & 4 != 0 { hz } else { -hz };
            [cx + x, cy + y, cz + z]
        })
        .collect();

    // Two triangles per face, one `parts` entry per face. Winding is irrelevant: the
    // renderer draws with culling off and shades flat (no normals).
    let faces: [[u32; 4]; 6] = [
        [4, 5, 6, 7], // +z
        [1, 0, 3, 2], // -z
        [0, 4, 7, 3], // -x
        [5, 1, 2, 6], // +x
        [7, 6, 2, 3], // +y
        [0, 1, 5, 4], // -y
    ];
    let mut triangles = Vec::with_capacity(36);
    let mut triangle_parts = Vec::with_capacity(12);
    for (part, [a, b, c, d]) in faces.iter().enumerate() {
        triangles.extend_from_slice(&[*a, *b, *c, *a, *c, *d]);
        triangle_parts.extend_from_slice(&[part as u32, part as u32]);
    }

    // The twelve edges, as corner pairs, each its own part after the six faces.
    let edges: Vec<u32> = [
        (0, 1),
        (1, 3),
        (3, 2),
        (2, 0),
        (4, 5),
        (5, 7),
        (7, 6),
        (6, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ]
    .iter()
    .flat_map(|(a, b)| [*a, *b])
    .collect();
    let segment_parts: Vec<u32> = (0..12).map(|edge| edge + 6).collect();

    let mut parts = Vec::with_capacity(18);
    parts.extend((1..=6).map(|face| format!("Face{face}")));
    parts.extend((1..=12).map(|edge| format!("Edge{edge}")));

    StageMesh {
        entity,
        object: name.to_string(),
        label: object.label.to_string(),
        positions,
        normals: Vec::new(),
        triangles,
        triangle_parts,
        edges,
        segment_parts,
        vertices: Vec::new(),
        vertex_parts: Vec::new(),
        parts,
        // The placement's base point: the minimum corner, as a box built from a corner
        // reports it (the triad hugs it).
        origin: Some([cx - hx, cy - hy, cz - hz]),
        color: None,
        display: None,
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
