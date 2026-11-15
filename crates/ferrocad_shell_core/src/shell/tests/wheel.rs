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

    // The globals, the single Create Dock toggle and Search are the dock's controls; the
    // tool, the verb and the pills are not.
    assert!(cx.debug_bounds("dock-App_Undo").is_some());
    assert!(cx.debug_bounds("dock-App_Fit").is_some());
    assert!(
        cx.debug_bounds("dock-create-toggle").is_some(),
        "the dock carries one Create Dock toggle, not the pills"
    );
    assert!(
        cx.debug_bounds("create-dock").is_some(),
        "the pills live in their own strip"
    );
    // The probe is a `Solid` tool, and it is the only creation tool here.
    assert!(
        cx.debug_bounds("creation-pill-0").is_some(),
        "the Solid pill is in the strip"
    );
    assert!(
        cx.debug_bounds("dock-Test_CreationProbe").is_none(),
        "a creation tool is not a dock button"
    );
    assert!(
        cx.debug_bounds("dock-Test_FaceProbe").is_none(),
        "a face verb is not a dock button"
    );
    assert!(cx.debug_bounds("dock-search").is_some());
    // The strip sits **above** the dock, not inside it.
    let strip = cx.debug_bounds("create-dock").expect("the strip");
    let dock_row = cx.debug_bounds("action-dock").expect("the dock");
    assert!(
        strip.origin.y + strip.size.height <= dock_row.origin.y,
        "the strip is above the dock ({strip:?} / {dock_row:?})"
    );

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
    click_at(&mut cx, "creation-pill-0-caret");
    draw(&mut cx);
    assert!(cx.debug_bounds("create-popover").is_some(), "the flyout opened");
    assert!(
        cx.debug_bounds("create-tabs").is_some(),
        "the flyout has a category header"
    );
    assert!(cx.debug_bounds("create-row-Test_CreationProbe").is_some());
    assert!(
        cx.debug_bounds("create-row-Test_FaceProbe").is_none(),
        "a wheel verb is not a creation row"
    );
    // The flyout grows upward from its pill, so it stays over the canvas and off the
    // strip and the dock both.
    let popover = cx.debug_bounds("create-popover").expect("the flyout");
    assert!(
        popover.origin.y + popover.size.height <= strip.origin.y + px(1.0),
        "the flyout sits above the strip ({popover:?} / {strip:?})"
    );

    // Picking the row loads the slot: the tool runs (it arms), and the pill behind the
    // flyout now names it, so the next click of the left half repeats it.
    click_at(&mut cx, "create-row-Test_CreationProbe");
    draw(&mut cx);
    assert!(
        model.lock().unwrap().status.contains("creation probe ran"),
        "the row arms the tool"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        Some("Test_CreationProbe".to_string())
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.memory("Solid").map(str::to_string)),
        Some("Test_CreationProbe".to_string()),
        "the pick fills its category's pill"
    );
    assert!(
        cx.debug_bounds("creation-pill-0").is_some(),
        "the pill survives the commit"
    );
    // The left half repeats the pill's tool.
    model.lock().unwrap().status.clear();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.cancel_tool(cx));
    });
    draw(&mut cx);
    click_at(&mut cx, "creation-pill-0-arm");
    draw(&mut cx);
    assert!(
        model.lock().unwrap().status.contains("creation probe ran"),
        "the pill's left half arms its tool"
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.cancel_tool(cx));
    });

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.removeCommand('Test_CreationProbe')\nFreeCADGui.removeCommand('Test_FaceProbe')",
    );
    assert!(cleanup.expect("remove the probes").ok);
}

/// The wheel's **root is stable** and its **categories are the context**.
///
/// `Move`, `Rotate`, `Toggle visibility` and `Delete` are root wedges on every target —
/// that is the bug where picking a face made Move and Rotate vanish — while the object
/// verbs inside `Modify ▸` are the ones the target filters.
#[gpui::test]
fn the_root_ring_is_stable_and_the_categories_carry_the_context(cx: &mut TestAppContext) {
    use ferrocad_geom::SelectionRef;

    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    let script = r#"
import FreeCADGui
def _probe(name, target, group, tone):
    class _Probe:
        def GetResources(self):
            return {
                "MenuText": name,
                "FerroCAD_Surface": "radial",
                "FerroCAD_Target": target,
                "FerroCAD_WheelGroup": group,
                "FerroCAD_Tone": tone,
            }
        def IsActive(self):
            return True
        def Activated(self):
            return {"status": name}
    return _Probe()

class _CreateProbe:
    def GetResources(self):
        return {
            "MenuText": "Line",
            "FerroCAD_Surface": "creation",
            "FerroCAD_Category": "Sketch",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "create probe"}

for _id, _command in (
    ("Test_BodyProbe", _probe("Body probe", "body", "Booleans", "additive")),
    ("Test_FaceProbe", _probe("Face probe", "face", "Surface", "structural")),
    ("Test_EdgeProbe", _probe("Edge probe", "edge", "Surface", "structural")),
    ("Test_CreateProbe", _CreateProbe()),
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

    // `(root wedge id, child ids)` per ring.
    let ids = |target: Option<SelectionRef>| -> Vec<(String, Vec<String>)> {
        shell.read_with(&cx, |shell, _| {
            shell
                .radial_slots(target.as_ref())
                .iter()
                .map(|slot| {
                    (
                        slot.id.to_string(),
                        slot.children
                            .iter()
                            .map(|child| child.id.to_string())
                            .collect(),
                    )
                })
                .collect()
        })
    };
    let roots = |ids: &[(String, Vec<String>)]| -> Vec<String> {
        ids.iter().map(|(root, _)| root.clone()).collect()
    };
    // Whether `id` is reachable in the ring: a root wedge, or a child of one (one level).
    let has = |ids: &[(String, Vec<String>)], id: &str| {
        ids.iter()
            .any(|(root, children)| root == id || children.iter().any(|child| child == id))
    };

    let body = ids(Some(SelectionRef::whole(0)));
    let face = ids(Some(SelectionRef::element(0, "Face6")));
    let edge = ids(Some(SelectionRef::element(0, "Edge3")));
    let empty = ids(None);

    // The root is the same compass everywhere: the three object verbs and Delete.
    for (label, ring) in [("body", &body), ("face", &face), ("edge", &edge), ("empty", &empty)] {
        let names = roots(ring);
        for id in ["App_Move", "App_Rotate", "App_ToggleVisibility", "App_Delete"] {
            assert!(
                names.iter().any(|root| root == id),
                "{id} is a root wedge on a {label} target: {names:?}"
            );
        }
        assert_eq!(
            names.last().map(String::as_str),
            Some("App_Delete"),
            "Delete keeps the last destructive slot on a {label} target: {names:?}"
        );
        assert!(
            names.len() <= ferrocad_widgets::wheel::MAX_WEDGES,
            "the root ring is {} wedges on a {label} target: {names:?}",
            names.len()
        );
        // Every category wedge has children, and no category is empty.
        for (root, children) in ring {
            if root.starts_with("group:") {
                assert!(!children.is_empty(), "{root} is not an empty category");
            }
        }
    }

    // The body verbs that a target filters live **inside** `Modify`, not at the root.
    assert!(has(&body, "Test_BodyProbe"), "{body:?}");
    assert!(!has(&body, "Test_FaceProbe"), "no face verb on a body: {body:?}");
    assert!(!has(&body, "Test_EdgeProbe"), "no edge verb on a body: {body:?}");
    assert!(has(&face, "Test_FaceProbe"), "{face:?}");
    assert!(!has(&face, "Test_BodyProbe"), "no body verb on a face: {face:?}");
    assert!(!has(&face, "Test_EdgeProbe"), "no edge verb on a face: {face:?}");
    assert!(has(&edge, "Test_EdgeProbe"), "{edge:?}");
    assert!(!has(&edge, "Test_FaceProbe"), "no face verb on an edge: {edge:?}");
    assert!(!has(&edge, "Test_BodyProbe"), "no body verb on an edge: {edge:?}");

    // `Create` is the Add hand: a root category on every target (it used to be a flat
    // fallback list of every creation tool, which is the ring that overflowed), and its
    // children are the creation categories, not the tools.
    for (label, ring) in [("body", &body), ("face", &face), ("edge", &edge)] {
        assert!(
            roots(ring).iter().any(|root| root == "group:Create"),
            "Create is a root category on a {label} target"
        );
    }
    let create = shell.read_with(&cx, |shell, _| {
        shell
            .radial_slots(Some(&SelectionRef::whole(0)))
            .iter()
            .find(|slot| slot.id.as_ref() == "group:Create")
            .map(|slot| {
                slot.children
                    .iter()
                    .map(|category| {
                        (
                            category.id.to_string(),
                            category
                                .children
                                .iter()
                                .map(|tool| tool.id.to_string())
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .expect("the Create category")
    });
    assert_eq!(create.len(), 1, "one creation category: {create:?}");
    assert_eq!(create[0].0, "group:Sketch", "{create:?}");
    assert_eq!(create[0].1, vec!["Test_CreateProbe".to_string()], "{create:?}");

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nfor _id in ('Test_BodyProbe', 'Test_FaceProbe', 'Test_EdgeProbe', 'Test_CreateProbe'):\n    FreeCADGui.removeCommand(_id)",
    );
    assert!(cleanup.expect("remove the probes").ok);
}

/// The ring is **hierarchical**: a verb that declares a `FerroCAD_WheelGroup` is not a
/// root wedge, and the groups that would overflow a ring are bucketed one level deeper.
///
/// That is the fix for the twenty-wedge circle — and for the nine-wedge panic: the root
/// is `Move`, `Rotate`, `Toggle visibility`, `Workplane ▸`, `Create ▸`, `Modify ▸`,
/// `Delete`, whatever the edition registers.
#[gpui::test]
fn grouped_verbs_live_under_their_category(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    let script = r#"
import FreeCADGui
def _probe(name, group, tone):
    class _Probe:
        def GetResources(self):
            return {
                "MenuText": name,
                "FerroCAD_Surface": "radial",
                "FerroCAD_Target": "body",
                "FerroCAD_Tone": tone,
                "FerroCAD_WheelGroup": group,
            }
        def IsActive(self):
            return True
        def Activated(self):
            return {"status": name}
    return _Probe()

for _id, _command in (
    ("Test_UnionProbe", _probe("Union probe", "Booleans", "additive")),
    ("Test_CutProbe", _probe("Cut probe", "Booleans", "additive")),
    ("Test_SoloProbe", _probe("Solo probe", "Workplane", "structural")),
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

    let ring = |id: &str| -> Vec<(String, Vec<(String, Vec<String>)>)> {
        shell.read_with(&cx, |shell, _| {
            let slots = shell.radial_slots(Some(&ferrocad_geom::SelectionRef::whole(0)));
            let Some(slot) = slots.iter().find(|slot| slot.id.as_ref() == id) else {
                return Vec::new();
            };
            slot.children
                .iter()
                .map(|child| {
                    (
                        child.id.to_string(),
                        child
                            .children
                            .iter()
                            .map(|grandchild| {
                                (
                                    grandchild.id.to_string(),
                                    grandchild
                                        .children
                                        .iter()
                                        .map(|verb| verb.id.to_string())
                                        .collect(),
                                )
                            })
                            .collect(),
                    )
                })
                .collect()
        })
    };
    let roots: Vec<String> = shell.read_with(&cx, |shell, _| {
        shell
            .radial_slots(Some(&ferrocad_geom::SelectionRef::whole(0)))
            .iter()
            .map(|slot| slot.id.to_string())
            .collect()
    });

    // The booleans are **behind** `Modify`, never at the root.
    assert!(
        !roots.iter().any(|root| root == "Test_UnionProbe"),
        "a grouped verb is not a root wedge: {roots:?}"
    );
    let modify = ring("group:Modify");
    let flat: Vec<&str> = modify.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        flat.contains(&"Test_UnionProbe") && flat.contains(&"Test_CutProbe"),
        "the booleans are Modify's children: {flat:?}"
    );

    // The workplane pair is its own root category; a probe that joins it is a child.
    let workplane = ring("group:Workplane");
    let workplane_verbs: Vec<&str> = workplane.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        workplane_verbs.contains(&"Test_SoloProbe"),
        "the workplane category holds its verbs: {workplane_verbs:?}"
    );
    assert!(!roots.iter().any(|root| root == "Test_SoloProbe"), "{roots:?}");

    // The ring is a ring: it never grows past the widget's eight readable labels, and
    // Delete keeps the last destructive slot.
    assert!(
        roots.len() <= ferrocad_widgets::wheel::MAX_WEDGES,
        "the root ring is {} wedges: {roots:?}",
        roots.len()
    );
    assert_eq!(
        roots.last().map(String::as_str),
        Some("App_Delete"),
        "Delete keeps the last destructive slot: {roots:?}"
    );

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nfor _id in ('Test_UnionProbe', 'Test_CutProbe', 'Test_SoloProbe'):\n    FreeCADGui.removeCommand(_id)",
    );
    assert!(cleanup.expect("remove the probes").ok);
}

/// `Backspace` at the root is the dismiss (the climb-out itself is the widget's own,
/// covered by `a_category_wedge_opens_its_ring_and_the_deadzone_climbs_back`).
#[gpui::test]
fn backspace_dismisses_the_root_ring(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);

    let wheel = shell.read_with(&cx, |shell, _| shell.wheel.clone());
    let anchor = point(px(400.), px(300.));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.open_radial(anchor, cx));
    });
    draw(&mut cx);
    assert!(wheel.read_with(&cx, |wheel, _| wheel.is_open()));
    assert!(cx.debug_bounds("radial-wheel").is_some());

    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("backspace");
    draw(&mut cx);
    assert!(
        !wheel.read_with(&cx, |wheel, _| wheel.is_open()),
        "Backspace at the root is the dismiss"
    );
}

/// The create dock is **one pill per category**, each remembering its own tool.
///
/// A pill reads `5 ⊕ <Category>: <Tool> ▾`: the left half arms it, the caret opens the
/// flyout, and the flyout's segmented header shows one category at a time. Picking a
/// tool **binds** it to its category and **runs** it — the same pair of actions a wheel
/// flick performs — while clicking a tab only points the pill at what that category
/// remembers. The keys `5`–`9` are the loadout's: a press arms the pill's tool and a
/// press on the equipped pill steps to the next tool of that category.
#[gpui::test]
fn each_creation_pill_remembers_its_own_tool(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell(cx);
    let script = r#"
import FreeCADGui
def _tool(name, category):
    class _Tool:
        def GetResources(self):
            return {
                "MenuText": name,
                "FerroCAD_Surface": "creation",
                "FerroCAD_Category": category,
            }
        def IsActive(self):
            return True
        def Activated(self):
            return {"status": name}
    return _Tool()

for _id, _command in (
    ("Test_SketchTool", _tool("Rectangle", "Sketch")),
    ("Test_SketchCircle", _tool("Circle", "Sketch")),
    ("Test_SolidTool", _tool("Box", "Solid")),
    ("Test_SurfaceTool", _tool("Loft", "Surface")),
):
    if _id not in FreeCADGui.listCommands():
        FreeCADGui.addCommand(_id, _command)
"#;
    let registered = crate::python::evaluate(script).expect("register the tools");
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

    // One pill per category, in the design's order (Sketch, Solid, Surface).
    assert!(cx.debug_bounds("create-dock").is_some(), "the strip is drawn");
    for index in 0..3 {
        let selector = match index {
            0 => "creation-pill-0",
            1 => "creation-pill-1",
            _ => "creation-pill-2",
        };
        assert!(cx.debug_bounds(selector).is_some(), "{selector} is a pill");
    }
    assert!(
        cx.debug_bounds("creation-pill-3").is_none(),
        "a category with no tool is not a pill"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        None,
        "a fresh session has armed nothing"
    );

    // The flyout has a tab per registered category, and the caret opens on its own.
    click_at(&mut cx, "creation-pill-1-caret");
    draw(&mut cx);
    assert!(cx.debug_bounds("create-popover").is_some());
    for tab in ["create-tab-Sketch", "create-tab-Solid", "create-tab-Surface"] {
        assert!(cx.debug_bounds(tab).is_some(), "{tab} is a category tab");
    }
    assert!(
        cx.debug_bounds("create-row-Test_SolidTool").is_some(),
        "the Solid pill opens on the Solid tab"
    );
    assert!(
        cx.debug_bounds("create-row-Test_SketchTool").is_none(),
        "one category at a time"
    );

    // A tab click shows that category's tools and points its pill at the remembered one.
    click_at(&mut cx, "create-tab-Surface");
    draw(&mut cx);
    assert!(cx.debug_bounds("create-row-Test_SurfaceTool").is_some());
    assert!(cx.debug_bounds("create-row-Test_SolidTool").is_none());
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.memory("Surface").map(str::to_string)),
        Some("Test_SurfaceTool".to_string()),
        "browsing Surface loads its pill"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        None,
        "browsing arms nothing"
    );

    // Picking the row runs (arms) the tool and keeps it in its own pill.
    click_at(&mut cx, "create-row-Test_SurfaceTool");
    draw(&mut cx);
    assert!(
        model.lock().unwrap().status.contains("Loft"),
        "the row runs the tool"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        Some("Test_SurfaceTool".to_string())
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.cancel_tool(cx));
    });

    // A Sketch pick fills **its** pill and leaves the others where they were.
    click_at(&mut cx, "creation-pill-1-caret");
    draw(&mut cx);
    click_at(&mut cx, "create-tab-Sketch");
    draw(&mut cx);
    click_at(&mut cx, "create-row-Test_SketchTool");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, _| (
            shell.creation.memory("Sketch").map(str::to_string),
            shell.creation.memory("Surface").map(str::to_string)
        )),
        (
            Some("Test_SketchTool".to_string()),
            Some("Test_SurfaceTool".to_string())
        ),
        "each category keeps its own tool"
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.cancel_tool(cx));
    });

    // The keys are the loadout's, so start from a fresh Add hand: a press **selects** the
    // pill and arms the tool it names, and a press on the pill that already holds the
    // armed tool **steps to the next tool of that category**. No press opens a menu.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _| shell.creation = CreationDock::default());
    });
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "5");
    assert!(
        model.lock().unwrap().status.contains("Rectangle"),
        "`5` arms the first pill's tool: {}",
        model.lock().unwrap().status
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        Some("Test_SketchTool".to_string()),
        "and that pill is the equipped one"
    );
    assert!(
        cx.debug_bounds("create-popover").is_none(),
        "the key never opens the menu"
    );

    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "5");
    assert!(
        model.lock().unwrap().status.contains("Circle"),
        "a second `5` steps to the next Sketch tool: {}",
        model.lock().unwrap().status
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.memory("Sketch").map(str::to_string)),
        Some("Test_SketchCircle".to_string()),
        "and the pill now holds it"
    );

    // Two Sketch tools, so a third press wraps back to the first.
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "5");
    assert!(
        model.lock().unwrap().status.contains("Rectangle"),
        "a third `5` wraps round the category: {}",
        model.lock().unwrap().status
    );

    // The flyout is the caret's route to the same pill. Its list is **numbered** (`1`–`8`,
    // in a column of its own), and a number picks that row into its pill without touching
    // the pointer.
    click_at(&mut cx, "creation-pill-0-caret");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("create-popover").is_some(),
        "the caret opens the pill's list"
    );
    assert!(
        cx.debug_bounds("create-row-Test_SketchTool-key").is_some(),
        "the row carries its selection key"
    );
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    assert!(
        model.lock().unwrap().status.contains("Rectangle"),
        "`1` runs the first row's tool: {}",
        model.lock().unwrap().status
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.memory("Sketch").map(str::to_string)),
        Some("Test_SketchTool".to_string()),
        "and loads its pill"
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_popover(cx);
            shell.cancel_tool(cx);
            shell.active_tool = None;
        });
    });

    // The same digit space keeps going: `6` is the Solid pill and `7` the Surface one, in
    // strip order. Solid holds one tool, so its key re-arms it — there is nothing to step
    // to — and still opens no menu.
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "6");
    assert!(
        model.lock().unwrap().status.contains("Box"),
        "`6` arms the second pill's tool: {}",
        model.lock().unwrap().status
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.active().map(str::to_string)),
        Some("Test_SolidTool".to_string()),
        "and it is the armed one"
    );
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "6");
    assert!(
        model.lock().unwrap().status.contains("Box"),
        "a single-tool slot re-arms itself: {}",
        model.lock().unwrap().status
    );
    assert!(
        cx.debug_bounds("create-popover").is_none(),
        "and still opens no menu"
    );
    model.lock().unwrap().status.clear();
    press_root(&mut cx, &shell, "7");
    assert!(
        model.lock().unwrap().status.contains("Loft"),
        "`7` arms the third pill's tool: {}",
        model.lock().unwrap().status
    );

    // The wheel's commit is the *same* seam: a tool chosen from `Create ▸` fills its
    // pill and runs, so the next flick down the ring starts from what it loaded.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.commit_wheel("Test_SolidTool", cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.creation.memory("Solid").map(str::to_string)),
        Some("Test_SolidTool".to_string()),
        "the wheel loads the Solid pill"
    );
    assert!(
        model.lock().unwrap().status.contains("Box"),
        "and runs the tool"
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.cancel_tool(cx));
    });

    // The dock's toggle hides the strip (and closes the flyout); the dock itself stays.
    click_at(&mut cx, "dock-create-toggle");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("create-dock").is_none(),
        "the toggle hides the strip"
    );
    assert!(
        cx.debug_bounds("action-dock").is_some(),
        "the dock is still there"
    );
    assert!(!shell.read_with(&cx, |shell, _| shell.creation.expanded()));
    click_at(&mut cx, "dock-create-toggle");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("create-dock").is_some(),
        "and shows it again"
    );

    let cleanup = crate::python::evaluate(
        "import FreeCADGui\nfor _id in ('Test_SketchTool', 'Test_SketchCircle', 'Test_SolidTool', 'Test_SurfaceTool'):\n    FreeCADGui.removeCommand(_id)",
    );
    assert!(cleanup.expect("remove the tools").ok);
}
