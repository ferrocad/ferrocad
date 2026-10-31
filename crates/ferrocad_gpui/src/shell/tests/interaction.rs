//! Shell tests: interaction.

use super::*;
use super::super::context::InteractionContext;

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
