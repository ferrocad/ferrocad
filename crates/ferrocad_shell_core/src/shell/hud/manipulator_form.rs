//! The manipulator's numeric twin: a docked field row for a pushed face or a scrubbable
//! edge.
//!
//! [`crate::shell::move_hud`] owns the three-field Move form; this owns the **one** row a
//! workbench manipulator publishes. The manipulator describes its own numbers
//! ([`Manipulator::numeric_fields`]), so this surface names none of them: the push/pull
//! stem shows one signed distance, the dress-up arc one radius (a chamfer, two
//! distances). Typing writes back through [`Manipulator::set_values`], which clamps,
//! snaps and rebuilds the ghost, and the held commit is re-read from
//! [`Manipulator::pending_commit`] — so a typed value and a drag are the *same* pending
//! edit and `✓` applies either one.
//!
//! The row is a view, not an owner: it is rebuilt when the armed manipulator (or the
//! shape of its value) changes, and it never holds a number of its own. `Tab`, a
//! non-slot digit, `-` or `.` opens it; `Enter` commits and `Esc` rolls back, exactly as
//! the in-context pair does.

use crate::shell::*;

/// How many fields one manipulator may publish. A fillet has one and a chamfer two; the
/// cap only stops a mis-declared mod from growing the dock without bound.
pub(in crate::shell) const MANIPULATOR_FIELDS: usize = 4;

/// The docked numeric row for the armed manipulator.
pub(in crate::shell) struct ManipulatorForm {
    /// The manipulator's registry id (`push_pull`, `dress_up`).
    id: String,
    /// The labels the inputs were built for. A mode switch (a chamfer's second distance)
    /// changes them and rebuilds the row rather than reusing stale inputs and carets.
    labels: Vec<String>,
    /// One input per published field, in the manipulator's order.
    pub(in crate::shell) inputs: Vec<Entity<TextInput>>,
    /// Keeps the change and submit subscriptions alive for the row's lifetime.
    _subs: Vec<Subscription>,
}

/// The text a field shows: the manipulator's own precision.
fn field_text(field: &NumericField) -> String {
    format!("{:.*}", field.decimals, field.value)
}

impl Shell {
    /// Whether the docked row is on screen.
    pub(in crate::shell) fn manipulator_form_open(&self) -> bool {
        self.manipulator_form.is_some()
    }

    /// The armed manipulator's id and numeric fields, if it publishes any.
    pub(in crate::shell) fn armed_numeric_fields(
        &self,
        cx: &App,
    ) -> Option<(String, Vec<NumericField>)> {
        self.viewport.as_ref()?.read(cx).numeric_manipulator()
    }

    /// Create, keep or drop the row so it matches the armed manipulator.
    ///
    /// Called from the render path: the viewport is the authority on which manipulators
    /// are armed, and it changes them from several places (a pick, a hand switch, a
    /// commit, a discard). Comparing the id and the field labels keeps the inputs — and
    /// the caret — across a re-arm of the same shape.
    pub(in crate::shell) fn ensure_manipulator_form(&mut self, cx: &mut Context<Self>) {
        let Some((id, fields)) = self.armed_numeric_fields(cx) else {
            self.manipulator_form = None;
            return;
        };
        let labels: Vec<String> = fields.iter().map(|field| field.label.clone()).collect();
        if self
            .manipulator_form
            .as_ref()
            .is_some_and(|form| form.id == id && form.labels == labels)
        {
            return;
        }
        let mut subs = Vec::new();
        let inputs = fields
            .iter()
            .take(MANIPULATOR_FIELDS)
            .map(|field| {
                let text = field_text(field);
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(cx, "");
                    input.set_text(text, cx);
                    input.set_char_filter(Some(is_number_char));
                    input
                });
                // Every edit is the manipulator's new value, and therefore the pending
                // commit: the field and the drag write the same place.
                subs.push(cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                        this.on_manipulator_typed(cx);
                    },
                ));
                subs.push(cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &SubmitEvent, cx| {
                        this.commit_pending_interaction(cx);
                    },
                ));
                input
            })
            .collect();
        self.manipulator_form = Some(ManipulatorForm {
            id,
            labels,
            inputs,
            _subs: subs,
        });
    }

    /// Mirror the manipulator's value into the row's unfocused fields.
    ///
    /// A drag updates the value with no shell event, so the row follows here; a focused
    /// field is skipped so typing is never rewritten under the caret.
    pub(in crate::shell) fn sync_manipulator_form(&self, window: &Window, cx: &mut Context<Self>) {
        let Some((_, fields)) = self.armed_numeric_fields(cx) else {
            return;
        };
        let Some(form) = self.manipulator_form.as_ref() else {
            return;
        };
        for (input, field) in form.inputs.iter().zip(fields.iter()) {
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            let text = field_text(field);
            if !focused && input.read(cx).text() != text {
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
    }

    /// A field changed: adopt the typed numbers as the manipulator's value.
    ///
    /// An unparsable field (an empty one mid-edit, a trailing `-`) keeps the last
    /// preview, like the Move form. A value that is no edit at all — a stem typed back to
    /// zero — clears the held commit instead of asking the user to apply nothing.
    pub(in crate::shell) fn on_manipulator_typed(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.manipulator_form.as_ref() else {
            return;
        };
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let id = form.id.clone();
        let mut values = Vec::with_capacity(form.inputs.len());
        for input in &form.inputs {
            match input.read(cx).text().trim().parse::<f64>() {
                Ok(value) if value.is_finite() => values.push(value),
                _ => return,
            }
        }
        if values.is_empty() {
            return;
        }
        viewport.update(cx, |viewport, _cx| {
            viewport.set_manipulator_values(&id, &values)
        });
        let commit = viewport.read(cx).manipulator_commit(&id);
        match commit {
            Some(commit) => self.hold_interaction(commit, cx),
            None => self.clear_pending_interaction(cx),
        }
    }

    /// `Tab` / a digit: put the caret in a field, replacing its text.
    pub(in crate::shell) fn focus_manipulator_field(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .manipulator_form
            .as_ref()
            .and_then(|form| form.inputs.get(index))
        else {
            return;
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        input.update(cx, |input, cx| input.select_all(cx));
    }

    /// `Tab`/`Shift+Tab` inside the row: move to the next field.
    pub(in crate::shell) fn cycle_manipulator_focus(
        &self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.manipulator_form.as_ref() else {
            return;
        };
        if form.inputs.is_empty() {
            return;
        }
        let current = form
            .inputs
            .iter()
            .position(|input| input.read(cx).focus_handle(cx).is_focused(window));
        let next = match (current, backward) {
            (None, false) => 0,
            (None, true) => form.inputs.len() - 1,
            (Some(index), false) => (index + 1) % form.inputs.len(),
            (Some(index), true) => (index + form.inputs.len() - 1) % form.inputs.len(),
        };
        self.focus_manipulator_field(next, window, cx);
    }

    /// A digit pressed on the canvas: open the row with that number.
    pub(in crate::shell) fn type_into_manipulator_field(
        &mut self,
        typed: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = self
            .manipulator_form
            .as_ref()
            .and_then(|form| form.inputs.first())
        else {
            return false;
        };
        input.update(cx, |input, cx| input.set_text(typed.to_string(), cx));
        // `set_text` is programmatic (no change event), so drive the value here.
        self.on_manipulator_typed(cx);
        self.focus_manipulator_field(0, window, cx);
        cx.notify();
        true
    }
}

/// The characters a numeric field accepts: a signed decimal, like the Move fields.
fn is_number_char(character: char) -> bool {
    character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | ',')
}
