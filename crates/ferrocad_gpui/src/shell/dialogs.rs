//! `Shell` methods: dialogs.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;
use super::pill::PillState;

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
                // delta a drag left behind and updates the session that owns it.
                let change = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                        this.on_move_typed(cx);
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
        // A translation spec is the Move pill. It opens expanded on X (a keyboard
        // escalation immediately re-focuses the armed axis) and, when the selection has
        // a stage, opens the session right away: typing then edits the session instead
        // of living only in the fields until a drag (U2b).
        let translation = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation());
        if translation {
            self.pill = PillState::expanded_at(0);
            self.open_move_session(cx);
        } else {
            self.pill = PillState::compact();
        }
        self.focus_dialog = true;
        self.sync_gizmo_origin(cx);
        cx.notify();
    }

    /// Open the session behind an open Move pill, when the selection has a stage.
    ///
    /// A shell with no stage (headless, or an object whose item has not arrived yet) has no
    /// datum to publish, so the fields and the accept callback remain the fallback. The
    /// session starts clean at the selection's current pose and is updated by typing or a
    /// drag.
    fn open_move_session(&mut self, _cx: &mut Context<Self>) {
        if self.session.is_some() {
            return;
        }
        self.begin_drag_session(None);
    }

    /// Open (or replace) the session for the current selection from the un-previewed stage.
    ///
    /// The datum is the primary's anchor and the targets are the selection, so a drag always
    /// accumulates against the current document pose and the current objects — a stage
    /// rebuild cannot leave a stale datum behind. `fallback` is the preview's target, used
    /// when the selection does not map onto the stage yet.
    fn begin_drag_session(&mut self, fallback: Option<u32>) {
        let mut targets: Vec<u32> = {
            let model = self.model.lock().unwrap();
            model
                .selection
                .iter()
                .filter_map(|item| self.display.entity_of(&item.object))
                .collect()
        };
        if targets.is_empty()
            && let Some(fallback) = fallback
        {
            targets.push(fallback);
        }
        self.session = self
            .last_stage
            .as_ref()
            .and_then(|stage| TransformSession::begin(stage, &targets));
    }

    /// A typed edit in the Move fields: the fields win, and the session (when one can be
    /// opened) carries the value from now on.
    fn on_move_typed(&mut self, cx: &mut Context<Self>) {
        if let Some(delta) = self.translation_in_fields(cx) {
            self.open_move_session(cx);
            if let Some(session) = self.session.as_mut() {
                session.set_translation(Vector3::new(delta[0], delta[1], delta[2]));
            }
        }
        self.preview_dialog(cx);
        self.sync_transform_tape(cx);
    }

    /// Whether the open dialog is the Move pill.
    pub(super) fn move_pill_open(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
    }

    /// Focus the pill field the [`PillState`](PillState) names.
    fn focus_pill_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let axis = self.pill.focus().min(AXIS_FIELDS.len() - 1);
        if let Some(input) = self.dialog_input(AXIS_FIELDS[axis]) {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Expand a compact pill at the axis the pending drag moved and focus that field.
    ///
    /// This is the click path; `Tab` keeps the [`PillState`](PillState)'s own
    /// escalation (`tab`) so it never loses the user's place.
    pub(super) fn expand_move_pill(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.move_pill_open() {
            return false;
        }
        let delta = self.pending_delta(cx).unwrap_or_default();
        self.pill.set_focus(axis_of(delta));
        let expanded = self.pill.expand();
        self.focus_pill_field(window, cx);
        cx.notify();
        expanded
    }

    /// `Tab`/`Shift+Tab` on the pill: expand when compact, otherwise cycle the fields.
    pub(super) fn cycle_pill_focus(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.move_pill_open() {
            return;
        }
        self.pill.tab(backward);
        self.focus_pill_field(window, cx);
        cx.notify();
    }

    /// A digit typed while the compact pill is showing: expand it and replace the
    /// focused field, the same one-transition entry [`escalate_to_move_hud`] performs.
    pub(super) fn type_into_move_pill(
        &mut self,
        typed: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.move_pill_open() {
            return false;
        }
        self.pill.expand();
        let axis = self.pill.focus().min(AXIS_FIELDS.len() - 1);
        let Some(input) = self.dialog_input(AXIS_FIELDS[axis]) else {
            return false;
        };
        input.update(cx, |input, cx| input.set_text(typed.to_string(), cx));
        // `set_text` is programmatic (no change event), so update the session and drive
        // the preview here.
        self.on_move_typed(cx);
        self.focus_pill_field(window, cx);
        cx.notify();
        true
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
        // path later.
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
    /// Opening the HUD mid-drag calls this first; the seed comes from the viewport's live
    /// readout ([`gizmo_seed`](Self::gizmo_seed)), so the drag's value is not lost. The
    /// baseline is cleared because the next tick starts a fresh accumulation from the seed.
    pub(super) fn clear_dialog_preview(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.drag_start = None;
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
    }

    /// Drop the pending value entirely (cancel, accept, or a competing interaction).
    pub(super) fn discard_pending(&mut self, cx: &mut Context<Self>) {
        self.drag_start = None;
        self.pill = PillState::compact();
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
            return self.seed_from_session();
        };
        let viewport = viewport.read(cx);
        let Some(gizmo) = viewport.gizmo() else {
            return self.seed_from_session();
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
        // The pill is already the Move HUD; the escalation decides its stage and
        // focused axis, and the seeded value edits the session the HUD opened.
        self.pill = PillState::expanded_at(seed.axis);
        if let Some(session) = self.session.as_mut() {
            // The exact seed, not the two-decimal text above: rounding here is the
            // 2026-10-30 bug (a release committed a value the user never saw).
            session.set_translation(Vector3::new(seed.delta[0], seed.delta[1], seed.delta[2]));
        }
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
            self.on_move_typed(cx);
        } else {
            // Select the seed so the next keystroke replaces it (`0.00` + `5` must be
            // `5`, not `0.005`).
            input.update(cx, |input, cx| input.select_all(cx));
        }
        cx.notify();
        true
    }

    /// The session's delta, when a session is live.
    fn session_delta(&self) -> Option<[f64; 3]> {
        self.session.as_ref().map(|session| {
            let delta = session.delta();
            [delta.x, delta.y, delta.z]
        })
    }

    /// The pending translation a Move pill would show: the session when it is dirty, else
    /// the parsed fields. A clean session carries no value the fields do not (a typed zero,
    /// or a drag that returned to the datum), so the fields stay authoritative for
    /// validation.
    fn pending_delta(&self, cx: &App) -> Option<[f64; 3]> {
        self.session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(|session| {
                let delta = session.delta();
                [delta.x, delta.y, delta.z]
            })
            .or_else(|| self.translation_in_fields(cx))
    }

    /// What a keyboard escalation seeds the HUD with, when there is no viewport to ask.
    fn seed_from_session(&self) -> GizmoSeed {
        let delta = self.session_delta().unwrap_or_default();
        GizmoSeed {
            axis: axis_of(delta),
            delta,
        }
    }

    /// The pending translation a Move session will commit.
    ///
    /// The session is the authority while it holds a non-zero value: it has the exact
    /// delta and the datum it was measured from. A clean session falls back to the Δ
    /// fields, which keeps validation (a half-typed `-` is an error on `Enter`, not a
    /// silent zero). `None` when the spec asks for no translation.
    pub(super) fn pending_translation(&self, cx: &App) -> Option<[f64; 3]> {
        let dialog = self.dialog.as_ref()?;
        if !dialog.spec.previews_translation() {
            return None;
        }
        self.pending_delta(cx)
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

    /// Push the session's tape snapshot into the viewport, or clear it.
    ///
    /// The tape is the **view** of the session; the session itself never knows it is
    /// drawn. A clean session has no tape, and clearing the session (commit, `Esc`, a
    /// context change) hides it with the ghost.
    pub(super) fn sync_transform_tape(&mut self, cx: &mut Context<Self>) {
        let tape = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(TransformTape::from_session);
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.set_transform_tape(tape) {
                cx.notify();
            }
        });
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
            // A drag is compact: the readout follows the pointer while the fields wait
            // behind `Tab` (the pill expands again, on the same axis, on request).
            if previews {
                self.pill.collapse();
            }
            // A drag reports its transform from its own press, so it adds to whatever the
            // session already holds — a second drag on a pending move continues it. The
            // session is re-opened from the un-previewed stage so its datum and targets are
            // the current document pose; the pending value is carried in `drag_start`.
            self.drag_start = Some(self.session_delta().unwrap_or_default());
            self.begin_drag_session(Some(preview.target_id));
        }
        if self.session.is_none() {
            // A tick without a `Begin` (a harness, or a host that missed it) still opens it.
            self.begin_drag_session(Some(preview.target_id));
        }
        let baseline = self.drag_start.unwrap_or_default();
        let delta = [
            baseline[0] + drag[0],
            baseline[1] + drag[1],
            baseline[2] + drag[2],
        ];
        if let Some(session) = self.session.as_mut() {
            session.set_translation(Vector3::new(delta[0], delta[1], delta[2]));
        }
        if preview.phase == PreviewPhase::End {
            self.drag_start = None;
        }
        if previews {
            self.set_translation_fields(delta, cx);
            self.preview_dialog(cx);
        } else if self.session.is_some() {
            // No HUD: the session still owns the preview, so a plain drag shows the same
            // ghost the open form would.
            self.preview_selection(Some(delta), cx);
        }
        // The tape follows the session, including after the release (SESSION_HOLD): it is
        // cleared by the commit/Esc/context paths, not by the end of the pointer gesture.
        self.sync_transform_tape(cx);
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
        let Some(delta) = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(TransformSession::delta)
        else {
            return false;
        };
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
        self.drag_start = None;
        self.pill = PillState::compact();
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
        self.finish_dialog(result, cx);
        true
    }

    /// Drop the pending session without writing anything (`Esc`).
    pub(super) fn drop_session(&mut self, status: &str, cx: &mut Context<Self>) -> bool {
        if self.session.is_none() {
            return false;
        }
        self.session = None;
        self.drag_start = None;
        self.pill = PillState::compact();
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
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
    /// back. Fields without an input (bool/enum) are not stops. The Move pill routes
    /// through [`PillState`](PillState), so its compact stage expands instead.
    pub(super) fn cycle_dialog_focus(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.move_pill_open() {
            self.cycle_pill_focus(backward, window, cx);
            return;
        }
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
        // A Move HUD's Δ fields are a rounded view of the session; the callback must get
        // the exact delta the user saw, or accepting a drag commits a nearby pose. A clean
        // session has no value the fields do not, so it keeps `collect`'s validation (a
        // half-typed field is an error, not a silent zero).
        let exact = if spec.previews_translation() {
            self.session
                .as_ref()
                .filter(|session| session.is_dirty())
                .map(|session| {
                    let delta = session.delta();
                    [delta.x, delta.y, delta.z]
                })
        } else {
            None
        };
        if let Some(delta) = exact {
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
        self.pill = PillState::compact();
        self.discard_pending(cx);
        self.sync_gizmo_origin(cx);
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
