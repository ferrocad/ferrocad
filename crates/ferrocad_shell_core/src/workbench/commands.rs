//! The workbench command bridge (A0/A1).
//!
//! Commands are registered by the engine (natively in Python on `FreeCADGui`; see
//! `docs/developer/workbenches/actions-and-tools.md`), and the shell consumes them
//! through the backend's `commands`/`run_command` seam. The typed vocabulary — the
//! bar's rows ([`CommandInfo`]) and a run's result ([`CommandOutcome`]) — lives in
//! [`ferrocad_shell_api::commands`] because it crosses that seam.
//!
//! Nothing here names a workbench. A command's id, label, state and meaning all
//! come from the registry.

use gpui::App;

pub use ferrocad_shell_api::commands::*;

/// The registered commands, with their live state.
pub fn list(cx: &App) -> Result<Vec<CommandInfo>, String> {
    crate::app::commands(cx)
}

/// Run a command by id.
pub fn run(cx: &mut App, id: &str) -> Result<CommandOutcome, String> {
    crate::app::run_command(cx, id)
}

/// Accept an open dialog: hand the shell's typed values to its callback.
///
/// The engine has already validated nothing; the shell did. The callback owns its
/// transaction and may return a new dialog spec (a wizard), which arrives in the
/// returned outcome.
pub fn accept(
    cx: &mut App,
    token: &str,
    values: &serde_json::Value,
) -> Result<CommandOutcome, String> {
    crate::app::dialog_accept(cx, token, values)
}

/// Cancel an open dialog and drop its token.
///
/// A cancellation is idempotent: a token the engine no longer holds is not an error.
pub fn cancel(cx: &mut App, token: &str) -> Result<CommandOutcome, String> {
    crate::app::dialog_cancel(cx, token)
}
