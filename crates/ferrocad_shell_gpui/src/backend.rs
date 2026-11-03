//! The native [`AppBackend`]: the embedded-CPython bridge.
//!
//! Every method forwards to [`crate::python`], which is a synchronous JSON bridge to
//! the app's Python script. The backend exists so the platform-neutral shell
//! (`ferrocad_shell_core`) can name a seam instead of `pyo3`.

use pyo3::prelude::{PyAnyMethods, Python};

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_shell_core::app::AppBackend;
use ferrocad_shell_core::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_core::display::StageMesh;
use ferrocad_shell_core::selection::SelectionItem;
use ferrocad_shell_core::types::{
    AxisSummary, ApplyResult, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome, GridSummary,
    OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult, WorkbenchReport,
};
use ferrocad_shell_core::view::ViewState;
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;
use gpui::App;

use crate::python;

/// The engine behind the native shell: the embedded interpreter and its app script.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonAppBackend;

impl PythonAppBackend {
    /// A backend over the already-`configure`d interpreter.
    pub fn new() -> Self {
        Self
    }
}

impl AppBackend for PythonAppBackend {
    fn bootstrap(&mut self) -> Result<Bootstrap, String> {
        python::bootstrap()
    }

    fn open_startup_document(
        &mut self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        python::open_startup_document(boot)
    }

    fn hello(&self) -> Result<String, String> {
        python::hello()
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        python::load_workbenches()
    }

    fn model_tree(&self) -> Result<Vec<DocumentNode>, String> {
        python::model_tree()
    }

    fn properties(&self, object: &str) -> Result<Vec<PropRow>, String> {
        python::properties(object)
    }

    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        python::stage_meshes()
    }

    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        python::commands()
    }

    fn appearance_view(&self) -> Result<CommandOutcome, String> {
        python::appearance_view()
    }

    fn set_view_state(&self, state: &ViewState) -> Result<(), String> {
        python::set_view_state(state)
    }

    fn set_selection(&self, items: &[SelectionItem]) -> Result<(), String> {
        python::set_selection(items)
    }

    fn take_selection_requests(&mut self) -> Result<Vec<SelectionRequest>, String> {
        python::take_selection_requests()
    }

    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        python::evaluate(code)
    }

    fn rotation_quaternion(&self, object: &str) -> Option<[f64; 4]> {
        python::rotation_quaternion(object)
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        python_tool(name)
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        python_tool_command(name, verb)
    }

    fn set_property(&mut self, object: &str, property: &str, value: &str) -> Result<SetResult, String> {
        python::set_property(object, property, value)
    }

    fn run_command(&mut self, id: &str) -> Result<CommandOutcome, String> {
        python::run_command(id)
    }

    fn set_provider(&mut self, feature: &str, backend: &str) -> Result<CommandOutcome, String> {
        python::set_provider(feature, backend)
    }

    fn refresh_providers(&mut self, feature: &str) -> Result<CommandOutcome, String> {
        python::refresh_providers(feature)
    }

    fn dialog_accept(
        &mut self,
        token: &str,
        values: &serde_json::Value,
    ) -> Result<CommandOutcome, String> {
        python::dialog_accept(token, values)
    }

    fn dialog_cancel(&mut self, token: &str) -> Result<CommandOutcome, String> {
        python::dialog_cancel(token)
    }

    fn add_datum_plane(
        &mut self,
        origin: Vector3,
        u: Vector3,
        v: Vector3,
    ) -> Result<String, String> {
        python::add_datum_plane(origin, u, v)
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, String> {
        python::manipulator_specs(object, element)
    }

    fn apply_interaction(
        &mut self,
        commit: &InteractionCommit,
        object: &str,
    ) -> Result<ApplyResult, String> {
        python::apply_interaction(commit, object)
    }

    fn grid_summary(&self) -> Result<GridSummary, String> {
        python::grid_summary()
    }

    fn axis_summary(&self) -> Result<AxisSummary, String> {
        python::axis_summary()
    }

    fn provider_features(&self) -> Result<Vec<ProviderFeature>, String> {
        python::provider_features()
    }

    fn provider_skin(&self, feature: &str) -> Result<Option<String>, String> {
        python::provider_skin(feature)
    }

    fn gpu_adapter(&self) -> Result<String, String> {
        python::gpu_adapter()
    }

    fn set_gpu_adapter(&mut self, value: &str) -> Result<GpuPreferenceOutcome, String> {
        python::set_gpu_adapter(value)
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        python::inventor_scene()
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(crate::events::PythonEventCallbacks)
    }
}

/// Install the Python backend as this app's engine.
///
/// Call after `python::configure` and before `Shell::boot`.
pub fn install(cx: &mut App) {
    ferrocad_shell_core::app::install_backend(cx, Box::new(PythonAppBackend::new()));
}

/// Build the Python-backed tool `name` through the app script's bridge.
///
/// The bridge is a `ferrocad.stage.Tool` instance; [`ferrocad_stage_py::PyTool`]
/// makes it a native [`ferrocad_stage::Tool`], so the viewport treats it exactly
/// like the Rust tools. A missing facade or bridge is reported as "no such
/// tool" by the caller, never as a panic: a build without the Python stage
/// extension still runs the rest of the shell.
fn python_tool(name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
    Python::with_gil(|py| {
        let module = py
            .import("ferrocad_shell")
            .map_err(|error| error.to_string())?;
        let object = module
            .getattr("tool_bridge")
            .map_err(|error| error.to_string())?
            .call1((name,))
            .map_err(|error| error.to_string())?;
        let tool = ferrocad_stage_py::extract_tool(py, object.unbind())
            .map_err(|error| error.to_string())?;
        Ok(Box::new(tool) as Box<dyn ferrocad_stage::Tool>)
    })
}

/// Send a session verb (`cancel`/`finish`) to a Python-backed tool.
fn python_tool_command(name: &str, verb: &str) -> bool {
    Python::with_gil(|py| {
        let result = py
            .import("ferrocad_shell")
            .and_then(|module| module.getattr("tool_command"))
            .and_then(|function| function.call1((name, verb)))
            .and_then(|value| value.extract::<bool>());
        match result {
            Ok(handled) => handled,
            Err(error) => {
                eprintln!("ferrocad: python tool {name}/{verb}: {error}");
                false
            }
        }
    })
}
