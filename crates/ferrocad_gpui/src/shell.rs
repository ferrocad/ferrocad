//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a 3D viewport, a property editor
//! and a Python console. The layout and the interpreter wiring are the S1 deliverable:
//! the tree, editor and console are already driven by the live document through
//! `python/ferrocad_shell`; the viewport renders the sample scene until the geometry
//! kernel/`ViewProvider`s produce real geometry; richer widgets, commands and menus
//! come in later slices (see `docs/direction/app-shell-vision.md`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    ScrollHandle, SharedString, Size, Subscription, Window, WindowHandle, WindowId, anchored,
    canvas, deferred, div, point, prelude::*, px, rgb, rgba, size,
};

use ferrocad_geom::SelectionRef;
use ferrocad_inventor::{Scene, Viewport};
use ferrocad_stage::{
    DeclarativeManipulator, InteractionCommit, Manipulator, ToolStatus, TransformGizmo,
};
use ferrocad_ui::Form;
use ferrocad_viewport::scene;
use ferrocad_viewport::{
    HoverReport, InventorViewport, PreviewPhase, SelectionAnchor, SelectionEvent, TransformPreview,
    ViewportEvent,
};
use ferrocad_wgpu::{AdapterChoice, AdapterSummary, WgpuRenderer};
use ferrocad_widgets::accordion::{AccordionSection, AccordionState, accordion};
use ferrocad_widgets::color;
use ferrocad_widgets::form;
use ferrocad_widgets::scroll::scroll_area;
use ferrocad_widgets::slider;
use ferrocad_widgets::wheel::{WheelEvent, WheelMenu, WheelSlot, WheelTone};
use ferrocad_widgets::window_chrome::window_frame_with_leading;
use ferrocad_widgets::{ChangeEvent, SubmitEvent, TextAreaState, TextInput};

use crate::commands::{self, CommandInfo};
use crate::contributions::Contributions;
use crate::dialog::{self, DialogSpec, Kind, ValueState};
use crate::display::{DisplaySync, StageMesh};
use crate::forms::{FormDialog, FormEvent, FormResult};
use crate::manipulators;
use crate::python::{self, AxisSummary, Bootstrap, DocumentNode, GridSummary, PropRow};
use crate::selection::{self, SelectionItem};
use crate::shortcuts::Accelerator;
use crate::dev_tools::DevTools;
use crate::view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};

// The packed-sRGB tokens live in the leaf `ferrocad_theme` crate (V0 of
// `docs/interaction/visual-language.md`). Re-exported here so the shell's modules
// keep their `crate::shell::FG` paths while the widget layer can use the same table
// without depending on the shell.
pub(crate) use ferrocad_theme::ui::{
    ACCENT, AXIS_X, AXIS_Y, AXIS_Z, BG, DOCK_BORDER, FG, MUTED, PANEL, PANEL_ALT, WHEEL_GLASS,
};

/// The registry feature whose provider supplies the gizmo skin.
const GIZMO_FEATURE: &str = "gizmo";

/// The property-row kind whose string value is a `#rrggbb` colour.
const KIND_COLOR: &str = "color";
/// The property-row kind of a boolean (rendered as a toggle switch).
const KIND_BOOL: &str = "bool";
/// The property-row kind of an enumeration (rendered as a select box).
const KIND_ENUM: &str = "enum";

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

/// The commands matching a palette query, case-insensitively over the id, the label,
/// the tooltip and the accelerator. An empty query lists every command.
fn palette_filtered<'a>(commands: &'a [CommandInfo], query: &str) -> Vec<&'a CommandInfo> {
    let needle = query.trim().to_lowercase();
    commands
        .iter()
        .filter(|command| {
            needle.is_empty()
                || command.id.to_lowercase().contains(&needle)
                || command.label().to_lowercase().contains(&needle)
                || command.tooltip.to_lowercase().contains(&needle)
                || command.accel.to_lowercase().contains(&needle)
        })
        .collect()
}

/// Whether the event is the palette's `Ctrl+K` (`Cmd+K` on macOS).
fn is_palette_shortcut(event: &KeyDownEvent) -> bool {
    let modifiers = &event.keystroke.modifiers;
    event.keystroke.key == "k"
        && (modifiers.control || modifiers.platform)
        && !modifiers.alt
        && !modifiers.shift
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
    /// The rendered form dialog (`FreeCADGui.Control.showDialog`). It owns the
    /// keyboard while present; a command dialog and a form never share the shell.
    form_dialog: Option<Entity<FormDialog>>,
    /// Keeps the form's [`FormEvent`] subscription alive; a footer-button close
    /// releases the shell modal through it.
    form_events: Option<Subscription>,
    /// Focus the shell root on the next render after a form opens, so `Esc`
    /// (reject) and `Enter` (accept) reach [`Shell::handle_key`].
    focus_form: bool,
    /// The result the last form left when it closed (`None` for `closeDialog`).
    form_result: Option<FormResult>,
    /// The Ctrl+K command palette, when it is open.
    palette: Option<Palette>,
    /// Focus the palette's field on the next render (it opens outside a window
    /// context, and from either window).
    focus_palette: bool,
    /// The focus to hand back when the palette closes, captured when it took the
    /// keyboard.
    palette_restore_focus: Option<FocusHandle>,
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
    /// The anchored overlay a property row opened (a colour palette or a range
    /// slider). It is UI state only: the value itself lives in the parameter store.
    row_popup: Option<RowPopup>,
    /// The radial sidecar: it owns the gesture and its overlay. The shell only
    /// triggers it (right-press / Space) and maps a selected id to a command.
    wheel: Entity<WheelMenu>,
    /// Keeps the wheel's event subscription alive.
    _wheel_events: Subscription,
    /// The context target the open wheel resolved. It is promoted to the selection
    /// only when a verb is chosen; closing the wheel discards it.
    wheel_target: Option<WheelTarget>,
    /// The last pointer position in window coordinates. Space opens the wheel
    /// there; a keyboard-only session falls back to the window centre.
    last_pointer: Option<Point<Pixels>>,
    /// The Δ fields at the moment a gizmo drag began. A drag reports its transform
    /// relative to its own start, so this is what makes successive drags accumulate
    /// into the same editable values.
    drag_baseline: Option<[f64; 3]>,
    /// The floating HUD's last painted rect (window coordinates). It is the hit-test
    /// for "is the pointer over the HUD" and supplies the real size for clamping.
    hud_rect: Option<Bounds<Pixels>>,
    /// The floating HUD's frozen origin. Set while the pointer is over the HUD or a
    /// field is focused, so typing a large Δ never slides the inputs out from under
    /// the cursor. Cleared when both conditions lapse.
    hud_lock: Option<Point<Pixels>>,
    /// The Dev Tools second window, if one has been opened. The window owns no model:
    /// its view reads this `Shell` entity. A closed window leaves the handle stale
    /// until the next toggle notices and reopens.
    dev_tools: Option<WindowHandle<DevTools>>,
    /// Which Dev Tools panel is expanded. The window's column cannot hold every
    /// pane, so the inactive ones collapse to their headers (see
    /// [`ferrocad_widgets::accordion`]); this outlives a closed and reopened window.
    dev_accordion: AccordionState,
    /// The Dev Tools panes' scroll positions. gpui reserves scrollbar space but
    /// paints no scrollbar, so the widget needs a tracked handle to size and place
    /// its thumb; owning them here keeps a pane's position across repaints and a
    /// closed/reopened window.
    dev_scroll: DevScroll,
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

/// One tracked scroll handle per Dev Tools pane.
///
/// `ScrollHandle` is a shared cell, so cloning it is cheap; the shell owns the set
/// only so each pane keeps its position across repaints and window reopens. The
/// settings card has its own handle because it is a separate surface over the same
/// property rows.
#[derive(Default)]
struct DevScroll {
    providers: ScrollHandle,
    gpu: ScrollHandle,
    commands: ScrollHandle,
    model: ScrollHandle,
    properties: ScrollHandle,
    settings: ScrollHandle,
}

/// What an anchored property-row overlay shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowPopupKind {
    /// The fixed colour palette (`kind == "color"`).
    Color,
    /// The log/linear range slider (a bounded number).
    Slider,
    /// The choices of an enumeration (`kind == "enum"`).
    Choices,
}

/// A slider's declared range.
#[derive(Clone, Copy, Debug)]
struct RowRange {
    min: f64,
    max: f64,
}

/// The overlay a property row opens, anchored below the pointer.
///
/// It floats over the pane instead of pushing its rows down (the palette used to
/// expand inline). `window` gates rendering: the same rows are built for the
/// settings card and for the Dev Tools window, and the overlay belongs to the one
/// that was clicked.
struct RowPopup {
    key: String,
    label: String,
    kind: RowPopupKind,
    /// The row value when the popup opened (`#94a3b8`, `80`, `10 mm`).
    value: String,
    /// The slider's range; `None` for the palette.
    range: Option<RowRange>,
    /// The quantity unit (`mm`), for the slider's readout.
    unit: String,
    /// An enumeration's options, for the dropdown.
    choices: Vec<String>,
    /// When set, a pick edits this dialog field instead of `key`'s property.
    dialog_field: Option<usize>,
    /// The pointer position that opened it (window coordinates).
    anchor: Point<Pixels>,
    /// The slider track's last painted bounds.
    track: Option<Bounds<Pixels>>,
    /// The window that opened it.
    window: WindowId,
    /// The slider value while dragging (committed on release).
    draft: Option<f64>,
    dragging: bool,
}

/// The modeless settings card (A3/T5): a draggable 380 px panel over the canvas.
///
/// It shows one `Settings/<Feature>` node and is **independent of the document
/// selection**: opening settings must not deselect the object being edited (which
/// used to drop its gizmo), and selecting something else must not close the card.
/// It is dismissed explicitly — its `✕` or `Esc`.
struct SettingsCard {
    /// The synthetic settings node the card edits (`Settings/Gizmo`).
    node: String,
    /// The rows for `node`, re-read after every edit: the card cannot use
    /// `ShellModel::properties`, which follows the selection.
    rows: Vec<PropRow>,
    /// Window coordinates of the card's top-left corner.
    position: Point<Pixels>,
    /// Pointer and card origin when a drag starts.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
}

/// The open command palette (Ctrl+K): a query, a cursor into the filtered rows,
/// and the focus to hand back when it closes.
struct Palette {
    /// The focused search field.
    input: Entity<TextInput>,
    /// The live query, mirrored from the field's `ChangeEvent`.
    query: String,
    /// The cursor into the filtered rows (clamped at render and accept time).
    selected: usize,
    /// The focus that held the keyboard before the palette took it.
    previous_focus: Option<FocusHandle>,
    /// Drop the field's subscriptions with the palette.
    _change: Subscription,
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
    /// Keep the per-field change subscriptions alive: a live preview reads them.
    _changes: Vec<Subscription>,
}

/// What a wheel was opened for: a **context target**, not a selection change.
///
/// Opening a context menu must not mutate the document's selection; the target is
/// promoted only when the user chooses an object verb.
#[derive(Clone, Debug)]
struct WheelTarget {
    reference: SelectionRef,
    anchor: Option<SelectionAnchor>,
}

impl Shell {
    /// Boot the interpreter, create the sample document, and return the model the
    /// view is built from. The view itself is created with `from_model` because
    /// entities (the console input) need an `App` context.
    pub fn boot() -> Result<Arc<Mutex<ShellModel>>, String> {
        let mut boot: Bootstrap = python::bootstrap()?;
        python::open_startup_document(&mut boot)?;
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
        // The edition's appearance preset is materialised in the parameter store, but a
        // *stored* value is not a request: without this the first window would draw the
        // built-in Rust defaults and only pick up the preset after the user touched a
        // setting. Ask the facade to fold the preset and the user's values into one
        // request, then apply it exactly like a command's.
        let mut view = ViewState::default();
        match python::appearance_view() {
            Ok(report) => {
                if let Some(request) = report.view.as_ref() {
                    if let Err(error) = view.apply(request) {
                        console.push(format!("appearance: {error}"));
                    }
                }
            }
            Err(error) => console.push(format!("appearance: {error}")),
        }
        let _ = python::set_view_state(&view);
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
            view,
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
        // The radial sidecar: one entity for the window's lifetime. It owns the
        // gesture and overlay; the shell maps its selection to a registry command.
        let wheel = cx.new(WheelMenu::new);
        let wheel_events = cx.subscribe(
            &wheel,
            |this: &mut Shell, _wheel, event: &WheelEvent, cx| match event {
                WheelEvent::Selected(id) => this.commit_wheel(id.as_ref(), cx),
                WheelEvent::Closed => {
                    this.wheel_target = None;
                    cx.notify();
                }
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
            form_dialog: None,
            form_events: None,
            focus_form: false,
            form_result: None,
            palette: None,
            focus_palette: false,
            palette_restore_focus: None,
            root_focus: cx.focus_handle(),
            open_popover: None,
            popover_anchor: None,
            grid_summary: None,
            axis_summary: None,
            provider_features: None,
            gpu_preference: None,
            gpu_adapters: None,
            settings_card: None,
            row_popup: None,
            wheel,
            _wheel_events: wheel_events,
            wheel_target: None,
            last_pointer: None,
            drag_baseline: None,
            hud_rect: None,
            hud_lock: None,
            dev_tools: None,
            dev_accordion: AccordionState::with_open([
                dev_section::COMMANDS,
                dev_section::MODEL,
                dev_section::PROPERTIES,
            ]),
            dev_scroll: DevScroll::default(),
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

    /// Open (or retarget) the modeless settings card for a `Settings/<Feature>`
    /// node.
    ///
    /// This deliberately does **not** touch the document selection: a settings
    /// panel is a tool over the model, not a selection, so the object being edited
    /// keeps its selection and its gizmo. The card is dismissed only by its own
    /// `✕` or `Esc`.
    fn open_settings_card(&mut self, node: &str, cx: &mut Context<Self>) {
        let rows = python::properties(node).unwrap_or_default();
        match self.settings_card.as_mut() {
            Some(card) => {
                card.node = node.to_string();
                card.rows = rows;
            }
            None => {
                self.settings_card = Some(SettingsCard {
                    node: node.to_string(),
                    rows,
                    position: point(px(220.), px(96.)),
                    drag: None,
                });
            }
        }
        cx.notify();
    }

    /// Re-read the card's rows (after an edit, a reset or a provider switch).
    fn refresh_settings_card(&mut self) {
        let Some(card) = self.settings_card.as_mut() else {
            return;
        };
        if let Ok(rows) = python::properties(&card.node) {
            card.rows = rows;
        }
    }

    /// Close the settings card; returns whether it was open.
    fn close_settings_card(&mut self, cx: &mut Context<Self>) -> bool {
        self.row_popup = None;
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

    /// Open the command palette (Ctrl+K, or the dock's Search slot).
    ///
    /// The palette is modal over the canvas, so it dismisses a header popover. It is
    /// deliberately not opened over a command dialog: that dialog owns the keyboard
    /// and its own accept/cancel path.
    pub(crate) fn open_palette(&mut self, cx: &mut Context<Self>) {
        if self.modal_open() {
            return;
        }
        // A latched wheel is a smaller layer; a modal replaces it rather than
        // surviving underneath the palette.
        let _ = self.close_radial(cx);
        self.open_popover = None;
        self.popover_anchor = None;
        let input = cx.new(|cx| TextInput::new(cx, "Search commands"));
        let change = cx.subscribe(
            &input,
            |this: &mut Shell, _input, event: &ChangeEvent, cx| {
                this.palette_query_changed(event.text.clone(), cx);
            },
        );
        // Enter in the field runs the selection; the field stops the key, so this
        // `SubmitEvent` is the palette's accept path.
        let submit = cx.subscribe(
            &input,
            |this: &mut Shell, _input, _event: &SubmitEvent, cx| {
                this.palette_accept(cx);
            },
        );
        self.palette = Some(Palette {
            input,
            query: String::new(),
            selected: 0,
            previous_focus: None,
            _change: change,
            _submit: submit,
        });
        self.focus_palette = true;
        cx.notify();
    }

    /// Close the palette; returns whether it was open. The focus that opened it is
    /// restored on the next render.
    fn close_palette(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(palette) = self.palette.take() else {
            return false;
        };
        self.palette_restore_focus = palette.previous_focus;
        cx.notify();
        true
    }

    /// `Ctrl+K`: open the palette, or close it when it is already open.
    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(cx);
        } else {
            self.open_palette(cx);
        }
    }

    /// Mirror the field's text into the query and rewind the cursor: a shorter list
    /// must never leave the cursor past its end.
    fn palette_query_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            palette.query = text;
            palette.selected = 0;
        }
        cx.notify();
    }

    /// Move the palette cursor over the filtered rows, wrapping at both ends.
    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let len = {
            let model = self.model.lock().unwrap();
            palette_filtered(&model.commands, &palette.query).len()
        };
        if len == 0 {
            return;
        }
        if let Some(palette) = self.palette.as_mut() {
            palette.selected =
                (palette.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    /// Enter in the palette: run the selected command, but only when it is enabled.
    /// A disabled row stays visible (like the dock's greyed entries) and says why
    /// nothing happened instead of silently doing nothing.
    fn palette_accept(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let query = palette.query.clone();
        let selected = palette.selected;
        let target = {
            let model = self.model.lock().unwrap();
            let rows = palette_filtered(&model.commands, &query);
            let index = selected.min(rows.len().saturating_sub(1));
            rows.get(index)
                .map(|command| (command.id.clone(), command.enabled))
        };
        let Some((id, enabled)) = target else {
            return;
        };
        if !enabled {
            let status = format!("{id}: disabled");
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }
        // Run it exactly as a click would: the palette has served its purpose.
        self.close_palette(cx);
        self.run_command(&id, cx);
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
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A rendered form dialog is modal like a command dialog or the palette:
        // Esc rejects it, Enter accepts it, and no registry accelerator (or Ctrl+K)
        // fires over it. Keys it does not own bubble without running a command.
        if self.form_dialog.is_some() {
            return match event.keystroke.key.as_str() {
                "escape" => {
                    self.reject_form(cx);
                    true
                }
                "enter" => {
                    self.accept_form(cx);
                    true
                }
                _ => false,
            };
        }
        // The palette is modal: Esc closes it before the popover, tool and card paths.
        if event.keystroke.key == "escape" && self.close_palette(cx) {
            return true;
        }
        if is_palette_shortcut(event) {
            self.toggle_palette(cx);
            return true;
        }
        // The radial menu closes before any popover: it is the most transient layer.
        if event.keystroke.key == "escape" && self.close_radial(cx) {
            return true;
        }
        // A floating dialog does not own the keyboard once the pointer clicks the
        // canvas (the viewport focuses itself), so dismiss it here as well: `Esc` must
        // cancel from anywhere while the HUD is open. A panel that already handled the
        // key has cleared `dialog`, making this a no-op.
        if event.keystroke.key == "escape" && self.dialog.is_some() {
            self.cancel_dialog(cx);
            return true;
        }
        // Space on a selection opens the object-verb wheel at the last pointer
        // position (or the window centre for a keyboard-only session). It is the
        // main window's gesture, so a Dev Tools key press does not open it off-screen.
        if !cancel_dialog
            && event.keystroke.key == "space"
            && self.dialog.is_none()
            && self.palette.is_none()
            && self.settings_card.is_none()
            && self.active_tool.is_none()
        {
            return self.space_radial(window, cx);
        }
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
            ViewportEvent::TransformPreview(preview) => self.on_transform_preview(preview, cx),
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
        // A selection change invalidates the object verbs the wheel was built for.
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
        let (manipulators, gizmo) = match selection {
            SelectionEvent::Picked { reference, .. } => {
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
    fn radial_slots(&self, target: Option<&SelectionRef>) -> Vec<WheelSlot> {
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
    fn wheel_target_at(
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
    fn open_radial_latched(&mut self, anchor: Point<Pixels>, cx: &mut Context<Self>) {
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
    fn commit_wheel(&mut self, id: &str, cx: &mut Context<Self>) {
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
                self.on_selection(
                    &SelectionEvent::Picked {
                        reference: target.reference,
                        anchor: target.anchor,
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
    fn hover_radial(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.pointer_move(position, cx));
    }

    /// Forward the arming release (the sidecar commits, latches or cancels).
    fn release_radial(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.pointer_up(position, cx));
    }

    /// The `Space` path: commit a hovered latched wedge, close an open wheel, or open
    /// **contextually** — the entity under the pointer is the context target, so the key
    /// is spatial rather than a blind global shortcut.
    fn space_radial(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
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
                // The factory owns the datum: the triad hugs the object's own origin
                // (`ShapeItem::anchor`), which also makes it an origin indicator and the
                // pivot a rotate gizmo needs. A click point is deliberately not used —
                // for translation the pivot does not change the result, and re-pivoting
                // will be an explicit, snapped gesture (see transform-ux.md).
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

    /// A manipulator drag ended: apply the commit through the app script's
    /// `interaction` hook, recompute, and re-read the shapes.
    fn on_interaction(&mut self, commit: &InteractionCommit, cx: &mut Context<Self>) {
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
    fn rearm_selection(&mut self, cx: &mut Context<Self>) {
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

    /// Open a command's dialog, seeding one control per field.
    fn open_dialog_state(&mut self, token: String, spec: DialogSpec, cx: &mut Context<Self>) {
        // A wizard or a rerun replaces the previous dialog: drop any preview it left.
        self.clear_dialog_preview(cx);
        // A new HUD starts free: the previous freeze belonged to the old anchor.
        self.hud_rect = None;
        self.hud_lock = None;
        // A dialog outranks the transient wheel.
        let _ = self.close_radial(cx);
        let previews_translation = spec.previews_translation();
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
        if previews_translation {
            // A gizmo drag now reports previews instead of committing: the fields and
            // the handles describe one pending move until the dialog accepts.
            self.defer_gizmo_preview(true, cx);
        }
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

    /// Drive a spec's live preview from the current field text.
    ///
    /// `"translation"` composes a temporary transform on the selected entities; the
    /// document is untouched, so `Esc` is a true cancel and `Enter` is still the only
    /// commit. Text that does not parse yet previews as zero rather than flashing an
    /// error mid-keystroke.
    fn preview_dialog(&mut self, cx: &mut Context<Self>) {
        let delta = {
            let Some(dialog) = self.dialog.as_ref() else {
                return;
            };
            if !dialog.spec.previews_translation() {
                return;
            }
            let states = self.dialog_states(cx);
            let Some(delta) = dialog::translation_preview(&dialog.spec, &states) else {
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
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            if viewport.preview_translation(&entities, delta) {
                cx.notify();
            }
        });
        // The gizmo (and therefore the HUD anchor) follows the previewed object, so
        // re-render the shell to keep the panel beside it.
        cx.notify();
    }

    /// Drop a typed preview without changing the document (cancel or dismiss).
    fn clear_dialog_preview(&mut self, cx: &mut Context<Self>) {
        self.drag_baseline = None;
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, cx| {
            viewport.set_gizmo_preview(false);
            if viewport.preview_translation(&[], [0.0; 3]) {
                cx.notify();
            }
        });
    }

    /// Enable or disable the viewport's deferred gizmo commit.
    fn defer_gizmo_preview(&mut self, deferred: bool, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        viewport.update(cx, |viewport, _cx| viewport.set_gizmo_preview(deferred));
    }

    /// The Δ values currently in the fields, when the spec previews a translation.
    fn translation_in_fields(&self, cx: &App) -> Option<[f64; 3]> {
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
    fn set_translation_fields(&mut self, delta: [f64; 3], cx: &mut Context<Self>) {
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

    /// A live gizmo drag while a translation HUD is open: mirror the accumulated delta
    /// into the Δ fields and keep the whole selection previewing. Nothing reaches the
    /// document, so the drag and the typed values describe the same pending move.
    fn on_transform_preview(&mut self, preview: &TransformPreview, cx: &mut Context<Self>) {
        if !self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
        {
            return;
        }
        if preview.phase == PreviewPhase::Begin {
            // A drag reports its transform relative to its own start; the fields keep
            // whatever they held, so successive drags accumulate into the same move.
            self.drag_baseline = self.translation_in_fields(cx);
        }
        let baseline = self.drag_baseline.unwrap_or([0.0; 3]);
        let m = &preview.total_transform.m;
        let delta = [baseline[0] + m[3], baseline[1] + m[7], baseline[2] + m[11]];
        self.set_translation_fields(delta, cx);
        self.preview_dialog(cx);
        if preview.phase == PreviewPhase::End {
            self.drag_baseline = None;
        }
    }

    /// Where a floating dialog should sit: beside the gizmo, else under the live
    /// pointer. The gizmo wins when it is visible, so the HUD hangs off the handles
    /// the user is about to drag; otherwise it opens where they are looking.
    fn dialog_anchor(&self, window: &Window, cx: &App) -> Point<Pixels> {
        self.viewport
            .as_ref()
            .and_then(|viewport| viewport.read(cx).gizmo_anchor())
            .unwrap_or_else(|| window.mouse_position())
    }

    /// Move focus between a dialog's textual fields. `Tab` cycles forward; `Shift+Tab`
    /// back. Fields without an input (bool/enum) are not stops.
    fn cycle_dialog_focus(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                // The pending move is now applied (or replaced by a wizard): release
                // the deferred gizmo and drop the ghost before the stage rebuild.
                self.clear_dialog_preview(cx);
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
    fn cancel_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let _ = commands::cancel(&dialog.token);
        self.clear_dialog_preview(cx);
        self.model.lock().unwrap().status = "dialog cancelled".to_string();
        cx.notify();
    }

    /// Whether a modal currently owns the shell: a command dialog, the palette or
    /// a rendered form. At most one of them may be present, so a second backdrop
    /// can never be stacked on the first.
    fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.palette.is_some() || self.form_dialog.is_some()
    }

    /// Whether a rendered form dialog is open (`FreeCADGui.Control.showDialog`).
    pub fn form_open(&self) -> bool {
        self.form_dialog.is_some()
    }

    /// The open form dialog, for tests and for the host that drives it.
    pub fn form_dialog(&self) -> Option<&Entity<FormDialog>> {
        self.form_dialog.as_ref()
    }

    /// The result the last form left (`None` for a plain `closeDialog`).
    pub fn last_form_result(&self) -> Option<FormResult> {
        self.form_result
    }

    /// Open a parsed form as the shell's modal.
    ///
    /// This mirrors `FreeCADGui.Control.showDialog`: the form is rendered above
    /// the workspace and owns `Esc` (reject) and `Enter` (accept). When a command
    /// dialog, the palette or another form already owns the shell, nothing is
    /// opened and `false` is returned, so the caller can drop its request.
    pub fn open_form(
        &mut self,
        form: Form,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.modal_open() {
            let status = "a dialog is already open".to_string();
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return false;
        }
        // A header menu or a latched wheel would sit behind the modal; dismiss them
        // with the open.
        let _ = self.close_radial(cx);
        self.open_popover = None;
        self.popover_anchor = None;
        let dialog = cx.new(|_| FormDialog::new(form, name));
        let events = cx.subscribe(&dialog, |this: &mut Shell, dialog, event: &FormEvent, cx| {
            this.finish_form(&dialog, event, cx);
        });
        let opened = dialog.read(cx).name().to_string();
        self.form_dialog = Some(dialog);
        self.form_events = Some(events);
        self.form_result = None;
        self.focus_form = true;
        let status = format!("form {opened}: open");
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
        true
    }

    /// Read a `.ui` file and open it as the shell's modal.
    ///
    /// `Ok(false)` means another modal already owns the shell (see [`open_form`]);
    /// `Err` is a read/parse failure with the file named.
    pub fn open_form_path(
        &mut self,
        path: impl AsRef<Path>,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let path = path.as_ref();
        let form = Form::from_path(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(self.open_form(form, name, cx))
    }

    /// Close the open form without recording a result
    /// (`FreeCADGui.Control.closeDialog`). Returns whether one was open.
    pub fn close_form(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.form_dialog.clone() else {
            return false;
        };
        dialog.update(cx, |dialog, cx| dialog.close(cx));
        true
    }

    /// Reject the open form; the modal is released through [`FormEvent`].
    fn reject_form(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.form_dialog.clone() {
            dialog.update(cx, |dialog, cx| dialog.reject(cx));
        }
    }

    /// Accept the open form; the modal is released through [`FormEvent`].
    fn accept_form(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.form_dialog.clone() {
            dialog.update(cx, |dialog, cx| dialog.accept(cx));
        }
    }

    /// Release the shell's modal when the form closes, whether that was `Esc`,
    /// `Enter`, a footer button or `close_form`.
    fn finish_form(
        &mut self,
        dialog: &Entity<FormDialog>,
        event: &FormEvent,
        cx: &mut Context<Self>,
    ) {
        // A late event from a form the shell already released must not clear a
        // newer modal.
        if self.form_dialog.as_ref().map(|dialog| dialog.entity_id()) != Some(dialog.entity_id()) {
            return;
        }
        let FormEvent::Closed(result) = event;
        let name = dialog.read(cx).name().to_string();
        self.form_dialog = None;
        self.form_events = None;
        self.form_result = *result;
        let status = match result {
            Some(FormResult::Accepted) => format!("form {name}: accepted"),
            Some(FormResult::Rejected) => format!("form {name}: rejected"),
            None => format!("form {name}: closed"),
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
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
        // Opening menus never mutates the selection. A **new** selection, however,
        // invalidates both an open wheel (its context target no longer matches the
        // selection) and a pending translation form (its delta was measured against
        // the old selection), so both close before the new selection is reported.
        let wheel = self.wheel.clone();
        let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
        self.wheel_target = None;
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.spec.previews_translation())
            && let Some(dialog) = self.dialog.take()
        {
            let _ = commands::cancel(&dialog.token);
            self.clear_dialog_preview(cx);
        }
        let primary = items.last().map(|item| item.object.clone());
        let properties = primary
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        self.prop_inputs.clear();
        self.row_popup = None;
        let status = match (items.len(), primary.as_deref()) {
            (0, _) | (_, None) => "selection cleared".to_string(),
            (1, Some(name)) => format!("selected {name}"),
            (count, Some(name)) => format!("selected {count} · {name}"),
        };
        // A tree click on a settings node retargets the card. Selecting an object
        // does **not** close it: the settings panel is dismissed explicitly, so a
        // user can keep editing while an object stays selected (and its gizmo on).
        let settings_node = primary
            .as_deref()
            .filter(|name| name.starts_with("Settings/"))
            .map(str::to_string);
        {
            let mut m = self.model.lock().unwrap();
            m.selection = items;
            m.selected = primary;
            m.properties = properties;
            m.status = status;
        }
        if let Some(node) = &settings_node {
            self.open_settings_card(node, cx);
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
        let (next, primary, pruned) = {
            let m = self.model.lock().unwrap();
            let mut next = m.selection.clone();
            next.retain(|item| live_node(&item.object, &live));
            let pruned = next.len() != m.selection.len();
            let primary = next
                .last()
                .map(|item| item.object.clone())
                .or_else(|| m.selected.clone().filter(|name| live_node(name, &live)));
            (next, primary, pruned)
        };
        // A deleted selection cannot keep a pending form or an open context menu.
        if pruned {
            let wheel = self.wheel.clone();
            let _ = wheel.update(cx, |wheel, cx| wheel.close(cx));
            self.wheel_target = None;
            if self
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.spec.previews_translation())
                && let Some(dialog) = self.dialog.take()
            {
                let _ = commands::cancel(&dialog.token);
                self.clear_dialog_preview(cx);
            }
        }
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
                // The edit is done, so a palette that asked for it can close (the
                // typed-hex path and the swatch path share this commit).
                self.row_popup = None;
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
                // A settings row can ask for the change to be drawn (a palette, a grid
                // spacing, a provider). Applying it here is what makes an edit visible
                // immediately rather than at the next unrelated repaint.
                if let Some(request) = result.view.clone() {
                    if let Err(error) = self.apply_view(&request, cx) {
                        self.console_line(format!("view: {error}"), cx);
                    }
                }
                self.refresh(cx);
                self.sync_stage(cx);
                self.rearm_selection(cx);
                // The card owns its own rows, so a settings edit has to re-read
                // them here as well as refresh the model's properties.
                self.refresh_settings_card();
                format!("set {key}")
            }
            Ok(result) => format!("property error: {}", result.error),
            Err(e) => format!("property error: {e}"),
        };
        self.model.lock().unwrap().status = status;
        cx.notify();
    }

    /// Open the colour palette under a property row.
    ///
    /// Opening is deliberately not a toggle: an outside press dismisses the
    /// overlay before the swatch's click is dispatched, so a toggle here would
    /// immediately reopen it (the same rule as the header popovers).
    fn open_color_popup(
        &mut self,
        key: &str,
        label: &str,
        value: &str,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: label.to_string(),
            kind: RowPopupKind::Color,
            value: value.to_string(),
            range: None,
            unit: String::new(),
            choices: Vec::new(),
            dialog_field: None,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Open the range slider for a bounded numeric property.
    #[allow(clippy::too_many_arguments)]
    fn open_slider_popup(
        &mut self,
        key: &str,
        label: &str,
        value: &str,
        min: f64,
        max: f64,
        unit: &str,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: label.to_string(),
            kind: RowPopupKind::Slider,
            value: value.to_string(),
            range: Some(RowRange { min, max }),
            unit: unit.to_string(),
            choices: Vec::new(),
            dialog_field: None,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Open the dropdown for an enumeration row or a dialog field.
    #[allow(clippy::too_many_arguments)]
    fn open_choice_popup(
        &mut self,
        key: &str,
        value: &str,
        choices: Vec<String>,
        dialog_field: Option<usize>,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: String::new(),
            kind: RowPopupKind::Choices,
            value: value.to_string(),
            range: None,
            unit: String::new(),
            choices,
            dialog_field,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Pick a dialog enum's option from the dropdown.
    fn set_dialog_choice(&mut self, index: usize, choice: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            let position = dialog
                .spec
                .fields
                .get(index)
                .and_then(|field| field.values.iter().position(|value| value == choice));
            if let Some(position) = position {
                if let Some(state) = dialog.states.get_mut(index) {
                    *state = ValueState::Choice(position);
                    dialog.error = None;
                }
            }
        }
        self.row_popup = None;
        cx.notify();
    }

    /// Dismiss the row overlay, if one is open.
    fn close_row_popup(&mut self, cx: &mut Context<Self>) {
        if self.row_popup.take().is_some() {
            cx.notify();
        }
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
        self.rearm_selection(cx);
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

/// The window margin a floating HUD keeps from every edge.
const HUD_MARGIN: f32 = 16.0;
/// The clearance below a floating HUD, so it never covers the bottom dock.
const HUD_DOCK_CLEARANCE: f32 = 64.0;
/// Where the HUD first appears relative to its anchor: below-right of the gizmo.
const HUD_OFFSET_X: f32 = 24.0;
const HUD_OFFSET_Y: f32 = 88.0;

/// The inclusive range of HUD origins that keep the whole rect inside the viewport
/// safe area (16 px margins, 64 px above the bottom dock).
fn hud_origin_range(viewport: Size<Pixels>, hud: Size<Pixels>) -> (f32, f32) {
    let max_x = (viewport.width.0 - hud.width.0 - HUD_MARGIN).max(HUD_MARGIN);
    let max_y = (viewport.height.0 - hud.height.0 - HUD_DOCK_CLEARANCE).max(HUD_MARGIN);
    (max_x, max_y)
}

/// Keep a HUD origin inside the viewport safe area.
fn clamp_hud(origin: Point<Pixels>, viewport: Size<Pixels>, hud: Size<Pixels>) -> Point<Pixels> {
    let (max_x, max_y) = hud_origin_range(viewport, hud);
    point(
        px(origin.x.0.clamp(HUD_MARGIN, max_x)),
        px(origin.y.0.clamp(HUD_MARGIN, max_y)),
    )
}

/// Where a floating HUD appears: below-right of `anchor`, clamped into the viewport
/// safe area. `anchor` is the gizmo's visible origin (or the live pointer); the HUD
/// must never leave the canvas, even when the gizmo is dragged into a corner.
fn place_hud(anchor: Point<Pixels>, viewport: Size<Pixels>, hud: Size<Pixels>) -> Point<Pixels> {
    let offset = point(px(anchor.x.0 + HUD_OFFSET_X), px(anchor.y.0 + HUD_OFFSET_Y));
    clamp_hud(offset, viewport, hud)
}

/// A command dialog (A3): a floating dielectric HUD, or a glass modal over a dimmed
/// shell when the spec keeps the default (`"presentation": "modal"`).
///
/// A floating spec has **no backdrop**: the canvas stays bright and the gizmo keeps
/// working, so typing a delta and dragging an arrow describe the same move. Only the
/// panel itself occludes the pointer. Its first field takes focus, `Tab` cycles the
/// fields, `Enter` accepts and `Esc` cancels; a text field's `Enter` submits through
/// its own `SubmitEvent`, wired to the same accept path.
fn dialog_overlay(shell: &mut Shell, window: &Window, cx: &mut Context<Shell>) -> AnyElement {
    let Some(dialog) = shell.dialog.as_ref() else {
        return div().into_any_element();
    };
    let floating = dialog.spec.is_floating();
    let spec = dialog.spec.clone();
    let states = dialog.states.clone();
    let inputs = dialog.inputs.clone();
    let error = dialog.error.clone();
    let focus = shell.dialog_focus.clone();

    let mut fields = div().flex().gap_2();
    fields = if floating {
        fields.flex_row().items_center().flex_wrap()
    } else {
        fields.flex_col()
    };
    for (index, field) in spec.fields.iter().enumerate() {
        let control: AnyElement = match field.kind() {
            Kind::Bool => {
                let checked = matches!(states.get(index), Some(ValueState::Flag(true)));
                form::toggle(
                    SharedString::from(format!("dialog-bool-{index}")),
                    checked,
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
                let key = format!("dialog-enum-{index}");
                let open = shell
                    .row_popup
                    .as_ref()
                    .is_some_and(|popup| popup.key == key);
                let values = field.values.clone();
                let click_key = key.clone();
                let click_value = value.clone();
                div()
                    .w(px(if floating { 132. } else { 170. }))
                    .flex_shrink_0()
                    .child(form::select_box(
                        SharedString::from(key),
                        &value,
                        open,
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.open_choice_popup(
                                &click_key,
                                &click_value,
                                values.clone(),
                                Some(index),
                                event.position(),
                                window.window_handle().window_id(),
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
            Kind::Unknown => div()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(format!("unsupported kind {}", field.kind))
                .into_any_element(),
            _ => match inputs.get(index).and_then(Option::as_ref) {
                Some(input) => div()
                    .w(px(if floating { 84. } else { 170. }))
                    .flex_shrink_0()
                    .child(input.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
        };
        let label_ink = if floating { axis_token(field) } else { MUTED };
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(label_ink))
                    .child(field.label().to_string()),
            )
            .child(control);
        if !floating {
            row = row.justify_between().gap_3();
        }
        fields = fields.child(row);
    }

    let accept = spec.accept_label().to_string();
    let cancel = spec.cancel_label().to_string();

    if floating {
        let viewport = window.viewport_size();
        // The last painted size is the best estimate before this frame paints; the
        // first frame uses a conservative default.
        let hud_size = shell
            .hud_rect
            .map(|rect| rect.size)
            .unwrap_or_else(|| size(px(600.0), px(72.0)));
        // Freeze while the user is working in the HUD: over its rect, or typing into
        // a field. Otherwise it follows the gizmo, clamped to the safe area. The
        // rect is the one painted last frame, so entering or leaving is compared
        // against what the pointer actually saw.
        let focused = inputs
            .iter()
            .flatten()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window));
        let hovered = shell
            .hud_rect
            .is_some_and(|rect| rect.contains(&window.mouse_position()));
        let follow = place_hud(shell.dialog_anchor(window, cx), viewport, hud_size);
        let origin = if hovered || focused {
            shell.hud_lock.unwrap_or(follow)
        } else {
            follow
        };
        // Resize or a new dialog may shrink the safe area under a frozen origin.
        let origin = clamp_hud(origin, viewport, hud_size);
        shell.hud_lock = Some(origin);
        let mut panel = div()
            .id("dialog-panel")
            .debug_selector(|| "dialog-panel".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgba(WHEEL_GLASS))
            .border_1()
            .border_color(rgba(DOCK_BORDER))
            .occlude()
            .track_focus(&focus)
            .focusable()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.cancel_dialog(cx),
                    "enter" => this.accept_dialog(cx),
                    "tab" => this.cycle_dialog_focus(event.keystroke.modifiers.shift, window, cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(spec.title.clone()),
                    )
                    .child(fields)
                    .child(dialog_buttons(accept, cancel, true, cx)),
            );
        if let Some(error) = &error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xef4444))
                    .child(error.clone()),
            );
        }
        // The wrapper measures the painted panel, so the next frame can hit-test the
        // pointer against exactly where the HUD sits. Storing bounds does not notify:
        // it is read on the following frame.
        let weak = cx.weak_entity();
        let hud = div()
            .on_children_prepainted(move |bounds, _window, cx| {
                if let Some(rect) = bounds.first().copied() {
                    let _ = weak.update(cx, |shell, _cx| shell.hud_rect = Some(rect));
                }
            })
            .child(panel);
        return deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(origin)
                .child(hud),
        )
        .with_priority(2)
        .into_any_element();
    }

    let mut panel = div()
        .id("dialog-panel")
        .debug_selector(|| "dialog-panel".to_string())
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
    panel = panel.child(dialog_buttons(accept, cancel, false, cx));

    div()
        .id("dialog-backdrop")
        .debug_selector(|| "dialog-backdrop".to_string())
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

/// The axis ink for a floating field: `dx`/`dy`/`dz` (or `x`/`y`/`z`) get their
/// tokens, everything else stays muted.
fn axis_token(field: &crate::dialog::DialogField) -> u32 {
    match field.name.to_ascii_lowercase().as_str() {
        "dx" | "x" => AXIS_X,
        "dy" | "y" => AXIS_Y,
        "dz" | "z" => AXIS_Z,
        _ => MUTED,
    }
}

/// The accept/cancel pair. `compact` tightens the padding for the floating HUD.
fn dialog_buttons(
    accept: String,
    cancel: String,
    compact: bool,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let pad = if compact { px(2.) } else { px(3.) };
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_2()
        .child(
            div()
                .id("dialog-cancel")
                .debug_selector(|| "dialog-cancel".to_string())
                .px(pad)
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
                .debug_selector(|| "dialog-accept".to_string())
                .px(pad)
                .py_1()
                .rounded_sm()
                .bg(rgb(ACCENT))
                .text_xs()
                .text_color(rgb(FG))
                .child(accept)
                .on_click(cx.listener(|this, _event, _window, cx| this.accept_dialog(cx))),
        )
}

/// The command palette (Ctrl+K): a focused search field over the filtered registry.
///
/// The field holds keyboard focus, so typing filters and `Esc` bubbles to the shell
/// root's [`Shell::handle_key`]. `Up`/`Down` are not editing keys, so they bubble to
/// this panel's key handler; `Enter` reaches the shell through the field's
/// `SubmitEvent` (see [`Shell::open_palette`]).
fn palette_overlay(
    shell: &Shell,
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let Some(palette) = shell.palette.as_ref() else {
        return div().into_any_element();
    };
    let input = palette.input.clone();
    let query = palette.query.clone();
    let commands = palette_filtered(&model.commands, &query);
    // The cursor may trail the list while commands refresh; clamp it here and in the
    // accept path so the highlighted row is always the row Enter runs.
    let selected = palette.selected.min(commands.len().saturating_sub(1));

    let mut rows = div()
        .id("palette-rows")
        .flex()
        .flex_col()
        .gap_1()
        .min_h_0()
        .max_h(px(320.))
        .overflow_y_scroll();
    if commands.is_empty() {
        rows = rows.child(
            div()
                .debug_selector(|| "palette-empty".to_string())
                .px_2()
                .py_2()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No matching commands"),
        );
    }
    for (index, command) in commands.iter().enumerate() {
        let id = command.id.clone();
        let selector = id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        let is_selected = index == selected;
        let mut row = div()
            .id(SharedString::from(format!("palette-row-{id}")))
            .debug_selector(move || format!("palette-row-{selector}"))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(is_selected, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL_ALT)))
            })
            .child(command.label().to_string())
            .child(div().flex_1())
            .when(checked, |el| {
                el.child(div().text_color(rgb(MUTED)).child("\u{2713}"))
            })
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child(command.surface().as_str()),
            );
        if enabled {
            row = row.on_click(cx.listener(move |this, _event, _window, cx| {
                this.close_palette(cx);
                this.run_command(&id, cx);
            }));
        }
        rows = rows.child(row);
    }

    let panel = div()
        .id("palette")
        .debug_selector(|| "palette".to_string())
        .w(px(520.))
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            match event.keystroke.key.as_str() {
                "up" => {
                    this.move_palette_selection(-1, cx);
                    cx.stop_propagation();
                }
                "down" => {
                    this.move_palette_selection(1, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .child(
            div()
                .debug_selector(|| "palette-input".to_string())
                .rounded_sm()
                .border_1()
                .border_color(rgba(0xffffff26))
                .child(input),
        )
        .child(rows)
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .px_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("{} command(s)", commands.len()))
                .child("\u{2191}\u{2193} select \u{00b7} Enter run \u{00b7} Esc close"),
        );

    div()
        .id("palette-backdrop")
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .pt(px(72.))
        .bg(rgba(0x0f1218b3))
        .occlude()
        .child(panel)
        .into_any_element()
}

/// The model inspector: documents and their objects.
///
/// The accordion supplies the section header, so the pane is only the scrollable
/// rows.
pub(crate) fn tree_panel(
    model: &ShellModel,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col();

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
        .child(scroll_area("dev-model", scroll, rows))
}

/// The property editor: read-only rows plus an uncontrolled field for each
/// editable property.
///
/// The Dev Tools pane shows the current selection; the settings card calls
/// [`properties_rows_panel`] directly with its own node and rows.
pub(crate) fn properties_panel(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement + use<> {
    properties_rows_panel(
        shell,
        model.selected.as_deref(),
        &model.properties,
        window,
        scroll,
        cx,
    )
}

/// The property rows for one node.
///
/// The rows come in as a parameter so a surface that does not own the selection
/// (the floating settings card) can show its own node.
fn properties_rows_panel(
    shell: &mut Shell,
    selected: Option<&str>,
    properties: &[PropRow],
    window: &Window,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement + use<> {
    let mut rows = div().flex().flex_col();

    match selected {
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
                    .child(name.to_string()),
            );
            for row in properties {
                let value: AnyElement = if let Some(command) = row.command.clone() {
                    // A generated action row (the settings card's `Reset to edition
                    // defaults`). It runs a real command, not a property edit, so the
                    // request it returns (the restored appearance) is applied by
                    // `run_command` rather than dropped.
                    let label = if row.label.is_empty() {
                        row.name.clone()
                    } else {
                        row.label.clone()
                    };
                    let selector = format!("prop-command-{command}");
                    div()
                        .debug_selector(move || selector.clone())
                        .id(SharedString::from(format!("prop-command-{command}")))
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .text_xs()
                        .text_color(rgb(FG))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(PANEL_ALT)))
                        .on_click(cx.listener(move |shell, _event, _window, cx| {
                            shell.run_command(&command, cx);
                            cx.notify();
                        }))
                        .child(label)
                        .into_any_element()
                } else if row.editable {
                    let key = format!("{name}.{}", row.name);
                    let selector = format!("prop-{key}");
                    let bounded = row.min.is_some()
                        && row.max.is_some()
                        && matches!(row.kind.as_str(), "float" | "int" | "quantity");
                    let label = row_label(row);
                    let open = shell
                        .row_popup
                        .as_ref()
                        .is_some_and(|popup| popup.key == key);
                    if row.kind == KIND_COLOR {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        // A colour row keeps its text field (a typed hex still works)
                        // and gains a swatch that opens the palette overlay.
                        let popup_key = key.clone();
                        let popup_label = label.clone();
                        let popup_value = row.value.clone();
                        div()
                            .debug_selector(move || selector.clone())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w(px(180.))
                            .flex_shrink_0()
                            .child(color::swatch(
                                SharedString::from(format!("color-{key}")),
                                &row.value,
                                open,
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_color_popup(
                                        &popup_key,
                                        &popup_label,
                                        &popup_value,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .child(div().w(px(120.)).child(state))
                            .into_any_element()
                    } else if row.kind == KIND_BOOL {
                        // A boolean is a switch, not a text field: a click flips it
                        // and commits the opposite of what the row shows.
                        let commit_key = key.clone();
                        let checked = row_bool(&row.value);
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(form::toggle(
                                SharedString::from(format!("prop-bool-{key}")),
                                checked,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.commit_property(
                                        &commit_key,
                                        if checked { "false" } else { "true" },
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else if row.kind == KIND_ENUM && !row.choices.is_empty() {
                        // An enumeration is a select box; its options open in the
                        // same anchored overlay as the palette.
                        let popup_key = key.clone();
                        let popup_value = row.value.clone();
                        let popup_choices = row.choices.clone();
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(form::select_box(
                                SharedString::from(format!("prop-enum-{key}")),
                                &row.value,
                                open,
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_choice_popup(
                                        &popup_key,
                                        &popup_value,
                                        popup_choices.clone(),
                                        None,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else if bounded {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        // A row with a declared range gets the mini track that opens
                        // the slider overlay; typing an exact value still works.
                        let popup_key = key.clone();
                        let popup_label = label.clone();
                        let popup_value = row.value.clone();
                        let popup_unit = row.unit.clone().unwrap_or_default();
                        let min = row.min.unwrap_or(0.0);
                        let max = row.max.unwrap_or(1.0);
                        let current = row_number(&row.value).unwrap_or(min);
                        div()
                            .debug_selector(move || selector.clone())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .w(px(180.))
                            .flex_shrink_0()
                            .child(div().w(px(112.)).child(state))
                            .child(slider::trigger(
                                SharedString::from(format!("slider-{key}")),
                                slider::fraction(current, min, max),
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_slider_popup(
                                        &popup_key,
                                        &popup_label,
                                        &popup_value,
                                        min,
                                        max,
                                        &popup_unit,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(state)
                            .into_any_element()
                    }
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
    panel.child(scroll_area("dev-properties", scroll, rows))
}

/// The display label for a property row: its `label` when the schema supplied
/// one, otherwise its name.
fn row_label(row: &crate::python::PropRow) -> String {
    if row.label.is_empty() {
        row.name.clone()
    } else {
        row.label.clone()
    }
}

/// The number a property row displays (`80`, `0.3`, `10 mm` -> 10).
fn row_number(value: &str) -> Option<f64> {
    value.split_whitespace().next()?.parse().ok()
}

/// Whether a property row's display value reads as `true` (`True`, `1`, `yes`,
/// `on` — the Python and parameter stores spell it differently).
fn row_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

/// The pointer position as a 0..1 fraction of the slider track.
fn track_fraction(popup: &RowPopup, x: Pixels) -> f32 {
    let Some(track) = popup.track else {
        return 0.0;
    };
    let width = track.size.width.0.max(1.0);
    ((x.0 - track.left().0) / width).clamp(0.0, 1.0)
}

/// The anchored overlay a property row opened: the colour palette or the range
/// slider.
///
/// It floats below the pointer, so it covers the pane instead of pushing its rows
/// down, and it is `deferred`, so it is not clipped by the scrolling pane. Only the
/// window that opened it renders it: the same property rows are built for the
/// settings card and for the Dev Tools window.
fn row_popup_overlay(
    shell: &mut Shell,
    window: &Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let popup = shell.row_popup.as_ref()?;
    if popup.window != window.window_handle().window_id() {
        return None;
    }
    let key = popup.key.clone();
    let label = popup.label.clone();
    let value = popup.value.clone();
    let anchor = popup.anchor;
    let kind = popup.kind;
    let range = popup.range;
    let unit = popup.unit.clone();
    let choices = popup.choices.clone();
    let dialog_field = popup.dialog_field;
    let draft = popup.draft;

    let close = {
        let weak = cx.weak_entity();
        move |_event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |shell, cx| shell.close_row_popup(cx));
        }
    };

    let body: AnyElement = match kind {
        RowPopupKind::Color => {
            let weak = cx.weak_entity();
            let pick_key = key.clone();
            color::palette(
                SharedString::from(format!("color-{key}")),
                &value,
                move |hex, _event, _window, cx| {
                    let _ = weak.update(cx, |shell, cx| {
                        shell.commit_property(&pick_key, hex, cx);
                    });
                },
            )
            .into_any_element()
        }
        RowPopupKind::Choices => {
            let weak = cx.weak_entity();
            let pick_key = key.clone();
            form::choice_list(
                SharedString::from(format!("choose-{key}")),
                &choices,
                &value,
                move |choice, _event, _window, cx| {
                    let _ = weak.update(cx, |shell, cx| match dialog_field {
                        Some(index) => shell.set_dialog_choice(index, choice, cx),
                        None => shell.commit_property(&pick_key, choice, cx),
                    });
                },
            )
            .into_any_element()
        }
        RowPopupKind::Slider => {
            let Some(range) = range else {
                return None;
            };
            let current = draft.unwrap_or_else(|| row_number(&value).unwrap_or(range.min));
            let position = slider::fraction(current, range.min, range.max);
            let readout = if unit.is_empty() {
                slider::format_value(current)
            } else {
                format!("{} {unit}", slider::format_value(current))
            };

            let track_selector = format!("slider-{key}-track");
            let marker = canvas(
                move |_bounds, _window, _cx| {},
                move |bounds, (), window, _cx| {
                    let height = px(6.);
                    let track = Bounds::new(
                        point(bounds.left(), bounds.center().y - height / 2.0),
                        size(bounds.size.width, height),
                    );
                    window.paint_quad(fill_track(track));
                    let filled = bounds.size.width.0 * position;
                    if filled > 0.0 {
                        window.paint_quad(fill_filled(Bounds::new(
                            point(bounds.left(), bounds.center().y - height / 2.0),
                            size(px(filled), height),
                        )));
                    }
                    let thumb_x = bounds.left().0 + position * (bounds.size.width.0 - 12.0).max(0.0);
                    window.paint_quad(fill_thumb(Bounds::new(
                        point(px(thumb_x), bounds.center().y - px(6.)),
                        size(px(12.), px(12.)),
                    )));
                },
            )
            .size_full();

            let down = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
                    let x = event.position.x;
                    let _ = weak.update(cx, |shell, cx| {
                        if let Some(popup) = shell.row_popup.as_mut() {
                            if popup.key == drag_key {
                                if let Some(range) = popup.range {
                                    popup.dragging = true;
                                    popup.draft = Some(slider::value_at(
                                        track_fraction(popup, x),
                                        range.min,
                                        range.max,
                                    ));
                                }
                            }
                        }
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            };
            let moved = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |event: &MouseMoveEvent, _window: &mut Window, cx: &mut App| {
                    if !event.dragging() {
                        return;
                    }
                    let x = event.position.x;
                    let _ = weak.update(cx, |shell, cx| {
                        if let Some(popup) = shell.row_popup.as_mut() {
                            if popup.key == drag_key && popup.dragging {
                                if let Some(range) = popup.range {
                                    popup.draft = Some(slider::value_at(
                                        track_fraction(popup, x),
                                        range.min,
                                        range.max,
                                    ));
                                    cx.notify();
                                }
                            }
                        }
                    });
                }
            };
            let commit = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |_event: &MouseUpEvent, _window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |shell, cx| {
                        if shell.row_popup.as_ref().is_some_and(|p| p.key == drag_key) {
                            if let Some(popup) = shell.row_popup.take() {
                                if let Some(draft) = popup.draft {
                                    shell.commit_property(
                                        &popup.key,
                                        &slider::format_value(draft),
                                        cx,
                                    );
                                }
                            }
                            cx.notify();
                        }
                    });
                }
            };
            let track = div()
                // The bounds listener has to sit on the plain `Div` (it is not on
                // `Stateful`), so the wrapper records its child's track rect.
                .on_children_prepainted({
                    let weak = cx.weak_entity();
                    let bounds_key = key.clone();
                    move |bounds, _window, cx| {
                        if let Some(rect) = bounds.first().copied() {
                            let _ = weak.update(cx, |shell, _cx| {
                                if let Some(popup) = shell.row_popup.as_mut() {
                                    if popup.key == bounds_key {
                                        popup.track = Some(rect);
                                    }
                                }
                            });
                        }
                    }
                })
                .child(
                    div()
                        .id(SharedString::from(track_selector.clone()))
                        .debug_selector(move || track_selector.clone())
                        .relative()
                        .h(px(20.))
                        .w_full()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, down)
                        .on_mouse_move(moved)
                        .on_mouse_up(MouseButton::Left, commit.clone())
                        .on_mouse_up_out(MouseButton::Left, commit)
                        .child(marker),
                );

            let ends = div()
                .flex()
                .flex_row()
                .justify_between()
                .text_color(rgb(MUTED))
                .child(slider::format_value(range.min))
                .child(slider::format_value(range.max));

            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_2()
                        .text_color(rgb(FG))
                        .child(label)
                        .child(div().text_color(rgb(MUTED)).child(readout)),
                )
                .child(track)
                .child(ends)
                .into_any_element()
        }
    };

    let panel = div()
        .id("row-popup")
        .debug_selector(|| "row-popup".to_string())
        .w(px(232.))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(rgba(WHEEL_GLASS))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .occlude()
        .text_xs()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_mouse_down_out(close)
        .child(body);

    Some(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(anchor.x, anchor.y + px(6.)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        // Above the settings card (priority 4): the palette and the slider are
        // opened *from* the card and must paint over it.
        .with_priority(5)
        .into_any_element(),
    )
}

/// The slider track's painted background, fill and thumb.
fn fill_track(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(PANEL_ALT)).corner_radii(px(3.))
}

fn fill_filled(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(ACCENT)).corner_radii(px(3.))
}

fn fill_thumb(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(0x48e5ff)).corner_radii(px(6.))
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
/// It has no backdrop, renders its own `Settings/<Feature>` rows (not the current
/// selection's), and is dismissed with its `✕` or `Esc` — never by a selection
/// change. The position is window coordinates so it survives a drag across the
/// canvas.
fn settings_card_view(
    shell: &mut Shell,
    window: &Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (position, node, rows) = match shell.settings_card.as_ref() {
        Some(card) => (card.position, card.node.clone(), card.rows.clone()),
        None => (point(px(220.), px(96.)), String::new(), Vec::new()),
    };
    let title = settings_title(&node);

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

    let panel = {
        let scroll = shell.dev_scroll.settings.clone();
        properties_rows_panel(shell, Some(&node), &rows, window, &scroll, cx)
    };
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
        .child(
            // A **column** wrapper: as a row, the panel's height was its content
            // height, so the rows were laid out past the card and clipped with
            // nothing to scroll. In a column, `flex_1` constrains the panel to the
            // card's content height and the scroll area gets a real viewport.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(panel),
        );

    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .position(position)
            .snap_to_window_with_margin(px(8.))
            .child(card),
    )
    // Above the floating HUD and the wheel: the card is what the user is working
    // in, and it must not be covered by a transient overlay.
    .with_priority(4)
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
        .h_full()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "console-panel".to_string())
        .child(console.clone())
        .child(actions)
}

/// Section indices in the Dev Tools accordion, in render order.
///
/// The body list is built in this order; the tests name indices through these
/// constants, which is why the module allows dead code in a non-test build.
#[allow(dead_code)]
mod dev_section {
    pub const PROVIDERS: usize = 0;
    pub const GPU: usize = 1;
    pub const COMMANDS: usize = 2;
    pub const MODEL: usize = 3;
    pub const PROPERTIES: usize = 4;
    pub const CONSOLE: usize = 5;
}

/// The Dev Tools window's body: the whole command surface, the inspector, the property
/// editor and the console in one column (P0.5).
///
/// The column cannot hold every pane at once, so the panes are an accordion
/// ([`ferrocad_widgets::accordion`]): each keeps a header, and the layout gives the
/// remaining height to the open one. Whichever pane is open survives closing and
/// reopening the window (`Shell::dev_accordion`).
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
    // Each pane keeps its scrollbar handle on the shell; cloning one is an `Rc` bump,
    // and it keeps the mutable `shell` borrow out of the argument lists below.
    let providers_scroll = shell.dev_scroll.providers.clone();
    let gpu_scroll = shell.dev_scroll.gpu.clone();
    let commands_scroll = shell.dev_scroll.commands.clone();
    let model_scroll = shell.dev_scroll.model.clone();
    let properties_scroll = shell.dev_scroll.properties.clone();

    // Preferred heights are the old fixed pane sizes; they are only used to decide
    // whether the whole stack fits (it does not in the default window).
    let gpu = shell.gpu_setting().to_string();
    let command_count = model.commands.len();
    let sections = vec![
        AccordionSection::new("providers", "Providers", px(160.)),
        AccordionSection::new("gpu", "GPU adapter", px(180.)).with_summary(gpu),
        AccordionSection::new("commands", "Commands", px(260.))
            .with_summary(format!("{command_count}")),
        AccordionSection::new("model", "Model", px(200.)),
        AccordionSection::new("properties", "Properties", px(240.)),
        AccordionSection::new("console", "Python console", px(160.)),
    ];
    let bodies: Vec<AnyElement> = vec![
        crate::chrome::providers_panel(shell.provider_features(), &providers_scroll, cx)
            .into_any_element(),
        crate::chrome::gpu_panel(
            shell.gpu_setting(),
            shell.gpu_adapters(),
            in_use.as_ref(),
            &gpu_scroll,
            cx,
        )
        .into_any_element(),
        crate::chrome::command_list_panel(&model, &inactive, &commands_scroll, cx)
            .into_any_element(),
        tree_panel(&model, &model_scroll, cx).into_any_element(),
        properties_panel(shell, &model, window, &properties_scroll, cx).into_any_element(),
        console_panel(&shell.console, cx).into_any_element(),
    ];
    let available = ferrocad_widgets::window_chrome::content_height(window);
    let weak = cx.weak_entity();
    let on_toggle =
        move |index: usize, _event: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |shell, cx| {
                shell.dev_accordion.toggle(index);
                cx.notify();
            });
        };

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
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if this.handle_key(event, true, window, cx) {
                cx.stop_propagation();
            }
        }))
        .child(accordion(
            "dev",
            sections,
            bodies,
            shell.dev_accordion.clone(),
            available,
            on_toggle,
        ))
        // A row overlay (the colour palette, the slider or an enum dropdown) opened
        // from this window's property pane. It is a window-root child so it does not
        // depend on which pane is open; `row_popup_overlay` ignores other windows.
        .when_some(row_popup_overlay(shell, window, cx), |root, overlay| {
            root.child(overlay)
        })
        .into_any_element()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.snapshot();
        // The app menu shows the window's real state; the registry cannot know it.
        let dev_tools_open = self.dev_tools_open(cx);

        // A palette that closed hands the keyboard back before a dialog it may have
        // opened takes it (that run is deferred to the `focus_dialog` block below).
        if let Some(focus) = self.palette_restore_focus.take() {
            window.focus(&focus, cx);
        }
        // A dialog was opened outside a window context; focus its first field (or the
        // panel, when it has none) once rendered so typing starts immediately and
        // `Esc` still bubbles to the cancel path.
        if std::mem::take(&mut self.focus_dialog) {
            let first = self
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.inputs.iter().flatten().next())
                .cloned();
            match first {
                Some(input) => {
                    let handle = input.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
                None => window.focus(&self.dialog_focus, cx),
            }
        }
        // A form opens outside a window context too. Its own element is not
        // focusable, so the shell root takes the keyboard and routes Esc/Enter.
        if std::mem::take(&mut self.focus_form) {
            window.focus(&self.root_focus, cx);
        }
        // The palette was just opened (from either window): remember what held the
        // keyboard so closing can give it back, then move focus to the query field.
        // The palette lives in the main window, so a Ctrl+K pressed in Dev Tools
        // brings that window forward; otherwise the field would take focus while the
        // keystrokes kept going to the other window.
        if std::mem::take(&mut self.focus_palette) {
            let previous = window.focused(cx);
            if let Some(palette) = self.palette.as_mut() {
                palette.previous_focus = previous;
            }
            if let Some(palette) = self.palette.as_ref() {
                window.activate_window();
                let input = palette.input.read(cx).focus_handle(cx);
                window.focus(&input, cx);
            }
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
            // The shell only triggers: a right-press arms the sidecar at the pointer,
            // and the move/release are forwarded so it can hover, flick or latch. The
            // sidecar owns the gesture state and its overlay. The viewport does not
            // consume right-clicks.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.last_pointer = Some(event.position);
                this.hover_radial(event.position, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                if this.active_tool.is_none() && this.dialog.is_none() {
                    this.last_pointer = Some(event.position);
                    this.open_radial(event.position, cx);
                }
            }))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                this.release_radial(event.position, cx);
            }))
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
            .child(crate::chrome::action_dock(&model, cx))
            .child(self.wheel.clone().into_any_element());
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_key(event, false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            // The settings card is dragged from its header, but the shell root sees
            // the moves so a fast drag does not stop at the card edge.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                // The canvas records moves over the scene; the root also catches the
                // shell's own background, so `last_pointer` stays warm for a
                // keyboard-opened wheel. (Hover over the HUD is read from the
                // window's mouse position, which every element sees.)
                this.last_pointer = Some(event.position);
                this.drag_settings_card(event.position, event.pressed_button, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                this.end_settings_drag(cx);
            }))
            .track_focus(&self.root_focus)
            .focusable()
            .child(content);
        // The form dialog draws its own full-window backdrop and occludes the
        // workspace. Only one modal is rendered at a time, so no second backdrop
        // can stack on it.
        let body = if let Some(dialog) = self.form_dialog.clone() {
            body.child(dialog)
        } else if self.dialog.is_some() {
            body.child(dialog_overlay(self, window, cx))
        } else if self.palette.is_some() {
            body.child(palette_overlay(self, &model, cx))
        } else if self.settings_card.is_some() {
            body.child(settings_card_view(self, window, cx))
        } else {
            body
        };
        // A row overlay (the colour palette, the slider or an enum dropdown) is
        // anchored at its click and can belong to the canvas, the settings card or a
        // dialog, so it is a root child rather than part of a pane. Its priority
        // keeps it above the card and the dialog HUD.
        let body = match row_popup_overlay(self, window, cx) {
            Some(overlay) => body.child(overlay),
            None => body,
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

    /// Boot the shell from a known appearance state.
    ///
    /// Tests share one interpreter and one parameter store, and stored appearance is
    /// applied at boot now that providers no longer write visibility — a toggle left
    /// behind by an earlier test would otherwise decide what this test's window draws.
    /// Reset the three capability stores first; before the first boot in a process the
    /// facade is not importable, and there is nothing to reset.
    fn boot_shell() -> Arc<Mutex<ShellModel>> {
        let _ = crate::python::evaluate(
            "import FerroCAD\nFerroCAD.Grid.reset()\nFerroCAD.Axis.reset()\nFerroCAD.Gizmo.reset()\n",
        );
        Shell::boot().expect("boot shell")
    }

    /// A *stored* visibility switch reaches the first window.
    ///
    /// This is the payoff of "the feature owns the switch": a provider's `activate`
    /// no longer writes `enabled`, so the boot appearance query returns the user's
    /// values instead of a feature that document activation turned on (opening the
    /// sample document used to enable the grid, which is why the boot path stripped
    /// visibility for a while).
    #[test]
    fn a_stored_visibility_switch_reaches_the_first_window() {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // First boot puts the facade on `sys.path`, which the writes below need.
        let _ = boot_shell();
        let result = crate::python::evaluate(
            "import FerroCAD\n\
             FerroCAD.Grid.enabled = False\n\
             FerroCAD.Axis.enabled = False\n\
             FerroCAD.Gizmo.enabled = False\n",
        )
        .expect("the interpreter ran");
        assert!(result.ok, "{}", result.output);

        // Deliberately not `boot_shell`: this boot must see the switches just written.
        let model = Shell::boot().expect("boot shell");
        {
            let view = model.lock().unwrap().view.clone();
            assert!(!view.grid, "the stored grid switch reaches the model");
            assert!(
                !view.axis.enabled,
                "the stored axis switch reaches the model"
            );
            assert!(
                !view.gizmo.enabled,
                "the stored gizmo switch reaches the model"
            );
        }

        // Leave the shared interpreter on the built-in defaults for other tests.
        reset_grid_state();
        let _ = crate::python::evaluate("import FerroCAD\nFerroCAD.Gizmo.reset()\n");
    }

    /// S1 end-to-end, headless: boot the interpreter, read the sample document
    /// through it, round-trip the console, and render the shell.
    #[gpui::test]
    fn shell_boots_lists_the_sample_document_and_is_renderable(cx: &mut TestAppContext) {
        // Tests share one CPython instance (module-global state); serialize them.
        let _guard = crate::PYTHON_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let model = boot_shell();
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
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::CONSOLE);
        assert!(
            dev_cx.debug_bounds("console-text-thumb").is_some(),
            "the console text area carries a scrollbar"
        );

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

        let model = boot_shell();
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
        // The property editor is a collapsed pane now: expand it before typing, or the
        // field would exist but never be painted (and so never take focus).
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);

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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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

        // The dialog held focus, so Escape cancels and drops its token.
        cx.simulate_keystrokes("escape");
        assert!(
            cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
            "Escape cancelled the modal"
        );
    }

    /// `App_Move` asks for a floating presentation: the panel paints **without** the
    /// dimming backdrop, so the canvas and gizmo stay visible and interactive. The
    /// first field takes focus and `Tab` walks the delta fields.
    #[gpui::test]
    fn the_move_dialog_floats_without_a_backdrop(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        draw(&mut cx);

        assert!(cx.debug_bounds("dialog-panel").is_some(), "the HUD paints");
        assert!(
            cx.debug_bounds("dialog-backdrop").is_none(),
            "a float has no dimming backdrop"
        );
        assert!(cx.debug_bounds("dialog-accept").is_some());
        assert!(cx.debug_bounds("dialog-cancel").is_some());

        // The first delta field took focus; Tab moves to the second.
        let second = shell.read_with(&cx, |shell, cx| {
            shell
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.inputs.iter().flatten().nth(1))
                .map(|input| input.read(cx).focus_handle(cx))
        });
        let second = second.expect("a second field");
        cx.simulate_keystrokes("tab");
        assert!(
            cx.update(|window, _cx| second.is_focused(window)),
            "Tab moves focus to the next delta field"
        );
    }

    /// Seam B: while a field is focused the floating HUD is frozen, so typing a large
    /// delta cannot slide the inputs out from under the pointer. Releasing focus and
    /// moving away lets it track the anchor again.
    #[gpui::test]
    fn a_focused_floating_hud_freezes_in_place(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        draw(&mut cx);

        let viewport = cx.update(|window, _cx| window.viewport_size());
        let before = cx.debug_bounds("dialog-panel").expect("the HUD paints");
        let hud_size = before.size;
        let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
        // Bottom-left of the canvas: well away from the HUD's first position.
        let far = point(
            canvas.origin.x + px(8.0),
            canvas.origin.y + canvas.size.height - px(8.0),
        );

        // The first delta field holds focus, so the HUD does not chase the pointer.
        cx.simulate_mouse_move(far, None::<MouseButton>, gpui::Modifiers::default());
        draw(&mut cx);
        let frozen = cx.debug_bounds("dialog-panel").expect("the HUD paints");
        assert_eq!(
            frozen.origin, before.origin,
            "a focused HUD stays put while the pointer moves"
        );

        // Release focus and move again: the HUD follows the new anchor, clamped.
        let root_focus = cx.update(|_window, cx| shell.read(cx).root_focus.clone());
        cx.update(|window, cx| window.focus(&root_focus, cx));
        let elsewhere = point(canvas.origin.x + px(8.0), canvas.origin.y + px(8.0));
        cx.simulate_mouse_move(elsewhere, None::<MouseButton>, gpui::Modifiers::default());
        draw(&mut cx);
        let resumed = cx.debug_bounds("dialog-panel").expect("the HUD paints");
        assert_eq!(
            resumed.origin,
            place_hud(elsewhere, viewport, hud_size),
            "with focus released the HUD follows the pointer, clamped"
        );
        assert_ne!(resumed.origin, frozen.origin, "the HUD moved again");
    }

    /// Seam B: a floating HUD is clamped into the viewport safe area.
    #[test]
    fn a_floating_hud_is_clamped_into_the_safe_area() {
        let viewport = size(px(800.0), px(600.0));
        let hud = size(px(400.0), px(72.0));

        let origin = place_hud(point(px(100.0), px(100.0)), viewport, hud);
        assert_eq!(
            origin,
            point(px(124.0), px(188.0)),
            "below-right of the anchor"
        );

        let origin = place_hud(point(px(790.0), px(590.0)), viewport, hud);
        assert_eq!(
            origin,
            point(
                px(800.0 - 400.0 - HUD_MARGIN),
                px(600.0 - 72.0 - HUD_DOCK_CLEARANCE)
            ),
            "a corner anchor still leaves the margins and the dock clear"
        );

        let frozen = clamp_hud(point(px(-40.0), px(900.0)), viewport, hud);
        assert_eq!(
            frozen,
            point(px(HUD_MARGIN), px(600.0 - 72.0 - HUD_DOCK_CLEARANCE)),
            "a frozen origin is clamped without the offset"
        );

        let tiny = size(px(200.0), px(100.0));
        let origin = place_hud(point(px(50.0), px(50.0)), tiny, hud);
        assert_eq!(
            origin,
            point(px(HUD_MARGIN), px(HUD_MARGIN)),
            "a viewport smaller than the HUD still yields the margin"
        );
    }

    /// The text in a named dialog field (empty when the field is absent).
    fn dialog_field(shell: &Shell, cx: &App, name: &str) -> String {
        let Some(dialog) = shell.dialog.as_ref() else {
            return String::new();
        };
        let Some(index) = dialog
            .spec
            .fields
            .iter()
            .position(|field| field.name == name)
        else {
            return String::new();
        };
        dialog
            .inputs
            .get(index)
            .and_then(Option::as_ref)
            .map(|input| input.read(cx).text().to_string())
            .unwrap_or_default()
    }

    /// A deferred gizmo drag mirrors its accumulated delta into the Δ fields — and
    /// successive drags accumulate from the values already there.
    #[gpui::test]
    fn a_gizmo_drag_updates_the_move_fields(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        draw(&mut cx);

        let field = |cx: &VisualTestContext, name: &str| {
            shell.read_with(cx, |shell, cx| dialog_field(shell, cx, name))
        };
        assert_eq!(field(&cx, "dx"), "0.0");

        let preview = |phase, x: f64, y: f64, z: f64| TransformPreview {
            target_id: 7,
            phase,
            total_transform: ferrocad_types::Matrix4::translation(x, y, z),
        };
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0, 0.0, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0, 0.0, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::End, 5.0, 0.0, 0.0), cx);
            })
        });
        assert_eq!(field(&cx, "dx"), "5.00");
        assert_eq!(field(&cx, "dy"), "0.00");

        // The next drag starts from 5.00 and adds its own delta.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0, 0.0, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::Update, 2.0, 1.0, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::End, 2.0, 1.0, 0.0), cx);
            })
        });
        assert_eq!(field(&cx, "dx"), "7.00");
        assert_eq!(field(&cx, "dy"), "1.00");
    }

    /// Any committed interaction supersedes the pending Move: a height change closes
    /// the HUD (and spends its token) instead of leaving a stale delta on screen.
    #[gpui::test]
    fn a_competing_interaction_closes_the_move_hud(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        draw(&mut cx);
        assert!(shell.read_with(&cx, |shell, _| shell.dialog.is_some()));

        let height = InteractionCommit::new("toy_height", 0, "set").with_values([30.0]);
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.on_interaction(&height, cx)));
        assert!(
            shell.read_with(&cx, |shell, _| shell.dialog.is_none()),
            "a height commit closes the Move HUD"
        );
    }

    /// The form is scoped to its selection: clearing or replacing the selection closes
    /// it instead of leaving a pending delta for objects that are no longer selected.
    #[gpui::test]
    fn a_selection_change_closes_the_move_hud(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        draw(&mut cx);
        assert!(shell.read_with(&cx, |shell, _| shell.dialog.is_some()));

        // Clearing the selection closes it.
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.clear_selection(cx)));
        assert!(
            shell.read_with(&cx, |shell, _| shell.dialog.is_none()),
            "unselecting closes the Move HUD"
        );

        // Replacing it (a tree click) does too.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Params", cx);
                shell.run_command("App_Move", cx);
            })
        });
        assert!(shell.read_with(&cx, |shell, _| shell.dialog.is_some()));
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Derived", cx)));
        assert!(
            shell.read_with(&cx, |shell, _| shell.dialog.is_none()),
            "a new selection closes the Move HUD"
        );
    }

    /// A small inline form for the shell-hosting tests: a root, one checkbox and
    /// the footer every [`FormDialog`] draws.
    const INLINE_FORM: &str = r#"
<ui version="4.0">
 <class>InlineForm</class>
 <widget class="QWidget" name="Form">
  <layout class="QVBoxLayout" name="column">
   <item><widget class="QCheckBox" name="flag">
    <property name="text"><string>Flag</string></property>
   </widget></item>
  </layout>
 </widget>
</ui>
"#;

    fn inline_form() -> Form {
        Form::parse_str(INLINE_FORM).expect("inline form")
    }

    /// Q3b: the shell hosts a rendered [`FormDialog`] as its modal, above the
    /// workspace and with the form's stable selectors. Only one modal (and one
    /// backdrop) exists at a time, and `closeDialog` releases it.
    #[gpui::test]
    fn a_form_dialog_opens_over_the_shell(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, _model) = palette_window(cx);

        let opened = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
        });
        assert!(opened, "the form opened");
        draw(&mut cx);

        for selector in [
            "form-Inline",
            "form-Inline-flag",
            "form-Inline-panel",
            "form-Inline-accept",
            "form-Inline-reject",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
        assert!(
            cx.debug_bounds("dialog-backdrop").is_none(),
            "the form draws the only backdrop"
        );
        assert!(
            cx.debug_bounds("palette").is_none(),
            "no palette is drawn behind the form"
        );

        // The command dialog and the palette both refuse while the form owns the
        // shell, so a second modal cannot stack on the first.
        let second = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.open_form(inline_form(), "Other", cx)
            })
        });
        assert!(!second, "one modal at a time");
        // `closeDialog` hides it and records no result.
        let closed = cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.close_form(cx)));
        assert!(closed, "close_form released the open form");
        assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
        assert_eq!(
            shell.read_with(&cx, |shell, _| shell.last_form_result()),
            None,
            "close records no result"
        );
        draw(&mut cx);
        assert!(cx.debug_bounds("form-Inline").is_none());
    }

    /// Q3b: Esc rejects and Enter accepts the open form; both are routed through
    /// the shell's key path because the form itself is not focusable.
    #[gpui::test]
    fn escape_rejects_and_enter_accepts_the_open_form(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, _model) = palette_window(cx);

        let opened = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
        });
        assert!(opened);
        draw(&mut cx);

        cx.simulate_keystrokes("escape");
        draw(&mut cx);
        assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
        assert_eq!(
            shell.read_with(&cx, |shell, _| shell.last_form_result()),
            Some(FormResult::Rejected)
        );
        assert!(cx.debug_bounds("form-Inline").is_none());

        let opened = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
        });
        assert!(opened, "a closed form can be reopened");
        draw(&mut cx);
        cx.simulate_keystrokes("enter");
        draw(&mut cx);
        assert!(!shell.read_with(&cx, |shell, _| shell.form_open()));
        assert_eq!(
            shell.read_with(&cx, |shell, _| shell.last_form_result()),
            Some(FormResult::Accepted)
        );
        assert!(cx.debug_bounds("form-Inline").is_none());
    }

    /// Q3b: a registry accelerator must not fire over the form modal, even though
    /// it would run when the shell root has focus normally.
    #[gpui::test]
    fn a_command_accelerator_does_not_fire_over_a_form(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, model) = palette_window(cx);

        let script = r#"
import FreeCADGui
class _FormShortcut:
    def GetResources(self):
        return {"MenuText": "Form shortcut", "Accel": "Ctrl+Alt+Shift+Y"}
    def Activated(self):
        return {"status": "shortcut ran over the form"}
if "Test_FormShortcut" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_FormShortcut", _FormShortcut())
"#;
        let registered = cx.update(|_window, _cx| crate::python::evaluate(script));
        assert!(registered.unwrap().ok);
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

        let opened = cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.open_form(inline_form(), "Inline", cx))
        });
        assert!(opened);
        draw(&mut cx);
        cx.simulate_keystrokes("ctrl-alt-shift-y");
        draw(&mut cx);

        let status = model.lock().unwrap().status.clone();
        assert_ne!(status, "shortcut ran over the form");
        assert!(
            shell.read_with(&cx, |shell, _| shell.form_open()),
            "the form still owns the shell"
        );
        assert!(cx.debug_bounds("form-Inline").is_some());
    }

    /// A4: a registered accelerator runs the same command path as a click, even
    /// while a text field has focus (the chord is unhandled there and bubbles).
    #[gpui::test]
    fn a_shortcut_runs_the_command(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::CONSOLE);
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let _model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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

    /// Show one Dev Tools pane and close the others.
    ///
    /// The panes share the remaining height, so a test that reads one pane's
    /// selectors narrows the set first. Multi-open state has its own test.
    fn show_dev_section(
        cx: &mut VisualTestContext,
        shell: &Entity<Shell>,
        dev_cx: &mut VisualTestContext,
        section: usize,
    ) {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.dev_accordion.open_only(section);
                cx.notify();
            })
        });
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// P0.5/D3: the main window keeps the frame, the viewport and the status line.
    /// The panels are in the second window, not merely hidden.
    #[gpui::test]
    fn the_main_window_carries_only_the_viewport_and_status(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
        let model = boot_shell();
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
        // The window is an accordion: Commands, Model and Properties start open and
        // share the height; the rest are headers only.
        assert!(dev_cx.debug_bounds("dev-command-list").is_some());
        for selector in [
            "dev-header-providers",
            "dev-header-gpu",
            "dev-header-commands",
            "dev-header-model",
            "dev-header-properties",
            "dev-header-console",
        ] {
            assert!(
                dev_cx.debug_bounds(selector).is_some(),
                "{selector} is a header"
            );
        }
        assert!(
            dev_cx.debug_bounds("tree-panel").is_some(),
            "the model tree is open beside the commands"
        );
        assert!(
            dev_cx.debug_bounds("properties-panel").is_some(),
            "the property editor is open beside the model tree"
        );
        assert!(dev_cx.debug_bounds("console-panel").is_none());
        assert!(
            dev_cx.debug_bounds("dev-command-App_Delete").is_some(),
            "a disabled command is still listed"
        );

        // Toggling a header adds a pane without closing the others.
        click_at(&mut dev_cx, "dev-header-console");
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(dev_cx.debug_bounds("console-panel").is_some());
        assert!(
            dev_cx.debug_bounds("dev-command-list").is_some(),
            "opening one pane leaves the others open"
        );

        // Narrowing the set is still available when a pane needs the height.
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);
        assert!(dev_cx.debug_bounds("properties-panel").is_some());
        assert!(
            dev_cx.debug_bounds("dev-properties-thumb").is_some(),
            "the property pane carries the scrollbar gpui never paints"
        );
        assert!(
            dev_cx.debug_bounds("dev-command-list").is_none(),
            "open_only closed the other panes"
        );
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::COMMANDS);
        assert!(dev_cx.debug_bounds("dev-command-list").is_some());
        assert!(dev_cx.debug_bounds("dev-commands-thumb").is_some());

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

    /// Clicking a header toggles only that pane; the other open panes keep their
    /// bodies. A second click on the same header closes it again.
    #[gpui::test]
    fn a_dev_tools_header_click_toggles_only_its_pane(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        assert!(dev_cx.debug_bounds("dev-command-list").is_some());

        click_at(&mut dev_cx, "dev-header-providers");
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            dev_cx.debug_bounds("dev-providers").is_some(),
            "the clicked header opens its pane"
        );
        assert!(
            dev_cx.debug_bounds("dev-command-list").is_some(),
            "the other open panes stay open"
        );
        assert!(
            cx.update(|_window, cx| shell.read(cx).dev_accordion.is_open(dev_section::PROVIDERS)),
            "the click reached the shell's state"
        );

        click_at(&mut dev_cx, "dev-header-providers");
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            dev_cx.debug_bounds("dev-providers").is_none(),
            "clicking the open header collapses it"
        );
        assert!(
            dev_cx.debug_bounds("dev-header-providers").is_some(),
            "the header itself stays"
        );
        assert!(
            dev_cx.debug_bounds("dev-command-list").is_some(),
            "closing one pane leaves the others open"
        );
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
        let model = boot_shell();
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
        let model = boot_shell();
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

        // The providers pane is collapsed by default; its header opens it.
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROVIDERS);
        assert!(
            dev_cx.debug_bounds("dev-providers").is_some(),
            "the providers panel is in the Dev Tools window"
        );
        assert!(
            dev_cx.debug_bounds("dev-providers-thumb").is_some(),
            "the pane carries a scrollbar"
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
        let model = boot_shell();
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

        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::GPU);
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let _model = boot_shell();

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
        assert_eq!(find("App_Delete").surface(), Surface::Radial);
        assert_eq!(find("App_Move").surface(), Surface::Radial);
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
        let model = boot_shell();
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
            cx.debug_bounds("grid-hairline-2").is_some(),
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
        let model = boot_shell();
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

    /// The snap filter set is spatial ambient state, so it lives beside Grid/Axes/WP
    /// in the header, not in the action dock. The label is the global switch and the
    /// popover is the six-mode checklist with de-noised labels.
    #[gpui::test]
    fn the_snap_control_lives_in_the_header_and_not_the_dock(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        // The control is the last item in the spatial pill, after WP.
        let header = cx.debug_bounds("viewport-header").expect("the control pill");
        let wp = cx.debug_bounds("wp-label").expect("the WP label");
        let snap = cx.debug_bounds("snap-label").expect("the Snap label");
        assert!(wp.origin.x < snap.origin.x, "Snap follows WP");
        assert!(
            header.contains(&snap.center()),
            "the pill owns the Snap control"
        );
        assert!(
            cx.debug_bounds("snap-state-dot").is_some(),
            "the Snap control carries a persistent on/off dot"
        );

        // The six modes are on the snap surface and parented to the switch, so the
        // dock does not render them any more.
        let command_ids: Vec<String> = {
            let model = model.lock().unwrap();
            model.commands.iter().map(|command| command.id.clone()).collect()
        };
        for id in [
            "Draft_Snap_Lock",
            "Draft_Snap_Grid",
            "Draft_Snap_Endpoint",
            "Draft_Snap_Midpoint",
            "Draft_Snap_Center",
            "Draft_Snap_Origin",
            "Draft_Snap_Vertex",
        ] {
            assert!(
                command_ids.iter().any(|known| known.as_str() == id),
                "{id} is registered"
            );
        }
        {
            let model = model.lock().unwrap();
            let lock = crate::commands::find(&model.commands, "Draft_Snap_Lock")
                .expect("the lock command");
            assert_eq!(lock.surface(), crate::commands::Surface::Snap);
            let grid_mode = crate::commands::find(&model.commands, "Draft_Snap_Grid")
                .expect("the Grid mode");
            assert_eq!(grid_mode.surface(), crate::commands::Surface::Snap);
            assert_eq!(grid_mode.parent(), Some("Draft_Snap_Lock"));
            assert_eq!(grid_mode.short_label(), "Grid");
        }
        assert!(
            cx.debug_bounds("dock-Draft_Snap_Grid").is_none(),
            "the snap modes left the action dock"
        );
        assert!(
            cx.debug_bounds("dock-App_Delete").is_none(),
            "the selection verbs are radial slots, not dock rows"
        );
        assert!(
            cx.debug_bounds("dock-search").is_some(),
            "the dock still carries the global search slot"
        );

        // Opening the caret does not toggle the switch.
        fn snapping_enabled() -> bool {
            let result = crate::python::evaluate(
                "import FreeCADGui\nprint(FreeCADGui.Snapper.isEnabled())",
            )
            .expect("read the snap switch");
            assert!(result.ok, "{}", result.output);
            result.output.trim() == "True"
        }
        let before = snapping_enabled();
        click_at(&mut cx, "snap-caret");
        assert!(cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
        assert_eq!(snapping_enabled(), before, "the caret ran no command");

        // A checkable row toggles in place: the menu stays open and the tick follows.
        fn mode_enabled(mode: &str) -> bool {
            let result = crate::python::evaluate(&format!(
                "import FreeCADGui\nprint(FreeCADGui.Snapper.isEnabled({mode:?}))"
            ))
            .expect("read the snap mode");
            assert!(result.ok, "{}", result.output);
            result.output.trim() == "True"
        }
        draw(&mut cx);
        // Every mode row carries the pill's left-aligned status dot; the old
        // right-aligned checkmark is gone.
        for dot in [
            "snap-row-Draft_Snap_Grid-dot",
            "snap-row-Draft_Snap_Endpoint-dot",
            "snap-row-Draft_Snap_Midpoint-dot",
            "snap-row-Draft_Snap_Center-dot",
            "snap-row-Draft_Snap_Origin-dot",
            "snap-row-Draft_Snap_Vertex-dot",
        ] {
            assert!(cx.debug_bounds(dot).is_some(), "{dot} exists");
        }
        assert!(
            cx.debug_bounds("snap-row-Draft_Snap_Grid").is_some(),
            "the Grid mode is a row"
        );
        let grid_mode = mode_enabled("Grid");
        click_at(&mut cx, "snap-row-Draft_Snap_Grid");
        assert!(
            cx.update(|_window, cx| shell.read(cx).popover_open("snap")),
            "toggling a checkable row keeps the menu open"
        );
        assert_eq!(mode_enabled("Grid"), !grid_mode, "the tick follows the mask");
        click_at(&mut cx, "snap-row-Draft_Snap_Grid");
        assert_eq!(mode_enabled("Grid"), grid_mode, "and back");

        // Closing stays explicit: Esc dismisses the menu and runs no command.
        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("escape");
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
        assert_eq!(snapping_enabled(), before, "Esc ran no command");

        // The label is the global switch, and each click flips it.
        click_at(&mut cx, "snap-label");
        assert_eq!(snapping_enabled(), !before);
        assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
        click_at(&mut cx, "snap-label");
        assert_eq!(snapping_enabled(), before, "and back");
    }

    /// A row that is not a toggle is a one-shot action, so it still dismisses the
    /// menu (the checkable-row exception must not leak into actions).
    #[gpui::test]
    fn a_one_shot_popover_row_still_closes_the_menu(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
        assert!(cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
        assert!(
            cx.debug_bounds("grid-row-App_ReloadGridSettings").is_some(),
            "the reload row is on the grid surface"
        );
        // A provider row is a toggle (it shows the active backend); a one-shot row
        // has no status dot.
        assert!(
            cx.debug_bounds("grid-row-ferrocad-provider:grid:ferrocad-dot")
                .is_some(),
            "the provider row carries a status dot"
        );
        assert!(
            cx.debug_bounds("grid-row-App_ReloadGridSettings-dot")
                .is_none(),
            "a one-shot action has no dot"
        );

        click_at(&mut cx, "grid-row-App_ReloadGridSettings");
        assert!(
            !cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "a one-shot row dismisses the menu"
        );
    }

    /// The canvas owns the full pane: there is no full-width header row above it, and
    /// the environment controls float as a compact pill over its top-left corner.
    #[gpui::test]
    fn the_canvas_starts_at_the_pane_top_and_the_controls_float_over_it(
        cx: &mut TestAppContext,
    ) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
        let model = boot_shell();
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
        // A settings command is an **action**, not a checkable state row (the
        // provider rows above it carry the active dot).
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
                !checked("GridOverlay_Settings"),
                "the settings row is an action, not a checked mode"
            );
            assert!(!checked("GridOverlayCoin_Settings"));
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
        let model = boot_shell();
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

        // The feature owns the switch: turn the grid on explicitly. A provider hook
        // selects an implementation and claims the drawing; it must not change
        // visibility, so the overlay assertion below is about drawing, not enabling.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
        });
        draw(&mut cx);
        assert!(
            model.lock().unwrap().view.grid,
            "the grid is on before the provider switch"
        );

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
            cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
            "a provider choice is a state change, so the popover stays open"
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
                "the provider's overlay claim was applied"
            );
            assert!(
                m.view.grid,
                "switching a provider must not change the visibility switch"
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
            cx.update(|_window, cx| shell.read(cx).popover_open("axis")),
            "the axis provider is a state row too, so the popover stays open"
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
    /// A settings entry in a header menu is an **action** row: clicking it closes
    /// the menu, opens the card for that feature, and leaves the selection alone.
    #[gpui::test]
    fn a_settings_menu_row_closes_the_popover(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
            shell.update(cx, |shell, cx| shell.select("Params", cx))
        });
        draw(&mut cx);

        click_at(&mut cx, "gizmo-caret");
        draw(&mut cx);
        assert!(cx.debug_bounds("gizmo-popover").is_some());

        let settings_row = if cx
            .debug_bounds("gizmo-row-GizmoOverlay_Settings")
            .is_some()
        {
            "gizmo-row-GizmoOverlay_Settings"
        } else {
            "gizmo-row-GizmoOverlayShader_Settings"
        };
        click_at(&mut cx, settings_row);
        draw(&mut cx);

        assert!(
            cx.debug_bounds("gizmo-popover").is_none(),
            "choosing a settings entry closes the menu"
        );
        let node = cx.update(|_window, cx| {
            shell
                .read(cx)
                .settings_card
                .as_ref()
                .map(|card| card.node.clone())
        });
        assert_eq!(
            node.as_deref(),
            Some("Settings/Gizmo"),
            "the card opened for the gizmo"
        );
        let selected = model.lock().unwrap().selected.clone();
        assert_eq!(
            selected.as_deref(),
            Some("Params"),
            "the object stayed selected"
        );
    }

    /// `View_GridSettings` opens the floating card **without** changing the
    /// selection: a settings panel is a tool, so the edited object keeps its
    /// selection and its gizmo.
    #[gpui::test]
    fn view_grid_settings_opens_the_card_without_selecting(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
            shell.update(cx, |shell, cx| shell.select("Params", cx))
        });

        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
        });

        {
            let m = model.lock().unwrap();
            assert_eq!(
                m.selected.as_deref(),
                Some("Params"),
                "opening settings keeps the object selected"
            );
            assert!(
                m.selection.iter().any(|item| item.is("Params")),
                "the selection list is untouched"
            );
            assert!(
                m.status.contains("Params"),
                "no settings node replaced the status: {}",
                m.status
            );
        }
        let (node, rows) = cx.update(|_window, cx| {
            let shell = shell.read(cx);
            let card = shell.settings_card.as_ref().expect("the settings card");
            (card.node.clone(), card.rows.clone())
        });
        assert_eq!(node, "Settings/Grid", "the card edits the requested node");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(names.contains(&"provider"), "rows: {names:?}");
        assert!(names.contains(&"minor"), "rows: {names:?}");
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
            cx.debug_bounds("prop-enum-Settings/Grid.provider-select")
                .is_some(),
            "the provider row is a select box"
        );
        assert!(
            cx.debug_bounds("prop-bool-Settings/Grid.enabled-toggle")
                .is_some(),
            "the enabled row is a toggle switch"
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

        // Selecting another object does not close the panel: it is dismissed only
        // by its own ✕ or Esc.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.select("Derived", cx))
        });
        assert!(
            cx.update(|_window, cx| shell.read(cx).settings_card.is_some()),
            "a selection change leaves the settings panel open"
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

    /// The settings rows are *painted*, not merely built: both the card and the Dev
    /// Tools property pane go through `properties_panel`, and the field elements must
    /// have real bounds inside the scroll area.
    #[gpui::test]
    fn the_settings_rows_are_painted(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        let area = cx
            .debug_bounds("dev-properties-area")
            .expect("the settings card's scroll area is painted");
        assert!(area.size.height > px(0.), "area bounds {area:?}");
        let field = cx
            .debug_bounds("prop-Settings/Grid.provider")
            .expect("the provider field is painted");
        assert!(
            field.size.width > px(0.) && field.size.height > px(0.),
            "field bounds {field:?}"
        );

        // The same pane in Dev Tools must not collapse either. It is selection
        // driven, so select the settings node there explicitly.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.select("Settings/Grid", cx))
        });
        let mut dev_cx = open_dev_tools(&mut cx, &shell);
        show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);
        let area = dev_cx
            .debug_bounds("dev-properties-area")
            .expect("the Dev Tools properties panes is painted");
        assert!(area.size.height > px(0.), "Dev Tools area bounds {area:?}");
        let field = dev_cx
            .debug_bounds("prop-Settings/Grid.provider")
            .expect("the Dev Tools settings field is painted");
        assert!(
            field.size.height > px(0.),
            "Dev Tools field bounds {field:?}"
        );

        reset_grid_state();
    }

    /// A colour row shows a swatch; clicking it expands the palette, and a pick
    /// commits through the same property path as a typed hex.
    #[gpui::test]
    fn a_colour_setting_opens_the_palette_and_commits(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        assert!(
            cx.debug_bounds("color-Settings/Grid.color-swatch")
                .is_some(),
            "the colour row has a swatch"
        );
        assert!(
            cx.debug_bounds("color-Settings/Grid.color-palette")
                .is_none()
        );

        click_at(&mut cx, "color-Settings/Grid.color-swatch");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("color-Settings/Grid.color-palette")
                .is_some(),
            "the swatch opens the palette"
        );

        click_at(&mut cx, "color-Settings/Grid.color-pick-ef4444");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("color-Settings/Grid.color-palette")
                .is_none(),
            "picking closes the palette"
        );
        let stored = cx.update(|_window, cx| {
            shell.read(cx).settings_card.as_ref().and_then(|card| {
                card.rows
                    .iter()
                    .find(|row| row.name == "color")
                    .map(|row| row.value.clone())
            })
        });
        assert_eq!(stored.as_deref(), Some("#ef4444"), "the pick was committed");

        reset_grid_state();
    }

    /// A bounded numeric setting opens the slider overlay; the drag commits on
    /// release. The mapping is logarithmic, so 75 % of 8..400 is well above the
    /// arithmetic midpoint.
    #[gpui::test]
    fn a_bounded_setting_opens_the_slider_and_commits(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
            shell.update(cx, |shell, cx| shell.select("Settings/Gizmo", cx))
        });
        draw(&mut cx);

        assert!(
            cx.debug_bounds("slider-Settings/Gizmo.size-trigger")
                .is_some(),
            "the bounded row has a slider trigger"
        );
        click_at(&mut cx, "slider-Settings/Gizmo.size-trigger");
        draw(&mut cx);

        let track = cx
            .debug_bounds("slider-Settings/Gizmo.size-track")
            .expect("the slider overlay is rendered");
        let at = point(
            track.left() + track.size.width * 0.75,
            track.center().y,
        );
        cx.simulate_mouse_down(at, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, gpui::Modifiers::default());
        draw(&mut cx);

        assert!(
            cx.debug_bounds("row-popup").is_none(),
            "the slider closes on release"
        );
        let stored = model
            .lock()
            .unwrap()
            .properties
            .iter()
            .find(|row| row.name == "size")
            .map(|row| row.value.clone())
            .expect("the size row");
        let size: f64 = stored
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .parse()
            .expect("a number");
        assert!(
            size > 100.0 && size < 400.0,
            "75 % of 8..400 on a log scale, got {stored:?}"
        );

        // Leave the shared interpreter on its default for the other tests.
        let _ = crate::python::evaluate("import FerroCAD\nFerroCAD.Gizmo.reset()");
    }

    /// A command dialog renders a bool as a toggle switch and an enum as a select
    /// box whose dropdown updates the dialog's pending value.
    #[gpui::test]
    fn a_dialog_uses_the_toggle_and_select_box(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
class _TestForm:
    def GetResources(self):
        return {"MenuText": "Form", "Group": "Edit", "CmdType": "ForEdit"}
    def IsActive(self):
        return True
    def Activated(self):
        return {
            "title": "Test form",
            "fields": [
                {"name": "on", "label": "On", "kind": "bool", "value": False},
                {"name": "mode", "label": "Mode", "kind": "enum",
                 "values": ["fast", "safe"], "value": "safe"},
            ],
            "on_accept": lambda values: {"status": "form ok"},
        }
if "Test_Form" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Form", _TestForm())
"#;
        assert!(crate::python::evaluate(script).unwrap().ok);
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.refresh_commands(cx);
                shell.run_command("Test_Form", cx);
            })
        });
        draw(&mut cx);

        assert!(
            cx.debug_bounds("dialog-bool-0-toggle").is_some(),
            "a bool field is a toggle"
        );
        assert!(
            cx.debug_bounds("dialog-enum-1-select").is_some(),
            "an enum field is a select box"
        );

        click_at(&mut cx, "dialog-bool-0-toggle");
        draw(&mut cx);
        let flag = cx.update(|_window, cx| {
            matches!(
                shell.read(cx).dialog.as_ref().and_then(|d| d.states.first()),
                Some(ValueState::Flag(true))
            )
        });
        assert!(flag, "the toggle flipped the dialog flag");

        click_at(&mut cx, "dialog-enum-1-select");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("choose-dialog-enum-1-choices").is_some(),
            "the dropdown lists the enum values"
        );
        click_at(&mut cx, "choose-dialog-enum-1-choice-0");
        draw(&mut cx);
        let choice = cx.update(|_window, cx| {
            match shell.read(cx).dialog.as_ref().and_then(|d| d.states.get(1)) {
                Some(ValueState::Choice(choice)) => Some(*choice),
                _ => None,
            }
        });
        assert_eq!(choice, Some(0), "the dropdown set the dialog choice");
    }

    /// A boolean setting is a toggle switch; a click commits the opposite value.
    #[gpui::test]
    fn a_bool_row_is_a_toggle_that_commits(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        let before = shell.read_with(&cx, |shell, _| {
            shell.settings_card.as_ref().and_then(|card| {
                card.rows
                    .iter()
                    .find(|row| row.name == "enabled")
                    .map(|row| row.value.clone())
            })
        });
        assert_eq!(before.as_deref(), Some("False"), "the grid starts off");
        assert!(
            cx.debug_bounds("prop-bool-Settings/Grid.enabled-toggle")
                .is_some(),
            "a bool row is a toggle"
        );

        click_at(&mut cx, "prop-bool-Settings/Grid.enabled-toggle");
        draw(&mut cx);
        let after = shell.read_with(&cx, |shell, _| {
            shell.settings_card.as_ref().and_then(|card| {
                card.rows
                    .iter()
                    .find(|row| row.name == "enabled")
                    .map(|row| row.value.clone())
            })
        });
        assert_eq!(after.as_deref(), Some("True"), "the toggle committed true");

        reset_grid_state();
    }

    /// An enumeration setting is a select box; a pick commits through the same
    /// property path as typing.
    #[gpui::test]
    fn an_enum_row_is_a_select_box_that_commits(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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

        assert!(
            cx.debug_bounds("prop-enum-Settings/Grid.provider-select")
                .is_some(),
            "the provider row is a select box"
        );
        click_at(&mut cx, "prop-enum-Settings/Grid.provider-select");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("choose-Settings/Grid.provider-choices")
                .is_some(),
            "the dropdown lists the providers"
        );

        // The providers are listed in registration order: native first, Coin second.
        click_at(&mut cx, "choose-Settings/Grid.provider-choice-1");
        draw(&mut cx);
        let backend = cx.update(|_window, cx| {
            shell
                .read(cx)
                .grid_summary
                .as_ref()
                .and_then(|summary| summary.backend.clone())
        });
        assert_eq!(backend.as_deref(), Some("coin"), "the pick switched the provider");

        // Leave the shared registry on the native backend for the other tests.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("ferrocad-provider:grid:ferrocad", cx)
            })
        });
        reset_grid_state();
    }

    /// A settings pane with more rows than its height is scrollable: the rows must
    /// not be compressed into the viewport (which would clip them instead of
    /// overflowing), and a wheel over it must move the tracked handle.
    #[gpui::test]
    fn a_crowded_settings_pane_scrolls(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // The card's last row must be visible inside its scroll viewport; if it is
        // clipped, the rows were compressed instead of overflowing.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.select("Settings/Axis", cx))
        });
        draw(&mut cx);
        let area = cx
            .debug_bounds("dev-properties-area")
            .expect("the card's scroll area");
        let last = cx
            .debug_bounds("prop-command-App_ResetAxisAppearance")
            .expect("the card's last row");
        let card = cx.debug_bounds("settings-card").expect("the card");
        assert_eq!(card.size.height, px(420.), "the card keeps its height");
        assert!(
            area.size.height < card.size.height,
            "the viewport stays inside the card: card {:?}, area {:?}",
            card.size.height,
            area.size.height
        );
        let max = cx.update(|_window, cx| shell.read(cx).dev_scroll.settings.max_offset().y);
        assert!(
            max > px(0.),
            "the axis rows overflow the viewport (max {max:?})"
        );
        assert!(
            last.bottom() > area.bottom(),
            "the last row starts below the viewport: {:?} vs {:?}",
            last.bottom(),
            area.bottom()
        );

        // A wheel over the card scrolls it.
        cx.simulate_mouse_move(area.center(), None, gpui::Modifiers::default());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: area.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-80.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(&mut cx);
        let offset = cx.update(|_window, cx| shell.read(cx).dev_scroll.settings.offset().y);
        assert!(
            offset < px(0.),
            "the wheel over the card scrolled it (offset {offset:?})"
        );

        // The default Dev Tools layout keeps Commands, Model and Properties open,
        // so the property pane is short and its settings rows must overflow.
        let mut dev_cx = open_dev_tools(&mut cx, &shell);
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.select("Settings/Gizmo", cx))
        });
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let max = cx.update(|_window, cx| shell.read(cx).dev_scroll.properties.max_offset().y);
        assert!(
            max > px(0.),
            "the settings rows overflow the pane, so it is scrollable (max {max:?})"
        );

        let area = dev_cx
            .debug_bounds("dev-properties-area")
            .expect("the properties pane");
        dev_cx.simulate_mouse_move(area.center(), None, gpui::Modifiers::default());
        dev_cx.simulate_event(gpui::ScrollWheelEvent {
            position: area.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-40.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        dev_cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let offset = cx.update(|_window, cx| shell.read(cx).dev_scroll.properties.offset().y);
        assert!(
            offset < px(0.),
            "a wheel over the pane scrolled it (offset {offset:?})"
        );
    }

    /// P0.8: a click/focus inside the card must not fall through to the canvas and
    /// clear the selection (which would drop the card).
    #[gpui::test]
    fn clicking_a_settings_field_keeps_the_card_open(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
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
            assert!(
                m.selected.is_none(),
                "opening settings does not select a node"
            );
        }
        let rows = cx.update(|_window, cx| {
            shell
                .read(cx)
                .settings_card
                .as_ref()
                .map(|card| card.rows.clone())
                .unwrap_or_default()
        });
        assert!(
            rows.iter().any(|row| row.name == "minor"),
            "the settings rows are still loaded"
        );
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
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
            cx.debug_bounds("dock-App_Delete").is_none(),
            "object verbs left the dock for the radial menu"
        );
        assert!(
            cx.debug_bounds("dock-App_Move").is_none(),
            "object verbs left the dock for the radial menu"
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
        assert!(
            !dock_enabled(&model, "App_Delete"),
            "nothing is selected yet"
        );

        // A selection turns the radial verbs on. They are not dock rows any more, so
        // the proof is the wheel: open it and check both slots and their cardinal
        // placement (Move north, Delete south).
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
        draw(&mut cx);
        assert!(dock_enabled(&model, "App_Delete"));
        assert!(dock_enabled(&model, "App_Move"));
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.open_radial(point(px(400.), px(300.)), cx)
            });
        });
        draw(&mut cx);
        assert!(cx.debug_bounds("radial-wheel").is_some(), "the wheel opened");
        let move_slot = cx
            .debug_bounds("radial-label-App_Move")
            .expect("Move is a north slot");
        let delete_slot = cx
            .debug_bounds("radial-label-App_Delete")
            .expect("Delete is a south slot");
        assert!(
            move_slot.origin.y < delete_slot.origin.y,
            "Move is above Delete (north/south)"
        );
        assert!(
            (move_slot.center().x.0 - delete_slot.center().x.0).abs() < 2.0,
            "both sit on the vertical axis"
        );
        cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.close_radial(cx)));
        draw(&mut cx);
        assert!(cx.debug_bounds("radial-wheel").is_none(), "the wheel closed");

        // The Search slot opens the command palette.
        click_at(&mut cx, "dock-search");
        draw(&mut cx);
        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_some()),
            "the Search slot opens the palette"
        );
        assert!(cx.debug_bounds("palette").is_some());
        assert!(cx.debug_bounds("palette-input").is_some());
    }

    /// The in-situ gesture: a right-press opens the wheel at the pointer, a move
    /// highlights the wedge under it, and the release runs that slot. A release in
    /// the hollow centre cancels without running anything.
    #[gpui::test]
    fn the_radial_gesture_opens_hovers_and_runs(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
        // A safe verb so the gesture test never deletes the shared sample document.
        let script = r#"
import FreeCADGui
class _RadialProbe:
    def GetResources(self):
        return {
            "MenuText": "Radial probe",
            "FerroCAD_Surface": "radial",
            "FerroCAD_Tone": "structural",
        }
    def Activated(self):
        return {"status": "radial probe ran"}
if "Test_RadialProbe" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_RadialProbe", _RadialProbe())
"#;
        let registered = crate::python::evaluate(script).expect("register the probe");
        assert!(registered.ok, "{}", registered.output);

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
                shell.refresh_commands(cx);
                shell.select("Params", cx);
            });
        });
        draw(&mut cx);

        let wheel = shell.read_with(&cx, |shell, _| shell.wheel.clone());
        let anchor = point(px(420.), px(320.));
        cx.simulate_mouse_down(anchor, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(
            wheel.read_with(&cx, |wheel, _| wheel.is_open()),
            "the right-press opened the wheel"
        );
        let anchor = wheel.read_with(&cx, |wheel, _| wheel.anchor());
        let slots = wheel.read_with(&cx, |wheel, _| wheel.slots().to_vec());
        let index = slots
            .iter()
            .position(|slot| slot.id.as_ref() == "Test_RadialProbe")
            .expect("the probe is a radial slot");
        assert!(
            cx.debug_bounds("radial-wheel").is_some(),
            "the wheel is painted"
        );
        assert!(
            cx.debug_bounds("radial-label-Test_RadialProbe").is_some(),
            "the probe has a label"
        );

        let offset = ferrocad_widgets::wheel::slot_offset(index, slots.len());
        let over_slot = point(anchor.x + offset.x, anchor.y + offset.y);
        cx.simulate_mouse_move(
            over_slot,
            MouseButton::Right,
            gpui::Modifiers::default(),
        );
        draw(&mut cx);
        assert_eq!(
            wheel.read_with(&cx, |wheel, _| wheel.hovered()),
            Some(index),
            "the pointer is over the probe wedge"
        );

        cx.simulate_mouse_up(over_slot, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(
            !wheel.read_with(&cx, |wheel, _| wheel.is_open()),
            "the release closes the wheel"
        );
        assert!(
            model.lock().unwrap().status.contains("radial probe ran"),
            "the released slot ran"
        );

        // A **trackpad tap** (quick down+up at the press) latches instead of
        // flashing closed. No command runs yet.
        model.lock().unwrap().status.clear();
        cx.simulate_mouse_down(anchor, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(wheel.read_with(&cx, |wheel, _| wheel.is_open()));
        cx.simulate_mouse_up(anchor, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(
            wheel.read_with(&cx, |wheel, _| wheel.is_latched()),
            "a quick tap latches the wheel"
        );
        assert!(!model.lock().unwrap().status.contains("radial probe ran"));

        // In the latched wheel a primary click commits the wedge (and does not
        // select the object under the overlay).
        cx.simulate_mouse_move(over_slot, None::<MouseButton>, gpui::Modifiers::default());
        draw(&mut cx);
        assert_eq!(
            wheel.read_with(&cx, |wheel, _| wheel.hovered()),
            Some(index)
        );
        cx.simulate_mouse_down(over_slot, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(over_slot, MouseButton::Left, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(!wheel.read_with(&cx, |wheel, _| wheel.is_open()));
        assert!(model.lock().unwrap().status.contains("radial probe ran"));

        // A long hold released in the deadzone is a cancel: no command, no status.
        model.lock().unwrap().status.clear();
        cx.simulate_mouse_down(anchor, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(wheel.read_with(&cx, |wheel, _| wheel.is_open()));
        std::thread::sleep(std::time::Duration::from_millis(220));
        cx.simulate_mouse_up(anchor, MouseButton::Right, gpui::Modifiers::default());
        draw(&mut cx);
        assert!(!wheel.read_with(&cx, |wheel, _| wheel.is_open()));
        assert!(!model.lock().unwrap().status.contains("radial probe ran"));

        let cleanup = crate::python::evaluate(
            "import FreeCADGui\nFreeCADGui.removeCommand('Test_RadialProbe')",
        );
        assert!(cleanup.expect("remove the probe").ok);
    }

    /// `Space` opens **latched** at the last pointer position, so a touchpad user can
    /// open with a key and then click a wedge; a second `Space` over a wedge commits.
    #[gpui::test]
    fn space_opens_a_latched_wheel_and_commits(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = boot_shell();
        let script = r#"
import FreeCADGui
class _SpaceProbe:
    def GetResources(self):
        return {
            "MenuText": "Space probe",
            "FerroCAD_Surface": "radial",
            "FerroCAD_Tone": "structural",
        }
    def Activated(self):
        return {"status": "space probe ran"}
if "Test_SpaceProbe" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_SpaceProbe", _SpaceProbe())
"#;
        let registered = crate::python::evaluate(script).expect("register the probe");
        assert!(registered.ok, "{}", registered.output);

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
                shell.refresh_commands(cx);
                shell.select("Params", cx);
            });
        });
        let anchor = point(px(360.), px(280.));
        cx.simulate_mouse_move(anchor, None::<MouseButton>, gpui::Modifiers::default());
        draw(&mut cx);

        let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("space");
        draw(&mut cx);
        let wheel = shell.read_with(&cx, |shell, _| shell.wheel.clone());
        assert!(
            wheel.read_with(&cx, |wheel, _| wheel.is_open()),
            "Space opened the wheel"
        );
        assert_eq!(
            wheel.read_with(&cx, |wheel, _| wheel.anchor()),
            anchor,
            "Space anchors at the last pointer"
        );
        assert!(
            wheel.read_with(&cx, |wheel, _| wheel.is_latched()),
            "a key opens latched"
        );

        let slots = wheel.read_with(&cx, |wheel, _| wheel.slots().to_vec());
        let index = slots
            .iter()
            .position(|slot| slot.id.as_ref() == "Test_SpaceProbe")
            .expect("the probe is a slot");
        let offset = ferrocad_widgets::wheel::slot_offset(index, slots.len());
        let over_slot = point(anchor.x + offset.x, anchor.y + offset.y);
        cx.simulate_mouse_move(over_slot, None::<MouseButton>, gpui::Modifiers::default());
        draw(&mut cx);
        assert_eq!(
            wheel.read_with(&cx, |wheel, _| wheel.hovered()),
            Some(index)
        );

        // Space over a hovered wedge commits it.
        cx.simulate_keystrokes("space");
        draw(&mut cx);
        assert!(!wheel.read_with(&cx, |wheel, _| wheel.is_open()));
        assert!(model.lock().unwrap().status.contains("space probe ran"));

        let cleanup = crate::python::evaluate(
            "import FreeCADGui\nFreeCADGui.removeCommand('Test_SpaceProbe')",
        );
        assert!(cleanup.expect("remove the probe").ok);
    }

    /// A headless shell window for the palette tests, already drawn once.
    fn palette_window(
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, Entity<Shell>, Arc<Mutex<ShellModel>>) {
        let model = boot_shell();
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut visual).unwrap();
        draw(&mut visual);
        (visual, shell, model)
    }

    /// Put the keyboard on the shell root and press `Ctrl+K`, then assert the palette
    /// opened. The key path is the same one a real window uses.
    fn open_palette_with_keyboard(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
        let root = shell.read_with(&*cx, |shell, _| shell.root_focus.clone());
        cx.update(|window, cx| window.focus(&root, cx));
        cx.simulate_keystrokes("ctrl-k");
        draw(cx);
        assert!(
            shell.read_with(&*cx, |shell, _| shell.palette.is_some()),
            "Ctrl+K opens the command palette"
        );
    }

    /// Ctrl+K opens the palette, and the query filters case-insensitively over the
    /// ids (and the labels/tooltips, via the same helper).
    #[gpui::test]
    fn ctrl_k_opens_the_palette_and_filters_the_commands(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, _model) = palette_window(cx);

        open_palette_with_keyboard(&mut cx, &shell);
        assert!(cx.debug_bounds("palette").is_some());
        assert!(cx.debug_bounds("palette-input").is_some());
        // Nothing is filtered yet, so the whole registry is listed, including a
        // disabled command (it is visible but greyed, like the dock).
        assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
        assert!(cx.debug_bounds("palette-row-App_Delete").is_some());

        // Case-insensitive label match: only the grid command survives.
        cx.simulate_input("GRID");
        draw(&mut cx);
        assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
        assert!(cx.debug_bounds("palette-row-App_Delete").is_none());
        assert!(cx.debug_bounds("palette-row-App_ToggleAxisCross").is_none());

        // The id is searched too, case-insensitively.
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("dev_toggletools");
        draw(&mut cx);
        assert!(cx.debug_bounds("palette-row-Dev_ToggleTools").is_some());
        assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_none());

        // A query with no matches shows the empty state instead of a blank panel.
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("zzzz-no-such-command");
        draw(&mut cx);
        assert!(cx.debug_bounds("palette-empty").is_some());
    }

    /// The key path is shared by both windows: Ctrl+K typed while Dev Tools has
    /// focus opens the same palette (drawn in the main window), and Esc there closes
    /// it too.
    #[gpui::test]
    fn ctrl_k_opens_the_palette_from_dev_tools(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, _model) = palette_window(cx);
        let mut dev_cx = open_dev_tools(&mut cx, &shell);

        dev_cx.simulate_keystrokes("ctrl-k");
        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_some()),
            "Ctrl+K in Dev Tools opens the shared palette"
        );
        draw(&mut cx);
        assert!(cx.debug_bounds("palette").is_some());

        dev_cx.simulate_keystrokes("escape");
        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_none()),
            "Esc in Dev Tools closes the palette"
        );
    }

    /// Down moves the cursor over the filtered rows and Enter runs the highlighted
    /// enabled command (the same `run_command` path a click uses).
    #[gpui::test]
    fn the_palette_arrows_move_the_selection_and_enter_runs_it(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, model) = palette_window(cx);
        reset_grid_state();

        open_palette_with_keyboard(&mut cx, &shell);
        // Registry order: App_ToggleGrid, App_ToggleAxisCross, Dev_ToggleTools.
        cx.simulate_input("toggle");
        draw(&mut cx);
        assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
        assert!(cx.debug_bounds("palette-row-App_ToggleAxisCross").is_some());

        let before = model.lock().unwrap().view.clone();
        cx.simulate_keystrokes("down");
        draw(&mut cx);
        cx.simulate_keystrokes("enter");
        draw(&mut cx);

        let after = model.lock().unwrap().view.clone();
        assert_eq!(before.grid, after.grid, "the first row (the grid) did not run");
        assert_ne!(
            before.axis_cross, after.axis_cross,
            "the selected row (the axis cross) ran"
        );
        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_none()),
            "running a command closes the palette"
        );
        assert!(cx.debug_bounds("palette").is_none());
        reset_grid_state();
    }

    /// Esc closes the palette and hands focus back to whatever held it when Ctrl+K
    /// opened it.
    #[gpui::test]
    fn escape_closes_the_palette_and_restores_focus(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, _model) = palette_window(cx);
        let root = shell.read_with(&cx, |shell, _| shell.root_focus.clone());

        open_palette_with_keyboard(&mut cx, &shell);
        cx.simulate_keystrokes("escape");
        draw(&mut cx);

        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_none()),
            "Esc closes the palette"
        );
        assert!(cx.debug_bounds("palette").is_none());
        let focused = cx.update(|window, cx| window.focused(cx));
        assert_eq!(focused, Some(root), "focus returns to the opener");
    }

    /// A disabled command stays listed, but Enter does not run it (and does not close
    /// the palette): `App_Delete` is inactive with nothing selected.
    #[gpui::test]
    fn a_disabled_palette_command_does_not_run(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut cx, shell, model) = palette_window(cx);

        open_palette_with_keyboard(&mut cx, &shell);
        cx.simulate_input("Delete");
        draw(&mut cx);
        assert!(cx.debug_bounds("palette-row-App_Delete").is_some());

        let objects_before: usize = model
            .lock()
            .unwrap()
            .tree
            .iter()
            .map(|doc| doc.objects.len())
            .sum();
        cx.simulate_keystrokes("enter");
        draw(&mut cx);

        let status = model.lock().unwrap().status.clone();
        assert!(status.contains("disabled"), "status: {status}");
        assert!(
            shell.read_with(&cx, |shell, _| shell.palette.is_some()),
            "a disabled command leaves the palette open"
        );
        let objects_after: usize = model
            .lock()
            .unwrap()
            .tree
            .iter()
            .map(|doc| doc.objects.len())
            .sum();
        assert_eq!(objects_before, objects_after, "the document is untouched");
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
        let model = boot_shell();
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
        let model = boot_shell();
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
        let model = boot_shell();
        let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

        cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                shell.run_command("App_ActivateWorkplane", cx)
            })
        });
        assert_eq!(model.lock().unwrap().status, "view: no viewport");
    }
}
