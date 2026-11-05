//! FerroCAD's platform-neutral `bite-gpui` shell.
//!
//! This crate is the UI half of the application: the window, the chrome, the panels,
//! the model inspector, the property editor, the viewport element, the HUD and the
//! workbench command vocabulary. It knows nothing about *how* the engine underneath
//! is reached — that is the [`AppBackend`](app::AppBackend) seam, which a host
//! installs before boot:
//!
//! - [`ferrocad_shell_gpui`](https://crates.io/crates/ferrocad_shell_gpui) is the
//!   native assembly: the shell plus the embedded-CPython backend and the
//!   `run`/`run_with`/`HostConfig` entry points.
//! - `ferrocad_web_gpui` is the browser host: the same shell plus a Rust backend
//!   over `ferrocad_core`, with no CPython anywhere in its dependency graph.
//!
//! The engine is no longer named here. A workbench contributes through the backend,
//! exactly as it always did through the `FreeCAD` facade.
//!
//! # Module map
//!
//! ```text
//! app/        AppBackend + install/look up        (the seam)
//! types/      Bootstrap, PropRow, DocumentNode    (the view models)
//! model/      display, selection, view            (shell-facing data)
//! runtime/    interop                             (zero-copy presentation)
//! ui/         chrome, dialog, forms,              (gpui surfaces)
//!             shortcuts, dev_tools
//! workbench/  commands, contributions,            (what a workbench registers)
//!             manipulators
//! shell/      Shell + bridge/, hud/, panels/      (the application shell)
//! ```
//!
//! Every submodule is public. The older flat paths (`ferrocad_shell_core::chrome`,
//! `::commands`, …) are kept as re-exports of the same modules.

pub mod app;
pub mod files;
pub mod fonts;
pub mod model;
pub mod runtime;
pub mod shell;
pub mod stage_view;
pub mod types;
pub mod ui;
pub mod workbench;

// The flat module paths predate the folder hierarchy and remain part of the stable
// library surface: `ferrocad_shell_core::chrome` and `ferrocad_shell_core::ui::chrome`
// name the same module.
pub use model::{display, selection, view};
#[cfg(all(feature = "gpu-interop", not(target_os = "macos")))]
pub use runtime::interop;
pub use ui::{chrome, dev_tools, dialog, forms, shortcuts};
pub use workbench::{commands, contributions, manipulators};

pub use app::{AppBackend, ScriptHost, install_backend, with_backend, with_backend_mut};
pub use commands::{CommandInfo, CommandOutcome, Surface};
pub use contributions::{Contributions, GizmoFactory, ManipulatorFactory, translate_gizmo};
pub use dev_tools::DevTools;
pub use dialog::{DialogField, DialogSpec};
pub use display::{DisplaySync, StageMesh};
pub use files::{FilePicker, FileTask, NativeFilePicker, OpenOutcome, SaveOutcome};
pub use selection::SelectionItem;
pub use shell::{Shell, ShellModel};
pub use shortcuts::Accelerator;
pub use types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridProvider, GridSummary, ObjectNode, OpenedDocument, PropRow, ProviderFeature, ProviderSkin,
    SelectionRequest, SetResult, VisibilityOutcome, WorkbenchError, WorkbenchReport,
};
pub use view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};

/// A zero-copy viewport presenter for this build and platform, if one is available.
///
/// Returns `None` when the `gpu-interop` feature is off, or on a platform whose
/// window renderer cannot share its device (macOS). The viewport then keeps the
/// readback `RenderImage` path — except on the web, where the readback path cannot
/// complete and the presenter is the only way to show a scene. Even when a
/// presenter is returned it may report itself unsupported at paint time (a headless
/// window or a non-wgpu renderer), in which case the viewport falls back as well.
pub fn zero_copy_presenter() -> Option<Box<dyn ferrocad_viewport::Presenter>> {
    #[cfg(all(feature = "gpu-interop", not(target_os = "macos")))]
    {
        Some(interop::presenter())
    }
    #[cfg(not(all(feature = "gpu-interop", not(target_os = "macos"))))]
    {
        None
    }
}

/// Serializes tests that drive the shared embedded CPython instance.
///
/// The unit tests run in one process and share the interpreter the native host
/// installs; every test that boots or evaluates holds this guard.
#[cfg(test)]
pub(crate) static PYTHON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only alias: the shell tests reach the real Python bridge directly.
///
/// The production paths go through [`app`]; the tests assert against the engine's
/// own functions (the same ones the backend calls), which keeps them honest about
/// what the seam forwards.
#[cfg(test)]
pub(crate) use ferrocad_shell_gpui::python;
