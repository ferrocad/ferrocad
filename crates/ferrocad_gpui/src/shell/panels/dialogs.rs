//! `Shell` methods: dialogs.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use crate::shell::*;
use crate::shell::move_hud::AXIS_COUNT;

impl Shell {
    /// Open a command's dialog, seeding one control per field.
    pub(in crate::shell) fn open_dialog_state(&mut self, token: String, spec: DialogSpec, cx: &mut Context<Self>) {
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
                // for one).
                let change = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                        this.on_dialog_change(cx);
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
        // A translation spec is the Move form, which has its own surface; a command
        // dialog is a modal and starts in its first field.
        self.focus_dialog = true;
        self.sync_gizmo_origin(cx);
        cx.notify();
    }

    /// Open (or replace) the session for the current selection from the un-previewed stage.
    ///
    /// The datum is the primary's anchor and the targets are the selection, so a drag always
    /// accumulates against the current document pose and the current objects — a stage
    /// rebuild cannot leave a stale datum behind. `fallback` is the preview's target, used
    /// when the selection does not map onto the stage yet.
    pub(in crate::shell) fn begin_drag_session(&mut self, fallback: Option<u32>) {
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

    /// A typed edit in a command dialog: drive its live preview.
    fn on_dialog_change(&mut self, cx: &mut Context<Self>) {
        self.preview_dialog(cx);
    }

    /// The collected state of every field; the text inputs are authoritative.
    pub(in crate::shell) fn dialog_states(&self, cx: &App) -> Vec<ValueState> {
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
    /// Only `"workplane_offset"` asks for one now: the Move form is its own surface
    /// (`move_hud.rs`) and drives its preview directly.
    pub(in crate::shell) fn preview_dialog(&mut self, cx: &mut Context<Self>) {
        let workplane_offset = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.drives_workplane_offset());
        if workplane_offset {
            // The typed half of the offset session: the field moves the plane (and its
            // stem) live, so typing and dragging describe the same pending offset.
            self.preview_workplane_offset(cx);
        }
    }

    /// Drop the ghost and the session without changing the document.
    ///
    /// Opening the HUD mid-drag calls this first; the seed comes from the viewport's live
    /// readout ([`gizmo_seed`](Self::gizmo_seed)), so the drag's value is not lost. The
    /// baseline is cleared because the next tick starts a fresh accumulation from the seed.
    pub(in crate::shell) fn clear_dialog_preview(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.drag_start = None;
        self.rotation_start = None;
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
    }

    /// Drop the pending value entirely (cancel, accept, or a competing interaction).
    pub(in crate::shell) fn discard_pending(&mut self, cx: &mut Context<Self>) {
        self.drag_start = None;
        self.rotation_start = None;
        self.pill.release();
        self.clear_dialog_preview(cx);
    }

    /// Clear every item's preview transform (and the overlay offsets that came with it).
    pub(in crate::shell) fn clear_preview_items(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            let mut changed = viewport.preview_transform(&[], None);
            changed |= viewport.set_protractor(None);
            if changed {
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
    pub(in crate::shell) fn gizmo_seed(&self, cx: &App) -> GizmoSeed {
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

    /// Escalate an armed gizmo into the Move form.
    ///
    /// The form belongs to the Move hand, so this is just [`open_move_hud`] plus a seed:
    /// the fields take the current drag delta (zero when idle) and the field the armed
    /// handle names takes focus. `typed` is the numeric key that opened it: it replaces
    /// the freshly seeded value and previews immediately.
    pub(in crate::shell) fn escalate_to_move_hud(
        &mut self,
        seed: GizmoSeed,
        typed: Option<char>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_tool.is_some() {
            return false;
        }
        // The verb is `Move`: equip the body hand and force the translate arrows, so the
        // fields and the visible handles agree.
        self.set_loadout_slot(Slot::Move, cx);
        self.set_gizmo_mode("translate", cx);
        self.open_move_hud(cx);
        if self.move_hud.is_none() {
            return false;
        }
        self.set_move_fields(seed.delta, cx);
        let axis = seed.axis.min(AXIS_COUNT - 1);
        self.pill.focus_axis(axis);
        if let Some(session) = self.session.as_mut() {
            // The exact seed, not the two-decimal text above: rounding here is the
            // 2026-10-30 bug (a release committed a value the user never saw).
            session.set_translation(Vector3::new(seed.delta[0], seed.delta[1], seed.delta[2]));
        }
        // `open_move_hud` asked the next render to give the keyboard back to the canvas.
        // The escalation is a keyboard act: focus the field the armed handle names.
        self.restore_viewport_focus = false;
        if let Some(ch) = typed {
            self.set_move_text(axis, ch.to_string(), cx);
            // `set_text` is programmatic (no change event), so drive the preview here.
            self.on_move_typed(cx);
        } else {
            // Select the seed so the next keystroke replaces it.
            self.select_move_field(axis, cx);
        }
        self.focus_move_field(window, cx);
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

    /// What a keyboard escalation seeds the HUD with, when there is no viewport to ask.
    fn seed_from_session(&self) -> GizmoSeed {
        let delta = self.session_delta().unwrap_or_default();
        GizmoSeed {
            axis: axis_of(delta),
            delta,
        }
    }

    /// Push the session's tape snapshot into the viewport, or clear it.
    ///
    /// The tape is the **view** of the session; the session itself never knows it is
    /// drawn. A clean session has no tape, and clearing the session (commit, `Esc`, a
    /// context change) hides it with the ghost.
    pub(in crate::shell) fn sync_transform_tape(&mut self, cx: &mut Context<Self>) {
        // The ruler measures the active frame, the same one as the handles and the fields:
        // a `LOC` drag shows its local displacement, not the world decomposition.
        let basis = self.frame_basis_now(cx);
        let tape = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(|session| TransformTape::from_session_in_frame(session, basis));
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        // A numeric tag (a ruler chip or a ring degree chip) may only take a click while
        // there is a form to focus; the viewport keeps the tags non-occluding during a
        // drag regardless.
        let clickable = self.move_pill_open();
        viewport.update(cx, |viewport, cx| {
            let mut changed = viewport.set_transform_tape(tape);
            changed |= viewport.set_tape_labels_clickable(clickable);
            if changed {
                cx.notify();
            }
        });
    }

    /// A live gizmo drag: accumulate it into the session, mirror it into the Δ fields, and
    /// keep the selection previewing. Nothing reaches the document, so the drag and the
    /// typed values describe the same pending move.
    ///
    /// There is **one mode**: the release never commits, with or without a HUD. The session
    /// stays live until `Enter` commits it, `Esc` drops it, or the context changes. A ring
    /// drag takes the same route; its value is an axis-angle rather than a translation, and
    /// it owns exactly one body (the commit hook applies a transform to one object).
    pub(in crate::shell) fn on_transform_preview(
        &mut self,
        preview: &TransformPreview,
        cx: &mut Context<Self>,
    ) {
        // A drag on another body would hijack the pending move (the session targets one
        // subject, and the tape measures it). The context gate refuses the stroke and the
        // viewport drops it, so the triad cannot end up on a body the session does not own.
        if !self
            .session
            .as_ref()
            .is_some_and(|session| session.targets().contains(&preview.target_id))
            && self.gate(Action::Arm, cx)
        {
            if let Some(viewport) = self.viewport.clone() {
                viewport.update(cx, |viewport, cx| {
                    viewport.cancel_gizmo_drag();
                    cx.notify();
                });
            }
            return;
        }
        let angular = match preview.value {
            GizmoValue::Angular {
                axis, applied_deg, ..
            } => {
                // The ring's world axis comes from its sweep (a local ring frame turns it
                // with the body); the label names the bar's segment.
                let world = preview
                    .ring
                    .as_ref()
                    .map(|ring| ring.axis)
                    .unwrap_or_else(|| gizmo_axis_vector(axis));
                Some((axis_index(axis), world, applied_deg))
            }
            _ => None,
        };
        let m = &preview.total_transform.m;
        let drag = [m[3], m[7], m[11]];
        let begin = preview.phase == PreviewPhase::Begin;
        let settle = preview.phase == PreviewPhase::End;

        if begin {
            // A press on the hover-to-arm triad is the user's first action on that
            // body: promote it before the session opens (U5). A drag on a latched
            // selection is already selected, so this is a no-op.
            self.promote_hover_target(preview.target_id, cx);
            if let Some((index, world_axis, _)) = angular {
                // A pending rotation owns one ring. A sibling press that reached the host
                // (a harness, or a host that missed the viewport gate) is refused rather
                // than committing behind the user's back.
                if let Some(locked) = self.rotation_pending() {
                    if locked != index {
                        self.show_rotation_lock(index, cx);
                        if let Some(viewport) = self.viewport.clone() {
                            viewport.update(cx, |viewport, cx| {
                                viewport.cancel_gizmo_drag();
                                cx.notify();
                            });
                        }
                        return;
                    }
                }
                // Dragging the same ring again adds to that ring's angle; a clean
                // session starts the step at zero.
                let baseline = self
                    .session
                    .as_ref()
                    .filter(|session| {
                        session.is_dirty()
                            && session.mode() == GizmoMode::Rotate
                            && session.pending_axis_index() == Some(index)
                    })
                    .map(TransformSession::pending_degrees)
                    .unwrap_or(0.0);
                self.rotation_start = Some(baseline);
                // Rotation is one subject: the interaction hook applies the matrix to one
                // object, so the session follows the triad's target rather than the whole
                // selection.
                self.session = self
                    .last_stage
                    .as_ref()
                    .and_then(|stage| TransformSession::begin(stage, &[preview.target_id]));
                // A promoted body makes the angular row real: it appears with the drag if
                // it was not already up.
                self.ensure_move_hud(cx);
                let orientation = self.object_rotation_now();
                if let Some(session) = self.session.as_mut() {
                    session.set_mode(GizmoMode::Rotate);
                    session.set_orientation_0(orientation);
                    session.set_rotation_step(index, world_axis, baseline);
                }
                if let Some(hud) = self.move_hud.as_mut() {
                    hud.axis = index;
                }
                self.pill.remember_axis(index);
            } else {
                // A drag reports its transform from its own press, so it adds to whatever
                // the session already holds — a second drag on a pending move continues it.
                // The session is re-opened from the un-previewed stage so its datum and
                // targets are the current document pose; the pending value is carried in
                // `drag_start`.
                self.drag_start = Some(self.session_delta().unwrap_or_default());
                self.begin_drag_session(Some(preview.target_id));
                // A promoted hover body makes the Move hand's target real: the form appears
                // with the drag, if it was not already up.
                self.ensure_move_hud(cx);
            }
            // The pointer owns this gesture: the form must not take the keyboard back
            // from the canvas mid-drag.
            self.restore_viewport_focus = false;
        }
        if self.session.is_none() {
            // A tick without a `Begin` (a harness, or a host that missed it) still opens it.
            if let Some((index, world_axis, _)) = angular {
                self.session = self
                    .last_stage
                    .as_ref()
                    .and_then(|stage| TransformSession::begin(stage, &[preview.target_id]));
                let orientation = self.object_rotation_now();
                if let Some(session) = self.session.as_mut() {
                    session.set_mode(GizmoMode::Rotate);
                    session.set_orientation_0(orientation);
                    session.set_rotation_step(index, world_axis, 0.0);
                }
                self.ensure_move_hud(cx);
            } else {
                self.begin_drag_session(Some(preview.target_id));
            }
        }

        if let Some((index, world_axis, applied_deg)) = angular {
            // A ring reports its sweep from its own press. The session is one step, so it
            // holds `baseline + applied` about the **same** axis. A sibling axis cannot
            // reach here while a step is pending: both the viewport and the Begin guard
            // refuse it.
            if let Some(locked) = self.rotation_pending() {
                if locked != index {
                    self.show_rotation_lock(index, cx);
                    return;
                }
            }
            let baseline = self.rotation_start.unwrap_or(0.0);
            if let Some(session) = self.session.as_mut() {
                session.set_mode(GizmoMode::Rotate);
                session.set_rotation_step(index, world_axis, baseline + applied_deg);
            }
            if let Some(hud) = self.move_hud.as_mut() {
                hud.axis = index;
            }
            self.pill.remember_axis(index);
            // The docked field is the numeric channel; the 3-D arc tag is the in-context
            // one. A drag never takes the keyboard, so this is a mirror only.
            if self.move_hud.is_some() {
                self.set_move_fields([0.0; 3], cx);
            }
            // One pending step means one protractor arc and one chip.
            let sweeps = self.session_protractor(cx);
            self.publish_protractor(sweeps, cx);
            self.sync_locked_ring(cx);
            if settle {
                self.drag_start = None;
                self.rotation_start = None;
            }
            // The session owns the preview matrix; a rotation has no measuring tape (its
            // delta is zero), while the docked field shows its single signed angle.
            if let Some(transform) = self.session.as_ref().map(TransformSession::transform) {
                self.preview_selection(Some(transform), settle, cx);
            }
            self.sync_transform_tape(cx);
            return;
        }

        let baseline = self.drag_start.unwrap_or_default();
        let delta = [
            baseline[0] + drag[0],
            baseline[1] + drag[1],
            baseline[2] + drag[2],
        ];
        let previews = self.move_hud.is_some();
        if let Some(session) = self.session.as_mut() {
            session.set_mode(GizmoMode::Translate);
            session.set_translation(Vector3::new(delta[0], delta[1], delta[2]));
        }
        if settle {
            self.drag_start = None;
        }
        // Remember where the user just worked, so the first `Tab` opens the field the
        // gesture moved instead of always `ΔX`. A drag never takes the keyboard from a
        // field: the fields tick live but stay unfocused (see `PillState`).
        if let Some(axis) = axis_memory(drag) {
            self.pill.remember_axis(axis);
        }
        if previews {
            self.set_move_fields(delta, cx);
            self.preview_move(settle, cx);
        } else if self.session.is_some() {
            // No HUD: the session still owns the preview, so a plain drag shows the same
            // ghost the open form would. The release settles the frame at the pending
            // pose even if the viewport still reports its drag for a tick.
            self.preview_selection(
                Some(Matrix4::translation(delta[0], delta[1], delta[2])),
                settle,
                cx,
            );
        }
        // The tape follows the session, including after the release (SESSION_HOLD): it is
        // cleared by the commit/Esc/context paths, not by the end of the pointer gesture.
        self.sync_transform_tape(cx);
    }

    /// Commit the pending move as **one** transaction, through the command a script uses.
    ///
    /// `App_Move` with no arguments hands back a token; accepting it with the exact
    /// delta runs `_move_selection`, which owns the transaction, the recompute and the
    /// status line. The surface never holds that token: it belongs to the hand, so one
    /// is acquired per commit and spent immediately. A clean form (nothing typed,
    /// nothing dragged) is a dismissal, not a zero-length transaction.
    ///
    /// Returns whether anything was committed.
    pub(in crate::shell) fn commit_session(&mut self, cx: &mut Context<Self>) -> bool {
        // A rotate session is one body and is not a translation: `App_Move` would apply the
        // delta (zero) to the selection, so it commits through the same transform
        // interaction a workbench gizmo uses. That hook owns the transaction and the
        // recompute, exactly as it does for a direct ring commit.
        let rotate = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty() && session.mode() == GizmoMode::Rotate)
            .map(|session| (session.primary(), session.transform()));
        if let Some((target, matrix)) = rotate {
            // The numeric readout lives on the 3-D arc tag; the status line stays a
            // short verb so the bottom-left text is not a second readout.
            let commit = InteractionCommit::new(TRANSFORM_ID, target, "apply")
                .with_values(matrix.m)
                .with_readout("rotated");
            self.session = None;
            self.drag_start = None;
            self.rotation_start = None;
            self.clear_preview_items(cx);
            self.sync_transform_tape(cx);
            self.on_interaction(&commit, cx);
            self.sync_locked_ring(cx);
            self.reset_move_fields(cx);
            self.restore_viewport_focus = true;
            cx.notify();
            return true;
        }
        let session_delta = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(|session| {
                let delta = session.delta();
                [delta.x, delta.y, delta.z]
            });
        // A visible field that does not parse blocks the commit even when a drag left a
        // dirty session: the user is looking at `-` and must not spend the old value.
        let fields = if self.move_hud.is_some() {
            match self.move_fields_delta(cx) {
                Some(delta) => Some(delta),
                None => {
                    self.model.lock().unwrap().status =
                        "move: check the coordinate values".to_string();
                    cx.notify();
                    return false;
                }
            }
        } else {
            None
        };
        let delta = match session_delta.or(fields) {
            Some(delta) => delta,
            None => return false,
        };
        if delta.iter().all(|value| *value == 0.0) {
            // Nothing pending: ✓/Enter resets the form and spends nothing.
            self.reset_move_fields(cx);
            self.restore_viewport_focus = true;
            cx.notify();
            return false;
        }
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
        let values = serde_json::json!({"dx": delta[0], "dy": delta[1], "dz": delta[2]});
        let result = commands::accept(&token, &values);
        // The move is applied (or the callback failed): the pending value is spent either
        // way, and the ghost goes with it before the stage rebuild.
        self.session = None;
        self.drag_start = None;
        self.rotation_start = None;
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
        match result {
            Ok(outcome) if outcome.ok => {
                let status = if outcome.status.is_empty() {
                    "moved".to_string()
                } else {
                    outcome.status
                };
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                self.refresh(cx);
                self.sync_stage(cx);
                self.sync_overlays(cx);
                self.rearm_selection(cx);
            }
            Ok(outcome) => {
                let error = if outcome.error.is_empty() {
                    "the move failed".to_string()
                } else {
                    outcome.error
                };
                self.model.lock().unwrap().status = error.clone();
                self.console_line(error, cx);
            }
            Err(error) => {
                self.model.lock().unwrap().status = error.clone();
                self.console_line(error, cx);
            }
        }
        self.reset_move_fields(cx);
        self.restore_viewport_focus = true;
        cx.notify();
        true
    }

    /// Drop the pending session without writing anything (`Esc`/`✗`).
    pub(in crate::shell) fn drop_session(&mut self, status: &str, cx: &mut Context<Self>) -> bool {
        if self.session.is_none() {
            return false;
        }
        self.session = None;
        self.drag_start = None;
        self.rotation_start = None;
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
        self.sync_locked_ring(cx);
        self.reset_move_fields(cx);
        self.model.lock().unwrap().status = status.to_string();
        self.restore_viewport_focus = true;
        cx.notify();
        true
    }

    /// Preview the current selection at `transform` without touching the document.
    ///
    /// This is the session's display half; it is the same write a form edit uses, so a
    /// drag with no form open previews exactly like one with it. The matrix carries a
    /// translation or a rotation, so both gestures on the session share this one path.
    pub(in crate::shell) fn preview_selection(
        &mut self,
        transform: Option<Matrix4>,
        settle: bool,
        cx: &mut Context<Self>,
    ) {
        // The session is the authority when it is live: a rotation owns exactly one body,
        // while a translation owns the whole selection. Falling back to the selection
        // keeps a typed preview working before a stage (and so a session) exists.
        let entities: Vec<u32> = self
            .session
            .as_ref()
            .map(|session| session.targets().to_vec())
            .unwrap_or_else(|| {
                let model = self.model.lock().unwrap();
                model
                    .selection
                    .iter()
                    .filter_map(|item| self.display.entity_of(&item.object))
                    .collect()
            });
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            let _changed = if settle {
                viewport.preview_transform_settled(&entities, transform)
            } else {
                viewport.preview_transform(&entities, transform)
            };
            cx.notify();
        });
        // The gizmo (and therefore the HUD anchor) follows the previewed object, so
        // re-render the shell to keep the panel beside it.
        cx.notify();
    }

    /// Where a floating dialog should sit: beside the gizmo, else at the viewport
    /// centre, else under the live pointer. The gizmo wins when it is visible, so the
    /// HUD hangs off the handles the user is about to drag; a viewport with no gizmo
    /// still anchors to the canvas instead of a stale pointer. The pointer is the last
    /// resort for a shell with no viewport at all.
    pub(in crate::shell) fn dialog_anchor(&self, window: &Window, cx: &App) -> Point<Pixels> {
        if let Some(viewport) = self.viewport.as_ref() {
            let viewport = viewport.read(cx);
            if let Some(anchor) = viewport
                .gizmo_anchor()
                .or_else(|| viewport.viewport_anchor())
            {
                return anchor;
            }
        }
        window.mouse_position()
    }

    /// Move focus between a dialog's textual fields. `Tab` cycles forward; `Shift+Tab`
    /// back. Fields without an input (bool/enum) are not stops.
    pub(in crate::shell) fn cycle_dialog_focus(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(in crate::shell) fn accept_dialog(&mut self, cx: &mut Context<Self>) {
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
    pub(in crate::shell) fn finish_dialog(
        &mut self,
        result: Result<commands::CommandOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(outcome) if outcome.ok => {
                // A wizard replaces the spec rather than stacking, and a plain accept
                // closes the surface. The Move form is not a dialog and is untouched.
                if let (Some(spec), Some(token)) = (outcome.dialog.clone(), outcome.token.clone()) {
                    self.open_dialog_state(token, spec, cx);
                } else {
                    self.dialog = None;
                    // A closed modal hands the keyboard back to the canvas.
                    self.restore_viewport_focus = true;
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
    pub(in crate::shell) fn cancel_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let _ = commands::cancel(&dialog.token);
        self.sync_gizmo_origin(cx);
        self.restore_viewport_focus = true;
        self.model.lock().unwrap().status = "dialog cancelled".to_string();
        cx.notify();
    }

    /// Whether a modal currently owns the shell: a command dialog, the palette or
    /// a rendered form. At most one of them may be present, so a second backdrop
    /// can never be stacked on the first.
    ///
    /// The Move form is **not** a dialog and never counts here: its fields are a
    /// visible readout with no focus by default, which is what lets `1`/`2`/`3` keep
    /// equipping slots and a command keep opening its own dialog while it is up.
    pub(in crate::shell) fn modal_open(&self) -> bool {
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

/// The world direction a ring with this axis label turns about.
///
/// The hosted triad is built in the world frame, so the label is the axis. A gizmo in a
/// rotated local frame would need [`TransformGizmo::frame`], which this stack does not
/// use yet.
fn gizmo_axis_vector(axis: GizmoAxis) -> Vector3 {
    match axis {
        GizmoAxis::X => Vector3::new(1.0, 0.0, 0.0),
        GizmoAxis::Y => Vector3::new(0.0, 1.0, 0.0),
        GizmoAxis::Z => Vector3::new(0.0, 0.0, 1.0),
    }
}

/// The axis a drag moved the most, as a Δ field index.
///
/// A single-axis drag names its axis; a plane drag names its dominant component, so a
/// `YZ` stroke with more travel on Y reopens the pill on `ΔY`. `None` for a stroke that
/// has not moved (a press or a release), which must not overwrite the memory.
fn axis_memory(delta: [f64; 3]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (axis, value) in delta.iter().enumerate() {
        if value.abs() < 1e-9 {
            continue;
        }
        let better = best.is_none_or(|slot| value.abs() > delta[slot].abs());
        if better {
            best = Some(axis);
        }
    }
    best
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
