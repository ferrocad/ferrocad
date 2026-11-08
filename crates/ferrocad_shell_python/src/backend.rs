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
    install_composed(cx, &[Runtime::Cpython], &[]);
}

/// Install the composition for one scripting configuration.
///
/// A configuration this build cannot serve is reported and replaced by [`Runtime::Plain`],
/// the same way the browser falls back: a scripting surface that cannot start must not
/// cost the window.
pub fn install_with(cx: &mut App, runtime: Runtime) {
    install_composed(cx, &[runtime], &[]);
}

/// [`install_with`] plus the wasm bundles to load beside it.
pub fn install_composed(cx: &mut App, runtimes: &[Runtime], addons: &[std::path::PathBuf]) {
    match compose_all(runtimes, addons) {
        Ok(backend) => ferrocad_shell_core::app::install_backend(cx, backend),
        Err(error) => {
            eprintln!("ferrocad: {error}");
            eprintln!(
                "ferrocad: continuing without a script half; the document half answers alone"
            );
            let plain = RustAppBackend::from_shared_registry();
            ferrocad_shell_core::app::install_backend(cx, Box::new(plain));
        }
    }
}

/// Compose the backend for one scripting configuration.
pub fn compose(runtime: Runtime) -> Result<Box<dyn AppBackend>, String> {
    compose_with(runtime, &[])
}

/// [`compose`] with sandboxed wasm workbenches loaded beside the script half.
///
/// `Plain` and `Cpython` are what this crate always links. `Pywasm` and `Js` are halves a
/// build opts into; the bundles behind `addons` are a third. `Err` is a request this build
/// cannot serve, not a failure — [`install_composed`] falls back to the document half. An
/// addon that will not load is logged and skipped, because one broken workbench must not
/// cost the window.
pub fn compose_with(
    runtime: Runtime,
    addons: &[std::path::PathBuf],
) -> Result<Box<dyn AppBackend>, String> {
    compose_all(&[runtime], addons)
}

/// Compose a **chain** of script halves, the first of which is the primary (B8c).
///
/// The list has to name one host per language: `cpython,pywasm` is refused here as well as
/// at the parser, so a caller that builds the list by hand cannot bypass the rule.
/// `plain` in a list with anything else is refused for the same reason — it is the absence
/// of a host. `Err` is a request this build cannot serve, not a failure.
pub fn compose_all(
    runtimes: &[Runtime],
    addons: &[std::path::PathBuf],
) -> Result<Box<dyn AppBackend>, String> {
    Runtime::validate_list(runtimes)?;
    let hosts = script_for_all(runtimes)?;
    let script = with_addons(hosts, addons)?;

    let mut backend = RustAppBackend::from_shared_registry();
    if let Some(script) = script {
        backend = backend.with_script_host(script);
    }
    Ok(Box::new(backend))
}

/// The script halves `runtimes` names, in the order it named them; `Plain` adds none.
fn script_for_all(
    runtimes: &[Runtime],
) -> Result<Vec<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>>, String> {
    let mut hosts = Vec::new();
    for runtime in runtimes {
        if let Some(host) = script_for(*runtime)? {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

/// The script half one configuration names, if it names one.
fn script_for(
    runtime: Runtime,
) -> Result<Option<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>>, String> {
    Ok(match runtime {
        Runtime::Plain => None,
        Runtime::Cpython => Some(Box::new(PythonScriptHost)),
        Runtime::Pywasm => Some(pywasm()?),
        Runtime::Js => Some(js_page()?),
    })
}

/// Wrap the halves in the addon composition, when there is anything to compose.
///
/// One half and no bundles is installed directly, so the common configuration does not pay
/// for the chain; anything else becomes an `Addons` chain, whose first half is the primary.
fn with_addons(
    hosts: Vec<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>>,
    addons: &[std::path::PathBuf],
) -> Result<Option<Box<dyn ferrocad_shell_api::ScriptHost>>, String> {
    #[cfg(feature = "addons")]
    {
        // A path may be a directory of workbenches (`<dir>/<Name>/addon.wasm`) or one
        // bundle file, so `--addons mods` and `--addons mods/toy/addon.wasm` both work.
        let mut bundles = Vec::new();
        for path in addons {
            if path.is_dir() {
                bundles.extend(ferrocad_shell_wasm::packs(std::slice::from_ref(path)));
            } else {
                bundles.push(path.clone());
            }
        }
        if !addons.is_empty() && bundles.is_empty() {
            eprintln!(
                "ferrocad: no wasm addons under {}; the script half runs alone",
                addons
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if bundles.is_empty() && hosts.len() <= 1 {
            return Ok(hosts
                .into_iter()
                .next()
                .map(|host| host as Box<dyn ferrocad_shell_api::ScriptHost>));
        }

        // The bundles reach the same registry the document half answers from, so an
        // addon's `create` lands in the tree beside a CPython edit — and the doorway is
        // given a sink, so the **primary** hears about that edit exactly as it hears
        // about the shell's own.
        use std::sync::Arc;

        let hosts: Vec<Arc<dyn ferrocad_shell_api::ScriptHost + Send + Sync>> =
            hosts.into_iter().map(Arc::from).collect();
        let document: Arc<dyn ferrocad_shell_api::DocumentApi> = match hosts.first() {
            Some(script) => Arc::new(ferrocad_doc_api::DocApi::with_events(
                ferrocad_shell_wasm::announce_to(Arc::clone(script)),
            )),
            None => Arc::new(ferrocad_doc_api::DocApi::new()),
        };
        let (host, errors) = ferrocad_shell_wasm::Addons::boot_chain(hosts, &bundles, document);
        for error in &errors {
            eprintln!("ferrocad: addon skipped: {error}");
        }
        Ok(Some(Box::new(host)))
    }
    #[cfg(not(feature = "addons"))]
    {
        if !addons.is_empty() {
            return Err("`--addons` needs a build with the `addons` feature: \
                 `cargo run -p ferrocad_parametric --features addons -- --addons mods`"
                .to_string());
        }
        if hosts.len() > 1 {
            return Err("combining script hosts needs a build with the `addons` feature, \
                        which is where the chain lives: `cargo run -p ferrocad_parametric \
                        --features addons,js -- --script-host=cpython,js`"
                .to_string());
        }
        Ok(hosts
            .into_iter()
            .next()
            .map(|host| host as Box<dyn ferrocad_shell_api::ScriptHost>))
    }
}

/// The desktop's RustPython half: the interpreter on the worker its compiler needs.
#[cfg(feature = "pywasm")]
fn pywasm() -> Result<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>, String> {
    let host = ferrocad_shell_pywasm::ThreadedScriptHost::live()
        .map_err(|error| format!("the RustPython thread did not boot: {error}"))?;
    Ok(Box::new(host))
}

#[cfg(not(feature = "pywasm"))]
fn pywasm() -> Result<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>, String> {
    Err("`--script-host=pywasm` needs a build with the `pywasm` feature: \
         `cargo run -p ferrocad_parametric --features pywasm -- --script-host=pywasm`"
        .to_string())
}

/// The desktop's JS page: the same `Page` the browser has, over an embedded QuickJS.
#[cfg(feature = "js")]
fn js_page() -> Result<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>, String> {
    use std::sync::Arc;

    let page: Arc<dyn ferrocad_shell_js::Page> = Arc::new(
        ferrocad_shell_js::EnginePage::from_mods_dir()
            .map_err(|error| format!("the JS page did not boot: {error}"))?,
    );
    // The page's own doorway announces to the page, as the browser's does.
    let events: Arc<dyn ferrocad_shell_api::DocumentEventSink> =
        Arc::new(ferrocad_shell_js::PageEvents::new(Arc::clone(&page)));
    let document = Arc::new(ferrocad_doc_api::DocApi::with_events(events));
    Ok(Box::new(ferrocad_shell_js::JsScriptHost::with_kernel(
        page, document,
    )))
}

#[cfg(not(feature = "js"))]
fn js_page() -> Result<Box<dyn ferrocad_shell_api::ScriptHost + Send + Sync>, String> {
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
