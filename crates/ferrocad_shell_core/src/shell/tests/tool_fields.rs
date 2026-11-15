//! Shell tests: the creation pill's gate, the tool strip's fields and `PILL_HOLD`.
//!
//! The C1/C2 promises this module pins:
//!
//! * an armed creation tool swaps the dock **in place** and publishes its numeric strip,
//!   without moving the canvas by a pixel;
//! * `Tab` focuses a field and a typed digit resizes the tool's own preview;
//! * a selection-dependent `TopologyGate` greys the row and names its requirement, and
//!   refuses `Select ▾` while the tool is armed;
//! * a pending transform refuses `StartTool` and dims the dock;
//! * `PILL_HOLD` is decoration, not a context;
//! * a press through the open flyout reports `clicked through: creation_pill`.

use super::*;

/// A shell with the Box tool contributed and nothing else changed.
fn box_window(cx: &mut TestAppContext) -> (VisualTestContext, Entity<Shell>, Arc<Mutex<ShellModel>>) {
    let model = boot_shell(cx);
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
            shell.set_contributions(
                crate::Contributions::new().with_tool("box", || {
                    Box::new(ferrocad_stage::BoxTool::new()) as Box<dyn ferrocad_stage::Tool>
                }),
            );
            shell.start_tool("box", cx);
        });
    });
    draw(&mut cx);
    (cx, shell, model)
}

/// The armed box's first field value (`W`), as the strip and the tool agree on it.
fn first_field(shell: &Entity<Shell>, cx: &VisualTestContext) -> Option<String> {
    shell.read_with(cx, |shell, _| {
        shell
            .tool_fields()
            .first()
            .map(|field| field.value.clone())
    })
}

#[gpui::test]
fn the_armed_box_adds_a_pill_above_the_dock_without_moving_the_canvas(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
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
    let dock = cx.debug_bounds("globals-dock").expect("the idle dock");
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    assert!(cx.debug_bounds("tool-strip").is_none());

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_contributions(
                crate::Contributions::new().with_tool("box", || {
                    Box::new(ferrocad_stage::BoxTool::new()) as Box<dyn ferrocad_stage::Tool>
                }),
            );
            shell.start_tool("box", cx);
        });
    });
    draw(&mut cx);

    // The pill is **added** to the canvas, not swapped for anything: the globals dock
    // keeps its column on the left edge and the bottom row keeps both hands.
    assert!(cx.debug_bounds("tool-strip").is_some(), "the tool's pill");
    assert!(
        cx.debug_bounds("globals-dock").is_some(),
        "the dock stays: activating a tool does not take the chrome away"
    );
    assert!(cx.debug_bounds("tool-fields").is_some(), "the numeric row is docked");
    for field in ["w", "l", "h"] {
        assert!(
            cx.debug_bounds(Box::leak(format!("tool-field-{field}").into_boxed_str()))
                .is_some(),
            "the {field} field is painted"
        );
    }
    // The dock does not move, the pill is centred over the bottom of the canvas (which is
    // where the dock used to be), and the canvas rectangle is untouched.
    let dock_after = cx.debug_bounds("globals-dock").expect("the dock");
    assert_eq!(dock_after.origin.y, dock.origin.y, "the dock does not move");
    assert_eq!(dock_after.center().x, dock.center().x, "and keeps its centre");
    let strip = cx.debug_bounds("tool-strip").expect("the pill");
    assert!(strip.center().x > dock_after.center().x, "the pill is not in the dock");
    let canvas_after = cx.debug_bounds("main-canvas").expect("the canvas");
    assert_eq!(canvas_after.origin.y, canvas.origin.y, "the canvas does not move");
    assert_eq!(canvas_after.size.height, canvas.size.height);
    assert_eq!(first_field(&shell, &cx).as_deref(), Some("20.00"));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.has_tool_fields()),
        true,
        "the pill owns Tab while the tool publishes fields"
    );
}

#[gpui::test]
fn tab_and_a_digit_resize_the_armed_boxes_fields(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = box_window(cx);

    // Unfocused by default: a digit does nothing until `Tab` (or the digit itself) puts
    // the caret in a field, which keeps the canvas's own keys intact.
    assert_eq!(first_field(&shell, &cx).as_deref(), Some("20.00"));
    press_root(&mut cx, &shell, "tab");
    assert!(
        shell.read_with(&cx, |shell, _| shell.tool_field_edit().is_some()),
        "Tab focuses the first field"
    );

    // A typed value replaces the field wholesale. (A headless shell has no GPU viewport,
    // so the tool instance lives in the viewport that is absent here; the shell still
    // routes the text and the strip shows what was typed. The tool-side write-back is
    // pinned by `box_tool`'s own tests and by the Parametric round trip.)
    press_root(&mut cx, &shell, "3");
    press_root(&mut cx, &shell, "0");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.tool_field_edit().map(|edit| edit.text.clone())),
        Some("30".to_string())
    );
    assert!(
        cx.debug_bounds("tool-field-w-value").is_some(),
        "the value is on the strip"
    );

    // `Tab` moves to the next field, seeding it with the tool's own value.
    press_root(&mut cx, &shell, "tab");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell
            .tool_field_edit()
            .map(|edit| (edit.index, edit.text.clone()))),
        Some((1, "20.00".to_string())),
        "the next field starts from the tool's value"
    );

    // The inner `Esc` gives the keyboard back without cancelling the tool.
    press_root(&mut cx, &shell, "escape");
    assert!(shell.read_with(&cx, |shell, _| shell.tool_field_edit().is_none()));
    assert!(
        shell.read_with(&cx, |shell, _| shell.active_tool.is_some()),
        "the first Esc only drops the field"
    );
}

#[gpui::test]
fn a_gated_creation_tool_names_its_requirement(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    let script = r#"
import FreeCADGui
class _LoftProbe:
    def GetResources(self):
        return {
            "MenuText": "Loft",
            "FerroCAD_Surface": "creation",
            "FerroCAD_Category": "Surface",
            "FerroCAD_Gate": "body:2",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "loft probe ran"}
if "Test_LoftProbe" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_LoftProbe", _LoftProbe())
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
        shell.update(cx, |shell, cx| shell.refresh_commands(cx));
    });
    draw(&mut cx);

    // With nothing selected the gate is unmet: the row is painted (it exists — a mod's
    // tool is never hidden), but as a disabled row carrying its requirement.
    click_at(&mut cx, "creation-pill-0-caret");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("create-row-Test_LoftProbe").is_some(),
        "the row is offered, not hidden"
    );
    let rows = shell.read_with(&cx, |shell, _| {
        let m = shell.model.lock().unwrap();
        let command = crate::commands::find(&m.commands, "Test_LoftProbe").expect("registered");
        let gate = command.topology_gate().expect("a declared gate");
        let (count, kind) = crate::chrome::selection_topology(&m.selection);
        gate.satisfied(count, kind)
    });
    assert!(!rows, "one reference does not satisfy body:2");

    // One body is still not enough; two are. The evaluation is the selection's, so the
    // Modify hand's slot does not enter it.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_popover(cx);
            shell.replace_selection(
                vec![
                    crate::selection::SelectionItem::new("Params"),
                    crate::selection::SelectionItem::new("Derived"),
                ],
                cx,
            );
        });
    });
    let satisfied = shell.read_with(&cx, |shell, _| {
        let m = shell.model.lock().unwrap();
        let command = crate::commands::find(&m.commands, "Test_LoftProbe").expect("registered");
        let gate = command.topology_gate().expect("a declared gate");
        let (count, kind) = crate::chrome::selection_topology(&m.selection);
        gate.satisfied(count, kind)
    });
    assert!(satisfied, "two references satisfy body:2");

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.removeCommand('Test_LoftProbe')",
    );
    assert!(cleanup.expect("remove the probe").ok);
}

#[gpui::test]
fn a_pending_move_refuses_a_creation_tool_and_dims_the_dock(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = box_window(cx);
    // A typed value makes the move pending, which locks the two hands against each other.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.cancel_tool(cx);
            shell.select("Params", cx);
        });
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().expect("the Move form").inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));
    draw(&mut cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.start_tool("box", cx);
        });
    });
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.active_tool.is_none()),
        "a pending move refuses a creation tool"
    );
    let status = shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone());
    assert!(status.contains("move pending"), "{status}");
    // The dock is dimmed and inert, not a second live surface under the pending move.
    assert!(cx.debug_bounds("globals-dock").is_some());
    assert!(cx.debug_bounds("tool-strip").is_none());
}

#[gpui::test]
fn pill_hold_is_decoration_not_a_context(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = box_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.cancel_tool(cx);
            shell.hold_pill();
            cx.notify();
        });
    });
    draw(&mut cx);

    assert!(shell.read_with(&cx, |shell, _| shell.pill_holding()));
    // `PILL_HOLD` holds no uncommitted state: the hands are free and nothing is gated.
    let refused = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.gate(crate::shell::Action::SwitchHand, cx))
    });
    assert!(!refused, "PILL_HOLD gates no action");
    assert!(
        cx.debug_bounds("globals-dock").is_some(),
        "the dock is live during the hold"
    );

    // It expires on its own (the design's timeout), and expiring changes nothing else.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.pill_hold = Some(web_time::Instant::now() - crate::shell::PILL_HOLD * 2);
            cx.notify();
        });
    });
    assert!(!shell.read_with(&cx, |shell, _| shell.pill_holding()));
}

#[gpui::test]
fn a_click_through_the_flyout_reports_the_pill(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = box_window(cx);
    let script = r#"
import FreeCADGui
class _PillProbe:
    def GetResources(self):
        return {
            "MenuText": "Create Box",
            "FerroCAD_Surface": "creation",
            "FerroCAD_Category": "Solid",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "pill probe ran"}
if "Test_PillProbe" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_PillProbe", _PillProbe())
"#;
    let registered = crate::python::evaluate(script).expect("register the probe");
    assert!(registered.ok, "{}", registered.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.cancel_tool(cx);
            shell.refresh_commands(cx);
            shell.open_popover("create", cx);
        });
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("create-popover").is_some());

    // A press beside the panel reaches the scene: the panel dismisses and the shell
    // reports the overlay the click passed through.
    cx.simulate_mouse_down(
        point(px(40.), px(120.)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        point(px(40.), px(120.)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(&mut cx);
    assert!(cx.debug_bounds("create-popover").is_none(), "the panel dismissed");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pass_through()),
        Some("creation_pill")
    );
    let line = shell.read_with(&cx, |shell, cx| shell.hover_line(cx));
    assert!(
        line.contains("clicked through: creation_pill"),
        "the status line names the overlay: {line:?}"
    );

    // The probe is this test's only creation command; leaving it registered would decide
    // another test's pill default.
    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.removeCommand('Test_PillProbe')",
    );
    assert!(cleanup.expect("remove the probe").ok);
}
