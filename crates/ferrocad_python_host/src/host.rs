//! The Python script half of [`ScriptHost`], over any [`PythonRuntime`].
//!
//! This is the whole seam in one place. Every method either crosses into the shell
//! module (`python/ferrocad_shell/__init__.py`, the same source in both interpreters) or
//! hands off to the one engine hook [`PythonRuntime`] declares for it. A capability the
//! facade does not implement is asked nothing — see [`Contract`] — and the document half
//! answers, which is what a build with no script half does anyway.
//!
//! The five merged methods of `RustAppBackend` (`model_tree`, `properties`,
//! `set_property`, `dialog_accept`, `dialog_cancel`) are deliberately *not* implemented
//! as `AppBackend` calls here. The document half owns them and asks this half only for
//! the part it cannot answer, through
//! [`extra_tree_nodes`](ScriptHost::extra_tree_nodes),
//! [`extra_properties`](ScriptHost::extra_properties) and
//! [`set_extra_property`](ScriptHost::set_extra_property).

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_shell_api::DocumentGrant;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::catalog;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, ProviderSkin, SelectionRequest,
    SetResult, WorkbenchReport,
};
use ferrocad_shell_api::view::ViewState;
use ferrocad_stage::{InteractionCommit, Tool};
use ferrocad_types::Vector3;
use serde::Deserialize;

use crate::runtime::PythonRuntime;

/// The synthetic root the app script puts above the documents in the model tree.
///
/// Its children are one `Settings/<Feature>` node per registered provider feature. It is
/// not a document: nothing about it is in `ferrocad_core`.
const SETTINGS_ROOT: &str = "Settings";

/// A [`ScriptHost`] over one interpreter.
///
/// The interpreter, its facade and the document registry are the runtime's; this type is
/// the seam over them. Every method is a synchronous call into the runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonHost<R> {
    runtime: R,
}

impl<R> PythonHost<R> {
    /// A host over `runtime`.
    pub const fn new(runtime: R) -> Self {
        Self { runtime }
    }

    /// The interpreter this host speaks to.
    pub fn runtime(&self) -> &R {
        &self.runtime
    }
}

impl<R: PythonRuntime> PythonHost<R> {
    /// Call a shell function with positional string arguments, returning its raw answer.
    pub fn call(&self, function: &str, args: &[&str]) -> Result<String, String> {
        self.runtime.call(function, args)
    }

    /// Call a shell function and decode its JSON answer.
    ///
    /// The error text is the one the runtime produced (a traceback for an exception, the
    /// serde error for a malformed answer). The shell shows it; the caller decides
    /// whether a failure means "not mine" or a refusal.
    pub fn json<T: for<'de> Deserialize<'de>>(
        &self,
        function: &str,
        args: &[&str],
    ) -> Result<T, String> {
        let raw = self.runtime.call(function, args)?;
        serde_json::from_str(&raw).map_err(|error| error.to_string())
    }

    /// Whether the registry has rows for `object`.
    ///
    /// A `Settings/<Feature>` node exists only once a provider registered that feature,
    /// and the registry is what `properties` reads, so the rows are the ownership test
    /// rather than a second query into the registry. It costs one round-trip on a
    /// settings *edit* (user-paced), never on a frame.
    fn owns_settings_node(&self, object: &str) -> bool {
        !self
            .json::<Vec<PropRow>>("properties", &[object])
            .unwrap_or_default()
            .is_empty()
    }

    /// The rows the script's registry would show.
    ///
    /// Catalogue ids are included on purpose: the backend keeps the catalogue's metadata
    /// and takes this row's live `enabled`/`checked`, because `IsActive`/`IsChecked` are
    /// the Gui app's answers. Workbench commands (ids the catalogue does not list) are
    /// appended by the backend.
    fn command_rows(&self) -> Vec<CommandInfo> {
        self.json("commands", &[]).unwrap_or_default()
    }
}

/// Whether `object` names something the script owns rather than the document.
///
/// The synthetic namespace is `Settings` and its `/`-separated children. A document
/// object's name never contains `/`, which is why the two cannot collide — the app
/// script's resolver relies on the same rule (`_settings_feature`).
fn settings_target(object: &str) -> bool {
    object == SETTINGS_ROOT || object.starts_with("Settings/")
}

/// The answer `add_datum_plane` reads back: the created object's name, or why not.
#[derive(Debug, Clone, Deserialize)]
struct DatumPlaneOutcome {
    ok: bool,
    #[serde(default)]
    object: String,
    #[serde(default)]
    error: String,
}

impl<R: PythonRuntime> ScriptHost for PythonHost<R> {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        self.json("evaluate", &[code])
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        // A facade with no filesystem loaded nothing; say so without crossing into an
        // interpreter whose `load_workbenches` would raise.
        let Some(roots) = self.runtime.mods_roots() else {
            return Ok(WorkbenchReport::default());
        };
        let dirs: Vec<String> = roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let arg = serde_json::to_string(&dirs).map_err(|error| error.to_string())?;
        self.json("load_workbenches", &[&arg])
    }

    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String> {
        self.runtime.tool(name)
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        self.runtime.tool_command(name, verb)
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        self.runtime.inventor_scene()
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        self.runtime.event_listener()
    }

    /// The app script's `bootstrap()`: it imports the entry module and creates the sample
    /// document in the shared registry.
    ///
    /// This is the whole point of the hook. `Shell::boot_with` asks for the boot report
    /// *before* it reads the model tree or loads a workbench, so the document has to exist
    /// by the time this returns.
    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        self.runtime.bootstrap()
    }

    /// `--open`: the app script opens the file and drops the sample it just made.
    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        self.runtime.open_startup_document(boot)
    }

    /// The file formats this app accepts (`.FCStd`, the native `.ferrocad`, a `.py`
    /// startup script) are the script's to read.
    fn open_path(&self, path: &std::path::Path) -> Option<Result<OpenedDocument, String>> {
        self.runtime.open_path(path)
    }

    fn save_path(&self, path: &std::path::Path) -> Option<Result<(), String>> {
        self.runtime.save_path(path)
    }

    /// `FreeCAD.setActiveDocument`: the switch fires the Gui app's document listeners.
    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        self.runtime.activate_document(name)
    }

    fn hello(&self) -> Result<Option<String>, String> {
        self.runtime.hello()
    }

    fn document_grant(&self) -> DocumentGrant {
        self.runtime.document_grant()
    }

    /// The edition's display provider: the display mode, datum planes' glass, and any
    /// object whose mesh is not its OCCT `Shape`.
    ///
    /// `have` is passed through to the app script, which skips the default Shape
    /// tessellator for those objects — that is what keeps a document sync from copying
    /// every triangle through the interpreter and back.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        self.runtime.stage_meshes(have)
    }

    /// A workbench manipulator's commit (the Part edge arc, a height grip).
    ///
    /// `None` when no provider owns the commit, which sends it back to the document half
    /// — the generic `transform`/`apply` in a build with no edition mod. That is the
    /// documented opt-in: registering a provider for a manipulator id is how a mod takes
    /// it over.
    fn apply_interaction(&self, commit: &InteractionCommit, object: &str) -> Option<ApplyResult> {
        if !self.runtime.contract().interaction {
            return None;
        }
        let values = match serde_json::to_string(&commit.values) {
            Ok(values) => values,
            Err(error) => {
                return Some(ApplyResult {
                    ok: false,
                    error: error.to_string(),
                });
            }
        };
        let element = commit.element.clone().unwrap_or_default();
        match self.json::<Option<ApplyResult>>(
            "interaction",
            &[
                object,
                commit.manipulator.as_str(),
                commit.action.as_str(),
                &element,
                &values,
            ],
        ) {
            Ok(result) => result,
            Err(error) => Some(ApplyResult { ok: false, error }),
        }
    }

    fn add_datum_plane(
        &self,
        origin: Vector3,
        u: Vector3,
        v: Vector3,
    ) -> Option<Result<String, String>> {
        if !self.runtime.contract().interaction {
            return None;
        }
        Some(self.datum_plane(origin, u, v))
    }

    /// Every row the registry would show, catalogue ids included; the backend
    /// reconciles them (see [`PythonHost::command_rows`]).
    fn commands(&self) -> Vec<CommandInfo> {
        if !self.runtime.contract().commands {
            return Vec::new();
        }
        self.command_rows()
    }

    /// Run a command this surface owns.
    ///
    /// A catalogue id is this host's only when [`catalog::executor`] says so; the document
    /// backend's own verbs (`App_Move`, `App_Undo`, …) must fall through rather than be
    /// refused by an interpreter that merely has a class registered for the same id. A
    /// workbench command (an id the catalogue does not list) is this host's when the
    /// registry lists it.
    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !self.runtime.contract().commands {
            return None;
        }
        let mine = catalog::executor(id) == catalog::Executor::Script
            || (catalog::find(id).is_none()
                && self.command_rows().iter().any(|command| command.id == id));
        if !mine {
            return None;
        }
        Some(
            self.json("run_command", &[id])
                .unwrap_or_else(|error| CommandOutcome {
                    ok: false,
                    error,
                    ..Default::default()
                }),
        )
    }

    /// Only the synthetic `Settings` root.
    ///
    /// The app script's `model_tree` answers for the whole application — the `Settings`
    /// root, then every open document. The document half already builds the documents, so
    /// passing those through would show each one twice.
    fn extra_tree_nodes(&self) -> Vec<DocumentNode> {
        if !self.runtime.contract().tree {
            return Vec::new();
        }
        self.json::<Vec<DocumentNode>>("model_tree", &[])
            .unwrap_or_default()
            .into_iter()
            .filter(|node| node.name == SETTINGS_ROOT)
            .collect()
    }

    /// The rows of a `Settings` node this surface actually has, and nothing else.
    ///
    /// `None` is what sends the node to the document half, which answers without a Python
    /// round-trip and without a second formatter that could disagree with
    /// `ferrocad_core::present`. A real document object is therefore never this host's,
    /// and neither is a `Settings/<Feature>` node with no rows: the registry only lists a
    /// feature a mod registered, so an empty answer means "nothing is registered here" —
    /// not "this surface owns an empty card". The document half fills in its grid card
    /// for exactly those builds, so the two halves never both claim one node.
    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        if !self.runtime.contract().settings || !settings_target(object) {
            return None;
        }
        let rows = self
            .json::<Vec<PropRow>>("properties", &[object])
            .unwrap_or_default();
        (!rows.is_empty()).then_some(rows)
    }

    /// Write a row of a card this surface has.
    ///
    /// The same test as [`extra_properties`](Self::extra_properties), so a node the card
    /// pane renders is one whose edits land here: a feature with no registered provider is
    /// the document half's, not a refusal from an interpreter that has never heard of it.
    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        if !self.runtime.contract().settings
            || !settings_target(object)
            || !self.owns_settings_node(object)
        {
            return None;
        }
        Some(
            self.json("set_property", &[object, property, value])
                .unwrap_or_else(|error| SetResult {
                    ok: false,
                    error,
                    view: None,
                }),
        )
    }

    fn on_selection_changed(&self, items: &[SelectionItem]) {
        // A notification: the shell has already applied it, so a registry that cannot
        // take it must not fail the click that caused it.
        if !self.runtime.contract().selection {
            return;
        }
        if let Err(error) = self.set_selection(items) {
            eprintln!("ferrocad: python selection mirror: {error}");
        }
    }

    fn take_selection_requests(&self) -> Vec<SelectionRequest> {
        if !self.runtime.contract().selection {
            return Vec::new();
        }
        self.json("take_selection_requests", &[])
            .unwrap_or_default()
    }

    fn on_view_state_changed(&self, state: &ViewState) {
        if !self.runtime.contract().view_state {
            return;
        }
        if let Err(error) = self.set_view_state(state) {
            eprintln!("ferrocad: python view state mirror: {error}");
        }
    }

    fn appearance_view(&self) -> Option<CommandOutcome> {
        if !self.runtime.contract().view {
            return None;
        }
        self.json("appearance_view", &[]).ok()
    }

    fn grid_summary(&self) -> Option<GridSummary> {
        if !self.runtime.contract().view {
            return None;
        }
        self.json("grid_summary", &[]).ok()
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        if !self.runtime.contract().view {
            return None;
        }
        self.json("axis_summary", &[]).ok()
    }

    fn provider_features(&self) -> Vec<ProviderFeature> {
        if !self.runtime.contract().providers {
            return Vec::new();
        }
        self.json("provider_features", &[]).unwrap_or_default()
    }

    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        if !self.runtime.contract().providers {
            return None;
        }
        let payload = serde_json::json!({ "feature": feature, "backend": backend }).to_string();
        self.json("set_provider", &[&payload]).ok()
    }

    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        if !self.runtime.contract().providers {
            return None;
        }
        self.json("refresh_providers", &[feature]).ok()
    }

    fn provider_skin(&self, feature: &str) -> Option<String> {
        if !self.runtime.contract().providers {
            return None;
        }
        let payload = serde_json::json!({ "feature": feature }).to_string();
        self.json::<ProviderSkin>("provider_skin", &[&payload])
            .ok()
            .and_then(|skin| skin.source)
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Option<Vec<serde_json::Value>> {
        if !self.runtime.contract().interaction {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            specs: Vec<serde_json::Value>,
        }
        let payload = serde_json::json!({ "object": object, "element": element }).to_string();
        self.json::<Envelope>("manipulator_specs", &[&payload])
            .ok()
            .map(|envelope| envelope.specs)
    }

    /// A dialog a workbench opened, by the token it handed the shell.
    ///
    /// The document backend answers its own Move token before asking here, so the two
    /// cannot spend each other's dialogs. A facade with no command registry has no
    /// dialogs either, which is why this rides on the `commands` group.
    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        if !self.runtime.contract().commands {
            return None;
        }
        let payload = serde_json::json!({ "token": token, "values": values }).to_string();
        self.json("dialog_accept", &[&payload]).ok()
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        if !self.runtime.contract().commands {
            return None;
        }
        self.json("dialog_cancel", &[token]).ok()
    }

    fn gpu_adapter(&self) -> Option<String> {
        if !self.runtime.contract().view {
            return None;
        }
        // The helper returns the stored value itself, not JSON.
        self.runtime.call("gpu_adapter", &[]).ok()
    }

    fn set_gpu_adapter(&self, value: &str) -> Option<GpuPreferenceOutcome> {
        if !self.runtime.contract().view {
            return None;
        }
        let payload = serde_json::json!({ "value": value }).to_string();
        self.json("set_gpu_adapter", &[&payload]).ok()
    }
}

impl<R: PythonRuntime> PythonHost<R> {
    /// Push the shell's selection into the facade (`FreeCADGui.Selection`).
    fn set_selection(&self, items: &[SelectionItem]) -> Result<(), String> {
        let payload = serde_json::to_string(items).map_err(|error| error.to_string())?;
        let _: serde_json::Value = self.json("set_selection", &[&payload])?;
        Ok(())
    }

    /// Mirror the shell's presentation state to `FerroCADGui.ViewState`.
    fn set_view_state(&self, state: &ViewState) -> Result<(), String> {
        let payload = serde_json::json!({
            "grid": state.grid,
            "axis_cross": state.axis.enabled,
            "projection": state.projection.as_str(),
            "grid_minor": state.grid_minor,
            "grid_major": state.grid_major,
            "grid_adaptive": state.grid_adaptive,
            "grid_minor_style": state.grid_minor_style.as_str(),
            "grid_major_width": state.grid_major_width,
            "grid_minor_width": state.grid_minor_width,
            "grid_radius": state.grid_radius,
        })
        .to_string();
        let _: serde_json::Value = self.json("set_view_state", &[&payload])?;
        Ok(())
    }

    /// Ask the entry module to create a selectable datum plane at `origin`/`u`/`v`.
    fn datum_plane(&self, origin: Vector3, u: Vector3, v: Vector3) -> Result<String, String> {
        let payload = serde_json::json!({
            "origin": [origin.x, origin.y, origin.z],
            "u": [u.x, u.y, u.z],
            "v": [v.x, v.y, v.z],
        })
        .to_string();
        let outcome: DatumPlaneOutcome = self.json("add_datum_plane", &[&payload])?;
        if !outcome.ok {
            return Err(if outcome.error.is_empty() {
                "the edition did not create the plane".to_string()
            } else {
                outcome.error
            });
        }
        Ok(outcome.object)
    }
}
