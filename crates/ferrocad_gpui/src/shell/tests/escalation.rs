//! Shell tests: fast escalation from an armed gizmo into the Move HUD.

use super::*;

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
fn typing_in_the_settings_card_does_not_open_the_move_hud(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    shell.update(&mut cx, |shell, cx| {
        shell.open_settings_card("Settings/Grid", cx);
    });
    draw(&mut cx);

    // A spacing edit starts with a digit; it must land in the field, not in `dx`.
    press_root(&mut cx, &shell, "5");

    assert!(
        shell.read_with(&cx, |shell, _| shell.dialog.is_none()),
        "a digit typed in the settings card must not open the Move HUD"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()),
        "no session was opened: no escalation happened"
    );
}

#[gpui::test]
fn tab_with_an_armed_gizmo_opens_the_move_hud_at_zero(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    press_root(&mut cx, &shell, "tab");

    let (open, dirty) = shell.read_with(&cx, |shell, _| {
        (
            shell.dialog.is_some(),
            shell.session.as_ref().is_some_and(|session| session.is_dirty()),
        )
    });
    assert!(open, "Tab opens the Move HUD while a gizmo is armed");
    assert!(!dirty, "no drag yet, so the session is clean");
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
    let delta = shell.read_with(&cx, |shell, _| {
        shell.session.as_ref().map(|session| session.delta())
    });
    assert_eq!(
        delta,
        Some(Vector3::new(5.0, 0.0, 0.0)),
        "the typed digit is the session's delta"
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
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the live drag is pending and cannot commit on release"
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

    // The existing cancel path: the field does not own Esc, so it bubbles to the HUD
    // and the shell.
    cx.simulate_keystrokes("escape");
    draw(&mut cx);

    let (open, session, armed) = shell.read_with(&cx, |shell, _| {
        (
            shell.dialog.is_some(),
            shell.session.is_some(),
            shell.model.lock().unwrap().gizmo_armed,
        )
    });
    assert!(!open, "Esc closes the escalated HUD");
    assert!(!session, "Esc drops the pending session too");
    assert!(armed, "the gizmo stays armed");
}

/// U2b: the two stages. A drag folds the pill to its compact readout; `Tab` unfolds it
/// again and then keeps cycling the fields, without losing the axis the drag moved.
#[gpui::test]
fn a_drag_collapses_the_pill_and_tab_expands_it(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    shell.update(&mut cx, |shell, cx| shell.run_command("App_Move", cx));
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.pill.is_expanded()),
        "an explicitly opened Move pill shows its fields"
    );
    assert!(cx.debug_bounds("move-pill-expanded").is_some());

    let preview = |phase, x: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, 0.0, 0.0),
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 5.0), cx);
        });
    });
    draw(&mut cx);
    assert!(
        !shell.read_with(&cx, |shell, _| shell.pill.is_expanded()),
        "a drag is the compact stage"
    );
    assert!(cx.debug_bounds("move-pill-compact").is_some());
    assert!(cx.debug_bounds("move-pill-expanded").is_none());

    press_root(&mut cx, &shell, "tab");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.pill.is_expanded()),
        "Tab escalates the compact readout to the fields"
    );
    assert!(cx.debug_bounds("move-pill-expanded").is_some());
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.focus()),
        0,
        "escalation keeps the field the drag started on"
    );

    press_root(&mut cx, &shell, "tab");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.focus()),
        1,
        "once expanded, Tab cycles X -> Y"
    );
}

/// U3: a selection change is a new session even when the old one is clean, so the next
/// drag cannot inherit its datum or targets (and its tape cannot outlive the selection).
#[gpui::test]
fn a_selection_change_drops_a_clean_session(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    shell.update(&mut cx, |shell, cx| shell.run_command("App_Move", cx));
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_some()),
        "the pill opens a session"
    );

    shell.update(&mut cx, |shell, cx| shell.select("Derived", cx));
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()),
        "a new selection drops even a clean session"
    );
}

/// U2b: a typed-only edit opens the session. Before this, typing had no session of its
/// own until a drag.
#[gpui::test]
fn a_typed_edit_opens_the_session(cx: &mut TestAppContext) {
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

    // A stage gives the session its datum; the object is the boot document's `Params`,
    // so the command stays active and the selection is a real object.
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
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| !session.is_dirty())),
        "opening the pill opens a clean session"
    );

    // The first field is focused and its seed is selected: `5` replaces it.
    cx.simulate_keystrokes("5");
    draw(&mut cx);
    let delta = shell.read_with(&cx, |shell, _| shell.session.as_ref().map(|s| s.delta()));
    assert_eq!(
        delta,
        Some(Vector3::new(5.0, 0.0, 0.0)),
        "the typed value is the session's delta"
    );

    // `Esc` is the rollback: no document write, no session, no dialog.
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none() && shell.dialog.is_none()),
        "Esc drops the typed session and its pill"
    );
}
