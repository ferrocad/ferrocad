//! `Shell` methods: command dispatch.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use pyo3::prelude::{PyAnyMethods, Python};

use super::*;

/// The id prefix for an interactive tool whose session lives in Python.
///
/// A command returns e.g. `python:draft`; `python/ferrocad_shell` serves the
/// `draft` bridge, which forwards viewport input to `FreeCADGui.draft`. Keeping
/// the prefix here (rather than a magic string in every path) also means the
/// edition's Rust tool registry and the Python bridge share one lookup.
pub(super) const PYTHON_TOOL_PREFIX: &str = "python:";

/// Build the Python-backed tool `name` through the shell bridge.
///
/// The bridge is a `ferrocad.stage.Tool` instance; [`ferrocad_stage_py::PyTool`]
/// makes it a native [`ferrocad_stage::Tool`], so the viewport treats it exactly
/// like the Rust tools. A missing facade or bridge is reported as "no such
/// tool" by the caller, never as a panic: a build without the Python stage
/// extension still runs the rest of the shell.
fn python_tool(name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
    Python::with_gil(|py| {
        let module = py.import("ferrocad_shell").map_err(|error| error.to_string())?;
        let object = module
            .getattr("tool_bridge")
            .map_err(|error| error.to_string())?
            .call1((name,))
            .map_err(|error| error.to_string())?;
        let tool = ferrocad_stage_py::extract_tool(py, object.unbind())
            .map_err(|error| error.to_string())?;
        Ok(Box::new(tool) as Box<dyn ferrocad_stage::Tool>)
    })
}

/// Send a session verb (`cancel`/`finish`) to a Python-backed tool.
fn python_tool_command(name: &str, verb: &str) -> bool {
    Python::with_gil(|py| {
        let result = py
            .import("ferrocad_shell")
            .and_then(|module| module.getattr("tool_command"))
            .and_then(|function| function.call1((name, verb)))
            .and_then(|value| value.extract::<bool>());
        match result {
            Ok(handled) => handled,
            Err(error) => {
                eprintln!("ferrocad: python tool {name}/{verb}: {error}");
                false
            }
        }
    })
}

impl Shell {
    /// The command a key event should run, if any.
    ///
    /// A modal owns the keyboard while it is open, and a disabled command does not
    /// fire (the same rule the bar shows).
    pub(super) fn shortcut_for(&self, event: &KeyDownEvent) -> Option<String> {
        // A modal owns the keyboard while any is open: a registry accelerator must
        // not fire over a dialog, a form, or while the user is typing a palette query.
        if self.modal_open() {
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

    /// Persist a GPU adapter row from the Dev Tools panel and rebuild the viewport.
    ///
    /// The row id is synthetic (`ferrocad-gpu:auto`, `ferrocad-gpu:adapter:<name>`),
    /// so it is intercepted before the registry dispatch. The value is validated by
    /// the Python helper and then applied by recreating the viewport through
    /// [`enable_viewport`](Self::enable_viewport): that path replaces `self.viewport`
    /// and its event subscription, so running it a second time is safe.
    ///
    /// The window compositor keeps its own adapter; the choice only reaches the
    /// image path while zero-copy presentation is active.
    pub(super) fn select_gpu_setting(&mut self, value: &str, cx: &mut Context<Self>) {
        let outcome = match crate::python::set_gpu_adapter(value) {
            Ok(outcome) => outcome,
            Err(error) => {
                let status = format!("viewport GPU: {error}");
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                cx.notify();
                return;
            }
        };

        if !outcome.ok {
            let status = if outcome.error.is_empty() {
                format!("viewport GPU: {value} rejected")
            } else {
                format!("viewport GPU: {}", outcome.error)
            };
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }

        self.gpu_preference = Some(outcome.value.clone());
        // Recreate the viewport so the new renderer is the one drawn. This is the
        // same path boot uses and replaces the entity and its subscription.
        self.enable_viewport(cx);

        let status = format!(
            "viewport GPU: {} (zero-copy keeps the window adapter)",
            outcome.value
        );
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Switch a feature's active provider from a header popover row or the Dev Tools
    /// providers panel.
    ///
    /// The row id is synthetic (`ferrocad-provider:<feature>:<backend>`), so it is
    /// intercepted before the registry dispatch. The bridge switches the backend,
    /// returns the merged presentation request, and the shell applies it through the
    /// same `apply_view` path a command uses.
    pub(crate) fn activate_provider(
        &mut self,
        feature: &str,
        backend: &str,
        cx: &mut Context<Self>,
    ) {
        // The row is in a popover (or the Dev Tools panel); choosing it dismisses
        // the menu.
        self.open_popover = None;
        let previous_backend = self.model.lock().unwrap().view.gizmo.backend.clone();
        let outcome = match crate::python::set_provider(feature, backend) {
            Ok(outcome) => outcome,
            Err(error) => {
                let status = format!("{feature}: {error}");
                self.model.lock().unwrap().status = status.clone();
                self.console_line(status, cx);
                cx.notify();
                return;
            }
        };

        if !outcome.ok {
            let status = if outcome.error.is_empty() {
                format!("{feature}: {backend} failed")
            } else {
                format!("{feature}: {}", outcome.error)
            };
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }

        let view_summary = outcome.view.as_ref().map(|request| {
            match self.apply_view(request, cx) {
                Ok(summary) => summary,
                Err(error) => format!("view: {error}"),
            }
        });

        // The cached summaries feed the open popover; a switch changes both the
        // active backend and its rows, so re-read them instead of waiting for the
        // next open. `refresh` also re-lists the commands, whose `IsChecked` now
        // follows the new provider.
        self.grid_summary = python::grid_summary().ok();
        self.axis_summary = python::axis_summary().ok();
        // A provider switch can change the settings schema the card shows.
        self.refresh_settings_card();
        self.provider_features = python::provider_features().ok();
        self.refresh(cx);

        // A switch can change *which* provider supplies the skin even when the backend
        // id is unchanged, so re-read it after the registry has switched. The
        // `apply_view` above already covered a backend change; this is the idempotent
        // second half, and it is what makes the report below honest for a same-id
        // provider change. A grid or axis switch cannot change the gizmo skin, so the
        // note is not evaluated for one at all.
        let skin_supplied = (feature == GIZMO_FEATURE).then(|| self.sync_gizmo_skin(cx));

        let mut status = if !outcome.status.is_empty() {
            outcome.status
        } else if let Some(summary) = view_summary.filter(|summary| !summary.is_empty()) {
            summary
        } else {
            format!("{feature}: {backend} is the active provider")
        };
        // Only a switched-to non-native backend with no supplied skin needs the note;
        // avoid repeating one `apply_view` already appended.
        if let Some(supplied) = skin_supplied {
            let current_backend = self.model.lock().unwrap().view.gizmo.backend.clone();
            if let Some(note) =
                gizmo_backend_note(&previous_backend, &current_backend, supplied)
            {
                if !status.contains(&note) {
                    status = format!("{status}; {note}");
                }
            }
        }
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Run a workbench command by id, then refresh the model and the stage.
    ///
    /// A command may instead return a **dialog** (A3): the spec is rendered as a
    /// modal and the shell accepts or cancels it later through its token. One
    /// dialog at a time — a second request while one is open is rejected and its
    /// token dropped, so the facade never keeps an unreachable callback.
    pub(crate) fn run_command(&mut self, id: &str, cx: &mut Context<Self>) {
        // Running any command dismisses the open popover: its menu has served its
        // purpose, and a new dialog should not sit under a still-open menu.
        self.open_popover = None;
        // A provider row is generated chrome, not a registry command: its id names
        // the feature and backend to switch to.
        if let Some(rest) = id.strip_prefix("ferrocad-provider:") {
            if let Some((feature, backend)) = rest.split_once(':') {
                self.activate_provider(feature, backend, cx);
                return;
            }
        }
        // A GPU adapter row is the same kind of generated chrome: the id names the
        // persisted value (`auto` or `adapter:<name>`) the row represents.
        if let Some(value) = id.strip_prefix("ferrocad-gpu:") {
            self.select_gpu_setting(value, cx);
            return;
        }
        // The Dev Tools toggle is shell chrome, not a workbench action: the window can
        // only exist when there is a GPUI app. The Python command stays registered so
        // every registry path (shortcut list, Dev Tools list, upstream script) sees it.
        if id == crate::dev_tools::TOGGLE_COMMAND {
            let status = self.toggle_dev_tools(cx);
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }
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

        // A gizmo-mode directive is session tool state. Apply it before a dialog:
        // `Move` returns `translate` (its Δ fields and the visible arrows must
        // agree) and opens the HUD in the same run.
        if let Some(mode) = outcome.gizmo.clone() {
            self.set_gizmo_mode(&mode, cx);
        }

        if let (Some(spec), Some(token)) = (outcome.dialog.clone(), outcome.token.clone()) {
            let status = if self.dialog.is_some() || self.form_dialog.is_some() {
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

        // A **select** directive (T5, S2) is not a failure: the command asks the
        // shell to show a model-tree node. A `Settings/<Feature>` node opens the
        // floating settings card **without** changing the selection — a settings
        // panel is a tool, not a selection, so the object being edited keeps its
        // selection and its gizmo. Every other node is a selection as before.
        let selected = outcome
            .select
            .clone()
            .filter(|name| !name.trim().is_empty());
        let changed_selection = match &selected {
            Some(name) if name.starts_with("Settings/") => {
                self.open_settings_card(name, cx);
                false
            }
            Some(name) => {
                self.select(name, cx);
                true
            }
            None => false,
        };

        self.refresh(cx);
        self.sync_stage(cx);
        // A settings action (the reset row, a provider switch) changes the rows the
        // card shows; this is a no-op while the card is closed.
        self.refresh_settings_card();
        if self.active_tool.is_none() && !changed_selection {
            // A command can move or resize a selected object (or undo such an edit);
            // keep its handles attached to the new geometry.
            self.rearm_selection(cx);
        }
        // A select directive brings its own status (`replace_selection` sets
        // it); a command's human status still wins when it supplies one.
        let status = if !outcome.status.is_empty() {
            outcome.status
        } else if selected.is_some() {
            self.model.lock().unwrap().status.clone()
        } else {
            format!("ran {id}")
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Run a command from an open popover **without dismissing the menu**.
    ///
    /// A **state** row — a command toggle or a generated provider choice — changes
    /// what the menu is showing. Closing after every click hides that change and turns
    /// a multi-select into a reopen-and-click cycle; keeping the menu open lets the
    /// dots follow `IsChecked` (or the new active provider), and the user closes it
    /// explicitly (an outside click, `Esc`, or the caret). One-shot action rows still
    /// use [`Self::run_command`] and dismiss the menu.
    ///
    /// A command that opens a modal wins: the menu is not restored under it.
    pub(crate) fn run_command_in_place(&mut self, id: &str, cx: &mut Context<Self>) {
        let popover = self.open_popover.clone();
        let card_before = self.settings_card.is_some();
        self.run_command(id, cx);
        // A run that opened a dialog, a form or the settings card owns the screen
        // now, so the menu must not stay on top of it even when the command reports
        // `IsChecked` (a settings command's check state means "this backend is
        // active", not "keep the menu open").
        let opened_surface = self.dialog.is_some()
            || self.form_dialog.is_some()
            || (self.settings_card.is_some() && !card_before);
        if !opened_surface {
            self.open_popover = popover;
        }
    }

    /// Apply a command's presentation request (A5, G5): perform a workplane action, or
    /// update the shell's view state, mirror it to `FerroCADGui.ViewState`, and apply it
    /// to the viewport when one exists. Returns a one-line summary for the status bar.
    pub(super) fn apply_view(
        &mut self,
        request: &ViewRequest,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        if let Some(action) = request.workplane_action()? {
            return self.apply_workplane(action, cx);
        }
        let mut state = self.model.lock().unwrap().view.clone();
        let previous_backend = state.gizmo.backend.clone();
        let mut summary = state.apply(request)?;
        let backend_changed = previous_backend != state.gizmo.backend;
        self.model.lock().unwrap().view = state.clone();
        if let Err(error) = python::set_view_state(&state) {
            self.console_line(format!("view state: {error}"), cx);
        }
        // A provider claim is stored, but only a skin its provider supplies can host
        // it (the native id is hosted by the build); say so rather than implying the
        // handles changed hands. The skin is re-read only when the backend changed,
        // because that is the only case this request can affect.
        let skin_supplied = if backend_changed {
            self.sync_gizmo_skin(cx)
        } else {
            false
        };
        if let Some(note) = gizmo_backend_note(&previous_backend, &state.gizmo.backend, skin_supplied)
        {
            summary = format!("{summary}; {note}");
        }
        if let Some(viewport) = self.viewport.clone() {
            let orthographic = state.projection == ProjectionName::Orthographic;
            let grid = state.grid;
            // A `view.axis` request carries the style; the deprecated `axis_cross` alias
            // still goes through `set_axis_cross`. Either way the style's `enabled` is the
            // single source of truth in the state.
            let axis = state.axis.clone();
            let axis_style_requested = request.axis.is_some();
            let axis_cross = state.axis_cross;
            let (minor, major, radius) = (state.grid_minor, state.grid_major, state.grid_radius);
            let (color, alpha) = (state.grid_color, state.grid_alpha);
            let grid_overlay = state.grid_overlay;
            let gizmo = state.gizmo.clone();
            viewport.update(cx, |viewport, cx| {
                let mut changed = viewport.set_orthographic(orthographic);
                changed |= viewport.set_grid(grid);
                changed |= if axis_style_requested {
                    viewport.set_axis_style(axis)
                } else {
                    viewport.set_axis_cross(axis_cross)
                };
                changed |= viewport.set_grid_steps(minor, major);
                changed |= viewport.set_grid_radius(radius);
                changed |= viewport.set_grid_color(color);
                changed |= viewport.set_grid_alpha(alpha);
                changed |= viewport.set_grid_overlay(grid_overlay);
                changed |= viewport.set_gizmo_style(gizmo);
                if changed {
                    cx.notify();
                }
            });
        }
        // The mirror above makes `IsChecked` see the new state.
        self.refresh_commands(cx);
        // A `{"view": …}` outcome is also how the fork says "the overlay tree changed"
        // (`grid_overlay`); pull it in without waiting for the next document refresh.
        self.sync_overlays(cx);
        Ok(summary)
    }

    /// Perform a workplane action on the viewport (G5). These are not presentation
    /// state: they act on the pointer hover, the sticky plane and the camera.
    pub(super) fn apply_workplane(
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

    /// Start an interactive tool through the edition's registry (A5), or through
    /// the Python bridge for a `python:<name>` id. The tool owns pointer events
    /// until it ends; no document transaction is opened. Its status is mirrored
    /// into the model immediately, so the tool strip appears even when no viewport
    /// is available (a headless window or a machine without a GPU).
    pub(super) fn start_tool(&mut self, id: &str, cx: &mut Context<Self>) -> String {
        let tool = match self.tool_for(id) {
            Ok(tool) => tool,
            Err(error) => return error,
        };
        let status = tool.status();
        self.active_tool = Some(id.to_string());
        self.model.lock().unwrap().tool = status.is_active().then_some(status);
        let mut pending = None;
        if let Some(viewport) = self.viewport.clone() {
            pending = viewport.update(cx, |viewport, cx| {
                // A tool session owns the pointer: the selection handles step aside.
                viewport.set_manipulators(Vec::new());
                viewport.set_gizmo(None);
                viewport.set_tool(Some(tool));
                cx.notify();
                viewport.take_tool_state()
            });
            // The gizmo is gone for the session, so the dock's contextual control
            // goes with it.
            self.model.lock().unwrap().gizmo_armed = false;
        }
        if let Some(status) = pending {
            self.on_viewport_event(&ViewportEvent::ToolState(status), cx);
        }
        cx.notify();
        format!("tool: {id}")
    }

    /// Resolve a tool id: the edition's Rust registry, or a Python bridge.
    pub(super) fn tool_for(
        &self,
        id: &str,
    ) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        if let Some(name) = id.strip_prefix(PYTHON_TOOL_PREFIX) {
            return python_tool(name).map_err(|error| format!("tool {id}: {error}"));
        }
        self.contributions
            .tool_for(id)
            .ok_or_else(|| format!("no such tool {id}"))
    }

    /// Cancel the active tool session (the tool strip's **Cancel** button). The tool
    /// clears its transient geometry and ends; the strip disappears. This is the same
    /// path as `Esc` and never writes the document. Returns whether a session existed.
    pub fn cancel_tool(&mut self, cx: &mut Context<Self>) -> bool {
        let had_tool = self.active_tool.is_some() || self.model.lock().unwrap().tool.is_some();
        // A Python session is not necessarily owned by a viewport (a headless
        // window has none), so end it here as well. The bridge action is
        // idempotent, so the viewport's `Tool::cancel` forwarding it again is safe.
        if let Some(name) = self
            .active_tool
            .as_deref()
            .and_then(|id| id.strip_prefix(PYTHON_TOOL_PREFIX))
        {
            python_tool_command(name, "cancel");
        }
        let mut pending = None;
        if let Some(viewport) = self.viewport.clone() {
            pending = viewport.update(cx, |viewport, cx| {
                let cancelled = viewport.cancel_tool();
                if cancelled {
                    cx.notify();
                }
                viewport.take_tool_state()
            });
        }
        if let Some(status) = pending {
            self.on_viewport_event(&ViewportEvent::ToolState(status), cx);
        }
        self.active_tool = None;
        self.model.lock().unwrap().tool = None;
        cx.notify();
        had_tool
    }

    /// Finish the active Python-backed tool session (`Enter`).
    ///
    /// The Python half commits through its own `doCommand` (a `Draft.make_line`
    /// for the Line tool), then the native session is dropped so the tool strip
    /// and the pointer both return to the shell. Returns whether a Python
    /// session was active.
    pub fn finish_tool(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(name) = self
            .active_tool
            .as_deref()
            .and_then(|id| id.strip_prefix(PYTHON_TOOL_PREFIX))
            .map(str::to_string)
        else {
            return false;
        };
        python_tool_command(&name, "finish");
        let mut pending = None;
        if let Some(viewport) = self.viewport.clone() {
            pending = viewport.update(cx, |viewport, cx| {
                let finished = viewport.finish_tool();
                if finished {
                    cx.notify();
                }
                viewport.take_tool_state()
            });
        }
        if let Some(status) = pending {
            self.on_viewport_event(&ViewportEvent::ToolState(status), cx);
        }
        self.active_tool = None;
        self.model.lock().unwrap().tool = None;
        // The commit ran through the Python `doCommand`, which the shell does not
        // see as a command result: refresh the model and rebuild the stage so the
        // new object appears without a manual reload.
        self.refresh(cx);
        self.sync_stage(cx);
        cx.notify();
        true
    }

}
