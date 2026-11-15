//! The Add hand: one creation pill per category, in a strip of their own.
//!
//! The action dock used to carry **one** memory pill (`⊕ <LastTool> ▾`) and a flat
//! flyout that mixed every tool an edition registered — 2-D drawing tools, primitives
//! and profile features in one column. The Add hand is now a **create dock**: a row of
//! pills above the action strip, one per category the edition registers
//! (`⊕ Sketch: Rectangle ▾`, `⊕ Solid: Box ▾`, `⊕ Surface: Loft ▾`, `⊕ Mesh: Mesh Box ▾`).
//!
//! - Each pill remembers the tool its category was left on, so the row is a **loadout**
//!   of the edition's creation tools rather than a menu you re-open.
//! - Its caret opens the flyout, which shows one category at a time behind a segmented
//!   header (`Sketch | Solid | Surface | …`, empty categories omitted). Picking a tab
//!   points that category's pill at the tool the category remembers, so the pill behind
//!   the flyout already shows what the next click would arm. Picking a tool runs it.
//! - `4`, `5`, … arm the pill at that position; pressing the same key again opens that
//!   pill's flyout. The action dock carries a single `⊕ Create Dock ^` toggle, which
//!   shows and hides the strip, so the strip is never mixed into the dock's own row.
//!
//! The radial menu's `Create ▸` commits through the same path, so a wheel flick loads a
//! pill and arms the tool at the pointer.
//!
//! See `docs/developer/interaction/creation-loadout.md` §0.1.

use std::collections::HashMap;

use crate::shell::*;

/// The most creation pills the strip holds: one per [`crate::chrome::CREATION_CATEGORIES`]
/// entry, in that order.
pub(crate) const CREATION_SLOTS: usize = 5;
/// The key labels, by pill position.
pub(crate) const CREATION_KEYS: [&str; CREATION_SLOTS] = ["4", "5", "6", "7", "8"];

/// The Add hand's state: which tool each category remembers, the last tool armed, the
/// flyout's tab, and whether the strip is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreationDock {
    /// The tool each category remembers, by command id.
    memory: HashMap<String, String>,
    /// The tool last armed overall: the pill's accent, `PILL_HOLD`, and the key re-press
    /// (a second press opens that pill's flyout instead of re-arming).
    active: Option<String>,
    /// The flyout's category tab while it is open.
    tab: Option<String>,
    /// Whether the create dock (the strip of pills) is showing. View state: the dock's
    /// `⊕ Create Dock` toggle flips it, and it is not saved with the document.
    expanded: bool,
}

impl Default for CreationDock {
    fn default() -> Self {
        Self {
            memory: HashMap::new(),
            active: None,
            tab: None,
            expanded: true,
        }
    }
}

impl CreationDock {
    /// The tool a category remembers.
    pub(crate) fn memory(&self, category: &str) -> Option<&str> {
        self.memory.get(category).map(String::as_str)
    }

    /// The tool last armed.
    pub(crate) fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The flyout's open tab.
    pub(crate) fn tab(&self) -> Option<&str> {
        self.tab.as_deref()
    }

    /// Whether the create dock is showing.
    pub(crate) fn expanded(&self) -> bool {
        self.expanded
    }

    /// Show or hide the create dock; returns the new state.
    pub(crate) fn toggle_expanded(&mut self) -> bool {
        self.expanded = !self.expanded;
        if !self.expanded {
            self.tab = None;
        }
        self.expanded
    }

    /// Record that `id` (in `category`) is now the armed tool.
    ///
    /// This is the one write a tool pick needs: the category remembers it (so its pill
    /// names it) and it becomes the armed tool.
    pub(crate) fn remember(&mut self, id: &str, category: Option<&str>) {
        if let Some(category) = category {
            self.memory.insert(category.to_string(), id.to_string());
        }
        self.active = Some(id.to_string());
    }

    /// Point a category at `id` without arming it (a tab click).
    ///
    /// The active tool is the tool that is **running**; browsing a category must not
    /// claim one, so `active` is left alone and only the category's memory moves.
    pub(crate) fn point(&mut self, category: &str, id: &str) {
        self.memory.insert(category.to_string(), id.to_string());
    }

    /// Open the flyout on `category`.
    pub(crate) fn open_tab(&mut self, category: &str) {
        self.tab = Some(category.to_string());
    }

    /// Close the flyout's tab (the popover closed).
    pub(crate) fn close_tab(&mut self) {
        self.tab = None;
    }
}

impl Shell {
    /// The creation pills, in bar order: `(category, tool)`, one per non-empty category.
    ///
    /// This is what both the strip and the key path read, so a pill's position is the
    /// same whether you click it or press its digit.
    pub(crate) fn creation_pills(&self) -> Vec<(String, crate::commands::CommandInfo)> {
        let model = self.model.lock().unwrap();
        crate::chrome::creation_pill_tools(&model.surfaces().creation, &self.creation)
            .into_iter()
            .map(|(category, command)| (category, command.clone()))
            .collect()
    }

    /// Arm the pill at `index` (`4`, `5`, …), or open its flyout when it is armed.
    ///
    /// Returns whether the key was the Add hand's. A second press while **that pill's**
    /// tool is the running one opens the flyout — "arm, then browse". A press while a
    /// different tool owns the canvas is not ours, so the digit still reaches that
    /// tool's own shortcuts and its numeric strip.
    pub(crate) fn activate_creation_slot(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some((category, command)) = self.creation_pills().into_iter().nth(index) else {
            return false;
        };
        if self.active_tool.is_some() && self.creation.active() == Some(command.id.as_str()) {
            self.open_creation_flyout(&category, cx);
            return true;
        }
        if self.active_tool.is_some() {
            return false;
        }
        if let Some(gate) = command.topology_gate() {
            let (count, kind) = {
                let model = self.model.lock().unwrap();
                crate::chrome::selection_topology(&model.selection)
            };
            if !gate.satisfied(count, kind) {
                let label = command.label().to_string();
                self.report_tool_requirement(&label, &gate.requirement(), cx);
                return true;
            }
        }
        self.run_command(&command.id, cx);
        true
    }

    /// Open the flyout on `category` (a pill's caret, or a second key press).
    pub(crate) fn open_creation_flyout(&mut self, category: &str, cx: &mut Context<Self>) {
        self.creation.open_tab(category);
        self.open_popover("create", cx);
    }

    /// Click a category tab: show that category's tools, and point its pill at the tool
    /// the category remembers.
    ///
    /// The pill behind the flyout therefore already names what the next click would arm,
    /// which is what makes browsing a category feel like loading the strip.
    pub(crate) fn set_creation_tab(&mut self, category: &str, cx: &mut Context<Self>) {
        self.creation.open_tab(category);
        let tool = {
            let model = self.model.lock().unwrap();
            let surfaces = model.surfaces();
            let mut tools = surfaces
                .creation
                .iter()
                .copied()
                .filter(|command| !command.dev_only)
                .filter(|command| {
                    command
                        .category()
                        .is_some_and(|name| name.eq_ignore_ascii_case(category))
                });
            self.creation
                .memory(category)
                .and_then(|id| tools.clone().find(|command| command.id == id))
                .or_else(|| tools.next())
                .map(|command| command.id.clone())
        };
        if let Some(id) = tool {
            self.creation.point(category, &id);
        }
        cx.notify();
    }

    /// Show or hide the create dock (the strip of pills above the action dock).
    ///
    /// Hiding it closes the flyout too: the flyout hangs from a pill that is no longer
    /// on screen.
    pub(crate) fn toggle_create_dock(&mut self, cx: &mut Context<Self>) {
        let expanded = self.creation.toggle_expanded();
        if !expanded {
            self.close_popover(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_fills_its_category_and_that_is_the_armed_tool() {
        let mut dock = CreationDock::default();
        dock.remember("SimpleCAD_Rectangle", Some("Sketch"));
        dock.remember("Part_Box", Some("Solid"));
        assert_eq!(dock.memory("Sketch"), Some("SimpleCAD_Rectangle"));
        assert_eq!(dock.memory("Solid"), Some("Part_Box"));
        assert_eq!(dock.active(), Some("Part_Box"), "the last pick is the armed one");

        // A Surface tool is its own pill: picking it leaves Solid's memory alone.
        dock.remember("SimpleCAD_Loft", Some("Surface"));
        assert_eq!(dock.memory("Solid"), Some("Part_Box"));
        assert_eq!(dock.memory("Surface"), Some("SimpleCAD_Loft"));
        assert_eq!(dock.active(), Some("SimpleCAD_Loft"));
    }

    #[test]
    fn a_tab_click_points_the_pill_without_arming_anything() {
        let mut dock = CreationDock::default();
        dock.remember("Part_Box", Some("Solid"));
        dock.open_tab("Surface");
        dock.point("Surface", "SimpleCAD_Loft");
        assert_eq!(dock.tab(), Some("Surface"));
        assert_eq!(dock.memory("Surface"), Some("SimpleCAD_Loft"));
        assert_eq!(dock.active(), Some("Part_Box"), "browsing arms nothing");

        dock.close_tab();
        assert_eq!(dock.tab(), None);
    }

    /// The strip is view state: the dock's `⊕ Create Dock` toggle flips it, and hiding
    /// it closes the flyout (which hangs from a pill that is no longer on screen).
    #[test]
    fn the_strip_is_shown_by_default_and_toggles() {
        let mut dock = CreationDock::default();
        assert!(dock.expanded(), "a fresh session shows the strip");
        dock.open_tab("Sketch");
        assert!(!dock.toggle_expanded());
        assert_eq!(dock.tab(), None, "hiding the strip closes its flyout");
        assert!(dock.toggle_expanded());
        assert_eq!(dock.tab(), None, "showing it again starts closed");
    }
}
