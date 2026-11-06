//! The engine: a real `ferrocad_core::Document` behind `AppBackend`.
//!
//! The tree, the property rows and the viewport stage are read from one document,
//! and the geometry is built by the OCCT kernel (compiled to wasm in the browser,
//! linked natively on the desktop). Nothing here knows which: the host supplies the
//! window (or none) and, when it has one, the file-picker hooks.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use ferrocad_core::DocumentHandle;
use ferrocad_core::{
    Document, Matrix4, ObjectId, Placement, Property, Rotation, Vector3, status_names,
};
use ferrocad_geom::TessellationOptions;
use ferrocad_occt::OcctBackend;
use ferrocad_part::{BOX_TYPE, FILLET_TYPE, backend, shape_of};
use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_api::catalog::{self, CommandDef};
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::{
    ApplyResult, Bootstrap, DocumentNode, EvalResult, ObjectNode, OpenedDocument, PropRow,
    SetResult, VisibilityOutcome,
};
use ferrocad_shell_api::view::{GizmoRequest, ViewRequest, ViewState as ShellViewState};
use ferrocad_stage::{InteractionCommit, TRANSFORM_ID};

/// The commands the Rust backend offers beyond the shared catalogue.
///
/// Their metadata is not in the catalogue: Fit, undo/redo and hide/show are this
/// backend's own verbs, and Draft's snap switch is not a shell built-in.
const SNAP: &str = "Draft_Snap_Lock";
const FIT: &str = "App_Fit";
const VISIBILITY: &str = "App_ToggleVisibility";
const UNDO: &str = "App_Undo";
const REDO: &str = "App_Redo";

/// The prefix of the dialog tokens this backend mints for `App_Move`.
///
/// The shell's Move form is the typed twin of the gizmo: `App_Move` returns a token
/// and `dialog_accept` applies the delta. The Python backend hands the token to an
/// interpreter; here it is only a marker, so any token carrying this prefix is ours.
const MOVE_TOKEN: &str = "rust-move";

/// The document property a hide adds on first use. The Python backend writes the same
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
///
/// It is `ferrocad_part::DISPLAY_DEFLECTION` so the document half and a display hook
/// share one memo entry per shape (see that constant).
const TOLERANCE: f64 = ferrocad_part::DISPLAY_DEFLECTION;

/// The opaque steel grey the desktop Part workbench gives document solids.
const DEFAULT_COLOR: [f32; 4] = [0.30, 0.34, 0.40, 1.0];

/// Where the backend's document lives.
///
/// The same backend serves two kinds of host. A host that stands alone (the browser,
/// the tests, an `--open` desktop) owns its document outright. A desktop build that
/// also embeds CPython must share one with `ferrocad_py`: `FreeCAD.ActiveDocument`
/// and the model tree have to be the *same* document, not two copies that drift.
///
/// The shared variant holds no handle. A script can open or close documents at run
/// time, so the active one is looked up per access rather than captured at boot.
enum DocSource {
    /// A document this backend owns: the sample, a file it opened, or a test's.
    Isolated {
        name: String,
        document: DocumentHandle,
    },
    /// The process-wide registry in `ferrocad_core::application`, which
    /// `ferrocad_py` binds `FreeCAD.ActiveDocument` to.
    SharedRegistry,
}

/// The mutable half of the backend: where the document lives, and the state the view
/// commands toggle.
struct State {
    source: DocSource,
    view: ShellViewState,
    snap: bool,
    /// The shell's selection, mirrored for `App_Move`: the typed move applies to the
    /// selection, not to a named target.
    selection: Vec<String>,
    /// How many Move dialogs this backend has minted, so two open forms cannot spend
    /// each other's token.
    move_dialogs: u64,
}

/// The Rust `AppBackend`: a real document, no interpreter.
///
/// A host installs one of these on the shell (the browser host does, and so does a
/// desktop edition) instead of an interpreter-backed backend.
pub struct RustAppBackend {
    state: RefCell<State>,
    /// Why the sample document could not be built, if it could not.
    boot_error: Option<String>,
    /// The scripting half, when the host augments this document with one (the JS
    /// surface, S7). The document half never needs it.
    script: Option<Box<dyn ferrocad_shell_api::app::ScriptHost>>,
}

impl RustAppBackend {
    /// Compose Part with the OCCT kernel and build the sample document.
    ///
    /// In a wasm build this must run after the module's static constructors
    /// (`__wasm_call_ctors`): the kernel's C++ runtime has to be initialised before
    /// the first call.
    pub fn new() -> Self {
        let (document, boot_error) = sample_document();
        Self::isolated(DOCUMENT, document, boot_error)
    }

    /// Boot against the process-wide registry `ferrocad_py` binds to.
    ///
    /// Nothing is built here. [`bootstrap`](AppBackend::bootstrap) adopts whatever the
    /// registry holds — the document an interpreter created, or the sample when the
    /// registry is still empty — so composing this with a script half can never put a
    /// second document in front of the one that half is about to make.
    ///
    /// Nothing is captured either: every access reads the registry's active document,
    /// so a script that opens or closes one is visible on the next call.
    pub fn from_shared_registry() -> Self {
        Self {
            state: RefCell::new(State {
                source: DocSource::SharedRegistry,
                view: ShellViewState::default(),
                snap: false,
                selection: Vec::new(),
                move_dialogs: 0,
            }),
            boot_error: None,
            script: None,
        }
    }

    /// A backend that owns `document` under `name`.
    fn isolated(name: &str, document: Document, boot_error: Option<String>) -> Self {
        Self {
            state: RefCell::new(State {
                source: DocSource::Isolated {
                    name: name.to_string(),
                    document: Arc::new(Mutex::new(document)),
                },
                view: ShellViewState::default(),
                snap: false,
                selection: Vec::new(),
                move_dialogs: 0,
            }),
            boot_error,
            script: None,
        }
    }

    /// Add a scripting surface to this document backend.
    ///
    /// The JS host (S7) is the first user: the document stays `RustAppBackend`, and
    /// the script half is a [`ScriptHost`](ferrocad_shell_api::app::ScriptHost) that
    /// calls into the page. The shell finds it through `script_host()`, so
    /// `evaluate`, the event listener and the script's commands all work unchanged.
    pub fn with_script_host(
        mut self,
        script: Box<dyn ferrocad_shell_api::app::ScriptHost>,
    ) -> Self {
        self.script = Some(script);
        self
    }

    /// The active document's name and shared handle.
    ///
    /// The registry lock is released before this returns. Nothing may hold it while a
    /// document is locked: a script callback that walks from the document back to the
    /// registry would deadlock against the other order.
    fn active_document(&self) -> Option<(String, DocumentHandle)> {
        let isolated = {
            let state = self.state.borrow();
            match &state.source {
                DocSource::Isolated { name, document } => {
                    Some((name.clone(), Arc::clone(document)))
                }
                DocSource::SharedRegistry => None,
            }
        };
        if isolated.is_some() {
            return isolated;
        }
        let registry = registry();
        let name = registry.active()?.to_string();
        let handle = registry.get(&name)?;
        drop(registry);
        Some((name, handle))
    }

    /// Run `read` with the active document and its name.
    ///
    /// `None` when a shared registry has no active document — every document was
    /// closed — which the read paths report as "nothing to show", not as an error.
    fn with_document<R>(&self, read: impl FnOnce(&str, &Document) -> R) -> Option<R> {
        let (name, handle) = self.active_document()?;
        let document = lock(&handle);
        Some(read(&name, &document))
    }

    /// Run `edit` with the active document, locked for writing.
    ///
    /// The edit happens inside the registry's own handle, so a document the Python
    /// wrappers already hold is the one that changes — including its undo history,
    /// which belongs to the document rather than to a copy of it.
    fn with_document_mut<R>(
        &self,
        edit: impl FnOnce(&str, &mut Document) -> Result<R, String>,
    ) -> Result<R, String> {
        let Some((name, handle)) = self.active_document() else {
            return Err("no active document".to_string());
        };
        let mut document = lock(&handle);
        edit(&name, &mut document)
    }

    /// Make `document` the active one, returning the name it is known by.
    ///
    /// In the registry an already-open name keeps its handle and the contents are
    /// replaced inside it, so a `PyDocument` wrapper survives a re-open; a new name is
    /// registered and activated, which is what makes `FreeCAD.ActiveDocument` the file
    /// that was just opened. An isolated backend swaps its own document.
    fn install_document(&mut self, proposed: &str, document: Document) -> String {
        if matches!(&self.state.borrow().source, DocSource::SharedRegistry) {
            let mut registry = registry();
            if let Some(handle) = registry.get(proposed) {
                registry.set_active(proposed);
                // The document lock is taken only after the registry's is gone.
                drop(registry);
                *lock(&handle) = document;
                return proposed.to_string();
            }
            let (name, _) = registry.insert_document(proposed, document);
            return name;
        }
        let mut state = self.state.borrow_mut();
        let DocSource::Isolated {
            name,
            document: handle,
        } = &mut state.source
        else {
            unreachable!("the shared case returned above")
        };
        *lock(handle) = document;
        *name = proposed.to_string();
        proposed.to_string()
    }
}

impl Default for RustAppBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AppBackend for RustAppBackend {
    fn bootstrap(&mut self) -> Result<Bootstrap, String> {
        if let Some(error) = self.boot_error.clone() {
            return Err(error);
        }
        // The script half boots the engine when it has one: it starts the interpreter
        // and creates (or adopts) the document in the registry. It runs here, not in
        // the constructor, because `backend::install` composes the backend before
        // `Shell::boot_with` calls this.
        let booted = match self.script.as_deref() {
            Some(host) => host.bootstrap()?,
            None => None,
        };
        if let Some(boot) = booted {
            self.boot_error = None;
            return Ok(boot);
        }
        // No script half, and nothing in the registry: build the sample now. This is
        // deliberately after the script half's chance to boot, so a host that makes
        // its own document never leaves the sample behind it.
        let shared = matches!(&self.state.borrow().source, DocSource::SharedRegistry);
        if shared && self.active_document().is_none() {
            let (document, boot_error) = sample_document();
            self.install_document(DOCUMENT, document);
            if let Some(error) = boot_error {
                self.boot_error = Some(error.clone());
                return Err(error);
            }
        }
        Ok(Bootstrap {
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "rust".to_string(),
            document: self
                .active_document()
                .map(|(name, _)| name)
                .unwrap_or_else(|| DOCUMENT.to_string()),
        })
    }

    fn hello(&self) -> Result<String, String> {
        // An installed interpreter names itself, and its line is the honest one for
        // this build: the document half has no interpreter of its own.
        if let Some(line) = self
            .script
            .as_deref()
            .and_then(|host| host.hello().ok())
            .flatten()
        {
            return Ok(line);
        }
        Ok("FerroCAD shell · Rust document backend · OCCT · no CPython".to_string())
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
        // The registry path registers and activates it, so `FreeCAD.ActiveDocument` is
        // the file the shell just opened rather than a second copy of it.
        let name = self.install_document(name, document);
        {
            let mut state = self.state.borrow_mut();
            state.selection.clear();
            state.move_dialogs = 0;
        }
        Ok(OpenedDocument {
            document: name,
            objects,
        })
    }

    fn save_document(&self) -> Result<Vec<u8>, String> {
        let Some(saved) = self.with_document(|name, document| document.to_saved(name)) else {
            return Err("no active document".to_string());
        };
        ferrocad_core::fcstd_write::encode_fcstd(&saved).map_err(|error| error.to_string())
    }

    fn model_tree(&self) -> Result<Vec<DocumentNode>, String> {
        let node = self.with_document(|name, document| DocumentNode {
            name: name.to_string(),
            label: name.to_string(),
            objects: document
                .object_ids()
                .iter()
                .filter_map(|id| document.object(*id))
                .map(|object| ObjectNode {
                    name: object.name.clone(),
                    label: object.label.clone(),
                    type_id: object.type_id.clone(),
                })
                .collect(),
        });
        let mut tree: Vec<DocumentNode> = node.into_iter().collect();
        // A script surface appends what it owns after the document's own nodes (the
        // synthetic `Settings/<Feature>` roots). The borrow above is released first:
        // the script's walk is its own code and must not see a document lock.
        if let Some(script) = &self.script {
            tree.extend(script.extra_tree_nodes());
        }
        Ok(tree)
    }

    fn properties(&self, object: &str) -> Result<Vec<PropRow>, String> {
        // A script-owned node first. `Settings/Grid` is not in the document, so the
        // lookup below would answer "no rows" for a node the tree just showed.
        if let Some(rows) = self
            .script
            .as_deref()
            .and_then(|host| host.extra_properties(object))
        {
            return Ok(rows);
        }
        Ok(self
            .with_document(|_, document| {
                let Some(id) = document.get_by_name(object) else {
                    return Vec::new();
                };
                let Some(object) = document.object(id) else {
                    return Vec::new();
                };
                property_rows(object)
            })
            .unwrap_or_default())
    }

    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        // The document builds every object it can. Its own tessellation is a memo hit
        // — 35 ms on the 900a assembly — where the interpreter's copy of the same rows
        // costs ~1.3 s, because the arrays cross the boundary twice (into Python
        // `bytes`, then back into Rust).
        let meshes = self.with_document(|_, document| {
            let mut meshes = Vec::new();
            // The engine decides what is drawn: a feature replaces the object it
            // consumes (the box under a fillet), and a hidden object is skipped. The
            // native host asks the same function, so the two cannot disagree.
            for id in ferrocad_core::drawable(document) {
                let Some(object) = document.object(id) else {
                    continue;
                };
                let Some(property) = object.properties.get("Shape").and_then(shape_of) else {
                    continue;
                };
                if property.shape().is_none() {
                    continue;
                };
                // The property memoises: a sync of an unchanged document copies, it
                // does not re-mesh (the kernel meshes per face either way). `rigid` hands
                // back the frame the mesh was built in plus the move that places it — the
                // rows stay identical across a move, so the viewport patches packet
                // transforms instead of re-emitting 1.24 M vertices.
                let (mesh, placement) = property
                    .tessellate_rigid(TOLERANCE, TessellationOptions::default().angular_deflection)
                    .map_err(|error| format!("{}: {error}", object.name))?;
                let mesh = std::sync::Arc::unwrap_or_clone(mesh);
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
                    placement: Some(placement),
                    color: Some(object_color(object)),
                    display: Some(display_key(object).to_string()),
                });
            }
            Ok::<_, String>(meshes)
        });
        let mut meshes = match meshes {
            Some(meshes) => meshes?,
            // No active document: nothing is drawn, which is not a failure.
            None => Vec::new(),
        };
        // Now ask the script half, for the objects the document cannot mesh — one with
        // no `Shape` (the toy edition's procedural meshes) — and for any object a mod
        // deliberately draws itself. It is told which ones are already built, so it
        // does not re-tessellate a shape the document has: on the 900a assembly that
        // would copy 50 MB through the interpreter on every sync.
        let have: Vec<&str> = meshes.iter().map(|mesh| mesh.object.as_str()).collect();
        if let Some(extra) = self
            .script
            .as_deref()
            .and_then(|host| host.stage_meshes(&have))
        {
            for mesh in extra? {
                match meshes
                    .iter_mut()
                    .find(|existing| existing.object == mesh.object)
                {
                    // A script that answers for an object the document meshed is
                    // overriding it on purpose.
                    Some(slot) => *slot = mesh,
                    None => meshes.push(mesh),
                }
            }
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

    /// Commands this document offers.
    ///
    /// The catalogue's built-ins plus the verbs only this backend has. A host that
    /// installed a scripting surface appends the commands it registered, so a
    /// script's verb reaches the palette like a built-in.
    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        let (view, snap, selection) = {
            let state = self.state.borrow();
            (state.view.clone(), state.snap, state.selection.clone())
        };
        // A document is not needed to list the commands: with none open (a shared
        // registry whose documents were all closed) the edit verbs are simply disabled.
        let (has_undo, has_redo, visible) = self
            .with_document(|_, document| {
                (
                    document.has_undo(),
                    document.has_redo(),
                    selection_visible(document, &selection),
                )
            })
            .unwrap_or((false, false, None));
        // The shared catalogue is the source of every built-in's metadata; this
        // backend fills in the live state and appends the verbs only it offers.
        let mut commands: Vec<CommandInfo> =
            catalog::builtin().iter().map(CommandDef::info).collect();
        for command in &mut commands {
            match command.id.as_str() {
                catalog::TOGGLE_GRID => command.checked = view.grid,
                catalog::TOGGLE_AXIS_CROSS => command.checked = view.axis_cross,
                catalog::TOGGLE_GIZMO => command.checked = view.gizmo.enabled,
                _ => {}
            }
        }

        // Snap is Draft's, not a shell built-in: this backend's own switch, with no
        // engine-side effect yet.
        let mut snap_row = row(SNAP, "Snap", "snap");
        snap_row.checkable = true;
        snap_row.checked = snap;
        commands.push(snap_row);

        // Fit frames the document; undo and redo are plain edit commands (the dock
        // lists them and the accelerators run them, but they are not wheel verbs).
        let mut fit = row(FIT, "Fit", "dock");
        fit.tooltip = "Frame the document".to_string();
        commands.push(fit);
        let mut undo = row(UNDO, "Undo", "dock");
        undo.cmd_type = "ForEdit".to_string();
        undo.group = "Edit".to_string();
        undo.tooltip = "Undo the last edit".to_string();
        undo.accel = "Ctrl+Z".to_string();
        undo.enabled = has_undo;
        commands.push(undo);
        let mut redo = row(REDO, "Redo", "dock");
        redo.cmd_type = "ForEdit".to_string();
        redo.group = "Edit".to_string();
        redo.tooltip = "Redo the last undone edit".to_string();
        redo.accel = "Ctrl+Shift+Z".to_string();
        redo.enabled = has_redo;
        commands.push(redo);

        // Hide/Show is the wheel's third object verb and toggles the selection.
        let mut visibility = row(
            VISIBILITY,
            if visible == Some(false) {
                "Show"
            } else {
                "Hide"
            },
            "radial",
        );
        visibility.cmd_type = "ForEdit".to_string();
        visibility.group = "Edit".to_string();
        visibility.tooltip = "Hide or show the selected objects".to_string();
        visibility.wheel = true;
        commands.push(visibility);

        // A scripting surface (the JS host, or the embedded interpreter) contributes
        // its own verbs; they go through the same list, so the palette and the wheels
        // resolve them. An id the catalogue already provides keeps the catalogue's row:
        // its metadata and live state are what the chrome is built from, and a
        // workbench that registers a class under a built-in id must not shadow it.
        if let Some(script) = &self.script {
            for command in script.commands() {
                if !commands.iter().any(|existing| existing.id == command.id) {
                    commands.push(command);
                }
            }
        }
        Ok(commands)
    }

    /// The scripting half, when the host installed one with `with_script_host`.
    fn script_host(&self) -> Option<&dyn ferrocad_shell_api::app::ScriptHost> {
        self.script.as_deref()
    }

    fn set_view_state(&self, state: &ShellViewState) -> Result<(), String> {
        self.state.borrow_mut().view = state.clone();
        // Mirrored, not queried: a `FreeCADGui` command answers `IsChecked` from this,
        // so it has to be current before the next command can run. The local borrow is
        // released first — the script's copy is its own.
        if let Some(script) = &self.script {
            script.on_view_state_changed(state);
        }
        Ok(())
    }

    fn set_selection(&self, items: &[SelectionItem]) -> Result<(), String> {
        // The shell owns the selection; the document needs it for `App_Move`, whose
        // delta applies to whatever is selected rather than to a named target.
        {
            let mut state = self.state.borrow_mut();
            state.selection = items.iter().map(|item| item.object.clone()).collect();
            state.selection.dedup();
        }
        if let Some(script) = &self.script {
            script.on_selection_changed(items);
        }
        Ok(())
    }

    fn set_visibility(
        &mut self,
        object: &str,
        visible: Option<bool>,
    ) -> Result<VisibilityOutcome, String> {
        self.with_document_mut(|_, document| {
            let Some(id) = document.get_by_name(object) else {
                return Ok(VisibilityOutcome {
                    ok: false,
                    visible: false,
                    error: format!("no object {object}"),
                });
            };
            match in_transaction(document, "Toggle visibility", |document| {
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
        })
    }

    fn rotation_quaternion(&self, object: &str) -> Option<[f64; 4]> {
        // The stage is world-space with an identity placement, so the object's own
        // orientation is only in the document. `Rotation::q` is already `[w, x, y, z]`.
        self.with_document(|_, document| {
            let id = document.get_by_name(object)?;
            Some(current_placement(document, id)?.rotation.q)
        })
        .flatten()
    }

    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        // An installed script surface is the interpreter: the document half has none
        // of its own, so without one there is nothing to evaluate against.
        match self.script.as_deref() {
            Some(host) => host.evaluate(code),
            None => Ok(EvalResult {
                ok: false,
                output: "this build has no Python interpreter".to_string(),
            }),
        }
    }

    fn set_property(
        &mut self,
        object: &str,
        property: &str,
        value: &str,
    ) -> Result<SetResult, String> {
        // A script-owned node first: `Settings/Grid` has no `PropertyContainer`, so the
        // document lookup below would refuse a row the tree just offered.
        if let Some(result) = self
            .script
            .as_deref()
            .and_then(|host| host.set_extra_property(object, property, value))
        {
            return Ok(result);
        }
        self.with_document_mut(|_, document| {
            let Some(id) = document.get_by_name(object) else {
                return Ok(refused(format!("no object {object}")));
            };

            if property == "Label" {
                in_transaction(document, "Rename", |document| {
                    document.set_label(id, value);
                    Ok(())
                })?;
                return Ok(SetResult {
                    ok: true,
                    error: String::new(),
                    view: None,
                });
            }

            let Some(current) = document
                .object(id)
                .and_then(|object| object.properties.get(property))
                .cloned()
            else {
                return Ok(refused(format!("no property {property}")));
            };
            if document
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
            document.open_transaction_named("Set property");
            if let Err(error) = document.set_property(id, property, next) {
                document.abort_transaction();
                return Ok(refused(error));
            }
            let recomputed = document.recompute();
            document.commit_transaction();
            match recomputed {
                Ok(_) => {
                    // A feature that refuses its input keeps its last good shape and
                    // reports the reason. That is document state, not a failed edit:
                    // the value was stored, so the pane keeps it.
                    if let Some(error) = document
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
        })
    }

    fn run_command(&mut self, id: &str) -> Result<CommandOutcome, String> {
        // `App_Open`/`App_Save` never arrive here: the shell owns both ids and asks
        // the host's `FilePicker` (S6). A backend only moves bytes.
        // The two object verbs are not view toggles: they hand the shell a session
        // (a gizmo mode, and for Move a token its form spends).
        if id == catalog::MOVE {
            let token = {
                let mut state = self.state.borrow_mut();
                state.move_dialogs += 1;
                format!("{MOVE_TOKEN}-{}", state.move_dialogs)
            };
            return Ok(CommandOutcome {
                ok: true,
                status: "move: drag a handle or type a value".to_string(),
                gizmo: Some(catalog::MOVE_GIZMO.to_string()),
                dialog: Some(catalog::move_dialog()),
                token: Some(token),
                ..Default::default()
            });
        }
        if id == catalog::ROTATE {
            return Ok(CommandOutcome {
                ok: true,
                status: "rotate: drag a ring".to_string(),
                gizmo: Some("rotate".to_string()),
                ..Default::default()
            });
        }
        if id == VISIBILITY {
            let selection = self.state.borrow().selection.clone();
            if selection.is_empty() {
                return Ok(CommandOutcome {
                    ok: false,
                    error: "nothing selected".to_string(),
                    ..Default::default()
                });
            }
            // One verb for the whole selection: if anything is still visible, hide
            // them all; otherwise show them all. One transaction, so one undo step.
            let visible = self
                .with_document(|_, document| selection_visible(document, &selection))
                .flatten();
            let hide = visible != Some(false);
            let verb = if hide { "Hide" } else { "Show" };
            let changed = self.with_document_mut(|_, document| {
                in_transaction(document, verb, |document| {
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
                })
            })?;
            let verb = if hide { "hid" } else { "showed" };
            return Ok(CommandOutcome {
                ok: true,
                status: format!("{verb} {changed} object(s)"),
                ..Default::default()
            });
        }
        // Undo and redo are the document's history, not the shell's: they run against
        // the shared handle, so a script that undid something and a shell that undoes
        // next see one history.
        if id == UNDO || id == REDO {
            let undo = id == UNDO;
            let name = self.with_document_mut(|_, document| {
                let names = if undo {
                    document.undo_names()
                } else {
                    document.redo_names()
                };
                let name = names.first().cloned().unwrap_or_default();
                let moved = if undo {
                    document.undo()
                } else {
                    document.redo()
                };
                if !moved {
                    return Ok(None);
                }
                document.recompute()?;
                Ok(Some(name))
            })?;
            let verb = if undo { "undo" } else { "redo" };
            return Ok(match name {
                Some(name) => CommandOutcome {
                    ok: true,
                    status: history_status(verb, &name),
                    ..Default::default()
                },
                None => CommandOutcome {
                    ok: false,
                    error: format!("nothing to {verb}"),
                    ..Default::default()
                },
            });
        }

        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let (status, view) = match id {
            catalog::TOGGLE_GRID => {
                state.view.grid = !state.view.grid;
                (
                    format!("grid {}", if state.view.grid { "on" } else { "off" }),
                    Some(ViewRequest {
                        grid: Some(state.view.grid),
                        ..Default::default()
                    }),
                )
            }
            catalog::TOGGLE_AXIS_CROSS => {
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
            catalog::TOGGLE_GIZMO => {
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
            catalog::VIEW_WORKPLANE => ("workplane: not wired in this backend".to_string(), None),
            FIT => (
                "fit".to_string(),
                Some(ViewRequest {
                    fit: Some(true),
                    ..Default::default()
                }),
            ),
            other => {
                // A command the scripting surface registered is not the document's:
                // ask it, and only report "not available" when it disclaims the id too.
                if let Some(outcome) = self.script.as_ref().and_then(|s| s.run_command(other)) {
                    return Ok(outcome);
                }
                return Ok(CommandOutcome {
                    ok: false,
                    error: format!("{other} is not available in this build"),
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
            // Not this backend's dialog. A script surface opened it, if anything did.
            if let Some(outcome) = self
                .script
                .as_deref()
                .and_then(|host| host.dialog_accept(token, values))
            {
                return Ok(outcome);
            }
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
        let selection = self.state.borrow().selection.clone();
        if selection.is_empty() {
            return Ok(CommandOutcome {
                ok: false,
                error: "nothing selected".to_string(),
                ..Default::default()
            });
        }
        match self.with_document_mut(|_, document| translate(document, &selection, delta)) {
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

    fn dialog_cancel(&mut self, token: &str) -> Result<CommandOutcome, String> {
        // A dialog the script opened has state of its own to drop.
        if let Some(outcome) = self
            .script
            .as_deref()
            .and_then(|host| host.dialog_cancel(token))
        {
            return Ok(outcome);
        }
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
            // Not the shared transform gizmo. A workbench's dress-up arc (the Part
            // fillet, a BIM grip) knows its own feature type, so the commit is the
            // script's to apply; only when it disclaims the id is it unavailable.
            if let Some(result) = self
                .script
                .as_deref()
                .and_then(|host| host.apply_interaction(commit, object))
            {
                return Ok(result);
            }
            return Ok(ApplyResult {
                ok: false,
                error: format!(
                    "{} {} is not available in this build",
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
        self.with_document_mut(|_, document| {
            let Some(id) = document.get_by_name(object) else {
                return Ok(ApplyResult {
                    ok: false,
                    error: format!("no object {object}"),
                });
            };
            let Some(placement) = current_placement(document, id) else {
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
            in_transaction(document, "Transform", |document| {
                document.set_property(id, "Placement", Property::Placement(next))?;
                document.recompute()?;
                Ok(())
            })?;
            Ok(ApplyResult {
                ok: true,
                error: String::new(),
            })
        })
    }
}

/// The property editor's rows for one document object.
///
/// `Label` is a document-object field, not a property-container entry; FreeCAD
/// presents it as an ordinary row and so does the native backend, so it always
/// comes first.
fn property_rows(object: &ferrocad_core::DocumentObject) -> Vec<PropRow> {
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
            Property::IntegerConstraint { min, max, .. } => (Some(*min as f64), Some(*max as f64)),
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
    rows
}
/// Move `names` by a world-space `delta`, one property write per object.
///
/// This is the native `App_Move` contract (`obj.Placement = delta * obj.Placement`),
/// applied to the placement's base only: a pure translation does not change the
/// rotation, so the rotation is kept as it is.
fn translate(document: &mut Document, names: &[String], delta: Vector3) -> Result<usize, String> {
    let ids: Vec<ObjectId> = names
        .iter()
        .filter_map(|name| document.get_by_name(name))
        .collect();
    if ids.is_empty() {
        return Err("nothing selected".to_string());
    }
    // One transaction for the whole selection: a typed move is one undo step.
    in_transaction(document, "Move", |document| {
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
fn selection_visible(document: &Document, selection: &[String]) -> Option<bool> {
    if selection.is_empty() {
        return None;
    }
    Some(selection.iter().all(|name| object_visible(document, name)))
}

/// The process-wide document registry (`FreeCAD.ActiveDocument`).
///
/// A poisoned lock still has a usable registry: the panic that poisoned it was in
/// somebody else's callback, and refusing to read the documents afterwards would turn
/// that into a dead application.
fn registry() -> std::sync::MutexGuard<'static, ferrocad_core::Application> {
    ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// A document handle, locked. Same rule as [`registry`]: a panic elsewhere does not
/// make the document unreadable.
fn lock(handle: &DocumentHandle) -> std::sync::MutexGuard<'_, Document> {
    handle.lock().unwrap_or_else(|error| error.into_inner())
}

/// The sample document and why it failed to build, if it did.
fn sample_document() -> (Document, Option<String>) {
    // Idempotent: the Part module keeps one backend per process.
    ferrocad_part::register(Arc::new(OcctBackend::new()));
    let mut document = Document::new();
    let boot_error = build_sample(&mut document).err();
    match boot_error.as_deref() {
        Some(error) => log::error!("ferrocad: the sample document did not build: {error}"),
        None => log_shapes(&document),
    }
    (document, boot_error)
}

/// Report the sample document's shapes: one line per tessellated object.
///
/// A feature the kernel refuses keeps its last good shape and records why; that is
/// document state, so it is logged rather than treated as a failed boot. The
/// triangle counts are the probe's "40 triangles" baseline, and make a silent
/// tessellation change visible in the browser console.
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

/// The `display` string a document object asks for.
///
/// A workbench's convention is stored as document data — `DisplayMode` (an
/// `App::PropertyEnumeration`) and the `Part::DatumPlane` type for the translucent
/// card — so the document half reads it instead of asking the display hook. A mod
/// that draws an object some other way still answers through that hook and replaces
/// this row by name.
fn display_key(object: &ferrocad_core::DocumentObject) -> &'static str {
    if object.type_id == "Part::DatumPlane" {
        return "glass";
    }
    let mode = match object.properties.get("DisplayMode") {
        Some(Property::Enumeration(values, index)) => values.get(*index).map(String::as_str),
        Some(Property::String(text)) => Some(text.as_str()),
        _ => None,
    };
    match mode {
        Some("Shaded") => "shaded",
        Some("Wireframe") => "wireframe",
        _ => "shaded_edges",
    }
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

/// One command row the shared catalogue does not carry.
///
/// Only the fields this backend sets differ; the rest keep the shell's defaults.
fn row(id: &str, menu: &str, surface: &str) -> CommandInfo {
    CommandInfo {
        id: id.to_string(),
        menu: menu.to_string(),
        tooltip: menu.to_string(),
        accel: String::new(),
        group: "View".to_string(),
        cmd_type: "ForView".to_string(),
        surface: surface.to_string(),
        short: String::new(),
        tone: String::new(),
        parent: String::new(),
        wheel: false,
        enabled: true,
        checked: false,
        checkable: false,
        dev_only: false,
        // The browser host's view switches carry no icon of their own; the shell
        // falls back to the command id, exactly as `catalog` does for an iconless row.
        pixmap: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ferrocad_inventor::{EventListener, Scene};
    use ferrocad_shell_api::app::{NoopListener, ScriptHost};
    use ferrocad_shell_api::types::{
        AxisSummary, GpuPreferenceOutcome, GridProvider, GridSummary, ProviderFeature,
        SelectionRequest, WorkbenchReport,
    };

    use super::*;

    /// The synthetic node the fake script owns: in the tree, absent from the document.
    const SETTINGS_GRID: &str = "Settings/Grid";

    /// What the fake records, kept outside the boxed host so a test can read it back.
    #[derive(Default)]
    struct Recorded {
        selected: Mutex<Vec<SelectionItem>>,
        view: Mutex<Option<ShellViewState>>,
        requests: Mutex<Vec<SelectionRequest>>,
    }

    /// The script half the tests install.
    ///
    /// It claims one synthetic node, one synthetic property and one dialog token, and
    /// records what the backend mirrored into it. Everything it does not own keeps the
    /// trait's default, so each test also exercises the fall-through.
    struct FakeHost {
        log: Arc<Recorded>,
    }

    impl ScriptHost for FakeHost {
        fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
            Ok(EvalResult {
                ok: true,
                output: format!("ran {code}"),
            })
        }

        fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
            Ok(WorkbenchReport::default())
        }

        fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
            Err(format!("no tool {name}"))
        }

        fn tool_command(&self, _name: &str, _verb: &str) -> bool {
            false
        }

        fn inventor_scene(&self) -> Result<Scene, String> {
            Ok(Scene::default())
        }

        fn event_listener(&self) -> Box<dyn EventListener> {
            Box::new(NoopListener)
        }

        fn extra_tree_nodes(&self) -> Vec<DocumentNode> {
            vec![DocumentNode {
                name: SETTINGS_GRID.to_string(),
                label: "Grid".to_string(),
                objects: Vec::new(),
            }]
        }

        fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
            (object == SETTINGS_GRID).then(|| {
                vec![PropRow {
                    name: "Spacing".to_string(),
                    label: String::new(),
                    type_id: "App::PropertyLength".to_string(),
                    value: "10 mm".to_string(),
                    status: String::new(),
                    editable: true,
                    command: None,
                    kind: "quantity".to_string(),
                    min: None,
                    max: None,
                    unit: Some("mm".to_string()),
                    choices: Vec::new(),
                }]
            })
        }

        fn set_extra_property(
            &self,
            object: &str,
            property: &str,
            value: &str,
        ) -> Option<SetResult> {
            (object == SETTINGS_GRID).then(|| SetResult {
                ok: property == "Spacing",
                error: format!("{object}.{property} = {value}"),
                view: None,
            })
        }

        fn on_selection_changed(&self, items: &[SelectionItem]) {
            *self.log.selected.lock().unwrap() = items.to_vec();
        }

        fn take_selection_requests(&self) -> Vec<SelectionRequest> {
            std::mem::take(&mut *self.log.requests.lock().unwrap())
        }

        fn on_view_state_changed(&self, state: &ShellViewState) {
            *self.log.view.lock().unwrap() = Some(state.clone());
        }

        fn appearance_view(&self) -> Option<CommandOutcome> {
            Some(CommandOutcome {
                ok: true,
                status: "scripted appearance".to_string(),
                ..Default::default()
            })
        }

        fn grid_summary(&self) -> Option<GridSummary> {
            Some(GridSummary {
                feature: "grid".to_string(),
                backend: Some("scripted".to_string()),
                ..Default::default()
            })
        }

        fn axis_summary(&self) -> Option<AxisSummary> {
            Some(AxisSummary {
                feature: "axis".to_string(),
                length: Some("80 px".to_string()),
                ..Default::default()
            })
        }

        fn provider_features(&self) -> Vec<ProviderFeature> {
            vec![ProviderFeature {
                feature: "grid".to_string(),
                providers: vec![GridProvider {
                    backend: "scripted".to_string(),
                    active: true,
                    ..Default::default()
                }],
            }]
        }

        fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
            (feature == "grid").then(|| CommandOutcome {
                ok: true,
                status: format!("grid: {backend}"),
                ..Default::default()
            })
        }

        fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
            (feature == "grid").then(|| CommandOutcome {
                ok: true,
                status: "grid refreshed".to_string(),
                ..Default::default()
            })
        }

        fn provider_skin(&self, feature: &str) -> Option<String> {
            (feature == "grid").then(|| "// scripted wgsl".to_string())
        }

        fn manipulator_specs(
            &self,
            object: &str,
            element: Option<&str>,
        ) -> Option<Vec<serde_json::Value>> {
            (object == "Box" && element == Some("Edge1"))
                .then(|| vec![serde_json::json!({ "kind": "fillet" })])
        }

        fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
            (token == DIALOG).then(|| CommandOutcome {
                ok: true,
                status: format!("accepted {values}"),
                ..Default::default()
            })
        }

        fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
            (token == DIALOG).then(|| CommandOutcome {
                ok: true,
                status: "cancelled".to_string(),
                ..Default::default()
            })
        }

        fn gpu_adapter(&self) -> Option<String> {
            Some("high".to_string())
        }

        fn set_gpu_adapter(&self, value: &str) -> Option<GpuPreferenceOutcome> {
            Some(GpuPreferenceOutcome {
                ok: value == "high",
                value: value.to_string(),
                error: String::new(),
            })
        }
    }

    /// A dialog token only the script knows: `MOVE_TOKEN` prefixed tokens stay the
    /// document backend's, everything else is the script's to answer.
    const DIALOG: &str = "py-dialog-1";

    fn backend_with_script() -> (RustAppBackend, Arc<Recorded>) {
        let log = Arc::new(Recorded::default());
        let backend =
            RustAppBackend::new().with_script_host(Box::new(FakeHost { log: log.clone() }));
        (backend, log)
    }

    #[test]
    fn the_tree_carries_the_nodes_the_script_owns() {
        let (backend, _) = backend_with_script();
        let tree = backend.model_tree().unwrap();
        // The document's own root first, then what the script appended.
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].name, DOCUMENT);
        assert_eq!(tree[1].name, SETTINGS_GRID);
        assert!(tree[0].objects.iter().any(|object| object.name == "Box"));
    }

    #[test]
    fn a_script_owned_node_is_read_before_the_document() {
        let (backend, _) = backend_with_script();
        // `Settings/Grid` is not in the document: without the intercept this answers
        // an empty list and the tree row the shell just showed would open blank.
        let rows = backend.properties(SETTINGS_GRID).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Spacing");
        assert_eq!(rows[0].value, "10 mm");
    }

    #[test]
    fn a_document_object_still_reads_from_the_document() {
        let (backend, _) = backend_with_script();
        let rows = backend.properties("Box").unwrap();
        // The script declined this object (`None`), so the document answered.
        assert!(rows.iter().any(|row| row.name == "Label"));
        assert!(rows.iter().any(|row| row.name == "Length"));
    }

    #[test]
    fn a_script_owned_property_is_written_without_the_document() {
        let (mut backend, _) = backend_with_script();
        let before = backend.model_tree().unwrap()[0].objects.len();
        let result = backend
            .set_property(SETTINGS_GRID, "Spacing", "25 mm")
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.error, "Settings/Grid.Spacing = 25 mm");
        // It never reached the document: nothing was created to hold it.
        assert_eq!(backend.model_tree().unwrap()[0].objects.len(), before);
    }

    #[test]
    fn the_selection_and_view_state_are_mirrored_to_the_script() {
        let (backend, log) = backend_with_script();
        let items = vec![SelectionItem::with_elements("Box", vec!["Face6".into()])];
        backend.set_selection(&items).unwrap();
        backend
            .set_view_state(&ShellViewState {
                grid: true,
                ..Default::default()
            })
            .unwrap();
        // The fake records what the backend mirrored, so a forwarding that silently
        // stopped would leave these empty.
        assert_eq!(log.selected.lock().unwrap().as_slice(), items.as_slice());
        assert!(log.view.lock().unwrap().as_ref().unwrap().grid);
    }

    #[test]
    fn selection_requests_drain_from_the_script() {
        let (mut backend, log) = backend_with_script();
        log.requests.lock().unwrap().push(SelectionRequest {
            op: "add".to_string(),
            object: "Box".to_string(),
            elements: Vec::new(),
            points: Vec::new(),
        });
        let drained = backend.take_selection_requests().unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].object, "Box");
        // Drained means drained: the second call is empty.
        assert!(backend.take_selection_requests().unwrap().is_empty());
    }

    #[test]
    fn the_provider_and_gpu_hooks_ask_the_script_first() {
        let (mut backend, _) = backend_with_script();
        assert_eq!(backend.grid_summary().unwrap().feature, "grid");
        assert_eq!(backend.axis_summary().unwrap().feature, "axis");
        assert_eq!(backend.provider_features().unwrap().len(), 1);
        assert_eq!(
            backend.provider_skin("grid").unwrap().as_deref(),
            Some("// scripted wgsl")
        );
        assert_eq!(
            backend.set_provider("grid", "scripted").unwrap().status,
            "grid: scripted"
        );
        assert_eq!(
            backend.refresh_providers("grid").unwrap().status,
            "grid refreshed"
        );
        // A feature the script disclaims falls through to the backend's default.
        assert!(
            backend
                .set_provider("axis", "anything")
                .unwrap()
                .status
                .is_empty()
        );
        assert_eq!(backend.gpu_adapter().unwrap(), "high");
        assert!(backend.set_gpu_adapter("high").unwrap().ok);
        assert!(!backend.set_gpu_adapter("low").unwrap().ok);
        assert_eq!(
            backend
                .manipulator_specs("Box", Some("Edge1"))
                .unwrap()
                .len(),
            1
        );
        assert!(backend.manipulator_specs("Box", None).unwrap().is_empty());
        assert_eq!(
            backend.appearance_view().unwrap().status,
            "scripted appearance"
        );
    }

    #[test]
    fn a_scripted_dialog_is_accepted_and_cancelled() {
        let (mut backend, _) = backend_with_script();
        let accepted = backend
            .dialog_accept(DIALOG, &serde_json::json!({ "r": 2.0 }))
            .unwrap();
        assert!(accepted.ok);
        assert!(accepted.status.starts_with("accepted"));
        assert!(backend.dialog_cancel(DIALOG).unwrap().ok);
        // A token neither half knows is still refused with the backend's message.
        let unknown = backend
            .dialog_accept("nobody-owns-this", &serde_json::json!({}))
            .unwrap();
        assert!(!unknown.ok);
        assert!(unknown.error.contains("not a dialog this build opened"));
    }

    #[test]
    fn a_scripted_console_line_reaches_the_interpreter() {
        // The document half has no interpreter of its own, so an installed host must
        // answer — otherwise `?js=1` would regress to "no Python interpreter".
        let (backend, _) = backend_with_script();
        assert_eq!(backend.evaluate("1 + 1").unwrap().output, "ran 1 + 1");
    }

    #[test]
    fn without_a_script_the_document_answers_alone() {
        let mut backend = RustAppBackend::new();
        assert_eq!(backend.model_tree().unwrap().len(), 1);
        assert!(backend.properties(SETTINGS_GRID).unwrap().is_empty());
        assert_eq!(backend.gpu_adapter().unwrap(), "auto");
        assert!(backend.take_selection_requests().unwrap().is_empty());
        let unknown = backend
            .dialog_accept("nobody-owns-this", &serde_json::json!({}))
            .unwrap();
        assert!(!unknown.ok);
        assert!(!backend.evaluate("1 + 1").unwrap().ok);
    }

    /// The registry is process-wide, so the shared tests take turns and start empty.
    static REGISTRY: Mutex<()> = Mutex::new(());

    /// Serialise a shared-registry test and hand it an empty registry.
    fn clear_registry() -> std::sync::MutexGuard<'static, ()> {
        let guard = REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
        let names = registry().names();
        let mut application = registry();
        for name in names {
            application.close_document(&name);
        }
        drop(application);
        guard
    }

    /// A shared-registry backend that has booted.
    ///
    /// Nothing is built before `bootstrap()`, so a shared test starts with the run.
    fn shared() -> RustAppBackend {
        let mut backend = RustAppBackend::from_shared_registry();
        backend.bootstrap().expect("boot the sample");
        backend
    }

    #[test]
    fn an_empty_registry_gets_the_sample() {
        let _serial = clear_registry();
        let mut backend = RustAppBackend::from_shared_registry();
        // Constructing the backend builds nothing: a script half boots *after* the
        // constructor runs, and a sample document sitting in front of the one it
        // creates is exactly the drift the merge exists to end.
        assert!(registry().is_empty(), "the constructor builds nothing");
        assert!(backend.model_tree().unwrap().is_empty());

        assert_eq!(backend.bootstrap().unwrap().document, DOCUMENT);
        assert_eq!(registry().active(), Some(DOCUMENT));
        let tree = backend.model_tree().unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, DOCUMENT);
        assert!(tree[0].objects.iter().any(|object| object.name == "Box"));
    }

    #[test]
    fn a_host_that_boots_supplies_the_document() {
        let _serial = clear_registry();

        /// The script half in this test: it starts "the interpreter" and creates its
        /// document, then reports it.
        struct Boots;

        impl ScriptHost for Boots {
            fn evaluate(&self, _code: &str) -> Result<EvalResult, String> {
                Err("no interpreter".to_string())
            }
            fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
                Ok(WorkbenchReport::default())
            }
            fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
                Err(format!("no tool {name}"))
            }
            fn tool_command(&self, _name: &str, _verb: &str) -> bool {
                false
            }
            fn inventor_scene(&self) -> Result<Scene, String> {
                Ok(Scene::default())
            }
            fn event_listener(&self) -> Box<dyn EventListener> {
                Box::new(NoopListener)
            }

            fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
                let mut document = Document::new();
                document.add_object("FromHost", BOX_TYPE);
                registry().insert_document("FromHost", document);
                Ok(Some(Bootstrap {
                    version: "9.9".to_string(),
                    backend: "script".to_string(),
                    document: "FromHost".to_string(),
                }))
            }
        }

        let mut backend = RustAppBackend::from_shared_registry().with_script_host(Box::new(Boots));
        let boot = backend.bootstrap().unwrap();
        assert_eq!(boot.backend, "script");
        assert_eq!(boot.document, "FromHost");
        // The sample never appeared: the registry holds exactly the script's document,
        // because the fallback is built only after the host declines.
        assert_eq!(registry().names(), vec!["FromHost".to_string()]);
        let tree = backend.model_tree().unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "FromHost");
    }

    #[test]
    fn an_existing_document_is_used_as_it_is() {
        let _serial = clear_registry();
        // The `python::bootstrap()` case: the interpreter made the document and the
        // shell must display *that* one, not build a sample beside it.
        {
            let mut document = Document::new();
            document.add_object("Imported", BOX_TYPE);
            registry().insert_document("FromPython", document);
        }
        let mut backend = RustAppBackend::from_shared_registry();
        assert_eq!(backend.bootstrap().unwrap().document, "FromPython");
        let tree = backend.model_tree().unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "FromPython");
        assert!(
            tree[0]
                .objects
                .iter()
                .any(|object| object.name == "Imported")
        );
        assert!(!tree[0].objects.iter().any(|object| object.name == "Box"));
    }

    #[test]
    fn opening_a_document_registers_and_activates_it() {
        let _serial = clear_registry();
        // A file written by one backend and opened by another: the same path the
        // browser host takes with bytes from its picker.
        let bytes = {
            let source = RustAppBackend::new();
            source.save_document().unwrap()
        };
        let mut backend = shared();
        let opened = backend.open_document("Opened", &bytes).unwrap();
        assert_eq!(opened.document, "Opened");
        // `FreeCAD.ActiveDocument` is the file the shell just opened.
        assert_eq!(registry().active(), Some("Opened"));
        assert!(registry().contains("Opened"));
        let tree = backend.model_tree().unwrap();
        assert_eq!(tree[0].name, "Opened");
        assert!(!tree[0].objects.is_empty());
    }

    #[test]
    fn reopening_a_name_keeps_the_registry_handle() {
        let _serial = clear_registry();
        let bytes = {
            let source = RustAppBackend::new();
            source.save_document().unwrap()
        };
        let mut backend = shared();
        let before = registry().get(DOCUMENT).unwrap();
        let name = backend.open_document(DOCUMENT, &bytes).unwrap().document;
        assert_eq!(name, DOCUMENT);
        let after = registry().get(DOCUMENT).unwrap();
        // Replacing the contents inside the same handle is what keeps a `PyDocument`
        // wrapper (and anything else holding the document) valid across a re-open.
        assert!(Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn an_edit_and_its_undo_run_against_the_registry_document() {
        let _serial = clear_registry();
        let mut backend = shared();
        let handle = registry().get(DOCUMENT).unwrap();
        let length = |document: &Document| {
            property_rows(
                document
                    .object(document.get_by_name("Box").unwrap())
                    .unwrap(),
            )
            .into_iter()
            .find(|row| row.name == "Length")
            .map(|row| row.value)
        };
        let original = length(&lock(&handle)).unwrap();

        assert!(backend.set_property("Box", "Length", "7").unwrap().ok);
        assert_ne!(length(&lock(&handle)).unwrap(), original);

        assert!(backend.run_command(UNDO).unwrap().ok);
        // Read through the registry's own handle: the undo reached the document the
        // Python wrappers hold, not a copy of it.
        assert_eq!(length(&lock(&handle)).unwrap(), original);
    }

    #[test]
    fn with_no_active_document_reads_are_empty_and_writes_refused() {
        let _serial = clear_registry();
        let mut backend = shared();
        {
            let mut application = registry();
            let name = application.active().unwrap().to_string();
            application.close_document(&name);
        }
        // A shell with every document closed shows an empty tree, not an error.
        assert!(backend.model_tree().unwrap().is_empty());
        assert!(backend.stage_meshes().unwrap().is_empty());
        assert!(backend.properties("Box").unwrap().is_empty());
        // An edit with nothing to edit is a failure, and says so.
        assert!(backend.save_document().is_err());
        assert!(backend.set_property("Box", "Length", "7").is_err());
        assert!(backend.set_visibility("Box", None).is_err());
    }
}
