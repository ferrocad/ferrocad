//! The desktop's engine: the composition the shell installs.
//!
//! [`engine`] is one `AppBackend` — [`RustAppBackend`] over the process-wide document
//! registry, with [`PythonScriptHost`] as its script half — so a document question is
//! answered in Rust and a scripting question through [`crate::python`].
//!
//! This module used to hold `PythonAppBackend`, which answered *every* method through
//! the interpreter. B5 of `docs/project/tasks/backend-parity.md` retired it: the
//! composition answers the same surface, in one core, and the harnesses that relied on
//! the legacy type now link the bindings instead.

use pyo3::prelude::{PyAnyMethods, Python};

use ferrocad_shell_rust::RustAppBackend;
use gpui::App;

use crate::script_host::PythonScriptHost;

/// Install the composed backend as this app's engine.
///
/// The desktop is one `AppBackend`: the Rust document half over the process-wide
/// registry, with the embedded interpreter as its script half. Both halves see the
/// same document, which is what makes `FreeCAD.ActiveDocument` in a workbench and
/// the model tree in the window the same object.
///
/// Call after `python::configure` and before `Shell::boot`.
pub fn install(cx: &mut App) {
    ferrocad_shell_core::app::install_backend(cx, Box::new(engine()));
}

/// The desktop's engine, for a host that installs it itself (`--check-init`).
///
/// Nothing is built here: `bootstrap()` starts the interpreter and adopts the
/// document it creates, so composing the two halves cannot put a sample document in
/// front of the real one.
pub fn engine() -> RustAppBackend {
    RustAppBackend::from_shared_registry().with_script_host(Box::new(PythonScriptHost))
}

/// Build the Python-backed tool `name` through the app script's bridge.
///
/// The bridge is a `ferrocad.stage.Tool` instance; [`ferrocad_stage_py::PyTool`]
/// makes it a native [`ferrocad_stage::Tool`], so the viewport treats it exactly
/// like the Rust tools. A missing facade or bridge is reported as "no such
/// tool" by the caller, never as a panic: a build without the Python stage
/// extension still runs the rest of the shell.
pub(crate) fn python_tool(name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
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
pub(crate) fn python_tool_command(name: &str, verb: &str) -> bool {
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
