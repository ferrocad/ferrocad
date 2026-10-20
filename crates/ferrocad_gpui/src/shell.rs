//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a 3D viewport, a property editor
//! and a Python console. The layout and the interpreter wiring are the S1 deliverable:
//! the tree, editor and console are already driven by the live document through
//! `python/ferrocad_shell`; the viewport renders the sample scene until the geometry
//! kernel/`ViewProvider`s produce real geometry; richer widgets, commands and menus
//! come in later slices (see `docs/app-shell-vision.md`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, Subscription, Window, div, prelude::*, px, rgb, rgba,
};

use ferrocad_geom::SelectionRef;
use ferrocad_inventor::{Scene, Viewport};
use ferrocad_stage::{InteractionCommit, Manipulator, TransformGizmo};
use ferrocad_viewport::scene;
use ferrocad_viewport::{HoverReport, InventorViewport, SelectionEvent, ViewportEvent};
use ferrocad_wgpu::WgpuRenderer;
use ferrocad_widgets::{SubmitEvent, TextAreaState, TextInput, window_frame};

use crate::commands::{self, CommandInfo};
use crate::contributions::Contributions;
use crate::dialog::{self, DialogSpec, Kind, ValueState};
use crate::display::{DisplaySync, StageMesh};
use crate::python::{self, Bootstrap, DocumentNode, PropRow};
use crate::selection::{self, SelectionItem};
use crate::shortcuts::Accelerator;
use crate::view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};

const BG: u32 = 0x101216;
const PANEL: u32 = 0x16181d;
const PANEL_ALT: u32 = 0x1d2026;
const FG: u32 = 0xd8dbe0;
const MUTED: u32 = 0x8b919a;
const ACCENT: u32 = 0x2f4a6d;

/// The console transcript: the log lines plus a fresh prompt.
fn transcript_of(model: &Arc<Mutex<ShellModel>>) -> String {
    let m = model.lock().unwrap();
    format!("{}\n>>> ", m.console.join("\n"))
}

/// Everything the window renders, kept in an `Arc<Mutex<_>>` so click handlers
/// can mutate it and tests can inspect it.
#[derive(Debug, Default, Clone)]
pub struct ShellModel {
    pub title: String,
    pub version: String,
    pub backend: String,
    pub tree: Vec<DocumentNode>,
    /// The primary selected object (the property editor's subject).
    pub selected: Option<String>,
    /// The full selection in click order (A2); the last item is the primary.
    pub selection: Vec<SelectionItem>,
    pub properties: Vec<PropRow>,
    /// The workbench actions, with their live enablement (A1).
    pub commands: Vec<CommandInfo>,
    /// The presentation state: grid and projection (A5).
    pub view: ViewState,
    pub console: Vec<String>,
    pub status: String,
}

pub struct Shell {
    model: Arc<Mutex<ShellModel>>,
    /// The console transcript plus the live input line (S5's text area).
    console: Entity<TextAreaState>,
    /// The 3D viewport, once [`Shell::enable_viewport`] has created a GPU renderer.
    /// `None` headlessly (no GPU), where the pane shows a placeholder.
    viewport: Option<Entity<InventorViewport<WgpuRenderer>>>,
    /// One uncontrolled field per editable property, keyed by `object.property`.
    prop_inputs: HashMap<String, PropInput>,
    /// The entity → document-object map, rebuilt from the app script's display meshes.
    display: DisplaySync,
    /// The workbench manipulators the edition contributes (none in the base app).
    contributions: Contributions,
    /// The enabled global accelerators, rebuilt with the command list (A4).
    shortcuts: Vec<(Accelerator, String)>,
    /// The interactive tool the last `{"tool": id}` started (A5).
    active_tool: Option<String>,
    /// The open command dialog (A3); one at a time.
    dialog: Option<OpenDialog>,
    /// Focus for the modal, so Esc cancels and Enter in a field accepts.
    dialog_focus: FocusHandle,
    /// Focus the modal on the next render (it is opened outside a window context).
    focus_dialog: bool,
    /// The viewport's event subscription (selection and interaction commits).
    _viewport_events: Option<Subscription>,
    _submit: Subscription,
}

/// An uncontrolled property field plus the value it was last seeded with.
struct PropInput {
    state: Entity<TextInput>,
    seeded: String,
    _submit: Subscription,
}

/// The open modal: the spec, the collected values, and its single-use token.
struct OpenDialog {
    token: String,
    spec: DialogSpec,
    /// One per field: text (text-like), flag (bool) or choice index (enum).
    states: Vec<ValueState>,
    /// The text input for a text-like field; `None` for a bool/enum row.
    inputs: Vec<Option<Entity<TextInput>>>,
    /// The first validation or callback error, shown inside the modal.
    error: Option<String>,
    /// Keep the per-field submit subscriptions alive for the dialog's lifetime.
    _submits: Vec<Subscription>,
}

impl Shell {
    /// Boot the interpreter, create the sample document, and return the model the
    /// view is built from. The view itself is created with `from_model` because
    /// entities (the console input) need an `App` context.
    pub fn boot() -> Result<Arc<Mutex<ShellModel>>, String> {
        let boot: Bootstrap = python::bootstrap()?;
        let tree = python::model_tree().unwrap_or_default();
        let mut console = vec![format!(
            "FerroCAD shell · FreeCAD {} · backend {}",
            boot.version, boot.backend
        )];
        match python::hello() {
            Ok(line) => console.push(line),
            Err(e) => console.push(format!("python: {e}")),
        }
        // Load workbenches best-effort: a broken one is reported, never fatal.
        match python::load_workbenches() {
            Ok(report) => {
                if !report.loaded.is_empty() {
                    console.push(format!(
                        "workbenches: loaded {} script(s)",
                        report.loaded.len()
                    ));
                }
                for error in &report.errors {
                    console.push(format!(
                        "workbench {} / {} failed (see stderr)",
                        error.workbench, error.script
                    ));
                    eprintln!(
                        "ferrocad: workbench {} {} failed:\n{}",
                        error.workbench, error.script, error.traceback
                    );
                }
            }
            Err(e) => console.push(format!("workbench loader: {e}")),
        }
        let objects: usize = tree.iter().map(|d| d.objects.len()).sum();
        let status = format!("{objects} object(s) · {}", boot.document);
        // A fresh document starts with an empty selection snapshot: the process may
        // have booted before (tests), and stale names must not leak into `IsActive`.
        let _ = selection::push(&[]);
        let _ = python::set_view_state(&ViewState::default());
        let commands = commands::list().unwrap_or_default();
        Ok(Arc::new(Mutex::new(ShellModel {
            title: format!("FerroCAD — {}", boot.document),
            version: boot.version,
            backend: boot.backend,
            tree,
            selected: None,
            selection: Vec::new(),
            properties: Vec::new(),
            commands,
            view: ViewState::default(),
            console,
            status,
        })))
    }

    /// Build a shell around an existing model (the window and tests use this).
    pub fn from_model(model: Arc<Mutex<ShellModel>>, cx: &mut Context<Self>) -> Self {
        let transcript = transcript_of(&model);
        let shortcuts = global_shortcuts(&model.lock().unwrap().commands);
        let console = cx.new(|cx| {
            let mut area = TextAreaState::new(cx);
            area.set_transcript(transcript, cx);
            area
        });
        let submit = cx.subscribe(
            &console,
            |this: &mut Shell, _emitter, event: &SubmitEvent, cx| {
                this.run(&event.text, cx);
            },
        );
        Self {
            model,
            console,
            viewport: None,
            prop_inputs: HashMap::new(),
            display: DisplaySync::new(),
            contributions: Contributions::new(),
            shortcuts,
            active_tool: None,
            dialog: None,
            dialog_focus: cx.focus_handle(),
            focus_dialog: false,
            _viewport_events: None,
            _submit: submit,
        }
    }

    /// Set the edition's workbench contributions (its manipulators).
    ///
    /// Call once, before [`enable_viewport`](Self::enable_viewport). The base app
    /// contributes none, so it links no workbench.
    pub fn set_contributions(&mut self, contributions: Contributions) {
        self.contributions = contributions;
    }

    /// The console's focus handle, so the window can put the caret there at boot.
    pub fn console_focus_handle(&self, cx: &App) -> FocusHandle {
        self.console.read(cx).focus_handle(cx)
    }

    /// Create the 3D viewport, best-effort: it needs a GPU.
    ///
    /// Called by the app after `from_model` (not by `from_model` itself), so headless
    /// tests stay GPU-free and render the placeholder. If no adapter is available the
    /// pane keeps the placeholder too.
    ///
    /// The scene is built the workbench way — a `from pivy import coin` script
    /// extracted into a [`ferrocad_inventor::Scene`] ([`python::inventor_scene`]) —
    /// falling back to the Rust sample if the interpreter is unavailable.
    pub fn enable_viewport(&mut self, cx: &mut Context<Self>) {
        // A bootstrap size only: `InventorViewport` resizes the render target and the
        // camera to its element's physical size on the first frame (see `sync_size`).
        const SIZE: [u32; 2] = [640, 480];
        let Ok(renderer) = WgpuRenderer::new_default(SIZE) else {
            return;
        };
        let camera = scene::sample_camera(Viewport::new(SIZE, 1.0));

        // The document's own shapes, when a display workbench is linked. Without them
        // (the base app has no kernel) the `pivy.coin` sample scene stays.
        let stage = match self.build_stage() {
            Ok(stage) if !stage.is_empty() => Some(stage),
            Ok(_) => None,
            Err(error) => {
                self.console_line(format!("stage: {error}"), cx);
                None
            }
        };
        let scene = if stage.is_some() {
            Scene::default()
        } else {
            python::inventor_scene().unwrap_or_else(|_| scene::sample_scene())
        };
        let navicube = ferrocad_viewport::NaviCube::new(camera.viewport);
        let mut viewport = InventorViewport::new(renderer, camera, scene)
            .with_listener(Box::new(scene::HoverHighlight::default()))
            .with_listener(Box::new(python::PythonEventCallbacks))
            .with_navicube(navicube);
        if let Some(stage) = stage {
            viewport.set_stage(stage);
        }
        // The view state is the source of truth: a grid toggled before the viewport
        // existed is applied now, with its steps.
        {
            let state = self.model.lock().unwrap().view.clone();
            viewport.set_grid(state.grid);
            viewport.set_axis_cross(state.axis_cross);
            viewport.set_grid_steps(state.grid_minor, state.grid_major);
            viewport.set_grid_radius(state.grid_radius);
        }
        // A tool started before the viewport existed (a script, a command while no
        // GPU was available) is applied now that there is one.
        if let Some(id) = self.active_tool.as_deref() {
            if let Some(tool) = self.contributions.tool_for(id) {
                viewport.set_tool(Some(tool));
            }
        }
        // The presenter is always safe to install: it falls back to the image path
        // when the window's device cannot be shared.
        let viewport = match crate::zero_copy_presenter() {
            Some(presenter) => viewport.with_presenter(presenter),
            None => viewport,
        };
        let viewport = cx.new(move |_cx| viewport);
        let events = cx.subscribe(&viewport, |this, _viewport, event: &ViewportEvent, cx| {
            this.on_viewport_event(event, cx)
        });
        self.viewport = Some(viewport);
        self._viewport_events = Some(events);
    }

    /// Append a line to the console transcript.
    fn console_line(&mut self, line: String, cx: &mut Context<Self>) {
        self.model.lock().unwrap().console.push(line);
        let transcript = transcript_of(&self.model);
        self.console
            .update(cx, |area, cx| area.set_transcript(transcript, cx));
    }

    /// Re-read the document's shapes into the viewport's stage.
    ///
    /// Call after any edit that can change geometry: a property commit, a console
    /// snippet, or an interaction commit (which syncs itself). An empty document or a
    /// build without a display workbench simply yields an empty stage.
    fn sync_stage(&mut self, cx: &mut Context<Self>) {
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

    /// Ask the app script for the display meshes and build a stage from them.
    ///
    /// The shell names no workbench: the script routes `stage_meshes` through
    /// `FreeCAD.display_mesh`, so whatever module owns the objects supplies the mesh.
    fn build_stage(&mut self) -> Result<ferrocad_stage::Stage, String> {
        let meshes: Vec<StageMesh> = python::stage_meshes()?;
        Ok(self.display.build(meshes))
    }

    /// React to a viewport event: a click selection, a manipulator commit, or a
    /// gizmo drag.
    fn on_viewport_event(&mut self, event: &ViewportEvent, cx: &mut Context<Self>) {
        match event {
            ViewportEvent::Selection(selection) => self.on_selection(selection, cx),
            ViewportEvent::Interaction(commit) => self.on_interaction(commit, cx),
            ViewportEvent::Transform(event) => self.on_interaction(&event.to_interaction(), cx),
        }
    }

    /// A click: select the object in the inspector, and arm the manipulator and gizmo
    /// the edition's workbenches contribute for that selection.
    fn on_selection(&mut self, selection: &SelectionEvent, cx: &mut Context<Self>) {
        let (manipulator, gizmo) = match selection {
            SelectionEvent::Picked(reference) => {
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
                (None, None)
            }
        };
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(manipulator.into_iter().collect());
                viewport.set_gizmo(gizmo);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// The manipulator and gizmo to host for a selection, if the edition contributes
    /// them.
    ///
    /// The stage is cloned out of the viewport before anything is updated: a re-entrant
    /// `update` on the same entity panics.
    fn arm(
        &self,
        reference: &SelectionRef,
        cx: &mut Context<Self>,
    ) -> (Option<Box<dyn Manipulator>>, Option<TransformGizmo>) {
        if self.contributions.is_empty() {
            return (None, None);
        }
        let Some(viewport) = self.viewport.as_ref() else {
            return (None, None);
        };
        let stage = viewport.read(cx).stage().clone();
        (
            self.contributions.manipulator_for(reference, &stage),
            self.contributions.gizmo_for(reference, &stage),
        )
    }

    /// A manipulator drag ended: apply the commit through the app script's
    /// `interaction` hook, recompute, and re-read the shapes.
    fn on_interaction(&mut self, commit: &InteractionCommit, cx: &mut Context<Self>) {
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
    fn after_document_edit(&mut self, cx: &mut Context<Self>) {
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
        self.refresh(cx);
    }

    /// Re-read the workbench commands and their enablement.
    ///
    /// Called after anything that can change `IsActive()`: a selection, a document
    /// edit, undo/redo, an open/close. It is deliberately not per frame — each call
    /// crosses into the interpreter.
    fn refresh_commands(&mut self, cx: &mut Context<Self>) {
        match commands::list() {
            Ok(commands) => {
                self.shortcuts = global_shortcuts(&commands);
                self.model.lock().unwrap().commands = commands;
            }
            Err(error) => self.console_line(format!("commands: {error}"), cx),
        }
    }

    /// The command a key event should run, if any.
    ///
    /// A modal owns the keyboard while it is open, and a disabled command does not
    /// fire (the same rule the bar shows).
    fn shortcut_for(&self, event: &KeyDownEvent) -> Option<String> {
        if self.dialog.is_some() {
            return None;
        }
        let (_, id) = self
            .shortcuts
            .iter()
            .find(|(accelerator, _)| accelerator.matches(&event.keystroke))?;
        let enabled = self
            .model
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|command| command.id == *id && command.enabled);
        enabled.then(|| id.clone())
    }

    /// Run a workbench command by id, then refresh the model and the stage.
    ///
    /// A command may instead return a **dialog** (A3): the spec is rendered as a
    /// modal and the shell accepts or cancels it later through its token. One
    /// dialog at a time — a second request while one is open is rejected and its
    /// token dropped, so the facade never keeps an unreachable callback.
    fn run_command(&mut self, id: &str, cx: &mut Context<Self>) {
        let outcome = match commands::run(id) {
            Ok(outcome) => outcome,
            Err(error) => {
                let status = format!("{id}: {error}");
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                cx.notify();
                return;
            }
        };

        if !outcome.ok {
            let status = if outcome.error.is_empty() {
                format!("{id} failed")
            } else {
                format!("{id}: {}", outcome.error)
            };
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }

        if let (Some(spec), Some(token)) = (outcome.dialog.clone(), outcome.token.clone()) {
            let status = if self.dialog.is_some() {
                let _ = commands::cancel(&token);
                "a dialog is already open".to_string()
            } else {
                self.open_dialog_state(token, spec, cx);
                if outcome.status.is_empty() {
                    format!("{id}: fill the dialog")
                } else {
                    outcome.status.clone()
                }
            };
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            // A command can return a dialog *and* have changed the document.
            self.refresh(cx);
            self.sync_stage(cx);
            cx.notify();
            return;
        }

        // A view request is presentation-only: apply it, mirror the state, and
        // re-list so a checkable command's `IsChecked` reflects the change.
        if let Some(request) = &outcome.view {
            let status = match self.apply_view(request, cx) {
                Ok(summary) => summary,
                Err(error) => format!("view: {error}"),
            };
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }

        // A tool request starts a session from the edition's Rust registry; it never
        // opens a transaction.
        if let Some(tool) = &outcome.tool {
            let status = self.start_tool(tool, cx);
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }

        self.refresh(cx);
        self.sync_stage(cx);
        let status = if outcome.status.is_empty() {
            format!("ran {id}")
        } else {
            outcome.status
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Open a command's dialog, seeding one control per field.
    fn open_dialog_state(&mut self, token: String, spec: DialogSpec, cx: &mut Context<Self>) {
        let mut states = Vec::with_capacity(spec.fields.len());
        let mut inputs = Vec::with_capacity(spec.fields.len());
        let mut submits = Vec::new();
        for field in &spec.fields {
            states.push(ValueState::for_field(field));
            if field.kind().is_textual() {
                let seed = field.seed_text();
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(cx, "");
                    input.set_text(seed, cx);
                    input
                });
                // Enter in a field accepts: the same path as the button.
                let submit = cx.subscribe(
                    &input,
                    |this: &mut Shell, _emitter, _event: &SubmitEvent, cx| {
                        this.accept_dialog(cx);
                    },
                );
                inputs.push(Some(input));
                submits.push(submit);
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
        });
        self.focus_dialog = true;
        cx.notify();
    }

    /// The collected state of every field; the text inputs are authoritative.
    fn dialog_states(&self, cx: &App) -> Vec<ValueState> {
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

    /// Validate and hand the dialog's values to the facade. Validation errors stay
    /// in the modal; a callback error is reported there too.
    fn accept_dialog(&mut self, cx: &mut Context<Self>) {
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
    fn finish_dialog(
        &mut self,
        result: Result<commands::CommandOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(outcome) if outcome.ok => {
                if let (Some(spec), Some(token)) = (outcome.dialog.clone(), outcome.token.clone()) {
                    self.open_dialog_state(token, spec, cx);
                } else {
                    self.dialog = None;
                }
                let status = if outcome.status.is_empty() {
                    "ok".to_string()
                } else {
                    outcome.status
                };
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                self.refresh(cx);
                self.sync_stage(cx);
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
    fn cancel_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let _ = commands::cancel(&dialog.token);
        self.model.lock().unwrap().status = "dialog cancelled".to_string();
        cx.notify();
    }

    /// Flip a bool field.
    fn toggle_flag(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            if let Some(ValueState::Flag(flag)) = dialog.states.get_mut(index) {
                *flag = !*flag;
                dialog.error = None;
            }
        }
        cx.notify();
    }

    /// Move an enum field by `delta` options, wrapping.
    fn cycle_choice(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            let len = dialog
                .spec
                .fields
                .get(index)
                .map(|field| field.values.len())
                .unwrap_or(0);
            if len > 0 {
                if let Some(ValueState::Choice(choice)) = dialog.states.get_mut(index) {
                    *choice = (*choice as isize + delta).rem_euclid(len as isize) as usize;
                    dialog.error = None;
                }
            }
        }
        cx.notify();
    }

    /// Apply a command's presentation request (A5, G5): perform a workplane action, or
    /// update the shell's view state, mirror it to `FreeCADGui.ViewState`, and apply it
    /// to the viewport when one exists. Returns a one-line summary for the status bar.
    fn apply_view(
        &mut self,
        request: &ViewRequest,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        if let Some(action) = request.workplane_action()? {
            return self.apply_workplane(action, cx);
        }
        let mut state = self.model.lock().unwrap().view.clone();
        let summary = state.apply(request)?;
        self.model.lock().unwrap().view = state.clone();
        if let Err(error) = python::set_view_state(&state) {
            self.console_line(format!("view state: {error}"), cx);
        }
        if let Some(viewport) = self.viewport.clone() {
            let orthographic = state.projection == ProjectionName::Orthographic;
            let grid = state.grid;
            let axis_cross = state.axis_cross;
            let (minor, major, radius) = (state.grid_minor, state.grid_major, state.grid_radius);
            viewport.update(cx, |viewport, cx| {
                let mut changed = viewport.set_orthographic(orthographic);
                changed |= viewport.set_grid(grid);
                changed |= viewport.set_axis_cross(axis_cross);
                changed |= viewport.set_grid_steps(minor, major);
                changed |= viewport.set_grid_radius(radius);
                if changed {
                    cx.notify();
                }
            });
        }
        // The mirror above makes `IsChecked` see the new state.
        self.refresh_commands(cx);
        Ok(summary)
    }

    /// Perform a workplane action on the viewport (G5). These are not presentation
    /// state: they act on the pointer hover, the sticky plane and the camera.
    fn apply_workplane(
        &mut self,
        action: WorkplaneAction,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let Some(viewport) = self.viewport.clone() else {
            return Err("no viewport".to_string());
        };
        Ok(viewport.update(cx, |viewport, cx| {
            let summary = match action {
                WorkplaneAction::Activate => {
                    if viewport.activate_hovered_workplane() {
                        "workplane activated".to_string()
                    } else {
                        "no face under the pointer".to_string()
                    }
                }
                WorkplaneAction::Clear => {
                    if viewport.clear_workplane() {
                        "workplane cleared".to_string()
                    } else {
                        "no active workplane".to_string()
                    }
                }
                WorkplaneAction::View => {
                    if viewport.view_workplane() {
                        "looking down the workplane".to_string()
                    } else {
                        "no active workplane".to_string()
                    }
                }
            };
            cx.notify();
            summary
        }))
    }

    /// Start an interactive tool through the edition's registry (A5). The tool owns
    /// pointer events until it ends; no document transaction is opened.
    fn start_tool(&mut self, id: &str, cx: &mut Context<Self>) -> String {
        let Some(tool) = self.contributions.tool_for(id) else {
            return format!("no such tool {id}");
        };
        self.active_tool = Some(id.to_string());
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                // A tool session owns the pointer: the selection handles step aside.
                viewport.set_manipulators(Vec::new());
                viewport.set_gizmo(None);
                viewport.set_tool(Some(tool));
                cx.notify();
            });
        }
        format!("tool: {id}")
    }

    /// Replace the selection with one whole object (a plain click).
    fn select(&mut self, name: &str, cx: &mut Context<Self>) {
        self.replace_selection(vec![SelectionItem::new(name)], cx);
    }

    /// Add or remove one object (Ctrl/Cmd- or Shift-click), keeping the rest.
    fn toggle_select(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut items = {
            let m = self.model.lock().unwrap();
            m.selection.clone()
        };
        if let Some(index) = items.iter().position(|item| item.is(name)) {
            items.remove(index);
        } else {
            items.push(SelectionItem::new(name));
        }
        self.replace_selection(items, cx);
    }

    /// Clear the selection.
    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.replace_selection(Vec::new(), cx);
    }

    /// Replace the whole selection: re-seed the property editor from the primary
    /// (last-clicked) object, push the facade snapshot, and re-evaluate enablement.
    ///
    /// The push happens **before** the commands are listed, so `IsActive` sees the
    /// selection the user just made.
    fn replace_selection(&mut self, items: Vec<SelectionItem>, cx: &mut Context<Self>) {
        let primary = items.last().map(|item| item.object.clone());
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        self.prop_inputs.clear();
        let status = match (items.len(), primary.as_deref()) {
            (0, _) | (_, None) => "selection cleared".to_string(),
            (1, Some(name)) => format!("selected {name}"),
            (count, Some(name)) => format!("selected {count} · {name}"),
        };
        {
            let mut m = self.model.lock().unwrap();
            m.selection = items;
            m.selected = primary;
            m.properties = properties;
            m.status = status;
        }
        if let Err(error) = self.push_selection() {
            self.console_line(format!("selection: {error}"), cx);
        }
        self.refresh_commands(cx);
        cx.notify();
    }

    /// Push the shell's selection to `FreeCADGui.Selection` (the facade snapshot).
    fn push_selection(&self) -> Result<(), String> {
        let items = {
            let m = self.model.lock().unwrap();
            m.selection.clone()
        };
        selection::push(&items)
    }

    /// The status line's pass-through segment: the overlay the pointer would click
    /// through, plus the document object beneath it. Empty when there is none.
    fn hover_line(&self, cx: &App) -> String {
        let Some(viewport) = self.viewport.as_ref() else {
            return String::new();
        };
        let report = viewport.read(cx).hover_report();
        let target = report
            .target
            .as_ref()
            .and_then(|reference| self.display.object_of(reference.object));
        hover_status(report, target)
    }

    /// Re-read the tree and the selected object's properties after a change.
    ///
    /// Selection names the document no longer has (a delete, or an undo of a
    /// creation) are dropped here, and the pruned set is pushed to the facade so a
    /// command's `IsActive` cannot see a deleted object.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let tree = python::model_tree().unwrap_or_default();
        let live: std::collections::HashSet<String> = tree
            .iter()
            .flat_map(|doc| doc.objects.iter().map(|obj| obj.name.clone()))
            .collect();
        let (next, primary) = {
            let m = self.model.lock().unwrap();
            let mut next = m.selection.clone();
            next.retain(|item| live.contains(&item.object));
            let primary = next
                .last()
                .map(|item| item.object.clone())
                .or_else(|| m.selected.clone().filter(|name| live.contains(name)));
            (next, primary)
        };
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        {
            let mut m = self.model.lock().unwrap();
            m.tree = tree;
            m.selection = next;
            m.selected = primary;
            m.properties = properties;
        }
        if let Err(error) = self.push_selection() {
            self.console_line(format!("selection: {error}"), cx);
        }
        self.refresh_commands(cx);
        cx.notify();
    }

    /// Get the field for a property, creating it (and its submit subscription) on
    /// first use. The field is re-seeded from the model when the value changed
    /// underneath and the user is not editing it.
    fn ensure_prop_input(
        &mut self,
        key: &str,
        value: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.prop_inputs.get_mut(key) {
            let focused = input.state.read(cx).focus_handle(cx).is_focused(window);
            if input.seeded != value && !focused {
                input
                    .state
                    .update(cx, |state, cx| state.set_text(value.to_string(), cx));
                input.seeded = value.to_string();
            }
            return input.state.clone();
        }

        let state = cx.new(|cx| {
            let mut state = TextInput::new(cx, "");
            state.set_text(value.to_string(), cx);
            state
        });
        let key = key.to_string();
        let key_for_submit = key.clone();
        let submit = cx.subscribe(
            &state,
            move |this: &mut Shell, _emitter, event: &SubmitEvent, cx| {
                this.commit_property(&key_for_submit, &event.text, cx);
            },
        );
        self.prop_inputs.insert(
            key,
            PropInput {
                state: state.clone(),
                seeded: value.to_string(),
                _submit: submit,
            },
        );
        state
    }

    /// Commit a property field: one filtered edit inside one transaction.
    fn commit_property(&mut self, key: &str, text: &str, cx: &mut Context<Self>) {
        let Some((object, property)) = key.split_once('.') else {
            return;
        };
        let status = match python::set_property(object, property, text) {
            Ok(result) if result.ok => {
                if let Some(input) = self.prop_inputs.get_mut(key) {
                    input.seeded = text.to_string();
                }
                self.refresh(cx);
                self.sync_stage(cx);
                format!("set {key}")
            }
            Ok(result) => format!("property error: {}", result.error),
            Err(e) => format!("property error: {e}"),
        };
        self.model.lock().unwrap().status = status;
        cx.notify();
    }

    /// Run a console snippet, refresh the tree (the model may have changed), and
    /// rebuild the transcript so the console text area shows the result and a
    /// fresh prompt.
    fn run(&mut self, code: &str, cx: &mut Context<Self>) {
        let result = python::evaluate(code);
        {
            let mut m = self.model.lock().unwrap();
            m.console.push(format!(">>> {code}"));
            match result {
                Ok(r) => {
                    if !r.output.is_empty() {
                        m.console.push(r.output);
                    }
                    m.status = if r.ok {
                        "ok".to_string()
                    } else {
                        "console error".to_string()
                    };
                }
                Err(e) => m.console.push(format!("error: {e}")),
            }
        }
        // A console line can add objects or change values; re-read the model.
        self.refresh(cx);
        self.sync_stage(cx);
        let transcript = transcript_of(&self.model);
        self.console
            .update(cx, |area, cx| area.set_transcript(transcript, cx));
        cx.notify();
    }

    fn snapshot(&self) -> ShellModel {
        self.model.lock().unwrap().clone()
    }
}

/// A slim panel header bar.
fn panel_header(text: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(MUTED))
        .child(text.to_string())
}

/// The workbench actions bar: one button per registered command (A1).
///
/// A disabled command stays visible — the user should see that the action exists —
/// and a checkable one carries the accent. The bar appears whenever the app or a
/// workbench registered something; the built-ins (`App_Delete`, `App_Move`) mean
/// even the base app has a bar, with its actions greyed until a selection.
fn actions_bar(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let mut bar = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgba(0xffffff14));
    for command in &model.commands {
        let id = command.id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        let mut button = div()
            .id(SharedString::from(format!("command-{id}")))
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(checked, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL_ALT)))
            })
            .child(command.label().to_string());
        if !command.tooltip.is_empty() {
            let mut tip = command.tooltip.clone();
            if !command.accel.is_empty() {
                tip.push_str("  ·  ");
                tip.push_str(&command.accel);
            }
            button = button.tooltip(move |_window, cx| {
                cx.new(|_| CommandTooltip {
                    text: tip.clone().into(),
                })
                .into()
            });
        }
        if enabled {
            button = button
                .on_click(cx.listener(move |this, _event, _window, cx| this.run_command(&id, cx)));
        }
        bar = bar.child(button);
    }
    bar
}

/// The actions bar's hover text: the command's `ToolTip` and its accelerator.
struct CommandTooltip {
    text: SharedString,
}

impl Render for CommandTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgba(0xffffff14))
            .text_xs()
            .text_color(rgb(FG))
            .child(self.text.clone())
    }
}

/// The modal for an open command dialog (A3): a glass panel over a dimmed shell.
///
/// The panel holds keyboard focus so `Esc` cancels; a text field's `Enter`
/// submits through its own `SubmitEvent`, which is wired to the same accept path.
fn dialog_overlay(shell: &mut Shell, cx: &mut Context<Shell>) -> AnyElement {
    let Some(dialog) = shell.dialog.as_ref() else {
        return div().into_any_element();
    };
    let spec = dialog.spec.clone();
    let states = dialog.states.clone();
    let inputs = dialog.inputs.clone();
    let error = dialog.error.clone();
    let focus = shell.dialog_focus.clone();

    let mut fields = div().flex().flex_col().gap_2();
    for (index, field) in spec.fields.iter().enumerate() {
        let control: AnyElement = match field.kind() {
            Kind::Bool => {
                let checked = matches!(states.get(index), Some(ValueState::Flag(true)));
                let label = if checked { "on" } else { "off" };
                div()
                    .id(SharedString::from(format!("dialog-bool-{index}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(if checked { ACCENT } else { PANEL_ALT }))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _event, _window, cx| this.toggle_flag(index, cx)),
                    )
                    .into_any_element()
            }
            Kind::Enum => {
                let choice = match states.get(index) {
                    Some(ValueState::Choice(choice)) => *choice,
                    _ => 0,
                };
                let value = field.values.get(choice).cloned().unwrap_or_default();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(dialog_arrow("‹", index, -1, cx))
                    .child(
                        div()
                            .min_w(px(84.))
                            .text_xs()
                            .text_color(rgb(FG))
                            .child(value),
                    )
                    .child(dialog_arrow("›", index, 1, cx))
                    .into_any_element()
            }
            Kind::Unknown => div()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(format!("unsupported kind {}", field.kind))
                .into_any_element(),
            _ => match inputs.get(index).and_then(Option::as_ref) {
                Some(input) => div()
                    .w(px(170.))
                    .flex_shrink_0()
                    .child(input.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
        };
        fields = fields.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(field.label().to_string()),
                )
                .child(control),
        );
    }

    let mut panel = div()
        .id("dialog-panel")
        .w(px(380.))
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .track_focus(&focus)
        .focusable()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => this.cancel_dialog(cx),
                "enter" => this.accept_dialog(cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_sm()
                .text_color(rgb(FG))
                .child(spec.title.clone()),
        )
        .child(fields);
    if let Some(error) = &error {
        panel = panel.child(
            div()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(error.clone()),
        );
    }
    let accept = spec.accept_label().to_string();
    let cancel = spec.cancel_label().to_string();
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(
                div()
                    .id("dialog-cancel")
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(PANEL_ALT))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(cancel)
                    .on_click(cx.listener(|this, _event, _window, cx| this.cancel_dialog(cx))),
            )
            .child(
                div()
                    .id("dialog-accept")
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(ACCENT))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(accept)
                    .on_click(cx.listener(|this, _event, _window, cx| this.accept_dialog(cx))),
            ),
    );

    div()
        .id("dialog-backdrop")
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0f1218cc))
        .occlude()
        .child(panel)
        .into_any_element()
}

/// An enum field's `‹` / `›` button.
fn dialog_arrow(
    label: &str,
    index: usize,
    delta: isize,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("dialog-arrow-{index}-{delta}")))
        .px_1()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(FG))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _event, _window, cx| this.cycle_choice(index, delta, cx)))
}

/// The model inspector: documents and their objects.
fn tree_panel(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let mut rows = div()
        .id("tree-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

    for doc in &model.tree {
        rows = rows.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("▾ {} ({})", doc.label, doc.name)),
        );
        for obj in &doc.objects {
            let name = obj.name.clone();
            let mut row = div()
                .id(SharedString::from(format!("obj-{name}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .py_1()
                .child(div().text_sm().text_color(rgb(FG)).child(obj.label.clone()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(obj.type_id.clone()),
                );
            if model.selection.iter().any(|item| item.is(&name)) {
                row = row.bg(rgb(ACCENT));
            }
            rows = rows.child(row.on_click(cx.listener(
                move |this, event: &gpui::ClickEvent, _window, cx| {
                    // Ctrl/Cmd (the secondary modifier) or Shift extends the selection;
                    // a plain click replaces it.
                    let modifiers = event.modifiers();
                    if modifiers.secondary() || modifiers.shift {
                        this.toggle_select(&name, cx);
                    } else {
                        this.select(&name, cx);
                    }
                },
            )));
        }
    }

    div()
        .flex()
        .flex_col()
        .w(px(260.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Model"))
        .child(rows)
}

/// The property editor: read-only rows plus an uncontrolled field for each
/// editable property.
fn properties_panel(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div()
        .id("properties-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

    match &model.selected {
        None => {
            rows = rows.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("No selection"),
            );
        }
        Some(name) => {
            rows = rows.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(name.clone()),
            );
            for row in &model.properties {
                let value: AnyElement = if row.editable {
                    let key = format!("{name}.{}", row.name);
                    let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                    div()
                        .w(px(150.))
                        .flex_shrink_0()
                        .child(state)
                        .into_any_element()
                } else {
                    let value = if row.status.is_empty() {
                        row.value.clone()
                    } else {
                        format!("{}  [{}]", row.value, row.status)
                    };
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(value)
                        .into_any_element()
                };

                rows = rows.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_1()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(rgb(FG))
                                        .child(row.name.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child(row.type_id.clone()),
                                ),
                        )
                        .child(value),
                );
            }
        }
    }

    div()
        .flex()
        .flex_col()
        .w(px(320.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Properties"))
        .child(rows)
}

/// The globally bindable accelerators for `commands`.
///
/// A shortcut is bound only when it is a safe global chord; a bare letter would
/// shadow typing in the console and the property editor (see [`Accelerator`]).
fn global_shortcuts(commands: &[CommandInfo]) -> Vec<(Accelerator, String)> {
    commands
        .iter()
        .filter_map(|command| {
            let accelerator = Accelerator::parse(&command.accel)?;
            accelerator
                .is_global()
                .then(|| (accelerator, command.id.clone()))
        })
        .collect()
}

/// The status line's pass-through text: `"clicked through: navicube → Slab"`.
///
/// Empty when no overlay is under the pointer, so the segment disappears rather
/// than saying "nothing".
fn hover_status(report: &HoverReport, target: Option<&str>) -> String {
    let Some(through) = report.through else {
        return String::new();
    };
    match target {
        Some(name) => format!("clicked through: {through} → {name}"),
        None => format!("clicked through: {through}"),
    }
}

/// A console action button.
fn console_button(
    id: &str,
    label: &str,
    code: &str,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let code = code.to_string();
    div()
        .id(SharedString::from(id.to_string()))
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(FG))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _ev, _win, cx| {
            this.run(&code, cx);
        }))
}

/// The Python console: one text area (scrollback plus prompt) and runnable
/// snippets.
fn console_panel(console: &Entity<TextAreaState>, cx: &mut Context<Shell>) -> impl IntoElement {
    let actions = div()
        .flex()
        .flex_row()
        .gap_2()
        .px_2()
        .py_1()
        .child(console_button("console-version", "version()", "FreeCAD.Version()", cx))
        .child(console_button("console-objects", "objects", "len(doc.Objects)", cx))
        .child(console_button("console-recompute", "recompute()", "doc.recompute()", cx));

    div()
        .flex()
        .flex_col()
        .h(px(160.))
        .bg(rgb(PANEL))
        .child(panel_header("Python console"))
        .child(console.clone())
        .child(actions)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.snapshot();

        // A modal was opened outside a window context; focus it once rendered so
        // Esc cancels and the panel receives bubbled keys.
        if std::mem::take(&mut self.focus_dialog) {
            window.focus(&self.dialog_focus, cx);
        }

        let viewport = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(BG))
            .child(panel_header("3D viewport"))
            .child(match &self.viewport {
                Some(viewport) => viewport.clone().into_any_element(),
                None => div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("viewport (no GPU)")
                    .into_any_element(),
            });

        let hover = self.hover_line(cx);
        let status = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(22.))
            .px_3()
            .bg(rgb(PANEL_ALT))
            .text_xs()
            .text_color(rgb(MUTED))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(model.status.clone())
                    .when(!hover.is_empty(), |el| {
                        el.child(div().text_color(rgb(0x38bdf8)).child(hover.clone()))
                    }),
            )
            .child(format!("FreeCAD {} · {}", model.version, model.backend));

        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG));
        let content = if model.commands.is_empty() {
            content
        } else {
            content.child(actions_bar(&model, cx))
        };
        let content = content
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(tree_panel(&model, cx))
                    .child(viewport)
                    .child(properties_panel(self, &model, window, cx)),
            )
            .child(console_panel(&self.console, cx))
            .child(status);

        // The modal sits above the whole shell, with a backdrop that blocks the
        // panes underneath.
        let body = div()
            .id("shell-root")
            .relative()
            .size_full()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if let Some(id) = this.shortcut_for(event) {
                    cx.stop_propagation();
                    this.run_command(&id, cx);
                }
            }))
            .child(content);
        let body = if self.dialog.is_some() {
            body.child(dialog_overlay(self, cx))
        } else {
            body
        };
        window_frame(window, model.title.clone(), body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    /// S1 end-to-end, headless: boot the interpreter, read the sample document
    /// through it, round-trip the console, and render the shell.
    #[gpui::test]
    fn shell_boots_lists_the_sample_document_and_is_renderable(cx: &mut TestAppContext) {
        // Tests share one CPython instance (module-global state); serialize them.
        let _guard = crate::PYTHON_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let model = Shell::boot().expect("boot shell");
        {
            let m = model.lock().unwrap();
            let doc = m
                .tree
                .iter()
                .find(|d| d.name == "Shell")
                .expect("Shell document");
            let names: Vec<&str> = doc.objects.iter().map(|o| o.name.as_str()).collect();
            assert!(names.contains(&"Params"), "objects: {names:?}");
            assert!(names.contains(&"Derived"));
            assert!(m.console.iter().any(|l| l.contains("hello from Python")));
        }

        // The property editor is fed from the live model.
        let props = crate::python::properties("Params").expect("properties");
        assert!(props.iter().any(|p| p.name == "Length"));

        // The console round-trips a snippet through the interpreter.
        let out = crate::python::evaluate("len(doc.Objects)").expect("evaluate");
        assert!(out.ok, "eval failed: {}", out.output);
        assert_eq!(out.output, "3");

        // The shell renders headlessly (no display server, no GPU).
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // Typing in the console input and pressing Enter runs the line through the
        // interpreter (S5 previewed): the log gains the echoed line and the result,
        // and the field is cleared.
        let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
        cx.update(|window, cx| window.focus(&console, cx));
        cx.simulate_input("len(doc.Objects)");
        cx.simulate_keystrokes("enter");
        {
            let m = model.lock().unwrap();
            assert!(
                m.console.iter().any(|l| l == ">>> len(doc.Objects)"),
                "console: {:?}",
                m.console
            );
            assert!(m.console.iter().any(|l| l == "3"), "console: {:?}", m.console);
        }
    }

    /// Slice E: an uncontrolled property field commits on Enter, inside a named
    /// transaction, and the change propagates through recompute.
    #[gpui::test]
    fn editing_a_property_commits_in_a_transaction(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // Select Params so the editor builds one field per editable property.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Params", cx);
                cx.notify();
            });
        });

        let field = shell.read_with(&cx, |shell, _| {
            shell
                .prop_inputs
                .get("Params.Length")
                .expect("Length field")
                .state
                .clone()
        });
        cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("25 mm");
        cx.simulate_keystrokes("enter");

        // The selected object's row reflects the committed value...
        let length = shell.read_with(&cx, |shell, _| {
            shell
                .model
                .lock()
                .unwrap()
                .properties
                .iter()
                .find(|row| row.name == "Length")
                .map(|row| row.value.clone())
        });
        assert!(
            length.as_deref().unwrap_or_default().contains("25"),
            "Length = {length:?}"
        );

        // ...and recompute propagated it to the expression-driven other object.
        let derived = crate::python::properties("Derived").expect("properties");
        let result = derived
            .iter()
            .find(|row| row.name == "Result")
            .map(|row| row.value.clone())
            .unwrap_or_default();
        assert!(result.contains("50"), "Derived.Result = {result:?}");
    }

    /// A2: the shell owns the selection, pushes it to `FreeCADGui.Selection`, and a
    /// multi-selection enables the built-ins. The inspector's Ctrl/Shift-click path
    /// is `toggle_select`, so this drives the same methods the click handler does.
    #[gpui::test]
    fn a_multi_selection_is_pushed_and_enables_the_built_ins(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        // Nothing selected: Delete and Move are visible but disabled.
        assert!(!command_enabled("App_Delete"));
        assert!(!command_enabled("App_Move"));
        assert!(!crate::python::commands().unwrap().is_empty());

        // A plain select replaces; a toggle extends (the inspector's modifier path).
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            shell.toggle_select("Derived", cx);
        });
        {
            let m = model.lock().unwrap();
            assert_eq!(m.selection.len(), 2, "selection: {:?}", m.selection);
            assert_eq!(
                m.selected.as_deref(),
                Some("Derived"),
                "the last click is the property editor's subject"
            );
            assert!(m.status.contains('2'), "status: {}", m.status);
        }
        assert!(
            command_enabled("App_Delete"),
            "the snapshot reached the facade"
        );
        assert!(command_enabled("App_Move"));

        // Toggling an object off keeps the rest, and the primary follows.
        shell.update(cx, |shell, cx| shell.toggle_select("Derived", cx));
        {
            let m = model.lock().unwrap();
            assert_eq!(m.selection.len(), 1);
            assert_eq!(m.selected.as_deref(), Some("Params"));
        }

        // Clearing empties the facade too, so IsActive flips back.
        shell.update(cx, |shell, cx| shell.clear_selection(cx));
        assert!(!command_enabled("App_Delete"));
        assert!(model.lock().unwrap().selection.is_empty());
    }

    /// A3: a command's dialog is validated in the shell, accepted once, and a bad
    /// value keeps the modal open. The command is registered through the console,
    /// so the test exercises the same registry a workbench uses.
    #[gpui::test]
    fn a_command_dialog_validates_accepts_and_cancels(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        let script = r#"
import FreeCADGui
class _TestDialog:
    def GetResources(self):
        return {"MenuText": "Test Dialog"}
    def Activated(self):
        return {
            "title": "Test Dialog",
            "accept": "Do it",
            "fields": [
                {"name": "Count", "label": "Count", "kind": "int", "value": 2, "min": 1, "max": 9},
                {"name": "Mode", "kind": "enum", "values": ["a", "b"], "value": "a"},
                {"name": "Solid", "kind": "bool", "value": False},
            ],
            "on_accept": lambda values: {"status": f"got {values['Count']} {values['Mode']} {values['Solid']}"},
        }
if "Test_Dialog" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Dialog", _TestDialog())
"#;
        let registered = crate::python::evaluate(script).expect("evaluate");
        assert!(registered.ok, "{}", registered.output);

        // The command opens a modal: a spec, a token, and no document change.
        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_Dialog", cx)));
        let (token, error) = cx.update(|cx| {
            let shell = shell.read(cx);
            let dialog = shell.dialog.as_ref().expect("the dialog opened");
            assert_eq!(dialog.spec.fields.len(), 3);
            (dialog.token.clone(), dialog.error.clone())
        });
        assert!(token.starts_with('d'), "token: {token}");
        assert!(error.is_none());

        // A value below the minimum stays in the modal and names the field.
        cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                let input = shell.dialog.as_ref().unwrap().inputs[0].clone().unwrap();
                input.update(cx, |input, cx| input.set_text("0", cx));
                shell.accept_dialog(cx);
            });
        });
        let error = cx.update(|cx| {
            shell
                .read(cx)
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.error.clone())
        });
        assert!(
            error.as_deref().unwrap_or_default().contains("Count"),
            "error: {error:?}"
        );

        // A valid value runs the callback and closes the modal.
        cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                let input = shell.dialog.as_ref().unwrap().inputs[0].clone().unwrap();
                input.update(cx, |input, cx| input.set_text("7", cx));
                shell.accept_dialog(cx);
            });
        });
        assert!(cx.update(|cx| shell.read(cx).dialog.is_none()));
        let status = model.lock().unwrap().status.clone();
        assert!(status.contains("got 7 a False"), "status: {status}");

        // Cancel drops the facade's token: a later accept finds nothing.
        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_Dialog", cx)));
        let token = cx.update(|cx| shell.read(cx).dialog.as_ref().unwrap().token.clone());
        cx.update(|cx| shell.update(cx, |shell, cx| shell.cancel_dialog(cx)));
        assert!(cx.update(|cx| shell.read(cx).dialog.is_none()));
        let spent = crate::commands::accept(&token, &serde_json::json!({})).expect("accept");
        assert!(!spent.ok, "the cancelled token is gone");
    }

    /// A3: the modal renders headlessly over the shell, and `Esc` cancels it.
    #[gpui::test]
    fn the_command_dialog_renders_and_escape_cancels(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // `App_Move` with a selection opens its dialog; the shell is built-ins only.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        let (open, fields) = cx.update(|_window, cx| {
            let shell = shell.read(cx);
            (
                shell.dialog.is_some(),
                shell
                    .dialog
                    .as_ref()
                    .map(|dialog| dialog.spec.fields.len())
                    .unwrap_or(0),
            )
        });
        assert!(open, "the dialog opened");
        assert_eq!(fields, 3, "ΔX, ΔY, ΔZ");

        // Force a full paint: the backdrop, fields and buttons are built.
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // The modal held focus, so Escape cancels and drops its token.
        cx.simulate_keystrokes("escape");
        assert!(
            cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
            "Escape cancelled the modal"
        );
    }

    /// A4: a registered accelerator runs the same command path as a click, even
    /// while a text field has focus (the chord is unhandled there and bubbles).
    #[gpui::test]
    fn a_shortcut_runs_the_command(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        let script = r#"
import FreeCADGui
class _TestShortcut:
    def GetResources(self):
        return {"MenuText": "Shortcut", "Accel": "Ctrl+Shift+K"}
    def Activated(self):
        return {"status": "shortcut ran"}
if "Test_Shortcut" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Shortcut", _TestShortcut())
"#;
        let registered = cx.update(|_window, _cx| crate::python::evaluate(script));
        assert!(registered.unwrap().ok);
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

        // The console owns focus; Ctrl+Shift+K is not a text-editing chord, so it
        // bubbles to the shell root and runs the command.
        let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
        cx.update(|window, cx| window.focus(&console, cx));
        cx.simulate_keystrokes("ctrl-shift-k");

        assert_eq!(model.lock().unwrap().status, "shortcut ran");
    }

    #[test]
    fn the_pass_through_line_names_the_overlay_and_target() {
        let report = HoverReport {
            target: Some(SelectionRef::element(7, "Face1")),
            through: Some("navicube"),
        };
        assert_eq!(
            hover_status(&report, Some("Slab")),
            "clicked through: navicube → Slab"
        );
        assert_eq!(hover_status(&report, None), "clicked through: navicube");
        assert!(hover_status(&HoverReport::default(), Some("Slab")).is_empty());
    }

    /// A5: a `{"view": ...}` request flips the shell state, the mirrored facade
    /// snapshot, and the command's check mark.
    #[gpui::test]
    fn a_view_request_toggles_the_grid_and_its_check_mark(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        assert!(!model.lock().unwrap().view.grid);
        assert!(command_enabled("App_ToggleGrid"));
        assert!(!command_checked("App_ToggleGrid"));

        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx)));
        let (grid, status) = {
            let m = model.lock().unwrap();
            (m.view.grid, m.status.clone())
        };
        assert!(grid, "the request turned the grid on");
        assert_eq!(status, "grid on");
        assert!(
            command_checked("App_ToggleGrid"),
            "IsChecked reads the mirrored ViewState"
        );

        // A second run flips it back; the state is not a document edit.
        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx)));
        assert!(!model.lock().unwrap().view.grid);
        assert!(!command_checked("App_ToggleGrid"));
    }

    /// G5: the axis cross is its own checkable flag, independent of the grid.
    #[gpui::test]
    fn the_axis_cross_is_toggled_separately(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        assert!(model.lock().unwrap().view.axis_cross, "on by default");
        assert!(command_checked("App_ToggleAxisCross"));

        cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleAxisCross", cx))
        });
        assert_eq!(model.lock().unwrap().status, "axis cross off");
        assert!(!model.lock().unwrap().view.axis_cross);
        assert!(!model.lock().unwrap().view.grid, "the grid is untouched");
        assert!(!command_checked("App_ToggleAxisCross"));
    }

    /// A5: a tool id resolves through the edition's registry; an unknown id reports
    /// cleanly and starts nothing.
    #[gpui::test]
    fn a_tool_request_resolves_by_id(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));
        cx.update(|cx| {
            shell.update(cx, |shell, _| {
                shell.set_contributions(Contributions::new().with_tool("test_tool", || {
                    Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
                }));
            })
        });

        let script = r#"
import FreeCADGui
class _TestTool:
    def Activated(self):
        return {"tool": "test_tool"}
class _TestMissingTool:
    def Activated(self):
        return {"tool": "no_such_tool"}
if "Test_Tool" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Tool", _TestTool())
    FreeCADGui.addCommand("Test_MissingTool", _TestMissingTool())
"#;
        let registered = cx.update(|cx| crate::python::evaluate(script));
        assert!(registered.unwrap().ok);
        cx.update(|cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_Tool", cx)));
        assert_eq!(model.lock().unwrap().status, "tool: test_tool");
        let active = cx.update(|cx| shell.read(cx).active_tool.clone());
        assert_eq!(active.as_deref(), Some("test_tool"));

        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_MissingTool", cx)));
        assert_eq!(model.lock().unwrap().status, "no such tool no_such_tool");
    }

    fn command_enabled(id: &str) -> bool {
        crate::python::commands()
            .expect("the command list")
            .into_iter()
            .find(|command| command.id == id)
            .map(|command| command.enabled)
            .unwrap_or(false)
    }

    fn command_checked(id: &str) -> bool {
        crate::python::commands()
            .expect("the command list")
            .into_iter()
            .find(|command| command.id == id)
            .map(|command| command.checked)
            .unwrap_or(false)
    }

    /// The shell attaches and paints the 3D viewport headlessly. No CPython needed:
    /// it builds a default model, attaches a GPU renderer when one is available
    /// (else the placeholder), and forces a paint so the viewport render path runs.
    #[gpui::test]
    fn shell_paints_the_viewport(cx: &mut TestAppContext) {
        let model = Arc::new(Mutex::new(ShellModel::default()));
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        let attached = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.enable_viewport(cx);
                shell.viewport.is_some()
            })
        });
        println!("shell viewport attached: {attached}");

        // Force a full paint (this is where `InventorViewport::render` draws and
        // reads the frame back). Works with the placeholder too when no GPU exists.
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// G2: the `App_ToggleGrid` view request reaches the viewport, and a viewport that
    /// starts after the toggle picks the state up. Skips without a GPU adapter.
    #[gpui::test]
    fn a_grid_request_reaches_the_viewport(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // Turn the grid on **before** the viewport exists: `enable_viewport` must apply
        // the mirrored view state on creation.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
        });

        let attached = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.enable_viewport(cx);
                shell.viewport.is_some()
            })
        });
        if !attached {
            eprintln!("no GPU adapter available; skipping grid-to-viewport test");
            return;
        }

        let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
        cx.update(|_window, cx| {
            assert!(viewport.read(cx).grid_enabled(), "the boot state wins");
        });

        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
        });
        cx.update(|_window, cx| {
            assert!(!viewport.read(cx).grid_enabled(), "the toggle turns it off");
        });

        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
        });
        cx.update(|_window, cx| {
            assert!(viewport.read(cx).grid_enabled(), "and back on");
        });
    }

    /// G5: the grid preferences (steps and extent) are read from the parameter group by
    /// `App_ReloadGridSettings` and reach the viewport without a restart.
    #[gpui::test]
    fn a_grid_settings_request_reaches_the_viewport(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        let attached = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.enable_viewport(cx);
                shell.viewport.is_some()
            })
        });

        // A preference change is just a parameter write; the command applies it.
        let script = r#"
import FreeCAD
prefs = FreeCAD.ParamGet("User parameter:BaseApp/Preferences/View")
prefs.SetFloat("GridMinorStep", 2.0)
prefs.SetFloat("GridMajorStep", 20.0)
prefs.SetFloat("GridRadius", 750.0)
"#;
        let evaluated = cx.update(|_window, _cx| crate::python::evaluate(script));
        assert!(evaluated.unwrap().ok);
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("App_ReloadGridSettings", cx)
            })
        });

        assert_eq!(
            model.lock().unwrap().status,
            "grid minor 2, grid major 20, grid radius 750"
        );
        assert_eq!(model.lock().unwrap().view.grid_minor, 2.0);
        assert_eq!(model.lock().unwrap().view.grid_major, 20.0);
        assert_eq!(model.lock().unwrap().view.grid_radius, Some(750.0));

        if !attached {
            eprintln!("no GPU adapter available; skipping the viewport step assertion");
            return;
        }
        let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
        cx.update(|_window, cx| {
            assert_eq!(viewport.read(cx).grid_steps(), (2.0, 20.0));
            assert_eq!(viewport.read(cx).grid_radius(), Some(750.0));
        });
    }

    /// G5: a workplane action is viewport work, so a headless shell reports it cleanly.
    #[gpui::test]
    fn a_workplane_request_needs_a_viewport(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("App_ActivateWorkplane", cx)
            })
        });
        assert_eq!(model.lock().unwrap().status, "view: no viewport");
    }
}
