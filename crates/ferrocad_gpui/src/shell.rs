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
    DeclarativeManipulator, GizmoAxis, GizmoHandle, GizmoMode, GizmoValue, InteractionCommit,
    Manipulator, SnapKind, Stage, ToolStatus, TransformGizmo,
};
use ferrocad_types::Vector3;
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
use ferrocad_widgets::scrub;
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

mod devtools;
mod popovers;
mod palette;
mod viewport;
mod dispatch;
mod dialogs;
mod forms;
mod picking;
mod properties;


mod render;
use render::*;
// `dev_tools.rs` renders the body through the shell module path.
pub(crate) use render::dev_tools_body;

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

/// The key that escalates an armed gizmo into the Move HUD.
///
/// `Tab` opens the HUD with no typed character; a numeric key (`0-9`, `-`, `.`)
/// opens it and the character is inserted into the focused field. Chorded keys and
/// shifted symbols are not escalation: they belong to the shortcut/typing path.
fn escalation_key(event: &KeyDownEvent) -> Option<Option<char>> {
    let modifiers = &event.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    if event.keystroke.key == "tab" {
        return Some(None);
    }
    if modifiers.shift {
        return None;
    }
    // Prefer the typed character: a shifted digit's `key` is still its base digit
    // while `key_char` carries the symbol the user actually typed.
    let typed = match event.keystroke.key_char.as_deref() {
        Some(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(ch),
                _ => None,
            }
        }
        None => {
            let mut chars = event.keystroke.key.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(ch),
                _ => None,
            }
        }
    }?;
    matches!(typed, '0'..='9' | '-' | '.').then_some(Some(typed))
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

/// The status-line name of a pivot snap.
fn snap_name(kind: SnapKind) -> &'static str {
    match kind {
        SnapKind::Point => "surface",
        SnapKind::Grid => "grid",
        SnapKind::Vertex => "vertex",
        SnapKind::EdgeMid => "edge mid",
        SnapKind::Centre => "face centre",
        SnapKind::Origin => "origin",
    }
}

/// The translation a gizmo drag is already showing, used to seed the Move HUD when
/// escalation starts.
///
/// `axis` is the field the HUD should focus (0 = X, 1 = Y, 2 = Z); a plane drag
/// focuses X, matching the typed-field order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct GizmoSeed {
    pub axis: usize,
    pub delta: [f64; 3],
}

impl GizmoSeed {
    /// Seed from a world-space translation, deriving the axis from the component
    /// that moved (a plane or an unmoved drag focuses X).
    fn from_translation(delta: [f64; 3]) -> Self {
        let axis = match (delta[0] != 0.0, delta[1] != 0.0, delta[2] != 0.0) {
            (false, true, false) => 1,
            (false, false, true) => 2,
            _ => 0,
        };
        Self { axis, delta }
    }
}

/// The gizmo mode a command's `gizmo` directive names.
///
/// `translate`/`move`, `rotate` and `universal`/`both` are the accepted spellings;
/// an unknown one is reported by the caller rather than guessed.
fn gizmo_mode_named(name: &str) -> Option<GizmoMode> {
    match name.trim().to_ascii_lowercase().as_str() {
        "translate" | "move" => Some(GizmoMode::Translate),
        "rotate" => Some(GizmoMode::Rotate),
        "universal" | "both" => Some(GizmoMode::Universal),
        _ => None,
    }
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
    /// Which handles the transform gizmo shows. Session tool state, not a document
    /// setting: it survives re-selection and re-arming, and a `Move` resets it to
    /// translate because that command's fields describe a translation.
    gizmo_mode: GizmoMode,
    /// The session pivot the user dragged the origin handle to. It is applied instead of
    /// the object origin while set; it is not undoable and not a document property. A
    /// selection change or a wheel promotion clears it, and `Esc` restores the object
    /// origin.
    user_origin: Option<Vector3>,
    /// The anchored overlay a property row opened (a colour palette or a range
    /// slider). It is UI state only: the value itself lives in the parameter store.
    row_popup: Option<RowPopup>,
    /// The live vertical scrub from a property row's handle. It owns the scale
    /// overlay and the release commit; `None` when no handle is held.
    scrub: Option<ScrubDrag>,
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
    /// Whether the current gizmo drag is deferred to the Move HUD. This mirrors the
    /// viewport's `set_gizmo_preview` so the shell can answer Esc/cancel without a
    /// viewport (headless tests, or before `enable_viewport`).
    gizmo_deferred: bool,
    /// The accumulated delta of the deferred drag, mirrored from the last
    /// [`TransformPreview`]. It seeds an escalation when there is no viewport to ask.
    gizmo_drag: Option<GizmoSeed>,
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
    /// The viewport's **notification** observation: a wheel zoom or an orbit moves the
    /// camera without emitting an event, and the facade's next `getCameraNode` must see
    /// the camera that gesture produced (not the one the last render published).
    _viewport_camera: Option<Subscription>,
    /// The document stage as last built, so a transient preview change can redraw
    /// without re-reading the tessellations from the kernel (S4.6).
    last_stage: Option<Stage>,
    /// The preview generation the drawn stage was built for.
    preview_generation: u64,
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
    /// The log/linear range slider (a number with declared bounds).
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
    /// The quantity unit (`mm`), for the slider's and dial's readout.
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

/// A live vertical scrub from a property row's handle.
///
/// The overlay is not a `RowPopup`: the handle is dragged directly, the scale
/// appears while the button is held, and the release commits. The window root
/// forwards the moves (a drag leaves the 18 px handle immediately), and `window`
/// keeps the overlay in the window that started it.
struct ScrubDrag {
    /// The `<object>.<property>` key `commit_property` takes.
    key: String,
    label: String,
    unit: String,
    scrub: scrub::Scrub,
    /// The press position; the scale is anchored beside it.
    anchor: Point<Pixels>,
    window: WindowId,
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
            gizmo_mode: GizmoMode::Translate,
            user_origin: None,
            row_popup: None,
            scrub: None,
            wheel,
            _wheel_events: wheel_events,
            wheel_target: None,
            last_pointer: None,
            drag_baseline: None,
            gizmo_deferred: false,
            gizmo_drag: None,
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
            _viewport_camera: None,
            last_stage: None,
            preview_generation: 0,
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

    /// The shell's global key path, shared by both windows.
    ///
    /// `cancel_dialog` is enabled when the event comes from the Dev Tools window: the
    /// modal lives in the main window, and `Esc` there must still reach it. The main
    /// window keeps the modal's own focused `Esc` handling.
    pub(crate) fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        cancel_dialog: bool,
        window: &mut Window,
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
            // A session pivot is a pending re-pivot: Esc drops it and rebuilds the
            // triad at the object origin.
            if self.clear_user_origin(cx) {
                return true;
            }
        }
        // Enter accepts a Python-backed tool session (Draft's Line/Wire collect
        // points there). The tool commits through its own `doCommand`; the shell
        // then drops the native session. Rust tools keep their own Enter handling
        // in the viewport.
        if event.keystroke.key == "enter"
            && !self.modal_open()
            && self.active_tool.as_deref().is_some_and(|id| {
                id.starts_with(dispatch::PYTHON_TOOL_PREFIX)
            })
        {
            self.finish_tool(cx);
            return true;
        }
        // Fast escalation (P3): while a TransformGizmo is armed, `Tab` or a numeric
        // key opens the floating Move HUD instead of traversing focus or typing into
        // whatever happened to be focused. It is seeded with the live drag delta
        // (zero when idle) and focuses the field for the armed handle. A modal keeps
        // its own keys, so `Tab` still cycles an open dialog's fields.
        if !self.modal_open()
            && self.model.lock().unwrap().gizmo_armed
            && let Some(typed) = escalation_key(event)
        {
            let seed = self.gizmo_seed(cx);
            if self.escalate_to_move_hud(seed, typed, window, cx) {
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
            viewport.set_stage(self.stage_with_previews(&stage));
            // A document's scene is usually not the sample camera's scale: frame it once
            // so the viewport does not open from inside the model.
            viewport.frame_stage();
            self.last_stage = Some(stage);
            self.preview_generation = ferrocad_stage_py::preview_generation();
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
        // GPU was available) is applied now that there is one. A Python-backed
        // tool gets a fresh bridge: the session state lives in Python, so the
        // object is stateless between frames.
        if let Some(id) = self.active_tool.clone() {
            match self.tool_for(&id) {
                Ok(tool) => viewport.set_tool(Some(tool)),
                Err(error) => self.console_line(error, cx),
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
        // Every camera change the viewport makes outside an event (a wheel notch, a
        // middle-button orbit, the navigation cube, a keyboard view preset) notifies the
        // entity but reports nothing, so the camera the facade answers with would stay
        // at the last render's. Observing the notification republishes it in the same
        // effect cycle as the gesture. The render path (`publish_view`) still covers
        // the animation-driven moves (inertia, a snap that settles between frames).
        let camera = cx.observe(&viewport, |this, _viewport, cx| this.publish_camera(cx));
        self.viewport = Some(viewport);
        self._viewport_events = Some(events);
        self._viewport_camera = Some(camera);
    }

    /// Append a line to the console transcript.
    fn console_line(&mut self, line: String, cx: &mut Context<Self>) {
        self.model.lock().unwrap().console.push(line);
        let transcript = transcript_of(&self.model);
        self.console
            .update(cx, |area, cx| area.set_transcript(transcript, cx));
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
        // A console line is how a script selects (`Gui.Selection.addSelection`); the
        // facade queued those writes, and they are applied to the shell's selection
        // now — a console-driven selection therefore arms the same manipulators a
        // click does.
        self.drain_selection_requests(cx);
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


impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A script's transient previews (S4.6) change without a document edit; the
        // generation check is a mutex read, and only a real change rebuilds the stage.
        self.sync_previews(cx);
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(format!("FreeCAD {} · {}", model.version, model.backend))
                    .child(crate::chrome::dev_tools_button(dev_tools_open, cx)),
            );

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
                this.drag_scrub(event.position, event.pressed_button, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.end_settings_drag(cx);
                    this.end_scrub(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.end_scrub(cx);
                }),
            )
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
        // The scale of a handle being dragged in the settings card (the card can
        // live in this window).
        let body = match scrub_overlay(self, window) {
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
mod tests;
