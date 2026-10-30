//! `Shell` methods: viewport.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;
use super::picking::click_target;
use ferrocad_stage::{
    GizmoSet, ShapeItem, Stage, WorkplaneLive, WorkplaneStem, WORKPLANE_OFFSET_ID,
};

/// The `Offset Workplane…` HUD's field that mirrors the session's pending offset.
const WORKPLANE_FIELD: &str = "offset";

impl Shell {
    /// Re-read the document's shapes into the viewport's stage.
    ///
    /// Call after any edit that can change geometry: a property commit, a console
    /// snippet, or an interaction commit (which syncs itself). An empty document or a
    /// build without a display workbench simply yields an empty stage.
    pub(super) fn sync_stage(&mut self, cx: &mut Context<Self>) {
        if self.viewport.is_none() {
            return;
        }
        match self.build_stage() {
            Ok(stage) => {
                // The Python view facade picks over exactly this stage; publish it with
                // the entity → object names before the viewport takes ownership.
                self.publish_stage(&stage);
                self.set_stage(stage, cx);
                self.publish_camera(cx);
            }
            Err(error) => self.console_line(format!("stage: {error}"), cx),
        }
    }

    /// Hand `stage` (the document's items) to the viewport with the transient previews.
    ///
    /// The document stage is kept so a preview change can redraw without re-reading the
    /// tessellations (`stage_meshes` crosses into the kernel). A stage with no preview
    /// published is handed over as built — the common case pays nothing.
    pub(super) fn set_stage(&mut self, stage: Stage, cx: &mut Context<Self>) {
        let generation = ferrocad_stage_py::preview_generation();
        self.preview_generation = generation;
        let merged = self.stage_with_previews(&stage);
        self.last_stage = Some(stage);
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.set_stage(merged);
                cx.notify();
            });
        }
    }

    /// The draw stage: the document's items plus the published preview items.
    pub(super) fn stage_with_previews(&self, stage: &Stage) -> Stage {
        if ferrocad_stage_py::preview_generation() == 0 {
            return stage.clone();
        }
        let mut merged = stage.clone();
        for item in ferrocad_stage_py::preview_items() {
            merged.add_item(item);
        }
        merged
    }

    /// Redraw when a script published or dropped preview items (S4.6).
    ///
    /// A preview changes without a document edit, so [`sync_stage`] would not run and
    /// the drawn stage would keep the old visual. The generation check is a mutex read;
    /// only a real change clones the document stage.
    pub(super) fn sync_previews(&mut self, cx: &mut Context<Self>) {
        // A `Offset Workplane…` HUD that closed without accepting has cancelled its
        // session: put the plane back. This runs before the cheap generation check
        // because a repaint is the first moment after Esc that the shell sees.
        self.reconcile_workplane_session(cx);
        // The stem's tick has already landed by the time a frame is painted (the
        // viewport observation runs first); this covers a drag that finished between
        // frames so the field never lags a release.
        self.sync_workplane_drag(cx);
        let generation = ferrocad_stage_py::preview_generation();
        if generation == self.preview_generation {
            return;
        }
        self.preview_generation = generation;
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let Some(stage) = self.last_stage.clone() else {
            return;
        };
        let merged = self.stage_with_previews(&stage);
        viewport.update(cx, |viewport, cx| {
            viewport.set_stage(merged);
            cx.notify();
        });
    }

    /// Publish the camera the Python view facade reports (`getCameraNode`, …).
    ///
    /// The shell owns the camera and the facade cannot see an `InventorViewport`, so
    /// the live camera is mirrored into `ferrocad_stage` (S4.1). Called from every
    /// shell render and from each viewport event, so a script that runs in response to
    /// a click sees the camera that click was made with.
    pub(super) fn publish_camera(&self, cx: &App) {
        let Some(viewport) = self.viewport.as_ref() else {
            return;
        };
        ferrocad_stage_py::publish_camera(viewport.read(cx).camera().clone());
    }

    /// Publish the camera **and**, when nothing has been published yet, the stage.
    ///
    /// `enable_viewport` builds the first stage itself (it is the boot path), so the
    /// stage the viewport is drawing may never have gone through [`sync_stage`]. This
    /// render-path publish closes that gap once; afterwards `sync_stage` owns it, so a
    /// frame never clones a stage.
    pub(super) fn publish_view(&self, cx: &App) {
        let Some(viewport) = self.viewport.as_ref() else {
            return;
        };
        let viewport = viewport.read(cx);
        ferrocad_stage_py::publish_camera(viewport.camera().clone());
        if !ferrocad_stage_py::has_stage() {
            self.publish_stage(viewport.stage());
        }
    }

    /// Publish the compiled display stage and its entity → object names.
    ///
    /// `publish_stage` compiles the stage for the pick **once per sync** (not per pick),
    /// so `getObjectsInfo` walks the same packets the viewport picks against. The
    /// sub-element name tables come from the items themselves (the kernel
    /// tessellation's `parts`).
    pub(super) fn publish_stage(&self, stage: &Stage) {
        let objects = stage
            .items()
            .iter()
            .filter_map(|item| {
                self.display
                    .object_of(item.entity_id())
                    .map(|name| (item.entity_id(), name.to_string()))
            })
            .collect();
        ferrocad_stage_py::publish_stage(stage, objects, None);
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
        // The facade's camera snapshot is refreshed before anything can reach Python:
        // a command or tool that reacts to this event reports the camera it happened in.
        self.publish_camera(cx);
        match event {
            ViewportEvent::Selection(selection) => self.on_selection(selection, cx),
            ViewportEvent::Interaction(commit) => self.on_interaction(commit, cx),
            ViewportEvent::Transform(event) => self.on_interaction(&event.to_interaction(), cx),
            ViewportEvent::TransformPreview(preview) => self.on_transform_preview(preview, cx),
            ViewportEvent::GizmoOrigin { point, kind } => self.on_gizmo_origin(*point, *kind, cx),
            ViewportEvent::ToolState(status) => self.on_tool_state(status, cx),
        }
        // A tool or a mod event callback can select through the facade; applying the
        // queue here keeps that write in the same frame as the event that caused it.
        self.drain_selection_requests(cx);
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
        // A selection change is a competing interaction: it closes the offset HUD and
        // puts the plane back where the session started (`selection-gizmos-and-forms.md`
        // §5.3). A `float` is not dismissed by clicking the canvas, so this is the
        // selection event, not the click.
        if self.workplane_hud_open() {
            self.cancel_dialog(cx);
            self.end_workplane_session(false, cx);
        }
        // A selection change invalidates the object verbs the wheel was built for.
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
        let (manipulators, gizmos) = match selection {
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
                (Vec::new(), GizmoSet::default())
            }
        };
        let armed = !gizmos.is_empty();
        self.model.lock().unwrap().gizmo_armed = armed;
        let mode = self.gizmo_mode;
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(manipulators);
                viewport.set_gizmos(gizmos);
                // The shell's session kind is authoritative: a viewport created after
                // the verb was chosen (a headless run that gained a GPU) starts at the
                // default and must catch up here.
                viewport.set_gizmo_mode(mode);
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
        let mut slots: Vec<WheelSlot> = model
            .commands
            .iter()
            .filter(|command| !command.dev_only)
            .filter(|command| {
                command.wheel
                    || commands::Surface::parse(&command.surface) == Some(commands::Surface::Radial)
            })
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

    /// The manipulators and gizmos to host for a selection.
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
    /// The shell always hosts (arms) the native manipulator/gizmo set for a selection,
    /// even when [`GizmoStyle::enabled`](ferrocad_stage::GizmoStyle::enabled) is false:
    /// the viewport owns that gate, and a disabled style keeps the objects hosted while
    /// drawing, hover, picking and drag servicing are suppressed. Keeping one authority
    /// for the switch means re-enabling the gizmo from settings restores the handles
    /// without a re-selection.
    ///
    /// The stage is cloned out of the viewport before anything is updated: a re-entrant
    /// `update` on the same entity panics.
    pub(super) fn arm(
        &mut self,
        reference: &SelectionRef,
        cx: &mut Context<Self>,
    ) -> (Vec<Box<dyn Manipulator>>, GizmoSet) {
        let mut manipulators: Vec<Box<dyn Manipulator>> = Vec::new();
        let mut gizmos = GizmoSet::default();
        let mut body_datum: Option<Vector3> = None;
        if let Some(viewport) = self.viewport.as_ref() {
            let stage = viewport.read(cx).stage().clone();
            body_datum = stage.item(reference.object).and_then(ShapeItem::anchor);
            if !self.contributions.is_empty() {
                if let Some(manipulator) = self.contributions.manipulator_for(reference, &stage) {
                    manipulators.push(manipulator);
                }
                // The factory owns the datum: the triad hugs the object's own origin
                // (`ShapeItem::anchor`), which also makes it an origin indicator and the
                // pivot a rotate gizmo needs. A click point is deliberately not used —
                // for translation the pivot does not change the result, and re-pivoting
                // will be an explicit, snapped gesture (see transform-ux.md).
                gizmos = self.arm_gizmos(reference, &stage);
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

        // A claimed sub-element owns the handles (workplanes note, §3.3): Part's face
        // gets its extrude stem and its edge the fillet arc, so the shared whole-body
        // triad — the entry anchored at the body datum — steps aside. Only the *shared*
        // triad yields: a workbench gizmo anchored elsewhere (a grip on the top face)
        // stays. When nothing claims the element the triad stays too, so a click never
        // ends up with no handles. `Move`/`Rotate` are explicit requests for the body
        // control and override the default until the selection changes.
        if !self.transform_verb && reference.elements.first().is_some() && !manipulators.is_empty() {
            let pivot = self.user_origin;
            gizmos = gizmos.retain(|gizmo| {
                let origin = gizmo.origin();
                Some(origin) != body_datum && Some(origin) != pivot
            });
        }

        // An accepted Move ends its session and takes the shared triad with it (upstream
        // closes the task panel's handles the same way). A workbench gizmo anchored
        // elsewhere is not the session's, so it stays.
        if self.triad_dismissed && !self.transform_verb {
            let pivot = self.user_origin;
            gizmos = gizmos.retain(|gizmo| {
                let origin = gizmo.origin();
                Some(origin) != body_datum && Some(origin) != pivot
            });
        }

        (manipulators, gizmos)
    }

    /// The edition's transform gizmos for `reference`, in the registry's hit-priority
    /// order, evaluated against `stage`.
    ///
    /// This is the hosted **set**: a workbench gizmo and the shared triad can coexist,
    /// and the first entry is the one that answers the pointer. The viewport narrows it
    /// to the session kind (the wheel's `Move`/`Rotate` verbs) and re-narrows it in place
    /// when the verb changes, so the kinds an edition registered are never lost.
    pub(super) fn armed_gizmos(&self, reference: &SelectionRef, stage: &Stage) -> GizmoSet {
        GizmoSet::new(self.contributions.gizmos_for(reference, stage))
    }

    /// The registry set with the **session policy** applied to every entry: the
    /// re-pivot's user origin replaces the factory datum while it is set, and the
    /// origin/centre handle is disabled while the translation HUD is open.
    ///
    /// The policy is applied per entry rather than after narrowing, because the viewport
    /// hosts the whole set: an entry the current verb does not offer is still a candidate
    /// for the next one and must already carry the datum. The session pivot re-anchors
    /// only the controls that sit **at the object's own datum** (the shared triad and any
    /// workbench control anchored with it); a workbench gizmo that owns a different
    /// anchor (a grip on the top face) keeps it, because the pivot is a property of the
    /// triad, not of every handle in the set.
    pub(super) fn arm_gizmos(&self, reference: &SelectionRef, stage: &Stage) -> GizmoSet {
        let mut gizmos = self.armed_gizmos(reference, stage);
        let datum = stage.item(reference.object).and_then(ShapeItem::anchor);
        for gizmo in gizmos.gizmos_mut() {
            let object_origin = gizmo.origin();
            if datum == Some(object_origin) {
                gizmo.set_origin(self.gizmo_origin_for(object_origin));
            }
            gizmo.set_origin_enabled(self.gizmo_origin_enabled());
        }
        gizmos
    }

    /// The edition's transform gizmo for `reference`, as a **single** control: the
    /// registry set narrowed to one kind with the session policy applied.
    ///
    /// Kept alongside [`arm_gizmos`](Self::arm_gizmos) for a host that shows one gizmo
    /// (and for the pivot tests): the set form is what the viewport hosts.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn armed_gizmo(
        &self,
        reference: &SelectionRef,
        stage: &Stage,
    ) -> Option<TransformGizmo> {
        // The registry's set is composed by the edition; the session kind narrows it. A
        // session user origin replaces the datum while set, so a re-arm keeps the user's
        // pivot.
        let mut gizmo = self.armed_gizmos(reference, stage).narrow(self.gizmo_mode)?;
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
        // The active plane's offset stem is transient viewport state: the shell applies
        // it itself, before any entity lookup and without the `interaction` hook. No
        // document object is involved and nothing is written.
        if commit.manipulator == WORKPLANE_OFFSET_ID {
            self.on_workplane_offset(commit, cx);
            return;
        }
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
        let Some(object) = (if commit.creates {
            // A *creating* commit has no entity yet: the workbench makes the object,
            // so there is nothing to look up and the hook receives no name.
            Some(String::new())
        } else {
            self.display.object_of(commit.target).map(str::to_string)
        }) else {
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
        if self.viewport.is_some() {
            match self.build_stage() {
                Ok(stage) => {
                    self.publish_stage(&stage);
                    self.set_stage(stage, cx);
                    if let Some(viewport) = self.viewport.clone() {
                        viewport.update(cx, |viewport, cx| {
                            viewport.set_manipulators(Vec::new());
                            viewport.set_gizmo(None);
                            cx.notify();
                        });
                    }
                }
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
        // The offset session owns the pointer while its HUD is open: keep its stem on
        // the live plane instead of handing the handles back to the selection.
        if self.workplane_hud_open() {
            self.arm_workplane_stem(cx);
            return;
        }
        // An accepted `Offset Workplane…` HUD ends its session here: the value is
        // already applied, so the plane keeps it and only the stem goes away.
        self.end_workplane_session(true, cx);
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
        let (manipulators, gizmos) = self.arm(&reference, cx);
        self.model.lock().unwrap().gizmo_armed = !gizmos.is_empty();
        let mode = self.gizmo_mode;
        viewport.update(cx, |viewport, cx| {
            viewport.set_manipulators(manipulators);
            viewport.set_gizmos(gizmos);
            viewport.set_gizmo_mode(mode);
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

    // -- W2: the `Offset Workplane…` session (stem + floating HUD) -------------

    /// Arm the active plane's `+N` stem (the `workplane_stem` directive).
    ///
    /// The stem is the gesture half of the offset session and the HUD is the typed
    /// half; both live in the viewport's transient state. A re-arm after a commit —
    /// the plane moved, so the stem must sit on it again — keeps the session's base;
    /// a fresh command starts a new one.
    pub(super) fn arm_workplane_stem(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let (plane, offset) = {
            let active = viewport.read(cx).active_workplane();
            (active.plane.clone(), active.offset)
        };
        let continuing = self.workplane_hud_open();
        let base = match (continuing, self.workplane_session.as_ref()) {
            (true, Some(session)) => session.base,
            _ => offset,
        };
        // The stem measures from the total the HUD shows: a drag tick is
        // `armed total + travel`, so the field mirrors the whole session rather than
        // the last gesture (a second drag continues the first one's number).
        let total = offset - base;
        let live = WorkplaneLive::new(total);
        self.workplane_session = Some(WorkplaneSession {
            base,
            live: live.clone(),
            field: Some(total),
        });
        viewport.update(cx, |viewport, cx| {
            viewport.set_manipulators(vec![Box::new(WorkplaneStem::tracked(&plane, live))]);
            cx.notify();
        });
        cx.notify();
    }

    /// The offset the live `Offset Workplane…` session started from, if one is open.
    pub(super) fn workplane_session_base(&self) -> Option<f64> {
        self.workplane_session.as_ref().map(|session| session.base)
    }

    /// Whether the open dialog is the `Offset Workplane…` HUD.
    fn workplane_hud_open(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.drives_workplane_offset())
    }

    /// Mirror the session's pending offset into the HUD's field.
    ///
    /// `TextInput::set_text` is programmatic, so this cannot re-emit a change event
    /// and loop back into the field.
    fn set_workplane_hud_value(&mut self, total: f64, cx: &mut Context<Self>) {
        if !self.workplane_hud_open() {
            return;
        }
        let input = {
            let Some(dialog) = self.dialog.as_ref() else {
                return;
            };
            let Some(index) = dialog
                .spec
                .fields
                .iter()
                .position(|field| field.name == WORKPLANE_FIELD)
            else {
                return;
            };
            dialog.inputs.get(index).and_then(Option::as_ref).cloned()
        };
        if let Some(input) = input {
            let text = format!("{total:.2}");
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
        if let Some(session) = self.workplane_session.as_mut() {
            session.field = Some(total);
        }
    }

    /// Mirror a live stem drag into the HUD's field (the drag→field half).
    ///
    /// The stem writes its tick total into the session's [`WorkplaneLive`], and the
    /// shell calls this from the viewport observation (a drag tick notifies the
    /// viewport but emits no event) and from the render path. The field is written
    /// only when the displayed total actually changes, and `set_text` is programmatic,
    /// so this cannot loop back through the input's change event.
    pub(super) fn sync_workplane_drag(&mut self, cx: &mut Context<Self>) {
        let Some(total) = self.workplane_session.as_ref().and_then(|s| s.live.total()) else {
            return;
        };
        let Some(shown) = self.workplane_session.as_ref().and_then(|s| s.field) else {
            return;
        };
        // The field is rendered with two decimals; a smaller move cannot change it.
        if (shown - total).abs() < 5e-3 {
            return;
        }
        if !self.workplane_hud_open() {
            return;
        }
        self.set_workplane_hud_value(total, cx);
    }

    /// Drive the plane from the offset HUD's typed total (the field→stem half).
    ///
    /// Typing moves the plane live, like the Move HUD's translation preview: the
    /// value is transient viewport state, so nothing is written and `Esc` still
    /// restores the session's base. Text that does not parse yet (a lone `-`, an
    /// empty field) leaves the plane where it is rather than snapping it back to the
    /// session base on every keystroke.
    pub(super) fn preview_workplane_offset(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.workplane_session_base() else {
            return;
        };
        let Some(total) = self.workplane_field_total(cx) else {
            return;
        };
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let (changed, plane, offset) = viewport.update(cx, |viewport, cx| {
            let current = viewport.active_workplane().offset;
            let changed = viewport.offset_workplane(base + total - current);
            if changed {
                cx.notify();
            }
            let active = viewport.active_workplane();
            (changed, active.plane.clone(), active.offset)
        });
        if !changed {
            return;
        }
        // The stem sits on the plane it offsets, so a typed move takes it along. Keep
        // the session's live handle: a later drag continues from the typed total.
        if let Some(live) = self.workplane_session.as_ref().map(|s| s.live.clone()) {
            live.set_base(offset - base);
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(vec![Box::new(WorkplaneStem::tracked(&plane, live))]);
                cx.notify();
            });
        }
        if let Some(session) = self.workplane_session.as_mut() {
            session.field = Some(offset - base);
        }
        cx.notify();
    }

    /// The offset HUD's field as a number, while its text parses.
    fn workplane_field_total(&self, cx: &App) -> Option<f64> {
        let dialog = self.dialog.as_ref()?;
        let index = dialog
            .spec
            .fields
            .iter()
            .position(|field| field.name == WORKPLANE_FIELD)?;
        let input = dialog.inputs.get(index).and_then(Option::as_ref)?;
        let text = input.read(cx).text().trim().to_string();
        let value: f64 = text.parse().ok()?;
        value.is_finite().then_some(value)
    }

    /// A workplane stem was released: apply its signed distance to the active plane.
    ///
    /// This is the one manipulator commit the shell applies itself. The offset is
    /// viewport state, so there is no entity to resolve (`commit.target` is therefore
    /// never used) and no `interaction` hook to call.
    fn on_workplane_offset(&mut self, commit: &InteractionCommit, cx: &mut Context<Self>) {
        let delta = commit.values.first().copied().unwrap_or(0.0);
        let Some(viewport) = self.viewport.clone() else {
            let status = "workplane: no viewport".to_string();
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        };
        let (applied, label, offset, plane) = viewport.update(cx, |viewport, cx| {
            let applied = viewport.offset_workplane(delta);
            if applied {
                cx.notify();
            }
            let active = viewport.active_workplane();
            (
                applied,
                active.display_label(),
                active.offset,
                active.plane.clone(),
            )
        });
        // The stem sits on the plane it offsets, so it follows the moved origin; the
        // next drag measures from there. The session's live handle survives the re-arm,
        // re-based on the new total, so the next drag keeps mirroring into the field.
        if let Some(live) = self.workplane_session.as_ref().map(|session| session.live.clone()) {
            if let Some(base) = self.workplane_session_base() {
                live.set_base(offset - base);
            }
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(vec![Box::new(WorkplaneStem::tracked(&plane, live))]);
                cx.notify();
            });
        }
        // The HUD shows the session's total, so a drag and a typed number describe the
        // same pending offset.
        if let Some(base) = self.workplane_session_base() {
            self.set_workplane_hud_value(offset - base, cx);
        }
        let status = if applied {
            format!("workplane: {label}")
        } else {
            "the workplane offset is unchanged".to_string()
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// End the `Offset Workplane…` session.
    ///
    /// `keep` is the accept path: the offset is already applied, so only the stem
    /// goes. Otherwise (a cancelled HUD, `Esc`) the plane returns to the offset the
    /// session began with. Returns whether a session was active.
    pub(super) fn end_workplane_session(&mut self, keep: bool, cx: &mut Context<Self>) -> bool {
        let base = self.workplane_session.take().map(|session| session.base);
        let Some(base) = base else {
            return false;
        };
        let Some(viewport) = self.viewport.clone() else {
            return true;
        };
        let restored = viewport.update(cx, |viewport, cx| {
            let changed = if keep {
                false
            } else {
                let current = viewport.active_workplane().offset;
                viewport.offset_workplane(base - current)
            };
            if changed {
                cx.notify();
            }
            // The stem belonged to the session; the selection's handles come back on
            // the next arm.
            viewport.set_manipulators(Vec::new());
            cx.notify();
            changed.then(|| viewport.active_workplane().display_label())
        });
        if let Some(label) = restored {
            let status = format!("workplane: {label}");
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
        }
        true
    }

    /// End a session whose HUD closed without accepting.
    ///
    /// `Esc` (and any other dismissal) drops the dialog through the shell's shared
    /// cancel path, which the shell cannot intercept itself; the next repaint is the
    /// first moment it can, and a transient viewport value restored then is never
    /// drawn out of place.
    pub(super) fn reconcile_workplane_session(&mut self, cx: &mut Context<Self>) {
        if self.workplane_session_base().is_none() {
            return;
        }
        if self.workplane_hud_open() {
            return;
        }
        self.end_workplane_session(false, cx);
    }
}
