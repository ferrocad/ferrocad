//! The always-on Move surface.
//!
//! Three coordinate fields, an absolute/relative toggle and a `✗`/`✓` pair, shown
//! whenever the Move hand has a target. It is deliberately **not** an
//! [`OpenDialog`](super::OpenDialog): the form belongs to the hand, not to one command
//! token, so it stays on screen across a commit and can coexist with a command dialog.
//! The commit itself still goes through `App_Move` ([`Shell::commit_session`]), which
//! acquires a fresh token at that moment, and the session created by
//! [`Shell::begin_drag_session`] remains the single owner of the pending value.
//!
//! The fields are a **view**: relative mode shows the session delta, absolute mode
//! shows `origin + delta`, where `origin` is the gizmo origin frozen when the current
//! (clean) session started. Typing in absolute mode subtracts the origin, so the
//! preview, the tape and the commit always speak deltas. A per-field `=`/`@` prefix
//! asks for an absolute coordinate even in relative mode.
//!
//! See `docs/project/tasks/transform-session.md` §3.2.

use super::*;
use ferrocad_types::Quantity;

/// The text a resting field shows.
const ZERO_TEXT: &str = "0.00";

/// Half of the last digit [`ZERO_TEXT`] shows.
///
/// An absolute coordinate is written back as `origin_0 + delta` rounded to two
/// decimals, so parsing it against the exact origin leaves up to half a hundredth of
/// residue. That residue is the resting pose, not a typed move: without this a body
/// whose anchor is not round would show a pending move (and its `✗`/`✓`) at rest.
const DISPLAY_EPSILON: f64 = 0.005;

/// One field per axis, in X/Y/Z order.
pub(super) const AXIS_COUNT: usize = 3;

/// The always-visible Move form.
///
/// `inputs` are the three fields; `origin` is the frozen world point absolute
/// coordinates are measured from. Both live exactly as long as the surface.
pub(super) struct MoveHud {
    pub(super) inputs: [Entity<TextInput>; AXIS_COUNT],
    /// Keeps the change/submit subscriptions alive for the surface's lifetime.
    _subs: Vec<Subscription>,
    /// The world point the absolute coordinates are measured from.
    origin: Vector3,
}

impl Shell {
    /// Whether the Move surface is on screen.
    pub(super) fn move_pill_open(&self) -> bool {
        self.move_hud.is_some()
    }

    /// Whether the form holds an uncommitted move.
    ///
    /// The session is the owner of the pending value, but it can only open once a stage
    /// reports a datum; a typed field before that (headless, or a stage still building)
    /// is still a pending edit. This is what materialises `✗`/`✓`: at rest it is false.
    pub(super) fn move_is_pending(&self, cx: &App) -> bool {
        self.session
            .as_ref()
            .is_some_and(TransformSession::is_dirty)
            || self
                .move_fields_delta(cx)
                .is_some_and(|delta| delta != [0.0; 3])
    }

    /// Whether the Move hand currently owns the form: Slot 1 equipped, a selection,
    /// the translate handles showing, and neither a tool nor the user's pause
    /// (`1` while the hand is already active) taking it off the canvas.
    fn move_hud_wanted(&self) -> bool {
        !self.move_disarmed
            && self.loadout.slot().wants_gizmo()
            && self.gizmo_mode == GizmoMode::Translate
            && self.active_tool.is_none()
            && !self.model.lock().unwrap().selection.is_empty()
    }

    /// Show or hide the form for the current hand and selection.
    ///
    /// This is the one seam every context change calls (a selection, a slot, a tool,
    /// an edit). Opening is idempotent, so a re-arm while the form is open does not
    /// reset the values the user is working on.
    pub(super) fn ensure_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.move_hud_wanted() {
            self.open_move_hud(cx);
        } else if self.move_hud.is_some() {
            self.close_move_hud(cx);
        }
    }

    /// Create the form, if it is not already up.
    ///
    /// The surface opens **unfocused**: the canvas keeps the keyboard, so `1`/`2`/`3`
    /// still equip loadout slots until the user asks for a field with `Tab`, a digit or
    /// a click. It also opens a clean session at the selection's current pose when one
    /// is not already live, so the first typed value has a datum.
    pub(super) fn open_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.is_some() {
            self.refresh_move_origin(cx);
            return;
        }
        let mut subs = Vec::new();
        let inputs = std::array::from_fn(|_| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(cx, "");
                input.set_text(ZERO_TEXT, cx);
                input
            });
            // Every edit is the move: it replaces the drag's exact delta and updates the
            // session that owns it. The fields are unfocused on open, but a programmatic
            // edit still drives the same path.
            subs.push(cx.subscribe(
                &input,
                |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                    this.on_move_typed(cx);
                },
            ));
            // Enter in a field commits, the same path as ✓.
            subs.push(cx.subscribe(
                &input,
                |this: &mut Shell, _emitter, _event: &SubmitEvent, cx| {
                    this.commit_session(cx);
                },
            ));
            input
        });
        self.move_hud = Some(MoveHud {
            inputs,
            _subs: subs,
            origin: Vector3::zero(),
        });
        if self.session.is_none() {
            self.begin_drag_session(None);
        }
        self.refresh_move_origin(cx);
        self.pill.release();
        self.sync_gizmo_origin(cx);
        self.sync_transform_tape(cx);
        // A readout first: the canvas keeps the keyboard.
        self.restore_viewport_focus = true;
        cx.notify();
    }

    /// Drop the surface without writing anything (the hand changed, the selection went
    /// away, or a tool took over). A dirty session is committed by the caller first.
    pub(super) fn close_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.take().is_none() {
            return;
        }
        self.session = None;
        self.drag_start = None;
        self.pill.release();
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
        self.sync_gizmo_origin(cx);
        // A focused field is going away with the surface.
        self.restore_viewport_focus = true;
        cx.notify();
    }

    /// `Esc`/`✗`: roll the pending move back, keeping the form.
    ///
    /// The form belongs to the Move hand, so it does not close; the fields return to the
    /// resting pose so the next nudge starts from zero.
    pub(super) fn dismiss_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.session.as_ref().is_some_and(TransformSession::is_dirty) {
            self.drop_session("move cancelled", cx);
        } else {
            self.reset_move_fields(cx);
            cx.notify();
        }
        self.pill.release();
        self.restore_viewport_focus = true;
    }

    /// Return the fields to the resting pose after a commit or a rollback.
    ///
    /// The session must already be gone (or clean): the origin is re-read from the
    /// gizmo, which by then sits at the committed pose.
    pub(super) fn reset_move_fields(&mut self, cx: &mut Context<Self>) {
        self.refresh_move_origin(cx);
        self.set_move_fields([0.0; 3], cx);
        self.pill.release();
    }

    /// Re-read the frozen origin, but only while no move is pending.
    ///
    /// While a session is dirty the origin is `origin_0` and must not move under the
    /// absolute fields; a commit or a rollback clears the session first. In absolute
    /// mode a changed origin rewrites the resting fields (`new_origin + 0`).
    pub(super) fn refresh_move_origin(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.is_none()
            || self.session.as_ref().is_some_and(TransformSession::is_dirty)
        {
            return;
        }
        let origin = self.move_origin_now(cx);
        let Some(hud) = self.move_hud.as_mut() else {
            return;
        };
        let changed = hud.origin != origin;
        hud.origin = origin;
        if changed && self.pill.absolute() {
            self.set_move_fields([0.0; 3], cx);
        }
    }

    /// The world point the absolute fields are measured from: the selection's gizmo
    /// origin when it is armed (a re-pivot included), else the item's own anchor.
    fn move_origin_now(&self, cx: &App) -> Vector3 {
        let primary = self.model.lock().unwrap().selection.last().cloned();
        let entity = primary
            .as_ref()
            .and_then(|item| self.display.entity_of(&item.object));
        if let (Some(viewport), Some(entity)) = (self.viewport.as_ref(), entity) {
            let viewport = viewport.read(cx);
            if let Some(gizmo) = viewport.gizmo().filter(|gizmo| gizmo.target_id() == entity) {
                return gizmo.origin();
            }
        }
        if let Some(entity) = entity
            && let Some(anchor) = self
                .last_stage
                .as_ref()
                .and_then(|stage| stage.item(entity))
                .and_then(ShapeItem::anchor)
        {
            return anchor;
        }
        Vector3::zero()
    }

    /// The origin the absolute fields are measured from (tests and the ruler-tag focus).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn move_origin(&self) -> Option<Vector3> {
        self.move_hud.as_ref().map(|hud| hud.origin)
    }

    /// The text in one field.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn move_field_text(&self, index: usize, cx: &App) -> Option<String> {
        let input = self.move_hud.as_ref()?.inputs.get(index)?;
        Some(input.read(cx).text().to_string())
    }

    /// Write one field (programmatic, so no change event fires).
    pub(super) fn set_move_text(&self, index: usize, text: String, cx: &mut Context<Self>) {
        if let Some(input) = self.move_hud.as_ref().and_then(|hud| hud.inputs.get(index)) {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    /// The field values that *display* `delta` under the current mode.
    fn display_values(&self, delta: [f64; 3]) -> [f64; 3] {
        let origin = self.move_hud.as_ref().map_or(Vector3::zero(), |hud| hud.origin);
        if self.pill.absolute() {
            [
                delta[0] + origin.x,
                delta[1] + origin.y,
                delta[2] + origin.z,
            ]
        } else {
            delta
        }
    }

    /// Mirror `delta` into the fields, in the current mode.
    ///
    /// Programmatic `set_text` does not emit a change event, so a drag cannot loop
    /// back into another preview.
    pub(super) fn set_move_fields(&self, delta: [f64; 3], cx: &mut Context<Self>) {
        let display = self.display_values(delta);
        for (index, value) in display.iter().enumerate() {
            self.set_move_text(index, format!("{value:.2}"), cx);
        }
    }

    /// The delta the fields represent, honouring the mode and a per-field `=`/`@`.
    ///
    /// Returns `None` while a field does not parse (a lone `-`), so a half-typed value
    /// neither previews nor commits a silent zero.
    pub(super) fn move_fields_delta(&self, cx: &App) -> Option<[f64; 3]> {
        let hud = self.move_hud.as_ref()?;
        let origin = hud.origin;
        let absolute = self.pill.absolute();
        let mut delta = [0.0; 3];
        for (index, input) in hud.inputs.iter().enumerate() {
            let text = input.read(cx).text().trim().to_string();
            let (number, marked) = match text.strip_prefix('=') {
                Some(rest) => (rest.trim(), true),
                None => match text.strip_prefix('@') {
                    Some(rest) => (rest.trim(), true),
                    None => (text.as_str(), false),
                },
            };
            let value = Quantity::parse(number).ok()?.canonical_value();
            delta[index] = if absolute || marked {
                let offset = value - component(origin, index);
                if offset.abs() <= DISPLAY_EPSILON {
                    0.0
                } else {
                    offset
                }
            } else {
                value
            };
        }
        Some(delta)
    }

    /// A programmatic field write (the seed and the mode toggle) is not a change event,
    /// so the session and the preview are driven here.
    pub(super) fn on_move_typed(&mut self, cx: &mut Context<Self>) {
        if let Some(delta) = self.move_fields_delta(cx) {
            if self.session.is_none() {
                self.begin_drag_session(None);
            }
            if let Some(session) = self.session.as_mut() {
                session.set_translation(Vector3::new(delta[0], delta[1], delta[2]));
            }
        }
        self.preview_move(false, cx);
        self.sync_transform_tape(cx);
        // A typed preview used to wait for the next pointer move to show.
        cx.notify();
    }

    /// Preview the pending delta on the selection, optionally **settling** the gizmo
    /// frame (a drag's `End`, where the pending pose becomes the resting pose).
    pub(super) fn preview_move(&mut self, settle: bool, cx: &mut Context<Self>) {
        let Some(delta) = self.pending_delta(cx) else {
            return;
        };
        self.preview_selection(Some(delta), settle, cx);
    }

    /// The pending translation the form would show: the session when it is dirty, else
    /// the parsed fields.
    fn pending_delta(&self, cx: &App) -> Option<[f64; 3]> {
        self.session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(|session| {
                let delta = session.delta();
                [delta.x, delta.y, delta.z]
            })
            .or_else(|| self.move_fields_delta(cx))
    }

    /// The pending translation a Move will commit, when a form is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn pending_translation(&self, cx: &App) -> Option<[f64; 3]> {
        if self.move_hud.is_none() {
            return None;
        }
        self.pending_delta(cx)
    }

    /// Flip between `ΔX/ΔY/ΔZ` and `X/Y/Z`, rewriting the fields in the new mode.
    pub(super) fn toggle_move_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.move_hud.is_none() {
            return;
        }
        // Read the value in the *old* mode before flipping.
        let delta = self.pending_delta(cx).unwrap_or_default();
        self.pill.toggle_absolute();
        self.set_move_fields(delta, cx);
        self.focus_move_field(window, cx);
        cx.notify();
    }

    /// Put the caret in the field the [`PillState`](PillState) names.
    pub(super) fn focus_move_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(axis) = self.pill.focus() else {
            return;
        };
        if let Some(input) = self.move_hud.as_ref().and_then(|hud| hud.inputs.get(axis)) {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Put the keyboard in the field for `axis` (a click on a ruler tag).
    ///
    /// A tag from a drag that never opened the form has no field to focus, so the
    /// click is a no-op rather than an implicit open that would drop the session the
    /// tag is measuring. The `window.focus` is deferred to the next render
    /// (`pill_focus_request`), because a viewport event arrives outside a window.
    pub(super) fn focus_move_axis(&mut self, axis: usize, cx: &mut Context<Self>) {
        if self.move_hud.is_none() {
            return;
        }
        self.pill.focus_axis(axis);
        self.pill_focus_request = Some(axis.min(AXIS_COUNT - 1));
        cx.notify();
    }

    /// `Tab`/`Shift+Tab`: open on the remembered drag axis, then cycle the fields.
    pub(super) fn cycle_move_focus(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_hud.is_none() {
            return;
        }
        self.pill.tab(backward);
        self.focus_move_field(window, cx);
        // `Tab` enters a field to replace its value: select the seed, so `5` over
        // `0.00` is `5` rather than `0.005`.
        if let Some(axis) = self.pill.focus() {
            self.select_move_field(axis, cx);
        }
        cx.notify();
    }

    /// A numeric key on the resting form: focus the remembered field and start the
    /// number there.
    ///
    /// `1`/`2`/`3` never take this path — they are loadout hotkeys and the shell routes
    /// them first. The seed text is replaced wholesale, so `5` over `0.00` is `5`.
    pub(super) fn type_into_move_field(
        &mut self,
        typed: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.move_hud.is_none() {
            return false;
        }
        let axis = self
            .pill
            .focus()
            .unwrap_or_else(|| self.pill.last_axis())
            .min(AXIS_COUNT - 1);
        self.pill.focus_axis(axis);
        self.set_move_text(axis, typed.to_string(), cx);
        // `set_text` is programmatic (no change event), so drive the preview here.
        self.on_move_typed(cx);
        self.focus_move_field(window, cx);
        cx.notify();
        true
    }

    /// Select a field's text so the next keystroke replaces it.
    pub(super) fn select_move_field(&self, axis: usize, cx: &mut Context<Self>) {
        if let Some(input) = self.move_hud.as_ref().and_then(|hud| hud.inputs.get(axis)) {
            input.update(cx, |input, cx| input.select_all(cx));
        }
    }
}

/// One component of a world vector.
fn component(vector: Vector3, axis: usize) -> f64 {
    match axis {
        0 => vector.x,
        1 => vector.y,
        _ => vector.z,
    }
}

/// The axis ink and label for a field: relative mode keeps the `Δ` prefix.
pub(super) fn axis_label(axis: usize, absolute: bool) -> String {
    let letter = ["X", "Y", "Z"][axis.min(AXIS_COUNT - 1)];
    if absolute {
        letter.to_string()
    } else {
        format!("Δ{letter}")
    }
}
