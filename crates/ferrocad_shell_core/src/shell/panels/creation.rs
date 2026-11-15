//! The Add hand: two fixed creation slots and the flyout's category tab.
//!
//! The action dock used to carry **one** memory pill (`⊕ <LastTool> ▾`) and a flat
//! flyout that mixed every tool an edition registered — 2-D drawing tools, primitives
//! and profile features in one column. The Add hand is two hands' worth of work, so it
//! is two slots:
//!
//! - **Slot `4`** — the **Sketch** family: the tools that draw on a plane;
//! - **Slot `5`** — **Solid/Surface**: everything that makes or skins a solid, plus any
//!   other category an edition invents (Text, Mesh), so a mod's tool always has a home.
//!
//! Each slot remembers the tool it was left on, and the flyout shows one category at a
//! time behind a segmented header (`Sketch | Solid | Surface | …`, empty categories
//! omitted). Picking a tab points that family's slot at the tool the category
//! remembers, so the pill behind the flyout already shows what the next click would
//! arm. Picking a tool runs it: the slot takes it, the pill's label follows, and the
//! tool arms at the pointer — the same two actions a wheel flick performs.
//!
//! See `docs/developer/interaction/creation-loadout.md` §1.

use std::collections::HashMap;

use crate::shell::*;

/// How many creation slots the dock has.
pub(crate) const CREATION_SLOTS: usize = 2;
/// Slot `4`: the Sketch family.
pub(crate) const SKETCH_SLOT: usize = 0;
/// Slot `5`: Solid/Surface, and every other category an edition invents.
pub(crate) const SOLID_SLOT: usize = 1;
/// The key labels, in bar order.
pub(crate) const CREATION_KEYS: [&str; CREATION_SLOTS] = ["4", "5"];

/// The slot a tool's category belongs to.
///
/// Sketch is its own hand (2-D work); Solid, Surface, Text, Mesh and anything a mod
/// invents share the other one, so the dock never grows a pill per category.
pub(crate) fn slot_of_category(category: Option<&str>) -> usize {
    match category {
        Some(name) if name.eq_ignore_ascii_case("Sketch") => SKETCH_SLOT,
        _ => SOLID_SLOT,
    }
}

/// The Add hand's memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CreationDock {
    /// The tool each slot holds, by command id.
    slots: [Option<String>; CREATION_SLOTS],
    /// The tool last used in each category, so a tab remembers where it was.
    memory: HashMap<String, String>,
    /// The tool last armed overall: the pill's accent, `PILL_HOLD`, and the `4`/`5`
    /// re-press (a second press opens the flyout instead of re-arming).
    active: Option<String>,
    /// The flyout's category tab while it is open.
    tab: Option<String>,
}

impl CreationDock {
    /// The tool a slot holds.
    pub(crate) fn slot(&self, index: usize) -> Option<&str> {
        self.slots.get(index).and_then(Option::as_deref)
    }

    /// The tool last armed.
    pub(crate) fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The tool a category remembers.
    pub(crate) fn memory(&self, category: &str) -> Option<&str> {
        self.memory.get(category).map(String::as_str)
    }

    /// The flyout's open tab.
    pub(crate) fn tab(&self) -> Option<&str> {
        self.tab.as_deref()
    }

    /// Record that `id` (in `category`) is now the armed tool.
    ///
    /// This is the one write a tool pick needs: the slot takes it, the category
    /// remembers it, and the pill names it.
    pub(crate) fn remember(&mut self, id: &str, category: Option<&str>) {
        self.slots[slot_of_category(category)] = Some(id.to_string());
        if let Some(category) = category {
            self.memory.insert(category.to_string(), id.to_string());
        }
        self.active = Some(id.to_string());
    }

    /// Point a slot at `id` without arming it (a tab click).
    ///
    /// The active tool is the tool that is **running**; browsing a category must not
    /// claim one, so `active` is left alone and only the slot's memory moves.
    pub(crate) fn point(&mut self, index: usize, id: &str) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = Some(id.to_string());
        }
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
    /// The command a creation slot currently offers.
    ///
    /// The slot's memory wins while it still names a live tool of that family; otherwise
    /// the family's first tool in registry order, so a fresh session's pill is useful.
    fn creation_slot_tool(&self, index: usize) -> Option<crate::commands::CommandInfo> {
        let model = self.model.lock().unwrap();
        crate::chrome::creation_slot_command(&model.surfaces().creation, &self.creation, index)
            .cloned()
    }

    /// `4`/`5` (or the pill): arm the slot's tool, or open its category list.
    ///
    /// Returns whether the key was the Add hand's. A second press while **that slot's**
    /// tool is the running one opens the flyout — "arm, then browse". A press while a
    /// different tool owns the canvas is not ours, so the digit still reaches that
    /// tool's own shortcuts and its numeric strip.
    pub(crate) fn activate_creation_slot(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(command) = self.creation_slot_tool(index) else {
            return false;
        };
        if self.active_tool.is_some() && self.creation.active() == Some(command.id.as_str()) {
            self.open_creation_flyout(index, cx);
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

    /// Open the flyout at the slot's own category.
    pub(crate) fn open_creation_flyout(&mut self, index: usize, cx: &mut Context<Self>) {
        match self
            .creation_slot_tool(index)
            .and_then(|command| command.category().map(str::to_string))
        {
            Some(category) => self.creation.open_tab(&category),
            None => self.creation.close_tab(),
        }
        self.open_popover("create", cx);
    }

    /// Click a category tab: show that category's tools, and point its family's slot at
    /// the tool the category remembers.
    ///
    /// The pill behind the flyout therefore already names what the next click would arm,
    /// which is what makes browsing a category feel like loading the slot.
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
            self.creation.point(slot_of_category(Some(category)), &id);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sketch_is_its_own_slot_and_everything_else_shares_the_other() {
        assert_eq!(slot_of_category(Some("Sketch")), SKETCH_SLOT);
        assert_eq!(slot_of_category(Some("sketch")), SKETCH_SLOT);
        for category in [Some("Solid"), Some("Surface"), Some("Text"), Some("Mesh"), None] {
            assert_eq!(slot_of_category(category), SOLID_SLOT, "{category:?}");
        }
    }

    #[test]
    fn a_pick_fills_its_slot_and_its_category_remembers_it() {
        let mut dock = CreationDock::default();
        dock.remember("SimpleCAD_Rectangle", Some("Sketch"));
        dock.remember("Part_Box", Some("Solid"));
        assert_eq!(dock.slot(SKETCH_SLOT), Some("SimpleCAD_Rectangle"));
        assert_eq!(dock.slot(SOLID_SLOT), Some("Part_Box"));
        assert_eq!(dock.memory("Sketch"), Some("SimpleCAD_Rectangle"));
        assert_eq!(dock.active(), Some("Part_Box"), "the last pick is the armed one");

        // A Surface tool takes the second slot and keeps its own category's memory, so
        // switching back to Solid restores Box.
        dock.remember("SimpleCAD_Loft", Some("Surface"));
        assert_eq!(dock.slot(SOLID_SLOT), Some("SimpleCAD_Loft"));
        assert_eq!(dock.memory("Solid"), Some("Part_Box"));
        assert_eq!(dock.memory("Surface"), Some("SimpleCAD_Loft"));
    }

    #[test]
    fn a_tab_click_points_the_slot_without_arming_anything() {
        let mut dock = CreationDock::default();
        dock.remember("Part_Box", Some("Solid"));
        dock.open_tab("Surface");
        dock.point(SOLID_SLOT, "SimpleCAD_Loft");
        assert_eq!(dock.tab(), Some("Surface"));
        assert_eq!(dock.slot(SOLID_SLOT), Some("SimpleCAD_Loft"));
        assert_eq!(dock.active(), Some("Part_Box"), "browsing arms nothing");

        dock.close_tab();
        assert_eq!(dock.tab(), None);
    }
}
