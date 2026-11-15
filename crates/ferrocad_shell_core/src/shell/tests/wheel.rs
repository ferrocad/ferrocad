//! Shell tests: wheel.

use super::*;

/// T3: the action dock sits over the bottom-centre of the canvas, lists the dock
/// surface, and its rows follow `IsActive` while staying visible when disabled.
#[gpui::test]
fn the_action_dock_follows_the_selection(cx: &mut TestAppContext) {
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
    // The dock is a command strip, so its glyphs come from the curated table; the
    // Search entry is the one glyph this harness can prove (it links no workbench,
    // so it registers no creation verbs).
    assert!(
        cx.debug_bounds("dock-search-icon").is_some(),
        "the search entry carries a magnifier"
    );
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
    // Each curated slot carries its command's glyph above the label.
    for selector in [
        "radial-slot-icon-App_Move",
        "radial-slot-icon-App_Rotate",
        "radial-slot-icon-App_Delete",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector} is painted");
    }
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
    let model = boot_shell(cx);
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
    let model = boot_shell(cx);
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

/// The **strict dock contract**: the idle strip is the globals, the split creation
/// pill and Search — never a list of creation or modification buttons — and its
/// rectangle never reaches the bottom-left hand column.
#[gpui::test]
fn the_dock_is_the_creation_pill_not_a_command_strip(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    // A creation tool and a face verb: the two kinds that used to be dock buttons.
    let script = r#"
import FreeCADGui
class _CreationProbe:
    def GetResources(self):
        return {
            "MenuText": "Create Box",
            "FerroCAD_Surface": "creation",
            "FerroCAD_Category": "Solid",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "creation probe ran"}

class _FaceProbe:
    def GetResources(self):
        return {
            "MenuText": "Shell",
            "FerroCAD_Surface": "radial",
            "FerroCAD_Target": "face",
            "FerroCAD_Tone": "structural",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "face probe ran"}

for _id, _command in (
    ("Test_CreationProbe", _CreationProbe()),
    ("Test_FaceProbe", _FaceProbe()),
):
    if _id not in FreeCADGui.listCommands():
        FreeCADGui.addCommand(_id, _command)
"#;
    let registered = crate::python::evaluate(script).expect("register the probes");
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

    // The globals and the pill are dock controls; the tool and the verb are not.
    assert!(cx.debug_bounds("dock-App_Undo").is_some());
    assert!(cx.debug_bounds("dock-App_Fit").is_some());
    assert!(cx.debug_bounds("creation-pill").is_some(), "the pill is in the dock");
    assert!(
        cx.debug_bounds("dock-Test_CreationProbe").is_none(),
        "a creation tool is not a dock button"
    );
    assert!(
        cx.debug_bounds("dock-Test_FaceProbe").is_none(),
        "a face verb is not a dock button"
    );
    assert!(cx.debug_bounds("dock-search").is_some());

    // The two bottom bars must not collide. They share a baseline by design (the dock
    // is centred, the hand column is bottom-left), so it is the **horizontal** gap that
    // keeps them apart — this is the assertion the overflowing strip failed.
    let dock = cx.debug_bounds("action-dock").expect("the dock");
    let loadout = cx.debug_bounds("loadout-bar").expect("the hand column");
    assert!(
        dock.origin.x >= loadout.origin.x + loadout.size.width,
        "the dock ({dock:?}) clears the loadout bar ({loadout:?})"
    );
    let intersect = dock.origin.x < loadout.origin.x + loadout.size.width
        && loadout.origin.x < dock.origin.x + dock.size.width
        && dock.origin.y < loadout.origin.y + loadout.size.height
        && loadout.origin.y < dock.origin.y + dock.size.height;
    assert!(!intersect, "the bars do not overlap ({dock:?} / {loadout:?})");

    // The caret opens the category flyout: the creation tool is a row, the verb is not.
    click_at(&mut cx, "creation-pill-caret");
    draw(&mut cx);
    assert!(cx.debug_bounds("create-popover").is_some(), "the flyout opened");
    assert!(cx.debug_bounds("create-row-Test_CreationProbe").is_some());
    assert!(
        cx.debug_bounds("create-row-Test_FaceProbe").is_none(),
        "a wheel verb is not a creation row"
    );
    // The flyout grows upward from the dock, so it stays over the canvas.
    let popover = cx.debug_bounds("create-popover").expect("the flyout");
    assert!(
        popover.origin.y + popover.size.height <= dock.origin.y,
        "the flyout sits above the dock ({popover:?} / {dock:?})"
    );

    // Running a row arms the tool and becomes the pill's memory.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.close_popover(cx));
    });
    click_at(&mut cx, "creation-pill-arm");
    draw(&mut cx);
    assert!(
        model.lock().unwrap().status.contains("creation probe ran"),
        "the fast path runs the remembered tool"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.active_creation.clone()),
        Some("Test_CreationProbe".to_string())
    );

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.removeCommand('Test_CreationProbe')\nFreeCADGui.removeCommand('Test_FaceProbe')",
    );
    assert!(cleanup.expect("remove the probes").ok);
}

/// The wheel is a **context** menu: the ring holds the verbs the target kind can use,
/// and `Delete` keeps the last destructive slot in every context.
#[gpui::test]
fn the_wheel_is_filtered_by_the_context_target(cx: &mut TestAppContext) {
    use ferrocad_geom::SelectionRef;

    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    let script = r#"
import FreeCADGui
def _probe(name, surface, target, tone):
    class _Probe:
        def GetResources(self):
            resources = {"MenuText": name, "FerroCAD_Surface": surface}
            if target:
                resources["FerroCAD_Target"] = target
            if tone:
                resources["FerroCAD_Tone"] = tone
            return resources
        def IsActive(self):
            return True
        def Activated(self):
            return {"status": name}
    return _Probe()

for _id, _command in (
    ("Test_BodyProbe", _probe("Body probe", "radial", "body", "")),
    ("Test_FaceProbe", _probe("Face probe", "radial", "face", "")),
    ("Test_EdgeProbe", _probe("Edge probe", "radial", "edge", "additive")),
    ("Test_DeleteProbe", _probe("Delete probe", "radial", "", "destructive")),
):
    if _id not in FreeCADGui.listCommands():
        FreeCADGui.addCommand(_id, _command)
"#;
    let registered = crate::python::evaluate(script).expect("register the probes");
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

    let ids = |target: Option<SelectionRef>| -> Vec<String> {
        shell.read_with(&cx, |shell, _| {
            shell
                .radial_slots(target.as_ref())
                .iter()
                .map(|slot| slot.id.to_string())
                .collect()
        })
    };
    let has = |ids: &[String], id: &str| ids.iter().any(|slot| slot == id);

    // A body target: the object verbs and the body-only probe; no face/edge wedges.
    let body = ids(Some(SelectionRef::whole(0)));
    assert!(has(&body, "Test_BodyProbe"), "{body:?}");
    assert!(has(&body, "App_Move"), "{body:?}");
    assert!(!has(&body, "Test_FaceProbe"), "no face verb on a body: {body:?}");
    assert!(!has(&body, "Test_EdgeProbe"), "no edge verb on a body: {body:?}");
    assert_eq!(
        body.last().map(String::as_str),
        Some("App_Delete"),
        "the built-in Delete is the final destructive slot: {body:?}"
    );
    assert!(
        !has(&body, "App_SetWorkplane"),
        "Set WP needs a face, so it is not a body wedge: {body:?}"
    );

    // A face target: face verbs only (plus the any-context Delete), and no Move.
    let face = ids(Some(SelectionRef::element(0, "Face6")));
    assert!(has(&face, "Test_FaceProbe"), "{face:?}");
    assert!(!has(&face, "Test_BodyProbe"), "no body verb on a face: {face:?}");
    assert!(!has(&face, "Test_EdgeProbe"), "no edge verb on a face: {face:?}");
    assert!(!has(&face, "App_Move"), "Move is a body verb: {face:?}");
    assert!(has(&face, "App_SetWorkplane"), "Set WP is a face wedge: {face:?}");
    assert_eq!(face.last().map(String::as_str), Some("App_Delete"));

    // An edge target: the edge verb, the any-context Save WP is face/body only, Delete
    // last.
    let edge = ids(Some(SelectionRef::element(0, "Edge3")));
    assert!(has(&edge, "Test_EdgeProbe"), "{edge:?}");
    assert!(!has(&edge, "Test_FaceProbe"), "no face verb on an edge: {edge:?}");
    assert!(!has(&edge, "Test_BodyProbe"), "no body verb on an edge: {edge:?}");
    assert!(!has(&edge, "App_SetWorkplane"), "Set WP is a face verb: {edge:?}");
    assert_eq!(edge.last().map(String::as_str), Some("App_Delete"));

    // The additive edge probe sorts after the structural ones, before Delete.
    let edge_probe = edge.iter().position(|id| id == "Test_EdgeProbe").unwrap();
    let structural = edge
        .iter()
        .take_while(|id| *id != "Test_EdgeProbe")
        .count();
    assert_eq!(edge_probe, structural, "tone order is structural → additive");

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nfor _id in ('Test_BodyProbe', 'Test_FaceProbe', 'Test_EdgeProbe', 'Test_DeleteProbe'):\n    FreeCADGui.removeCommand(_id)",
    );
    assert!(cleanup.expect("remove the probes").ok);
}
