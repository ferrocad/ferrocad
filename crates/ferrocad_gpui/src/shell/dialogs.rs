//! `Shell` methods: dialogs.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Open a command's dialog, seeding one control per field.
    pub(super) fn open_dialog_state(&mut self, token: String, spec: DialogSpec, cx: &mut Context<Self>) {
        // A wizard or a rerun replaces the previous dialog: drop any preview it left.
        self.clear_dialog_preview(cx);
        // A new HUD starts free: the previous freeze belonged to the old anchor.
        self.hud_rect = None;
        self.hud_lock = None;
        // A dialog outranks the transient wheel.
        let _ = self.close_radial(cx);
        let previews_translation = spec.previews_translation();
        let mut states = Vec::with_capacity(spec.fields.len());
        let mut inputs = Vec::with_capacity(spec.fields.len());
        let mut submits = Vec::new();
        let mut changes = Vec::new();
        for field in &spec.fields {
            states.push(ValueState::for_field(field));
            if field.kind().is_textual() {
                let seed = field.seed_text();
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(cx, "");
                    input.set_text(seed, cx);
                    input
                });
                // The seed is selected so the first keystroke replaces it (`0.0` + `15`
                // must be `15`, not `0.015`).
                input.update(cx, |input, cx| input.select_all(cx));
                // Enter in a field accepts: the same path as the button.
                let submit = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &SubmitEvent, cx| {
                        this.accept_dialog(cx);
                    },
                );
                // Every edit drives the spec's live preview (a no-op unless it asks
                // for one).
                let change = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                        this.preview_dialog(cx);
                    },
                );
                inputs.push(Some(input));
                submits.push(submit);
                changes.push(change);
            } else {
                inputs.push(None);
            }
        }
        self.dialog = Some(OpenDialog {
            token,
            spec,
            states,
            inputs,
            error: None,
            _submits: submits,
            _changes: changes,
        });
        self.focus_dialog = true;
        if previews_translation {
            // A gizmo drag now reports previews instead of committing: the fields and
            // the handles describe one pending move until the dialog accepts.
            self.defer_gizmo_preview(true, cx);
        }
        cx.notify();
    }

    /// The collected state of every field; the text inputs are authoritative.
    pub(super) fn dialog_states(&self, cx: &App) -> Vec<ValueState> {
        let Some(dialog) = self.dialog.as_ref() else {
            return Vec::new();
        };
        dialog
            .spec
            .fields
            .iter()
            .enumerate()
            .map(
                |(index, field)| match dialog.inputs.get(index).and_then(Option::as_ref) {
                    Some(input) => ValueState::Text(input.read(cx).text().to_string()),
                    None => dialog
                        .states
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| ValueState::for_field(field)),
                },
            )
            .collect()
    }

    /// Drive a spec's live preview from the current field text.
    ///
    /// `"translation"` composes a temporary transform on the selected entities; the
    /// document is untouched, so `Esc` is a true cancel and `Enter` is still the only
    /// commit. Text that does not parse yet previews as zero rather than flashing an
    /// error mid-keystroke.
    pub(super) fn preview_dialog(&mut self, cx: &mut Context<Self>) {
        let delta = {
            let Some(dialog) = self.dialog.as_ref() else {
                return;
            };
            if !dialog.spec.previews_translation() {
                return;
            }
            let states = self.dialog_states(cx);
            let Some(delta) = dialog::translation_preview(&dialog.spec, &states) else {
                return;
            };
            delta
        };
        let entities: Vec<u32> = {
            let model = self.model.lock().unwrap();
            model
                .selection
                .iter()
                .filter_map(|item| self.display.entity_of(&item.object))
                .collect()
        };
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.preview_translation(&entities, delta) {
                cx.notify();
            }
        });
        // The gizmo (and therefore the HUD anchor) follows the previewed object, so
        // re-render the shell to keep the panel beside it.
        cx.notify();
    }

    /// Drop a typed preview without changing the document (cancel or dismiss).
    pub(super) fn clear_dialog_preview(&mut self, cx: &mut Context<Self>) {
        self.drag_baseline = None;
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            viewport.set_gizmo_preview(false);
            if viewport.preview_translation(&[], [0.0; 3]) {
                cx.notify();
            }
        });
    }

    /// Enable or disable the viewport's deferred gizmo commit.
    pub(super) fn defer_gizmo_preview(&mut self, deferred: bool, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, _cx| viewport.set_gizmo_preview(deferred));
    }

    /// The Δ values currently in the fields, when the spec previews a translation.
    pub(super) fn translation_in_fields(&self, cx: &App) -> Option<[f64; 3]> {
        let dialog = self.dialog.as_ref()?;
        if !dialog.spec.previews_translation() {
            return None;
        }
        let states = self.dialog_states(cx);
        dialog::translation_preview(&dialog.spec, &states)
    }

    /// Write a delta into the `dx`/`dy`/`dz` inputs.
    ///
    /// `TextInput::set_text` is programmatic, so this does **not** emit a change event:
    /// mirroring a drag back into the fields cannot loop into another preview.
    pub(super) fn set_translation_fields(&mut self, delta: [f64; 3], cx: &mut Context<Self>) {
        let inputs: Vec<(usize, Entity<TextInput>)> = {
            let Some(dialog) = self.dialog.as_ref() else {
                return;
            };
            dialog
                .spec
                .fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| {
                    let slot = match field.name.as_str() {
                        "dx" => 0,
                        "dy" => 1,
                        "dz" => 2,
                        _ => return None,
                    };
                    let input = dialog.inputs.get(index).and_then(Option::as_ref)?.clone();
                    Some((slot, input))
                })
                .collect()
        };
        for (slot, input) in inputs {
            let text = format!("{:.2}", delta[slot]);
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    /// A live gizmo drag while a translation HUD is open: mirror the accumulated delta
    /// into the Δ fields and keep the whole selection previewing. Nothing reaches the
    /// document, so the drag and the typed values describe the same pending move.
    pub(super) fn on_transform_preview(&mut self, preview: &TransformPreview, cx: &mut Context<Self>) {
        if !self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
        {
            return;
        }
        if preview.phase == PreviewPhase::Begin {
            // A drag reports its transform relative to its own start; the fields keep
            // whatever they held, so successive drags accumulate into the same move.
            self.drag_baseline = self.translation_in_fields(cx);
        }
        let baseline = self.drag_baseline.unwrap_or([0.0; 3]);
        let m = &preview.total_transform.m;
        let delta = [baseline[0] + m[3], baseline[1] + m[7], baseline[2] + m[11]];
        self.set_translation_fields(delta, cx);
        self.preview_dialog(cx);
        if preview.phase == PreviewPhase::End {
            self.drag_baseline = None;
        }
    }

    /// Where a floating dialog should sit: beside the gizmo, else under the live
    /// pointer. The gizmo wins when it is visible, so the HUD hangs off the handles
    /// the user is about to drag; otherwise it opens where they are looking.
    pub(super) fn dialog_anchor(&self, window: &Window, cx: &App) -> Point<Pixels> {
        self.viewport
            .as_ref()
            .and_then(|viewport| viewport.read(cx).gizmo_anchor())
            .unwrap_or_else(|| window.mouse_position())
    }

    /// Move focus between a dialog's textual fields. `Tab` cycles forward; `Shift+Tab`
    /// back. Fields without an input (bool/enum) are not stops.
    pub(super) fn cycle_dialog_focus(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        let handles: Vec<FocusHandle> = dialog
            .inputs
            .iter()
            .flatten()
            .map(|input| input.read(cx).focus_handle(cx))
            .collect();
        if handles.is_empty() {
            return;
        }
        let current = handles.iter().position(|handle| handle.is_focused(window));
        let next = match (current, backward) {
            (None, false) => 0,
            (None, true) => handles.len() - 1,
            (Some(index), false) => (index + 1) % handles.len(),
            (Some(index), true) => (index + handles.len() - 1) % handles.len(),
        };
        window.focus(&handles[next], cx);
    }

    /// Validate and hand the dialog's values to the facade. Validation errors stay
    /// in the modal; a callback error is reported there too.
    pub(super) fn accept_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        let token = dialog.token.clone();
        let spec = dialog.spec.clone();
        let states = self.dialog_states(cx);
        match dialog::collect(&spec, &states) {
            Err(error) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.error = Some(error.clone());
                }
                self.model.lock().unwrap().status = format!("dialog: {error}");
                cx.notify();
            }
            Ok(values) => {
                let outcome = commands::accept(&token, &values);
                self.finish_dialog(outcome, cx);
            }
        }
    }

    /// Finish an accept: close, replace with a new dialog (a wizard), or keep the
    /// modal open with the callback's error.
    pub(super) fn finish_dialog(
        &mut self,
        result: Result<commands::CommandOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(outcome) if outcome.ok => {
                // The pending move is now applied (or replaced by a wizard): release
                // the deferred gizmo and drop the ghost before the stage rebuild.
                self.clear_dialog_preview(cx);
                if let (Some(spec), Some(token)) = (outcome.dialog.clone(), outcome.token.clone()) {
                    self.open_dialog_state(token, spec, cx);
                } else {
                    self.dialog = None;
                }
                // A dialog callback may return a presentation request (G2, the grid
                // settings dialog); apply it exactly as a plain command's does.
                let status = if let Some(request) = &outcome.view {
                    match self.apply_view(request, cx) {
                        Ok(summary) => summary,
                        Err(error) => format!("view: {error}"),
                    }
                } else if outcome.status.is_empty() {
                    "ok".to_string()
                } else {
                    outcome.status
                };
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                self.refresh(cx);
                self.sync_stage(cx);
                self.rearm_selection(cx);
                // A dialog callback is how the A1 fork triggers the refresh: it has
                // already registered or removed the overlay through the bridge.
                self.sync_overlays(cx);
            }
            Ok(outcome) => {
                let error = if outcome.error.is_empty() {
                    "the command failed".to_string()
                } else {
                    outcome.error
                };
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.error = Some(error.clone());
                }
                self.model.lock().unwrap().status = error;
            }
            Err(error) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.error = Some(error.clone());
                }
                self.model.lock().unwrap().status = error;
            }
        }
        cx.notify();
    }

    /// Cancel the open dialog and drop its token (the callback is never run).
    pub(super) fn cancel_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let _ = commands::cancel(&dialog.token);
        self.clear_dialog_preview(cx);
        self.model.lock().unwrap().status = "dialog cancelled".to_string();
        cx.notify();
    }

    /// Whether a modal currently owns the shell: a command dialog, the palette or
    /// a rendered form. At most one of them may be present, so a second backdrop
    /// can never be stacked on the first.
    pub(super) fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.palette.is_some() || self.form_dialog.is_some()
    }

}
