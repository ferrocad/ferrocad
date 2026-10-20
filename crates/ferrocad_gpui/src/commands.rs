//! The workbench command bridge (A0/A1).
//!
//! Commands are registered in Python on `FreeCADGui` (see
//! `docs/actions-and-tools.md`), and the shell consumes them through the app
//! script's `commands` / `run_command` functions. This module is the typed Rust
//! side: the bar's rows ([`CommandInfo`]) and a run's result ([`CommandOutcome`]).
//!
//! Nothing here names a workbench. A command's id, label, state and meaning all
//! come from the registry.

use serde::Deserialize;

/// One command, as the actions bar needs it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct CommandInfo {
    /// The registry id (`"Toy_MakeSlab"`).
    pub id: String,
    /// The button label (`GetResources()["MenuText"]`), defaulting to the id.
    #[serde(default)]
    pub menu: String,
    /// Hover text.
    #[serde(default)]
    pub tooltip: String,
    /// The shortcut (`"Shift+S"`); parsed in A4.
    #[serde(default)]
    pub accel: String,
    /// The group, for ordering.
    #[serde(default)]
    pub group: String,
    /// `CmdType` (`"ForEdit"`, `"ForView"`, …); metadata only.
    #[serde(default)]
    pub cmd_type: String,
    /// `IsActive()`.
    #[serde(default)]
    pub enabled: bool,
    /// `IsChecked()` — for checkable commands.
    #[serde(default)]
    pub checked: bool,
}

impl CommandInfo {
    /// The text to show on the button: `MenuText`, or the id when it is empty.
    pub fn label(&self) -> &str {
        if self.menu.is_empty() {
            &self.id
        } else {
            &self.menu
        }
    }
}

/// What running a command produced.
///
/// `ok` is false for an unknown id or a raising `Activated`; the command may add
/// a human `status`, and A3 adds the dialog spec and its single-use token.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct CommandOutcome {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub status: String,
    /// The `repr` of a non-dict return value, when the command returned one.
    #[serde(default)]
    pub value: Option<String>,
    /// A modal the command wants the shell to show.
    #[serde(default)]
    pub dialog: Option<crate::dialog::DialogSpec>,
    /// The single-use token for the dialog's `on_accept` callback.
    #[serde(default)]
    pub token: Option<String>,
}

/// The registered commands, with their live state.
pub fn list() -> Result<Vec<CommandInfo>, String> {
    crate::python::commands()
}

/// Run a command by id.
pub fn run(id: &str) -> Result<CommandOutcome, String> {
    crate::python::run_command(id)
}

/// Accept an open dialog: hand the shell's typed values to its callback.
///
/// The facade has already validated nothing; the shell did. The callback owns its
/// transaction and may return a new dialog spec (a wizard), which arrives in the
/// returned outcome.
pub fn accept(token: &str, values: &serde_json::Value) -> Result<CommandOutcome, String> {
    crate::python::dialog_accept(token, values)
}

/// Cancel an open dialog and drop its token.
///
pub fn cancel(token: &str) -> Result<CommandOutcome, String> {
    crate::python::dialog_cancel(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_info_deserializes_with_defaults() {
        let info: CommandInfo = serde_json::from_str(r#"{"id": "Toy_MakeSlab"}"#).unwrap();
        assert_eq!(info.id, "Toy_MakeSlab");
        assert_eq!(info.label(), "Toy_MakeSlab");
        assert!(!info.enabled);
        assert!(!info.checked);
    }

    #[test]
    fn the_menu_text_wins_when_present() {
        let info: CommandInfo = serde_json::from_str(
            r#"{"id":"Toy_MakeSlab","menu":"Create Slab","enabled":true,"checked":true}"#,
        )
        .unwrap();
        assert_eq!(info.label(), "Create Slab");
        assert!(info.enabled);
        assert!(info.checked);
    }

    #[test]
    fn a_command_outcome_deserializes() {
        let outcome: CommandOutcome =
            serde_json::from_str(r#"{"ok": true, "status": "created a slab"}"#).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.status, "created a slab");
        assert!(outcome.error.is_empty());
        assert!(outcome.value.is_none());
    }
}
