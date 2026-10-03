//! Embedded-CPython bridge for the app shell.
//!
//! The shell never touches the document model directly: like a workbench, it
//! goes through the public `FreeCAD` Python API. This module boots the
//! interpreter, puts the `python/` facade on `sys.path`, and marshals small
//! JSON payloads across the boundary (defined by `python/ferrocad_shell`).

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyAnyMethods;
use serde::Deserialize;

/// Host status returned by [`bootstrap`].
#[derive(Debug, Clone, Deserialize)]
pub struct Bootstrap {
    pub version: String,
    pub backend: String,
    pub document: String,
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

/// Call a `ferrocad_shell` function that returns a JSON string and decode it.
fn call_json<T: for<'de> Deserialize<'de>>(func: &str, arg: Option<&str>) -> Result<T, String> {
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let module = py.import("ferrocad_shell").map_err(|e| e.to_string())?;
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
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let sys = py.import("sys").map_err(|e| e.to_string())?;
        let path = sys.getattr("path").map_err(|e| e.to_string())?;
        // A duplicate entry is harmless; inserting unconditionally is simplest.
        path.call_method1("insert", (0, dir)).map_err(|e| e.to_string())?;

        let module = py.import("ferrocad_shell").map_err(|e| e.to_string())?;
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

/// The console "hello world" line.
pub fn hello() -> Result<String, String> {
    Python::with_gil(|py| {
        let module = py.import("ferrocad_shell").map_err(|e| e.to_string())?;
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
