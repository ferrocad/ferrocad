//! The `AppBackend` seam: the contract between the shell and its engine.
//!
//! The shell is platform-neutral but not engine-neutral: it paints a model tree, a
//! property inspector and a 3D stage, and something has to produce those. Natively
//! that something is the embedded CPython interpreter (the `FreeCAD` facade); in
//! the browser it is a Rust backend over `ferrocad_core`.
//!
//! The contract lives in this crate because it is *shared*: the shell implements
//! the UI, a host implements the trait, and neither half owns the vocabulary. The
//! methods return **UI-ready view models** (see [`crate::types`]), never raw document
//! objects. They are distinct from `ferrocad_core::Document`, which has no notion of
//! a panel row or a toolbar command.
//!
//! # Threading
//!
//! No `Send`: GPUI is single-threaded and the backend is touched from the UI thread
//! only. The shell stores it as a GPUI `Global` (see `ferrocad_shell_core::app`).
//!
//! # Defaults
//!
//! Every method has a default: a read-only backend (the first browser milestone)
//! implements only what it can answer. A host that installs no backend at all still
//! renders a window; the shell's wrappers report the missing backend.

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_stage::{InteractionCommit, Tool};
use ferrocad_types::Vector3;

use crate::commands::{CommandInfo, CommandOutcome};
use crate::display::StageMesh;
use crate::selection::SelectionItem;
use crate::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    VisibilityOutcome, WorkbenchReport,
};
use crate::view::ViewState;

/// The engine behind the shell.
///
/// Reads take `&self`; anything that can change the document takes `&mut self`.
pub trait AppBackend {
    // --- boot -----------------------------------------------------------------

    /// Boot the engine and describe the sample document it created.
    fn bootstrap(&mut self) -> Result<Bootstrap, String> {
        Ok(Bootstrap {
            version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "unavailable".to_string(),
            document: "Untitled".to_string(),
        })
    }

    /// Apply the document the host asked to open at boot, if any.
    fn open_startup_document(
        &mut self,
        _boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        Ok(None)
    }

    /// Replace the active document with one the host decoded from bytes.
    ///
    /// The browser host owns the file picker, so it hands over the file's name (for
    /// the title and the next save) and its bytes (`ferrocad_core::fcstd`); a backend
    /// that owns paths can leave this at the refusal.
    fn open_document(&mut self, _name: &str, _bytes: &[u8]) -> Result<OpenedDocument, String> {
        Err("opening a document is not supported by this backend".to_string())
    }

    /// Serialize the active document for the host to save.
    ///
    /// The host writes the bytes (a browser download); the format is the backend's.
    fn save_document(&self) -> Result<Vec<u8>, String> {
        Err("saving a document is not supported by this backend".to_string())
    }

    /// The console's greeting line.
    fn hello(&self) -> Result<String, String> {
        Ok("FerroCAD shell · no engine backend".to_string())
    }

    /// Load workbench (`Init.py`/`InitGui.py`) scripts. Best-effort.
    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        Ok(WorkbenchReport::default())
    }

    // --- view models ----------------------------------------------------------

    fn model_tree(&self) -> Result<Vec<DocumentNode>, String> {
        Ok(Vec::new())
    }

    fn properties(&self, _object: &str) -> Result<Vec<PropRow>, String> {
        Ok(Vec::new())
    }

    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String> {
        Ok(Vec::new())
    }

    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        Ok(Vec::new())
    }

    /// The appearance the edition's preset resolves to, as a command outcome whose
    /// `view` the boot path applies.
    fn appearance_view(&self) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    // --- view state and selection --------------------------------------------

    /// Mirror the shell's presentation state to the engine's view facade.
    fn set_view_state(&self, _state: &ViewState) -> Result<(), String> {
        Ok(())
    }

    /// Mirror the shell's selection to the engine's selection facade.
    fn set_selection(&self, _items: &[SelectionItem]) -> Result<(), String> {
        Ok(())
    }

    /// Show or hide an object. `visible` is `None` to toggle the current state.
    ///
    /// The backend owns what visibility *is* — the native one writes a document
    /// `Visibility` property, the browser one a headless view state — and answers with
    /// the state that now applies. A backend that cannot change it leaves the default.
    fn set_visibility(
        &mut self,
        _object: &str,
        _visible: Option<bool>,
    ) -> Result<VisibilityOutcome, String> {
        Err("visibility is not supported by this backend".to_string())
    }

    /// Drain the selection writes a script queued.
    fn take_selection_requests(&mut self) -> Result<Vec<SelectionRequest>, String> {
        Ok(Vec::new())
    }

    // --- scripting ------------------------------------------------------------

    fn evaluate(&self, _code: &str) -> Result<EvalResult, String> {
        Err("scripting is not available in this build".to_string())
    }

    fn rotation_quaternion(&self, _object: &str) -> Option<[f64; 4]> {
        None
    }

    /// Resolve a scripted interactive tool (`python:<name>`) to its native handle.
    fn tool(&self, _name: &str) -> Result<Box<dyn Tool>, String> {
        Err("scripted tools are not available in this build".to_string())
    }

    /// Send a session verb (`cancel`/`finish`) to a scripted tool.
    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    // --- editing --------------------------------------------------------------

    fn set_property(
        &mut self,
        _object: &str,
        _property: &str,
        _value: &str,
    ) -> Result<SetResult, String> {
        Ok(SetResult {
            ok: false,
            error: "not supported".to_string(),
            view: None,
        })
    }

    fn run_command(&mut self, _id: &str) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    fn set_provider(&mut self, _feature: &str, _backend: &str) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    fn refresh_providers(&mut self, _feature: &str) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    fn dialog_accept(
        &mut self,
        _token: &str,
        _values: &serde_json::Value,
    ) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    fn dialog_cancel(&mut self, _token: &str) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::default())
    }

    fn add_datum_plane(
        &mut self,
        _origin: Vector3,
        _u: Vector3,
        _v: Vector3,
    ) -> Result<String, String> {
        Err("not supported".to_string())
    }

    fn manipulator_specs(
        &self,
        _object: &str,
        _element: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, String> {
        Ok(Vec::new())
    }

    fn apply_interaction(
        &mut self,
        _commit: &InteractionCommit,
        _object: &str,
    ) -> Result<ApplyResult, String> {
        Ok(ApplyResult {
            ok: false,
            error: "not supported".to_string(),
        })
    }

    // --- providers and GPU ----------------------------------------------------

    fn grid_summary(&self) -> Result<GridSummary, String> {
        Ok(GridSummary::default())
    }

    fn axis_summary(&self) -> Result<AxisSummary, String> {
        Ok(AxisSummary::default())
    }

    fn provider_features(&self) -> Result<Vec<ProviderFeature>, String> {
        Ok(Vec::new())
    }

    fn provider_skin(&self, _feature: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn gpu_adapter(&self) -> Result<String, String> {
        Ok("auto".to_string())
    }

    fn set_gpu_adapter(&mut self, value: &str) -> Result<GpuPreferenceOutcome, String> {
        Ok(GpuPreferenceOutcome {
            ok: true,
            value: value.to_string(),
            error: String::new(),
        })
    }

    // --- scene and events -----------------------------------------------------

    /// The `pivy.coin`-style scene the viewport falls back to when the stage is empty.
    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    /// The listener that forwards viewport events to the engine's callbacks.
    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(NoopListener)
    }
}

/// Forwards nothing. The listener a host gets when it supplies none.
pub struct NoopListener;

impl EventListener for NoopListener {
    fn on_event(
        &mut self,
        _event: &ferrocad_inventor::Event,
        _hit: Option<&ferrocad_inventor::Hit>,
        _scene: &mut Scene,
        _directives: &ferrocad_inventor::Directives,
    ) -> bool {
        false
    }
}
