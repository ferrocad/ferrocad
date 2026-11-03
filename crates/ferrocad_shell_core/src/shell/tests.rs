//! Shell tests, split by surface.
//!
//! Shared helpers live here; each child module holds one surface's tests.

use super::*;
use gpui::{Entity, TestAppContext, VisualTestContext};

/// Boot the shell from a known appearance state.
///
/// Tests share one interpreter and one parameter store, and stored appearance is
/// applied at boot now that providers no longer write visibility — a toggle left
/// behind by an earlier test would otherwise decide what this test's window draws.
/// Reset the three capability stores first; before the first boot in a process the
/// facade is not importable, and there is nothing to reset.
///
/// The native assembly's backend is installed on the test `App` first: the shell's
/// runtime paths resolve through it, and boot reads it.
fn boot_shell(cx: &mut TestAppContext) -> Arc<Mutex<ShellModel>> {
    let _ = crate::python::evaluate(
        "import FerroCAD\nFerroCAD.Grid.reset()\nFerroCAD.Axis.reset()\nFerroCAD.Gizmo.reset()\n",
    );
    cx.update(|cx| {
        // Install the native assembly's backend through *this* crate's slot: the
        // test build of `ferrocad_shell_core` is a separate compilation, so the
        // host's own `backend::install` would write a different `Global`. The
        // backend value itself is type-compatible (the trait is in
        // `ferrocad_shell_api`).
        crate::app::install_backend(cx, Box::new(ferrocad_shell_gpui::PythonAppBackend::new()));
        Shell::boot(cx).expect("boot shell")
    })
}

/// Boot without a gpui `App` (plain `#[test]`s that only inspect the model).
fn boot_shell_without_app() -> Arc<Mutex<ShellModel>> {
    let _ = crate::python::evaluate(
        "import FerroCAD\nFerroCAD.Grid.reset()\nFerroCAD.Axis.reset()\nFerroCAD.Gizmo.reset()\n",
    );
    boot_again_without_app()
}

/// Boot without a gpui `App` or the appearance reset, so a test can observe the
/// state a previous boot/write left behind.
fn boot_again_without_app() -> Arc<Mutex<ShellModel>> {
    let mut backend = ferrocad_shell_gpui::PythonAppBackend::new();
    Shell::boot_with(&mut backend).expect("boot shell")
}

/// Register a minimal `transform`/`apply` interaction provider for headless tests.
///
/// The boot app links no workbench, so without one the commit route has nothing to
/// call. This is deliberately not `ferrocad_toy`: importing an edition declares its
/// appearance preset, which would leak into the other shell tests' boot state.
fn register_transform_provider() {
    let result = crate::python::evaluate(
        r#"
import FerroCAD
import FreeCAD as _fc
def _test_transform(obj, manipulator, action, element, values):
    if manipulator != 'transform' or action != 'apply' or len(values) != 16:
        return None
    doc = obj.Document
    doc.openTransaction('Rotate')
    try:
        obj.Placement = _fc.Placement(_fc.Matrix(*values)) * obj.Placement
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {'ok': False, 'error': str(exc)}
    return {'ok': True}
FerroCAD.register_interaction_provider(_test_transform)
"#,
    )
    .expect("register a transform interaction provider");
    assert!(result.ok, "{}", result.output);
}

/// The boot object is an `App::FeaturePython`, which has no `Placement`. Give it one
/// so a transform or `_move_selection` can write a real transaction and the undo
/// count is meaningful.
fn give_params_a_placement() {
    let result = crate::python::evaluate(
        r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
if obj is not None and 'Placement' not in obj.PropertiesList:
    obj.addProperty('App::PropertyPlacement', 'Placement', 'Base', 'Placement')
"#,
    )
    .expect("add Placement to the boot object");
    assert!(result.ok, "{}", result.output);
}

fn quantity_row(value: &str, min: Option<f64>, max: Option<f64>) -> PropRow {
    PropRow {
        name: "Length".into(),
        label: String::new(),
        type_id: "App::PropertyLength".into(),
        value: value.into(),
        status: String::new(),
        editable: true,
        command: None,
        kind: "quantity".into(),
        min,
        max,
        unit: None,
        choices: Vec::new(),
    }
}

/// The text in a named dialog field (empty when the field is absent).
fn dialog_field(shell: &Shell, cx: &App, name: &str) -> String {
    // The Move form is its own surface, not a dialog: its fields have fixed names.
    if let Some(index) = ["dx", "dy", "dz"].iter().position(|field| *field == name)
        && let Some(text) = shell.move_field_text(index, cx)
    {
        return text;
    }
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

/// Paint the window once so a following `simulate_*` has hitboxes to dispatch to.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

/// The undo depth of the active document.
fn undo_count() -> usize {
    let result = crate::python::evaluate("FreeCAD.ActiveDocument.getAvailableUndos()")
        .expect("read the undo count");
    assert!(result.ok, "{}", result.output);
    result.output.trim().parse().expect("an integer undo count")
}

/// A windowed shell with a selected real object and a stage whose entity 7 is it.
///
/// The base app links no viewport/workbench, so `arm` cannot build a triad headlessly.
/// `ShellModel::gizmo_armed` is the shell's own "a TransformGizmo is hosted" state and
/// is what the key path consults. The stage gives a transform session its datum.
fn armed_window(
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<Shell>, Arc<Mutex<ShellModel>>) {
    let model = boot_shell(cx);
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut visual).unwrap();
    let mesh: StageMesh = serde_json::from_str(
        r#"{
            "entity": 7,
            "object": "Params",
            "positions": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            "triangles": [0, 1, 2],
            "triangle_parts": [0, 0, 0],
            "edges": [],
            "parts": []
        }"#,
    )
    .expect("the test mesh");
    visual.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let stage = shell.display.build(vec![mesh]);
            shell.set_stage(stage, cx);
            shell.select("Params", cx);
            shell.model.lock().unwrap().gizmo_armed = true;
        });
    });
    draw(&mut visual);
    (visual, shell, model)
}

/// Put the keyboard on the shell root and press `keys`.
fn press_root(cx: &mut VisualTestContext, shell: &Entity<Shell>, keys: &str) {
    let root = shell.read_with(&*cx, |shell, _| shell.root_focus.clone());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes(keys);
    draw(cx);
}

/// Click the centre of the element with `selector` (it must have been drawn).
fn click_at(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} has no bounds"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
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

/// A headless shell window for the palette tests, already drawn once.
fn palette_window(
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<Shell>, Arc<Mutex<ShellModel>>) {
    let model = boot_shell(cx);
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

mod boot;
mod tree;
mod interaction;
mod forms;
mod view;
mod devtools;
mod toolbar;
mod wheel;
mod palette;
mod viewport;

mod picking;
mod escalation;
mod loadout;
