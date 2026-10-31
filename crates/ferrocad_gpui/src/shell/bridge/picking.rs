//! `Shell` methods: picking and selection.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use crate::shell::*;
use crate::python::SelectionRequest;

/// The selection a click asks for.
///
/// A single click keeps the sub-element the picker resolved (`Face6`). A double
/// click — gpui reports `click_count >= 2` on the second press — upgrades that pick
/// to the whole body, so the triad names the body and Rotate pivots about its own
/// origin. The count travels with the [`SelectionEvent`], so the shell never guesses
/// a double click from time or distance.
pub(in crate::shell) fn click_target(reference: &SelectionRef, click_count: usize) -> SelectionRef {
    if click_count >= 2 {
        SelectionRef::whole(reference.object)
    } else {
        reference.clone()
    }
}

impl Shell {
    /// Replace the selection with one whole object (a plain click).
    pub(in crate::shell) fn select(&mut self, name: &str, cx: &mut Context<Self>) {
        self.replace_selection(vec![SelectionItem::new(name)], cx);
    }

    /// Add or remove one object (Ctrl/Cmd- or Shift-click), keeping the rest.
    pub(in crate::shell) fn toggle_select(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut items = {
            let m = self.model.lock().unwrap();
            m.selection.clone()
        };
        if let Some(index) = items.iter().position(|item| item.is(name)) {
            items.remove(index);
        } else {
            items.push(SelectionItem::new(name));
        }
        self.replace_selection(items, cx);
    }

    /// Clear the selection.
    pub(in crate::shell) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.replace_selection(Vec::new(), cx);
    }

    /// Replace the whole selection: re-seed the property editor from the primary
    /// (last-clicked) object, push the facade snapshot, and re-evaluate enablement.
    ///
    /// The push happens **before** the commands are listed, so `IsActive` sees the
    /// selection the user just made.
    pub(in crate::shell) fn replace_selection(&mut self, items: Vec<SelectionItem>, cx: &mut Context<Self>) {
        // The topology filter is applied where the shell **reports** the selection, so
        // the tree, the facade and the arm seam all see the same scope: `Select: Body`
        // rewrites a `Face6` pick to its whole body, `Auto`/`Face`/`Edge` keep the
        // element. The pick itself still reports the sub-element (hover and snapping
        // need it); only the reported selection is narrowed.
        let filter = self.loadout.filter();
        let items: Vec<SelectionItem> = items
            .into_iter()
            .map(|item| filter.apply_item(item))
            .collect();
        // A paused Move hand is about *this* subject: a new selection arms it again.
        let changed = {
            let model = self.model.lock().unwrap();
            model.selection != items
        };
        // Re-applying the same selection is a no-op, and it must not run the session reset
        // below: a pick on the already-selected body used to drop a pending move silently.
        if !changed {
            return;
        }
        // A pending move is explicit state: the user has `✗`/`✓` on screen and has not
        // decided yet, so another object may neither spend the move nor drop it. The
        // subject cannot change until the move is finished — the context gate owns that
        // rule, so every entry point agrees.
        let target = items
            .last()
            .and_then(|item| self.display.entity_of(&item.object));
        if self.gate(Action::Select { target }, cx) {
            return;
        }
        self.move_disarmed = false;
        // Opening menus never mutates the selection. A **new** selection, however,
        // invalidates both an open wheel (its context target no longer matches the
        // selection) and a pending translation form (its delta was measured against
        // the old selection), so both close before the new selection is reported.
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
        self.wheel_target = None;
        // A new selection is a new pivot subject: the session user origin goes away.
        // The loadout slot is a tool choice, not a selection property, so it stays;
        // a hover-to-arm preview is replaced by the latched selection's own handles.
        self.user_origin = None;
        self.hover_arm = None;
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
            && let Some(dialog) = self.dialog.take()
        {
            let _ = commands::cancel(&dialog.token);
            self.discard_pending(cx);
        }
        // A new selection is a new session. A dirty one was committed above; a clean one
        // still carries a datum and targets for the old objects, so it is dropped with its
        // tape rather than reused by the next drag.
        self.session = None;
        self.drag_start = None;
        self.rotation_start = None;
        self.sync_transform_tape(cx);
        let primary = items.last().map(|item| item.object.clone());
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        self.prop_inputs.clear();
        self.row_popup = None;
        let status = match (items.len(), primary.as_deref()) {
            (0, _) | (_, None) => "selection cleared".to_string(),
            (1, Some(name)) => format!("selected {name}"),
            (count, Some(name)) => format!("selected {count} · {name}"),
        };
        // A tree click on a settings node retargets the card. Selecting an object
        // does **not** close it: the settings panel is dismissed explicitly, so a
        // user can keep editing while an object stays selected (and its gizmo on).
        let settings_node = primary
            .as_deref()
            .filter(|name| name.starts_with("Settings/"))
            .map(str::to_string);
        {
            let mut m = self.model.lock().unwrap();
            m.selection = items;
            m.selected = primary;
            m.properties = properties;
            m.status = status;
        }
        if let Some(node) = &settings_node {
            self.open_settings_card(node, cx);
        }
        if let Err(error) = self.push_selection() {
            self.console_line(format!("selection: {error}"), cx);
        }
        self.refresh_commands(cx);
        // The canvas follows the selection: re-arm its handles, or clear them when the
        // selection is empty. Without this the previous target's triad stayed hosted and
        // kept consuming the hover, so the idle preview appeared only after a pointer
        // move (or never). With nothing selected, re-offer the preview for a body the
        // pointer is still over: it often has not moved since the clear.
        self.rearm_selection(cx);
        if self.model.lock().unwrap().selection.is_empty() {
            self.sync_hover_arm(cx);
        }
        cx.notify();
    }

    /// Push the shell's selection to `FreeCADGui.Selection` (the facade snapshot).
    pub(in crate::shell) fn push_selection(&self) -> Result<(), String> {
        let items = {
            let m = self.model.lock().unwrap();
            m.selection.clone()
        };
        selection::push(&items)
    }

    /// Apply the selection writes a script queued on `FreeCADGui.Selection`.
    ///
    /// This is the write half of S4.3 (the read half is [`Self::push_selection`]).
    /// The facade cannot command the viewport synchronously, so a script's
    /// `addSelection`/`removeSelection`/`clearSelection` records the operation and
    /// the shell applies it to **its own** selection — the same state a click
    /// produces. The selection is then pushed back down (so the facade snapshot and
    /// the shell agree, without a second request) and the primary item's workbench
    /// manipulators are re-armed, which is what makes a `Select Face6` from a script
    /// arm the Part push/pull stem exactly as a click does.
    ///
    /// A `preselect` record is session hover state, not a selection: the viewport
    /// owns hover, so it is dropped here (the facade keeps its own copy for
    /// `getPreselection`). The facade refuses an object the document does not have
    /// before it queues anything, so a record always names a live object; one deleted
    /// between the write and the drain is pruned by the next
    /// [`refresh`](Self::refresh).
    pub(in crate::shell) fn drain_selection_requests(&mut self, cx: &mut Context<Self>) {
        let requests = match python::take_selection_requests() {
            Ok(requests) => requests,
            Err(error) => {
                self.console_line(format!("selection: {error}"), cx);
                return;
            }
        };
        let mut items = {
            let m = self.model.lock().unwrap();
            m.selection.clone()
        };
        let before = items.clone();
        for request in requests {
            apply_selection_request(&mut items, request);
        }
        if items == before {
            return;
        }
        self.replace_selection(items, cx);
        self.rearm_selection(cx);
    }

    /// The status line's pass-through segment: the overlay the pointer would click
    /// through, plus the document object beneath it. Empty when there is none.
    pub(in crate::shell) fn hover_line(&self, cx: &App) -> String {
        let Some(viewport) = self.viewport.as_ref() else {
            return String::new();
        };
        // Every shell render publishes the camera: an orbit or a zoom moves it without
        // telling the shell, and the next render is the first point a Python call can
        // observe it. This is the freshness the view facade's getCameraNode relies on.
        self.publish_view(cx);
        let report = viewport.read(cx).hover_report();
        let target = report
            .target
            .as_ref()
            .and_then(|reference| self.display.object_of(reference.object));
        hover_status(report, target)
    }

    /// Re-read the tree and the selected object's properties after a change.
    ///
    /// Selection names the document no longer has (a delete, or an undo of a
    /// creation) are dropped here, and the pruned set is pushed to the facade so a
    /// command's `IsActive` cannot see a deleted object.
    pub(in crate::shell) fn refresh(&mut self, cx: &mut Context<Self>) {
        let tree = python::model_tree().unwrap_or_default();
        let live: HashSet<String> = tree
            .iter()
            .flat_map(|doc| doc.objects.iter().map(|obj| obj.name.clone()))
            .collect();
        let (next, primary, pruned) = {
            let m = self.model.lock().unwrap();
            let mut next = m.selection.clone();
            next.retain(|item| live_node(&item.object, &live));
            let pruned = next.len() != m.selection.len();
            let primary = next
                .last()
                .map(|item| item.object.clone())
                .or_else(|| m.selected.clone().filter(|name| live_node(name, &live)));
            (next, primary, pruned)
        };
        // A deleted selection cannot keep a pending form or an open context menu.
        if pruned {
            let wheel = self.wheel.clone();
            let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
            self.wheel_target = None;
            if self
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.spec.previews_translation())
                && let Some(dialog) = self.dialog.take()
            {
                let _ = commands::cancel(&dialog.token);
                self.discard_pending(cx);
            }
        }
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        {
            let mut m = self.model.lock().unwrap();
            m.tree = tree;
            m.selection = next;
            m.selected = primary;
            m.properties = properties;
        }
        if let Err(error) = self.push_selection() {
            self.console_line(format!("selection: {error}"), cx);
        }
        self.refresh_commands(cx);
        self.sync_overlays(cx);
        cx.notify();
    }

}

/// Apply one facade selection write to the shell's selection list.
///
/// The rules mirror `FreeCADGui.Selection`'s own: an `add` promotes the object to the
/// **primary** (last) slot, merges sub-elements instead of duplicating them, and a
/// whole-object add replaces a stale element list. A `remove` of the last sub-element
/// drops the object; `clear` empties everything. `preselect` is hover state and is
/// ignored here, and an unknown op is ignored so a newer facade cannot break an older
/// shell.
pub(in crate::shell) fn apply_selection_request(items: &mut Vec<SelectionItem>, request: SelectionRequest) {
    match request.op.as_str() {
        "add" => {
            if request.object.is_empty() {
                return;
            }
            let index = items.iter().position(|item| item.is(&request.object));
            let mut item = index
                .map(|index| items.remove(index))
                .unwrap_or_else(|| SelectionItem::new(request.object.clone()));
            if request.elements.is_empty() {
                item.elements.clear();
            } else {
                for element in request.elements {
                    if !item.elements.contains(&element) {
                        item.elements.push(element);
                    }
                }
            }
            items.push(item);
        }
        "remove" => {
            let Some(index) = items.iter().position(|item| item.is(&request.object)) else {
                return;
            };
            if request.elements.is_empty() {
                items.remove(index);
                return;
            }
            items[index]
                .elements
                .retain(|element| !request.elements.contains(element));
            if items[index].elements.is_empty() {
                items.remove(index);
            }
        }
        "clear" => items.clear(),
        // Hover (`preselect`) belongs to the viewport; an unknown op is a newer
        // facade's, and ignoring it keeps an older shell working.
        _ => {}
    }
}
