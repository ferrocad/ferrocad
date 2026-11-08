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

use ferrocad_shell_api::Runtime;
use ferrocad_shell_api::app::AppBackend;
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
    install_with(cx, Runtime::Cpython);
}

/// Install the composition for one scripting configuration.
///
/// A configuration this build cannot serve is reported and replaced by [`Runtime::Plain`],
/// the same way the browser falls back: a scripting surface that cannot start must not
/// cost the window.
pub fn install_with(cx: &mut App, runtime: Runtime) {
    match compose(runtime) {
        Ok(backend) => ferrocad_shell_core::app::install_backend(cx, backend),
        Err(error) => {
            eprintln!("ferrocad: {error}");
            eprintln!(
                "ferrocad: continuing without a script half; the document half answers alone"
            );
            let plain = compose(Runtime::Plain)
                .expect("the plain configuration is always compilable");
            ferrocad_shell_core::app::install_backend(cx, plain);
        }
    }
}

/// Compose the backend for one scripting configuration.
///
/// `Plain` and `Cpython` are what this crate always links. `Pywasm` and `Js` are halves a
/// build opts into: the JS page is behind the `js` feature, and RustPython needs a
/// big-stack worker before it can be installed here (see
/// `docs/project/tasks/backend-parity.md` B7). `Err` is a request this build cannot
/// serve, not a failure — [`install_with`] falls back to the document half.
pub fn compose(runtime: Runtime) -> Result<Box<dyn AppBackend>, String> {
    Ok(match runtime {
        Runtime::Plain => Box::new(RustAppBackend::from_shared_registry()),
        Runtime::Cpython => Box::new(engine()),
        Runtime::Pywasm => pywasm()?,
        Runtime::Js => js_page()?,
    })
}

/// The desktop's RustPython half: the interpreter on the worker its compiler needs.
#[cfg(feature = "pywasm")]
fn pywasm() -> Result<Box<dyn AppBackend>, String> {
    let script = ferrocad_shell_pywasm::ThreadedScriptHost::live()
        .map_err(|error| format!("the RustPython thread did not boot: {error}"))?;
    Ok(Box::new(
        RustAppBackend::from_shared_registry().with_script_host(Box::new(script)),
    ))
}

#[cfg(not(feature = "pywasm"))]
fn pywasm() -> Result<Box<dyn AppBackend>, String> {
    Err("`--script-host=pywasm` needs a build with the `pywasm` feature: \
         `cargo run -p ferrocad_parametric --features pywasm -- --script-host=pywasm`"
        .to_string())
}

/// The desktop's JS page: the same `Page` the browser has, over an embedded QuickJS.
#[cfg(feature = "js")]
fn js_page() -> Result<Box<dyn AppBackend>, String> {
    use std::sync::Arc;

    let page: Arc<dyn ferrocad_shell_js::Page> = Arc::new(
        ferrocad_shell_js::EnginePage::from_mods_dir()
            .map_err(|error| format!("the JS page did not boot: {error}"))?,
    );
    let events: Arc<dyn ferrocad_shell_api::DocumentEventSink> =
        Arc::new(ferrocad_shell_js::PageEvents::new(Arc::clone(&page)));
    let document = Arc::new(ferrocad_doc_api::DocApi::with_events(events));
    Ok(Box::new(
        RustAppBackend::from_shared_registry()
            .with_script_host(Box::new(ferrocad_shell_js::JsScriptHost::with_kernel(page, document))),
    ))
}

#[cfg(not(feature = "js"))]
fn js_page() -> Result<Box<dyn AppBackend>, String> {
    Err("`--script-host=js` needs a build with the `js` feature: \
         `cargo run -p ferrocad_parametric --features js -- --script-host=js`"
        .to_string())
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
