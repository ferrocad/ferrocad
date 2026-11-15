//! The workbench command bridge (A0/A1).
//!
//! Commands are registered in Python on `FreeCADGui` (see
//! `docs/developer/workbenches/actions-and-tools.md`), and the shell consumes them through the app
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
/// put a button. An unannotated command lands in the **palette**, never nowhere: a mod
/// that predates this extension must still be reachable (Search and Dev Tools list it),
/// and must never appear on a canvas strip it did not ask for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The grid split button in the viewport header.
    Grid,
    /// The axis-cross toggle in the viewport header.
    Axis,
    /// The workplane split button in the viewport header.
    Workplane,
    /// The snap split button in the viewport header: the global switch on the label,
    /// the individual snap modes in its popover.
    Snap,
    /// The in-situ radial menu's object verbs. These are not dock actions: they
    /// materialise around the pointer when geometry is selected.
    Radial,
    /// The mods/provider control in the viewport header.
    Mods,
    /// The **creation pill**: a tool that creates new geometry, offered by the one
    /// split control in the action dock and by the palette. It never becomes a dock
    /// button of its own — a dock full of creation verbs is a toolbar, and the pill is
    /// what the Add hand replaced it with.
    Creation,
    /// Search-only: registered and listed in the palette (and Dev Tools), offered by no
    /// viewport surface. A utility that is not context-appropriate anywhere belongs
    /// here rather than in the wheel — and it is the **default**, so a mod that declares
    /// no surface cannot overflow a canvas strip.
    #[default]
    Palette,
    /// The selection-driven action dock over the canvas. Only the built-in globals
    /// (`Undo`, `Redo`, `Fit`) declare it: the strip's other two controls are the
    /// creation pill and Search, which are shell chrome rather than registry rows.
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
            "snap" | "snapping" | "header:snap" | "header:snapping" => Some(Self::Snap),
            "radial" | "wheel" => Some(Self::Radial),
            "mods" | "header:mods" => Some(Self::Mods),
            "creation" | "pill" | "create" => Some(Self::Creation),
            "palette" | "search" => Some(Self::Palette),
            // A capability surface with no chrome of its own: the gizmo's settings are
            // reached through the wheel's `Move`/`Rotate` verbs and the Settings tree,
            // never as a standalone toolbar entry.
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
            Self::Snap => "snap",
            Self::Radial => "radial",
            Self::Mods => "mods",
            Self::Creation => "creation",
            Self::Palette => "palette",
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
    /// `GetResources()["FerroCAD_Short"]` — a compact label for a header popover,
    /// where the control's own name makes the full one repetitive (P0.6 extension).
    #[serde(default)]
    pub short: String,
    /// `GetResources()["FerroCAD_Tone"]` — a radial slot's semantic class:
    /// `structural` (default), `additive` or `destructive` (P0.6 extension).
    #[serde(default)]
    pub tone: String,
    /// `GetResources()["FerroCAD_Category"]` — the creation category a tool is offered
    /// under in the creation pill (`Sketch`, `Solid`, `Surface`, `Text`, `Mesh`).
    #[serde(default)]
    pub category: String,
    /// `GetResources()["FerroCAD_Target"]` — which context targets a radial verb
    /// applies to: `body`, `face`, `edge`, or a comma-separated combination. Empty (or
    /// `any`) means every context, which is the right default for a mod's object verb
    /// and the wrong answer for a face-only tool, so the wheel's own verbs declare it.
    #[serde(default)]
    pub target: String,
    /// `GetResources()["FerroCAD_Gate"]` — the topology a creation command needs before
    /// it can run (`"body:2"`, `"face:1:1"`, `"edge:1"`). Empty means always enabled.
    ///
    /// This is the chrome's half of `TopologyGate`: it is evaluated against the current
    /// selection so the pill can grey a row and the scope switch can refuse to move the
    /// ground under an armed tool. The command's own `IsActive` stays the authority on
    /// whether it can *run*.
    #[serde(default)]
    pub gate: String,
    /// `GetResources()["FerroCAD_WheelGroup"]` — the wheel category a radial verb is a
    /// **child** of (`"Surface"`, `"Booleans"`, `"Edge"`, `"Workplane"`).
    ///
    /// The root ring stays six to eight wedges because a group is one wedge: the verbs
    /// materialise only once the group is opened. Empty means the verb is a root wedge.
    #[serde(default)]
    pub wheel_group: String,
    /// `GetResources()["FerroCAD_Parent"]` — the surface's primary command id, for a
    /// command offered in that control's popover (P0.6 extension).
    #[serde(default)]
    pub parent: String,
    /// `GetResources()["Pixmap"]` — upstream's icon resource. The shell resolves it
    /// **first** as a vendored icon key (so a mod can name a glyph directly), then
    /// through the curated alias table, and finally falls back to the command id.
    #[serde(default)]
    pub pixmap: String,
    /// `GetResources()["FerroCAD_Wheel"]` — also offer this command in the radial
    /// menu, beside the `radial`-surface object verbs (P0.6 extension). A view-only
    /// action that still belongs to the geometry under the pointer (`Set WP`) is not
    /// an object verb, but it is exactly what the wheel is for.
    #[serde(default)]
    pub wheel: bool,
    /// `IsActive()`.
    #[serde(default)]
    pub enabled: bool,
    /// `IsChecked()` — for checkable commands.
    #[serde(default)]
    pub checked: bool,
    /// `GetResources()["Checkable"]` — the row is a toggle rather than a one-shot
    /// action, so a popover keeps its menu open after the click.
    #[serde(default)]
    pub checkable: bool,
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

    /// The compact label a header popover shows: `FerroCAD_Short`, else `MenuText`.
    pub fn short_label(&self) -> &str {
        if self.short.is_empty() {
            self.label()
        } else {
            &self.short
        }
    }

    /// The command's surface; an absent or unknown value defaults to the palette.
    pub fn surface(&self) -> Surface {
        Surface::parse(&self.surface).unwrap_or_default()
    }

    /// The popover parent's id, when declared.
    pub fn parent(&self) -> Option<&str> {
        let parent = self.parent.trim();
        (!parent.is_empty()).then_some(parent)
    }

    /// The creation category, when the command declares one.
    pub fn category(&self) -> Option<&str> {
        let category = self.category.trim();
        (!category.is_empty()).then_some(category)
    }

    /// The target kinds a radial verb applies to; `None` means "any".
    pub fn targets(&self) -> Option<Vec<TargetKind>> {
        let mut kinds = Vec::new();
        for word in self.target.split(|c: char| c == ',' || c.is_whitespace()) {
            match word.trim().to_ascii_lowercase().as_str() {
                "body" | "object" | "solid" => kinds.push(TargetKind::Body),
                "face" => kinds.push(TargetKind::Face),
                "edge" | "wire" => kinds.push(TargetKind::Edge),
                _ => {}
            }
        }
        (!kinds.is_empty()).then_some(kinds)
    }

    /// Whether the verb is offered for a resolved context target.
    pub fn applies_to(&self, kind: TargetKind) -> bool {
        self.targets()
            .is_none_or(|kinds| kinds.contains(&kind))
    }

    /// The parsed gate, when the command declares one.
    pub fn topology_gate(&self) -> Option<TopologyGate> {
        TopologyGate::parse(&self.gate)
    }

    /// The wheel group this verb belongs to, when it is a child rather than a root wedge.
    pub fn wheel_group(&self) -> Option<&str> {
        let group = self.wheel_group.trim();
        (!group.is_empty()).then_some(group)
    }
}

/// What a creation command needs selected before it can run.
///
/// Declared as `FerroCAD_Gate: "<kind>:<min>[:<max>]"` — `body:2` (two or more
/// references), `face:1:1` (exactly one face), `edge:1`, `profile:2` (a sweep needs two
/// closed profiles). A **reference** is one selected object or sub-element, so a
/// whole-object selection counts as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyGate {
    /// The kind of reference required.
    pub kind: ElementKind,
    /// The fewest references that satisfy the gate.
    pub min_arity: usize,
    /// The most references it accepts, when it has a ceiling.
    pub max_arity: Option<usize>,
}

impl TopologyGate {
    /// Parse `"body:2"`, `"face:1:1"`, `"any:0"`; `None` for empty or malformed.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let mut parts = value.split(':');
        let kind = match parts.next()?.trim().to_ascii_lowercase().as_str() {
            "any" | "" => ElementKind::Any,
            "body" | "object" | "solid" => ElementKind::Body,
            "face" => ElementKind::Face,
            "edge" | "wire" => ElementKind::Edge,
            // A profile is what a loft or a sweep consumes — a closed wire, a face or a
            // whole solid. It is its own kind so the hint can say *profiles* while any
            // of the three still satisfies it.
            "profile" | "profiles" => ElementKind::Profile,
            _ => return None,
        };
        let min_arity = parts.next().and_then(|min| min.trim().parse().ok()).unwrap_or(0);
        let max_arity = parts.next().and_then(|max| max.trim().parse().ok());
        Some(Self {
            kind,
            min_arity,
            max_arity,
        })
    }

    /// Whether `count` references of `kind` satisfy the gate.
    pub fn satisfied(&self, count: usize, kind: ElementKind) -> bool {
        let kind_ok = match self.kind {
            // A profile is any closed reference the kernel can loft: a whole body, a
            // face, or a wire (which a selection reports as an edge).
            ElementKind::Profile => kind != ElementKind::Profile || count > 0,
            mine => mine == ElementKind::Any || kind == ElementKind::Any || kind == mine,
        };
        kind_ok && count >= self.min_arity && self.max_arity.is_none_or(|max| count <= max)
    }

    /// The sentence a refused click reports (`select 1 face`).
    pub fn requirement(&self) -> String {
        if self.kind == ElementKind::Any && self.min_arity == 0 {
            return String::new();
        }
        let (one, many) = match self.kind {
            ElementKind::Any => ("reference", "references"),
            ElementKind::Body => ("body", "bodies"),
            ElementKind::Face => ("face", "faces"),
            ElementKind::Edge => ("edge", "edges"),
            ElementKind::Profile => ("profile", "profiles"),
        };
        match self.max_arity {
            Some(max) if max == self.min_arity => {
                format!("select {max} {}", if max == 1 { one } else { many })
            }
            Some(max) => format!("select {}–{max} {many}", self.min_arity),
            None if self.min_arity <= 1 => format!("select a {one}"),
            None => format!("select at least {} {many}", self.min_arity),
        }
    }
}

/// The kind of reference a [`TopologyGate`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementKind {
    /// Any reference at all.
    Any,
    /// A whole body (or several).
    Body,
    /// A face (`Face6`).
    Face,
    /// An edge (`Edge3`).
    Edge,
    /// A closed profile a loft or a sweep consumes: a wire, a face, or a whole solid.
    /// Any of the three satisfies it; the kind exists so the requirement can say
    /// "select at least 2 profiles" instead of naming one of them.
    Profile,
}

impl ElementKind {
    /// The kind a selected sub-element name names, or `Body` for a whole object.
    pub fn of_element(element: Option<&str>) -> Self {
        match element {
            Some(name) if name.starts_with("Face") => Self::Face,
            Some(name) if name.starts_with("Edge") => Self::Edge,
            _ => Self::Body,
        }
    }
}

/// The kind of thing under the pointer, as the wheel contextualises it.
///
/// The wheel is a **context** menu, not a toolbar: a face has shell/draft verbs, an
/// edge has dress-up verbs, and the ring stays a ring because each verb says which
/// kinds it applies to (`FerroCAD_Target`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// A whole body (or several): object verbs.
    Body,
    /// A planar or curved face (`Face6`): surface verbs.
    Face,
    /// An edge (`Edge3`): dress-up verbs.
    Edge,
}

impl TargetKind {
    /// The kind a selected sub-element name names, if it names one.
    pub fn of_element(element: &str) -> Option<Self> {
        if element.starts_with("Face") {
            Some(Self::Face)
        } else if element.starts_with("Edge") {
            Some(Self::Edge)
        } else {
            None
        }
    }
}

/// The commands grouped by the surface that owns them (T0).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SurfaceGroups<'a> {
    pub grid: Vec<&'a CommandInfo>,
    pub axis: Vec<&'a CommandInfo>,
    pub workplane: Vec<&'a CommandInfo>,
    pub snap: Vec<&'a CommandInfo>,
    pub radial: Vec<&'a CommandInfo>,
    pub creation: Vec<&'a CommandInfo>,
    pub mods: Vec<&'a CommandInfo>,
    pub dock: Vec<&'a CommandInfo>,
    pub palette: Vec<&'a CommandInfo>,
    pub hidden: Vec<&'a CommandInfo>,
}

impl<'a> SurfaceGroups<'a> {
    /// The commands on one surface.
    pub fn get(&self, surface: Surface) -> &[&'a CommandInfo] {
        match surface {
            Surface::Grid => &self.grid,
            Surface::Axis => &self.axis,
            Surface::Workplane => &self.workplane,
            Surface::Snap => &self.snap,
            Surface::Radial => &self.radial,
            Surface::Mods => &self.mods,
            Surface::Creation => &self.creation,
            Surface::Palette => &self.palette,
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
            self.snap.as_slice(),
            self.radial.as_slice(),
            self.creation.as_slice(),
            self.mods.as_slice(),
            self.dock.as_slice(),
            self.palette.as_slice(),
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
            Surface::Snap => groups.snap.push(command),
            Surface::Radial => groups.radial.push(command),
            Surface::Mods => groups.mods.push(command),
            Surface::Creation => groups.creation.push(command),
            Surface::Palette => groups.palette.push(command),
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
    /// A transform-gizmo mode the command wants (`translate`, `rotate`,
    /// `universal`). Session tool state, not a document edit: `Move` returns
    /// `translate` so the deferred HUD's fields and the visible arrows agree, and
    /// the wheel's `Rotate` returns `rotate`.
    #[serde(default)]
    pub gizmo: Option<String>,
    /// Arm the active plane's `+N` offset stem (`WorkplaneStem`).
    ///
    /// `Offset Workplane…` returns this with its floating HUD: the stem is the
    /// gesture half of one session and the dialog is the typed half. It is transient
    /// viewport state — no document object, no transaction, no undo step — so the
    /// shell intercepts the stem's commit instead of routing it through the app
    /// script.
    ///
    /// The Python facade forwards this key, and `Offset Workplane…` sends it beside
    /// its HUD; the older `gizmo: "workplane_offset"` spelling still rides along and
    /// `Shell::run_command` accepts either, so a facade and a shell of different
    /// vintages still open the same session.
    #[serde(default)]
    pub workplane_stem: bool,
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

    #[test]
    fn a_workplane_stem_directive_deserializes() {
        let outcome: CommandOutcome =
            serde_json::from_str(r#"{"ok": true, "workplane_stem": true}"#).unwrap();
        assert!(outcome.workplane_stem, "the offset command asks for the stem");

        let bare: CommandOutcome = serde_json::from_str(r#"{"ok": true}"#).unwrap();
        assert!(!bare.workplane_stem, "the directive is opt-in");
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
        assert_eq!(Surface::parse("snap"), Some(Surface::Snap));
        assert_eq!(Surface::parse("header:snap"), Some(Surface::Snap));
        assert_eq!(Surface::parse("snapping"), Some(Surface::Snap));
        assert_eq!(Surface::parse("radial"), Some(Surface::Radial));
        assert_eq!(Surface::parse("wheel"), Some(Surface::Radial));
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
        assert_eq!(
            bare.surface(),
            Surface::Palette,
            "unannotated never disappears, and never lands on a canvas strip"
        );
        assert!(bare.parent().is_none());

        let odd = command("Mod_Bar", "nowhere", "");
        assert_eq!(
            odd.surface(),
            Surface::Palette,
            "an unknown surface is the palette, not the dock"
        );

        let annotated: CommandInfo = serde_json::from_str(
            r#"{"id":"View_GridSettings","surface":"grid","parent":"App_ToggleGrid"}"#,
        )
        .unwrap();
        assert_eq!(annotated.surface(), Surface::Grid);
        assert_eq!(annotated.parent(), Some("App_ToggleGrid"));
    }

    /// A header popover shows `FerroCAD_Short` when the full label repeats the
    /// control's own name (`Snap Grid` under the `Snap` control).
    #[test]
    fn a_short_label_is_used_in_a_popover_and_falls_back_to_the_menu_label() {
        let annotated: CommandInfo = serde_json::from_str(
            r#"{"id":"Draft_Snap_Grid","menu":"Snap Grid","short":"Grid"}"#,
        )
        .unwrap();
        assert_eq!(annotated.label(), "Snap Grid");
        assert_eq!(annotated.short_label(), "Grid");

        let bare: CommandInfo =
            serde_json::from_str(r#"{"id":"Draft_Snap_Origin","menu":"Snap Origin"}"#)
                .unwrap();
        assert_eq!(bare.short_label(), "Snap Origin", "no short label, no change");
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
            groups
                .palette
                .iter()
                .any(|command| command.id == "Mod_Unannotated"),
            "the unannotated command is searchable, not a dock button"
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

    /// A **profile** gate is what a loft or a sweep consumes: two closed references,
    /// whatever they are (wires, faces, solids). The requirement has to say "profiles"
    /// rather than claiming two whole bodies.
    #[test]
    fn a_profile_gate_names_profiles_and_accepts_any_closed_reference() {
        let gate = TopologyGate::parse("profile:2").expect("a profile gate");
        assert_eq!(gate.kind, ElementKind::Profile);
        assert_eq!(gate.requirement(), "select at least 2 profiles");
        for kind in [
            ElementKind::Body,
            ElementKind::Face,
            ElementKind::Edge,
            ElementKind::Any,
        ] {
            assert!(!gate.satisfied(1, kind), "one {kind:?} is not two profiles");
            assert!(gate.satisfied(2, kind), "two {kind:?} are two profiles");
        }
        // The single kind that is not a reference at all.
        assert_eq!(
            TopologyGate::parse("face:1:1").map(|gate| gate.requirement()),
            Some("select 1 face".to_string())
        );
    }
}
