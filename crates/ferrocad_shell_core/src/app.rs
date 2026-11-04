//! The shell's side of the [`AppBackend`] seam.
//!
//! The trait and its data live in [`ferrocad_shell_api`]; this module stores the
//! installed backend on the GPUI [`App`] and gives the shell one helper per method.
//! A helper fixes the missing-backend policy once, so a misconfigured host still
//! renders a window instead of aborting.
//!
//! # Threading
//!
//! No `Send`: GPUI is single-threaded and the backend is touched from the UI thread
//! only. It is stored as a GPUI [`Global`], not in a process `static`.

use std::cell::RefCell;

use gpui::{App, Global};

use ferrocad_inventor::Scene;
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;

use crate::model::display::StageMesh;
use crate::model::selection::SelectionItem;
use crate::model::view::ViewState;
use crate::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    VisibilityOutcome, WorkbenchReport,
};
use crate::workbench::commands::{CommandInfo, CommandOutcome};

pub use ferrocad_shell_api::app::{AppBackend, NoopListener};

/// The installed backend. One per `App`; the host installs it before `Shell::boot`.
struct AppBackendSlot(RefCell<Box<dyn AppBackend>>);

impl Global for AppBackendSlot {}

/// Install `backend` as this app's engine.
///
/// Call before [`Shell::boot`](crate::shell::Shell::boot); boot reads the backend.
pub fn install_backend(cx: &mut App, backend: Box<dyn AppBackend>) {
    cx.set_global(AppBackendSlot(RefCell::new(backend)));
}

/// Run `f` with the installed backend, or `None` when none is installed (or a read
/// re-entered an active mutable borrow).
pub fn with_backend<T>(cx: &App, f: impl FnOnce(&dyn AppBackend) -> T) -> Option<T> {
    let slot = cx.try_global::<AppBackendSlot>()?;
    let cell = slot.0.try_borrow().ok()?;
    Some(f(&**cell))
}

/// Run `f` with the installed backend, or `None` when none is installed (or a write
/// re-entered an active borrow).
pub fn with_backend_mut<T>(cx: &mut App, f: impl FnOnce(&mut dyn AppBackend) -> T) -> Option<T> {
    if cx.try_global::<AppBackendSlot>().is_none() {
        return None;
    }
    let slot = cx.global_mut::<AppBackendSlot>();
    let mut cell = slot.0.try_borrow_mut().ok()?;
    Some(f(&mut **cell))
}

/// The error a helper reports when the host forgot [`install_backend`].
fn no_backend() -> String {
    "no app backend installed (the host must call install_backend before boot)".to_string()
}

// --- the shell-facing helpers -------------------------------------------------
//
// One per backend method, with the same name and result shape, so the call sites
// keep reading as "ask the engine".

pub fn bootstrap(cx: &mut App) -> Result<Bootstrap, String> {
    with_backend_mut(cx, |backend| backend.bootstrap()).unwrap_or_else(|| Err(no_backend()))
}

pub fn open_startup_document(
    cx: &mut App,
    boot: &mut Bootstrap,
) -> Result<Option<OpenedDocument>, String> {
    with_backend_mut(cx, |backend| backend.open_startup_document(boot))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn hello(cx: &App) -> Result<String, String> {
    with_backend(cx, |backend| backend.hello()).unwrap_or_else(|| Err(no_backend()))
}

/// Replace the active document with bytes the host decoded (a browser file picker).
pub fn open_document(cx: &mut App, name: &str, bytes: &[u8]) -> Result<OpenedDocument, String> {
    with_backend_mut(cx, |backend| backend.open_document(name, bytes))
        .unwrap_or_else(|| Err(no_backend()))
}

/// Serialize the active document for the host to save (a browser download).
pub fn save_document(cx: &App) -> Result<Vec<u8>, String> {
    with_backend(cx, |backend| backend.save_document()).unwrap_or_else(|| Err(no_backend()))
}

/// Open a document from a path the host picked (the native file dialog).
pub fn open_path(cx: &mut App, path: &std::path::Path) -> Result<OpenedDocument, String> {
    with_backend_mut(cx, |backend| backend.open_path(path)).unwrap_or_else(|| Err(no_backend()))
}

/// Save the active document to a path the host picked.
pub fn save_path(cx: &App, path: &std::path::Path) -> Result<(), String> {
    with_backend(cx, |backend| backend.save_path(path)).unwrap_or_else(|| Err(no_backend()))
}

pub fn load_workbenches(cx: &App) -> Result<WorkbenchReport, String> {
    with_backend(cx, |backend| backend.load_workbenches()).unwrap_or_else(|| Err(no_backend()))
}

pub fn model_tree(cx: &App) -> Result<Vec<DocumentNode>, String> {
    with_backend(cx, |backend| backend.model_tree()).unwrap_or_else(|| Ok(Vec::new()))
}

pub fn properties(cx: &App, object: &str) -> Result<Vec<PropRow>, String> {
    with_backend(cx, |backend| backend.properties(object)).unwrap_or_else(|| Ok(Vec::new()))
}

pub fn stage_meshes(cx: &App) -> Result<Vec<StageMesh>, String> {
    with_backend(cx, |backend| backend.stage_meshes()).unwrap_or_else(|| Ok(Vec::new()))
}

pub fn commands(cx: &App) -> Result<Vec<CommandInfo>, String> {
    with_backend(cx, |backend| backend.commands()).unwrap_or_else(|| Ok(Vec::new()))
}

pub fn appearance_view(cx: &App) -> Result<CommandOutcome, String> {
    with_backend(cx, |backend| backend.appearance_view())
        .unwrap_or_else(|| Ok(CommandOutcome::default()))
}

pub fn set_view_state(cx: &App, state: &ViewState) -> Result<(), String> {
    with_backend(cx, |backend| backend.set_view_state(state)).unwrap_or(Ok(()))
}

pub fn set_selection(cx: &App, items: &[SelectionItem]) -> Result<(), String> {
    with_backend(cx, |backend| backend.set_selection(items)).unwrap_or(Ok(()))
}

pub fn set_visibility(
    cx: &mut App,
    object: &str,
    visible: Option<bool>,
) -> Result<VisibilityOutcome, String> {
    with_backend_mut(cx, |backend| backend.set_visibility(object, visible))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn take_selection_requests(cx: &mut App) -> Result<Vec<SelectionRequest>, String> {
    with_backend_mut(cx, |backend| backend.take_selection_requests())
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub fn evaluate(cx: &App, code: &str) -> Result<EvalResult, String> {
    with_backend(cx, |backend| backend.evaluate(code)).unwrap_or_else(|| Err(no_backend()))
}

pub fn rotation_quaternion(cx: &App, object: &str) -> Option<[f64; 4]> {
    with_backend(cx, |backend| backend.rotation_quaternion(object)).flatten()
}

pub fn tool(cx: &App, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
    with_backend(cx, |backend| backend.tool(name)).unwrap_or_else(|| Err(no_backend()))
}

pub fn tool_command(cx: &App, name: &str, verb: &str) -> bool {
    with_backend(cx, |backend| backend.tool_command(name, verb)).unwrap_or(false)
}

pub fn set_property(
    cx: &mut App,
    object: &str,
    property: &str,
    value: &str,
) -> Result<SetResult, String> {
    with_backend_mut(cx, |backend| backend.set_property(object, property, value))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn run_command(cx: &mut App, id: &str) -> Result<CommandOutcome, String> {
    with_backend_mut(cx, |backend| backend.run_command(id)).unwrap_or_else(|| Err(no_backend()))
}

pub fn set_provider(cx: &mut App, feature: &str, backend: &str) -> Result<CommandOutcome, String> {
    with_backend_mut(cx, |slot| slot.set_provider(feature, backend))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn refresh_providers(cx: &mut App, feature: &str) -> Result<CommandOutcome, String> {
    with_backend_mut(cx, |backend| backend.refresh_providers(feature))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn dialog_accept(
    cx: &mut App,
    token: &str,
    values: &serde_json::Value,
) -> Result<CommandOutcome, String> {
    with_backend_mut(cx, |backend| backend.dialog_accept(token, values))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn dialog_cancel(cx: &mut App, token: &str) -> Result<CommandOutcome, String> {
    with_backend_mut(cx, |backend| backend.dialog_cancel(token))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn add_datum_plane(
    cx: &mut App,
    origin: Vector3,
    u: Vector3,
    v: Vector3,
) -> Result<String, String> {
    with_backend_mut(cx, |backend| backend.add_datum_plane(origin, u, v))
        .unwrap_or_else(|| Err(no_backend()))
}

pub fn manipulator_specs(
    cx: &App,
    object: &str,
    element: Option<&str>,
) -> Result<Vec<serde_json::Value>, String> {
    with_backend(cx, |backend| backend.manipulator_specs(object, element))
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub fn apply_interaction(
    cx: &mut App,
    commit: &InteractionCommit,
    object: &str,
) -> Result<ApplyResult, String> {
    with_backend_mut(cx, |backend| backend.apply_interaction(commit, object)).unwrap_or_else(|| {
        Ok(ApplyResult {
            ok: false,
            error: no_backend(),
        })
    })
}

pub fn grid_summary(cx: &App) -> Result<GridSummary, String> {
    with_backend(cx, |backend| backend.grid_summary()).unwrap_or_else(|| Ok(GridSummary::default()))
}

pub fn axis_summary(cx: &App) -> Result<AxisSummary, String> {
    with_backend(cx, |backend| backend.axis_summary()).unwrap_or_else(|| Ok(AxisSummary::default()))
}

pub fn provider_features(cx: &App) -> Result<Vec<ProviderFeature>, String> {
    with_backend(cx, |backend| backend.provider_features()).unwrap_or_else(|| Ok(Vec::new()))
}

pub fn provider_skin(cx: &App, feature: &str) -> Result<Option<String>, String> {
    with_backend(cx, |backend| backend.provider_skin(feature)).unwrap_or_else(|| Ok(None))
}

pub fn gpu_adapter(cx: &App) -> Result<String, String> {
    with_backend(cx, |backend| backend.gpu_adapter()).unwrap_or_else(|| Ok("auto".to_string()))
}

pub fn set_gpu_adapter(cx: &mut App, value: &str) -> Result<GpuPreferenceOutcome, String> {
    with_backend_mut(cx, |backend| backend.set_gpu_adapter(value)).unwrap_or_else(|| {
        Ok(GpuPreferenceOutcome {
            ok: true,
            value: value.to_string(),
            error: String::new(),
        })
    })
}

pub fn inventor_scene(cx: &App) -> Result<Scene, String> {
    with_backend(cx, |backend| backend.inventor_scene()).unwrap_or_else(|| Ok(Scene::default()))
}

pub fn event_listener(cx: &App) -> Box<dyn ferrocad_inventor::EventListener> {
    with_backend(cx, |backend| backend.event_listener()).unwrap_or_else(|| Box::new(NoopListener))
}
