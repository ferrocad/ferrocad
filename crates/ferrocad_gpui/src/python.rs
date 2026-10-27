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

use std::path::{Path, PathBuf};
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

/// The document `--open` asked for, if any. Applied by [`open_startup_document`].
static STARTUP_DOCUMENT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Whether the shell is a GUI (`FreeCAD.GuiUp`). Set by [`configure`].
static GUI: OnceLock<bool> = OnceLock::new();

/// Configure the app side of the boundary, before boot.
///
/// `entry_module` must expose the shell's data functions (`bootstrap`,
/// `model_tree`, `properties`, `set_property`, `evaluate`) and the viewport
/// functions (`stage_meshes`, `interaction`, `commands`, `run_command`).
/// `python_paths` are the app's script and mod directories, added to `sys.path`.
/// `mods_paths` are the workbench directories scanned for `Init.py`/`InitGui.py`.
/// `gui` sets `FreeCAD.GuiUp`: true for the app window, false for `--check-init`
/// and headless tests.
pub fn configure(entry_module: &str, python_paths: &[PathBuf], mods_paths: &[PathBuf], gui: bool) {
    let _ = ENTRY_MODULE.set(entry_module.to_string());
    let _ = APP_PATHS.set(python_paths.to_vec());
    let _ = MODS_PATHS.set(mods_paths.to_vec());
    let _ = GUI.set(gui);
}

/// Configure the document `--open` should load after bootstrap.
///
/// This is separate from [`configure`] so the many tests that boot the shell
/// directly keep their current four-argument call and never open a file.
pub fn configure_startup_document(path: Option<&Path>) {
    let _ = STARTUP_DOCUMENT.set(path.map(Path::to_path_buf));
}

/// The configured startup document, if any.
pub fn startup_document() -> Option<PathBuf> {
    STARTUP_DOCUMENT.get().cloned().flatten()
}

/// Whether the shell is running as a GUI.
fn gui() -> bool {
    GUI.get().copied().unwrap_or(false)
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

/// The document `open_document` loaded.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenedDocument {
    pub document: String,
    #[serde(default)]
    pub objects: usize,
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
    /// A display label for a generated row (a command button). Empty for a normal
    /// property, whose label is its name.
    #[serde(default)]
    pub label: String,
    #[serde(rename = "type")]
    pub type_id: String,
    pub value: String,
    pub status: String,
    /// Whether the editor can change this property.
    #[serde(default)]
    pub editable: bool,
    /// The command this row runs instead of editing a value (`Reset to edition
    /// defaults` on a settings card).
    #[serde(default)]
    pub command: Option<String>,
    /// The editor kind (`text`, `bool`, `int`, `float`, `quantity`, `enum`,
    /// `color`). It picks the control: a `color` row gets a swatch and a palette,
    /// a bounded number gets the slider overlay.
    #[serde(default)]
    pub kind: String,
    /// The editor's inclusive bounds, when the schema declares them.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// The quantity unit (`mm`), shown in the slider's readout.
    #[serde(default)]
    pub unit: Option<String>,
    /// An enumeration's options. Empty for every other kind; an enum row with
    /// choices renders as a select box instead of a text field.
    #[serde(default)]
    pub choices: Vec<String>,
}

/// The outcome of a property edit.
#[derive(Debug, Clone, Deserialize)]
pub struct SetResult {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    /// A presentation change the edit asks for (a settings row that changes the grid
    /// spacing or the gizmo palette). Without this the edit would be stored but the
    /// viewport would keep drawing the old state until something else repainted it.
    #[serde(default)]
    pub view: Option<crate::view::ViewRequest>,
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

/// The grid's active provider and spacing, for the header popover (T4).
///
/// The shape is closed: with no provider registered, `backend`/`label`/`spacing`
/// are `None` and `providers` is empty, so the popover never special-cases it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GridSummary {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The active provider's spacing as its display string (`"10 mm"`).
    #[serde(default)]
    pub spacing: Option<String>,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// One registered grid backend, as the popover lists it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GridProvider {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub active: bool,
    /// The registry id this backend owns (`None` when it offers no command).
    ///
    /// The Dev Tools command list uses it to disable the losing backend's entry,
    /// so two same-purpose commands do not look equally runnable. This is a UI
    /// filter only: the registry is advisory and cannot stop a mod from drawing.
    #[serde(default)]
    pub command: Option<String>,
}

/// The axis cross's active provider and geometry, for the header popover.
///
/// The shape mirrors [`GridSummary`]: with no axis provider registered,
/// `backend`/`label`/`length`/`width` are `None` and `providers` is empty, so the
/// popover never special-cases the empty state. The providers reuse
/// [`GridProvider`] (backend, label, active) — the list has the same meaning.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AxisSummary {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The stem length as its display string (`"80 px"`).
    #[serde(default)]
    pub length: Option<String>,
    /// The stem width as its display string (`"1.5 px"`).
    #[serde(default)]
    pub width: Option<String>,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// One provider feature and its backends, for the Dev Tools providers panel.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProviderFeature {
    #[serde(default)]
    pub feature: String,
    #[serde(default)]
    pub providers: Vec<GridProvider>,
}

/// The result of persisting a GPU adapter preference.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GpuPreferenceOutcome {
    /// Whether the value was accepted and stored.
    pub ok: bool,
    /// The stored value (empty when rejected).
    #[serde(default)]
    pub value: String,
    /// Why the value was rejected; empty on success.
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

        // The app is a GUI: tell the facade before the app script imports, so
        // import-time workbench code sees the same `GuiUp` upstream has.
        if gui() {
            py.import("FreeCAD")
                .map_err(|e| e.to_string())?
                .setattr("GuiUp", 1)
                .map_err(|e| e.to_string())?;
        }

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

/// Open a saved document and make it active (the `--open` path).
pub fn open_document(path: &Path) -> Result<OpenedDocument, String> {
    let path = path.to_string_lossy();
    call_json("open_document", Some(path.as_ref()))
}

/// Close a document by name (used to drop the bootstrap sample).
pub fn close_document(name: &str) -> Result<(), String> {
    let _: serde_json::Value = call_json("close_document", Some(name))?;
    Ok(())
}

/// Apply the configured startup document, if one was set.
///
/// `boot` describes the sample document `bootstrap` created. When `--open` named
/// a file, this opens it, makes it active and closes the sample (unless the file
/// itself has that name). The shell then reads `model_tree` and `stage_meshes`
/// from the requested document, so the window and the headless render agree.
pub fn open_startup_document(boot: &mut Bootstrap) -> Result<Option<OpenedDocument>, String> {
    let Some(path) = startup_document() else {
        return Ok(None);
    };
    let opened = open_document(&path)?;
    if opened.document != boot.document {
        // A failure to drop the sample does not hide the document the user asked
        // for; the tree simply carries one extra entry.
        let _ = close_document(&boot.document);
    }
    boot.document = opened.document.clone();
    Ok(Some(opened))
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
/// Empty when no module can display the document, which the shell treats as "keep the
/// sample scene".
pub fn stage_meshes() -> Result<Vec<crate::display::StageMesh>, String> {
    call_json("stage_meshes", None)
}

/// Execute a Python file in the embedded interpreter (the `--script` path).
///
/// The file runs with `__name__ == "__main__"`, after the edition's `bootstrap`
/// has created the sample document. It may create objects, replace the active
/// document or save files; the subsequent `stage_meshes` call sees its result.
pub fn run_script(path: &std::path::Path) -> Result<(), String> {
    let path = path.to_string_lossy().into_owned();
    Python::with_gil(|py| {
        let runpy = py.import("runpy").map_err(|e| e.to_string())?;
        let kwargs = PyDict::new(py);
        kwargs
            .set_item("run_name", "__main__")
            .map_err(|e| e.to_string())?;
        runpy
            .call_method("run_path", (path,), Some(&kwargs))
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// The declarative handles the mods declare for a selection.
///
/// The values are kept raw so one malformed spec cannot make its siblings vanish;
/// the caller converts each with [`crate::manipulators::handle_from_value`] and
/// reports the ones that fail. The payload carries the selected object's name and
/// its first sub-element (the shell's selection vocabulary).
pub fn manipulator_specs(
    object: &str,
    element: Option<&str>,
) -> Result<Vec<serde_json::Value>, String> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        specs: Vec<serde_json::Value>,
    }

    let payload = serde_json::json!({ "object": object, "element": element }).to_string();
    let envelope: Envelope = call_json("manipulator_specs", Some(&payload))?;
    Ok(envelope.specs)
}

/// Apply a manipulator commit to the document through the app script's `interaction`
/// hook.
///
/// The shell does not know what the manipulator means: the script routes the commit to
/// the workbench that registered an interaction provider (for Part, a
/// `Part::Fillet`/`Part::Chamfer`). `values` are the manipulator's numbers in its
/// documented order.
pub fn apply_interaction(
    commit: &ferrocad_stage::InteractionCommit,
    object: &str,
) -> Result<ApplyResult, String> {
    let values = serde_json::to_string(&commit.values).map_err(|e| e.to_string())?;
    let element = commit.element.clone().unwrap_or_default();
    let raw = Python::with_gil(|py| -> Result<String, String> {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("interaction")
            .map_err(|e| e.to_string())?
            .call1((
                object,
                commit.manipulator.as_str(),
                commit.action.as_str(),
                element,
                values,
            ))
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Replace the facade's selection snapshot (`FreeCADGui.Selection`).
///
/// The shell owns the selection; the facade mirrors it so a command's `IsActive`
/// and `Activated` see what the UI shows. Called after every selection change.
pub fn set_selection(items: &[crate::selection::SelectionItem]) -> Result<(), String> {
    let payload = serde_json::to_string(items).map_err(|e| e.to_string())?;
    let _: serde_json::Value = call_json("set_selection", Some(&payload))?;
    Ok(())
}

/// One write a script queued on `FreeCADGui.Selection`.
///
/// `op` is `add`, `remove`, `clear` or `preselect`. `object` is a document object
/// name (empty for `clear`), `elements` the kernel sub-element names, and `points`
/// the world points the pick reported (as far as the script knew them). The shape is
/// deliberately the facade's own record so the two sides cannot drift.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SelectionRequest {
    pub op: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub elements: Vec<String>,
    #[serde(default)]
    pub points: Vec<(f64, f64, f64)>,
}

/// Drain the selection writes a script queued (`FreeCADGui.Selection.addSelection` …).
///
/// The facade cannot command the viewport synchronously, so `addSelection`,
/// `removeSelection`, `clearSelection` and `setPreselection` update its snapshot and
/// queue the operation. The shell applies the queue to its own selection, which is
/// what makes a scripted selection visible to the viewport (and arms the workbench's
/// manipulators for it). Draining empties the queue.
pub fn take_selection_requests() -> Result<Vec<SelectionRequest>, String> {
    call_json("take_selection_requests", None)
}

/// Mirror the shell's presentation state to `FerroCADGui.ViewState`.
///
/// A checkable command (`App_ToggleGrid`) reads this in `IsChecked`, so the push
/// happens before the command list is refreshed.
pub fn set_view_state(state: &crate::view::ViewState) -> Result<(), String> {
    let payload = serde_json::json!({
        "grid": state.grid,
        "axis_cross": state.axis.enabled,
        "projection": state.projection.as_str(),
        "grid_minor": state.grid_minor,
        "grid_major": state.grid_major,
        "grid_radius": state.grid_radius,
    })
    .to_string();
    let _: serde_json::Value = call_json("set_view_state", Some(&payload))?;
    Ok(())
}

/// The registered workbench commands, with their live state.
pub fn commands() -> Result<Vec<crate::commands::CommandInfo>, String> {
    call_json("commands", None)
}

/// The grid's active provider and spacing (T4).
///
/// Read once when the grid popover opens, not per frame: it crosses into the
/// interpreter and the provider only changes on a settings edit.
pub fn grid_summary() -> Result<GridSummary, String> {
    call_json("grid_summary", None)
}

/// The axis cross's active provider and geometry.
///
/// Read once when the axis popover opens, not per frame: it crosses into the
/// interpreter and the provider only changes on a settings edit.
pub fn axis_summary() -> Result<AxisSummary, String> {
    call_json("axis_summary", None)
}

/// Every registered provider feature for the Dev Tools providers panel.
pub fn provider_features() -> Result<Vec<ProviderFeature>, String> {
    call_json("provider_features", None)
}

/// The persisted viewport GPU adapter setting.
///
/// The Python helper returns the stored string itself (not JSON), so this
/// extracts it directly instead of going through [`call_json`]. The shell reads
/// it when it creates the viewport and when the Dev Tools panel opens.
pub fn gpu_adapter() -> Result<String, String> {
    Python::with_gil(|py| {
        let module = py.import(entry_module()).map_err(|e| e.to_string())?;
        module
            .getattr("gpu_adapter")
            .map_err(|e| e.to_string())?
            .call0()
            .map_err(|e| e.to_string())?
            .extract::<String>()
            .map_err(|e| e.to_string())
    })
}

/// Persist the viewport GPU adapter setting and report the outcome.
///
/// The payload is `{"value": "..."}`; a rejected value comes back as an
/// outcome with `ok: false`, never as a bridge error, so the shell can show it.
pub fn set_gpu_adapter(value: &str) -> Result<GpuPreferenceOutcome, String> {
    let payload = serde_json::json!({ "value": value }).to_string();
    call_json("set_gpu_adapter", Some(&payload))
}

/// Run a workbench command by id.
pub fn run_command(name: &str) -> Result<crate::commands::CommandOutcome, String> {
    call_json("run_command", Some(name))
}

/// Switch the active provider for `feature` to `backend`.
///
/// The payload is `{"feature": ..., "backend": ...}`; the answer is the same
/// `CommandOutcome` shape as `run_command`, carrying the merged `view` request and
/// a status line.
pub fn set_provider(
    feature: &str,
    backend: &str,
) -> Result<crate::commands::CommandOutcome, String> {
    let payload = serde_json::json!({"feature": feature, "backend": backend}).to_string();
    call_json("set_provider", Some(&payload))
}

/// The WGSL skin the active provider of `feature` supplies, if any.
///
/// The shader lives in the mod, not in the app: the registry is the only channel.
/// It answers for the **active** backend, so a provider switch can never leave a
/// stale skin behind in the shell.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSkin {
    /// The source text, or `None` when the active backend supplies no skin.
    pub source: Option<String>,
}

/// The WGSL source the active provider of `feature` supplies, or `None`.
pub fn provider_skin(feature: &str) -> Result<Option<String>, String> {
    let payload = serde_json::json!({ "feature": feature }).to_string();
    let skin: ProviderSkin = call_json("provider_skin", Some(&payload))?;
    Ok(skin.source)
}

/// Accept an open dialog: one JSON payload (the token and the typed values).
pub fn dialog_accept(
    token: &str,
    values: &serde_json::Value,
) -> Result<crate::commands::CommandOutcome, String> {
    let payload = serde_json::json!({ "token": token, "values": values }).to_string();
    call_json("dialog_accept", Some(&payload))
}

/// Cancel an open dialog; the facade drops its token.
pub fn dialog_cancel(token: &str) -> Result<crate::commands::CommandOutcome, String> {
    call_json("dialog_cancel", Some(token))
}

pub fn properties(object: &str) -> Result<Vec<PropRow>, String> {
    call_json("properties", Some(object))
}

/// The appearance the edition's preset (and the user's stored values) resolve to.
///
/// Returns a command-outcome shape so the boot path can apply `view` through the same
/// [`ViewState::apply`](crate::view::ViewState::apply) it uses for a command. Called
/// once, before the first window: stored appearance is a *value*, not a request, so
/// nothing would draw it otherwise.
pub fn appearance_view() -> Result<crate::commands::CommandOutcome, String> {
    call_json("appearance_view", None)
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
/// [`Directives`] beside the draw queue (see `docs/viewport/directives.md`); the callables live
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
