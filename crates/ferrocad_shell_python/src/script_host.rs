//! The CPython half of the [`ScriptHost`] seam, on its own.
//!
//! [`PythonAppBackend`](crate::PythonAppBackend) implements both halves, which is
//! why it cannot be composed with anything. This is the script half alone, so a
//! **document** backend can be paired with it:
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

use std::collections::HashSet;

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::catalog;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::{
    AxisSummary, DocumentNode, EvalResult, GpuPreferenceOutcome, GridSummary, PropRow,
    ProviderFeature, SelectionRequest, SetResult, WorkbenchReport,
};
use ferrocad_shell_api::view::ViewState;

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

/// The commands the script registered that the shared catalogue does not provide.
///
/// The app script registers one `FreeCADGui` command per catalogue id, so the
/// desktop and the browser run the same verbs; the document backend answers those
/// from [`catalog::builtin`] itself. Handing the whole registry over would list
/// every built-in twice — the palette, the app menu and the wheels all read this
/// list.
fn scripted_commands() -> Vec<CommandInfo> {
    let builtin: HashSet<&'static str> = catalog::builtin().iter().map(|def| def.id).collect();
    python::commands()
        .unwrap_or_default()
        .into_iter()
        .filter(|command| !builtin.contains(command.id.as_str()))
        .collect()
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

    /// A workbench's own verbs, without the catalogue's.
    fn commands(&self) -> Vec<CommandInfo> {
        scripted_commands()
    }

    /// Run a command this surface registered.
    ///
    /// An id the script does not list is not this host's to answer — the document
    /// backend's own verbs (`App_Move`, `App_Undo`, …) reach here too, and they must
    /// fall through rather than be refused by an interpreter that merely has a class
    /// registered for the same id.
    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !scripted_commands().iter().any(|command| command.id == id) {
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
