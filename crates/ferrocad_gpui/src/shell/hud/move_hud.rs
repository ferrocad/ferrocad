//! The always-on Move surface.
//!
//! One row, two shapes: the arrows show three translation fields (`ΔX/ΔY/ΔZ`), the rings
//! show `ΔθX/ΔθY/ΔθZ` with only the active cell editable. The rotate form has **one**
//! angle because a rotation is an axis and an angle, not three independent coordinates;
//! while a step is pending its ring is locked, the siblings show `—`, and the only exits
//! are `✓`/`Enter` and `✗`/`Esc`. A `✗`/`✓` pair appears while either value is pending. It
//! is deliberately **not** an [`OpenDialog`](crate::shell::OpenDialog): the form belongs to the
//! hand, not to one command token, so it stays on screen across a commit and can coexist
//! with a command dialog. The commit itself still goes through `App_Move`
//! ([`Shell::commit_session`]) for a translation and `transform`/`apply` for a rotation,
//! and the session created by [`Shell::begin_drag_session`] remains the single owner of the
//! pending value.
//!
//! The translation fields are a **view**: relative mode shows the session delta, absolute
//! mode shows `origin + delta`, where `origin` is the gizmo origin frozen when the current
//! (clean) session started. Typing in absolute mode subtracts the origin, so the preview,
//! the tape and the commit always speak deltas. A per-field `=`/`@` prefix asks for an
//! absolute coordinate even in relative mode.
//!
//! See `docs/project/tasks/transform-session.md` §3.2.

use crate::shell::*;
use crate::shell::pill::AxisFrame;
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
pub(in crate::shell) const AXIS_COUNT: usize = 3;

/// The always-visible Move form.
///
/// `inputs` are the three translation fields; `angle` is the rotate form's single angle.
/// `origin` is the frozen world point absolute coordinates are measured from;
/// `orientation` is the body's committed orientation the local ring frame is built from.
/// `axis` is the ring a typed angle previews around. All live exactly as long as the
/// surface.
pub(in crate::shell) struct MoveHud {
    pub(in crate::shell) inputs: [Entity<TextInput>; AXIS_COUNT],
    /// The rotate form's only numeric field (`Δθ`). A rotation is one axis and one angle,
    /// so there is deliberately no `ΔθX/ΔθY/ΔθZ` triple.
    pub(in crate::shell) angle: Entity<TextInput>,
    /// Keeps the change/submit subscriptions alive for the surface's lifetime.
    _subs: Vec<Subscription>,
    /// The world point the absolute coordinates are measured from.
    origin: Vector3,
    /// The body's absolute orientation while no rotate session is live. The local ring
    /// frame and `Reset 0°` are measured from it.
    orientation: Rotation,
    /// The rotate ring a typed angle previews around (`0` = X, `1` = Y, `2` = Z).
    pub(in crate::shell) axis: usize,
}

impl Shell {
    /// Whether the Move surface is on screen.
    pub(in crate::shell) fn move_pill_open(&self) -> bool {
        self.move_hud.is_some()
    }

    /// Whether the form holds an uncommitted move.
    ///
    /// The session is the owner of the pending value, but it can only open once a stage
    /// reports a datum; a typed field before that (headless, or a stage still building)
    /// is still a pending edit. This is what materialises `✗`/`✓`: at rest it is false.
    pub(in crate::shell) fn move_is_pending(&self, cx: &App) -> bool {
        if self.angular() {
            return self.session.as_ref().is_some_and(TransformSession::is_dirty)
                || self.move_angle(cx).is_some_and(|angle| angle != 0.0);
        }
        self.session
            .as_ref()
            .is_some_and(TransformSession::is_dirty)
            || self
                .move_fields_delta(cx)
                .is_some_and(|delta| delta != [0.0; 3])
    }

    /// Whether the Move hand currently owns the form: Slot 1 equipped, a selection,
    /// and no tool taking it off the canvas. Both handle sets have a form: the arrows
    /// show `ΔX/ΔY/ΔZ` in millimetres, the rings show one `Δθ` in degrees.
    fn move_hud_wanted(&self) -> bool {
        self.loadout.slot().wants_gizmo()
            && self.active_tool.is_none()
            && !self.model.lock().unwrap().selection.is_empty()
    }

    /// Whether the form edits a rotation rather than a translation.
    pub(in crate::shell) fn angular(&self) -> bool {
        self.gizmo_mode == GizmoMode::Rotate
    }

    /// Show or hide the form for the current hand and selection.
    ///
    /// This is the one seam every context change calls (a selection, a slot, a tool,
    /// an edit). Opening is idempotent, so a re-arm while the form is open does not
    /// reset the values the user is working on.
    pub(in crate::shell) fn ensure_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.move_hud_wanted() {
            self.open_move_hud(cx);
        } else if self.move_hud.is_some() {
            self.close_move_hud(cx);
        }
    }

    /// Create the form, if it is not already up.
    ///
    /// The surface opens **unfocused**: the canvas keeps the keyboard, so `0`–`3`
    /// still equip loadout slots until the user asks for a field with `Tab`, a digit or
    /// a click. It also opens a clean session at the selection's current pose when one
    /// is not already live, so the first typed value has a datum.
    pub(in crate::shell) fn open_move_hud(&mut self, cx: &mut Context<Self>) {
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
        // The rotate form's single angle field. It has its own change handler because a
        // typed angle is a step around the selected ring, not a translation component.
        let angle = cx.new(|cx| {
            let mut input = TextInput::new(cx, "");
            input.set_text(ZERO_TEXT, cx);
            // A numeric field: letters are refused and reported, so `X`/`Y`/`Z` can
            // switch the ring while the caret stays in `Δθ`.
            input.set_char_filter(Some(is_angle_char));
            input
        });
        subs.push(cx.subscribe(
            &angle,
            |this: &mut Shell, _emitter, _event: &ChangeEvent, cx| {
                this.on_angle_typed(cx);
            },
        ));
        subs.push(cx.subscribe(
            &angle,
            |this: &mut Shell, _emitter, _event: &SubmitEvent, cx| {
                this.commit_session(cx);
            },
        ));
        subs.push(cx.subscribe(
            &angle,
            |this: &mut Shell, _emitter, event: &RejectedChar, cx| {
                this.on_angle_rejected(event.character, cx);
            },
        ));
        self.move_hud = Some(MoveHud {
            inputs,
            angle,
            _subs: subs,
            origin: Vector3::zero(),
            orientation: Rotation::identity(),
            axis: 0,
        });
        if self.session.is_none() {
            self.begin_drag_session(None);
        }
        self.refresh_move_origin(cx);
        // A fresh rotation session starts from the body's document orientation, so the
        // local ring frame and a reset have a base and the relative field starts clean.
        if self.angular() {
            let orientation = self
                .move_hud
                .as_ref()
                .map_or_else(Rotation::identity, |hud| hud.orientation);
            if let Some(session) = self.session.as_mut() {
                session.set_mode(GizmoMode::Rotate);
                if !session.is_dirty() {
                    session.set_orientation_0(orientation);
                }
            }
            self.set_move_fields([0.0; 3], cx);
        }
        self.pill.release();
        self.sync_gizmo_origin(cx);
        self.sync_transform_tape(cx);
        self.sync_locked_ring(cx);
        // A readout first: the canvas keeps the keyboard.
        self.restore_viewport_focus = true;
        cx.notify();
    }

    /// Drop the surface without writing anything (the hand changed, the selection went
    /// away, or a tool took over). A dirty session is committed by the caller first.
    pub(in crate::shell) fn close_move_hud(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.take().is_none() {
            return;
        }
        self.session = None;
        self.drag_start = None;
        self.rotation_start = None;
        self.pill.release();
        self.clear_preview_items(cx);
        self.sync_transform_tape(cx);
        self.sync_locked_ring(cx);
        self.sync_gizmo_origin(cx);
        // A focused field is going away with the surface.
        self.restore_viewport_focus = true;
        cx.notify();
    }

    /// `Esc`/`✗`: roll the pending move back, keeping the form.
    ///
    /// The form belongs to the Move hand, so it does not close; the fields return to the
    /// resting pose so the next nudge starts from zero.
    pub(in crate::shell) fn dismiss_move_hud(&mut self, cx: &mut Context<Self>) {
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
    pub(in crate::shell) fn reset_move_fields(&mut self, cx: &mut Context<Self>) {
        self.refresh_move_origin(cx);
        self.set_move_fields([0.0; 3], cx);
        self.pill.release();
    }

    /// Re-read the frozen origin, but only while no move is pending.
    ///
    /// While a session is dirty the origin is `origin_0` and must not move under the
    /// absolute fields; a commit or a rollback clears the session first. In absolute
    /// mode a changed origin rewrites the resting fields (`new_origin + 0`).
    pub(in crate::shell) fn refresh_move_origin(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.is_none()
            || self.session.as_ref().is_some_and(TransformSession::is_dirty)
        {
            return;
        }
        if self.angular() {
            // The local ring frame and a reset are measured from the body's own document
            // orientation, not a point.
            let orientation = self.object_rotation_now();
            let Some(hud) = self.move_hud.as_mut() else {
                return;
            };
            hud.orientation = orientation;
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

    /// The selected body's absolute rotation, when the document reports one.
    ///
    /// The stage is world-space, so the orientation is not on the item. One console
    /// expression is the smallest bridge; a missing `Placement` reads as identity.
    pub(in crate::shell) fn object_rotation_now(&self) -> Rotation {
        let name = self
            .model
            .lock()
            .unwrap()
            .selection
            .last()
            .map(|item| item.object.clone());
        name.as_deref()
            .and_then(crate::python::rotation_quaternion)
            .map(|q| Rotation { q, raw_axis: None })
            .unwrap_or_else(Rotation::identity)
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
    pub(in crate::shell) fn move_origin(&self) -> Option<Vector3> {
        self.move_hud.as_ref().map(|hud| hud.origin)
    }

    /// The text in one field.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::shell) fn move_field_text(&self, index: usize, cx: &App) -> Option<String> {
        let input = self.move_hud.as_ref()?.inputs.get(index)?;
        Some(input.read(cx).text().to_string())
    }

    /// Write one field (programmatic, so no change event fires).
    pub(in crate::shell) fn set_move_text(&self, index: usize, text: String, cx: &mut Context<Self>) {
        if let Some(input) = self.move_hud.as_ref().and_then(|hud| hud.inputs.get(index)) {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    /// The field values that *display* the pending translation under the current mode.
    ///
    /// The delta is world-space; the fields read it in the current axis frame. Absolute
    /// values are coordinates along that frame from its anchor (the world origin, the
    /// body's origin, or the workplane's origin).
    fn display_values(&self, delta: [f64; 3], cx: &App) -> [f64; 3] {
        let basis = self.frame_basis_now(cx);
        let world = Vector3::new(delta[0], delta[1], delta[2]);
        let coordinates = [
            world.dot(&basis[0]),
            world.dot(&basis[1]),
            world.dot(&basis[2]),
        ];
        if !self.pill.absolute() {
            return coordinates;
        }
        let origin = self
            .move_hud
            .as_ref()
            .map_or(Vector3::zero(), |hud| hud.origin);
        let base = origin.sub(&self.frame_anchor(cx));
        [
            coordinates[0] + base.dot(&basis[0]),
            coordinates[1] + base.dot(&basis[1]),
            coordinates[2] + base.dot(&basis[2]),
        ]
    }

    /// The transform form's basis, reading the active plane when `WP` is selected.
    pub(in crate::shell) fn frame_basis_now(&self, cx: &App) -> [Vector3; 3] {
        let plane = self.active_frame_plane(cx);
        self.frame_basis(plane.as_ref())
    }

    /// The world point the absolute fields measure from, in the current frame.
    ///
    /// `WLD` measures from the world origin, `LOC` from the body's own origin (so its
    /// coordinates are zero at rest), and `WP` from the workplane's origin.
    fn frame_anchor(&self, cx: &App) -> Vector3 {
        match self.pill.axis_frame() {
            AxisFrame::World => Vector3::zero(),
            AxisFrame::Local => self
                .move_hud
                .as_ref()
                .map_or(Vector3::zero(), |hud| hud.origin),
            AxisFrame::Workplane => self
                .active_frame_plane(cx)
                .map_or(Vector3::zero(), |plane| plane.origin),
        }
    }

    /// Mirror the pending value into the fields, in the current mode.
    ///
    /// Programmatic `set_text` does not emit a change event, so a drag cannot loop
    /// back into another preview. The rotate form has one field: the pending step's
    /// signed angle, or `0.00` while clean.
    pub(in crate::shell) fn set_move_fields(&self, delta: [f64; 3], cx: &mut Context<Self>) {
        if self.angular() {
            self.set_angle_text(self.display_angle(), cx);
            return;
        }
        let display = self.display_values(delta, cx);
        for (index, value) in display.iter().enumerate() {
            self.set_move_text(index, format!("{value:.2}"), cx);
        }
    }

    /// The angle the rotate field should show: the pending step, or zero while clean.
    fn display_angle(&self) -> f64 {
        self.session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(TransformSession::pending_degrees)
            .unwrap_or(0.0)
    }

    /// The text the rotate field currently holds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::shell) fn angle_field_text(&self, cx: &App) -> Option<String> {
        let input = &self.move_hud.as_ref()?.angle;
        Some(input.read(cx).text().to_string())
    }

    /// Write the single angle field (programmatic, so no change event fires).
    pub(in crate::shell) fn set_angle_text(&self, value: f64, cx: &mut Context<Self>) {
        if let Some(input) = self.move_hud.as_ref().map(|hud| &hud.angle) {
            input.update(cx, |input, cx| input.set_text(format!("{value:.2}"), cx));
        }
    }

    /// The angle the rotate field parses, if it is a complete number.
    pub(in crate::shell) fn move_angle(&self, cx: &App) -> Option<f64> {
        let input = &self.move_hud.as_ref()?.angle;
        let text = input.read(cx).text().trim().to_string();
        let number = text
            .strip_prefix('=')
            .or_else(|| text.strip_prefix('@'))
            .unwrap_or(&text)
            .trim();
        Quantity::parse(number).ok().map(|value| value.canonical_value())
    }

    /// The translation delta the fields represent, honouring absolute mode, a per-field
    /// `=`/`@`, and the current axis frame. The rotate form has no translation, so it
    /// returns `None`.
    ///
    /// The fields are read in the frame (a typed `10` in X is ten along the body's X, not
    /// the world's) and converted to the world-space delta the session stores. Returns
    /// `None` while a field does not parse (a lone `-`), so a half-typed value neither
    /// previews nor commits a silent zero.
    pub(in crate::shell) fn move_fields_delta(&self, cx: &App) -> Option<[f64; 3]> {
        let hud = self.move_hud.as_ref()?;
        if self.angular() {
            return None;
        }
        let basis = self.frame_basis_now(cx);
        let absolute = self.pill.absolute();
        let base = hud.origin.sub(&self.frame_anchor(cx));
        let mut coordinates = [0.0; 3];
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
            coordinates[index] = if absolute || marked {
                let offset = value - base.dot(&basis[index]);
                if offset.abs() <= DISPLAY_EPSILON {
                    0.0
                } else {
                    offset
                }
            } else {
                value
            };
        }
        let delta = basis[0]
            .scale(coordinates[0])
            .add(&basis[1].scale(coordinates[1]))
            .add(&basis[2].scale(coordinates[2]));
        Some([delta.x, delta.y, delta.z])
    }

    /// A programmatic translation-field write (the seed and the mode toggle) is not a
    /// change event, so the session and the preview are driven here.
    pub(in crate::shell) fn on_move_typed(&mut self, cx: &mut Context<Self>) {
        if self.angular() {
            return;
        }
        if let Some(value) = self.move_fields_delta(cx) {
            if self.session.is_none() {
                self.begin_drag_session(None);
            }
            if let Some(session) = self.session.as_mut() {
                session.set_mode(GizmoMode::Translate);
                session.set_translation(Vector3::new(value[0], value[1], value[2]));
            }
        }
        self.preview_move(false, cx);
        self.sync_transform_tape(cx);
        // A typed preview used to wait for the next pointer move to show.
        cx.notify();
    }

    /// Open (or re-base) the rotate session a typed angle edits.
    fn ensure_rotate_session(&mut self) {
        let orientation = self.object_rotation_now();
        if self.session.is_none() {
            self.begin_drag_session(None);
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.set_mode(GizmoMode::Rotate);
        if !session.is_dirty() {
            session.set_orientation_0(orientation);
        }
    }

    /// The axis a typed angle previews around, as `(index, world axis)`.
    ///
    /// The index is the bar's selected segment. `WLD` is the coordinate axis, `LOC` is
    /// that axis turned by the body's committed orientation, and `WP` is the active
    /// workplane's axis — exactly the direction the matching ring turns about.
    fn rotate_axis_world(&self, cx: &App) -> (usize, Vector3) {
        let index = self
            .move_hud
            .as_ref()
            .map_or(0, |hud| hud.axis.min(AXIS_COUNT - 1));
        let plane = self.active_frame_plane(cx);
        (index, self.frame_basis(plane.as_ref())[index])
    }

    /// The three world axis directions for the current frame mode.
    ///
    /// `World` is the coordinate grid, `LOC` is the body's committed orientation and
    /// `WP` is the active workplane's `u`/`v`/`n`. The plane is passed in because only
    /// the caller can read it from the viewport. The same basis orients the translate
    /// arrows and the rotate rings.
    pub(in crate::shell) fn frame_basis(
        &self,
        plane: Option<&ferrocad_stage::Workplane>,
    ) -> [Vector3; 3] {
        let world = [
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ];
        match self.pill.axis_frame() {
            AxisFrame::World => world,
            AxisFrame::Local => {
                ferrocad_stage::ManipulatorFrame::from_rotation(
                    Vector3::zero(),
                    self.object_rotation_now(),
                )
                .basis
            }
            AxisFrame::Workplane => plane
                .map(|plane| [plane.u, plane.v, plane.n])
                .unwrap_or(world),
        }
    }

    /// The active workplane while the form uses `WP` axes.
    ///
    /// `None` for the other frames, and when the shell has no viewport (headless),
    /// where `frame_basis` falls back to the world grid.
    pub(in crate::shell) fn active_frame_plane(&self, cx: &App) -> Option<ferrocad_stage::Workplane> {
        if self.pill.axis_frame() != AxisFrame::Workplane {
            return None;
        }
        let viewport = self.viewport.as_ref()?;
        Some(viewport.read(cx).active_workplane().plane.clone())
    }

    /// A typed angle in the rotate form: it is the whole pending step, replacing any
    /// drag value on the same ring.
    pub(in crate::shell) fn on_angle_typed(&mut self, cx: &mut Context<Self>) {
        if !self.angular() {
            return;
        }
        let Some(degrees) = self.move_angle(cx) else {
            // A half-typed value (`-`) is not a pending angle: the single field is the
            // numeric truth, so the session drops to clean instead of keeping a value the
            // bar no longer shows. Typing a complete number rebuilds the step.
            if let Some(session) = self.session.as_mut() {
                session.set_pending_degrees(0.0);
            }
            self.preview_move(false, cx);
            self.sync_transform_tape(cx);
            cx.notify();
            return;
        };
        self.ensure_rotate_session();
        // A pending step owns its ring; the bar cannot edit another one until it is
        // applied or cancelled. The guard is here as well as in the bar so a harness or a
        // future surface cannot create a second dirty axis.
        if let Some(locked) = self.rotation_pending() {
            let index = self.move_hud.as_ref().map_or(locked, |hud| hud.axis);
            if index != locked {
                self.show_rotation_lock(index, cx);
                return;
            }
        }
        let (index, axis) = self.rotate_axis_world(cx);
        if let Some(session) = self.session.as_mut() {
            session.set_mode(GizmoMode::Rotate);
            session.set_rotation_step(index, axis, degrees);
        }
        self.preview_move(false, cx);
        self.sync_transform_tape(cx);
        cx.notify();
    }

    /// A refused character in the angle field. `X`/`Y`/`Z` choose the ring while the
    /// caret stays in `Δθ`; a sibling is refused once a step is pending.
    pub(in crate::shell) fn on_angle_rejected(&mut self, character: char, cx: &mut Context<Self>) {
        if !self.angular() {
            return;
        }
        let index = match character.to_ascii_lowercase() {
            'x' => 0,
            'y' => 1,
            'z' => 2,
            _ => return,
        };
        if self.switch_rotate_axis(index, cx) {
            // The user is typing: the field keeps the keyboard.
            self.restore_viewport_focus = false;
            cx.notify();
        }
    }

    /// The axis of a dirty rotate step, if one is pending.
    ///
    /// While this is `Some`, the other two rings are locked: the viewport refuses their
    /// presses, and the bar's frame switch and sibling cells are inert. That keeps
    /// `dirty_ring_count <= 1`, so `✗`/`Esc` always cancels the exact step the protractor
    /// shows and no ring switch spends a hidden transaction.
    pub(in crate::shell) fn rotation_pending(&self) -> Option<usize> {
        self.session
            .as_ref()
            .filter(|session| session.is_dirty() && session.mode() == GizmoMode::Rotate)
            .and_then(TransformSession::pending_axis_index)
    }

    /// Explain why a ring, bar cell or the frame switch is refused right now.
    pub(in crate::shell) fn show_rotation_lock(&mut self, attempted: usize, cx: &mut Context<Self>) {
        let axis = self.rotation_pending().unwrap_or(attempted).min(AXIS_COUNT - 1);
        let axis = ["X", "Y", "Z"][axis];
        self.model.lock().unwrap().status = format!(
            "Pending rotate {axis} — apply (✓ / Enter) or cancel (✗ / Esc) before changing the axis"
        );
        cx.notify();
    }

    /// A ring press/hover the viewport refused because a pending rotation owns another
    /// axis.
    pub(in crate::shell) fn on_locked_ring(
        &mut self,
        axis: ferrocad_stage::GizmoAxis,
        cx: &mut Context<Self>,
    ) {
        let attempted = match axis {
            ferrocad_stage::GizmoAxis::X => 0,
            ferrocad_stage::GizmoAxis::Y => 1,
            ferrocad_stage::GizmoAxis::Z => 2,
        };
        self.show_rotation_lock(attempted, cx);
    }

    /// Push the session's locked ring into the viewport (the visuals and the press gate).
    ///
    /// Called whenever the pending step can change: a drag tick, a typed angle, a commit,
    /// a rollback or a re-arm. A clean session unlocks every ring.
    pub(in crate::shell) fn sync_locked_ring(&mut self, cx: &mut Context<Self>) {
        let locked = self.rotation_pending();
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.set_locked_ring(locked) {
                cx.notify();
            }
        });
    }

    /// Switch the bar's selected ring while the session is clean. A dirty step owns its
    /// ring, so the other cells are inert until it is applied or cancelled.
    /// Returns whether the axis changed.
    fn switch_rotate_axis(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if self.move_hud.is_none() || !self.angular() {
            return false;
        }
        let index = index.min(AXIS_COUNT - 1);
        if let Some(locked) = self.rotation_pending() {
            if locked != index {
                self.show_rotation_lock(index, cx);
            }
            return false;
        }
        let current = self.move_hud.as_ref().map_or(0, |hud| hud.axis);
        if current == index {
            return false;
        }
        if let Some(hud) = self.move_hud.as_mut() {
            hud.axis = index;
        }
        self.pill.remember_axis(index);
        self.set_move_fields([0.0; 3], cx);
        cx.notify();
        true
    }

    /// Click a cell in the rotate bar's `ΔθX/ΔθY/ΔθZ` row.
    ///
    /// With no pending step this chooses the ring the typed angle previews around. With a
    /// pending step it is refused unless it is the locked ring's own cell.
    pub(in crate::shell) fn select_rotate_axis(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owned = self.rotation_pending().is_some_and(|locked| locked == index);
        if self.switch_rotate_axis(index, cx) || owned {
            self.focus_move_field(window, cx);
        }
    }

    /// The body's committed orientation while no rotate session is live.
    ///
    /// This is what `Reset 0°` will target (the stored `Placement.Rotation` in the flat
    /// editions) and what the button labels itself with.
    pub(in crate::shell) fn stored_orientation(&self) -> Rotation {
        self.move_hud
            .as_ref()
            .map_or_else(Rotation::identity, |hud| hud.orientation)
    }

    /// The `Reset 0°` label and whether it is actionable.
    ///
    /// A stored identity disables the button. A pure X/Y/Z rotation names the axis and
    /// angle (`Reset (Z 45°)`); a compound 3-D tilt shows the true shortest angle from
    /// identity (`Reset (21.4°)`), never a fractional per-axis decomposition.
    pub(in crate::shell) fn reset_rotation_label(&self) -> (String, bool) {
        let orientation = self.stored_orientation();
        if orientation.is_same(&Rotation::identity(), 1e-9) {
            return ("Reset 0°".to_string(), false);
        }
        let (axis, degrees) = shortest_axis_angle(orientation);
        let name = if axis.x.abs() > 0.999 {
            Some("X")
        } else if axis.y.abs() > 0.999 {
            Some("Y")
        } else if axis.z.abs() > 0.999 {
            Some("Z")
        } else {
            None
        };
        let text = match name {
            Some(name) => format!("Reset ({name} {})", degrees_text(degrees)),
            None => format!("Reset ({})", degrees_text(degrees)),
        };
        (text, true)
    }

    /// `Reset 0°`: commit the rotation that cancels the body's committed orientation.
    ///
    /// While a step is pending this is refused: the user must apply or cancel the step
    /// they can see before a second transaction replaces it.
    pub(in crate::shell) fn reset_rotation(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.is_none() || !self.angular() {
            return;
        }
        if self.rotation_pending().is_some() {
            self.show_rotation_lock(self.move_hud.as_ref().map_or(0, |hud| hud.axis), cx);
            return;
        }
        let orientation = self.object_rotation_now();
        if orientation.is_same(&Rotation::identity(), 1e-9) {
            self.model.lock().unwrap().status = "rotation is already 0°".to_string();
            cx.notify();
            return;
        }
        if self.session.as_ref().is_some_and(TransformSession::is_dirty) {
            self.session = None;
            self.drag_start = None;
            self.rotation_start = None;
            self.clear_preview_items(cx);
            self.sync_transform_tape(cx);
        }
        self.ensure_rotate_session();
        let inverse = orientation.inverse();
        let mut degrees = inverse.angle().to_degrees();
        let mut axis = inverse.axis();
        if degrees > 180.0 {
            degrees -= 360.0;
            axis = axis.scale(-1.0);
        }
        if let Some(session) = self.session.as_mut() {
            session.set_rotation_axis_angle(axis, degrees);
        }
        self.preview_move(true, cx);
        self.commit_session(cx);
    }

    /// Preview the pending value on the selection, optionally **settling** the gizmo
    /// frame (a drag's `End`, where the pending pose becomes the resting pose).
    pub(in crate::shell) fn preview_move(&mut self, settle: bool, cx: &mut Context<Self>) {
        if self.angular() {
            let transform = self
                .session
                .as_ref()
                .filter(|session| session.is_dirty())
                .map(TransformSession::transform);
            self.preview_selection(transform, settle, cx);
            let sweeps = self.session_protractor(cx);
            self.publish_protractor(sweeps, cx);
            self.sync_locked_ring(cx);
            return;
        }
        let Some(delta) = self.pending_delta(cx) else {
            return;
        };
        self.preview_selection(
            Some(Matrix4::translation(delta[0], delta[1], delta[2])),
            settle,
            cx,
        );
    }

    /// Publish the rotate session's protractor sweeps (or clear them).
    pub(in crate::shell) fn publish_protractor(
        &mut self,
        sweeps: Option<Vec<RingSweep>>,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.set_protractor(sweeps) {
                cx.notify();
            }
        });
    }

    /// The one canonical wedge for the pending rotation step.
    ///
    /// A session holds exactly one `(axis, angle)`, so the protractor is one arc and one
    /// chip. The start spoke is the **ring's own zero**: the canonical direction turned by
    /// the committed orientation for a local ring frame, projected into the ring plane.
    /// The arc's value is the same signed scalar the docked field shows.
    pub(in crate::shell) fn session_protractor(&self, cx: &App) -> Option<Vec<RingSweep>> {
        let session = self
            .session
            .as_ref()
            .filter(|session| session.is_dirty() && session.mode() == GizmoMode::Rotate)?;
        let axis = session.pending_axis()?;
        let degrees = session.pending_degrees();
        if degrees.abs() < 0.05 {
            return None;
        }
        let viewport = self.viewport.as_ref()?.read(cx);
        let gizmo = viewport.gizmo()?;
        let radius = gizmo.scale();
        let detent_deg = gizmo.style().ring_snap_deg;
        let origin = session.datum();
        // The canonical zero direction for the ring: X turns from +Y, Y from +Z and Z
        // from +X. A local frame turns that direction with the body's committed pose.
        let canonical = match session.pending_axis_index() {
            Some(0) => Vector3::new(0.0, 1.0, 0.0),
            Some(1) => Vector3::new(0.0, 0.0, 1.0),
            Some(2) => Vector3::new(1.0, 0.0, 0.0),
            // A reset step has no coordinate ring; pick a stable direction perpendicular
            // to its axis so the wedge still has a visible zero spoke.
            _ => {
                let pick = if axis.x.abs() < 0.9 {
                    Vector3::new(1.0, 0.0, 0.0)
                } else {
                    Vector3::new(0.0, 1.0, 0.0)
                };
                pick.sub(&axis.scale(pick.dot(&axis))).normalize()
            }
        };
        let plane = self.active_frame_plane(cx);
        let basis = self.frame_basis(plane.as_ref());
        // The canonical zero direction is a world vector; the ring frame maps it onto
        // the frame the rings actually occupy.
        let turned = basis[0]
            .scale(canonical.x)
            .add(&basis[1].scale(canonical.y))
            .add(&basis[2].scale(canonical.z));
        let in_plane = turned.sub(&axis.scale(turned.dot(&axis)));
        let start = if in_plane.length() > 1e-6 {
            in_plane.normalize()
        } else {
            canonical
        };
        let current = Rotation::from_axis_angle(&axis, degrees.to_radians())
            .to_matrix()
            .transform(&start);
        Some(vec![RingSweep {
            origin,
            axis,
            start,
            current,
            radius,
            degrees,
            detent_deg,
        }])
    }

    /// The pending translation the form would show: the session when it is dirty, else
    /// the parsed fields.
    fn pending_delta(&self, cx: &App) -> Option<[f64; 3]> {
        if self.angular() {
            return None;
        }
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
    pub(in crate::shell) fn pending_translation(&self, cx: &App) -> Option<[f64; 3]> {
        if self.move_hud.is_none() || self.angular() {
            return None;
        }
        self.pending_delta(cx)
    }

    /// The pending angular step in degrees, when the rotate form is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::shell) fn pending_angle(&self) -> Option<f64> {
        if self.move_hud.is_none() || !self.angular() {
            return None;
        }
        self.session
            .as_ref()
            .filter(|session| session.is_dirty())
            .map(TransformSession::pending_degrees)
    }

    /// Cycle the form's axis basis (`WLD → LOC → WP`); the handles and the fields follow.
    ///
    /// A pending transform freezes the basis: its value was measured in the old frame,
    /// so the user finishes it with `✓`/`✗` first. Both hands share this switch.
    pub(in crate::shell) fn cycle_axis_frame(&mut self, cx: &mut Context<Self>) {
        if self.move_hud.is_none() {
            return;
        }
        if self.gate(Action::SwitchFrame, cx) {
            return;
        }
        // Read the value in the **old** frame before changing the basis. In coordinate
        // mode the field text is a position in the current frame, so reading it after
        // the switch would reinterpret the same numbers in the new frame: the fields
        // would not translate and the residual difference would read as a pending move.
        let delta = self.pending_delta(cx).unwrap_or_default();
        self.pill.cycle_axis_frame();
        // The pending value does not move; only its decomposition into the fields and
        // the handles' basis change.
        self.rearm_selection(cx);
        self.set_move_fields(delta, cx);
        cx.notify();
    }

    /// Cycle the rotate form's ring basis `WLD → LOC → WP`, or flip the translation form
    /// between offsets and coordinates.
    ///
    /// A frame switch is a context exit for a pending rotation: the pending step was
    /// measured about the old ring, so it is committed before the rings are rebuilt.
    pub(in crate::shell) fn toggle_move_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.move_hud.is_none() {
            return;
        }
        if self.angular() {
            if self.rotation_pending().is_some() {
                self.show_rotation_lock(self.move_hud.as_ref().map_or(0, |hud| hud.axis), cx);
                return;
            }
            self.pill.cycle_axis_frame();
            // The hosted rings take their frame from the arming seam, so rebuild them.
            self.rearm_selection(cx);
            self.set_move_fields([0.0; 3], cx);
            self.focus_move_field(window, cx);
            cx.notify();
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
    ///
    /// An explicit focus request cancels a pending hand-back to the viewport: the user
    /// asked for the field (a segment click, `Tab`, a digit), so the next render must not
    /// steal it back.
    pub(in crate::shell) fn focus_move_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(axis) = self.pill.focus() else {
            return;
        };
        let input = if self.angular() {
            self.move_hud.as_ref().map(|hud| &hud.angle)
        } else {
            self.move_hud.as_ref().and_then(|hud| hud.inputs.get(axis))
        };
        if let Some(input) = input {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            self.restore_viewport_focus = false;
        }
    }

    /// Put the keyboard in the field for `axis` (a click on a ruler tag).
    ///
    /// A tag from a drag that never opened the form has no field to focus, so the
    /// click is a no-op rather than an implicit open that would drop the session the
    /// tag is measuring. The `window.focus` is deferred to the next render
    /// (`pill_focus_request`), because a viewport event arrives outside a window.
    pub(in crate::shell) fn focus_move_axis(&mut self, axis: usize, cx: &mut Context<Self>) {
        if self.move_hud.is_none() {
            return;
        }
        // The rotate form has one field, so an arc tag focuses it without changing the
        // selected ring (a local ring's world axis need not spell its local label).
        let axis = if self.angular() { 0 } else { axis };
        self.pill.focus_axis(axis);
        self.pill_focus_request = Some(axis.min(AXIS_COUNT - 1));
        cx.notify();
    }

    /// `Tab`/`Shift+Tab`: open on the remembered drag axis, then cycle the fields. The
    /// rotate form has one field, so any `Tab` focuses it directly.
    pub(in crate::shell) fn cycle_move_focus(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_hud.is_none() {
            return;
        }
        if self.angular() {
            self.pill.focus_axis(0);
            self.focus_move_field(window, cx);
            self.select_move_field(0, cx);
            cx.notify();
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
    /// `0`–`3` never take this path — they are loadout hotkeys and the shell routes
    /// them first. The seed text is replaced wholesale, so `5` over `0.00` is `5`.
    pub(in crate::shell) fn type_into_move_field(
        &mut self,
        typed: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.move_hud.is_none() {
            return false;
        }
        if self.angular() {
            self.pill.focus_axis(0);
            self.set_angle_text(0.0, cx);
            if let Some(input) = self.move_hud.as_ref().map(|hud| &hud.angle) {
                input.update(cx, |input, cx| input.set_text(typed.to_string(), cx));
            }
            // `set_text` is programmatic (no change event), so drive the preview here.
            self.on_angle_typed(cx);
            self.focus_move_field(window, cx);
            cx.notify();
            return true;
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
    pub(in crate::shell) fn select_move_field(&self, axis: usize, cx: &mut Context<Self>) {
        let input = if self.angular() {
            self.move_hud.as_ref().map(|hud| &hud.angle)
        } else {
            self.move_hud.as_ref().and_then(|hud| hud.inputs.get(axis))
        };
        if let Some(input) = input {
            input.update(cx, |input, cx| input.select_all(cx));
        }
    }
}

/// The shortest axis-angle form of `rotation` (angle in `0..=180`).
fn shortest_axis_angle(rotation: Rotation) -> (Vector3, f64) {
    let mut degrees = rotation.angle().to_degrees();
    let mut axis = rotation.axis();
    if degrees > 180.0 {
        degrees = 360.0 - degrees;
        axis = axis.scale(-1.0);
    }
    (axis, degrees)
}

/// A compact degree label: `45°` for a whole value, `21.4°` otherwise.
fn degrees_text(degrees: f64) -> String {
    let rounded = (degrees * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 1e-9 {
        format!("{rounded:.0}°")
    } else {
        format!("{rounded:.1}°")
    }
}

/// The characters the rotate field accepts. Digits and a leading sign/decimal are the
/// numeric surface; `X`/`Y`/`Z` deliberately fall through to [`RejectedChar`] so they can
/// switch the ring while the field keeps the caret.
fn is_angle_char(character: char) -> bool {
    character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | ',' | '=' | '@')
}

/// The axis label for a translation field: absolute mode drops the `Δ` prefix.
pub(in crate::shell) fn axis_label(axis: usize, absolute: bool) -> String {
    let letter = ["X", "Y", "Z"][axis.min(AXIS_COUNT - 1)];
    if absolute {
        letter.to_string()
    } else {
        format!("Δ{letter}")
    }
}
