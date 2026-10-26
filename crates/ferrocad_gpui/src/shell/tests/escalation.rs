//! Shell tests: fast escalation from an armed gizmo into the Move HUD.

use super::*;

/// A windowed shell with a selected object and the gizmo "armed".
///
/// The base app links no viewport/workbench, so `arm` cannot build a triad headlessly.
/// `ShellModel::gizmo_armed` is the shell's own "a TransformGizmo is hosted" state and
/// is what the key path consults.
fn armed_window(
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
    visual.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
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

/// The focus handle of a dialog field by position.
fn axis_focus(shell: &Entity<Shell>, index: usize, cx: &VisualTestContext) -> FocusHandle {
    shell
        .read_with(cx, |shell, cx| {
            shell
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.inputs.get(index))
                .and_then(Option::as_ref)
                .map(|input| input.read(cx).focus_handle(cx))
        })
        .expect("the dialog field")
}

/// The three Δ values, parsed.
fn fields(shell: &Entity<Shell>, cx: &VisualTestContext) -> [f64; 3] {
    ["dx", "dy", "dz"].map(|name| {
        shell
            .read_with(cx, |shell, cx| dialog_field(shell, cx, name))
            .parse()
            .unwrap_or(f64::NAN)
    })
}

#[gpui::test]
fn tab_with_an_armed_gizmo_opens_the_move_hud_at_zero(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    press_root(&mut cx, &shell, "tab");

    let (open, deferred) = shell.read_with(&cx, |shell, _| {
        (shell.dialog.is_some(), shell.gizmo_deferred)
    });
    assert!(open, "Tab opens the Move HUD while a gizmo is armed");
    assert!(deferred, "the drag session switched to deferred mode");
    assert_eq!(
        fields(&shell, &cx),
        [0.0, 0.0, 0.0],
        "an idle escalation seeds zero"
    );

    let dx = axis_focus(&shell, 0, &cx);
    cx.update(|window, _cx| {
        assert!(dx.is_focused(window), "the X field takes focus");
    });
}

#[gpui::test]
fn a_digit_opens_the_hud_and_types_the_digit(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    press_root(&mut cx, &shell, "5");

    assert!(
        shell.read_with(&cx, |shell, _| shell.dialog.is_some()),
        "a digit opens the Move HUD"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.gizmo_deferred),
        "the HUD defers the drag"
    );
    // Natural entry replaces the selected `0.00` seed.
    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "5"
    );
    let dx = axis_focus(&shell, 0, &cx);
    cx.update(|window, _cx| {
        assert!(dx.is_focused(window), "the typed field keeps focus");
    });
}

#[gpui::test]
fn a_mid_drag_escalation_seeds_the_drag_and_defers(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    // The viewport reports the accumulated world transform on every deferred tick;
    // mirror it before the HUD exists, as a mid-drag escalation would.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(
                &TransformPreview {
                    target_id: 7,
                    phase: PreviewPhase::Update,
                    total_transform: ferrocad_types::Matrix4::translation(0.0, 7.0, 0.0),
                },
                cx,
            );
        });
    });

    press_root(&mut cx, &shell, "tab");

    assert!(
        shell.read_with(&cx, |shell, _| shell.dialog.is_some()),
        "Tab opens the HUD mid-drag"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.gizmo_deferred),
        "the live drag is now deferred and cannot commit on release"
    );
    assert_eq!(
        fields(&shell, &cx),
        [0.0, 7.0, 0.0],
        "the current drag delta seeds the fields"
    );
    let dy = axis_focus(&shell, 1, &cx);
    cx.update(|window, _cx| {
        assert!(dy.is_focused(window), "the dragged axis takes focus");
    });
}

#[gpui::test]
fn tab_without_a_gizmo_still_cycles_the_open_move_hud(cx: &mut TestAppContext) {
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

    // A selection in the base app hosts no TransformGizmo, so Tab keeps its normal
    // meaning: cycling the open HUD's fields.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Params", cx);
            shell.run_command("App_Move", cx);
        });
    });
    assert!(
        !shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().gizmo_armed),
        "no gizmo is armed"
    );
    draw(&mut cx);
    let second = axis_focus(&shell, 1, &cx);
    cx.simulate_keystrokes("tab");
    assert!(
        cx.update(|window, _cx| second.is_focused(window)),
        "Tab still cycles the open HUD"
    );
}

#[gpui::test]
fn escape_cancels_the_escalated_hud_and_restores_the_gizmo(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    press_root(&mut cx, &shell, "tab");
    assert!(shell.read_with(&cx, |shell, _| shell.dialog.is_some()));
    assert!(shell.read_with(&cx, |shell, _| shell.gizmo_deferred));

    // The existing cancel path: the field does not own Esc, so it bubbles to the HUD
    // and the shell.
    cx.simulate_keystrokes("escape");
    draw(&mut cx);

    let (open, deferred, armed) = shell.read_with(&cx, |shell, _| {
        (
            shell.dialog.is_some(),
            shell.gizmo_deferred,
            shell.model.lock().unwrap().gizmo_armed,
        )
    });
    assert!(!open, "Esc closes the escalated HUD");
    assert!(!deferred, "the gizmo returns to its normal commit-on-release path");
    assert!(armed, "the gizmo stays armed");
}
