//! Embedded-CPython bridge for the app shell.
//!
//! The shell never touches the document model directly: like a workbench, it
//! goes through the public `FreeCAD` Python API. This module boots the
//! interpreter, puts the app's script directories on `sys.path`, and marshals
//! small JSON payloads across the boundary.
//!
//! The Python *scripts* are not part of this library. The importing app declares
//! them through [`configure`] (from its `HostConfig`): the module to boot and the
//! directories to search. This mirrors FreeCAD, where the interpreter belongs to
//! the application but the workbench (`Mod/`) scripts ship with the distribution.

use std::path::PathBuf;
use std::sync::OnceLock;

use pyo3::prelude::*;
use pyo3::types::PyAnyMethods;
use serde::Deserialize;

/// The Python module the shell boots and then calls. It is the app's script, so
/// the library ships no copy; [`configure`] sets it before boot.
static ENTRY_MODULE: OnceLock<String> = OnceLock::new();

/// Extra `sys.path` entries supplied by the app (its startup scripts and mods).
static APP_PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Workbench (`mods/`) directories scanned at boot.
static MODS_PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Configure the app side of the boundary, before boot.
///
/// `entry_module` must expose the shell's data functions (`bootstrap`,
/// `model_tree`, `properties`, `set_property`, `evaluate`). `python_paths` are
/// the app's script and mod directories, added to `sys.path`. `mods_paths` are
/// the workbench directories scanned for `Init.py`/`InitGui.py`.
pub fn configure(entry_module: &str, python_paths: &[PathBuf], mods_paths: &[PathBuf]) {
    let _ = ENTRY_MODULE.set(entry_module.to_string());
    let _ = APP_PATHS.set(python_paths.to_vec());
    let _ = MODS_PATHS.set(mods_paths.to_vec());
}

/// The app's entry module.
///
/// Falls back to the base app's development module so the library's own tests can
/// boot without a host; a real app always calls [`configure`].
fn entry_module() -> &'static str {
    ENTRY_MODULE.get().map(String::as_str).unwrap_or("ferrocad_shell")
}

/// Shell status returned by [`bootstrap`].
#[derive(Debug, Clone, Deserialize)]
pub struct Bootstrap {
    pub version: String,
    pub backend: String,
    pub document: String,
}

/// The outcome of loading workbench scripts: what loaded, and what raised.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkbenchReport {
    #[serde(default)]
    pub loaded: Vec<String>,
    #[serde(default)]
    pub errors: Vec<WorkbenchError>,
}

/// A workbench script that raised. Loading is best-effort: the shell reports
/// these instead of failing.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkbenchError {
    pub workbench: String,
    pub script: String,
    #[serde(default)]
    pub traceback: String,
}

/// A document in the model tree.
#[derive(Debug, Clone, Deserialize)]
pub struct DocumentNode {
    pub name: String,
    pub label: String,
    pub objects: Vec<ObjectNode>,
}

/// An object inside a document.
#[derive(Debug, Clone, Deserialize)]
pub struct ObjectNode {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub type_id: String,
}

/// One row of the property editor.
#[derive(Debug, Clone, Deserialize)]
pub struct PropRow {
    pub name: String,
    #[serde(rename = "type")]
    pub type_id: String,
    pub value: String,
    pub status: String,
    /// Whether the editor can change this property.
    #[serde(default)]
    pub editable: bool,
    /// The editor kind (`text`, `bool`, `int`, `float`, `quantity`, `enum`).
    /// Used to pick the control once the editor has more than a text field.
    #[serde(default)]
    #[allow(dead_code)]
    pub kind: String,
}

/// The outcome of a property edit.
#[derive(Debug, Clone, Deserialize)]
pub struct SetResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

/// The outcome of evaluating a console line.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalResult {
    pub ok: bool,
    pub output: String,
}

/// Locate the `python/` facade directory (the one containing `FreeCAD/`).
///
/// Checks `FERROCAD_PYTHON_PATH`, then walks up from the current directory —
/// which covers launching from either the workspace root or the crate dir.
pub fn find_python_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("FERROCAD_PYTHON_PATH") {
        return Ok(PathBuf::from(path));
    }
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let candidate = dir.join("python");
        if candidate.join("FreeCAD").join("__init__.py").is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("could not locate the python/ facade directory (set FERROCAD_PYTHON_PATH)".to_string())
}

/// Call a function on the app's entry module that returns JSON, and decode it.
fn call_json<T: for<'de> Deserialize<'de>>(func: &str, arg: Option<&str>) -> Result<T, String> {
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        let f = module.getattr(func).map_err(|e| e.to_string())?;
        let value = match arg {
            Some(a) => f.call1((a,)),
            None => f.call0(),
        }
        .map_err(|e| e.to_string())?;
        value.extract::<String>().map_err(|e| e.to_string())
    })?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Boot the interpreter: add `python/` to `sys.path`, import the shell helpers
/// and create the sample document.
pub fn bootstrap() -> Result<Bootstrap, String> {
    let dir = find_python_dir()?;
    let dir = dir.to_string_lossy().into_owned();
    let app: Vec<String> = APP_PATHS
        .get()
        .map(|paths| {
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let sys = py.import("sys").map_err(|e| e.to_string())?;
        let path = sys.getattr("path").map_err(|e| e.to_string())?;
        // A duplicate entry is harmless; inserting unconditionally is simplest.
        // The app's directories go first, then the facade directory.
        for p in app.iter().rev() {
            path.call_method1("insert", (0, p.as_str()))
                .map_err(|e| e.to_string())?;
        }
        path.call_method1("insert", (0, dir.as_str())).map_err(|e| e.to_string())?;

        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("bootstrap")
            .map_err(|e| e.to_string())?
            .call0()
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// The workbench directories to scan: explicit config, then `FERROCAD_MODS_PATH`,
/// then a `mods/` directory beside the discovered `python/` tree.
fn mods_dirs() -> Vec<PathBuf> {
    if let Some(paths) = MODS_PATHS.get() {
        if !paths.is_empty() {
            return paths.clone();
        }
    }
    if let Ok(value) = std::env::var("FERROCAD_MODS_PATH") {
        let paths: Vec<PathBuf> = std::env::split_paths(&value).collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    if let Ok(python_dir) = find_python_dir() {
        if let Some(parent) = python_dir.parent() {
            let mods = parent.join("mods");
            if mods.is_dir() {
                return vec![mods];
            }
        }
    }
    Vec::new()
}

/// Load workbench scripts (`Init.py`/`InitGui.py`) from the configured
/// directories. A broken workbench comes back as an entry in `errors`, never as a
/// hard failure.
pub fn load_workbenches() -> Result<WorkbenchReport, String> {
    let dirs: Vec<String> = mods_dirs()
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let arg = serde_json::to_string(&dirs).map_err(|e| e.to_string())?;
    call_json("load_workbenches", Some(&arg))
}

/// The console "hello world" line.
pub fn hello() -> Result<String, String> {
    Python::with_gil(|py| {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("hello")
            .map_err(|e| e.to_string())?
            .call0()
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })
}

pub fn model_tree() -> Result<Vec<DocumentNode>, String> {
    call_json("model_tree", None)
}

pub fn properties(object: &str) -> Result<Vec<PropRow>, String> {
    call_json("properties", Some(object))
}

pub fn evaluate(code: &str) -> Result<EvalResult, String> {
    call_json("evaluate", Some(code))
}

/// Set one property from its editor string. The Python side wraps the change in
/// a named transaction and recomputes.
pub fn set_property(object: &str, property: &str, value: &str) -> Result<SetResult, String> {
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("set_property")
            .map_err(|e| e.to_string())?
            .call1((object, property, value))
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}
