//! Shell tests: wheel.

use super::*;

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
    let rotate_slot = cx
        .debug_bounds("radial-label-App_Rotate")
        .expect("Rotate is a slot");
    let delete_slot = cx
        .debug_bounds("radial-label-App_Delete")
        .expect("Delete is a slot");
    assert!(
        move_slot.origin.y < rotate_slot.origin.y && move_slot.origin.y < delete_slot.origin.y,
        "Move is the north slot (above Rotate and Delete)"
    );
    assert!(
        rotate_slot.center().x > delete_slot.center().x,
        "Rotate is the south-east slot and Delete the south-west one"
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
