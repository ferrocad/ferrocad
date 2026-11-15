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
        // A hard bound may have clamped the value (a radius below the kernel floor, a
        // radius past the body's extent): show what the manipulator actually took, so the
        // field and the ghost cannot disagree. A value that was accepted as typed is left
        // alone — rewriting it would fight the caret mid-word (`7.5` → `7.50`).
        if let Some(fields) = viewport.read(cx).manipulator_fields(&id) {
            for ((input, field), typed) in form.inputs.iter().zip(fields.iter()).zip(values.iter()) {
                if (field.value - typed).abs() > 1e-9 {
                    let text = field_text(field);
                    input.update(cx, |input, cx| input.set_text(text, cx));
                }
            }
        }
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

    /// The primary selection's object and element, which is what a probe is about.
    ///
    /// The manipulator knows only the stage entity; the document half knows only names,
    /// so this is the one place the two meet. A whole-body selection has no element, and
    /// the probe is about the object's own shape.
    pub(in crate::shell) fn armed_element(&self) -> Option<(String, String)> {
        let model = self.model.lock().unwrap();
        let primary = model.selection.last()?;
        Some((
            primary.object.clone(),
            primary.elements.first().cloned().unwrap_or_default(),
        ))
    }

    /// Ask the document half about the armed manipulator, at most once per key.
    ///
    /// Two answers arrive together: the **seed** (the value an existing feature already
    /// holds) and the **range** (the values the kernel completes). The stage's own range
    /// is already on the fields; this is the sharper answer. The request is **deferred**
    /// one executor tick and a short delay, so a burst of armings (a hand switch, a
    /// re-selection, a commit) collapses into one kernel probe, and the paint that armed
    /// the handle never pays for it. The probe itself runs on the UI thread — it calls
    /// into the document half, which is not safe to enter from a worker — and the answer
    /// is cached until the shape changes.
    pub(in crate::shell) fn sync_manipulator_probe(&mut self, cx: &mut Context<Self>) {
        let Some((id, _)) = self.armed_numeric_fields(cx) else {
            return;
        };
        let Some((object, element)) = self.armed_element() else {
            return;
        };
        let key = (object, element, id);
        if self.probe_cache.contains_key(&key) {
            if let Some(probe) = self.probe_cache.get(&key).cloned().flatten() {
                self.apply_manipulator_probe(&probe, cx);
            }
            return;
        }
        if self.probe_pending.as_ref() == Some(&key) {
            return;
        }
        self.probe_pending = Some(key);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |shell, cx| {
            executor
                .timer(std::time::Duration::from_millis(120))
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.resolve_manipulator_probe(cx);
            });
        })
        .detach();
    }

    /// Run the probe the pending request names (the deferred task's body).
    ///
    /// Separate from the scheduling so a test — or a host that wants the answer sooner —
    /// can run it directly. Returns whether a probe ran.
    pub(in crate::shell) fn resolve_manipulator_probe(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(key) = self.probe_pending.take() else {
            return false;
        };
        let (object, element, manipulator) = key.clone();
        let element = (!element.is_empty()).then_some(element.as_str());
        let answer = crate::app::manipulator_probe(cx, &object, element, &manipulator);
        let probe = match answer {
            Ok(probe) => probe,
            Err(error) => {
                // The probe failed (a broken provider). Cache the "no opinion" so it is
                // not retried every frame, but say why once in the console.
                self.console_line(format!("manipulator probe: {error}"), cx);
                None
            }
        };
        self.probe_cache.insert(key, probe.clone());
        if let Some(probe) = probe {
            self.apply_manipulator_probe(&probe, cx);
        }
        true
    }

    /// Apply a probe: seed the handle with an existing value, then clamp it to the range.
    ///
    /// The order matters. A seed only lands while the handle is at rest — a drag, a typed
    /// value or a held commit is the user's, and the probe must not overwrite it. The
    /// range is authoritative whenever it arrives. The answer is only for the handle it
    /// was asked about: a probe that lands after the user re-armed on another element is
    /// dropped.
    fn apply_manipulator_probe(
        &mut self,
        probe: &crate::types::ManipulatorProbe,
        cx: &mut Context<Self>,
    ) {
        let Some((id, _)) = self.armed_numeric_fields(cx) else {
            return;
        };
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        // The picked feature decides Slot 3's mode. Re-opening a chamfer while the bar
        // reads `3 Fillet` would rebuild a fillet over it, so the slot follows the
        // feature and the scrubber is re-armed in that mode. The probe is cached, so the
        // re-arm's own probe request is answered from the same entry and settles here.
        if let Some(kind) = ferrocad_stage::DressUpKind::parse(&probe.mode)
            && self.loadout.slot() == Slot::Fillet
            && self.loadout.dress_up() != kind
            && self.pending_interaction.is_none()
        {
            self.loadout.set_dress_up(kind);
            let status = format!(
                "tool: {}",
                Slot::Fillet.status_label(self.gizmo_mode, kind)
            );
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            self.rearm_selection(cx);
            return;
        }
        let mut changed = false;
        // A seed is the operation the selection already carries. It must never overwrite
        // an edit in progress: a held commit is the user's, not the probe's.
        if !probe.values.is_empty() && self.pending_interaction.is_none() {
            changed |= viewport.update(cx, |viewport, _cx| {
                viewport.seed_manipulator_values(&id, &probe.values)
            });
        }
        // The note is how "this edits what you already have" is said: the row prints it
        // muted after the value. Only a probe that carries a value has one.
        let note = (!probe.values.is_empty() && !probe.note.is_empty())
            .then(|| probe.note.clone());
        changed |= viewport.update(cx, |viewport, _cx| viewport.set_manipulator_note(&id, note));
        changed |= viewport.update(cx, |viewport, _cx| {
            viewport.set_manipulator_value_bounds(&id, probe.min, probe.max)
        });
        if !changed {
            return;
        }
        // A field whose text no longer matches the value (seeded, or clamped by the
        // range) is rewritten: the probe is authoritative, and the row must not disagree
        // with the ghost. A field the user is typing into keeps its text unless it moved.
        if let Some(fields) = viewport.read(cx).manipulator_fields(&id) {
            if let Some(form) = self.manipulator_form.as_ref() {
                for (input, field) in form.inputs.iter().zip(fields.iter()) {
                    let typed = input.read(cx).text().trim().parse::<f64>().ok();
                    let differs = typed.map_or(true, |typed| (typed - field.value).abs() > 1e-9);
                    if differs {
                        let text = field_text(field);
                        input.update(cx, |input, cx| input.set_text(text, cx));
                    }
                }
            }
        }
        // Only a value the user was already holding is refreshed. A seeded handle holds
        // nothing yet: `✓` must stay absent until the gesture (or a typed number) makes
        // an edit.
        if self.pending_interaction.is_some() {
            if let Some(commit) = viewport.read(cx).manipulator_commit(&id) {
                self.hold_interaction(commit, cx);
            } else {
                cx.notify();
            }
        } else {
            cx.notify();
        }
    }
}

/// The characters a numeric field accepts: a signed decimal, like the Move fields.
fn is_number_char(character: char) -> bool {
    character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | ',')
}
