//! `Shell` methods: viewport.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;
use super::picking::click_target;
use ferrocad_stage::Stage;

impl Shell {
    /// Re-read the document's shapes into the viewport's stage.
    ///
    /// Call after any edit that can change geometry: a property commit, a console
    /// snippet, or an interaction commit (which syncs itself). An empty document or a
    /// build without a display workbench simply yields an empty stage.
    pub(super) fn sync_stage(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        match self.build_stage() {
            Ok(stage) => viewport.update(cx, |viewport, cx| {
                viewport.set_stage(stage);
                cx.notify();
            }),
            Err(error) => self.console_line(format!("stage: {error}"), cx),
        }
    }

    /// Pull the registered overlay scenes into the viewport when the set changed (G5).
    ///
    /// The registry is the authority; this is deliberately a pull so the shell never
    /// re-extracts or recompiles a scene whose overlay set is unchanged. Registration
    /// is enough to draw: the viewport does not gate the program queue on
    /// `grid_overlay`, which only hides the native ground. With no
    /// viewport (headless, or before [`enable_viewport`](Self::enable_viewport)) the
    /// dirty flag is left set, so the next viewport applies it.
    pub(super) fn sync_overlays(&mut self, cx: &mut Context<Self>) {
        if !ferrocad_inventor_py::overlays_dirty() {
            return;
        }
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let overlays = ferrocad_inventor_py::overlay_scenes();
        let error = viewport.update(cx, |viewport, cx| {
            viewport.set_overlays(overlays);
            cx.notify();
            viewport.take_shader_error()
        });
        if let Some(error) = error {
            self.console_line(format!("shader: {error}"), cx);
        }
    }

    /// The WGSL skin the active `gizmo` provider supplies, if any.
    ///
    /// The registry is the only channel: the shader ships with a mod, and asking the
    /// active backend means a provider switch cannot leave a stale skin behind. A
    /// bridge failure is logged and treated as "no skin" (the native material).
    pub(super) fn active_gizmo_skin(&mut self, cx: &mut Context<Self>) -> Option<Arc<str>> {
        match python::provider_skin(GIZMO_FEATURE) {
            Ok(source) => source.map(Arc::from),
            Err(error) => {
                self.console_line(format!("gizmo skin: {error}"), cx);
                None
            }
        }
    }

    /// Pull the active provider's gizmo skin into the viewport.
    ///
    /// Returns whether a source was supplied. A missing viewport still reports that
    /// state: it is simply not applied until [`enable_viewport`](Self::enable_viewport)
    /// runs. The viewport is notified only when the stored program changed.
    pub(super) fn sync_gizmo_skin(&mut self, cx: &mut Context<Self>) -> bool {
        let skin = self.active_gizmo_skin(cx);
        let supplied = skin.is_some();
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                if viewport.set_gizmo_skin(skin) {
                    cx.notify();
                }
            });
        }
        supplied
    }

    /// Ask the app script for the display meshes and build a stage from them.
    ///
    /// The shell names no workbench: the script routes `stage_meshes` through
    /// `FerroCAD.display_mesh`, so whatever module owns the objects supplies the mesh.
    pub(super) fn build_stage(&mut self) -> Result<ferrocad_stage::Stage, String> {
        let meshes: Vec<StageMesh> = python::stage_meshes()?;
        Ok(self.display.build(meshes))
    }

    /// React to a viewport event: a click selection, a manipulator commit, or a
    /// gizmo drag.
    pub(super) fn on_viewport_event(&mut self, event: &ViewportEvent, cx: &mut Context<Self>) {
        match event {
            ViewportEvent::Selection(selection) => self.on_selection(selection, cx),
            ViewportEvent::Interaction(commit) => self.on_interaction(commit, cx),
            ViewportEvent::Transform(event) => self.on_interaction(&event.to_interaction(), cx),
            ViewportEvent::TransformPreview(preview) => self.on_transform_preview(preview, cx),
            ViewportEvent::GizmoOrigin { point, kind } => self.on_gizmo_origin(*point, *kind, cx),
            ViewportEvent::ToolState(status) => self.on_tool_state(status, cx),
        }
    }

    /// The origin/centre handle was released: remember the snapped pivot as the session
    /// user origin. This is not a transform commit and never touches the document.
    pub(super) fn on_gizmo_origin(&mut self, point: Vector3, kind: SnapKind, cx: &mut Context<Self>) {
        self.user_origin = Some(point);
        self.model.lock().unwrap().status = format!("pivot: {}", snap_name(kind));
        cx.notify();
    }

    /// A tool started, gained a point, finished or was cancelled: mirror its status
    /// into the model so the tool strip shows the label, point count and hint, and
    /// hides as soon as the session is over. The document is never touched here.
    pub(super) fn on_tool_state(&mut self, status: &ToolStatus, cx: &mut Context<Self>) {
        let active = status.is_active();
        if !active {
            self.active_tool = None;
        }
        self.model.lock().unwrap().tool = active.then(|| status.clone());
        cx.notify();
    }

    /// A click: select the object in the inspector, and arm the manipulators and gizmo
    /// the edition's workbenches and mods contribute for that selection.
    pub(super) fn on_selection(&mut self, selection: &SelectionEvent, cx: &mut Context<Self>) {
        // A selection change invalidates the object verbs the wheel was built for.
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
        let (manipulators, gizmo) = match selection {
            SelectionEvent::Picked {
                reference,
                click_count,
                ..
            } => {
                // A double click on a face or edge promotes the pick to the whole
                // body; a single click keeps the sub-element the picker resolved.
                let reference = &click_target(reference, *click_count);
                if let Some(name) = self.display.object_of(reference.object).map(str::to_string) {
                    // The viewport reports sub-elements too (`Face6`), which the facade
                    // snapshot carries for `getSelectionEx`.
                    self.replace_selection(
                        vec![SelectionItem::with_elements(
                            name,
                            reference.elements.clone(),
                        )],
                        cx,
                    );
                }
                self.arm(reference, cx)
            }
            SelectionEvent::Cleared => {
                self.clear_selection(cx);
                (Vec::new(), None)
            }
        };
        let armed = gizmo.is_some();
        self.model.lock().unwrap().gizmo_armed = armed;
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(manipulators);
                viewport.set_gizmo(gizmo);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// The object verbs the radial menu shows, in compass order.
    ///
    /// `target` is the wheel's context target (the object under the pointer), or `None`
    /// when the pointer is over empty space. The registry decides which commands are
    /// object verbs (`Surface::Radial`); the shell only orders them so the destructive
    /// slot is last (south) and a disabled slot stays visibly muted.
    ///
    /// A target **outside** the current selection is not selected yet (opening a menu is
    /// not a mutation), so the registry's selection-dependent `IsActive` cannot be the
    /// gate. Its verbs are offered and the target is promoted when one is chosen; the
    /// command then validates against a selection that contains it.
    pub(super) fn radial_slots(&self, target: Option<&SelectionRef>) -> Vec<WheelSlot> {
        let model = self.model.lock().unwrap();
        let groups = model.surfaces();
        let target_selected = target.map_or(true, |target| {
            self.display
                .object_of(target.object)
                .is_some_and(|name| model.selection.iter().any(|item| item.object == name))
        });
        let pending = target.is_some() && !target_selected;
        let mut slots: Vec<WheelSlot> = groups
            .radial
            .iter()
            .filter(|command| !command.dev_only)
            .map(|command| WheelSlot {
                id: command.id.clone().into(),
                label: command.short_label().into(),
                tone: match command.tone.as_str() {
                    "additive" => WheelTone::Additive,
                    "destructive" => WheelTone::Destructive,
                    _ => WheelTone::Structural,
                },
                enabled: command.enabled || pending,
            })
            .collect();
        if !slots.iter().any(|slot| slot.enabled) {
            // Context: nothing to manipulate. The same gesture still deserves a
            // wheel, so offer the creation verbs from the dock (the "creation /
            // global" context). With a selection, the object verbs above win.
            slots = groups
                .dock
                .iter()
                .filter(|command| !command.dev_only)
                .map(|command| WheelSlot {
                    id: command.id.clone().into(),
                    label: command.short_label().into(),
                    tone: WheelTone::Structural,
                    enabled: command.enabled,
                })
                .collect();
        }
        // Stable sort: structural, additive, destructive. With the two built-ins this
        // puts Move north and Delete south, so "flick down to delete" never moves.
        slots.sort_by_key(|slot| match slot.tone {
            WheelTone::Structural => 0,
            WheelTone::Additive => 1,
            WheelTone::Destructive => 2,
        });
        slots
    }

    /// Resolve the object under a **window-space** pointer position without changing
    /// the selection.
    ///
    /// Opening a menu is not a mutation: the wheel's context target is remembered and
    /// promoted only when an object verb is chosen. `hover_report` is normally current
    /// because pointer moves keep it so, but a press can arrive without an intervening
    /// move (a trackpad tap can even deliver `Down` and `Up` before the next move), and
    /// the press never reaches the viewport's own pick because the canvas consumes it,
    /// so this re-picks through the same G-buffer path a click uses.
    pub(super) fn wheel_target_at(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<WheelTarget> {
        let viewport = self.viewport.clone()?;
        let (reference, anchor) = viewport.update(cx, |viewport, cx| {
            if viewport.handle_pointer_at(position) {
                cx.notify();
            }
            (
                viewport.hover_report().target.clone(),
                viewport.hover_anchor(),
            )
        });
        Some(WheelTarget {
            reference: reference?,
            anchor,
        })
    }

    /// Open the radial menu **armed** by a button press. The sidecar owns the
    /// flick-vs-tap decision; the shell supplies the anchor, the context target and the
    /// slots.
    pub(crate) fn open_radial(&mut self, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        // A press is a pointer event of its own: resolve the context at its position,
        // but leave the selection alone until a verb is chosen.
        self.wheel_target = self.wheel_target_at(anchor, cx);
        let slots = self.radial_slots(self.wheel_target.as_ref().map(|target| &target.reference));
        let wheel = self.wheel.clone();
        let opened = wheel.update(cx, |wheel, cx| wheel.open_armed(anchor, slots, cx));
        if opened {
            self.open_popover = None;
        }
    }

    /// Open the radial menu **latched**, for a keyboard trigger.
    pub(super) fn open_radial_latched(&mut self, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        let slots = self.radial_slots(self.wheel_target.as_ref().map(|target| &target.reference));
        let wheel = self.wheel.clone();
        let opened = wheel.update(cx, |wheel, cx| wheel.open_latched(anchor, slots, cx));
        if opened {
            self.open_popover = None;
        }
    }

    /// Commit a wheel choice: promote its context target (if any), then run the verb.
    ///
    /// The promotion is what makes a context menu act on the pointed object without
    /// the mere act of opening it changing the selection. The target is one-shot; a
    /// cancel discards it in the `Closed` handler.
    pub(super) fn commit_wheel(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(target) = self.wheel_target.take() {
            let already_selected =
                self.display
                    .object_of(target.reference.object)
                    .is_some_and(|name| {
                        self.model
                            .lock()
                            .unwrap()
                            .selection
                            .iter()
                            .any(|item| item.object == name)
                    });
            if !already_selected {
                // A promotion changes the selection subject; a session pivot belonged to
                // the old subject, so it goes away. (`replace_selection` clears it too;
                // this also covers a host whose display map cannot name the target.)
                self.user_origin = None;
                self.on_selection(
                    &SelectionEvent::Picked {
                        reference: target.reference,
                        anchor: target.anchor,
                        click_count: 1,
                    },
                    cx,
                );
            }
        }
        self.run_command(id, cx);
    }

    /// Close the radial menu; returns whether one was open.
    pub(crate) fn close_radial(&mut self, cx: &mut Context<Self>) -> bool {
        let wheel = self.wheel.clone();
        wheel.update(cx, |wheel, cx| wheel.close(cx))
    }

    /// Forward a pointer move to the sidecar (it owns the hover).
    pub(super) fn hover_radial(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.pointer_move(position, cx));
    }

    /// Forward the arming release (the sidecar commits, latches or cancels).
    pub(super) fn release_radial(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.pointer_up(position, cx));
    }

    /// The `Space` path: commit a hovered latched wedge, close an open wheel, or open
    /// **contextually** — the entity under the pointer is the context target, so the key
    /// is spatial rather than a blind global shortcut.
    pub(super) fn space_radial(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let wheel = self.wheel.clone();
        if wheel.read(cx).is_open() {
            let committed = wheel.update(cx, |wheel, cx| wheel.commit_hovered(cx));
            if !committed {
                let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
            }
            return true;
        }

        // `Space` acts on what is under the pointer, not on a global notion of "the
        // selection". Re-pick at the last pointer through the viewport's G-buffer
        // path, but only as a context target: the selection changes when a verb is
        // chosen, not when the menu opens.
        self.wheel_target = self
            .last_pointer
            .and_then(|position| self.wheel_target_at(position, cx));

        let anchor = self.last_pointer.unwrap_or_else(|| {
            let size = window.viewport_size();
            point(px(size.width.0 / 2.0), px(size.height.0 / 2.0))
        });
        self.open_radial_latched(anchor, cx);
        true
    }

    /// The manipulators and gizmo to host for a selection.
    ///
    /// The compiled workbench manipulator (if any) comes first; the handles a mod
    /// declares for this selection follow it, in provider order. The viewport picks
    /// the most recently added manipulator first, so a declared handle takes the
    /// pointer before the native one. One selection arms exactly the compiled
    /// manipulator plus every declared handle; no handle survives a selection
    /// change.
    ///
    /// A bad declaration is logged and dropped, never allowed to cost the native
    /// manipulator: conversion errors are appended to the console after the valid
    /// handles were collected.
    ///
    /// The shell always hosts (arms) the native manipulator/gizmo for a selection, even
    /// when [`GizmoStyle::enabled`](ferrocad_stage::GizmoStyle::enabled) is false: the viewport
    /// owns that gate, and a disabled style keeps the objects hosted while drawing,
    /// hover, picking and drag servicing are suppressed. Keeping one authority for the
    /// switch means re-enabling the gizmo from settings restores the handles without a
    /// re-selection.
    ///
    /// The stage is cloned out of the viewport before anything is updated: a re-entrant
    /// `update` on the same entity panics.
    pub(super) fn arm(
        &mut self,
        reference: &SelectionRef,
        cx: &mut Context<Self>,
    ) -> (Vec<Box<dyn Manipulator>>, Option<TransformGizmo>) {
        let mut manipulators: Vec<Box<dyn Manipulator>> = Vec::new();
        let mut gizmo = None;
        if let Some(viewport) = self.viewport.as_ref() {
            let stage = viewport.read(cx).stage().clone();
            if !self.contributions.is_empty() {
                if let Some(manipulator) = self.contributions.manipulator_for(reference, &stage) {
                    manipulators.push(manipulator);
                }
                // The factory owns the datum: the triad hugs the object's own origin
                // (`ShapeItem::anchor`), which also makes it an origin indicator and the
                // pivot a rotate gizmo needs. A click point is deliberately not used —
                // for translation the pivot does not change the result, and re-pivoting
                // will be an explicit, snapped gesture (see transform-ux.md).
                gizmo = self.armed_gizmo(reference, &stage);
            }
        }

        // Mod-declared handles come after the compiled one. The selection's object
        // name and first sub-element are what a provider needs to decide.
        let object = self
            .display
            .object_of(reference.object)
            .unwrap_or_default()
            .to_string();
        let element = reference.elements.first().map(String::as_str);
        match python::manipulator_specs(&object, element) {
            Ok(specs) => {
                let mut errors = Vec::new();
                for spec in specs {
                    match manipulators::handle_from_value(&spec, reference.object) {
                        Ok((target, handle)) => {
                            manipulators
                                .push(Box::new(DeclarativeManipulator::new(target, handle)));
                        }
                        Err(error) => errors.push(error),
                    }
                }
                for error in errors {
                    self.console_line(error, cx);
                }
            }
            Err(error) => {
                self.console_line(format!("manipulator specs unavailable: {error}"), cx);
            }
        }

        (manipulators, gizmo)
    }

    /// The edition's transform gizmo for `reference`, evaluated against `stage` and
    /// carrying the session policy.
    ///
    /// The factory owns the datum: the triad hugs the object's own origin
    /// (`ShapeItem::anchor`). The shell applies the sticky handle set (`gizmo_mode`)
    /// and, while set, the session user origin. Split out of [`arm`](Self::arm) so the
    /// pivot policy is testable without a GPU viewport; a whole-body selection and a
    /// sub-element selection resolve to the same body origin.
    pub(super) fn armed_gizmo(
        &self,
        reference: &SelectionRef,
        stage: &Stage,
    ) -> Option<TransformGizmo> {
        // The factory's datum is the object origin; the handle set is the shell's
        // session mode (every factory returns a fresh Translate gizmo). A session user
        // origin replaces the datum while set, so a re-arm keeps the user's pivot.
        let mut gizmo = self.contributions.gizmo_for(reference, stage)?;
        gizmo.set_mode(self.gizmo_mode);
        let object_origin = gizmo.origin();
        gizmo.set_origin(self.gizmo_origin_for(object_origin));
        gizmo.set_origin_enabled(self.gizmo_origin_enabled());
        Some(gizmo)
    }

    /// The pivot the triad should use for an object whose factory reported
    /// `object_origin`: the session user origin while set, else that datum.
    pub(super) fn gizmo_origin_for(&self, object_origin: Vector3) -> Vector3 {
        self.user_origin.unwrap_or(object_origin)
    }

    /// Whether the origin/centre handle is enabled. The floating Move HUD is anchored
    /// to the visible origin, so the pivot handle is disabled while its translation
    /// form is open; closing the form re-enables it.
    pub(super) fn gizmo_origin_enabled(&self) -> bool {
        !self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
    }

    /// Drop the session pivot and rebuild the selection's triad at the object origin.
    /// Returns whether a user origin was set.
    pub(super) fn clear_user_origin(&mut self, cx: &mut Context<Self>) -> bool {
        if self.user_origin.take().is_none() {
            return false;
        }
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.cancel_gizmo_origin_drag();
                cx.notify();
            });
        }
        // The factory reports the object origin, so a re-arm restores the frame.
        self.rearm_selection(cx);
        cx.notify();
        true
    }

    /// Switch the transform gizmo's handle set from a command's `gizmo` directive.
    ///
    /// The mode is session state: it survives re-selection and re-arming (the next
    /// `arm` applies it), and `Move` resets it to translate because that command's
    /// fields describe a translation. Switching away from translate cancels an open
    /// translation HUD: its Δ fields would no longer describe the visible handles.
    pub(super) fn set_gizmo_mode(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mode) = gizmo_mode_named(name) else {
            self.console_line(format!("gizmo: unknown mode {name:?}"), cx);
            return;
        };
        if self.gizmo_mode == mode {
            return;
        }
        if mode != GizmoMode::Translate
            && self
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.spec.previews_translation())
        {
            self.cancel_dialog(cx);
        }
        self.gizmo_mode = mode;
        // A viewport with no gizmo armed keeps the stored mode and applies it at the
        // next `arm`; one that does rebuilds its handle overlay now.
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                if viewport.set_gizmo_mode(mode) {
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    /// A manipulator drag ended: apply the commit through the app script's
    /// `interaction` hook, recompute, and re-read the shapes.
    pub(super) fn on_interaction(&mut self, commit: &InteractionCommit, cx: &mut Context<Self>) {
        // Any document interaction supersedes a pending Move HUD: the height grip may
        // have resized the body, and the pending delta is stale. Close the dialog (and
        // its token) so it cannot apply on top of the committed change.
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
            && let Some(dialog) = self.dialog.take()
        {
            let _ = commands::cancel(&dialog.token);
            self.clear_dialog_preview(cx);
        }
        let Some(object) = self.display.object_of(commit.target).map(str::to_string) else {
            let status = format!("no document object for entity {}", commit.target);
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        };
        let status = match python::apply_interaction(commit, &object) {
            Ok(result) if result.ok => {
                self.after_document_edit(cx);
                match (commit.element.as_deref(), commit.readout.is_empty()) {
                    (Some(element), false) => {
                        format!("{element} {} = {}", commit.action, commit.readout)
                    }
                    (Some(element), true) => format!("{element} {}", commit.action),
                    (None, false) => commit.readout.clone(),
                    (None, true) => commit.action.clone(),
                }
            }
            Ok(result) if result.error.is_empty() => "interaction failed".to_string(),
            Ok(result) => format!("interaction error: {}", result.error),
            Err(error) => format!("interaction error: {error}"),
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Refresh the model panes and the stage after the document changed.
    pub(super) fn after_document_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(viewport) = self.viewport.clone() {
            match self.build_stage() {
                Ok(stage) => viewport.update(cx, |viewport, cx| {
                    viewport.set_stage(stage);
                    viewport.set_manipulators(Vec::new());
                    viewport.set_gizmo(None);
                    cx.notify();
                }),
                Err(error) => self.console_line(format!("stage: {error}"), cx),
            }
        }
        // A recompute prunes names the document lost; re-arm only what survives.
        self.refresh(cx);
        self.rearm_selection(cx);
    }

    /// Re-arm the current selection's overlays after the geometry changed.
    ///
    /// A move or a resize invalidates the world anchors the handles were built for. The
    /// height grip must follow the moved body's top face, and the transform triad must
    /// stay attached after its own commit — so this rebuilds from the (already pruned)
    /// selection against the new stage. The factory re-reads the object's origin, so the
    /// triad tracks the geometry without any shell-side bookkeeping.
    pub(super) fn rearm_selection(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        if self.active_tool.is_some() {
            return;
        }
        let reference = {
            let model = self.model.lock().unwrap();
            model.selection.last().and_then(|primary| {
                let object = self.display.entity_of(&primary.object)?;
                Some(SelectionRef {
                    object,
                    elements: primary.elements.clone(),
                })
            })
        };
        let Some(reference) = reference else {
            self.model.lock().unwrap().gizmo_armed = false;
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(Vec::new());
                viewport.set_gizmo(None);
                cx.notify();
            });
            return;
        };
        let (manipulators, gizmo) = self.arm(&reference, cx);
        self.model.lock().unwrap().gizmo_armed = gizmo.is_some();
        viewport.update(cx, |viewport, cx| {
            viewport.set_manipulators(manipulators);
            viewport.set_gizmo(gizmo);
            cx.notify();
        });
    }

    /// Re-read the workbench commands and their enablement.
    ///
    /// Called after anything that can change `IsActive()`: a selection, a document
    /// edit, undo/redo, an open/close. It is deliberately not per frame — each call
    /// crosses into the interpreter.
    pub(super) fn refresh_commands(&mut self, cx: &mut Context<Self>) {
        match commands::list() {
            Ok(commands) => {
                self.shortcuts = global_shortcuts(&commands);
                self.model.lock().unwrap().commands = commands;
            }
            Err(error) => self.console_line(format!("commands: {error}"), cx),
        }
    }

}
