//! Shell tests: interaction.

use super::*;

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
        shell.update(cx, |shell, _| shell.dialog.is_some()),
        "the Move HUD opened"
    );
    shell.update(cx, |shell, cx| shell.cancel_dialog(cx));
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

/// The Move form is anchored to the visible origin, so the pivot handle is disabled
/// while it is open and re-enabled when the form closes.
#[gpui::test]
fn the_move_hud_disables_the_origin_handle(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert!(shell.read_with(cx, |shell, _| shell.gizmo_origin_enabled()));

    shell.update(cx, |shell, cx| shell.run_command("App_Move", cx));
    assert!(
        !shell.read_with(cx, |shell, _| shell.gizmo_origin_enabled()),
        "the translation HUD owns the anchor"
    );

    shell.update(cx, |shell, cx| shell.cancel_dialog(cx));
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

/// Accepting the Move HUD ends the session it opened: the verb is cleared and the
/// shared triad is dismissed until the next verb or selection.
#[gpui::test]
fn accepting_the_move_hud_ends_the_transform_session(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    shell.update(cx, |shell, cx| shell.select("Params", cx));
    shell.update(cx, |shell, cx| shell.run_command("App_Move", cx));
    assert!(
        shell.read_with(cx, |shell, _| shell.transform_verb),
        "Move is an explicit body-control request"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.dialog.is_some()),
        "Move opens its HUD"
    );

    shell.update(cx, |shell, cx| shell.accept_dialog(cx));
    assert!(
        shell.read_with(cx, |shell, _| shell.dialog.is_none()),
        "the HUD closes on accept"
    );
    assert!(
        !shell.read_with(cx, |shell, _| shell.transform_verb),
        "the verb session ended with the move"
    );
    assert!(
        shell.read_with(cx, |shell, _| shell.triad_dismissed),
        "the triad stays down until the next verb or selection"
    );

    // A new selection re-arms the handles normally.
    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert!(
        !shell.read_with(cx, |shell, _| shell.triad_dismissed),
        "a new selection is a new session"
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
