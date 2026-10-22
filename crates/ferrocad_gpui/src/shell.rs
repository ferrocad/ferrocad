//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a 3D viewport, a property editor
//! and a Python console. The layout and the interpreter wiring are the S1 deliverable:
//! the tree, editor and console are already driven by the live document through
//! `python/ferrocad_shell`; the viewport renders the sample scene until the geometry
//! kernel/`ViewProvider`s produce real geometry; richer widgets, commands and menus
//! come in later slices (see `docs/direction/app-shell-vision.md`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use gpui::{
    Anchor, AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Subscription, Window, WindowHandle, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};

use ferrocad_geom::SelectionRef;
use ferrocad_inventor::{Scene, Viewport};
use ferrocad_stage::{
    DeclarativeManipulator, InteractionCommit, Manipulator, ToolStatus, TransformGizmo,
};
use ferrocad_viewport::scene;
use ferrocad_viewport::{HoverReport, InventorViewport, SelectionEvent, ViewportEvent};
use ferrocad_wgpu::{AdapterChoice, AdapterSummary, WgpuRenderer};
use ferrocad_widgets::window_chrome::window_frame_with_leading;
use ferrocad_widgets::{SubmitEvent, TextAreaState, TextInput};

use crate::commands::{self, CommandInfo};
use crate::contributions::Contributions;
use crate::dialog::{self, DialogSpec, Kind, ValueState};
use crate::display::{DisplaySync, StageMesh};
use crate::manipulators;
use crate::python::{self, AxisSummary, Bootstrap, DocumentNode, GridSummary, PropRow};
use crate::selection::{self, SelectionItem};
use crate::shortcuts::Accelerator;
use crate::dev_tools::DevTools;
use crate::view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};

pub(crate) const BG: u32 = 0x0f1218;
pub(crate) const PANEL: u32 = 0x16181d;
pub(crate) const PANEL_ALT: u32 = 0x1d2026;
pub(crate) const FG: u32 = 0xd8dbe0;
pub(crate) const MUTED: u32 = 0x8b919a;
pub(crate) const ACCENT: u32 = 0x2f4a6d;

/// The registry feature whose provider supplies the gizmo skin.
const GIZMO_FEATURE: &str = "gizmo";

/// Whether two names address the same tree node.
///
/// Document objects match exactly. The synthetic `Settings` namespace accepts a
/// title-cased feature segment (`Settings/Grid`) as well as the registered id
/// (`Settings/grid`), because the Python resolver accepts both spellings.
fn same_node(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let alias = |candidate: &str, other: &str| {
        let Some((root, segment)) = candidate.split_once('/') else {
            return false;
        };
        root == "Settings"
            && other
                .strip_prefix("Settings/")
                .is_some_and(|live_segment| live_segment.eq_ignore_ascii_case(segment))
    };
    alias(left, right) || alias(right, left)
}

/// Whether `name` names a live tree node.
///
/// Document objects match exactly; a `Settings` node may use either spelling, so
/// a `select` directive is not pruned just because it uses the display form.
fn live_node(name: &str, live: &HashSet<String>) -> bool {
    live.contains(name) || live.iter().any(|candidate| same_node(name, candidate))
}

/// The console transcript: the log lines plus a fresh prompt.
fn transcript_of(model: &Arc<Mutex<ShellModel>>) -> String {
    let m = model.lock().unwrap();
    format!("{}\n>>> ", m.console.join("\n"))
}

/// The note to append when the view state accepts a `gizmo.backend` the build cannot
/// host.
///
/// [`ferrocad_stage::NATIVE_BACKEND`] is hosted by the build itself. Any other id is
/// hosted only by the skin its provider supplies through the registry; when that
/// provider supplies none, the shell says so rather than implying the handles changed
/// hands. Either way the native manipulator remains the event owner (hover, picking,
/// drag servicing and commits): the skin changes materials, never behaviour.
///
/// `None` when the backend did not change, when it names the native backend, or when
/// a skin was supplied.
fn gizmo_backend_note(previous: &str, current: &str, skin_supplied: bool) -> Option<String> {
    if previous == current || current == ferrocad_stage::NATIVE_BACKEND || skin_supplied {
        return None;
    }
    Some(format!(
        "gizmo backend {current} supplied no skin (native handles stay active)"
    ))
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
    /// The active tool's status while its session runs (the tool strip). `None` — or
    /// an inactive status — hides the strip.
    pub tool: Option<ToolStatus>,
    /// Whether the current selection armed a transform gizmo. The gizmo is a transient
    /// tool handle, so the dock's `Gizmo` control exists only while this is true.
    pub gizmo_armed: bool,
    pub console: Vec<String>,
    pub status: String,
}

impl ShellModel {
    /// The commands grouped by the surface that owns their control (P0.6, T0).
    pub fn surfaces(&self) -> commands::SurfaceGroups<'_> {
        commands::group_by_surface(&self.commands)
    }
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
    /// The window root's focus handle: `Esc` and the command shortcuts need a focused
    /// element in the main window now that the console lives in Dev Tools.
    root_focus: FocusHandle,
    /// The one open header popover, keyed by its control (`"grid"`, `"wp"`, `"mods"`).
    /// At most one is open at a time.
    open_popover: Option<String>,
    /// The window-space pointer position of the press that opened the popover.
    /// The flex header gives an `Anchored` child a negative layout origin, so the
    /// shell anchors from the actual event instead.
    popover_anchor: Option<Point<Pixels>>,
    /// The grid's provider and spacing, read once when its popover opens (T4).
    /// `None` until the first open, or after a settings edit invalidates it.
    grid_summary: Option<GridSummary>,
    /// The axis cross's provider and geometry, read once when its popover opens.
    /// `None` until the first open, or after a settings edit invalidates it.
    axis_summary: Option<AxisSummary>,
    /// Every provider feature for the Dev Tools providers panel, refreshed when the
    /// window opens and after a switch. `None` until the first open.
    provider_features: Option<Vec<crate::python::ProviderFeature>>,
    /// The persisted viewport GPU setting for the Dev Tools GPU panel. `None` until
    /// the window is opened or a viewport is created.
    gpu_preference: Option<String>,
    /// Every adapter the machine offers, for the GPU panel. Enumerated when the Dev
    /// Tools window opens, never per frame. `None` until the first open.
    gpu_adapters: Option<Vec<AdapterSummary>>,
    /// The modeless settings card, opened for a `Settings/<Feature>` node.
    settings_card: Option<SettingsCard>,
    /// The Dev Tools second window, if one has been opened. The window owns no model:
    /// its view reads this `Shell` entity. A closed window leaves the handle stale
    /// until the next toggle notices and reopens.
    dev_tools: Option<WindowHandle<DevTools>>,
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

/// The modeless settings card (A3/T5): a draggable 380 px panel over the canvas
/// with no backdrop, so the viewport stays interactive. It shows the selected
/// `Settings/<Feature>` node through the same property rows the Dev Tools pane uses.
struct SettingsCard {
    /// Window coordinates of the card's top-left corner.
    position: Point<Pixels>,
    /// Pointer and card origin when a drag starts.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
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
            tool: None,
            gizmo_armed: false,
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
            root_focus: cx.focus_handle(),
            open_popover: None,
            popover_anchor: None,
            grid_summary: None,
            axis_summary: None,
            provider_features: None,
            gpu_preference: None,
            gpu_adapters: None,
            settings_card: None,
            dev_tools: None,
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

    /// Whether the Dev Tools window is currently open.
    pub fn dev_tools_open(&self, cx: &App) -> bool {
        self.dev_tools
            .and_then(|handle| cx.window_handle(handle.window_id()))
            .is_some()
    }

    /// Open, focus or close the Dev Tools window (`Dev_ToggleTools`, `Ctrl+Shift+D`).
    ///
    /// The window is a second view over this same `Shell` entity — it never owns the
    /// model and never boots a second interpreter. Opening is deferred by one effect
    /// cycle: building the window draws its first frame, and that frame reads this
    /// entity, so it must not happen while the caller already holds it mutably (the
    /// command path does).
    ///
    /// Returns the status line for the toggle; the deferred open replaces it with
    /// `dev tools: open` once the window exists.
    pub fn toggle_dev_tools(&mut self, cx: &mut Context<Self>) -> String {
        if self.dev_tools_open(cx) {
            let Some(handle) = self.dev_tools.take() else {
                return "dev tools: closed".to_string();
            };
            return match handle.update(cx, |_view, window, _cx| window.remove_window()) {
                Ok(()) => "dev tools: closed".to_string(),
                Err(_) => "dev tools: window already closed".to_string(),
            };
        }
        // The handle may name a window the user closed with the title bar.
        self.dev_tools = None;
        self.open_dev_tools(cx)
    }

    /// Create the Dev Tools window around this shell entity.
    fn open_dev_tools(&mut self, cx: &mut Context<Self>) -> String {
        // The providers and GPU panels read their state once per open, not per frame.
        self.provider_features = python::provider_features().ok();
        self.gpu_preference = Some(python::gpu_adapter().unwrap_or_else(|_| "auto".to_string()));
        self.gpu_adapters = Some(ferrocad_wgpu::adapter::adapters());
        let shell = cx.entity();
        cx.defer(move |cx| {
            let handle = cx.open_window(crate::dev_tools::window_options(), {
                let shell = shell.clone();
                move |_window, cx| cx.new(|cx| DevTools::new(shell, cx))
            });
            match handle {
                Ok(handle) => {
                    let _ = handle.update(cx, |view, window, cx| {
                        window.activate_window();
                        window.focus(&view.focus_handle(cx), cx);
                    });
                    shell.update(cx, |shell, cx| {
                        shell.dev_tools = Some(handle);
                        shell.model.lock().unwrap().status = "dev tools: open".to_string();
                        cx.notify();
                    });
                }
                Err(error) => {
                    shell.update(cx, |shell, cx| {
                        let status = format!("dev tools: cannot open a window ({error})");
                        shell.model.lock().unwrap().status = status.clone();
                        shell.console_line(status, cx);
                        cx.notify();
                    });
                }
            }
        });
        "dev tools: opening".to_string()
    }

    /// The main window root's focus handle, so boot can focus the shell (the console
    /// now lives in Dev Tools and is not in this window's element tree).
    pub fn root_focus_handle(&self) -> FocusHandle {
        self.root_focus.clone()
    }

    /// Every provider feature and its backends for the Dev Tools providers panel.
    /// Empty until the Dev Tools window is opened (or a switch refreshes it).
    pub(crate) fn provider_features(&self) -> &[crate::python::ProviderFeature] {
        self.provider_features.as_deref().unwrap_or(&[])
    }

    /// The persisted GPU setting, for the Dev Tools GPU panel's selected row.
    /// Empty until the window opens or a viewport is created.
    pub(crate) fn gpu_setting(&self) -> &str {
        self.gpu_preference.as_deref().unwrap_or("auto")
    }

    /// Every adapter the machine offers, for the Dev Tools GPU panel.
    /// Empty until the Dev Tools window has been opened once.
    pub(crate) fn gpu_adapters(&self) -> &[AdapterSummary] {
        self.gpu_adapters.as_deref().unwrap_or(&[])
    }

    /// The adapter the viewport's own renderer runs on, if there is a viewport.
    ///
    /// Zero-copy presentation builds a separate renderer on the window's adapter;
    /// this reports the image-path renderer's adapter (see the panel's note).
    pub(crate) fn in_use_adapter(&self, cx: &App) -> Option<AdapterSummary> {
        self.viewport
            .as_ref()
            .map(|viewport| viewport.read(cx).view().adapter_info())
    }

    /// Whether the popover keyed `key` is the open one.
    pub(crate) fn popover_open(&self, key: &str) -> bool {
        self.open_popover.as_deref() == Some(key)
    }

    /// Open a header popover. Opening never runs the control's primary command, and
    /// replaces any other open popover (one at a time).
    pub(crate) fn open_popover(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.popover_open(key) {
            self.open_popover = Some(key.to_string());
        }
        // The Grid and Axis menus are the provider-aware popovers (T4): read their
        // summary once when they open, not on every frame. A failure leaves the rows
        // out (and reports why) rather than showing values from a previous open.
        if key == "grid" {
            match python::grid_summary() {
                Ok(summary) => self.grid_summary = Some(summary),
                Err(error) => {
                    self.grid_summary = None;
                    self.console_line(format!("grid: {error}"), cx);
                }
            }
        }
        if key == "axis" {
            match python::axis_summary() {
                Ok(summary) => self.axis_summary = Some(summary),
                Err(error) => {
                    self.axis_summary = None;
                    self.console_line(format!("axis: {error}"), cx);
                }
            }
        }
        // The dock's gizmo menu is provider-aware like Grid and Axes: the rows are the
        // registered `gizmo` backends, so read the registry once when it opens. Without
        // this the menu showed only its config rows and the two providers (native and
        // shader) were invisible.
        if key == "gizmo" {
            match python::provider_features() {
                Ok(features) => self.provider_features = Some(features),
                Err(error) => {
                    self.provider_features = None;
                    self.console_line(format!("providers: {error}"), cx);
                }
            }
        }
        cx.notify();
    }

    /// Close whichever popover is open; returns whether one was open.
    pub(crate) fn close_popover(&mut self, cx: &mut Context<Self>) -> bool {
        self.popover_anchor = None;
        if self.open_popover.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Record where the pointer pressed the control that owns the popover.
    pub(crate) fn set_popover_anchor(&mut self, position: Point<Pixels>) {
        self.popover_anchor = Some(position);
    }

    /// Open the modeless settings card for a `Settings/<Feature>` selection.
    ///
    /// The card keeps its last position and size when reopened; it is the visible
    /// surface T5 previously reached only through the Dev Tools property pane.
    fn open_settings_card(&mut self, cx: &mut Context<Self>) {
        if self.settings_card.is_none() {
            self.settings_card = Some(SettingsCard {
                position: point(px(220.), px(96.)),
                drag: None,
            });
            cx.notify();
        }
    }

    /// Close the settings card; returns whether it was open.
    fn close_settings_card(&mut self, cx: &mut Context<Self>) -> bool {
        if self.settings_card.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Start dragging the card from its header.
    fn begin_settings_drag(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(card) = &mut self.settings_card {
            card.drag = Some((pointer, card.position));
            cx.notify();
        }
    }

    /// Follow a left-button drag; moves outside the header still count because the
    /// shell root forwards its pointer moves here.
    fn drag_settings_card(
        &mut self,
        pointer: Point<Pixels>,
        pressed: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if pressed != Some(MouseButton::Left) {
            return;
        }
        let Some(card) = &mut self.settings_card else {
            return;
        };
        let Some((start_pointer, start_position)) = card.drag else {
            return;
        };
        card.position = point(
            start_position.x + pointer.x - start_pointer.x,
            start_position.y + pointer.y - start_pointer.y,
        );
        cx.notify();
    }

    fn end_settings_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(card) = &mut self.settings_card {
            if card.drag.take().is_some() {
                cx.notify();
            }
        }
    }

    /// The dock's `Search (Ctrl+K)` slot: a stub until the command palette exists.
    /// It reports where the commands actually are rather than pretending to search.
    pub(crate) fn search_stub(&mut self, cx: &mut Context<Self>) {
        let status =
            "search: no command palette yet — Dev Tools (Ctrl+Shift+D) lists every command"
                .to_string();
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// The shell's global key path, shared by both windows.
    ///
    /// `cancel_dialog` is enabled when the event comes from the Dev Tools window: the
    /// modal lives in the main window, and `Esc` there must still reach it. The main
    /// window keeps the modal's own focused `Esc` handling.
    pub(crate) fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        cancel_dialog: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key == "escape" && self.close_popover(cx) {
            return true;
        }
        if event.keystroke.key == "escape" {
            if cancel_dialog && self.dialog.is_some() {
                self.cancel_dialog(cx);
                return true;
            }
            if self.dialog.is_none() && self.active_tool.is_some() {
                self.cancel_tool(cx);
                return true;
            }
            // The settings card is modeless, so Esc is its normal dismissal.
            if self.dialog.is_none() && self.close_settings_card(cx) {
                return true;
            }
        }
        if let Some(id) = self.shortcut_for(event) {
            self.run_command(&id, cx);
            return true;
        }
        false
    }

    /// The persisted adapter choice, falling back to [`AdapterChoice::Auto`] with a
    /// console line when the stored value is malformed.
    ///
    /// The setting is read across the Python bridge (file-backed preferences); a
    /// parse failure is a result the user can see and fix, not a fatal error.
    fn stored_gpu_choice(&mut self, cx: &mut Context<Self>) -> AdapterChoice {
        let value = match python::gpu_adapter() {
            Ok(value) => value,
            Err(error) => {
                self.console_line(format!("viewport GPU: {error}; using auto"), cx);
                return AdapterChoice::Auto;
            }
        };
        // Keep the panel's selected row in step with the persisted value.
        self.gpu_preference = Some(value.clone());
        match AdapterChoice::parse(&value) {
            Ok(choice) => choice,
            Err(error) => {
                self.console_line(format!("viewport GPU: {error}; using auto"), cx);
                AdapterChoice::Auto
            }
        }
    }

    /// Create the 3D viewport, best-effort: it needs a GPU.
    ///
    /// Called by the app after `from_model` (not by `from_model` itself), so headless
    /// tests stay GPU-free and render the placeholder. If no adapter is available the
    /// pane keeps the placeholder too.
    ///
    /// The adapter comes from the persisted setting (`Adapters` panel); a malformed
    /// value falls back to **Auto** with a console line. The window compositor keeps
    /// its own adapter, so a forced choice only reaches the image path.
    ///
    /// The scene is built the workbench way — a `from pivy import coin` script
    /// extracted into a [`ferrocad_inventor::Scene`] ([`python::inventor_scene`]) —
    /// falling back to the Rust sample if the interpreter is unavailable.
    pub fn enable_viewport(&mut self, cx: &mut Context<Self>) {
        // A bootstrap size only: `InventorViewport` resizes the render target and the
        // camera to its element's physical size on the first frame (see `sync_size`).
        const SIZE: [u32; 2] = [640, 480];
        let choice = self.stored_gpu_choice(cx);
        let renderer = match WgpuRenderer::new_with_choice(SIZE, &choice) {
            Ok(renderer) => renderer,
            Err(error) => {
                // Auto keeps the old silent "no GPU" path; an explicit choice that
                // cannot be satisfied is worth reporting.
                if choice != AdapterChoice::Auto {
                    self.console_line(format!("viewport GPU: {error}"), cx);
                }
                return;
            }
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
            // A document's scene is usually not the sample camera's scale: frame it once
            // so the viewport does not open from inside the model.
            viewport.frame_stage();
        }
        // G5: overlays registered through `FerroCADGui.addOverlay` before the viewport
        // existed are pulled in now, once the scene they merge beside is built.
        if ferrocad_inventor_py::overlays_dirty() {
            let overlays = ferrocad_inventor_py::overlay_scenes();
            viewport.set_overlays(overlays);
        }
        // The view state is the source of truth: a grid toggled before the viewport
        // existed is applied now, with its steps.
        //
        // G2: the skin is supplied by the active provider through the registry, not
        // shipped by the app. Fetch it before boxing the viewport; the viewport itself
        // stays GPU- and source-agnostic.
        let gizmo_skin = self.active_gizmo_skin(cx);
        {
            let state = self.model.lock().unwrap().view.clone();
            viewport.set_grid(state.grid);
            // The whole style is the source of truth (the boolean alias is folded into
            // `axis.enabled` before it reaches the state).
            viewport.set_axis_style(state.axis.clone());
            viewport.set_grid_steps(state.grid_minor, state.grid_major);
            viewport.set_grid_radius(state.grid_radius);
            viewport.set_grid_overlay(state.grid_overlay);
            // The gizmo style is host policy too: a gizmo disabled or restyled before
            // the viewport existed is applied now, and stays applied to selections
            // armed later.
            viewport.set_gizmo_style(state.gizmo.clone());
            viewport.set_gizmo_skin(gizmo_skin);
        }
        // A tool started before the viewport existed (a script, a command while no
        // GPU was available) is applied now that there is one.
        if let Some(id) = self.active_tool.as_deref() {
            if let Some(tool) = self.contributions.tool_for(id) {
                viewport.set_tool(Some(tool));
            }
        }
        if let Some(error) = viewport.take_shader_error() {
            self.console_line(format!("shader: {error}"), cx);
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

    /// Pull the registered overlay scenes into the viewport when the set changed (G5).
    ///
    /// The registry is the authority; this is deliberately a pull so the shell never
    /// re-extracts or recompiles a scene whose overlay set is unchanged. Registration
    /// is enough to draw: the viewport does not gate the program queue on
    /// `grid_overlay`, which only hides the native ground. With no
    /// viewport (headless, or before [`enable_viewport`](Self::enable_viewport)) the
    /// dirty flag is left set, so the next viewport applies it.
    fn sync_overlays(&mut self, cx: &mut Context<Self>) {
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
    fn active_gizmo_skin(&mut self, cx: &mut Context<Self>) -> Option<Arc<str>> {
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
    fn sync_gizmo_skin(&mut self, cx: &mut Context<Self>) -> bool {
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
            ViewportEvent::ToolState(status) => self.on_tool_state(status, cx),
        }
    }

    /// A tool started, gained a point, finished or was cancelled: mirror its status
    /// into the model so the tool strip shows the label, point count and hint, and
    /// hides as soon as the session is over. The document is never touched here.
    fn on_tool_state(&mut self, status: &ToolStatus, cx: &mut Context<Self>) {
        let active = status.is_active();
        if !active {
            self.active_tool = None;
        }
        self.model.lock().unwrap().tool = active.then(|| status.clone());
        cx.notify();
    }

    /// A click: select the object in the inspector, and arm the manipulators and gizmo
    /// the edition's workbenches and mods contribute for that selection.
    fn on_selection(&mut self, selection: &SelectionEvent, cx: &mut Context<Self>) {
        let (manipulators, gizmo) = match selection {
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
    fn arm(
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
                gizmo = self.contributions.gizmo_for(reference, &stage);
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

    /// Arm the transform gizmo for the current selection (the dock's `Move` label).
    ///
    /// The label is the interactive tool; the numeric command stays on the caret and
    /// on its shortcut. A selection made in the tree carries a name rather than a pick
    /// id, so it is resolved through the display map — that is what makes `Move` work
    /// without first clicking the object in the viewport.
    ///
    /// Re-arming for the same selection is idempotent (a fresh `TransformGizmo` at the
    /// item's centre), so clicking the tool twice never leaves two handles behind.
    pub(crate) fn arm_dock_tool(&mut self, key: &str, cx: &mut Context<Self>) {
        if key != "gizmo" {
            return;
        }
        let status = self.arm_transform_gizmo(cx);
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// The arm step behind [`arm_dock_tool`](Self::arm_dock_tool), returning the status.
    fn arm_transform_gizmo(&mut self, cx: &mut Context<Self>) -> String {
        let Some(object) = self.model.lock().unwrap().selected.clone() else {
            return "select an object to transform".to_string();
        };
        let Some(viewport) = self.viewport.clone() else {
            return "no viewport".to_string();
        };
        let Some(entity) = self.display.entity_of(&object) else {
            return format!("{object} is not displayed");
        };
        let stage = viewport.read(cx).stage().clone();
        let gizmo = self
            .contributions
            .gizmo_for(&SelectionRef::whole(entity), &stage);
        let armed = gizmo.is_some();
        viewport.update(cx, |viewport, cx| {
            viewport.set_gizmo(gizmo);
            cx.notify();
        });
        self.model.lock().unwrap().gizmo_armed = armed;
        if armed {
            format!("{object}: drag the gizmo")
        } else {
            format!("{object}: this selection has no transform handle")
        }
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
            // A recompute drops the gizmo with the old selection; the dock's
            // contextual control must go with it.
            self.model.lock().unwrap().gizmo_armed = false;
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
    fn select_gpu_setting(&mut self, value: &str, cx: &mut Context<Self>) {
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

        // A **select** directive (T5, S2) is not a failure: the command asks the
        // shell to show a model-tree node — the synthetic `Settings/<Feature>`
        // root — so the Properties pane renders its rows. It goes through the
        // same selection path a tree click uses, so the pane, the facade
        // snapshot and every command's `IsActive` agree.
        let selected = outcome
            .select
            .clone()
            .filter(|name| !name.trim().is_empty());
        if let Some(name) = &selected {
            self.select(name, cx);
            if name.starts_with("Settings/") {
                self.open_settings_card(cx);
            }
        }

        self.refresh(cx);
        self.sync_stage(cx);
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
    /// update the shell's view state, mirror it to `FerroCADGui.ViewState`, and apply it
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
    /// pointer events until it ends; no document transaction is opened. Its status is
    /// mirrored into the model immediately, so the tool strip appears even when no
    /// viewport is available (a headless window or a machine without a GPU).
    fn start_tool(&mut self, id: &str, cx: &mut Context<Self>) -> String {
        let Some(tool) = self.contributions.tool_for(id) else {
            return format!("no such tool {id}");
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

    /// Cancel the active tool session (the tool strip's **Cancel** button). The tool
    /// clears its transient geometry and ends; the strip disappears. This is the same
    /// path as `Esc` and never writes the document. Returns whether a session existed.
    pub fn cancel_tool(&mut self, cx: &mut Context<Self>) -> bool {
        let had_tool = self.active_tool.is_some() || self.model.lock().unwrap().tool.is_some();
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
        let settings_node = primary
            .as_deref()
            .is_some_and(|name| name.starts_with("Settings/"));
        {
            let mut m = self.model.lock().unwrap();
            m.selection = items;
            m.selected = primary;
            m.properties = properties;
            m.status = status;
        }
        if settings_node {
            self.open_settings_card(cx);
        } else {
            self.settings_card = None;
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
        let live: HashSet<String> = tree
            .iter()
            .flat_map(|doc| doc.objects.iter().map(|obj| obj.name.clone()))
            .collect();
        let (next, primary) = {
            let m = self.model.lock().unwrap();
            let mut next = m.selection.clone();
            next.retain(|item| live_node(&item.object, &live));
            let primary = next
                .last()
                .map(|item| item.object.clone())
                .or_else(|| m.selected.clone().filter(|name| live_node(name, &live)));
            (next, primary)
        };
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        // A document selection has no settings card; a `Settings/...` node keeps it.
        if !primary
            .as_deref()
            .is_some_and(|name| name.starts_with("Settings/"))
        {
            self.settings_card = None;
        }
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
        self.sync_overlays(cx);
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
                // A settings edit can switch the provider or change the spacing
                // the Grid popover advertises; re-read the summary so an open
                // popover follows the row the user just changed. The axis cross's
                // provider and geometry follow the same rule.
                if key.starts_with("Settings/") {
                    self.grid_summary = python::grid_summary().ok();
                    self.axis_summary = python::axis_summary().ok();
                    self.provider_features = python::provider_features().ok();
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

/// The tool status strip: the active tool's label, point count, hint and a **Cancel**
/// button. It is drawn above the viewport only while a tool session is active (see
/// `ShellModel::tool`).
fn tool_strip(status: &ToolStatus, cx: &mut Context<Shell>) -> impl IntoElement {
    div()
        .id("tool-strip")
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .border_1()
        .border_color(rgba(0xffffff14))
        .text_xs()
        .child(div().text_color(rgb(FG)).child(status.label.clone()))
        .child(div().text_color(rgb(MUTED)).child(format!(
            "{} point{}",
            status.points,
            if status.points == 1 { "" } else { "s" }
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(MUTED))
                .child(status.hint.clone()),
        )
        .child(
            div()
                .id("tool-cancel")
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(ACCENT))
                .text_color(rgb(FG))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL)))
                .child("Cancel")
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.cancel_tool(cx);
                })),
        )
}

/// A slim panel header bar.
pub(crate) fn panel_header(text: &str) -> impl IntoElement {
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
pub(crate) fn tree_panel(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
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
            if model
                .selection
                .iter()
                .any(|item| same_node(&item.object, &name))
            {
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
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "tree-panel".to_string())
        .child(panel_header("Model"))
        .child(rows)
}

/// The property editor: read-only rows plus an uncontrolled field for each
/// editable property.
pub(crate) fn properties_panel(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    title: Option<&str>,
    cx: &mut Context<Shell>,
) -> impl IntoElement + use<> {
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
                    let selector = format!("prop-{key}");
                    div()
                        .debug_selector(move || selector.clone())
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

    let panel = div()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "properties-panel".to_string());
    let panel = match title {
        Some(title) => panel.child(panel_header(title)),
        None => panel,
    };
    panel.child(rows)
}

/// The title the settings card uses for a synthetic node (`Settings/Grid` ->
/// `Grid settings`).
fn settings_title(name: &str) -> String {
    let feature = name.strip_prefix("Settings/").unwrap_or(name);
    let mut chars = feature.chars();
    let title = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    format!("{title} settings")
}

/// The modeless settings card (A3/T5): a draggable `380 px` panel over the canvas.
///
/// It has no backdrop, renders the same rows as the Dev Tools property pane, and is
/// dismissed with its `✕` or `Esc`. The position is window coordinates so it survives
/// a drag across the canvas.
fn settings_card_view(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let position = shell
        .settings_card
        .as_ref()
        .map(|card| card.position)
        .unwrap_or_else(|| point(px(220.), px(96.)));
    let title = model
        .selected
        .as_deref()
        .map(settings_title)
        .unwrap_or_else(|| "Settings".to_string());

    let header = div()
        .id("settings-card-header")
        .debug_selector(|| "settings-card-header".to_string())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(FG))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.begin_settings_drag(event.position, cx);
        }))
        .child(title)
        .child(
            div()
                .id("settings-card-close")
                .debug_selector(|| "settings-card-close".to_string())
                .px_1()
                .rounded_sm()
                .text_color(rgb(MUTED))
                .hover(|style| style.bg(rgb(PANEL)))
                .child("✕")
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.close_settings_card(cx);
                })),
        );

    let panel = properties_panel(shell, model, window, None, cx);
    let card = div()
        .id("settings-card")
        .debug_selector(|| "settings-card".to_string())
        // The card floats over the canvas: own the hitbox so a press/drag inside it
        // cannot orbit or pick the viewport underneath.
        .occlude()
        .flex()
        .flex_col()
        .w(px(380.))
        .h(px(420.))
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .overflow_hidden()
        // `occlude` blocks the shell root's pointer moves while the drag is over the
        // card, so the card follows the drag itself.
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.drag_settings_card(event.position, event.pressed_button, cx);
        }))
        .on_mouse_up(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
            this.end_settings_drag(cx);
        }))
        .child(header)
        .child(div().flex_1().min_h_0().overflow_hidden().child(panel));

    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .position(position)
            .snap_to_window_with_margin(px(8.))
            .child(card),
    )
    .with_priority(2)
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
pub(crate) fn console_panel(
    console: &Entity<TextAreaState>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
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
        .debug_selector(|| "console-panel".to_string())
        .child(panel_header("Python console"))
        .child(console.clone())
        .child(actions)
}

/// The Dev Tools window's body: the whole command surface, the inspector, the property
/// editor and the console in one column (P0.5).
///
/// It is built from the shell's `Context`, so every row runs against the same `Shell`
/// entity the main window renders. The viewport is deliberately **not** here: gpui can
/// render an entity in two windows, but the 3D viewport must only be drawn once.
pub(crate) fn dev_tools_body(
    shell: &mut Shell,
    focus: &FocusHandle,
    window: &Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let model = shell.snapshot();
    // The adapter is read from the viewport entity once per frame; the list itself
    // is cached when the window opens.
    let in_use = shell.in_use_adapter(cx);
    // The commands the losing provider owns: listed, but not runnable (P0 provider UX).
    let inactive = crate::chrome::inactive_provider_commands(shell.provider_features());
    div()
        .id("dev-tools-root")
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(BG))
        .text_color(rgb(FG))
        .debug_selector(|| "dev-tools".to_string())
        .track_focus(focus)
        .focusable()
        // Commands and shortcuts run from either window. `Esc` also reaches the modal
        // and tool paths, which stay on the shell root.
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            if this.handle_key(event, true, cx) {
                cx.stop_propagation();
            }
        }))
        .child(crate::chrome::providers_panel(shell.provider_features(), cx))
        .child(crate::chrome::gpu_panel(
            shell.gpu_setting(),
            shell.gpu_adapters(),
            in_use.as_ref(),
            cx,
        ))
        .child(crate::chrome::command_list_panel(&model, &inactive, cx))
        .child(tree_panel(&model, cx))
        .child(properties_panel(shell, &model, window, Some("Properties"), cx))
        .child(console_panel(&shell.console, cx))
        .into_any_element()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.snapshot();
        // The app menu shows the window's real state; the registry cannot know it.
        let dev_tools_open = self.dev_tools_open(cx);

        // A modal was opened outside a window context; focus it once rendered so
        // Esc cancels and the panel receives bubbled keys.
        if std::mem::take(&mut self.focus_dialog) {
            window.focus(&self.dialog_focus, cx);
        }

        // The pane is the canvas now: the environment controls float over it as a
        // compact pill (see `chrome::viewport_header`) instead of taking a header row,
        // so the scene reaches the title bar.
        let viewport = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(BG))
            .debug_selector(|| "main-viewport".to_string());
        // The tool strip is transient session feedback, not chrome; it stays a normal
        // row above the canvas and vanishes when the tool finishes or is cancelled.
        let viewport = match model.tool.as_ref().filter(|tool| tool.is_active()) {
            Some(tool) => viewport.child(tool_strip(tool, cx)),
            None => viewport,
        };
        // The canvas hosts the viewport, the floating control pill (top-left) and the
        // action dock (bottom-centre), which both have their own hitboxes so the scene
        // still receives pointer events around them.
        let canvas = div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .debug_selector(|| "main-canvas".to_string())
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
            })
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .child(crate::chrome::viewport_header(
                        &model,
                        self.grid_summary.as_ref(),
                        self.axis_summary.as_ref(),
                        self.open_popover.as_deref(),
                        self.popover_anchor,
                        cx,
                    )),
            )
            .child(crate::chrome::action_dock(
                &model,
                self.provider_features.as_deref().unwrap_or(&[]),
                self.open_popover.as_deref(),
                self.popover_anchor,
                cx,
            ));
        let viewport = viewport.child(canvas);

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

        // The reset: the main window is the frame, the viewport and the status line.
        // The actions, inspector, property editor and console live in Dev Tools, and a
        // modal still opens here, over the canvas the user is looking at.
        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(viewport)
            .child(status);

        let body = div()
            .id("shell-root")
            .relative()
            .size_full()
            .debug_selector(|| "main-shell".to_string())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if this.handle_key(event, false, cx) {
                    cx.stop_propagation();
                }
            }))
            // The settings card is dragged from its header, but the shell root sees
            // the moves so a fast drag does not stop at the card edge.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.drag_settings_card(event.position, event.pressed_button, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                this.end_settings_drag(cx);
            }))
            .track_focus(&self.root_focus)
            .focusable()
            .child(content);
        let body = if self.dialog.is_some() {
            body.child(dialog_overlay(self, cx))
        } else if self.settings_card.is_some() {
            body.child(settings_card_view(self, &model, window, cx))
        } else {
            body
        };
        // The app menu lives in the window title bar, not the viewport header: it is
        // app-level chrome, and the title bar is the one place a native-looking app
        // menu belongs. The Dev Tools window passes no leading element.
        let leading = crate::chrome::app_menu(
            self.popover_open("app"),
            &model,
            dev_tools_open,
            cx,
        )
        .into_any_element();
        window_frame_with_leading(window, model.title.clone(), Some(leading), body)
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

        // The console lives in Dev Tools now: open it, then drive the same focus path.
        let mut dev_cx = open_dev_tools(&mut cx, &shell);

        // Typing in the console input and pressing Enter runs the line through the
        // interpreter (S5 previewed): the log gains the echoed line and the result,
        // and the field is cleared.
        let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
        dev_cx.update(|window, cx| window.focus(&console, cx));
        dev_cx.simulate_input("len(doc.Objects)");
        dev_cx.simulate_keystrokes("enter");
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

        // Select Params so the editor builds one field per editable property. The
        // property editor is in Dev Tools now, so the window must be open and drawn
        // before the field exists.
        let mut dev_cx = open_dev_tools(&mut cx, &shell);
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Params", cx);
                cx.notify();
            });
        });
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let field = shell.read_with(&cx, |shell, _| {
            shell
                .prop_inputs
                .get("Params.Length")
                .expect("Length field")
                .state
                .clone()
        });
        // The field is rendered in the Dev Tools window; type there.
        dev_cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
        dev_cx.simulate_keystrokes("cmd-a");
        dev_cx.simulate_input("25 mm");
        dev_cx.simulate_keystrokes("enter");

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

    /// G2: a dialog callback may return a `{"view": …}` request. `finish_dialog`
    /// applies it exactly as a plain command's request is applied.
    #[gpui::test]
    fn a_dialog_view_request_changes_the_view_state(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        let script = r#"
import FreeCADGui
class _DialogView:
    def GetResources(self):
        return {"MenuText": "Dialog view"}
    def Activated(self):
        return {
            "title": "Dialog view",
            "fields": [{"name": "Show", "kind": "bool", "value": True}],
            "on_accept": lambda values: {"view": {"grid": bool(values["Show"])}},
        }
if "Test_DialogView" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_DialogView", _DialogView())
"#;
        let registered = crate::python::evaluate(script).expect("evaluate");
        assert!(registered.ok, "{}", registered.output);

        assert!(!model.lock().unwrap().view.grid, "the grid starts off");
        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_DialogView", cx)));
        assert!(
            cx.update(|cx| shell.read(cx).dialog.is_some()),
            "the dialog opened"
        );

        cx.update(|cx| shell.update(cx, |shell, cx| shell.accept_dialog(cx)));
        assert!(
            cx.update(|cx| shell.read(cx).dialog.is_none()),
            "the dialog closed"
        );
        let (grid, status) = {
            let m = model.lock().unwrap();
            (m.view.grid, m.status.clone())
        };
        assert!(grid, "the callback's request reached the view state");
        assert_eq!(status, "grid on");
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
        // bubbles to the Dev Tools root and runs the command — the same shell path as
        // a click, from either window.
        let mut dev_cx = open_dev_tools(&mut cx, &shell);
        let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
        dev_cx.update(|window, cx| window.focus(&console, cx));
        dev_cx.simulate_keystrokes("ctrl-shift-k");

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
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
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
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        assert!(model.lock().unwrap().view.axis_cross, "on by default");
        assert!(command_checked("App_ToggleAxisCross"));

        cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleAxisCross", cx))
        });
        assert_eq!(model.lock().unwrap().status, "axis off");
        assert!(!model.lock().unwrap().view.axis_cross);
        assert!(!model.lock().unwrap().view.axis.enabled);
        assert!(!model.lock().unwrap().view.grid, "the grid is untouched");
        assert!(!command_checked("App_ToggleAxisCross"));
    }

    /// The parameterized `view.axis` request reaches the shell's view state; the bool
    /// `axis_cross` stays the mirror of `axis.enabled`, and the deprecated alias still
    /// drives the same flag.
    #[gpui::test]
    fn a_view_axis_request_reaches_the_state(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        let request: ViewRequest = serde_json::from_str(
            r##"{"axis": {"enabled": false, "color_x": "#010203", "length": 40.0, "width": 3.0}}"##,
        )
        .expect("a view.axis request");
        let summary = cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.apply_view(&request, cx))
        });
        assert_eq!(summary.unwrap(), "axis off");

        let state = model.lock().unwrap().view.clone();
        assert!(!state.axis.enabled);
        assert!(!state.axis_cross, "the bool mirrors axis.enabled");
        assert_eq!(
            state.axis.colors[0],
            ferrocad_stage::grid::parse_srgb_hex("#010203").unwrap()
        );
        assert_eq!(state.axis.length, 40.0);
        assert_eq!(state.axis.width, 3.0);

        // The legacy alias still toggles the same flag (and the same mirror).
        let alias: ViewRequest = serde_json::from_str(r#"{"axis_cross": true}"#).unwrap();
        let summary = cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.apply_view(&alias, cx))
        });
        assert_eq!(summary.unwrap(), "axis cross on");
        let state = model.lock().unwrap().view.clone();
        assert!(state.axis.enabled);
        assert!(state.axis_cross);
    }

    /// A non-native provider claim is reported when it changes *and* its provider
    /// supplies no skin; the native backend is always hosted by the build.
    #[test]
    fn a_gizmo_backend_claim_is_reported_only_when_it_changes() {
        assert_eq!(
            gizmo_backend_note(ferrocad_stage::NATIVE_BACKEND, "ferrocad", false),
            None,
            "the unchanged native backend says nothing"
        );
        assert_eq!(gizmo_backend_note("shader", "shader", false), None);
        assert_eq!(
            gizmo_backend_note(
                ferrocad_stage::NATIVE_BACKEND,
                ferrocad_stage::SKIN_BACKEND,
                true
            ),
            None,
            "a supplied skin hosts the switch, so it is not a warning"
        );
        assert_eq!(
            gizmo_backend_note(
                ferrocad_stage::SKIN_BACKEND,
                ferrocad_stage::NATIVE_BACKEND,
                false
            ),
            None,
            "coming back to the native backend needs no warning"
        );

        let note = gizmo_backend_note(ferrocad_stage::NATIVE_BACKEND, "shader", false)
            .expect("a foreign backend with no skin is reported");
        assert!(note.contains("shader"), "{note}");
        assert!(note.contains("supplied no skin"), "{note}");
    }

    /// The skin channel's default: a plain boot (no mods) has no provider supplying a
    /// gizmo skin, so the shell draws the native material.
    #[test]
    fn a_plain_boot_supplies_no_gizmo_skin() {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _model = Shell::boot().expect("boot shell");
        assert_eq!(
            crate::python::provider_skin("gizmo").expect("the provider skin bridge"),
            None
        );
    }

    /// G1: a `view.gizmo` request restyles the workspace gizmo and reaches the
    /// mirrored view state; a non-native backend with no supplied skin is stored but
    /// reported as unhosted, because the native handles stay the event owner.
    #[gpui::test]
    fn a_view_gizmo_request_reaches_the_state_and_reports_the_backend(
        cx: &mut TestAppContext,
    ) {
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

        let request: ViewRequest = serde_json::from_str(
            r##"{"gizmo": {"enabled": true, "backend": "ferrocad", "active_color": "#38bdf8", "inactive_color": "#94a3b8", "size": 96.0, "width": 2.0, "opacity": 0.8, "ghost_alpha": 0.5, "ring_snap_deg": 5.0, "show_readout": false}}"##,
        )
        .expect("a view.gizmo request");
        let summary = cx
            .update(|_window, cx| {
                shell.update(cx, |shell, cx| shell.apply_view(&request, cx))
            })
            .expect("the request applies");
        assert!(summary.contains("gizmo style"), "{summary}");

        {
            let state = model.lock().unwrap().view.clone();
            let gizmo = &state.gizmo;
            assert!(gizmo.enabled);
            assert_eq!(gizmo.backend, ferrocad_stage::NATIVE_BACKEND);
            // The requested hexes are the documented defaults, so the 8-bit comparison
            // keeps the authored tokens rather than re-parsing them.
            let defaults = ferrocad_stage::GizmoStyle::default();
            assert_eq!(gizmo.active, defaults.active);
            assert_eq!(gizmo.inactive, defaults.inactive);
            assert_eq!(gizmo.inactive[3], 0.9, "the token's alpha is kept");
            assert_eq!(gizmo.size, 96.0);
            assert_eq!(gizmo.width, 2.0);
            assert_eq!(gizmo.opacity, 0.8);
            assert_eq!(gizmo.ghost_alpha, 0.5);
            assert_eq!(gizmo.ring_snap_deg, 5.0);
            assert!(!gizmo.show_readout);
        }

        // The claim is stored, but no provider declares `shader`, so it supplies no
        // skin: the shell says so, and the native manipulators remain the event owner.
        let claim: ViewRequest =
            serde_json::from_str(r#"{"gizmo": {"backend": "shader"}}"#).unwrap();
        let summary = cx
            .update(|_window, cx| {
                shell.update(cx, |shell, cx| shell.apply_view(&claim, cx))
            })
            .expect("the claim applies");
        assert!(summary.contains("gizmo backend shader"), "{summary}");
        assert!(summary.contains("supplied no skin"), "{summary}");
        assert_eq!(model.lock().unwrap().view.gizmo.backend, "shader");
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
        let registered = cx.update(|_cx| crate::python::evaluate(script));
        assert!(registered.unwrap().ok);
        cx.update(|cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_Tool", cx)));
        assert_eq!(model.lock().unwrap().status, "tool: test_tool");
        let active = cx.update(|cx| shell.read(cx).active_tool.clone());
        assert_eq!(active.as_deref(), Some("test_tool"));

        cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_MissingTool", cx)));
        assert_eq!(model.lock().unwrap().status, "no such tool no_such_tool");
    }

    /// A live tool session publishes a status the strip can draw, and the Cancel
    /// handler ends it without a document edit. Headless: no viewport (and so no GPU)
    /// is created.
    #[gpui::test]
    fn cancelling_a_tool_clears_the_strip_and_commits_nothing(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));
        cx.update(|cx| {
            shell.update(cx, |shell, _| {
                shell.set_contributions(Contributions::new().with_tool("test_polyline", || {
                    Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
                }));
            })
        });

        let status = cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.start_tool("test_polyline", cx))
        });
        assert_eq!(status, "tool: test_polyline");
        let status_line = model.lock().unwrap().status.clone();
        {
            let m = model.lock().unwrap();
            let tool = m.tool.as_ref().expect("the strip's status");
            assert!(tool.is_active());
            assert_eq!(tool.label, "Polyline");
            assert_eq!(tool.points, 0);
            assert!(tool.hint.contains("Esc to cancel"), "{}", tool.hint);
        }

        // The Cancel button's handler: the session ends and the strip's status is gone.
        assert!(cx.update(|cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))));
        assert!(model.lock().unwrap().tool.is_none(), "the strip is hidden");
        assert!(cx.update(|cx| shell.read(cx).active_tool.is_none()));
        assert!(
            !cx.update(|cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))),
            "a second cancel is a no-op"
        );
        // No InteractionCommit was produced: the shell's status line/console are intact.
        assert_eq!(model.lock().unwrap().status, status_line);
    }

    /// The strip builds and paints headlessly, and disappears after Cancel.
    #[gpui::test]
    fn the_tool_strip_renders_without_a_display(cx: &mut TestAppContext) {
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

        cx.update(|_window, cx| {
            shell.update(cx, |shell, _| {
                shell.set_contributions(Contributions::new().with_tool("test_polyline", || {
                    Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
                }));
            })
        });
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.start_tool("test_polyline", cx);
            })
        });
        assert!(model.lock().unwrap().tool.is_some());

        // Paint the strip (the fake window has no display or GPU).
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Cancel through the same path as the button, then paint again without it.
        assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))));
        assert!(model.lock().unwrap().tool.is_none());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Open the Dev Tools window from `shell` and return a visual context for it.
    ///
    /// Opening is deferred by one effect cycle (the first frame reads the shell), so
    /// the window exists by the time `update` returns.
    fn open_dev_tools(cx: &mut VisualTestContext, shell: &Entity<Shell>) -> VisualTestContext {
        let status = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.toggle_dev_tools(cx))
        });
        assert_eq!(status, "dev tools: opening");
        let handle = cx
            .windows()
            .into_iter()
            .find(|handle| handle.downcast::<DevTools>().is_some())
            .expect("the Dev Tools window opened");
        let mut dev_cx = VisualTestContext::from_window(handle, cx);
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        dev_cx
    }

    /// P0.5/D3: the main window keeps the frame, the viewport and the status line.
    /// The panels are in the second window, not merely hidden.
    #[gpui::test]
    fn the_main_window_carries_only_the_viewport_and_status(cx: &mut TestAppContext) {
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
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        assert!(cx.debug_bounds("main-shell").is_some());
        assert!(cx.debug_bounds("main-viewport").is_some());
        assert!(
            cx.debug_bounds("dev-command-list").is_none(),
            "the command bar left the main window"
        );
        assert!(cx.debug_bounds("tree-panel").is_none());
        assert!(cx.debug_bounds("properties-panel").is_none());
        assert!(cx.debug_bounds("console-panel").is_none());
    }

    /// P0.5/D2/D5: the Dev Tools window renders the four panels over the same shell and
    /// lists every command with its live enablement (disabled rows stay visible).
    #[gpui::test]
    fn the_dev_tools_window_lists_commands_with_enablement(cx: &mut TestAppContext) {
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

        // The built-ins are present and disabled until a selection.
        let commands = model.lock().unwrap().commands.clone();
        assert!(!commands.is_empty());
        let delete = commands
            .iter()
            .find(|command| command.id == "App_Delete")
            .expect("App_Delete is registered");
        assert!(!delete.enabled, "nothing is selected yet");
        assert!(commands.iter().all(|command| !command.id.is_empty()));

        let mut dev_cx = open_dev_tools(&mut cx, &shell);
        assert!(dev_cx.debug_bounds("dev-command-list").is_some());
        assert!(dev_cx.debug_bounds("tree-panel").is_some());
        assert!(dev_cx.debug_bounds("properties-panel").is_some());
        assert!(dev_cx.debug_bounds("console-panel").is_some());
        assert!(
            dev_cx.debug_bounds("dev-command-App_Delete").is_some(),
            "a disabled command is still listed"
        );

        // Selecting in the inspector feeds the property editor from the same entity.
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let has_field = cx.update(|_window, cx| {
            shell.read(cx).prop_inputs.contains_key("Params.Length")
        });
        assert!(has_field, "the Dev Tools property editor built the field");
        let delete_enabled = model
            .lock()
            .unwrap()
            .commands
            .iter()
            .find(|command| command.id == "App_Delete")
            .map(|command| command.enabled)
            .unwrap_or(false);
        assert!(delete_enabled, "the list follows IsActive");

        // A modal opens in the main window (the shell owns it); Esc from the Dev
        // Tools window must still reach it instead of being swallowed.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_Move", cx))
        });
        assert!(
            cx.update(|_window, cx| shell.read(cx).dialog.is_some()),
            "the move dialog opened"
        );
        dev_cx.simulate_keystrokes("escape");
        assert!(
            cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
            "Esc in Dev Tools cancelled the modal"
        );

        // A command run from the shell updates the same model the window reads.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ViewWorkplane", cx))
        });
        assert_eq!(model.lock().unwrap().status, "view: no viewport");
    }

    /// Reset the interpreter's process-global grid store and preferences.
    ///
    /// Tests share one CPython instance. `FerroCAD.Grid` lives in the parameter
    /// store, not in the fresh document, so a test that toggles it or writes a
    /// preference leaks into the next one. Call this (with the lock held) when a
    /// test's assertions depend on the default grid.
    fn reset_grid_state() {
        // S4 moved the grid store to the native provider's group; use the public
        // facade so this reset follows the one store instead of a legacy key.
        // The axis cross's store is process-global too (and its `enabled` migrates
        // from the legacy grid/view flags), so clear it and those flags alongside.
        let script = r#"
import FreeCAD
import FerroCAD
FerroCAD.Grid.reset()
FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/axis/ferrocad").Clear()
legacy = (
    FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/grid/ferrocad"),
    FreeCAD.ParamGet("User parameter:BaseApp/Preferences/View"),
)
for group in legacy:
    if "GridAxisCross" in group.GetBools():
        group.RemBool("GridAxisCross")
"#;
        let reset = crate::python::evaluate(script).expect("reset the grid state");
        assert!(reset.ok, "{}", reset.output);
    }


    /// P0.5/D4/D5: closing and reopening preserves the model, the selection and the
    /// console; the second window never owns a copy.
    #[gpui::test]
    fn toggling_dev_tools_preserves_the_model(cx: &mut TestAppContext) {
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

        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
        let (console_len, selection) = {
            let m = model.lock().unwrap();
            (m.console.len(), m.selection.clone())
        };

        let _dev_cx = open_dev_tools(&mut cx, &shell);
        assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

        let status = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.toggle_dev_tools(cx))
        });
        assert_eq!(status, "dev tools: closed");
        assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
        assert!(
            cx.windows().len() == 1,
            "only the main window remains: {:?}",
            cx.windows()
        );

        // Nothing in the document, the selection or the transcript changed.
        {
            let m = model.lock().unwrap();
            assert_eq!(m.console.len(), console_len);
            assert_eq!(m.selection, selection);
            // The tree also carries the synthetic `Settings` root (S1); this
            // assertion is about documents, so exclude it by name.
            let documents: Vec<&str> = m
                .tree
                .iter()
                .map(|doc| doc.name.as_str())
                .filter(|name| *name != "Settings")
                .collect();
            assert_eq!(
                documents.len(),
                1,
                "the sample document is intact; open documents: {documents:?}"
            );
        }

        // Reopening builds a new view around the same shell entity and model.
        let _dev_cx = open_dev_tools(&mut cx, &shell);
        assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
        let shared = cx.update(|_window, cx| {
            shell.read(cx).model.lock().unwrap().selection.clone()
        });
        assert_eq!(shared, selection);
    }

    /// The Dev Tools providers panel switches a backend without opening the Mods
    /// dialog; the header popover and this panel share `activate_provider`.
    #[gpui::test]
    fn the_dev_tools_providers_panel_switches_a_backend(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();
        let mut dev_cx = open_dev_tools(&mut cx, &shell);

        assert!(
            dev_cx.debug_bounds("dev-providers").is_some(),
            "the providers panel is in the Dev Tools window"
        );
        assert!(
            dev_cx.debug_bounds("dev-provider-grid-coin").is_some(),
            "the grid backends are offered"
        );
        assert!(dev_cx.debug_bounds("dev-provider-axis-coin").is_some());

        click_at(&mut dev_cx, "dev-provider-grid-coin");
        let summary = cx
            .update(|_window, cx| shell.read(cx).grid_summary.clone())
            .expect("the switch refreshed the summary");
        assert_eq!(summary.backend.as_deref(), Some("coin"));
        assert!(
            summary
                .providers
                .iter()
                .any(|provider| provider.backend == "coin" && provider.active),
            "the Dev Tools switch follows the registry"
        );
        // The losing backend's command stays listed but is not runnable; the switch is
        // the only way to reach it, so the command list must follow the active backend.
        let inactive = cx.update(|_window, cx| {
            crate::chrome::inactive_provider_commands(shell.read(cx).provider_features())
        });
        assert!(
            inactive.contains_key("GridOverlay_Settings"),
            "the native grid command is inactive once coin owns the feature"
        );
        assert!(!inactive.contains_key("GridOverlayCoin_Settings"));

        // Leave the shared interpreter on the default for the other tests.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("ferrocad-provider:grid:ferrocad", cx)
            })
        });
        let inactive = cx.update(|_window, cx| {
            crate::chrome::inactive_provider_commands(shell.read(cx).provider_features())
        });
        assert!(inactive.contains_key("GridOverlayCoin_Settings"));
        assert!(!inactive.contains_key("GridOverlay_Settings"));
    }

    /// Write the GPU preference through the live facade, bypassing the bridge's
    /// validation (so a test can store a deliberately malformed value).
    fn write_gpu_preference(value: &str) {
        let script = format!(
            "import FreeCAD\nFreeCAD.ParamGet('User parameter:BaseApp/Preferences/View')\
             .SetString('GpuAdapter', {value:?})"
        );
        let result = crate::python::evaluate(&script).expect("write the GPU preference");
        assert!(result.ok, "{}", result.output);
    }

    /// The Dev Tools GPU panel lists `Auto` plus every adapter and marks the
    /// persisted choice, using synthetic adapters so the test needs no GPU.
    #[gpui::test]
    fn the_dev_tools_gpu_panel_marks_the_active_row(cx: &mut TestAppContext) {
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
        let mut dev_cx = open_dev_tools(&mut cx, &shell);

        cx.update(|_window, cx| {
            shell.update(cx, |shell, _| {
                shell.gpu_preference = Some("adapter:Llvm".to_string());
                shell.gpu_adapters = Some(vec![
                    AdapterSummary {
                        name: "Llvm".to_string(),
                        backend: "vulkan".to_string(),
                        device_type: "cpu".to_string(),
                        driver: "llvmpipe".to_string(),
                    },
                    AdapterSummary {
                        name: "Discrete".to_string(),
                        backend: "vulkan".to_string(),
                        device_type: "discrete".to_string(),
                        driver: "mesa".to_string(),
                    },
                ]);
            })
        });
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        assert!(
            dev_cx.debug_bounds("gpu-panel").is_some(),
            "the GPU panel is between Providers and Commands"
        );
        assert!(dev_cx.debug_bounds("dev-gpu-auto").is_some());
        assert!(dev_cx.debug_bounds("dev-gpu-adapter-0").is_some());
        assert!(dev_cx.debug_bounds("dev-gpu-adapter-1").is_some());
        assert!(
            dev_cx.debug_bounds("dev-gpu-adapter-0-active").is_some(),
            "the persisted adapter is marked active"
        );
        assert!(dev_cx.debug_bounds("dev-gpu-adapter-1-active").is_none());
        assert!(dev_cx.debug_bounds("dev-gpu-auto-active").is_none());
        assert!(
            dev_cx.debug_bounds("gpu-in-use").is_some(),
            "the panel reports that there is no viewport"
        );
    }

    /// A malformed persisted value falls back to `Auto` and says why in the console.
    #[gpui::test]
    fn an_unknown_gpu_setting_falls_back_to_auto(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        write_gpu_preference("not-a-real-form");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        let (choice, console) = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                let choice = shell.stored_gpu_choice(cx);
                let console = shell.model.lock().unwrap().console.clone();
                (choice, console)
            })
        });
        assert_eq!(choice, AdapterChoice::Auto);
        assert!(
            console
                .iter()
                .any(|line| line.contains("viewport GPU") && line.contains("using auto")),
            "console: {console:?}"
        );

        write_gpu_preference("auto");
    }

    /// Selecting a row persists its value and recreates the viewport. Skipped where
    /// the machine offers no adapter, because the recreate path needs one.
    #[gpui::test]
    fn a_gpu_panel_row_persists_the_choice(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let adapters = ferrocad_wgpu::adapter::adapters();
        let Some(adapter) = adapters.first() else {
            return;
        };
        let setting = format!("adapter:{}", adapter.name);

        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command(&format!("ferrocad-gpu:{setting}"), cx)
            })
        });
        assert_eq!(
            crate::python::gpu_adapter().expect("the stored setting"),
            setting
        );
        assert_eq!(
            model.lock().unwrap().status,
            format!("viewport GPU: {setting} (zero-copy keeps the window adapter)")
        );

        write_gpu_preference("auto");
    }

    /// P0.5/D4: `Dev_ToggleTools` is registered and its headless path reports a status
    /// instead of pretending a window exists.
    #[gpui::test]
    fn the_dev_tools_command_has_a_headless_status(_cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        drop(model);

        let outcome = crate::commands::run(crate::dev_tools::TOGGLE_COMMAND).expect("the bridge");
        assert!(outcome.ok, "{}", outcome.error);
        assert!(
            outcome.status.contains("dev tools"),
            "status: {}",
            outcome.status
        );
        let commands = crate::python::commands().expect("the command list");
        assert!(
            commands
                .iter()
                .any(|command| command.id == crate::dev_tools::TOGGLE_COMMAND),
            "the toggle is in the registry"
        );
    }

    /// Paint the window once so a following `simulate_*` has hitboxes to dispatch to.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Click the centre of the element with `selector` (it must have been drawn).
    fn click_at(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} has no bounds"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    }

    /// T0/T6: every command the registry currently reports is on exactly one surface,
    /// and the shipped metadata matches the documented mapping.
    #[gpui::test]
    fn every_registered_command_lands_on_exactly_one_surface(_cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _model = Shell::boot().expect("boot shell");

        // A mod that predates the metadata must still appear, on the dock.
        let script = r#"
import FreeCADGui
class _Unannotated:
    def Activated(self):
        return {"status": "unannotated ran"}
if "Test_Unannotated" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Unannotated", _Unannotated())
"#;
        let registered = crate::python::evaluate(script).expect("evaluate");
        assert!(registered.ok, "{}", registered.output);

        let commands = crate::python::commands().expect("the command list");
        let groups = crate::commands::group_by_surface(&commands);
        assert_eq!(
            groups.all().len(),
            commands.len(),
            "every command lands on one surface"
        );
        let mut ids: Vec<&str> = groups.all().iter().map(|command| command.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), commands.len(), "no command is on two surfaces");

        let find = |id: &str| {
            commands
                .iter()
                .find(|command| command.id == id)
                .unwrap_or_else(|| panic!("{id} is registered"))
        };
        use crate::commands::Surface;
        assert_eq!(find("App_ToggleGrid").surface(), Surface::Grid);
        assert_eq!(find("App_ToggleAxisCross").surface(), Surface::Axis);
        assert_eq!(find("App_ViewWorkplane").surface(), Surface::Workplane);
        assert_eq!(find("Mods_Settings").surface(), Surface::Mods);
        assert_eq!(find("App_Delete").surface(), Surface::Dock);
        assert_eq!(
            find("Dev_ToggleTools").surface(),
            Surface::Hidden,
            "the Dev Tools toggle is chrome, not a workbench action"
        );
        assert_eq!(
            find("View_GridSettings").parent(),
            Some("App_ToggleGrid")
        );
        assert_eq!(
            find("App_ReloadGridSettings").parent(),
            Some("App_ToggleGrid")
        );
        assert_eq!(
            find("App_ActivateWorkplane").parent(),
            Some("App_ViewWorkplane")
        );
        assert_eq!(find("Test_Unannotated").surface(), Surface::Dock);
    }

    /// T1/T2: the header split button's label runs the primary command; the caret
    /// opens the popover without running it; `Esc` and an outside click dismiss it.
    #[gpui::test]
    fn the_grid_split_button_runs_the_label_and_the_caret_opens(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();
        draw(&mut cx);

        // The controls exist in the floating pill at the canvas' top-left, ordered
        // left to right. This is a compact cluster, not a full-width header row.
        let width = cx.update(|window, _| window.viewport_size().width);
        let pane = cx.debug_bounds("main-viewport").expect("the viewport pane");
        let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
        let header = cx.debug_bounds("viewport-header").expect("the control pill");
        let grid = cx.debug_bounds("grid-label").expect("the Grid label");
        let axis = cx.debug_bounds("axis-label").expect("the Axes label");
        let wp = cx.debug_bounds("wp-label").expect("the WP label");
        assert!(grid.origin.x < axis.origin.x);
        assert!(axis.origin.x < wp.origin.x);
        assert!(
            cx.debug_bounds("mods-label").is_none(),
            "the Mods control left the header"
        );
        assert!(
            cx.debug_bounds("mods-caret").is_none(),
            "the Mods caret left the header"
        );
        assert!(
            header.origin.x < width * 0.5,
            "the pill sits at the leading edge (at {:?}, width {width:?})",
            header.origin.x
        );
        assert!(
            header.size.width < canvas.size.width,
            "the pill is compact, not a full-width header row (pill {:?}, canvas {:?})",
            header.size.width,
            canvas.size.width
        );
        assert!(
            header.origin.x >= canvas.origin.x
                && header.origin.x < canvas.origin.x + px(16.)
                && header.origin.y >= canvas.origin.y
                && header.origin.y < canvas.origin.y + px(16.),
            "the pill floats over the canvas' top-left (pill {:?}, canvas {:?}, pane {:?})",
            header.origin,
            canvas.origin,
            pane.origin
        );
        for control in [&grid, &axis, &wp] {
            assert!(
                header.contains(&control.center()),
                "the pill owns every control"
            );
        }
        assert!(
            cx.debug_bounds("grid-state-dot").is_some(),
            "the Grid control carries a persistent on/off dot"
        );

        // The caret opens the grid popover and does not toggle the grid.
        assert!(!model.lock().unwrap().view.grid);
        click_at(&mut cx, "grid-caret");
        assert!(
            cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "the caret opened the popover"
        );
        assert!(!model.lock().unwrap().view.grid, "opening ran no command");
        draw(&mut cx);
        let popover = cx.debug_bounds("grid-popover").expect("the grid popover");
        let caret = cx.debug_bounds("grid-caret").expect("the caret");
        assert!(
            popover.origin.y >= caret.origin.y + caret.size.height,
            "the popover drops below the control (popover {:?}, caret {:?})",
            popover.origin,
            caret.origin
        );
        assert!(
            popover.origin.x + popover.size.width <= width + px(8.),
            "the popover stays inside the window (right edge {:?})",
            popover.origin.x + popover.size.width
        );
        assert!(
            cx.debug_bounds("grid-row-ferrocad-provider:grid:ferrocad").is_some(),
            "the provider switch rows lead the popover"
        );
        assert!(cx.debug_bounds("grid-row-ferrocad-provider:grid:coin").is_some());
        assert!(
            cx.debug_bounds("grid-row-View_GridSettings").is_some(),
            "the popover lists the grid surface's children"
        );
        assert!(cx.debug_bounds("grid-row-App_ReloadGridSettings").is_some());
        assert!(
            cx.debug_bounds("grid-row-GridOverlay_Settings").is_none(),
            "a dev-only backend settings command is filtered from the popover"
        );
        let providers_row = cx
            .debug_bounds("grid-row-ferrocad-provider:grid:ferrocad")
            .expect("the provider row");
        let settings_row = cx
            .debug_bounds("grid-row-View_GridSettings")
            .expect("the settings row");
        assert!(
            providers_row.origin.y < settings_row.origin.y,
            "provider rows come before the generic settings rows"
        );
        assert!(
            cx.debug_bounds("hairline-2").is_some(),
            "the provider group and the config group have a hairline"
        );

        // Esc dismisses it and leaves the grid alone.
        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("escape");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
        assert!(!model.lock().unwrap().view.grid);

        // A right-click on the label opens the popover and never runs the command.
        let label = cx.debug_bounds("grid-label").expect("the Grid label");
        cx.simulate_mouse_down(
            label.center(),
            gpui::MouseButton::Right,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            label.center(),
            gpui::MouseButton::Right,
            gpui::Modifiers::default(),
        );
        assert!(
            cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "a right-click opened the popover"
        );
        assert!(!model.lock().unwrap().view.grid, "and ran nothing");
        cx.simulate_keystrokes("escape");

        // An outside click dismisses it too (the viewport keeps the event: the
        // popover has no backdrop).
        draw(&mut cx);
        click_at(&mut cx, "grid-caret");
        assert!(cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
        draw(&mut cx);
        cx.simulate_click(gpui::point(px(200.), px(300.)), gpui::Modifiers::default());
        assert!(
            !cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "an outside click dismissed the popover"
        );

        // One popover at a time: opening the workplane menu replaces the grid one.
        draw(&mut cx);
        click_at(&mut cx, "grid-caret");
        draw(&mut cx);
        click_at(&mut cx, "wp-caret");
        assert!(cx.update(|_window, cx| shell.read(cx).popover_open("wp")));
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
        draw(&mut cx);
        assert!(cx.debug_bounds("wp-popover").is_some());
        assert!(cx.debug_bounds("grid-popover").is_none());
        assert!(cx.debug_bounds("wp-row-App_ActivateWorkplane").is_some());
        assert!(cx.debug_bounds("wp-row-App_ClearWorkplane").is_some());

        // The label runs the primary command in one click and closes the menu.
        click_at(&mut cx, "grid-label");
        assert!(model.lock().unwrap().view.grid, "the label toggled the grid");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
        click_at(&mut cx, "grid-label");
        assert!(!model.lock().unwrap().view.grid, "and back off");

        // The Axes control is a split button; its label runs the checkable command.
        assert!(model.lock().unwrap().view.axis_cross);
        click_at(&mut cx, "axis-label");
        assert!(!model.lock().unwrap().view.axis_cross);
    }

    /// T1/T4: the `Axes` label runs `App_ToggleAxisCross`; the caret opens the axis
    /// popover without toggling. The popover lists both backend settings rows and the
    /// reload command, and the control keeps a persistent state dot.
    #[gpui::test]
    fn the_axis_split_button_runs_the_label_and_the_caret_opens(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // The Python facade owns `App_ReloadAxisSettings` (axis surface, parent
        // `App_ToggleAxisCross`). Until that registration lands, install the same
        // metadata so this test still exercises the popover wiring; when the real
        // command is present this branch is skipped and the real one is asserted.
        let reload_registered = crate::python::commands()
            .expect("the command list")
            .iter()
            .any(|command| command.id == "App_ReloadAxisSettings");
        if !reload_registered {
            let script = r#"
import FreeCADGui
class _TestReloadAxisSettings:
    def GetResources(self):
        return {
            "MenuText": "Reload axis settings",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "axis settings reloaded"}
if "App_ReloadAxisSettings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("App_ReloadAxisSettings", _TestReloadAxisSettings())
"#;
            let registered = crate::python::evaluate(script).expect("register the stand-in");
            assert!(registered.ok, "{}", registered.output);
            cx.update(|_window, cx| {
                shell.update(cx, |shell, cx| shell.refresh_commands(cx));
            });
        }
        draw(&mut cx);

        // The split button carries its own persistent on/off dot.
        assert!(
            cx.debug_bounds("axis-state-dot").is_some(),
            "the Axes control carries a state dot"
        );
        assert!(cx.debug_bounds("axis-label").is_some());
        assert!(cx.debug_bounds("axis-caret").is_some());
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));

        // The caret opens the popover and does not toggle the cross.
        assert!(model.lock().unwrap().view.axis_cross, "on by default");
        click_at(&mut cx, "axis-caret");
        assert!(
            cx.update(|_window, cx| shell.read(cx).popover_open("axis")),
            "the caret opened the axis popover"
        );
        assert!(
            model.lock().unwrap().view.axis_cross,
            "opening ran no command"
        );
        draw(&mut cx);
        assert!(cx.debug_bounds("axis-popover").is_some());
        assert!(
            cx.debug_bounds("axis-row-ferrocad-provider:axis:ferrocad").is_some(),
            "the native provider switch row leads the popover"
        );
        assert!(
            cx.debug_bounds("axis-row-ferrocad-provider:axis:coin").is_some(),
            "the Coin provider switch row is offered"
        );
        assert!(
            cx.debug_bounds("axis-row-AxisOverlay_Settings").is_none(),
            "the native backend's settings command is dev-only"
        );
        assert!(
            cx.debug_bounds("axis-row-AxisOverlayCoin_Settings").is_none(),
            "the Coin backend's settings command is dev-only"
        );
        assert!(
            cx.debug_bounds("axis-row-App_ReloadAxisSettings").is_some(),
            "the axis surface's reload command"
        );
        assert!(
            cx.debug_bounds("axis-row-View_AxisSettings").is_some(),
            "the axis surface's settings command"
        );
        let providers_row = cx
            .debug_bounds("axis-row-ferrocad-provider:axis:ferrocad")
            .expect("the provider row");
        let reload_row = cx
            .debug_bounds("axis-row-App_ReloadAxisSettings")
            .expect("the reload row");
        let settings_row = cx
            .debug_bounds("axis-row-View_AxisSettings")
            .expect("the settings row");
        assert!(
            providers_row.origin.y < settings_row.origin.y
                && providers_row.origin.y < reload_row.origin.y,
            "provider rows come before the generic settings rows"
        );

        // Esc dismisses it and leaves the cross alone.
        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("escape");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));
        assert!(model.lock().unwrap().view.axis_cross);

        // The label runs the primary command and closes the menu.
        click_at(&mut cx, "axis-label");
        assert!(
            !model.lock().unwrap().view.axis_cross,
            "the label toggled the cross"
        );
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));
        click_at(&mut cx, "axis-label");
        assert!(model.lock().unwrap().view.axis_cross, "and back on");

        // Leave the shared interpreter on the default cross for later tests.
        if !reload_registered {
            let cleanup = crate::python::evaluate(
                "import FreeCADGui\nFreeCADGui.removeCommand('App_ReloadAxisSettings')",
            )
            .expect("remove the stand-in");
            assert!(cleanup.ok, "{}", cleanup.output);
        }
        reset_grid_state();
    }

    /// The canvas owns the full pane: there is no full-width header row above it, and
    /// the environment controls float as a compact pill over its top-left corner.
    #[gpui::test]
    fn the_canvas_starts_at_the_pane_top_and_the_controls_float_over_it(
        cx: &mut TestAppContext,
    ) {
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
        draw(&mut cx);

        let pane = cx.debug_bounds("main-viewport").expect("the viewport pane");
        let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
        let pill = cx.debug_bounds("viewport-header").expect("the control pill");

        // The canvas is the pane's first row: no header pushes the scene down.
        assert_eq!(
            canvas.origin.y, pane.origin.y,
            "the canvas starts at the pane top (canvas {canvas:?}, pane {pane:?})"
        );
        assert_eq!(
            canvas.size.height, pane.size.height,
            "the canvas fills the pane (canvas {canvas:?}, pane {pane:?})"
        );

        // The pill overlays the canvas' top-left corner, inside it and compact.
        assert!(
            pill.origin.x >= canvas.origin.x
                && pill.origin.x < canvas.origin.x + px(16.)
                && pill.origin.y >= canvas.origin.y
                && pill.origin.y < canvas.origin.y + px(16.),
            "the pill floats at the canvas' top-left (pill {pill:?}, canvas {canvas:?})"
        );
        assert!(
            pill.origin.x + pill.size.width <= canvas.origin.x + canvas.size.width
                && pill.origin.y + pill.size.height <= canvas.origin.y + canvas.size.height,
            "the pill stays inside the canvas (pill {pill:?}, canvas {canvas:?})"
        );
        assert!(
            pill.size.width < canvas.size.width,
            "the pill is not a full-width header row (pill {pill:?}, canvas {canvas:?})"
        );
    }

    /// T4: the Grid popover reports the active provider and spacing above its
    /// command rows, and follows a provider switch.
    #[gpui::test]
    fn the_grid_popover_reports_the_active_provider_and_spacing(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();
        draw(&mut cx);

        click_at(&mut cx, "grid-caret");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("grid-row-provider").is_some(),
            "the popover reports the provider above its commands"
        );
        assert!(
            cx.debug_bounds("grid-row-spacing").is_some(),
            "and the active provider's spacing"
        );
        assert!(
            cx.debug_bounds("grid-row-no-provider").is_none(),
            "there is a provider"
        );
        {
            let summary = cx
                .update(|_window, cx| shell.read(cx).grid_summary.clone())
                .expect("opening the popover fetched the summary");
            assert_eq!(summary.feature, "grid");
            assert_eq!(summary.backend.as_deref(), Some("ferrocad"));
            assert_eq!(summary.label.as_deref(), Some("FerroCAD (native)"));
            assert_eq!(summary.spacing.as_deref(), Some("10 mm"));
            assert!(
                summary
                    .providers
                    .iter()
                    .any(|provider| provider.backend == "coin" && !provider.active),
                "the other backend is listed but not active"
            );
        }
        // The active backend's mode row is checked; the inactive one is not.
        {
            let model = model.lock().unwrap();
            let checked = |id: &str| {
                model
                    .commands
                    .iter()
                    .find(|command| command.id == id)
                    .map(|command| command.checked)
                    .unwrap_or(false)
            };
            assert!(
                checked("GridOverlay_Settings"),
                "the active native backend reads as checked"
            );
            assert!(
                !checked("GridOverlayCoin_Settings"),
                "the inactive Coin backend is unchecked"
            );
        }

        // Switching the provider through the Settings node's selector refreshes
        // the cached summary, and an open popover follows it.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.commit_property("Settings/Grid.provider", "Coin (WGSL)", cx);
            });
        });
        draw(&mut cx);
        {
            let summary = cx
                .update(|_window, cx| shell.read(cx).grid_summary.clone())
                .expect("the settings edit refreshed the summary");
            assert_eq!(summary.backend.as_deref(), Some("coin"));
            assert_eq!(summary.label.as_deref(), Some("Coin (WGSL)"));
        }
        assert!(
            cx.debug_bounds("grid-row-provider").is_some(),
            "the provider row is still there after the switch"
        );

        // Leave the shared interpreter on the default provider for later tests.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.commit_property("Settings/Grid.provider", "FerroCAD (native)", cx);
            });
        });
        reset_grid_state();
    }

    /// T4: a provider row in the Grid/Axes popover switches the active backend
    /// through the synthetic id and applies the returned view; no registry command
    /// is involved.
    #[gpui::test]
    fn provider_rows_switch_the_active_backend(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();
        draw(&mut cx);

        // Grid: the Coin row switches the backend and closes the menu.
        click_at(&mut cx, "grid-caret");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("grid-row-ferrocad-provider:grid:coin")
                .is_some(),
            "the coin backend is offered"
        );
        click_at(&mut cx, "grid-row-ferrocad-provider:grid:coin");
        assert!(
            !cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "choosing a provider closes the popover"
        );
        {
            let summary = cx
                .update(|_window, cx| shell.read(cx).grid_summary.clone())
                .expect("the switch refreshed the summary");
            assert_eq!(summary.backend.as_deref(), Some("coin"));
            assert!(
                summary
                    .providers
                    .iter()
                    .any(|provider| provider.backend == "coin" && provider.active),
                "the summary follows the switch"
            );
        }
        {
            let m = model.lock().unwrap();
            assert!(
                m.status.contains("coin is the active provider"),
                "the provider status is reported: {}",
                m.status
            );
            assert!(
                m.view.grid_overlay,
                "the returned view request was applied"
            );
        }

        // Switch back through the same intercept.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("ferrocad-provider:grid:ferrocad", cx)
            })
        });
        {
            let summary = cx
                .update(|_window, cx| shell.read(cx).grid_summary.clone())
                .expect("the switch back refreshed the summary");
            assert_eq!(summary.backend.as_deref(), Some("ferrocad"));
            assert!(!model.lock().unwrap().view.grid_overlay);
        }

        // Axes: the Coin row does the same on the axis feature.
        click_at(&mut cx, "axis-caret");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("axis-row-ferrocad-provider:axis:coin")
                .is_some(),
            "the coin axis backend is offered"
        );
        click_at(&mut cx, "axis-row-ferrocad-provider:axis:coin");
        assert!(
            !cx.update(|_window, cx| shell.read(cx).popover_open("axis")),
            "choosing a provider closes the axis popover"
        );
        {
            let summary = cx
                .update(|_window, cx| shell.read(cx).axis_summary.clone())
                .expect("the switch refreshed the axis summary");
            assert_eq!(summary.backend.as_deref(), Some("coin"));
        }
        assert!(
            model.lock().unwrap().view.axis.overlay,
            "the axis provider's claim was applied"
        );

        // Leave the shared interpreter on the default providers for later tests.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("ferrocad-provider:axis:ferrocad", cx)
            })
        });
        assert!(!model.lock().unwrap().view.axis.overlay);
        reset_grid_state();
    }

    /// T5/S2: `View_GridSettings` asks the shell to select the synthetic
    /// `Settings/Grid` node, and the existing Properties pane renders its rows.
    #[gpui::test]
    fn view_grid_settings_selects_the_settings_node(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        // The command returns the directive; `FreeCADGui._payload` must forward it.
        let forwarded = crate::commands::run("View_GridSettings").expect("run the command");
        assert_eq!(
            forwarded.select.as_deref(),
            Some("Settings/Grid"),
            "the bridge forwards the command's select directive"
        );

        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
        });

        {
            let m = model.lock().unwrap();
            assert_eq!(
                m.selected.as_deref(),
                Some("Settings/Grid"),
                "the directive selected the node"
            );
            assert!(
                m.selection.iter().any(|item| item.is("Settings/Grid")),
                "through the normal selection path"
            );
            let names: Vec<&str> = m.properties.iter().map(|row| row.name.as_str()).collect();
            assert!(names.contains(&"provider"), "rows: {names:?}");
            assert!(names.contains(&"minor"), "rows: {names:?}");
            assert!(
                m.status.contains("Settings/Grid"),
                "the selection status is kept: {}",
                m.status
            );
        }
        assert!(
            cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
            "the select directive is not the modal fallback"
        );

        // The modeless card is the visible settings surface (A3/T5): it renders one
        // field per editable row, without opening the Dev Tools window.
        draw(&mut cx);
        let card = cx.debug_bounds("settings-card").expect("the settings card");
        assert_eq!(card.size.width, px(380.), "the card is 380 px wide");
        assert!(cx.debug_bounds("settings-card-header").is_some());
        assert!(cx.debug_bounds("settings-card-close").is_some());
        let keys = cx.update(|_window, cx| {
            let mut keys: Vec<String> = shell.read(cx).prop_inputs.keys().cloned().collect();
            keys.sort();
            keys
        });
        assert!(
            keys.iter().any(|key| key == "Settings/Grid.provider"),
            "fields: {keys:?}"
        );
        assert!(
            keys.iter().any(|key| key == "Settings/Grid.minor"),
            "fields: {keys:?}"
        );

        // The header drags the card; the shell root follows the pointer.
        let header = cx
            .debug_bounds("settings-card-header")
            .expect("the card header");
        let start = header.center();
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + point(px(40.), px(30.)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            start + point(px(40.), px(30.)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        let moved = cx.update(|_window, cx| {
            shell
                .read(cx)
                .settings_card
                .as_ref()
                .expect("the card is still open")
                .position
        });
        assert_eq!(
            moved,
            point(px(260.), px(126.)),
            "the header drag moved the card"
        );

        // The close button and Esc both dismiss it without losing the selection.
        click_at(&mut cx, "settings-card-close");
        assert!(cx.update(|_window, cx| shell.read(cx).settings_card.is_none()));
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
        });
        draw(&mut cx);
        assert!(cx.debug_bounds("settings-card").is_some());
        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("escape");
        assert!(
            cx.update(|_window, cx| shell.read(cx).settings_card.is_none()),
            "Esc dismisses the modeless card"
        );
        reset_grid_state();
    }

    /// P0.8: a click/focus inside the card must not fall through to the canvas and
    /// clear the selection (which would drop the card).
    #[gpui::test]
    fn clicking_a_settings_field_keeps_the_card_open(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
        });
        draw(&mut cx);

        let field = cx
            .debug_bounds("prop-Settings/Grid.minor")
            .expect("the minor field");
        cx.simulate_mouse_down(
            field.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            field.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(&mut cx);

        {
            let m = model.lock().unwrap();
            assert_eq!(
                m.selected.as_deref(),
                Some("Settings/Grid"),
                "the field click did not clear the selection"
            );
            assert!(
                m.properties.iter().any(|row| row.name == "minor"),
                "the settings rows are still loaded"
            );
        }
        assert!(
            cx.update(|_window, cx| shell.read(cx).settings_card.is_some()),
            "the card stays open while a field is edited"
        );
        reset_grid_state();
    }

    /// S2: editing a `Settings/Grid` row through the property editor commits to
    /// the provider store, and the pane re-reads the row.
    #[gpui::test]
    fn editing_a_settings_row_updates_the_properties(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // The modeless card renders the same property editor; selecting the node
        // opens it and builds one field per editable row.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Settings/Grid", cx);
                cx.notify();
            });
        });
        draw(&mut cx);

        let field = shell.read_with(&cx, |shell, _| {
            shell
                .prop_inputs
                .get("Settings/Grid.minor")
                .expect("the Minor spacing field")
                .state
                .clone()
        });
        cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("25 mm");
        cx.simulate_keystrokes("enter");

        let minor = shell.read_with(&cx, |shell, _| {
            shell
                .model
                .lock()
                .unwrap()
                .properties
                .iter()
                .find(|row| row.name == "minor")
                .map(|row| row.value.clone())
        });
        assert_eq!(minor.as_deref(), Some("25 mm"));
        assert_eq!(model.lock().unwrap().status, "set Settings/Grid.minor");
        // The store, not just the mirrored row, changed.
        let stored = crate::python::properties("Settings/Grid")
            .expect("settings rows")
            .into_iter()
            .find(|row| row.name == "minor")
            .map(|row| row.value);
        assert_eq!(stored.as_deref(), Some("25 mm"));
        reset_grid_state();
    }

    /// T7: the app menu sits in the **title bar** and is the discoverable way back
    /// into Dev Tools — the window starts closed and the command is hidden from every
    /// other surface. The viewport header keeps only the environment controls.
    ///
    /// The headless test platform always reports server-side decorations, so
    /// `window_frame_with_leading` returns the content unchanged and paints no title
    /// bar. This renders the same `title_bar` element the frame composes, with the
    /// shell's real menu, so the popover and its rows can be clicked. The frame's own
    /// client-decoration branch stays visual-only (see
    /// `docs/tasks/toolbar-chrome.md` §10).
    #[gpui::test]
    fn the_app_menu_opens_dev_tools(cx: &mut TestAppContext) {
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
        draw(&mut cx);

        // The menu left the viewport header; Grid/Axes/WP did not. The test
        // platform draws server-side decorations, so the frame's fallback leading bar
        // (below the platform title bar) is where the menu lives here.
        let header = cx.debug_bounds("viewport-header").expect("the viewport header");
        assert!(cx.debug_bounds("grid-label").is_some());
        let menu = cx
            .debug_bounds("app-menu")
            .expect("the app menu in the leading bar");
        assert!(
            !header.contains(&menu.center()),
            "the app menu is no longer in the viewport header"
        );
        assert!(
            cx.debug_bounds("leading-bar").is_some(),
            "server decorations get the leading-bar fallback"
        );

        // A second window whose content is only the title bar (see `TitleBarHarness`).
        let size = cx.update(|window, _| window.viewport_size());
        let shell_for_bar = shell.clone();
        let bar = cx.cx.open_window(size, move |_window, _cx| TitleBarHarness {
            shell: shell_for_bar.clone(),
        });
        let mut bar_cx = VisualTestContext::from_window(bar.into(), &cx);
        draw(&mut bar_cx);

        let title_bar = bar_cx.debug_bounds("titlebar").expect("the title bar");
        let menu = bar_cx.debug_bounds("app-menu").expect("the app menu");
        assert!(
            title_bar.contains(&menu.center()),
            "the app menu is inside the title bar (menu {menu:?}, bar {title_bar:?})"
        );
        assert!(
            menu.origin.x < title_bar.size.width * 0.5,
            "the app menu leads the title bar (at {:?})",
            menu.origin.x
        );
        assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

        // It lists Dev Tools, which no other surface offers.
        click_at(&mut bar_cx, "app-menu");
        assert!(cx.update(|_window, cx| shell.read(cx).popover_open("app")));
        draw(&mut bar_cx);
        let popover = bar_cx.debug_bounds("app-popover").expect("the app popover");
        assert!(
            popover.origin.y > title_bar.center().y,
            "the popover drops below the title bar (at {:?})",
            popover.origin.y
        );
        assert!(bar_cx.debug_bounds("app-row-Dev_ToggleTools").is_some());

        // Esc closes it and opens nothing.
        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("escape");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("app")));
        assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

        // Running the row opens the second window and dismisses the menu.
        draw(&mut bar_cx);
        click_at(&mut bar_cx, "app-menu");
        draw(&mut bar_cx);
        click_at(&mut bar_cx, "app-row-Dev_ToggleTools");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("app")));
        assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
        assert!(
            cx.windows()
                .iter()
                .any(|handle| handle.downcast::<DevTools>().is_some()),
            "the Dev Tools window exists"
        );
    }

    /// A window whose whole content is the production title bar, so the shell's app
    /// menu can be clicked in a headless test (test windows report server-side
    /// decorations, so no window frame draws chrome).
    struct TitleBarHarness {
        shell: Entity<Shell>,
    }

    impl Render for TitleBarHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let leading = self.shell.update(cx, |shell, cx| {
                let model = shell.snapshot();
                crate::chrome::app_menu(
                    shell.popover_open("app"),
                    &model,
                    shell.dev_tools_open(cx),
                    cx,
                )
                .into_any_element()
            });
            ferrocad_widgets::window_chrome::title_bar("FerroCAD", Some(leading))
        }
    }

    /// T3: the action dock sits over the bottom-centre of the canvas, lists the dock
    /// surface, and its rows follow `IsActive` while staying visible when disabled.
    #[gpui::test]
    fn the_action_dock_follows_the_selection(cx: &mut TestAppContext) {
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
        draw(&mut cx);

        let size = cx.update(|window, _| window.viewport_size());
        let dock = cx.debug_bounds("action-dock").expect("the dock");
        assert!(
            dock.origin.y > size.height * 0.6,
            "the dock is at the bottom of the canvas"
        );
        let centre = dock.center().x;
        assert!(
            centre > size.width * 0.4 && centre < size.width * 0.6,
            "the dock is centred (at {centre:?}, width {:?})",
            size.width
        );
        assert!(
            cx.debug_bounds("dock-App_Delete").is_some(),
            "a disabled dock command is still visible"
        );
        assert!(cx.debug_bounds("dock-search").is_some());
        assert!(
            cx.debug_bounds("dock-GizmoOverlay_Settings").is_none(),
            "the native gizmo settings are dev-only and must not be on the dock"
        );
        assert!(
            cx.debug_bounds("dock-GizmoOverlayShader_Settings").is_none(),
            "the shader gizmo settings are dev-only and must not be on the dock"
        );
        // The base shell only has the two modification commands, so there is no
        // modification/creation boundary yet; a workbench that contributes a
        // creation command inserts `dock-divider-1` there.
        assert!(
            cx.debug_bounds("dock-divider-2").is_some(),
            "the Search slot has a hairline"
        );
        assert!(
            !dock_enabled(&model, "App_Delete"),
            "nothing is selected yet"
        );

        // A selection turns the built-ins on: the rows follow IsActive.
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
        draw(&mut cx);
        assert!(dock_enabled(&model, "App_Delete"));
        assert!(dock_enabled(&model, "App_Move"));

        // The Search slot is a stub that reports where the commands are.
        click_at(&mut cx, "dock-search");
        assert!(
            model.lock().unwrap().status.contains("no command palette"),
            "status: {}",
            model.lock().unwrap().status
        );
    }

    /// G3 UI (option 2): the gizmo's options live on the tool. `Move` grows a caret
    /// only while a selection has armed a gizmo; an idle dock shows plain `Move`. The
    /// caret opens upward, away from the bottom edge the dock sits on.
    #[gpui::test]
    fn the_dock_shows_the_gizmo_control_only_while_a_gizmo_is_armed(
        cx: &mut TestAppContext,
    ) {
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
        draw(&mut cx);

        assert!(
            cx.debug_bounds("dock-App_Move-caret").is_none(),
            "nothing is armed, so Move stays plain text"
        );

        // The base shell has no gizmo factory, so set the flag the selection path sets
        // when `arm` returns a gizmo.
        arm_and_redraw(&model, &shell, &mut cx, true);
        let label = cx.debug_bounds("dock-App_Move").expect("the Move entry");
        let caret = cx
            .debug_bounds("dock-App_Move-caret")
            .expect("the options caret");
        assert!(
            caret.origin.x >= label.origin.x,
            "the caret follows the tool label"
        );
        assert!(
            cx.debug_bounds("dock-gizmo-label").is_none(),
            "the options live on the tool, not in a second row"
        );
        let dock = cx.debug_bounds("action-dock").expect("the dock");

        // The caret opens the menu upward into the canvas.
        click_at(&mut cx, "dock-App_Move-caret");
        let menu = cx
            .debug_bounds("gizmo-popover")
            .expect("the gizmo menu");
        assert!(
            menu.origin.y < dock.origin.y,
            "the menu opens above the dock (menu {menu:?}, dock {dock:?})"
        );
        assert!(
            shell.read_with(&cx, |shell, _| shell.open_popover.clone()).as_deref()
                == Some("gizmo"),
            "the dock owns the gizmo popover"
        );

        // Losing the gizmo removes the caret with it, but the tool entry stays.
        arm_and_redraw(&model, &shell, &mut cx, false);
        assert!(
            cx.debug_bounds("dock-App_Move-caret").is_none(),
            "the options go with the handle"
        );
        assert!(
            cx.debug_bounds("dock-App_Move").is_some(),
            "Move itself remains a plain dock entry"
        );
    }

    /// The dock's gizmo menu is provider-aware like Grid and Axes: opening it reads the
    /// registry, so the backends (native and shader) are visible where the tool is.
    #[gpui::test]
    fn the_gizmo_menu_lists_every_registered_provider(cx: &mut TestAppContext) {
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
        arm_and_redraw(&model, &shell, &mut cx, true);

        // Opening the menu is what reads the registry (the same lazy read Grid and Axis
        // use); before that the shell has no provider list to render.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.open_popover("gizmo", cx));
        });
        draw(&mut cx);
        let features = shell
            .read_with(&cx, |shell, _| shell.provider_features.clone())
            .expect("opening the gizmo menu reads the provider registry");
        let gizmo = features
            .iter()
            .find(|feature| feature.feature == "gizmo")
            .expect("the gizmo mods register their feature at boot");
        assert!(!gizmo.providers.is_empty(), "the gizmo feature has a backend");
        for (backend, selector) in [
            ("ferrocad", "gizmo-row-ferrocad-provider:gizmo:ferrocad"),
            (
                "ferrocad-shader",
                "gizmo-row-ferrocad-provider:gizmo:ferrocad-shader",
            ),
        ] {
            if gizmo.providers.iter().any(|provider| provider.backend == backend) {
                assert!(
                    cx.debug_bounds(selector).is_some(),
                    "{backend} is listed in the tool's menu"
                );
            }
        }
    }

    /// Set the armed flag the way a selection does, and force a frame.
    fn arm_and_redraw(
        model: &Arc<Mutex<ShellModel>>,
        shell: &Entity<Shell>,
        cx: &mut VisualTestContext,
        armed: bool,
    ) {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                model.lock().unwrap().gizmo_armed = armed;
                if !armed {
                    shell.open_popover = None;
                }
                cx.notify();
            });
        });
        draw(cx);
    }

    /// The dock's enablement comes from the same `IsActive` list the shell keeps.
    fn dock_enabled(model: &Arc<Mutex<ShellModel>>, id: &str) -> bool {
        model
            .lock()
            .unwrap()
            .commands
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.enabled)
            .unwrap_or(false)
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
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
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
        // Boot first: it puts the facade on `sys.path`, which the reset needs.
        reset_grid_state();
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

        // A preference change goes through the one grid store; the command applies it.
        let script = r#"
import FerroCAD
grid = FerroCAD.Grid
grid.minor = 2.0
grid.major = 20.0
grid.radius = 750.0
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
