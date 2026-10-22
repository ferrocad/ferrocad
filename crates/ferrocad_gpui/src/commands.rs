//! The workbench command bridge (A0/A1).
//!
//! Commands are registered in Python on `FreeCADGui` (see
//! `docs/workbenches/actions-and-tools.md`), and the shell consumes them through the app
//! script's `commands` / `run_command` functions. This module is the typed Rust
//! side: the bar's rows ([`CommandInfo`]) and a run's result ([`CommandOutcome`]).
//!
//! Nothing here names a workbench. A command's id, label, state and meaning all
//! come from the registry.

use serde::Deserialize;

/// Which surface owns a command's control (P0.6, T0).
///
/// A command declares this in `GetResources()["FerroCAD_Surface"]`; the registry
/// does not derive it from `Group`, because `Group` says how to sort, not where to
/// put a button. An unannotated command lands on the **dock**, never nowhere: a mod
/// that predates this extension must still be reachable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The grid split button in the viewport header.
    Grid,
    /// The axis-cross toggle in the viewport header.
    Axis,
    /// The workplane split button in the viewport header.
    Workplane,
    /// The mods/provider control in the viewport header.
    Mods,
    /// The selection-driven action dock over the canvas — the default.
    #[default]
    Dock,
    /// Registered but not offered on a viewport surface. The Dev Tools command list
    /// always shows it, and the app menu may offer it explicitly (Dev Tools is the
    /// one such command today).
    Hidden,
}

impl Surface {
    /// Parse a `FerroCAD_Surface` resource value. Both the short form (`"grid"`) and
    /// the namespaced form (`"header:grid"`) are accepted.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "grid" | "header:grid" => Some(Self::Grid),
            "axis" | "axes" | "header:axis" | "header:axes" => Some(Self::Axis),
            "workplane" | "wp" | "header:workplane" | "header:wp" => Some(Self::Workplane),
            "mods" | "header:mods" => Some(Self::Mods),
            // A capability surface with no chrome of its own: the gizmo's settings are
            // reached through the transform tool's options (the dock's `Move ▾`) and
            // the Settings tree, never as a standalone toolbar entry.
            "gizmo" | "header:gizmo" => Some(Self::Hidden),
            "dock" => Some(Self::Dock),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }

    /// The canonical spelling (what the Dev Tools list shows).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Axis => "axis",
            Self::Workplane => "workplane",
            Self::Mods => "mods",
            Self::Dock => "dock",
            Self::Hidden => "hidden",
        }
    }
}

/// One command, as the chrome needs it.
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
    /// `GetResources()["FerroCAD_Surface"]` — the owning surface (P0.6 extension).
    #[serde(default)]
    pub surface: String,
    /// `GetResources()["FerroCAD_Parent"]` — the surface's primary command id, for a
    /// command offered in that control's popover (P0.6 extension).
    #[serde(default)]
    pub parent: String,
    /// `IsActive()`.
    #[serde(default)]
    pub enabled: bool,
    /// `IsChecked()` — for checkable commands.
    #[serde(default)]
    pub checked: bool,
    /// `GetResources()["FerroCAD_DevOnly"]` — listed in Dev Tools, hidden from the
    /// header/dock surfaces (P0.6 extension).
    #[serde(default)]
    pub dev_only: bool,
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

    /// The command's surface; an absent or unknown value defaults to the dock.
    pub fn surface(&self) -> Surface {
        Surface::parse(&self.surface).unwrap_or_default()
    }

    /// The popover parent's id, when declared.
    pub fn parent(&self) -> Option<&str> {
        let parent = self.parent.trim();
        (!parent.is_empty()).then_some(parent)
    }
}

/// The commands grouped by the surface that owns them (T0).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SurfaceGroups<'a> {
    pub grid: Vec<&'a CommandInfo>,
    pub axis: Vec<&'a CommandInfo>,
    pub workplane: Vec<&'a CommandInfo>,
    pub mods: Vec<&'a CommandInfo>,
    pub dock: Vec<&'a CommandInfo>,
    pub hidden: Vec<&'a CommandInfo>,
}

impl<'a> SurfaceGroups<'a> {
    /// The commands on one surface.
    pub fn get(&self, surface: Surface) -> &[&'a CommandInfo] {
        match surface {
            Surface::Grid => &self.grid,
            Surface::Axis => &self.axis,
            Surface::Workplane => &self.workplane,
            Surface::Mods => &self.mods,
            Surface::Dock => &self.dock,
            Surface::Hidden => &self.hidden,
        }
    }

    /// Every grouped command, in surface order. Used by the coverage test: the
    /// length must equal the command list's, and ids must not repeat.
    pub fn all(&self) -> Vec<&'a CommandInfo> {
        [
            self.grid.as_slice(),
            self.axis.as_slice(),
            self.workplane.as_slice(),
            self.mods.as_slice(),
            self.dock.as_slice(),
            self.hidden.as_slice(),
        ]
        .into_iter()
        .flatten()
        .copied()
        .collect()
    }
}

/// Partition every command by its surface.
pub fn group_by_surface(commands: &[CommandInfo]) -> SurfaceGroups<'_> {
    let mut groups = SurfaceGroups::default();
    for command in commands {
        match command.surface() {
            Surface::Grid => groups.grid.push(command),
            Surface::Axis => groups.axis.push(command),
            Surface::Workplane => groups.workplane.push(command),
            Surface::Mods => groups.mods.push(command),
            Surface::Dock => groups.dock.push(command),
            Surface::Hidden => groups.hidden.push(command),
        }
    }
    groups
}

/// The rows a header control's popover shows.
///
/// These are the commands on `surface` whose `FerroCAD_Parent` names the control's
/// primary command. When none declare that parent — a surface with a single command,
/// or a mod that predates the metadata — every command on the surface is shown
/// instead, including the primary, so the caret is never a dead end.
pub fn popover_commands<'a>(
    commands: &'a [CommandInfo],
    surface: Surface,
    primary: &str,
) -> Vec<&'a CommandInfo> {
    let on_surface: Vec<&CommandInfo> = commands
        .iter()
        .filter(|command| command.surface() == surface && !command.dev_only)
        .collect();
    let children: Vec<&CommandInfo> = on_surface
        .iter()
        .copied()
        .filter(|command| command.parent() == Some(primary))
        .collect();
    if children.is_empty() {
        on_surface
    } else {
        children
    }
}

/// Find a command by id.
pub fn find<'a>(commands: &'a [CommandInfo], id: &str) -> Option<&'a CommandInfo> {
    commands.iter().find(|command| command.id == id)
}

/// What running a command produced.
///
/// `ok` is false for an unknown id or a raising `Activated`; the command may add
/// a human `status`, a dialog spec with its single-use token (A3), a presentation
/// request, or a `select` directive naming a model-tree node to show.
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
    /// A presentation-only change (A5); applies no transaction.
    #[serde(default)]
    pub view: Option<crate::view::ViewRequest>,
    /// An interactive tool id to start (A5); resolved by the edition's registry.
    #[serde(default)]
    pub tool: Option<String>,
    /// A model-tree node the command wants the shell to select (S2/T5).
    ///
    /// Provider-advertised settings live at synthetic nodes such as
    /// `Settings/Grid`; returning their path here makes the shell select the node
    /// so the existing Properties pane renders its rows. It is a directive, not a
    /// failure, and needs no transaction.
    #[serde(default)]
    pub select: Option<String>,
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
        assert!(outcome.view.is_none());
        assert!(outcome.tool.is_none());
    }

    #[test]
    fn view_and_tool_requests_deserialize() {
        let outcome: CommandOutcome =
            serde_json::from_str(r#"{"ok": true, "view": {"grid": true}, "tool": "toy_polyline"}"#)
                .unwrap();
        assert_eq!(outcome.view.unwrap().grid, Some(true));
        assert_eq!(outcome.tool.as_deref(), Some("toy_polyline"));
    }

    #[test]
    fn a_select_directive_deserializes() {
        let outcome: CommandOutcome =
            serde_json::from_str(r#"{"ok": true, "select": "Settings/Grid"}"#).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.select.as_deref(), Some("Settings/Grid"));
        assert!(outcome.dialog.is_none(), "a select is not a dialog");

        let bare: CommandOutcome = serde_json::from_str(r#"{"ok": true}"#).unwrap();
        assert!(bare.select.is_none(), "select is optional");
    }

    fn command(id: &str, surface: &str, parent: &str) -> CommandInfo {
        CommandInfo {
            id: id.to_string(),
            surface: surface.to_string(),
            parent: parent.to_string(),
            ..Default::default()
        }
    }

    /// T0: `FerroCAD_Surface` is a FerroCAD extension of `GetResources`; both
    /// spellings parse and an unknown value falls back to the dock.
    #[test]
    fn surface_metadata_parses_and_defaults_to_the_dock() {
        assert_eq!(Surface::parse("grid"), Some(Surface::Grid));
        assert_eq!(Surface::parse("header:grid"), Some(Surface::Grid));
        assert_eq!(Surface::parse(" axis "), Some(Surface::Axis));
        assert_eq!(Surface::parse("header:workplane"), Some(Surface::Workplane));
        assert_eq!(Surface::parse("wp"), Some(Surface::Workplane));
        assert_eq!(Surface::parse("mods"), Some(Surface::Mods));
        assert_eq!(
            Surface::parse("gizmo"),
            Some(Surface::Hidden),
            "a capability surface with no chrome must not fall back to the dock"
        );
        assert_eq!(Surface::parse("dock"), Some(Surface::Dock));
        assert_eq!(Surface::parse("hidden"), Some(Surface::Hidden));
        assert_eq!(Surface::parse("nowhere"), None);

        let bare: CommandInfo = serde_json::from_str(r#"{"id": "Mod_Foo"}"#).unwrap();
        assert_eq!(bare.surface(), Surface::Dock, "unannotated never disappears");
        assert!(bare.parent().is_none());

        let odd = command("Mod_Bar", "nowhere", "");
        assert_eq!(odd.surface(), Surface::Dock, "an unknown surface is the dock");

        let annotated: CommandInfo = serde_json::from_str(
            r#"{"id":"View_GridSettings","surface":"grid","parent":"App_ToggleGrid"}"#,
        )
        .unwrap();
        assert_eq!(annotated.surface(), Surface::Grid);
        assert_eq!(annotated.parent(), Some("App_ToggleGrid"));
    }

    /// T0/T6: every command lands on exactly one surface — the partition is total and
    /// disjoint.
    #[test]
    fn every_command_lands_on_exactly_one_surface() {
        let commands = vec![
            command("App_ToggleGrid", "grid", ""),
            command("View_GridSettings", "grid", "App_ToggleGrid"),
            command("App_ToggleAxisCross", "axis", ""),
            command("App_ViewWorkplane", "workplane", ""),
            command("App_ActivateWorkplane", "workplane", "App_ViewWorkplane"),
            command("Mods_Settings", "mods", ""),
            command("Toy_MakeSlab", "dock", ""),
            command("Dev_ToggleTools", "hidden", ""),
            command("Mod_Unannotated", "", ""),
        ];
        let groups = group_by_surface(&commands);
        let all = groups.all();
        assert_eq!(all.len(), commands.len(), "nothing is dropped");
        let mut ids: Vec<&str> = all.iter().map(|command| command.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), commands.len(), "nothing is on two surfaces");
        assert!(
            groups.dock.iter().any(|command| command.id == "Mod_Unannotated"),
            "the unannotated command is on the dock"
        );
        assert_eq!(groups.grid.len(), 2);
        assert_eq!(groups.workplane.len(), 2);
    }

    /// T2 support: a popover shows the surface's children, and falls back to the
    /// surface's own commands rather than rendering nothing.
    #[test]
    fn a_popover_lists_the_surface_children_or_the_surface_itself() {
        let commands = vec![
            command("App_ToggleGrid", "grid", ""),
            command("View_GridSettings", "grid", "App_ToggleGrid"),
            command("App_ReloadGridSettings", "grid", "App_ToggleGrid"),
            command("Mods_Settings", "mods", ""),
        ];
        let grid = popover_commands(&commands, Surface::Grid, "App_ToggleGrid");
        let ids: Vec<&str> = grid.iter().map(|command| command.id.as_str()).collect();
        assert_eq!(ids, ["View_GridSettings", "App_ReloadGridSettings"]);

        let mods = popover_commands(&commands, Surface::Mods, "Mods_Settings");
        let ids: Vec<&str> = mods.iter().map(|command| command.id.as_str()).collect();
        assert_eq!(ids, ["Mods_Settings"], "the fallback keeps the caret useful");
    }

    /// A dev-only command is still registered and grouped, but a header popover
    /// never offers it.
    #[test]
    fn a_dev_only_child_is_filtered_from_the_popover() {
        let mut dev_only = command("View_GridDevSettings", "grid", "App_ToggleGrid");
        dev_only.dev_only = true;
        let commands = vec![
            command("App_ToggleGrid", "grid", ""),
            command("View_GridSettings", "grid", "App_ToggleGrid"),
            dev_only,
        ];
        let ids: Vec<&str> = popover_commands(&commands, Surface::Grid, "App_ToggleGrid")
            .into_iter()
            .map(|command| command.id.as_str())
            .collect();
        assert_eq!(ids, ["View_GridSettings"]);
    }

    /// The filter is only for popovers: the Dev Tools list and `find` still see
    /// the command, so hiding it from a surface never makes it unreachable.
    #[test]
    fn a_dev_only_command_stays_in_the_full_list_and_find() {
        let mut dev_only = command("Mods_Settings", "mods", "");
        dev_only.dev_only = true;
        let commands = vec![command("App_ToggleGrid", "grid", ""), dev_only];
        assert!(find(&commands, "Mods_Settings").is_some());
        let groups = group_by_surface(&commands);
        assert!(
            groups.mods.iter().any(|command| command.id == "Mods_Settings"),
            "the command is still grouped on its surface"
        );
        assert_eq!(groups.all().len(), commands.len());
    }
}
