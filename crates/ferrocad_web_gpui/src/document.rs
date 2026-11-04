//! The browser's engine: a real `ferrocad_core::Document` behind `AppBackend`.
//!
//! This replaces the M2 sample: the tree, the property rows and the viewport stage
//! are read from one document, and the geometry is built by the OCCT kernel compiled
//! to wasm. The sample document is a `Part::Box` with a `Part::Fillet` on one edge —
//! the smallest document that exercises a dependency (the fillet recomputes after
//! the box), a parametric feature (`Radius`) and a real tessellation.
//!
//! # What is real, and what is not
//!
//! Real: `Document::recompute`, the Part object types, the kernel, tessellation into
//! `StageMesh`es, picking (the shell's CPU ray cast over that stage), and property
//! edits through [`AppBackend::set_property`] followed by a recompute.
//!
//! Not yet: undo/redo (the browser has no UI for it), expressions, binary file I/O,
//! and anything scripted — there is no interpreter in this build, so `evaluate`
//! says so rather than pretending.

use std::cell::RefCell;
use std::sync::Arc;

use ferrocad_core::{
    Document, Matrix4, ObjectId, Placement, Property, Rotation, Vector3, status_names,
};
use ferrocad_geom::TessellationOptions;
use ferrocad_occt::OcctBackend;
use ferrocad_part::{BOX_TYPE, FILLET_TYPE, backend, shape_of};
use ferrocad_shell_core::app::AppBackend;
use ferrocad_shell_core::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_core::display::StageMesh;
use ferrocad_shell_core::selection::SelectionItem;
use ferrocad_shell_core::types::{
    ApplyResult, Bootstrap, DocumentNode, EvalResult, ObjectNode, OpenedDocument, PropRow,
    SetResult, VisibilityOutcome,
};
use ferrocad_shell_core::view::{GizmoRequest, ViewRequest, ViewState as ShellViewState};
use ferrocad_shell_core::{DialogField, DialogSpec};
use ferrocad_stage::{InteractionCommit, TRANSFORM_ID};

/// Document and command ids shared with the native registry, so the chrome's controls
/// and the app menu resolve exactly as they do natively.
const GRID: &str = "App_ToggleGrid";
const AXIS: &str = "App_ToggleAxisCross";
const WORKPLANE: &str = "App_ViewWorkplane";
const GIZMO: &str = "App_ToggleGizmo";
const SNAP: &str = "Draft_Snap_Lock";
const DEV_TOOLS: &str = "Dev_ToggleTools";
const FIT: &str = "App_Fit";
const MOVE: &str = "App_Move";
const ROTATE: &str = "App_Rotate";
const VISIBILITY: &str = "App_ToggleVisibility";
const UNDO: &str = "App_Undo";
const REDO: &str = "App_Redo";
const OPEN: &str = "App_Open";
const SAVE: &str = "App_Save";

/// The prefix of the dialog tokens this backend mints for `App_Move`.
///
/// The shell's Move form is the typed twin of the gizmo: `App_Move` returns a token
/// and `dialog_accept` applies the delta. The native backend hands the token to
/// Python; here it is only a marker, so any token carrying this prefix is ours.
const MOVE_TOKEN: &str = "browser-move";

/// The document property a hide adds on first use. The native backend writes the same
/// switch, and keeping it on the document is what makes a hide undoable.
const VISIBILITY_PROPERTY: &str = "Visibility";

/// The sample document's name (the window title and the status line use it).
const DOCUMENT: &str = "Sample";

/// The edge the fillet is applied to, as the canvas demo does.
const EDGE: &str = "Edge1";

/// The fillet radius, in millimetres. Large enough to see, well under the radius at
/// which the kernel refuses it (the demo's 10 mm box fails at 10 or more).
const RADIUS: f64 = 3.0;

/// The tessellation tolerance, in millimetres (the probe's).
const TOLERANCE: f64 = 0.2;

/// The opaque steel grey the desktop Part workbench gives document solids.
const DEFAULT_COLOR: [f32; 4] = [0.30, 0.34, 0.40, 1.0];

/// The mutable half of the backend: the document and the state the view commands
/// toggle.
struct State {
    document: Document,
    /// The document's display name: the sample's at boot, the file's after an open.
    /// It is what the next save writes and what the title shows.
    name: String,
    view: ShellViewState,
    snap: bool,
    /// The shell's selection, mirrored for `App_Move`: the typed move applies to the
    /// selection, not to a named target.
    selection: Vec<String>,
    /// How many Move dialogs this backend has minted, so two open forms cannot spend
    /// each other's token.
    move_dialogs: u64,
}

/// The browser's `AppBackend`, over a real document.
pub struct DocumentBackend {
    state: RefCell<State>,
    /// Why the sample document could not be built, if it could not.
    boot_error: Option<String>,
}

impl DocumentBackend {
    /// Compose Part with the wasm OCCT kernel and build the sample document.
    ///
    /// Must run after the module's static constructors (`__wasm_call_ctors`): the
    /// kernel's C++ runtime has to be initialised before the first call.
    pub fn new() -> Self {
        // Idempotent: the Part module keeps one backend per process.
        ferrocad_part::register(Arc::new(OcctBackend::new()));

        let mut document = Document::new();
        let boot_error = build_sample(&mut document).err();
        match boot_error.as_deref() {
            Some(error) => log::error!("ferrocad: the sample document did not build: {error}"),
            None => log_shapes(&document),
        }

        Self {
            state: RefCell::new(State {
                document,
                name: DOCUMENT.to_string(),
                view: ShellViewState::default(),
                snap: false,
                selection: Vec::new(),
                move_dialogs: 0,
            }),
            boot_error,
        }
    }

    /// The document's object for an internal name.
    fn object_id(state: &State, name: &str) -> Option<ObjectId> {
        state.document.get_by_name(name)
    }
}

impl Default for DocumentBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AppBackend for DocumentBackend {
    fn bootstrap(&mut self) -> Result<Bootstrap, String> {
        if let Some(error) = self.boot_error.clone() {
            return Err(error);
        }
        Ok(Bootstrap {
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "rust".to_string(),
            document: self.state.borrow().name.clone(),
        })
    }

    fn hello(&self) -> Result<String, String> {
        Ok("FerroCAD browser shell · Rust document backend · OCCT wasm · no CPython".to_string())
    }

    /// Replace the sample with a decoded `.FCStd` file.
    ///
    /// The host owns the picker, so this is the only place a document arrives from
    /// outside the build. The saved shapes are recomputed so a file from another build
    /// cannot rely on geometry it did not rebuild here.
    fn open_document(&mut self, name: &str, bytes: &[u8]) -> Result<OpenedDocument, String> {
        let saved = ferrocad_core::fcstd::decode_fcstd(bytes).map_err(|error| error.to_string())?;
        let mut document = Document::from_saved(&saved);
        document.recompute()?;
        let objects = document.object_ids().len();
        let mut state = self.state.borrow_mut();
        state.document = document;
        state.name = name.to_string();
        state.selection.clear();
        state.move_dialogs = 0;
        Ok(OpenedDocument {
            document: state.name.clone(),
            objects,
        })
    }

    fn save_document(&self) -> Result<Vec<u8>, String> {
        let state = self.state.borrow();
        let saved = state.document.to_saved(&state.name);
        ferrocad_core::fcstd_write::encode_fcstd(&saved).map_err(|error| error.to_string())
    }

    fn model_tree(&self) -> Result<Vec<DocumentNode>, String> {
        let state = self.state.borrow();
        let objects = state
            .document
            .object_ids()
            .iter()
            .filter_map(|id| state.document.object(*id))
            .map(|object| ObjectNode {
                name: object.name.clone(),
                label: object.label.clone(),
                type_id: object.type_id.clone(),
            })
            .collect();
        Ok(vec![DocumentNode {
            name: state.name.clone(),
            label: state.name.clone(),
            objects,
        }])
    }

    fn properties(&self, object: &str) -> Result<Vec<PropRow>, String> {
        let state = self.state.borrow();
        let Some(id) = Self::object_id(&state, object) else {
            return Ok(Vec::new());
        };
        let Some(object) = state.document.object(id) else {
            return Ok(Vec::new());
        };

        let mut rows = Vec::with_capacity(object.properties.len() + 1);
        // `Label` is a document-object field, not a property-container entry; FreeCAD
        // presents it as an ordinary row and so does the native backend.
        rows.push(PropRow {
            name: "Label".to_string(),
            label: String::new(),
            type_id: "App::PropertyString".to_string(),
            value: object.label.clone(),
            status: String::new(),
            editable: true,
            command: None,
            kind: "text".to_string(),
            min: None,
            max: None,
            unit: None,
            choices: Vec::new(),
        });

        for (name, value) in object.properties.iter() {
            // One rule for both hosts: the kind and the text come from the value, so
            // a Part length reads as `10 mm` here exactly as it does natively.
            let kind = ferrocad_core::present::kind(value);
            let driven = object.expressions.contains_key(name);
            let (min, max) = match value {
                Property::FloatConstraint { min, max, .. } => (Some(*min), Some(*max)),
                Property::IntegerConstraint { min, max, .. } => {
                    (Some(*min as f64), Some(*max as f64))
                }
                _ => (None, None),
            };
            rows.push(PropRow {
                name: name.clone(),
                label: String::new(),
                value: ferrocad_core::present::format(value),
                status: object
                    .properties
                    .status(name)
                    .map(|status| status_names(status).join(", "))
                    .unwrap_or_default(),
                editable: kind.is_some() && !driven,
                command: None,
                type_id: value.type_name().to_string(),
                kind: kind
                    .map(|kind| kind.as_str())
                    .unwrap_or_default()
                    .to_string(),
                min,
                max,
                // A document quantity carries its unit in the text; a schema row
                // declares one instead. `row_unit` reads either.
                unit: None,
                choices: match value {
                    Property::Enumeration(values, _) => values.clone(),
                    _ => Vec::new(),
                },
            });
        }
        Ok(rows)
    }

    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        let state = self.state.borrow();
        let state = &*state;
        let backend = backend();
        let mut meshes = Vec::new();
        // The engine decides what is drawn: a feature replaces the object it consumes
        // (the box under a fillet), and a hidden object is skipped. The native host
        // asks the same function, so the two cannot disagree.
        for id in ferrocad_core::drawable(&state.document) {
            let Some(object) = state.document.object(id) else {
                continue;
            };
            let Some(shape) = object
                .properties
                .get("Shape")
                .and_then(shape_of)
                .and_then(|property| property.shape())
            else {
                continue;
            };
            let mesh = backend
                .tessellate(&shape, &TessellationOptions::absolute(TOLERANCE))
                .map_err(|error| format!("{}: {error}", object.name))?;
            meshes.push(StageMesh {
                // A stable per-document pick id; `DisplaySync` maps it to the name.
                entity: id as u32 + 1,
                object: object.name.clone(),
                label: object.label.clone(),
                positions: mesh.positions,
                normals: mesh.normals,
                triangles: mesh.triangles,
                triangle_parts: mesh.triangle_parts,
                edges: mesh.edges,
                segment_parts: mesh.segment_parts,
                vertices: mesh.vertices,
                vertex_parts: mesh.vertex_parts,
                parts: mesh.parts,
                origin: placement_base(object),
                color: Some(object_color(object)),
                display: None,
            });
        }
        // A diagnostics line, not the per-shape tessellation counts `log_shapes`
        // prints: this is the set the stage actually draws, so the smoke check can
        // prove that a feature is not drawn beside the object it consumed.
        log::info!(
            "ferrocad: stage draws {}",
            meshes
                .iter()
                .map(|mesh| mesh.object.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(meshes)
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
            command(UNDO, "Undo", "dock", false, false),
            command(REDO, "Redo", "dock", false, false),
            command(OPEN, "Open…", "dock", false, false),
            command(SAVE, "Save As…", "dock", false, false),
            command(MOVE, "Move", "radial", false, false),
            command(ROTATE, "Rotate", "radial", false, false),
            command(
                VISIBILITY,
                if selection_visible(&state) == Some(false) {
                    "Show"
                } else {
                    "Hide"
                },
                "radial",
                false,
                false,
            ),
        ];
        // The app menu lists the first entry by id; give Fit a friendly tooltip too.
        for command in &mut commands {
            command.cmd_type = "ForView".to_string();
            command.group = "View".to_string();
        }
        if let Some(fit) = commands.iter_mut().find(|command| command.id == FIT) {
            fit.tooltip = "Frame the document".to_string();
        }
        // The object verbs are the right-click wheel's: Move opens the typed Move form
        // (the same one the gizmo's arrows fill), Rotate switches the triad's rings on,
        // and Hide/Show toggles the selection. Their accelerators are the native
        // registry's, so the muscle memory carries.
        for (id, accel, tooltip) in [
            (MOVE, "Ctrl+Shift+M", "Move the selected objects by a delta"),
            (ROTATE, "Ctrl+Shift+R", "Rotate the selected object"),
            (VISIBILITY, "", "Hide or show the selected objects"),
        ] {
            if let Some(command) = commands.iter_mut().find(|command| command.id == id) {
                command.cmd_type = "ForEdit".to_string();
                command.group = "Edit".to_string();
                command.tooltip = tooltip.to_string();
                command.accel = accel.to_string();
                command.wheel = true;
            }
        }
        // Undo and redo are plain edit commands: the action dock lists them and the
        // accelerators run them, but they are not wheel verbs.
        for (id, accel, tooltip) in [
            (UNDO, "Ctrl+Z", "Undo the last edit"),
            (REDO, "Ctrl+Shift+Z", "Redo the last undone edit"),
        ] {
            if let Some(command) = commands.iter_mut().find(|command| command.id == id) {
                command.cmd_type = "ForEdit".to_string();
                command.group = "Edit".to_string();
                command.tooltip = tooltip.to_string();
                command.accel = accel.to_string();
            }
        }
        // Undo/redo report their availability from the document's transaction stack.
        // The shell refreshes the tree, the properties and the stage after the run, so
        // an undone edit is removed everywhere, not just from the viewport.
        if let Some(command) = commands.iter_mut().find(|command| command.id == UNDO) {
            command.enabled = state.document.has_undo();
        }
        if let Some(command) = commands.iter_mut().find(|command| command.id == REDO) {
            command.enabled = state.document.has_redo();
        }
        // File verbs live in the dock and on the browser's own chords. Opening takes
        // the document's place, so it is gated like an edit; saving only reads it.
        for (id, accel, tooltip, cmd_type) in [
            (OPEN, "Ctrl+O", "Open a .FCStd document", "ForEdit"),
            (SAVE, "Ctrl+S", "Download the document as .FCStd", "ForView"),
        ] {
            if let Some(command) = commands.iter_mut().find(|command| command.id == id) {
                command.cmd_type = cmd_type.to_string();
                command.group = "File".to_string();
                command.tooltip = tooltip.to_string();
                command.accel = accel.to_string();
            }
        }
        Ok(commands)
    }

    fn set_view_state(&self, state: &ShellViewState) -> Result<(), String> {
        self.state.borrow_mut().view = state.clone();
        Ok(())
    }

    fn set_selection(&self, items: &[SelectionItem]) -> Result<(), String> {
        // The shell owns the selection; the document needs it for `App_Move`, whose
        // delta applies to whatever is selected rather than to a named target.
        let mut state = self.state.borrow_mut();
        state.selection = items.iter().map(|item| item.object.clone()).collect();
        state.selection.dedup();
        Ok(())
    }

    fn set_visibility(
        &mut self,
        object: &str,
        visible: Option<bool>,
    ) -> Result<VisibilityOutcome, String> {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let Some(id) = Self::object_id(state, object) else {
            return Ok(VisibilityOutcome {
                ok: false,
                visible: false,
                error: format!("no object {object}"),
            });
        };
        match in_transaction(&mut state.document, "Toggle visibility", |document| {
            hide_object(document, id, visible).ok_or_else(|| format!("no object {object}"))
        }) {
            Ok(next) => Ok(VisibilityOutcome {
                ok: true,
                visible: next,
                error: String::new(),
            }),
            Err(error) => Ok(VisibilityOutcome {
                ok: false,
                visible: false,
                error,
            }),
        }
    }

    fn rotation_quaternion(&self, object: &str) -> Option<[f64; 4]> {
        // The stage is world-space with an identity placement, so the object's own
        // orientation is only in the document. `Rotation::q` is already `[w, x, y, z]`.
        let state = self.state.borrow();
        let id = state.document.get_by_name(object)?;
        Some(current_placement(&state.document, id)?.rotation.q)
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
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let Some(id) = Self::object_id(state, object) else {
            return Ok(refused(format!("no object {object}")));
        };

        if property == "Label" {
            in_transaction(&mut state.document, "Rename", |document| {
                document.set_label(id, value);
                Ok(())
            })?;
            return Ok(SetResult {
                ok: true,
                error: String::new(),
                view: None,
            });
        }

        let Some(current) = state
            .document
            .object(id)
            .and_then(|object| object.properties.get(property))
            .cloned()
        else {
            return Ok(refused(format!("no property {property}")));
        };
        if state
            .document
            .object(id)
            .is_some_and(|object| object.expressions.contains_key(property))
        {
            return Ok(refused(format!("{property} is set by an expression")));
        }
        let next = match ferrocad_core::present::coerce(&current, value) {
            Ok(next) => next,
            Err(error) => return Ok(refused(error)),
        };
        // One named transaction per edit: the property pane, the typed Move and a
        // transform commit all land in the undo history, as they do natively.
        state.document.open_transaction_named("Set property");
        if let Err(error) = state.document.set_property(id, property, next) {
            state.document.abort_transaction();
            return Ok(refused(error));
        }
        let recomputed = state.document.recompute();
        state.document.commit_transaction();
        match recomputed {
            Ok(_) => {
                // A feature that refuses its input keeps its last good shape and
                // reports the reason. That is document state, not a failed edit:
                // the value was stored, so the pane keeps it.
                if let Some(error) = state
                    .document
                    .object(id)
                    .and_then(|object| object.invalid.then(|| object.error.clone()))
                    .flatten()
                {
                    return Ok(SetResult {
                        ok: true,
                        error,
                        view: None,
                    });
                }
                Ok(SetResult {
                    ok: true,
                    error: String::new(),
                    view: None,
                })
            }
            Err(error) => Ok(refused(error)),
        }
    }

    fn run_command(&mut self, id: &str) -> Result<CommandOutcome, String> {
        // The file verbs are the host's half of I/O: the browser owns the picker and
        // the download, the backend owns the bytes. They run before the state borrow
        // because `save_document` borrows it again.
        if id == OPEN {
            return Ok(match crate::host::request_open() {
                Ok(()) => CommandOutcome {
                    ok: true,
                    status: "open: choose a .FCStd file".to_string(),
                    ..Default::default()
                },
                Err(error) => CommandOutcome {
                    ok: false,
                    error: format!("open: {error:?}"),
                    ..Default::default()
                },
            });
        }
        if id == SAVE {
            let name = self.state.borrow().name.clone();
            return Ok(match self.save_document() {
                Ok(bytes) => match crate::host::download(&name, &bytes) {
                    Ok(()) => CommandOutcome {
                        ok: true,
                        status: format!("saved {name} ({} bytes)", bytes.len()),
                        ..Default::default()
                    },
                    Err(error) => CommandOutcome {
                        ok: false,
                        error: format!("save: {error:?}"),
                        ..Default::default()
                    },
                },
                Err(error) => CommandOutcome {
                    ok: false,
                    error,
                    ..Default::default()
                },
            });
        }
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        // The two object verbs are not view toggles: they hand the shell a session
        // (a gizmo mode, and for Move a token its form spends).
        if id == MOVE {
            state.move_dialogs += 1;
            let token = format!("{MOVE_TOKEN}-{}", state.move_dialogs);
            return Ok(CommandOutcome {
                ok: true,
                status: "move: drag a handle or type a value".to_string(),
                gizmo: Some("translate".to_string()),
                dialog: Some(move_dialog()),
                token: Some(token),
                ..Default::default()
            });
        }
        if id == ROTATE {
            return Ok(CommandOutcome {
                ok: true,
                status: "rotate: drag a ring".to_string(),
                gizmo: Some("rotate".to_string()),
                ..Default::default()
            });
        }
        if id == VISIBILITY {
            let selection = state.selection.clone();
            if selection.is_empty() {
                return Ok(CommandOutcome {
                    ok: false,
                    error: "nothing selected".to_string(),
                    ..Default::default()
                });
            }
            // One verb for the whole selection: if anything is still visible, hide
            // them all; otherwise show them all. One transaction, so one undo step.
            let hide = selection_visible(state) != Some(false);
            let verb = if hide { "Hide" } else { "Show" };
            let changed = in_transaction(&mut state.document, verb, |document| {
                let mut changed = 0;
                for name in &selection {
                    let Some(id) = document.get_by_name(name) else {
                        continue;
                    };
                    if hide_object(document, id, Some(!hide)).is_some() {
                        changed += 1;
                    }
                }
                Ok(changed)
            })?;
            let verb = if hide { "hid" } else { "showed" };
            return Ok(CommandOutcome {
                ok: true,
                status: format!("{verb} {changed} object(s)"),
                ..Default::default()
            });
        }
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
            FIT => (
                "fit".to_string(),
                Some(ViewRequest {
                    fit: Some(true),
                    ..Default::default()
                }),
            ),
            UNDO => {
                let name = state
                    .document
                    .undo_names()
                    .first()
                    .cloned()
                    .unwrap_or_default();
                if !state.document.undo() {
                    return Ok(CommandOutcome {
                        ok: false,
                        error: "nothing to undo".to_string(),
                        ..Default::default()
                    });
                }
                state.document.recompute()?;
                (history_status("undo", &name), None)
            }
            REDO => {
                let name = state
                    .document
                    .redo_names()
                    .first()
                    .cloned()
                    .unwrap_or_default();
                if !state.document.redo() {
                    return Ok(CommandOutcome {
                        ok: false,
                        error: "nothing to redo".to_string(),
                        ..Default::default()
                    });
                }
                state.document.recompute()?;
                (history_status("redo", &name), None)
            }
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

    /// The typed half of the Move gesture: apply `(dx, dy, dz)` to the selection.
    ///
    /// The shell's Move form and the gizmo arrows commit through this one call, so a
    /// typed move and a dragged one cannot drift. The token is the one `App_Move`
    /// handed out; there is no pending Python callback to spend.
    fn dialog_accept(
        &mut self,
        token: &str,
        values: &serde_json::Value,
    ) -> Result<CommandOutcome, String> {
        if !token.starts_with(MOVE_TOKEN) {
            return Ok(CommandOutcome {
                ok: false,
                error: format!("{token} is not a dialog this build opened"),
                ..Default::default()
            });
        }
        let delta = Vector3::new(
            values
                .get("dx")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
            values
                .get("dy")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
            values
                .get("dz")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
        );
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let selection = state.selection.clone();
        if selection.is_empty() {
            return Ok(CommandOutcome {
                ok: false,
                error: "nothing selected".to_string(),
                ..Default::default()
            });
        }
        match translate(state, &selection, delta) {
            Ok(moved) => Ok(CommandOutcome {
                ok: true,
                status: format!("moved {moved} object(s)"),
                ..Default::default()
            }),
            Err(error) => Ok(CommandOutcome {
                ok: false,
                error,
                ..Default::default()
            }),
        }
    }

    fn dialog_cancel(&mut self, _token: &str) -> Result<CommandOutcome, String> {
        // Nothing is pending on this side: the values come back through
        // `dialog_accept`, and a cancelled form simply writes nothing.
        Ok(CommandOutcome::default())
    }

    /// A manipulator commit: the rotation rings' `transform`/`apply`, and nothing else.
    ///
    /// The contract matches the native workbench's: `values` are the 16 row-major
    /// elements of the **world-space** transform, composed on the left of the object's
    /// placement (`delta · placement`). Dress-up manipulators (fillet/chamfer handles)
    /// need a Part workbench behind them; the sample document has no handles to grab.
    fn apply_interaction(
        &mut self,
        commit: &InteractionCommit,
        object: &str,
    ) -> Result<ApplyResult, String> {
        if commit.manipulator != TRANSFORM_ID || commit.action != "apply" {
            return Ok(ApplyResult {
                ok: false,
                error: format!(
                    "{} {} is not available in the browser build",
                    commit.manipulator, commit.action
                ),
            });
        }
        let Ok(elements) = <[f64; 16]>::try_from(commit.values.as_slice()) else {
            return Ok(ApplyResult {
                ok: false,
                error: "a transform commit needs 16 matrix values".to_string(),
            });
        };
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let Some(id) = Self::object_id(state, object) else {
            return Ok(ApplyResult {
                ok: false,
                error: format!("no object {object}"),
            });
        };
        let Some(placement) = current_placement(&state.document, id) else {
            return Ok(ApplyResult {
                ok: false,
                error: format!("{object} has no Placement"),
            });
        };
        let delta = Matrix4 { m: elements };
        let composed = delta.mul(&placement.to_matrix());
        let next = Placement::new(
            Vector3::new(composed.m[3], composed.m[7], composed.m[11]),
            Rotation::from_matrix(&composed),
        );
        // One transaction per commit, so a rotate ring drag is one undo step.
        in_transaction(&mut state.document, "Transform", |document| {
            document.set_property(id, "Placement", Property::Placement(next))?;
            document.recompute()?;
            Ok(())
        })?;
        Ok(ApplyResult {
            ok: true,
            error: String::new(),
        })
    }
}

/// The Move form: the same floating HUD the native `App_Move` opens.
///
/// `preview: "translation"` is what tells the shell to present it as the hand's
/// always-on form (and to drop the token it handed in) rather than a modal. The
/// fields are the Δ quantities the gizmo's arrows fill while dragging.
fn move_dialog() -> DialogSpec {
    let field = |name: &str, label: &str| DialogField {
        name: name.to_string(),
        label: label.to_string(),
        kind: "quantity".to_string(),
        value: Some(serde_json::json!(0.0)),
        ..Default::default()
    };
    DialogSpec {
        title: "Move".to_string(),
        accept: "Move".to_string(),
        presentation: "float".to_string(),
        preview: "translation".to_string(),
        fields: vec![field("dx", "ΔX"), field("dy", "ΔY"), field("dz", "ΔZ")],
        ..Default::default()
    }
}

/// Move `names` by a world-space `delta`, one property write per object.
///
/// This is the native `App_Move` contract (`obj.Placement = delta * obj.Placement`),
/// applied to the placement's base only: a pure translation does not change the
/// rotation, so the rotation is kept as it is.
fn translate(state: &mut State, names: &[String], delta: Vector3) -> Result<usize, String> {
    let ids: Vec<ObjectId> = names
        .iter()
        .filter_map(|name| state.document.get_by_name(name))
        .collect();
    if ids.is_empty() {
        return Err("nothing selected".to_string());
    }
    // One transaction for the whole selection: a typed move is one undo step.
    in_transaction(&mut state.document, "Move", |document| {
        let mut moved = 0;
        for id in ids {
            let Some(placement) = current_placement(document, id) else {
                continue;
            };
            let next = Placement::new(placement.base.add(&delta), placement.rotation);
            document.set_property(id, "Placement", Property::Placement(next))?;
            moved += 1;
        }
        document.recompute()?;
        Ok(moved)
    })
}

/// The status line for an undo or redo: the transaction's name when it has one.
fn history_status(verb: &str, name: &str) -> String {
    if name.is_empty() {
        verb.to_string()
    } else {
        format!("{verb}: {name}")
    }
}

/// Run `edit` inside one named transaction, aborting it when `edit` fails.
///
/// A transaction is what puts an edit into the document's undo stack; the shell's
/// `App_Undo`/`App_Redo` then find it by name.
fn in_transaction<T>(
    document: &mut Document,
    name: &str,
    edit: impl FnOnce(&mut Document) -> Result<T, String>,
) -> Result<T, String> {
    document.open_transaction_named(name);
    match edit(document) {
        Ok(value) => {
            document.commit_transaction();
            Ok(value)
        }
        Err(error) => {
            document.abort_transaction();
            Err(error)
        }
    }
}

/// The object's placement property, if it has one.
fn current_placement(document: &Document, id: ObjectId) -> Option<Placement> {
    match document.object(id)?.properties.get("Placement")? {
        Property::Placement(placement) => Some(*placement),
        _ => None,
    }
}

/// Apply a visibility change to one object; `None` toggles.
///
/// Returns the state that now applies, or `None` when the object is gone. The switch is
/// a document `Visibility` property (added on first use, as the native backend does),
/// not a display-only flag: that is what puts a hide in the undo history and what
/// `stage_meshes` reads.
///
/// The caller owns the transaction; this only writes the property.
fn hide_object(document: &mut Document, id: ObjectId, visible: Option<bool>) -> Option<bool> {
    let object = document.object(id)?;
    let current = visibility_property(object)
        .or_else(|| object.view.as_ref().map(|view| view.visibility))
        .unwrap_or(true);
    let next = visible.unwrap_or(!current);
    if object.properties.get(VISIBILITY_PROPERTY).is_some() {
        document
            .set_property(id, VISIBILITY_PROPERTY, Property::Bool(next))
            .ok()?;
    } else {
        // A fresh boolean defaults to false, so the first write states the value
        // explicitly; `add_property` records it, so undo removes it again.
        document
            .add_property(
                id,
                VISIBILITY_PROPERTY,
                Property::Bool(next),
                0,
                "Base",
                "Whether the object is shown",
            )
            .ok()?;
    }
    Some(next)
}

/// Whether one named object is currently drawn.
fn object_visible(document: &Document, name: &str) -> bool {
    document
        .get_by_name(name)
        .and_then(|id| document.object(id))
        .map(ferrocad_core::is_visible)
        .unwrap_or(true)
}

/// Whether the whole selection is drawn: `None` when nothing is selected.
fn selection_visible(state: &State) -> Option<bool> {
    if state.selection.is_empty() {
        return None;
    }
    Some(
        state
            .selection
            .iter()
            .all(|name| object_visible(&state.document, name)),
    )
}

/// Report the sample document's shapes: one line per tessellated object.
///
/// A feature the kernel refuses keeps its last good shape and records why; that is
/// document state, so it is logged rather than treated as a failed boot. The
/// triangle counts are the browser build's version of the probe's "40 triangles"
/// baseline, and make a silent tessellation change visible in the console.
fn log_shapes(document: &Document) {
    let backend = backend();
    for id in document.object_ids() {
        let Some(object) = document.object(id) else {
            continue;
        };
        if object.invalid {
            log::warn!(
                "ferrocad: {} is invalid: {}",
                object.name,
                object.error.as_deref().unwrap_or("no reason recorded")
            );
        }
        let Some(shape) = object
            .properties
            .get("Shape")
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            continue;
        };
        match backend.tessellate(&shape, &TessellationOptions::absolute(TOLERANCE)) {
            Ok(mesh) => log::info!(
                "ferrocad: {} · {} triangles, {} edges",
                object.name,
                mesh.triangles.len() / 3,
                mesh.edges.len() / 2
            ),
            Err(error) => log::error!("ferrocad: {} did not tessellate: {error}", object.name),
        }
    }
}

/// The sample document: a box centred on the origin with a fillet on `Edge1`.
fn build_sample(document: &mut Document) -> Result<(), String> {
    // `make_box` builds from the corner at the origin; centre it so the orbit camera,
    // which pivots on the origin, frames the whole box.
    let base = document.add_object("Box", BOX_TYPE);
    document.set_property(
        base,
        "Placement",
        Property::Placement(Placement::new(
            Vector3::new(-5.0, -5.0, -5.0),
            Rotation::identity(),
        )),
    )?;

    let fillet = document.add_object("Fillet", FILLET_TYPE);
    document.set_property(
        fillet,
        "Base",
        Property::LinkSub("Box".to_string(), vec![EDGE.to_string()]),
    )?;
    document.set_property(fillet, "Radius", Property::Float(RADIUS))?;
    document.recompute()?;
    Ok(())
}

/// The object's document `Visibility` switch, when it has one.
///
/// The native backend keeps visibility on the view object; here the first hide adds a
/// document property so the edit is undoable and appears in the property pane.
fn visibility_property(object: &ferrocad_core::DocumentObject) -> Option<bool> {
    match object.properties.get(VISIBILITY_PROPERTY)? {
        Property::Bool(value) => Some(*value),
        _ => None,
    }
}

/// The object's own origin in world space (the placement's base point), which the
/// transform triad hugs and a rotate pivot turns about.
fn placement_base(object: &ferrocad_core::DocumentObject) -> Option<[f32; 3]> {
    ferrocad_core::origin(object).map(|[x, y, z]| [x as f32, y as f32, z as f32])
}

/// The object's surface colour: its view state's effective colour, or the Part default.
fn object_color(object: &ferrocad_core::DocumentObject) -> [f32; 4] {
    match ferrocad_core::effective_color(object) {
        Some(color) => [
            color[0] as f32,
            color[1] as f32,
            color[2] as f32,
            color[3] as f32,
        ],
        None => DEFAULT_COLOR,
    }
}

/// A refused edit: `ok` is false, so the pane keeps the old value and shows why.
fn refused(error: String) -> SetResult {
    SetResult {
        ok: false,
        error,
        view: None,
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
