//! `Shell` methods: the completion row's numeric fields (C2).
//!
//! An armed creation tool publishes its parameters on [`ToolStatus::fields`]; once the
//! tool has **begun** (a click placed its anchor) the completion row draws them as the
//! same boxed fields the transform forms use, and this module owns the small editing
//! machine behind it:
//!
//! ```text
//! Tab / Shift+Tab   focus the next / previous field (the tool's own values seed it)
//! a digit, `-`, `.` focus the first field and start replacing its text
//! Backspace         delete a character, ending the edit when the text empties
//! Enter             commit the strip's edit, then finish the tool (one object)
//! Esc               drop the edit first; a second Esc cancels the tool
//! ```
//!
//! The values live **on the tool**: every keystroke is written back through
//! [`Tool::set_field`], so the preview resizes as the user types and the commit reads the
//! same numbers the strip showed. Nothing here holds a second copy of a value.

use crate::shell::*;

/// The field the strip is editing: which one, and the text typed so far.
///
/// The text starts as the field's current value and is replaced wholesale on the first
/// typed character (the design's "a digit replaces the value" rule), so a user who wants
/// `30` never has to erase `20.00` first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolFieldEdit {
    /// The index into [`ToolStatus::fields`].
    pub index: usize,
    /// The text being typed.
    pub text: String,
    /// Whether the next character starts a fresh value.
    pub replace: bool,
}

impl Shell {
    /// The fields the completion row is **showing**, if it is showing at all.
    ///
    /// Empty before the first click: a tool that has placed nothing has no row on screen
    /// (see `chrome::tool_has_begun`), and a field nobody can see must not take the
    /// keyboard — the digits belong to the loadout until the row exists.
    pub(crate) fn tool_fields(&self) -> Vec<ferrocad_stage::ToolField> {
        self.model
            .lock()
            .unwrap()
            .tool
            .as_ref()
            .filter(|status| status.active && crate::chrome::tool_has_begun(status))
            .map(|status| status.fields.clone())
            .unwrap_or_default()
    }

    /// The field being edited, for the strip to paint.
    pub(crate) fn tool_field_edit(&self) -> Option<&ToolFieldEdit> {
        self.tool_field.as_ref()
    }

    /// Whether the completion row is on screen with fields to edit (so `Tab` belongs to
    /// it).
    pub(crate) fn has_tool_fields(&self) -> bool {
        !self.tool_fields().is_empty()
    }

    /// Focus the next (or previous) field, wrapping.
    ///
    /// Returns false when the armed tool publishes no fields, so `Tab` falls through to
    /// whatever else owns it (the Move HUD, a dialog).
    pub(crate) fn cycle_tool_field(&mut self, backwards: bool, cx: &mut Context<Self>) -> bool {
        let fields = self.tool_fields();
        if fields.is_empty() {
            return false;
        }
        let next = match self.tool_field.take() {
            None => {
                if backwards {
                    fields.len() - 1
                } else {
                    0
                }
            }
            Some(edit) if backwards => (edit.index + fields.len() - 1) % fields.len(),
            Some(edit) => (edit.index + 1) % fields.len(),
        };
        self.tool_field = Some(ToolFieldEdit {
            index: next,
            text: fields[next].value.clone(),
            replace: true,
        });
        cx.notify();
        true
    }

    /// Focus one field by index (a click on its box), seeded with its current value.
    ///
    /// The row is driven by the shell's edit state rather than a `TextInput`, so a click
    /// only has to name the field: the root key path then types into it exactly as `Tab`
    /// does.
    pub(crate) fn focus_tool_field(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let fields = self.tool_fields();
        let Some(field) = fields.get(index) else {
            return false;
        };
        self.tool_field = Some(ToolFieldEdit {
            index,
            text: field.value.clone(),
            replace: true,
        });
        cx.notify();
        true
    }

    /// Start editing the first field with `ch`, or append it to the focused one.
    ///
    /// Returns whether a field consumed the character.
    pub(crate) fn type_into_tool_field(&mut self, ch: char, cx: &mut Context<Self>) -> bool {
        if !(ch.is_ascii_digit() || ch == '.' || ch == '-') {
            return false;
        }
        let fields = self.tool_fields();
        if fields.is_empty() {
            return false;
        }
        let mut edit = match self.tool_field.take() {
            Some(edit) => edit,
            None => ToolFieldEdit {
                index: 0,
                text: fields[0].value.clone(),
                replace: true,
            },
        };
        if edit.replace {
            edit.text.clear();
            edit.replace = false;
        }
        edit.text.push(ch);
        let index = edit.index;
        self.tool_field = Some(edit.clone());
        self.write_tool_field(index, &edit.text, cx);
        true
    }

    /// Delete the last character of the focused field (and end the edit when empty).
    pub(crate) fn backspace_tool_field(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mut edit) = self.tool_field.take() else {
            return false;
        };
        edit.text.pop();
        edit.replace = false;
        let index = edit.index;
        let text = edit.text.clone();
        if text.is_empty() {
            cx.notify();
            return true;
        }
        self.tool_field = Some(edit);
        self.write_tool_field(index, &text, cx);
        true
    }

    /// Drop the edit (the inner `Esc`), leaving the tool and its value alone.
    pub(crate) fn clear_tool_field(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tool_field.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// Write `text` into the tool's field `index`.
    ///
    /// A refusal is not an error: the tool keeps its value and the strip keeps the typed
    /// text, so the user can keep typing (`2.` parses as nothing yet). When the tool
    /// accepts, its new status replaces the strip's numbers — the preview and the row
    /// cannot disagree.
    fn write_tool_field(&mut self, index: usize, text: &str, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let id = self
            .tool_fields()
            .get(index)
            .map(|field| field.id.clone())
            .unwrap_or_default();
        if id.is_empty() {
            return;
        }
        viewport.update(cx, |viewport, _cx| {
            viewport.set_tool_field(&id, text);
        });
        // The viewport queued the tool's fresh status; take it now so the strip's
        // values are the tool's, not the typed text's.
        if let Some(status) = viewport.update(cx, |viewport, _cx| viewport.take_tool_state()) {
            self.model.lock().unwrap().tool = Some(status);
        }
        cx.notify();
    }
}
