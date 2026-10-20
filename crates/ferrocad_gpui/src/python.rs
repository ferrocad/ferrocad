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
use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods};
use serde::Deserialize;

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};

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
/// `model_tree`, `properties`, `set_property`, `evaluate`) and the viewport
/// functions (`stage_meshes`, `dress_up`). `python_paths` are the app's script and
/// mod directories, added to `sys.path`. `mods_paths` are the workbench directories
/// scanned for `Init.py`/`InitGui.py`.
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

/// The outcome of a dress-up edit.
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

/// Locate the `python/` facade directory (the one containing `FreeCAD/`).
///
/// Order: `FERROCAD_PYTHON_PATH`; a directory the host configured explicitly
/// (an extracted embedded payload, say); a loose payload beside the executable
/// (`<dir>/python` or the bundled `<dir>/../python`); then walking up from the
/// current directory, which covers launching from either the workspace root or
/// the crate dir.
pub fn find_python_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("FERROCAD_PYTHON_PATH") {
        return Ok(PathBuf::from(path));
    }
    // The host may hand us an extracted payload through `configure`. Only a path
    // that actually holds the facade counts (the same list may hold plain script
    // directories).
    if let Some(paths) = APP_PATHS.get() {
        if let Some(found) = paths.iter().find(|p| is_facade(p)) {
            return Ok(found.clone());
        }
    }
    // A loose payload beside the executable: the bundled layout is
    // `<root>/bin/ferrocad` with `<root>/python`, so check both `<dir>/python`
    // and `<dir>/../python`.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [dir.join("python"), dir.join("..").join("python")] {
                if is_facade(&candidate) {
                    return Ok(candidate);
                }
            }
        }
    }
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let candidate = dir.join("python");
        if is_facade(&candidate) {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("could not locate the python/ facade directory (set FERROCAD_PYTHON_PATH)".to_string())
}

/// Is `dir` a facade directory, i.e. does it contain `FreeCAD/__init__.py`?
fn is_facade(dir: &std::path::Path) -> bool {
    dir.join("FreeCAD").join("__init__.py").is_file()
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

        // Bring the scene layer (`pivy.coin`) up before any script imports it — the
        // analogue of `SoFCDB::init()` at GUI startup.
        ferrocad_inventor_py::install(py).map_err(|e| e.to_string())?;

        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        // The modern presentation layer (`ferrocad.stage`) attaches itself to the
        // `ferrocad` extension, so register it now that booting has imported it.
        ferrocad_stage_py::install(py).map_err(|e| e.to_string())?;
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

/// The tessellated document shapes the viewport draws.
///
/// Empty when the Part workbench is not linked, which the shell treats as "keep the
/// sample scene".
pub fn stage_meshes() -> Result<Vec<crate::dress_up::StageMesh>, String> {
    call_json("stage_meshes", None)
}

/// Create or update a `Part::Fillet`/`Part::Chamfer` and recompute.
pub fn apply_dress_up(
    object: &str,
    mode: &str,
    edge: &str,
    first: f64,
    second: f64,
) -> Result<ApplyResult, String> {
    let edges = serde_json::to_string(&[edge]).map_err(|e| e.to_string())?;
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("dress_up")
            .map_err(|e| e.to_string())?
            .call1((object, mode, edges.as_str(), first, second))
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
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

/// The `pivy.coin` script that builds the demo cube, the way a workbench would.
///
/// It is the Python twin of [`ferrocad_viewport::scene::sample_scene`], so the
/// viewport shows the same object however the scene is built. The host runs it and
/// extracts the
/// tree it leaves in `root`.
const INVENTOR_SCENE_SCRIPT: &str = r#"
from pivy import coin

root = coin.SoSeparator()

material = coin.SoMaterial()
material.diffuseColor.setValue((0.9, 0.6, 0.2, 1.0))
root.addChild(material)

transform = coin.SoTransform()
transform.rotation = (1.0, 1.0, 0.0, 0.6)
root.addChild(transform)

coords = coin.SoCoordinate3()
coords.point.setValues(0, [
    (-1.0, -1.0, -1.0), (1.0, -1.0, -1.0), (1.0, 1.0, -1.0), (-1.0, 1.0, -1.0),
    (-1.0, -1.0, 1.0), (1.0, -1.0, 1.0), (1.0, 1.0, 1.0), (-1.0, 1.0, 1.0),
])
root.addChild(coords)

faces = coin.SoIndexedFaceSet()
faces.coordIndex.setValues(0, [
    4, 5, 6, 4, 6, 7,
    1, 0, 3, 1, 3, 2,
    0, 4, 7, 0, 7, 3,
    5, 1, 2, 5, 2, 6,
    7, 6, 2, 7, 2, 3,
    0, 1, 5, 0, 5, 4,
])
root.addChild(faces)

# A workbench-style callback: report the pick when a mouse button is released.
def _report(event_cb):
    event = event_cb.getEvent()
    picked = event_cb.getPickedPoint()
    if picked is not None:
        print("ferrocad: SoEventCallback picked", picked.getPoint(),
              "face", picked.getDetail().getFaceIndex())

_events = coin.SoEventCallback()
_events.addEventCallback(coin.SoMouseButtonEvent.getClassTypeId(), _report)
root.addChild(_events)
"#;

/// Build the viewport scene the workbench way: run a
/// `from pivy import coin` script and extract the tree it builds.
///
/// The scene layer is registered into the embedded interpreter on demand (so the
/// demo works without a full [`bootstrap`]), and the tree is copied into a
/// [`Scene`] the renderer compiles — the same path the Rust sample uses.
pub fn inventor_scene() -> Result<Scene, String> {
    Python::with_gil(|py| -> Result<Scene, String> {
        ferrocad_inventor_py::install(py).map_err(|e| e.to_string())?;
        let globals = PyDict::new(py);
        let code = std::ffi::CString::new(INVENTOR_SCENE_SCRIPT).map_err(|e| e.to_string())?;
        py.run(&code, Some(&globals), None).map_err(|e| e.to_string())?;
        let root = globals
            .get_item("root")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the scene script did not leave a `root` node".to_string())?;
        ferrocad_inventor_py::extract_scene(&root).map_err(|e| e.to_string())
    })
}

/// Dispatch an input event (with its pick) to the active Python `SoEventCallback`s.
///
/// Returns whether any callback ran. The active callback ids travel in the neutral
/// [`Directives`] beside the draw queue (see `docs/directives.md`); the callables live
/// on the facade side, so the tree does not need to be re-extracted to reach them.
pub fn dispatch(directives: &Directives, event: &Event, hit: Option<&Hit>) -> bool {
    Python::with_gil(|py| {
        let result = ferrocad_inventor_py::dispatch_event(py, directives, event, hit)
            .unwrap_or_else(|e| {
                eprintln!("ferrocad: event dispatch failed: {e}");
                false
            });
        // A callback's `print` is block-buffered and the embedded interpreter is not
        // finalized, so flush the streams here rather than lose the output.
        flush_output(py);
        result
    })
}

/// Flush Python's standard streams, so `print` from a script reaches the terminal.
pub fn flush_output(py: Python<'_>) {
    if let Ok(sys) = py.import("sys") {
        for name in ["stdout", "stderr"] {
            if let Ok(stream) = sys.getattr(name) {
                let _ = stream.call_method0("flush");
            }
        }
    }
}

/// An [`EventListener`] that forwards pointer events to the Python
/// `SoEventCallback`s in the scene (the scripted counterpart of the Rust listeners).
///
/// It only delivers the event: because the extracted scene is a copy, a callback
/// does not yet change what is rendered, so it reports no scene change.
pub struct PythonEventCallbacks;

impl EventListener for PythonEventCallbacks {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        _scene: &mut Scene,
        directives: &Directives,
    ) -> bool {
        let _ = dispatch(directives, event, hit);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::compile;

    /// The host builds its viewport scene from a `pivy.coin` script; check
    /// that it extracts and compiles to the expected cube without a GPU.
    #[test]
    fn inventor_scene_script_builds_a_compilable_cube() {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scene = inventor_scene().expect("build the scene from Python");
        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1, "one mesh in the cube");
        assert_eq!(queue.packets[0].state.color, [0.9, 0.6, 0.2, 1.0]);
    }

    /// The script registers an `SoEventCallback`; dispatching a mouse-button event
    /// must reach it.
    #[test]
    fn dispatch_reaches_the_scripted_event_callback() {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scene = inventor_scene().expect("build the scene from Python");
        let (_, directives) = compile(&scene);
        let hit = Hit {
            id: ferrocad_inventor::PickId(0),
            point: [0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            part: 0,
            element: Some(1),
            topology: ferrocad_inventor::Topology::Triangles,
            distance: 1.0,
        };
        let event = Event::MouseButton {
            position: [10, 10],
            button: 1,
            down: true,
        };
        assert!(
            dispatch(&directives, &event, Some(&hit)),
            "the scripted SoEventCallback should have run"
        );
    }

    /// The host registers the modern-stack facade after `ferrocad` is imported, so a
    /// script can reach it as `ferrocad.stage`.
    #[test]
    fn the_stage_facade_attaches_to_ferrocad() {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Python::with_gil(|py| {
            // What the host does: put the facade directory on `sys.path`.
            let dir = find_python_dir().expect("locate python/");
            let sys = py.import("sys").unwrap();
            sys.getattr("path")
                .unwrap()
                .call_method1("insert", (0, dir.to_string_lossy().to_string()))
                .unwrap();

            let ferrocad = py.import("ferrocad").expect("import the ferrocad extension");
            ferrocad_stage_py::install(py).expect("install ferrocad.stage");
            let stage = ferrocad.getattr("stage").expect("ferrocad.stage");
            assert!(stage.getattr("ShapeItem").is_ok(), "stage.ShapeItem");
        });
    }
}
