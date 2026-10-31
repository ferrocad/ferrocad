//! Shell tests: interaction.

use super::*;
use super::super::context::InteractionContext;
use super::super::pill::AxisFrame;

/// The wheel's Rotate verb is the discoverable way into the rings: it switches
/// the session mode, and Move forces Translate again so its Δ HUD and the
/// visible arrows agree.
#[gpui::test]
fn the_rotate_verb_switches_the_gizmo_mode(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    shell.update(cx, |shell, cx| shell.select("Params", cx));
    let slots: Vec<(String, bool)> = shell.update(cx, |shell, _| {
        shell
            .radial_slots(None)
            .into_iter()
            .map(|slot| (slot.id.to_string(), slot.enabled))
            .collect()
    });
    assert!(
        slots
            .iter()
            .any(|(id, enabled)| id == "App_Rotate" && *enabled),
        "the wheel offers Rotate for a selected object: {slots:?}"
    );
    assert!(
        slots.iter().any(|(id, _)| id == "App_Move"),
        "slots: {slots:?}"
    );
    assert_eq!(
        shell.update(cx, |shell, _| shell.gizmo_mode),
        GizmoMode::Translate,
        "translate is the session default"
    );

    shell.update(cx, |shell, cx| shell.run_command("App_Rotate", cx));
    assert_eq!(
        shell.update(cx, |shell, _| shell.gizmo_mode),
        GizmoMode::Rotate,
        "the verb put the triad in its ring mode"
    );
    let status = shell.update(cx, |shell, _| shell.model.lock().unwrap().status.clone());
    assert!(status.contains("ring"), "status: {status}");

    // Move resets the mode before it opens its floating HUD.
    shell.update(cx, |shell, cx| shell.run_command("App_Move", cx));
    assert_eq!(
        shell.update(cx, |shell, _| shell.gizmo_mode),
        GizmoMode::Translate,
        "Move forces the arrows so its fields match the handles"
    );
    assert!(
        shell.update(cx, |shell, _| shell.move_pill_open()),
        "the Move form opened"
    );
}

/// Slice 3: the origin handle reports a session pivot, the triad uses it, a new
/// selection or a wheel promotion clears it, and `Esc` restores the object origin.
#[gpui::test]
fn the_session_pivot_follows_the_origin_event_and_escape_restores_it(cx: &mut TestAppContext) {
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

    let pivot = Vector3::new(2.0, 3.0, 0.0);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            shell.on_viewport_event(
                &ViewportEvent::GizmoOrigin {
                    point: pivot,
                    kind: SnapKind::Vertex,
                },
                cx,
            );
        });
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.user_origin),
        Some(pivot)
    );

    // The frame uses the session pivot: a fresh factory gizmo is moved to it.
    let mut gizmo = TransformGizmo::new(7, Vector3::zero(), GizmoMode::Translate);
    let object_origin = gizmo.origin();
    let origin = shell.read_with(&cx, |shell, _| shell.gizmo_origin_for(object_origin));
    gizmo.set_origin(origin);
    assert_eq!(gizmo.origin(), pivot);

    // Esc drops the pivot; the next arm reports the object origin again.
    let root = shell.read_with(&cx, |shell, _| shell.root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(shell.read_with(&cx, |shell, _| shell.user_origin), None);
    let fresh = TransformGizmo::new(7, Vector3::zero(), GizmoMode::Translate);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.gizmo_origin_for(fresh.origin())),
        Vector3::zero()
    );

    // A wheel promotion (a context target acted on) clears it as well.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_viewport_event(
                &ViewportEvent::GizmoOrigin {
                    point: pivot,
                    kind: SnapKind::Grid,
                },
                cx,
            );
            shell.wheel_target = Some(WheelTarget {
                reference: SelectionRef {
                    object: 7,
                    elements: Vec::new(),
                },
                anchor: None,
            });
            shell.commit_wheel("App_Rotate", cx);
        });
    });
    assert_eq!(shell.read_with(&cx, |shell, _| shell.user_origin), None);

    // A new selection clears it too.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_viewport_event(
                &ViewportEvent::GizmoOrigin {
                    point: pivot,
                    kind: SnapKind::EdgeMid,
                },
                cx,
            );
            shell.toggle_select("Derived", cx);
        });
    });
    assert_eq!(shell.read_with(&cx, |shell, _| shell.user_origin), None);
}

/// The form is anchored to the visible origin, so the pivot handle is disabled while it
/// is open; equipping another hand re-enables it.
#[gpui::test]
fn the_move_hud_disables_the_origin_handle(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert!(
        !shell.read_with(cx, |shell, _| shell.gizmo_origin_enabled()),
        "the always-on form owns the anchor"
    );

    shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    assert!(
        shell.read_with(cx, |shell, _| shell.gizmo_origin_enabled()),
        "closing the form re-enables the handle"
    );
}

/// The copyable report names the reason and the settings that usually fix it, so a
/// pasted notice is actionable on its own.
#[test]
fn the_gpu_notice_report_names_the_reason_and_the_fix() {
    let details = crate::shell::gpu_notice_details(
        "dma-buf ring: an exported wgpu surface needs a Vulkan wgpu device",
    );
    assert!(details.contains("image path"));
    assert!(details.contains("Reason:"));
    assert!(details.contains("needs a Vulkan wgpu device"));
    assert!(details.contains("VK_ICD_FILENAMES"));
    assert!(details.contains("ZED_DEVICE_ID"));
}

/// ✓ spends a pending move, ends the session and keeps the form: the form belongs to
/// the Move hand, so the object the user just placed is still draggable.
#[gpui::test]
fn accepting_the_move_hud_ends_the_transform_session(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "the Move hand owns the selection"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.move_pill_open()),
        "the Move form is always on with a target"
    );

    // A drag makes the session pending; ✓ spends it.
    let preview = |phase, x: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, 0.0, 0.0),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
    };
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 5.0), cx);
            shell.commit_session(cx);
        })
    });
    assert!(
        !shell.read_with(cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the pending session is spent"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.move_pill_open()),
        "the form stays after a commit"
    );
    assert_eq!(
        shell.read_with(cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "0.00",
        "the fields return to rest"
    );
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "the triad stays armed on the object that was just moved"
    );

    // A new selection keeps the hand: the loadout is a tool choice, not a selection
    // property.
    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "a new selection does not change the hand"
    );
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

    // A modal command (the Move form is not one): the shell renders it over a dimmed
    // backdrop and `Esc` cancels it.
    let script = r#"
import FreeCADGui
class _TestModalRender:
    def GetResources(self):
        return {"MenuText": "Modal render"}
    def Activated(self):
        return {
            "title": "Modal render",
            "fields": [{"name": "Note", "kind": "text", "value": "x"}],
            "on_accept": lambda values: {"status": "ok"},
        }
if "Test_ModalRender" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_ModalRender", _TestModalRender())
"#;
    let registered = crate::python::evaluate(script).expect("evaluate");
    assert!(registered.ok, "{}", registered.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("Test_ModalRender", cx))
    });
    assert!(
        shell.read_with(&cx, |shell, _| shell.dialog.is_some()),
        "the modal opened"
    );

    // Force a full paint: the backdrop, fields and buttons are built.
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        cx.debug_bounds("dialog-backdrop").is_some(),
        "a modal paints its dimmed backdrop"
    );

    // The first field held focus, so Escape cancels and drops its token.
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

    assert!(cx.debug_bounds("move-hud").is_some(), "the HUD paints");
    assert!(
        cx.debug_bounds("dialog-backdrop").is_none(),
        "a float has no dimming backdrop"
    );
    // The Move HUD is the single 3-field pill. At rest it is an inspector, so the
    // ✓/✗ finish pair has nothing to spend and is absent.
    assert!(cx.debug_bounds("move-hud").is_some());
    assert!(
        cx.debug_bounds("dialog-accept").is_none(),
        "no ✓ while the move is clean"
    );
    assert!(
        cx.debug_bounds("dialog-cancel").is_none(),
        "no ✗ while the move is clean"
    );

    // A typed value makes the move pending, and the pair materialises.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("dialog-accept").is_some(), "✓ finishes the move");
    assert!(cx.debug_bounds("dialog-cancel").is_some(), "✗ cancels it");

    // The form opens with no field focused, so the loadout keys keep working; the first
    // `Tab` takes the keyboard and the second reaches ΔY.
    let second = shell.read_with(&cx, |shell, cx| {
        shell
            .move_hud
            .as_ref()
            .map(|hud| hud.inputs[1].read(cx).focus_handle(cx))
    });
    let second = second.expect("a second field");
    cx.simulate_keystrokes("tab");
    draw(&mut cx);
    cx.simulate_keystrokes("tab");
    assert!(
        cx.update(|window, _cx| second.is_focused(window)),
        "Tab moves focus to the next delta field"
    );
}

/// The pending move's chrome is two pieces: the fields dock bottom-left with the
/// loadout, and `✗`/`✓` float in their own pill beside the gizmo.
#[gpui::test]
fn the_move_form_docks_the_fields_and_floats_the_finish_pair(cx: &mut TestAppContext) {
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
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
            shell.on_move_typed(cx);
        })
    });
    draw(&mut cx);

    let form = cx.debug_bounds("move-hud").expect("the docked form");
    let mode = cx.debug_bounds("move-mode").expect("the mode switch");
    let first = cx.debug_bounds("move-axis-0").expect("ΔX");
    let last = cx.debug_bounds("move-axis-2").expect("ΔZ");
    let finish = cx.debug_bounds("move-finish").expect("the finish pill");
    let cancel = cx.debug_bounds("dialog-cancel").expect("✗");
    let accept = cx.debug_bounds("dialog-accept").expect("✓");

    // The docked row: the mode switch leads, then the fields run X → Z.
    assert!(form.contains(&mode.center()));
    assert!(form.contains(&first.center()));
    assert!(form.contains(&last.center()));
    assert!(mode.origin.x <= first.origin.x, "the mode switch leads");
    assert!(first.origin.x < last.origin.x, "the fields run X → Z");

    // The finish pair is a separate in-context pill, not part of the dock.
    assert!(
        !form.contains(&accept.center()),
        "✓ is not in the docked row"
    );
    assert!(
        !form.contains(&cancel.center()),
        "✗ is not in the docked row"
    );
    assert!(finish.contains(&cancel.center()));
    assert!(finish.contains(&accept.center()));
    assert!(cancel.origin.x < accept.origin.x, "✗ precedes ✓");
}

/// The always-on form and a command modal are different surfaces: the modal opens over
/// the form (which hides its glass panel), and the form returns when it closes.
#[gpui::test]
fn the_move_form_coexists_with_a_command_dialog(cx: &mut TestAppContext) {
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
    draw(&mut cx);
    assert!(cx.debug_bounds("move-hud").is_some(), "the form paints");

    let script = r#"
import FreeCADGui
class _TestMoveModal:
    def GetResources(self):
        return {"MenuText": "Move modal"}
    def Activated(self):
        return {
            "title": "Move modal",
            "fields": [{"name": "Note", "kind": "text", "value": "x"}],
            "on_accept": lambda values: {"status": "ok"},
        }
if "Test_MoveModal" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_MoveModal", _TestMoveModal())
"#;
    let registered = crate::python::evaluate(script).expect("evaluate");
    assert!(registered.ok, "{}", registered.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("Test_MoveModal", cx))
    });
    draw(&mut cx);

    assert!(
        shell.read_with(&cx, |shell, _| shell.dialog.is_some()
            && shell.move_pill_open()),
        "the modal opened over the form's state"
    );
    assert!(
        cx.debug_bounds("dialog-backdrop").is_some(),
        "the modal paints its backdrop"
    );
    assert!(
        cx.debug_bounds("move-hud").is_none(),
        "the form hides under the modal rather than stacking"
    );

    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert!(!shell.read_with(&cx, |shell, _| shell.dialog.is_some()));
    assert!(
        cx.debug_bounds("move-hud").is_some(),
        "the form returns with the canvas"
    );
}

/// The explicit context model: a pending move is its own context, and the gate refuses
/// an edit command while it lasts. View-only commands stay live. Finishing the move
/// lifts the lock.
#[gpui::test]
fn an_edit_command_is_refused_while_a_move_is_pending(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    let script = r#"
import FreeCADGui
class _TestGateEdit:
    def GetResources(self):
        return {"MenuText": "Test gate edit", "CmdType": "ForEdit"}
    def Activated(self):
        return {"status": "test edit ran"}
class _TestGateView:
    def GetResources(self):
        return {"MenuText": "Test gate view", "CmdType": "ForView"}
    def Activated(self):
        return {"status": "test view ran"}
if "Test_GateEdit" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_GateEdit", _TestGateEdit())
if "Test_GateView" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_GateView", _TestGateView())
"#;
    let registered = crate::python::evaluate(script).expect("evaluate");
    assert!(registered.ok, "{}", registered.output);
    // The gate reads the shell's registry snapshot, so take it after registering.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.context(cx)),
        InteractionContext::PendingMove { target: Some(7) },
        "the context carries the dirty object"
    );

    // An edit command is discarded, with a reason naming it.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.run_command("Test_GateEdit", cx)));
    let status = shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone());
    assert!(status.contains("move pending"), "the command was refused: {status}");
    assert!(status.contains("Test gate edit"), "the status names it: {status}");

    // A view command is harmless and still runs under the lock.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.run_command("Test_GateView", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone()),
        "test view ran"
    );

    // ✗ releases the lock: the same edit command now runs.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.dismiss_move_hud(cx)));
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.run_command("Test_GateEdit", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone()),
        "test edit ran"
    );
}

/// A drag that begins on a body the session does not own is refused: it would move the
/// old selection while the triad sits on another box.
#[gpui::test]
fn a_drag_on_another_body_is_refused_while_a_move_is_pending(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            // Entity 8 is not the session's target (the session owns 7).
            shell.on_transform_preview(
                &TransformPreview {
                    target_id: 8,
                    phase: PreviewPhase::Begin,
                    total_transform: ferrocad_types::Matrix4::translation(50.0, 0.0, 0.0),
                    value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
                    ring: None,
                },
                cx,
            );
        })
    });
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([5.0, 0.0, 0.0]),
        "the pending move is untouched"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("move pending"),
        "the drag was refused"
    );
}

/// A pending move locks its **subject**: unselecting or picking another object is
/// refused, so the move is neither committed nor dropped. Finishing it releases the lock.
#[gpui::test]
fn a_pending_move_locks_the_selection(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));

    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Derived", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().selection.clone()),
        vec![SelectionItem::new("Params")],
        "another object cannot be selected under a pending move"
    );
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.clear_selection(cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().selection.clone()),
        vec![SelectionItem::new("Params")],
        "and the selection cannot be cleared either"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("move pending"),
        "the status says why the click did nothing"
    );
    assert!(
        shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)),
        "the move is still pending"
    );

    // ✗ releases the lock; the same selection change then goes through.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.dismiss_move_hud(cx))
    });
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Derived", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().selection.clone()),
        vec![SelectionItem::new("Derived")],
        "with the move finished the subject can change"
    );
}

/// The rotate bar shows all three axes, but only the active one holds an input: the
/// siblings are `0.00` choices when clean and `—` locked cells once a step is pending.
#[gpui::test]
fn the_rotate_bar_locks_the_sibling_axes(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("move-hud").is_some(), "the rotate bar is docked");
    for axis in ["rotate-field-0", "rotate-field-1", "rotate-field-2"] {
        assert!(
            cx.debug_bounds(axis).is_some(),
            "the ΔθX/ΔθY/ΔθZ cells are visible"
        );
    }
    assert!(
        cx.debug_bounds("rotate-angle").is_some(),
        "the active cell holds the angle input"
    );
    assert!(
        cx.debug_bounds("rotate-choice-1").is_some(),
        "a clean sibling is a selectable 0.00 cell"
    );
    assert!(
        cx.debug_bounds("rotate-locked-1").is_none(),
        "nothing is locked yet"
    );
    assert!(
        cx.debug_bounds("rotate-reset").is_some(),
        "Reset 0° is offered"
    );
    assert!(
        cx.debug_bounds("move-axis-0").is_none(),
        "the three translation fields are not in the rotate bar"
    );

    // A pending X step locks the siblings to `—` and keeps one input.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_angle_text(15.0, cx);
            shell.on_angle_typed(cx);
        });
    });
    draw(&mut cx);
    assert!(
        cx.debug_bounds("rotate-locked-1").is_some(),
        "Y is locked while X is pending"
    );
    assert!(
        cx.debug_bounds("rotate-choice-1").is_none(),
        "the sibling is no longer a choice"
    );
    assert!(
        cx.debug_bounds("rotate-angle").is_some(),
        "the locked ring keeps its input"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.angle_field_text(cx)),
        Some("15.00".to_string())
    );
}

/// A ring drag is the same session as a move: it opens a pending value, locks the
/// subject, and commits **once** through the transform interaction. The rotate hand
/// has no coordinate row (those fields are a translation), but the `✗`/`✓` strip is
/// the same control.
#[gpui::test]
fn a_rotation_drag_opens_and_commits_a_pending_session(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_pill_open()),
        "the rotate hand docks its angular row"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.angle_field_text(cx)),
        Some("0.00".to_string()),
        "the angular field rests at zero"
    );

    let preview = |phase, degrees: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::identity(),
        value: GizmoValue::Angular {
            axis: GizmoAxis::Z,
            raw_deg: degrees,
            applied_deg: degrees,
            detent_deg: 15.0,
        },
        ring: None,
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 30.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 30.0), cx);
        });
    });
    assert!(
        shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)),
        "the release leaves the sweep pending"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.context(cx)),
        InteractionContext::PendingRotation { target: Some(7) },
        "rotation has its own pending context"
    );
    let pending = shell.read_with(&cx, |shell, _| {
        shell.session.as_ref().map(|session| {
            (
                session.mode(),
                session.pending_axis_index(),
                session.pending_degrees(),
            )
        })
    });
    assert!(
        matches!(
            pending,
            Some((GizmoMode::Rotate, Some(2), degrees)) if (degrees - 30.0).abs() < 1e-9
        ),
        "the session holds one Z step: {pending:?}"
    );
    let docked = shell.read_with(&cx, |shell, _| shell.pending_angle());
    assert!(
        matches!(docked, Some(degrees) if (degrees - 30.0).abs() < 1e-9),
        "the docked angular field mirrors the sweep: {docked:?}"
    );

    // ✓ spends it once, through the workbench's transform hook.
    let before = undo_count();
    let committed = cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.commit_session(cx)));
    assert!(committed, "the sweep committed");
    assert_eq!(undo_count(), before + 1, "exactly one undo step");
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()),
        "the session is spent"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("rotated"),
        "the status reports the verb, not a second readout"
    );

    // ✗ drops a second sweep without writing.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 15.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 15.0), cx);
        });
    });
    let before = undo_count();
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let _ = shell.drop_session("rotation discarded", cx);
        })
    });
    assert_eq!(undo_count(), before, "discarding writes nothing");
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()),
        "the session is dropped"
    );
}

/// The rotate bar shows three axis cells and **one** input. Typing previews a single
/// `(axis, angle)` step, the siblings lock while it is pending, and `Reset 0°` restores
/// world alignment.
#[gpui::test]
fn typing_an_angular_value_previews_and_commits(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });

    // X is the default ring; a typed 45 is exactly 45 about X.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_angle_text(45.0, cx);
            shell.on_angle_typed(cx);
        });
    });
    let pending = shell.read_with(&cx, |shell, _| {
        shell.session.as_ref().map(|session| {
            (
                session.mode(),
                session.pending_axis_index(),
                session.pending_degrees(),
            )
        })
    });
    assert!(
        matches!(
            pending,
            Some((GizmoMode::Rotate, Some(0), degrees)) if (degrees - 45.0).abs() < 1e-9
        ),
        "one X step: {pending:?}"
    );
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.angle_field_text(cx)),
        Some("45.00".to_string())
    );

    // Selecting Y while X is pending is refused: the visible X step must be applied or
    // cancelled first, so no transaction happens behind the user's back.
    let before = undo_count();
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.select_rotate_axis(1, window, cx));
    });
    assert_eq!(undo_count(), before, "a locked ring spends nothing");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(0),
        "X still owns the step"
    );
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));

    // Finish X, then Y is selectable and starts clean.
    assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.commit_session(cx))));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.select_rotate_axis(1, window, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(1),
        "Y is the selected cell"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.angle_field_text(cx)),
        Some("0.00".to_string())
    );

    // A typed 30 on Y is one Y step and spends one transaction.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_angle_text(30.0, cx);
            shell.on_angle_typed(cx);
        });
    });
    let pending = shell.read_with(&cx, |shell, _| {
        shell.session.as_ref().map(|session| {
            (
                session.pending_axis_index(),
                session.pending_degrees(),
            )
        })
    });
    assert!(
        matches!(pending, Some((Some(1), degrees)) if (degrees - 30.0).abs() < 1e-9),
        "one Y step: {pending:?}"
    );
    let before = undo_count();
    assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.commit_session(cx))));
    assert_eq!(undo_count(), before + 1, "exactly one undo step");

    // `WLD` is the default; the badge cycles the rings to the body, then the workplane.
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.axis_frame()),
        AxisFrame::World
    );
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.toggle_move_mode(window, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.axis_frame()),
        AxisFrame::Local
    );
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.toggle_move_mode(window, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.axis_frame()),
        AxisFrame::Workplane,
        "the third state is the active workplane"
    );

    // `Reset 0°` commits the rotation that cancels the body's committed orientation.
    let orientation = shell.read_with(&cx, |shell, _| shell.object_rotation_now());
    assert!(
        !orientation.is_same(&Rotation::identity(), 1e-6),
        "the test body really is rotated before the reset"
    );
    let before = undo_count();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.reset_rotation(cx));
    });
    assert_eq!(undo_count(), before + 1, "the reset is one transaction");
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.object_rotation_now())
            .is_same(&Rotation::identity(), 1e-6),
        "the body is world-aligned again"
    );
}

/// Successive drags on the **same** ring are one pending step: 30° + 30° + 30° reads
/// 90°, exactly as successive translation drags accumulate.
#[gpui::test]
fn successive_rotation_drags_accumulate_on_the_same_ring(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });

    let preview = |phase, degrees: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::identity(),
        value: GizmoValue::Angular {
            axis: GizmoAxis::Z,
            raw_deg: degrees,
            applied_deg: degrees,
            detent_deg: 15.0,
        },
        ring: None,
    };
    for expected in [30.0, 60.0, 90.0] {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::Update, 30.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::End, 30.0), cx);
            });
        });
        let pending = shell.read_with(&cx, |shell, _| {
            shell.session.as_ref().map(|session| {
                (
                    session.pending_axis_index(),
                    session.pending_degrees(),
                )
            })
        });
        assert!(
            matches!(pending, Some((Some(2), degrees)) if (degrees - expected).abs() < 1e-9),
            "the Z steps accumulate to {expected}: {pending:?}"
        );
    }
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));
}

/// `X`/`Y`/`Z` typed in the focused angle field choose the ring while the form is clean;
/// once a step is pending the siblings are refused rather than committing behind the
/// user's back.
#[gpui::test]
fn the_angle_field_switches_rings_on_xyz(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_angle_text(15.0, cx);
            shell.on_angle_typed(cx);
        });
    });
    assert!(shell.read_with(&cx, |shell, _| shell.rotation_pending().is_some()));

    // The pending X step owns the ring: `y` is refused and spends nothing.
    let before = undo_count();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_angle_rejected('y', cx));
    });
    assert_eq!(undo_count(), before, "a locked ring is not a hidden commit");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(0),
        "X still owns the step"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("Pending rotate X"),
        "the status explains the lock"
    );

    // After the step is finished, the same keys switch rings again.
    assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.commit_session(cx))));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_angle_rejected('y', cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(1),
        "Y is selected once nothing is pending"
    );
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_angle_rejected('z', cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(2),
        "Z is selected"
    );
    // A letter with no ring meaning is ignored (and is still not inserted by the field).
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_angle_rejected('q', cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_hud.as_ref().map(|hud| hud.axis)),
        Some(2)
    );
}

/// A pending rotation owns its ring: grabbing a sibling ring is refused (and consumes
/// the press) rather than spending a hidden transaction. Dragging the same ring still
/// accumulates; the step is exact, with no fractional residue on the other axes.
#[gpui::test]
fn grabbing_a_locked_ring_is_refused_until_the_step_is_finished(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });

    let preview = |axis: GizmoAxis, phase, degrees: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::identity(),
        value: GizmoValue::Angular {
            axis,
            raw_deg: degrees,
            applied_deg: degrees,
            detent_deg: 15.0,
        },
        ring: None,
    };
    let pending = |cx: &VisualTestContext| {
        shell.read_with(cx, |shell, _| {
            shell.session.as_ref().map(|session| {
                (
                    session.pending_axis_index(),
                    session.pending_degrees(),
                )
            })
        })
    };

    // Drag X +15 and leave it pending.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::Update, 15.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::End, 15.0), cx);
        });
    });
    assert!(
        matches!(pending(&cx), Some((Some(0), degrees)) if (degrees - 15.0).abs() < 1e-9),
        "X is pending"
    );

    // Grab Y: refused. No undo entry, and the X step is untouched.
    let before = undo_count();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(GizmoAxis::Y, PreviewPhase::Begin, 0.0), cx);
        });
    });
    assert_eq!(undo_count(), before, "a locked ring is not a hidden commit");
    assert!(
        matches!(pending(&cx), Some((Some(0), degrees)) if (degrees - 15.0).abs() < 1e-9),
        "X still owns the step"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("Pending rotate X"),
        "the status explains the lock"
    );

    // The same ring still accumulates: a second X drag adds to the pending angle.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::Update, 15.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::X, PreviewPhase::End, 15.0), cx);
        });
    });
    assert!(
        matches!(pending(&cx), Some((Some(0), degrees)) if (degrees - 30.0).abs() < 1e-12),
        "the X step is exactly 30 degrees"
    );

    // Finish X (one transaction), then Y is free again.
    let before = undo_count();
    assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.commit_session(cx))));
    assert_eq!(undo_count(), before + 1);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(GizmoAxis::Y, PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::Y, PreviewPhase::Update, 15.0), cx);
            shell.on_transform_preview(&preview(GizmoAxis::Y, PreviewPhase::End, 15.0), cx);
        });
    });
    assert!(
        matches!(pending(&cx), Some((Some(1), degrees)) if (degrees - 15.0).abs() < 1e-12),
        "one exact Y step: no fractional X/Z residue"
    );
}

/// `LOC` and `WP` move the rings off the world grid: the body frame follows the
/// committed rotation, the workplane frame follows a plane's `u`/`v`/`n`, and both
/// reach the armed gizmos.
#[gpui::test]
fn the_axis_frames_reach_the_body_and_the_workplane(cx: &mut TestAppContext) {
    use crate::contributions::Contributions;
    use ferrocad_stage::{Stage, TransformGizmo, Workplane, WorkplaneKind};

    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    // The base shell contributes no triad; host a rotate gizmo so the arming seam has
    // something to frame.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _cx| {
            shell.set_contributions(Contributions::new().with_gizmo(
                |_selection: &SelectionRef, stage: &Stage| {
                    Some(TransformGizmo::rotate(7, stage.item(7)?.anchor()?))
                },
            ));
        });
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });
    // 45° about +Z, so the body's X ring points along (√½, √½, 0).
    let rotated = cx.update(|_window, _cx| {
        crate::python::evaluate(
            r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
obj.Placement = FreeCAD.Placement(FreeCAD.Vector(0, 0, 0), FreeCAD.Rotation(FreeCAD.Vector(0, 0, 1), 45))
"#,
        )
    });
    assert!(rotated.expect("set the placement").ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh_move_origin(cx));
    });

    let stage = shell.read_with(&cx, |shell, _| {
        shell.last_stage.clone().expect("the test stage")
    });
    let reference = SelectionRef::whole(7);

    // `WLD`: the coordinate axes.
    let world = shell.read_with(&cx, |shell, _| shell.frame_basis(None));
    assert!(world[0].is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-9));
    assert!(world[2].is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9));

    // `LOC`: the X ring turns with the body, and the armed rings carry that basis.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _| {
            let _ = shell.pill.cycle_axis_frame();
        });
    });
    let local = shell.read_with(&cx, |shell, _| shell.frame_basis(None));
    let half = std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        local[0].is_equal(&Vector3::new(half, half, 0.0), 1e-9),
        "X follows the +45° Z rotation: {local:?}"
    );
    assert!(local[2].is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9));
    let set = shell.read_with(&cx, |shell, _| {
        shell.arm_gizmos(&reference, &stage, Some(local))
    });
    assert!(
        set.gizmos()
            .iter()
            .any(|gizmo| gizmo.frame().basis[0].is_equal(&local[0], 1e-9)),
        "the armed rings take the body basis"
    );

    // `WP`: a plane's `u`/`v`/`n` become the ring axes; `n = u × v = +X`.
    let plane = Workplane::new(
        Vector3::zero(),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        WorkplaneKind::Custom,
    )
    .expect("a valid plane");
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _| {
            let _ = shell.pill.cycle_axis_frame();
        });
    });
    let workplane = shell.read_with(&cx, |shell, _| shell.frame_basis(Some(&plane)));
    assert!(workplane[0].is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));
    assert!(workplane[1].is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9));
    assert!(workplane[2].is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-9));
    let set = shell.read_with(&cx, |shell, _| {
        shell.arm_gizmos(&reference, &stage, Some(workplane))
    });
    assert!(
        set.gizmos()
            .iter()
            .any(|gizmo| gizmo.frame().basis[2].is_equal(&workplane[2], 1e-9)),
        "the armed rings take the workplane basis"
    );
}

/// `LOC` translates along the body's own axes: a typed ΔX moves along the rotated X,
/// and the absolute fields read coordinates along that basis from the body's origin.
#[gpui::test]
fn a_local_translation_uses_the_body_axes(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    register_transform_provider();
    give_params_a_placement();
    // 90° about +Z: the body's X points along world +Y.
    let rotated = cx.update(|_window, _cx| {
        crate::python::evaluate(
            r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
obj.Placement = FreeCAD.Placement(FreeCAD.Vector(0, 0, 0), FreeCAD.Rotation(FreeCAD.Vector(0, 0, 1), 90))
"#,
        )
    });
    assert!(rotated.expect("set the placement").ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh_move_origin(cx));
    });

    // WLD → LOC.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.cycle_axis_frame(cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.axis_frame()),
        AxisFrame::Local
    );

    // A typed ΔX of 10 is ten along the body's X: world +Y.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_move_text(0, "10".to_string(), cx);
            shell.on_move_typed(cx);
        })
    });
    let delta = shell
        .read_with(&cx, |shell, _| shell.session.as_ref().map(|session| session.delta()))
        .expect("a pending session");
    assert!(
        delta.x.abs() < 1e-9 && (delta.y - 10.0).abs() < 1e-9 && delta.z.abs() < 1e-9,
        "local X is world +Y: {delta:?}"
    );

    // In LOC an absolute coordinate is measured from the body's own origin, so the
    // local frame reads zero at rest (it is anchored to the object).
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.pill.toggle_absolute();
            shell.set_move_fields([0.0; 3], cx);
        })
    });
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.move_field_text(0, cx)),
        Some("0.00".to_string())
    );
}

/// `Reset 0°` names the stored orientation without ever printing a fractional Euler
/// decomposition: a pure axis names itself, a compound tilt shows the shortest total angle,
/// and an identity dims the button.
#[gpui::test]
fn the_reset_button_names_the_stored_orientation(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    give_params_a_placement();
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx));
    });
    let label = |cx: &VisualTestContext| {
        shell.read_with(cx, |shell, _| shell.reset_rotation_label())
    };

    assert_eq!(label(&cx), ("Reset 0°".to_string(), false), "identity is inert");

    let rotated = cx.update(|_window, _cx| {
        crate::python::evaluate(
            r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
obj.Placement = FreeCAD.Placement(FreeCAD.Vector(0, 0, 0), FreeCAD.Rotation(FreeCAD.Vector(0, 0, 1), 45))
"#,
        )
    });
    assert!(rotated.expect("set the placement").ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh_move_origin(cx));
    });
    assert_eq!(
        label(&cx),
        ("Reset (Z 45°)".to_string(), true),
        "a pure axis is named exactly"
    );

    let compound = cx.update(|_window, _cx| {
        crate::python::evaluate(
            r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
obj.Placement = FreeCAD.Placement(FreeCAD.Vector(0, 0, 0), FreeCAD.Rotation(10.0, 20.0, 30.0))
"#,
        )
    });
    assert!(compound.expect("set a compound placement").ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh_move_origin(cx));
    });
    let (text, actionable) = label(&cx);
    assert!(actionable);
    assert!(text.starts_with("Reset (") && text.ends_with("°)"), "{text}");
    assert!(
        !text.contains("(X ") && !text.contains("(Y ") && !text.contains("(Z "),
        "a compound tilt has no single ring to name: {text}"
    );
}

/// A **pick** is refused before anything is re-targeted: clicking another body must not
/// move the triad (or re-arm it) while the pending move owns the current subject.
#[gpui::test]
fn a_pick_on_another_body_is_refused_while_a_move_is_pending(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    let pick = |entity: u32| SelectionEvent::Picked {
        reference: SelectionRef::whole(entity),
        anchor: None,
        click_count: 1,
    };

    // Another body: refused, selection and pending value untouched.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_selection(&pick(8), cx))
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().selection.clone()),
        vec![SelectionItem::new("Params")],
        "the pick did not become the selection"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([5.0, 0.0, 0.0]),
        "and the pending move is untouched"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("move pending"),
        "the status says why the pick did nothing"
    );

    // The already-selected body is a no-op too: re-picking must not reset the session.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.on_selection(&pick(7), cx))
    });
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([5.0, 0.0, 0.0]),
        "a pick on the dirty body keeps the pending value"
    );
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));
}

/// The handle set is a tool too: `Rotate` is refused while a move is pending, so the
/// form's delta is not dropped by the mode change.
#[gpui::test]
fn switching_the_handle_set_waits_for_a_pending_move(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
        })
    });
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx))
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.gizmo_mode),
        GizmoMode::Translate,
        "the rotate hand waits for the move to finish"
    );
    assert!(shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)));

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.dismiss_move_hud(cx))
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_gizmo_mode("rotate", cx))
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.gizmo_mode),
        GizmoMode::Rotate,
        "with no pending move the mode changes"
    );
}

/// The finish pill is placed in the **empty screen octant**: the triad's projected stems
/// and the incoming tape leg are summed and negated, so it never covers a stem or the
/// ruler. Under the screenshot's camera (Z up, Y up-right, X down-right, tape from
/// up-left) that lands down-left of the origin.
#[test]
fn the_finish_pill_repels_from_the_stems_and_the_ruler() {
    let viewport = size(px(800.0), px(600.0));
    let pill = size(px(80.0), px(24.0));
    let origin = point(px(400.0), px(300.0));
    let centre = |placed: Point<Pixels>| {
        point(
            px(placed.x.0 + pill.width.0 * 0.5),
            px(placed.y.0 + pill.height.0 * 0.5),
        )
    };

    let stems = Some([[0.7, 0.7], [0.7, -0.7], [0.0, -1.0]]);
    let placed = place_finish(origin, stems, Some([-0.7, -0.7]), viewport, pill);
    let middle = centre(placed);
    assert!(
        middle.x.0 < origin.x.0,
        "left of the triad: {placed:?}"
    );
    assert!(
        middle.y.0 > origin.y.0,
        "below the triad: {placed:?}"
    );
    let reach = ((middle.x.0 - origin.x.0).powi(2) + (middle.y.0 - origin.y.0).powi(2)).sqrt();
    assert!((reach - 36.0).abs() < 0.5, "36 px out from the origin: {placed:?}");

    // The tape alone pushes it to the opposite side.
    let placed = place_finish(
        origin,
        Some([[0.0, 0.0]; 3]),
        Some([1.0, 0.0]),
        viewport,
        pill,
    );
    assert!(
        centre(placed).x.0 < origin.x.0,
        "a tape from the right puts the pill on the left: {placed:?}"
    );

    // Nothing to repel from: the bottom-right fallback.
    let placed = place_finish(origin, None, None, viewport, pill);
    let middle = centre(placed);
    assert!(
        middle.x.0 > origin.x.0 && middle.y.0 > origin.y.0,
        "the fallback is down-right: {placed:?}"
    );

    // At an edge the clamp keeps it reachable.
    let placed = place_finish(point(px(4.0), px(596.0)), None, None, viewport, pill);
    assert!(placed.x.0 >= HUD_MARGIN);
    assert!(placed.y.0 + pill.height.0 <= 600.0 - HUD_DOCK_CLEARANCE);
}

/// While a gizmo drag runs the pair is **hidden**, so the stroke can never end on top of
/// it. It reappears on the next paint after the release.
#[gpui::test]
fn the_finish_pill_is_hidden_while_a_drag_runs(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    // Park the pointer mid-canvas: with no viewport the pill anchors to it.
    cx.simulate_mouse_move(
        point(px(600.0), px(400.0)),
        None::<MouseButton>,
        gpui::Modifiers::default(),
    );
    draw(&mut cx);

    let preview = |phase, y: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(0.0, y, 0.0),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0), cx);
        })
    });
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.drag_start.is_some()),
        "the drag is running"
    );
    assert!(
        cx.debug_bounds("move-finish").is_none(),
        "the pair is out of the way while the pointer owns the stroke"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([0.0, 5.0, 0.0]),
        "the pending drag is untouched"
    );

    // Released: the pair comes back and a click on it spends the move.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::End, 5.0), cx);
        })
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("move-finish").is_some(), "the pair returns");
    click_at(&mut cx, "dialog-accept");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([0.0, 0.0, 0.0]),
        "✓ spends it after the release"
    );
}

/// The finish pair floats beside the gizmo and freezes under the pointer, so a click
/// cannot slide the buttons away; the coordinate row is docked and never moves.
#[gpui::test]
fn the_finish_pill_tracks_the_anchor_and_the_form_stays_docked(cx: &mut TestAppContext) {
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
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
            shell.on_move_typed(cx);
        })
    });
    draw(&mut cx);

    let viewport = cx.update(|window, _cx| window.viewport_size());
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    let docked = cx.debug_bounds("move-hud").expect("the form");
    let bar = cx.debug_bounds("loadout-bar").expect("the loadout bar");
    assert_eq!(
        docked.origin.x, bar.origin.x,
        "the coordinate row is left-aligned with the loadout"
    );
    assert!(
        docked.origin.y + docked.size.height <= bar.origin.y,
        "the row stacks above the loadout: {docked:?} vs {bar:?}"
    );
    assert!(
        bar.origin.y + bar.size.height > canvas.origin.y + canvas.size.height - px(24.0),
        "the loadout keeps the dock baseline"
    );

    // With no viewport the anchor is the pointer, so the pair follows it.
    let first = cx.debug_bounds("move-finish").expect("the pair");
    assert_eq!(
        first.origin,
        place_finish(point(px(0.0), px(0.0)), None, None, viewport, first.size),
        "the pair hangs off the anchor"
    );
    let elsewhere = point(px(300.0), px(200.0));
    cx.simulate_mouse_move(elsewhere, None::<MouseButton>, gpui::Modifiers::default());
    draw(&mut cx);
    let moved = cx.debug_bounds("move-finish").expect("the pair");
    let expected = place_finish(elsewhere, None, None, viewport, moved.size);
    assert!(
        (moved.origin.x.0 - expected.x.0).abs() <= 1.0
            && (moved.origin.y.0 - expected.y.0).abs() <= 1.0,
        "moving the anchor moves the pair: {moved:?} vs {expected:?}"
    );
    assert_ne!(moved.origin, first.origin, "the pair moved");

    // A pointer inside the pill freezes it: a click must not slide the buttons away.
    let inside = point(moved.origin.x + px(30.0), moved.origin.y + px(18.0));
    cx.simulate_mouse_move(inside, None::<MouseButton>, gpui::Modifiers::default());
    draw(&mut cx);
    let frozen = cx.debug_bounds("move-finish").expect("the pair");
    assert_eq!(frozen.origin, moved.origin, "the pair freezes under the pointer");

    // The docked fields never move with the pointer or the pair.
    let form_after = cx.debug_bounds("move-hud").expect("the form");
    assert_eq!(
        form_after.origin, docked.origin,
        "the docked coordinates stay put"
    );
}

/// Seam B: a floating HUD is clamped into the viewport safe area.
#[test]
fn a_floating_hud_is_clamped_into_the_safe_area() {
    let viewport = size(px(800.0), px(600.0));
    let hud = size(px(400.0), px(72.0));

    let origin = place_hud(point(px(100.0), px(100.0)), viewport, hud);
    assert_eq!(
        origin,
        point(px(124.0), px(HUD_MARGIN)),
        "above-right of the anchor, its bottom 36 px clear of it (clamped at the top)"
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
    // A stage is what a drag publishes into; the session takes its datum from it.
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
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let stage = shell.display.build(vec![mesh]);
            shell.set_stage(stage, cx);
            shell.select("Params", cx);
            shell.run_command("App_Move", cx);
        })
    });
    draw(&mut cx);

    let field = |cx: &VisualTestContext, name: &str| {
        shell.read_with(cx, |shell, cx| dialog_field(shell, cx, name))
    };
    assert_eq!(field(&cx, "dx"), "0.00");

    let preview = |phase, x: f64, y: f64, z: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, y, z),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
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

/// A committed interaction supersedes a **pending** move, but the form stays: it
/// belongs to the hand, not to one target.
#[gpui::test]
fn a_competing_interaction_drops_a_pending_move(cx: &mut TestAppContext) {
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
            // A stage gives the session its datum.
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
            let stage = shell.display.build(vec![mesh]);
            shell.set_stage(stage, cx);
            shell.select("Params", cx);
            let preview = |phase, x: f64| TransformPreview {
                target_id: 7,
                phase,
                total_transform: ferrocad_types::Matrix4::translation(x, 0.0, 0.0),
                value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
                ring: None,
            };
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0), cx);
        })
    });
    draw(&mut cx);
    assert!(shell.read_with(&cx, |shell, _| shell.move_pill_open()));
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the drag is pending"
    );

    let height = InteractionCommit::new("toy_height", 0, "set").with_values([30.0]);
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.on_interaction(&height, cx)));
    assert!(
        !shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "a height commit drops the stale pending move"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_pill_open()),
        "the form stays for the hand's target"
    );
}

/// The form follows the selection: clearing it closes the form; replacing it keeps
/// the form, re-measured on the new target.
#[gpui::test]
fn a_selection_change_retargets_the_move_hud(cx: &mut TestAppContext) {
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
    draw(&mut cx);
    assert!(shell.read_with(&cx, |shell, _| shell.move_pill_open()));

    // Clearing the selection removes the hand's target, so the form goes.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.clear_selection(cx)));
    assert!(
        !shell.read_with(&cx, |shell, _| shell.move_pill_open()),
        "unselecting closes the Move form"
    );

    // Replacing it keeps the form, now for the new target, with a clean session.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Params", cx)));
    assert!(shell.read_with(&cx, |shell, _| shell.move_pill_open()));
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select("Derived", cx)));
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_pill_open()),
        "a new selection keeps the hand's form"
    );
    assert!(
        !shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the new selection starts clean"
    );
}
