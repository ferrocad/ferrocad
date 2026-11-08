//! The CPython half of the [`ScriptHost`] seam, on its own.
//!
//! [`RustAppBackend`](ferrocad_shell_rust::RustAppBackend) is the document half; this
//! is the script half alone, so the two compose into the desktop's engine:
//!
//! ```text
//! RustAppBackend::from_shared_registry()
//!     .with_script_host(Box::new(PythonScriptHost))
//! ```
//!
//! Both halves then point at the one document in
//! `ferrocad_core::application`, so `FreeCAD.ActiveDocument` in a workbench and the
//! model tree in the shell are the same object. The document answers the tree, the
//! rows of real objects, the stage and the edits; this answers what a script owns —
//! the synthetic `Settings/<Feature>` nodes, the provider registry, the commands a
//! workbench registered, its dialogs, and the console.
//!
//! # What is deliberately *not* here
//!
//! The five merged methods of `RustAppBackend` (`model_tree`, `properties`,
//! `set_property`, `dialog_accept`, `dialog_cancel`) are not implemented as
//! `AppBackend` calls here. The document half owns them and asks this half only for
//! the part it cannot answer, through [`extra_tree_nodes`], [`extra_properties`]
//! and [`set_extra_property`] — the hooks that keep a node the document has never
//! heard of (`Settings/Grid`) resolvable without turning every property read into a
//! Python round-trip.
//!
//! [`extra_tree_nodes`]: ScriptHost::extra_tree_nodes
//! [`extra_properties`]: ScriptHost::extra_properties
//! [`set_extra_property`]: ScriptHost::set_extra_property

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::catalog;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    WorkbenchReport,
};
use ferrocad_shell_api::view::ViewState;
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;

use crate::backend::{python_tool, python_tool_command};
use crate::events::PythonEventCallbacks;
use crate::python;

/// The synthetic root the app script puts above the documents in the model tree.
///
/// Its children are one `Settings/<Feature>` node per registered provider feature.
/// It is not a document: nothing about it is in `ferrocad_core`.
const SETTINGS_ROOT: &str = "Settings";

/// The embedded interpreter as a [`ScriptHost`].
///
/// Stateless: the interpreter, its facades and the document registry are all
/// process-wide, so a host is a handle rather than a holder. Every method is a
/// synchronous call into [`crate::python`].
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonScriptHost;

impl PythonScriptHost {
    /// A host over the already-`configure`d interpreter.
    pub fn new() -> Self {
        Self
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

/// The rows the script's registry would show.
///
/// Catalogue ids are included on purpose: the backend keeps the catalogue's metadata
/// and takes this row's live `enabled`/`checked`, because `IsActive`/`IsChecked` are
/// the Gui app's answers. Workbench commands (ids the catalogue does not list) are
/// appended by the backend.
fn command_rows() -> Vec<CommandInfo> {
    python::commands().unwrap_or_default()
}

impl ScriptHost for PythonScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        python::evaluate(code)
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        python::load_workbenches()
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        python_tool(name)
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        python_tool_command(name, verb)
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        python::inventor_scene()
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(PythonEventCallbacks)
    }

    /// The app script's `bootstrap()`: it starts the interpreter, imports the entry
    /// module and creates the sample document in the shared registry.
    ///
    /// This is the whole point of the hook. `Shell::boot_with` asks for the boot
    /// report *before* it reads the model tree or loads a workbench, so the document
    /// has to exist by the time this returns.
    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        python::bootstrap().map(Some)
    }

    /// `--open`: the app script opens the file and drops the sample it just made.
    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        python::open_startup_document(boot)
    }

    /// The file formats this app accepts (`.FCStd`, the native `.ferrocad`, a `.py`
    /// startup script) are the script's to read.
    fn open_path(&self, path: &std::path::Path) -> Option<Result<OpenedDocument, String>> {
        Some(python::open_document(path))
    }

    fn save_path(&self, path: &std::path::Path) -> Option<Result<(), String>> {
        Some(python::save_document(path))
    }

    /// `FreeCAD.setActiveDocument`: the switch fires the Gui app's document listeners.
    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        Some(python::activate_document(name))
    }

    fn hello(&self) -> Result<Option<String>, String> {
        python::hello().map(Some)
    }

    /// The edition's display provider: the display mode, datum planes' glass, and
    /// any object whose mesh is not its OCCT `Shape`.
    ///
    /// `have` is passed through to the app script, which skips the default Shape
    /// tessellator for those objects — that is what keeps a document sync from
    /// copying every triangle through the interpreter and back.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        Some(python::stage_meshes_skipping(have))
    }

    /// A workbench manipulator's commit (the Part edge arc, a height grip).
    ///
    /// `None` when no provider owns the commit, which sends it back to the document
    /// half — the generic `transform`/`apply` in a build with no edition mod. That is
    /// the documented opt-in: registering a provider for a manipulator id is how a mod
    /// takes it over.
    fn apply_interaction(&self, commit: &InteractionCommit, object: &str) -> Option<ApplyResult> {
        match python::interaction(commit, object) {
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
        Some(python::add_datum_plane(origin, u, v))
    }

    /// Every row the registry would show, catalogue ids included; the backend
    /// reconciles them (see [`command_rows`]).
    fn commands(&self) -> Vec<CommandInfo> {
        command_rows()
    }

    /// Run a command this surface owns.
    ///
    /// A catalogue id is this host's only when [`catalog::executor`] says so; the
    /// document backend's own verbs (`App_Move`, `App_Undo`, …) must fall through
    /// rather than be refused by an interpreter that merely has a class registered
    /// for the same id. A workbench command (an id the catalogue does not list) is
    /// this host's when the registry lists it.
    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        let mine = catalog::executor(id) == catalog::Executor::Script
            || (catalog::find(id).is_none()
                && command_rows().iter().any(|command| command.id == id));
        if !mine {
            return None;
        }
        Some(
            python::run_command(id).unwrap_or_else(|error| CommandOutcome {
                ok: false,
                error,
                ..Default::default()
            }),
        )
    }

    /// Only the synthetic `Settings` root.
    ///
    /// The app script's `model_tree` answers for the whole application — the
    /// `Settings` root, then every open document. The document half already builds
    /// the documents, so passing those through would show each one twice.
    fn extra_tree_nodes(&self) -> Vec<DocumentNode> {
        python::model_tree()
            .unwrap_or_default()
            .into_iter()
            .filter(|node| node.name == SETTINGS_ROOT)
            .collect()
    }

    /// The rows of a `Settings` node, and nothing else.
    ///
    /// `None` is what sends a real document object to the document half, which
    /// answers without a Python round-trip and without a second formatter that
    /// could disagree with `ferrocad_core::present`.
    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        settings_target(object).then(|| python::properties(object).unwrap_or_default())
    }

    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        if !settings_target(object) {
            return None;
        }
        Some(
            python::set_property(object, property, value).unwrap_or_else(|error| SetResult {
                ok: false,
                error,
                view: None,
            }),
        )
    }

    fn on_selection_changed(&self, items: &[SelectionItem]) {
        // A notification: the shell has already applied it, so a registry that
        // cannot take it must not fail the click that caused it.
        if let Err(error) = python::set_selection(items) {
            eprintln!("ferrocad: python selection mirror: {error}");
        }
    }

    fn take_selection_requests(&self) -> Vec<SelectionRequest> {
        python::take_selection_requests().unwrap_or_default()
    }

    fn on_view_state_changed(&self, state: &ViewState) {
        if let Err(error) = python::set_view_state(state) {
            eprintln!("ferrocad: python view state mirror: {error}");
        }
    }

    fn appearance_view(&self) -> Option<CommandOutcome> {
        python::appearance_view().ok()
    }

    fn grid_summary(&self) -> Option<GridSummary> {
        python::grid_summary().ok()
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        python::axis_summary().ok()
    }

    fn provider_features(&self) -> Vec<ProviderFeature> {
        python::provider_features().unwrap_or_default()
    }

    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        python::set_provider(feature, backend).ok()
    }

    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        python::refresh_providers(feature).ok()
    }

    fn provider_skin(&self, feature: &str) -> Option<String> {
        python::provider_skin(feature).ok().flatten()
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Option<Vec<serde_json::Value>> {
        python::manipulator_specs(object, element).ok()
    }

    /// A dialog a workbench opened, by the token it handed the shell.
    ///
    /// The document backend answers its own Move token before asking here, so the
    /// two cannot spend each other's dialogs.
    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        python::dialog_accept(token, values).ok()
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        python::dialog_cancel(token).ok()
    }

    fn gpu_adapter(&self) -> Option<String> {
        python::gpu_adapter().ok()
    }

    fn set_gpu_adapter(&self, value: &str) -> Option<GpuPreferenceOutcome> {
        python::set_gpu_adapter(value).ok()
    }
}
