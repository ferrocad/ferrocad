//! The built-in command catalogue, as **data**.
//!
//! Both hosts offer the same core commands, but each used to spell their metadata:
//! the native registry in `python/ferrocad_shell`, the browser backend in a
//! hand-written `commands()`. The two drifted (an axis label, an accelerator, two
//! file verbs that were in one host's dock but not the other's), and the Move form
//! existed twice.
//!
//! This module is the one table. It is static data (no Python, no gpui, no engine):
//!
//! - [`builtin`] feeds a Rust backend directly (see `ferrocad_web_gpui`);
//! - [`json_text`] is generated into `python/ferrocad_shell/catalog.json`, which the
//!   native registry reads, so a label has one spelling everywhere;
//! - a test compares the checked-in JSON with this table, and a Python test compares
//!   the live `FreeCADGui` registry with the JSON — either side drifting fails CI.
//!
//! Workbenches keep registering their own commands at runtime (`FreeCADGui.addCommand`);
//! this is only the **built-in** set.

use serde_json::{Value, json};

use crate::commands::CommandInfo;
use crate::dialog::{DialogField, DialogSpec};

/// One built-in command's metadata.
///
/// The field names are the shell's vocabulary, not FreeCAD's `GetResources` keys;
/// [`json_text`] maps them to the native registry's spelling once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDef {
    pub id: &'static str,
    /// `MenuText` — the button/row label.
    pub menu: &'static str,
    pub tooltip: &'static str,
    /// `Accel`, in the upstream grammar (`"Ctrl+Shift+M"`); empty for none.
    pub accel: &'static str,
    pub group: &'static str,
    /// `CmdType` (`"ForView"` / `"ForEdit"`).
    pub cmd_type: &'static str,
    /// `FerroCAD_Surface` (`"grid"`, `"radial"`, `"hidden"`, …).
    pub surface: &'static str,
    /// `FerroCAD_Short` — the compact popover label.
    pub short: &'static str,
    /// `FerroCAD_Tone` — a radial slot's semantic class.
    pub tone: &'static str,
    /// `FerroCAD_Parent` — the surface's primary command id.
    pub parent: &'static str,
    /// `FerroCAD_Wheel` — also offer it in the radial menu.
    pub wheel: bool,
    /// `Checkable` (or a command that implements `IsChecked`).
    pub checkable: bool,
    /// `FerroCAD_DevOnly` — diagnostics only.
    pub dev_only: bool,
}

const fn def(
    id: &'static str,
    menu: &'static str,
    tooltip: &'static str,
    accel: &'static str,
    group: &'static str,
    cmd_type: &'static str,
    surface: &'static str,
) -> CommandDef {
    CommandDef {
        id,
        menu,
        tooltip,
        accel,
        group,
        cmd_type,
        surface,
        short: "",
        tone: "",
        parent: "",
        wheel: false,
        checkable: false,
        dev_only: false,
    }
}

/// The built-in ids, named once.
///
/// The table below spells each id through these constants, and a Rust host can match
/// on the same names instead of repeating the strings.
pub const TOGGLE_GRID: &str = "App_ToggleGrid";
pub const TOGGLE_AXIS_CROSS: &str = "App_ToggleAxisCross";
pub const VIEW_WORKPLANE: &str = "App_ViewWorkplane";
pub const TOGGLE_GIZMO: &str = "App_ToggleGizmo";
pub const DEV_TOOLS: &str = "Dev_ToggleTools";
pub const OPEN: &str = "App_Open";
pub const SAVE: &str = "App_Save";
pub const MOVE: &str = "App_Move";
pub const ROTATE: &str = "App_Rotate";

/// The built-in commands **both hosts** offer.
///
/// The labels are the native registry's (it is the FreeCAD-shaped reference); the
/// browser renders the same strings. Where a header control wants a compact label,
/// `short` carries it (`Axes`, `WP`).
pub const BUILTIN: &[CommandDef] = &[
    CommandDef {
        checkable: true,
        ..def(
            TOGGLE_GRID,
            "Grid",
            "Show the reference grid",
            "",
            "View",
            "ForView",
            "grid",
        )
    },
    CommandDef {
        short: "Axes",
        checkable: true,
        ..def(
            TOGGLE_AXIS_CROSS,
            "Axis cross",
            "Show the world axis cross",
            "",
            "View",
            "ForView",
            "axis",
        )
    },
    CommandDef {
        short: "WP",
        parent: "App_Workplane",
        ..def(
            VIEW_WORKPLANE,
            "Align camera to WP",
            "Look down the active workplane's normal",
            "",
            "View",
            "ForView",
            "workplane",
        )
    },
    CommandDef {
        checkable: true,
        ..def(
            TOGGLE_GIZMO,
            "Gizmo",
            "Show the transform gizmo",
            "",
            "View",
            "ForView",
            "gizmo",
        )
    },
    def(
        DEV_TOOLS,
        "Dev Tools",
        "Open or close the developer tools window",
        "Ctrl+Shift+D",
        "View",
        "ForView",
        "hidden",
    ),
    def(
        OPEN,
        "Open…",
        "Open a saved document",
        "Ctrl+O",
        "File",
        "ForView",
        "hidden",
    ),
    def(
        SAVE,
        "Save As…",
        "Save the document as a file",
        "Ctrl+S",
        "File",
        "ForView",
        "hidden",
    ),
    CommandDef {
        tone: "structural",
        wheel: true,
        ..def(
            MOVE,
            "Move",
            "Move the selected objects by a delta",
            "Ctrl+Shift+M",
            "Edit",
            "ForEdit",
            "radial",
        )
    },
    CommandDef {
        tone: "structural",
        wheel: true,
        ..def(
            ROTATE,
            "Rotate",
            "Rotate the selected objects about the gizmo origin",
            "Ctrl+Shift+R",
            "Edit",
            "ForEdit",
            "radial",
        )
    },
];

/// The catalogue, for a Rust host.
pub fn builtin() -> &'static [CommandDef] {
    BUILTIN
}

/// One built-in by id.
pub fn find(id: &str) -> Option<&'static CommandDef> {
    BUILTIN.iter().find(|def| def.id == id)
}

impl CommandDef {
    /// This definition as the shell's per-frame row.
    ///
    /// Live state (`enabled`/`checked`) is the backend's to fill in; the catalogue
    /// only carries metadata, so a host cannot disagree about a label.
    pub fn info(&self) -> CommandInfo {
        CommandInfo {
            id: self.id.to_string(),
            menu: self.menu.to_string(),
            tooltip: self.tooltip.to_string(),
            accel: self.accel.to_string(),
            group: self.group.to_string(),
            cmd_type: self.cmd_type.to_string(),
            surface: self.surface.to_string(),
            short: self.short.to_string(),
            tone: self.tone.to_string(),
            parent: self.parent.to_string(),
            wheel: self.wheel,
            enabled: true,
            checked: false,
            checkable: self.checkable,
            dev_only: self.dev_only,
        }
    }
}

/// The `App_Move` form — the typed twin of the gizmo.
///
/// `presentation: "float"` and `preview: "translation"` are what make it the hand's
/// always-on HUD (anchored beside the triad) rather than a modal, and let the typed
/// delta preview on the selection while the geometry stays interactive.
pub fn move_dialog() -> DialogSpec {
    let field = |name: &str, label: &str| DialogField {
        name: name.to_string(),
        label: label.to_string(),
        kind: "quantity".to_string(),
        value: Some(json!(0.0)),
        ..Default::default()
    };
    DialogSpec {
        title: "Move".to_string(),
        accept: "Move".to_string(),
        presentation: "float".to_string(),
        preview: "translation".to_string(),
        fields: vec![field("dx", "ΔX"), field("dy", "ΔY"), field("dz", "ΔZ")],
        ..Default::default()
    }
}

/// The `gizmo` directive `App_Move` returns with its form (the arrows, not the rings).
pub const MOVE_GIZMO: &str = "translate";

/// The catalogue as the document the native registry reads.
///
/// Resource keys are spelled the way FreeCAD's `GetResources` spells them, so the
/// Python side is a straight lookup; the shell's field names are an internal detail.
pub fn to_json() -> Value {
    let commands: Vec<Value> = BUILTIN
        .iter()
        .map(|def| {
            let mut object = serde_json::Map::new();
            object.insert("id".to_string(), json!(def.id));
            object.insert("menu".to_string(), json!(def.menu));
            object.insert("tooltip".to_string(), json!(def.tooltip));
            if !def.accel.is_empty() {
                object.insert("accel".to_string(), json!(def.accel));
            }
            object.insert("group".to_string(), json!(def.group));
            object.insert("cmd_type".to_string(), json!(def.cmd_type));
            if !def.surface.is_empty() {
                object.insert("surface".to_string(), json!(def.surface));
            }
            if !def.short.is_empty() {
                object.insert("short".to_string(), json!(def.short));
            }
            if !def.tone.is_empty() {
                object.insert("tone".to_string(), json!(def.tone));
            }
            if !def.parent.is_empty() {
                object.insert("parent".to_string(), json!(def.parent));
            }
            if def.wheel {
                object.insert("wheel".to_string(), json!(true));
            }
            if def.checkable {
                object.insert("checkable".to_string(), json!(true));
            }
            if def.dev_only {
                object.insert("dev_only".to_string(), json!(true));
            }
            Value::Object(object)
        })
        .collect();

    let dialog = move_dialog();
    let fields: Vec<Value> = dialog
        .fields
        .iter()
        .map(|field| {
            json!({
                "name": field.name,
                "label": field.label,
                "kind": field.kind,
                "value": field.value,
            })
        })
        .collect();

    json!({
        "generated_by": "ferrocad_shell_api::catalog",
        "commands": commands,
        "dialogs": {
            (MOVE): {
                "title": dialog.title,
                "accept": dialog.accept,
                "presentation": dialog.presentation,
                "preview": dialog.preview,
                "gizmo": MOVE_GIZMO,
                "fields": fields,
            }
        }
    })
}

/// [`to_json`], pretty-printed (what lands in `python/ferrocad_shell/catalog.json`).
pub fn json_text() -> String {
    let mut text = serde_json::to_string_pretty(&to_json()).expect("the catalogue serializes");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Surface;

    #[test]
    fn every_id_is_unique_and_every_surface_parses() {
        let mut ids = std::collections::BTreeSet::new();
        for def in BUILTIN {
            assert!(ids.insert(def.id), "duplicate catalogue id {}", def.id);
            assert!(!def.menu.is_empty(), "{} has no label", def.id);
            assert!(
                Surface::parse(def.surface).is_some(),
                "{}: unknown surface {:?}",
                def.id,
                def.surface
            );
            assert!(
                def.cmd_type == "ForView" || def.cmd_type == "ForEdit",
                "{}: unexpected CmdType {:?}",
                def.id,
                def.cmd_type
            );
        }
    }

    #[test]
    fn the_row_carries_the_definition() {
        let def = find("App_Move").expect("Move is a built-in");
        let row = def.info();
        assert_eq!(row.id, "App_Move");
        assert_eq!(row.label(), "Move");
        assert_eq!(row.accel, "Ctrl+Shift+M");
        assert_eq!(row.surface(), Surface::Radial);
        assert!(row.wheel);
        assert!(row.enabled, "metadata only: the host fills live state");
        assert!(!row.checked);
    }

    /// The JSON checked into the Python package is this table, field for field.
    ///
    /// The test reads the file at run time (not `include_str!`), so packaging the
    /// crate on its own does not depend on a file outside it: a published copy just
    /// skips this check.
    #[test]
    fn the_checked_in_catalogue_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../python/ferrocad_shell/catalog.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        assert_eq!(
            text,
            json_text(),
            "python/ferrocad_shell/catalog.json is stale: run \
             `cargo run -p ferrocad_shell_api --example catalog > {}`",
            path.display()
        );
    }
}
