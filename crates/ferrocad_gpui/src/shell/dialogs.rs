//! `Shell` methods: dialogs.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

/// The Δ field names in X/Y/Z order.
const AXIS_FIELDS: [&str; 3] = ["dx", "dy", "dz"];

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
                // for one). A typed value **is** the move, so it replaces the exact
                // delta a drag left behind.
                let change = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                        this.pending_move.typed();
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
        let workplane_offset = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.drives_workplane_offset());
        if workplane_offset {
            // The typed half of the offset session: the field moves the plane (and its
            // stem) live, so typing and dragging describe the same pending offset.
            self.preview_workplane_offset(cx);
            return;
        }
        let delta = {
            let Some(dialog) = self.dialog.as_ref() else {
                return;
            };
            if !dialog.spec.previews_translation() {
                return;
            }
            // The exact delta, not the two-decimal text: rounding it here snapped the
            // object and the triad back to a nearby pose (to the origin for a drag
            // smaller than 0.005 mm).
            let Some(delta) = self.pending_translation(cx) else {
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
        // The session supplies the matrix; it is a translation today, but the input is the
        // one A1b made the single owner of item previews, so a rotation needs no second
        // path later. The endpoints are identical to the old `preview_translation` call.
        let transform = Matrix4::translation(delta[0], delta[1], delta[2]);
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.preview_transform(&entities, Some(transform)) {
                cx.notify();
            }
        });
        // The gizmo (and therefore the HUD anchor) follows the previewed object, so
        // re-render the shell to keep the panel beside it.
        cx.notify();
    }

    /// Drop the ghost and the session without changing the document.
    ///
    /// It deliberately leaves [`PendingMove`] alone: opening the HUD mid-drag calls this
    /// first, and the drag's value is what seeds the form. Discarding that value is an
    /// explicit act — [`discard_pending`](Self::discard_pending).
    pub(super) fn clear_dialog_preview(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.clear_preview_items(cx);
    }

    /// Drop the pending value entirely (cancel, accept, or a competing interaction).
    pub(super) fn discard_pending(&mut self, cx: &mut Context<Self>) {
        self.pending_move.reset();
        self.clear_dialog_preview(cx);
    }

    /// Clear every item's preview transform (and the overlay offsets that came with it).
    fn clear_preview_items(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.preview_transform(&[], None) {
                cx.notify();
            }
        });
    }

    /// The current drag translation to seed the Move HUD from.
    ///
    /// The viewport is the authority while it exists: the live readout names the
    /// constrained axis, and the draw-only offset is the accumulated translation the
    /// handles are already showing. Without one (headless, or before
    /// `enable_viewport`) the last mirrored preview is used; an idle gizmo seeds zero
    /// and only picks the focus from the handle under the pointer.
    pub(super) fn gizmo_seed(&self, cx: &App) -> GizmoSeed {
        let Some(viewport) = self.viewport.as_ref() else {
            return self.pending_move.seed();
        };
        let viewport = viewport.read(cx);
        let Some(gizmo) = viewport.gizmo() else {
            return self.pending_move.seed();
        };
        let mut seed = GizmoSeed::default();
        if let Some(value) = viewport.gizmo_value() {
            match value {
                GizmoValue::Linear { axis, applied, .. } => {
                    seed.axis = axis_index(axis);
                    seed.delta[seed.axis] = applied;
                }
                GizmoValue::Planar { applied, .. } => {
                    // A plane has two free axes; the HUD's convention is to focus X.
                    seed.axis = 0;
                    seed.delta = [applied.x, applied.y, applied.z];
                }
                GizmoValue::Angular { axis, .. } => {
                    // A ring drag has no translation to seed yet; the axis still
                    // decides which field takes focus.
                    seed.axis = axis_index(axis);
                }
            }
        }
        // The offset is the whole drag's preview translation (the readout's `applied`
        // is only the constrained component), so it wins when present.
        if let Some(offset) = gizmo.display_offset() {
            seed = GizmoSeed {
                axis: seed.axis,
                delta: [offset.x, offset.y, offset.z],
            };
        }
        // Idle: there is no tick, so the field follows the handle under the pointer.
        if viewport.gizmo_value().is_none()
            && let Some(handle) = gizmo.hovered()
        {
            seed.axis = handle_axis_index(handle);
        }
        seed
    }

    /// The input for the dialog field named `name`, when it has one.
    fn dialog_input(&self, name: &str) -> Option<Entity<TextInput>> {
        let dialog = self.dialog.as_ref()?;
        let index = dialog
            .spec
            .fields
            .iter()
            .position(|field| field.name == name)?;
        dialog.inputs.get(index).and_then(Option::as_ref).cloned()
    }

    /// Escalate an armed gizmo into the floating Move HUD.
    ///
    /// Reuses the wheel's `Move` verb (same token, mode switch and deferred session),
    /// seeds the Δ fields with `seed`, and focuses the field for the armed handle.
    /// `typed` is the numeric key that opened it: it replaces the freshly seeded value
    /// and previews immediately, like typing into a selected field.
    pub(super) fn escalate_to_move_hud(
        &mut self,
        seed: GizmoSeed,
        typed: Option<char>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The wheel's verb, so there is one HUD definition and one token protocol.
        self.run_command("App_Move", cx);
        let Some(dialog) = self.dialog.as_ref() else {
            return false;
        };
        if !dialog.spec.previews_translation() {
            return true;
        }
        // Deferral is the one mode and is set when the viewport is created, so an
        // escalation inherits it; nothing here has to switch it.
        self.set_translation_fields(seed.delta, cx);
        let name = AXIS_FIELDS[seed.axis.min(AXIS_FIELDS.len() - 1)];
        let Some(input) = self.dialog_input(name) else {
            return true;
        };
        // `open_dialog_state` asks the next render to focus the first field. The
        // escalation always opens by keyboard, so take that over now and focus the
        // field the armed handle names instead.
        self.focus_dialog = false;
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        if let Some(ch) = typed {
            input.update(cx, |input, cx| input.set_text(ch.to_string(), cx));
            // `set_text` is programmatic (no change event), so drive the preview here.
            self.preview_dialog(cx);
        } else {
            // Select the seed so the next keystroke replaces it (`0.00` + `5` must be
            // `5`, not `0.005`).
            input.update(cx, |input, cx| input.select_all(cx));
        }
        cx.notify();
        true
    }

    /// The pending translation a Move session will commit.
    ///
    /// The session is the authority when a drag set the value: it holds the exact delta and
    /// the datum it was measured from. [`PendingMove`] is the HUD's view and remains the
    /// fallback for a shell with no stage (headless tests, or a target whose item has not
    /// arrived yet), where a session cannot be opened; the Δ fields are authoritative only
    /// after a typed edit. `None` when the spec asks for no translation.
    pub(super) fn pending_translation(&self, cx: &App) -> Option<[f64; 3]> {
        let dialog = self.dialog.as_ref()?;
        if !dialog.spec.previews_translation() {
            return None;
        }
        if self.pending_move.is_dragged()
            && let Some(session) = self.session
        {
            let delta = session.delta();
            return Some([delta.x, delta.y, delta.z]);
        }
        self.pending_move
            .dragged()
            .or_else(|| self.translation_in_fields(cx))
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

    /// A live gizmo drag: accumulate it into the session, mirror it into the Δ fields, and
    /// keep the selection previewing. Nothing reaches the document, so the drag and the
    /// typed values describe the same pending move.
    ///
    /// There is **one mode**: the release never commits, with or without a HUD. The session
    /// stays live until `Enter` commits it, `Esc` drops it, or the context changes.
    pub(super) fn on_transform_preview(&mut self, preview: &TransformPreview, cx: &mut Context<Self>) {
        let m = &preview.total_transform.m;
        // A translation is all this can be: a rotate drag never emits a preview (the
        // viewport previews and commits it from the full matrix itself).
        let drag = [m[3], m[7], m[11]];
        let previews = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation());

        if preview.phase == PreviewPhase::Begin {
            // A drag reports its transform from its own press, so it adds to whatever the
            // session already holds — a second drag on a pending move continues it.
            // The value the drag adds to: the session's delta when there is a stage to
            // open one from, otherwise the HUD's own mirror (a headless shell).
            let current = self
                .session
                .map(|session| {
                    let delta = session.delta();
                    [delta.x, delta.y, delta.z]
                })
                .or_else(|| self.pending_move.dragged())
                .unwrap_or_default();
            self.pending_move.begin_drag(current);
            // The session opens at the press, from the un-previewed stage, so its datum is
            // the pose the gesture started from.
            self.session = self
                .last_stage
                .as_ref()
                .and_then(|stage| TransformSession::begin(stage, preview.target_id));
        }
        let delta = self.pending_move.tick(drag);
        if let Some(session) = self.session.as_mut() {
            session.set_translation(Vector3::new(delta[0], delta[1], delta[2]));
        }
        if preview.phase == PreviewPhase::End {
            self.pending_move.end_drag();
        }
        if previews {
            self.set_translation_fields(self.pending_move.dragged().unwrap_or_default(), cx);
            self.preview_dialog(cx);
        } else if let Some(session) = self.session {
            // No HUD: the session still owns the preview, so a plain drag shows the same
            // ghost the open form would.
            let delta = session.delta();
            self.preview_selection(Some([delta.x, delta.y, delta.z]), cx);
        }
    }

    /// Commit the pending session as **one** transaction, through the command a script uses.
    ///
    /// `App_Move` with no arguments hands back its spec and a token; accepting that token
    /// with the session's delta runs `_move_selection`, which owns the transaction, the
    /// recompute and the status line. The HUD is only that command's form — the session does
    /// not need it to exist, which is why the route is unchanged by U1.
    ///
    /// Returns whether anything was committed.
    pub(super) fn commit_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session.filter(|session| session.is_dirty()) else {
            return false;
        };
        let delta = session.delta();
        let outcome = match commands::run("App_Move") {
            Ok(outcome) => outcome,
            Err(error) => {
                self.console_line(format!("move: {error}"), cx);
                return false;
            }
        };
        let Some(token) = outcome.token.clone() else {
            self.console_line("move: App_Move returned no token".to_string(), cx);
            return false;
        };
        let values = serde_json::json!({"dx": delta.x, "dy": delta.y, "dz": delta.z});
        let result = commands::accept(&token, &values);
        // The move is applied (or replaced by the command's own dialog): the pending value
        // is spent either way, and the ghost goes with it before the stage rebuild.
        self.session = None;
        self.pending_move.reset();
        self.clear_preview_items(cx);
        self.finish_dialog(result, cx);
        true
    }

    /// Drop the pending session without writing anything (`Esc`).
    pub(super) fn drop_session(&mut self, status: &str, cx: &mut Context<Self>) -> bool {
        if self.session.is_none() {
            return false;
        }
        self.session = None;
        self.pending_move.reset();
        self.clear_preview_items(cx);
        self.model.lock().unwrap().status = status.to_string();
        cx.notify();
        true
    }

    /// Preview the current selection at `delta` without touching the document.
    ///
    /// This is the session's display half; it is the same write `preview_dialog` used to
    /// do inline, so a drag with no HUD open previews exactly like one with it.
    fn preview_selection(&mut self, delta: Option<[f64; 3]>, cx: &mut Context<Self>) {
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
        let transform = delta.map(|d| Matrix4::translation(d[0], d[1], d[2]));
        viewport.update(cx, |viewport, cx| {
            if viewport.preview_transform(&entities, transform) {
                cx.notify();
            }
        });
        // The gizmo (and therefore the HUD anchor) follows the previewed object, so
        // re-render the shell to keep the panel beside it.
        cx.notify();
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
        let mut states = self.dialog_states(cx);
        // A Move HUD's Δ fields are a rounded view of the drag; the callback must get
        // the exact delta the user saw, or accepting a drag commits a nearby pose. Only
        // a **drag** overrides the fields — typed text keeps `collect`'s validation.
        let dragged = if self.pending_move.is_dragged() {
            self.pending_translation(cx)
        } else {
            None
        };
        if let Some(delta) = dragged {
            for (field, state) in spec.fields.iter().zip(states.iter_mut()) {
                let slot = match field.name.as_str() {
                    "dx" => 0,
                    "dy" => 1,
                    "dz" => 2,
                    _ => continue,
                };
                *state = ValueState::Text(format!("{}", delta[slot]));
            }
        }
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
                // A translation HUD is a Move session. Accepting it ends the session, but
                // the triad **stays** on the object: `sync_stage`/`rearm_selection` below
                // rebuild it at the moved anchor, so the user keeps a handle to nudge
                // something they just placed. (Dismissing it left them with no handles
                // and no way back except re-selecting.)
                if outcome.dialog.is_none()
                    && self
                        .dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.spec.previews_translation())
                {
                    self.transform_verb = true;
                }
                // The pending move is now applied (or replaced by a wizard): release
                // the session and the ghost before the stage rebuild.
                self.discard_pending(cx);
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
        self.discard_pending(cx);
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

/// The field index for a gizmo axis (X, Y, Z).
fn axis_index(axis: GizmoAxis) -> usize {
    match axis {
        GizmoAxis::X => 0,
        GizmoAxis::Y => 1,
        GizmoAxis::Z => 2,
    }
}

/// The field index for an armed handle. A plane (or the origin) focuses X, per the
/// HUD's typed-field order.
fn handle_axis_index(handle: GizmoHandle) -> usize {
    match handle {
        GizmoHandle::AxisX | GizmoHandle::RotateX => 0,
        GizmoHandle::AxisY | GizmoHandle::RotateY => 1,
        GizmoHandle::AxisZ | GizmoHandle::RotateZ => 2,
        GizmoHandle::PlaneXY | GizmoHandle::PlaneXZ | GizmoHandle::PlaneYZ => 0,
    }
}
