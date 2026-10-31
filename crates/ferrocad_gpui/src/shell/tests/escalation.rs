//! Shell tests: fast escalation from an armed gizmo into the Move HUD.

use super::*;

/// The focus handle of a Move field by position.
fn axis_focus(shell: &Entity<Shell>, index: usize, cx: &VisualTestContext) -> FocusHandle {
    shell
        .read_with(cx, |shell, cx| {
            shell
                .move_hud
                .as_ref()
                .map(|hud| hud.inputs[index].read(cx).focus_handle(cx))
        })
        .expect("the Move field")
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

    // A spacing edit starts with a digit; it must land in the settings field, not in
    // the Move form (which opens with the selection, unfocused).
    press_root(&mut cx, &shell, "5");

    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "0.00",
        "a digit typed in the settings card must not reach the Move form"
    );
    assert!(
        !shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "no move was started: no escalation happened"
    );
}

#[gpui::test]
fn tab_with_an_armed_gizmo_opens_the_move_hud_at_zero(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    press_root(&mut cx, &shell, "tab");

    let (open, dirty) = shell.read_with(&cx, |shell, _| {
        (
            shell.move_hud.is_some(),
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
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
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
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
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
    let first = axis_focus(&shell, 0, &cx);
    let second = axis_focus(&shell, 1, &cx);
    cx.simulate_keystrokes("tab");
    assert!(
        cx.update(|window, _cx| first.is_focused(window)),
        "Tab opens the resting HUD on the remembered axis"
    );
    cx.simulate_keystrokes("tab");
    assert!(
        cx.update(|window, _cx| second.is_focused(window)),
        "Tab still cycles the open HUD"
    );
}

#[gpui::test]
fn escape_rolls_back_and_keeps_the_move_form(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    // A pending drag: the form exists and its session is dirty.
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
    assert!(shell.read_with(&cx, |shell, _| shell.move_hud.is_some()));

    // The field does not own Esc, so it bubbles to the form and the shell.
    cx.simulate_keystrokes("escape");
    draw(&mut cx);

    let (open, session, armed) = shell.read_with(&cx, |shell, _| {
        (
            shell.move_hud.is_some(),
            shell.session.as_ref().is_some_and(|session| session.is_dirty()),
            shell.model.lock().unwrap().gizmo_armed,
        )
    });
    assert!(open, "the form belongs to the Move hand and stays");
    assert!(!session, "Esc drops the pending move");
    assert!(armed, "the gizmo stays armed");
    assert_eq!(
        fields(&shell, &cx),
        [0.0, 0.0, 0.0],
        "the rolled-back form shows the resting pose"
    );
}

/// U2b, revisited: there is **one** form, not two stages. A drag leaves the Δ fields on
/// screen (they tick live) but takes no keyboard; the first `Tab` focuses the field the
/// drag moved, and further tabs cycle.
#[gpui::test]
fn a_drag_leaves_the_fields_visible_and_tab_focuses_the_drag_axis(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    shell.update(&mut cx, |shell, cx| shell.run_command("App_Move", cx));
    draw(&mut cx);
    assert!(
        !shell.read_with(&cx, |shell, _| shell.pill.has_focus()),
        "the Move HUD opens with no field focused"
    );
    assert!(cx.debug_bounds("move-hud").is_some());

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
        !shell.read_with(&cx, |shell, _| shell.pill.has_focus()),
        "a drag never takes the keyboard"
    );
    assert!(
        cx.debug_bounds("move-hud").is_some(),
        "the fields stay visible through the drag"
    );

    press_root(&mut cx, &shell, "tab");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.focus()),
        Some(0),
        "Tab opens on the axis the drag moved"
    );

    press_root(&mut cx, &shell, "tab");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.focus()),
        Some(1),
        "a focused pill keeps cycling X -> Y"
    );
}

/// The resting fields must not swallow the loadout hotkeys: `2` equips Push/Pull and
/// closes the Move HUD instead of typing into `ΔX`.
#[gpui::test]
fn a_resting_move_hud_lets_a_digit_equip_a_slot(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    shell.update(&mut cx, |shell, cx| shell.run_command("App_Move", cx));
    draw(&mut cx);

    press_root(&mut cx, &shell, "2");
    draw(&mut cx);

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::PushPull,
        "a digit equips the next hand"
    );
    assert!(
        !shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "the Move form belongs to the Move hand and closes with it"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "",
        "nothing was typed into ΔX"
    );
}

/// At rest the form is an inspector: `✓`/`✗` have nothing to spend, so they are absent.
/// A pending move materialises them, and `✓` spends it and returns to rest.
#[gpui::test]
fn the_finish_pair_tracks_whether_a_move_is_pending(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    assert!(shell.read_with(&cx, |shell, _| shell.move_hud.is_some()));
    assert!(
        cx.debug_bounds("dialog-accept").is_none(),
        "at rest there is nothing to ✓"
    );
    assert!(cx.debug_bounds("dialog-cancel").is_none());

    // A typed value makes the move pending: the finish pair appears.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("5", cx));
            shell.on_move_typed(cx);
        });
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("dialog-accept").is_some(), "✓ appears while dirty");
    assert!(cx.debug_bounds("dialog-cancel").is_some(), "✗ appears while dirty");

    // ✓ spends it and the form returns to its resting inspector state.
    click_at(&mut cx, "dialog-accept");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "✓ keeps the always-on form"
    );
    assert!(
        !shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the pending move is spent"
    );
    assert_eq!(fields(&shell, &cx), [0.0, 0.0, 0.0]);
    assert!(
        cx.debug_bounds("dialog-accept").is_none(),
        "back at rest, the pair is gone"
    );
}

/// A half-typed field is a validation error, not an empty move: the "nothing to
/// apply" path must not swallow it.
#[gpui::test]
fn a_half_typed_delta_is_an_error_not_an_empty_move(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("-", cx));
            shell.commit_session(cx);
        });
    });

    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "an invalid field keeps the form open"
    );
    assert!(
        !shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "no half-typed value was committed"
    );
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("check the coordinate values"),
        "the status names the invalid field"
    );
}

/// A click on a ruler tag focuses its Δ field without touching the values.
#[gpui::test]
fn clicking_a_ruler_tag_focuses_its_field(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    shell.update(&mut cx, |shell, cx| shell.run_command("App_Move", cx));
    draw(&mut cx);
    assert!(!shell.read_with(&cx, |shell, _| shell.pill.has_focus()));

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_viewport_event(&ViewportEvent::FocusDelta(2), cx)
        });
    });
    // The event carries no window, so the field takes the keyboard on the next paint.
    draw(&mut cx);

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.pill.focus()),
        Some(2),
        "the tag names the field"
    );
    let dz = axis_focus(&shell, 2, &cx);
    cx.update(|window, _cx| {
        assert!(dz.is_focused(window), "ΔZ takes the keyboard")
    });
    assert_eq!(fields(&shell, &cx), [0.0, 0.0, 0.0], "no value was changed");
}

/// A selection change is a new session even when the old one is clean, so the next
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

    // `Esc` is the rollback: no document write, no pending session, and the form stays
    // (it belongs to the Move hand).
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()
            && shell.move_hud.is_some()),
        "Esc drops the typed move but keeps the form"
    );
}

/// The coordinate mode: `Δ` shows offsets, `XYZ` shows `origin + delta`, and a typed
/// coordinate is turned back into a delta for the preview and the commit.
#[gpui::test]
fn the_coordinate_mode_shows_and_parses_absolute_values(cx: &mut TestAppContext) {
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

    // The item's anchor is its bounding-box minimum: (10, 20, 30), so the absolute
    // fields have a non-zero origin to be measured from.
    let mesh: StageMesh = serde_json::from_str(
        r#"{
            "entity": 7,
            "object": "Params",
            "positions": [[10.0, 20.0, 30.0], [11.0, 20.0, 30.0], [10.0, 21.0, 30.0]],
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
        })
    });
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_pill_open()),
        "the form opens with the selection"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.move_origin()),
        Some(Vector3::new(10.0, 20.0, 30.0)),
        "origin_0 is the gizmo's anchor"
    );
    assert_eq!(fields(&shell, &cx), [0.0, 0.0, 0.0]);

    // The badge is a click target: it flips to world coordinates in place.
    click_at(&mut cx, "move-mode");
    assert!(shell.read_with(&cx, |shell, _| shell.pill.absolute()));
    assert_eq!(
        fields(&shell, &cx),
        [10.0, 20.0, 30.0],
        "absolute mode shows origin_0 + delta"
    );

    // Typing an X coordinate is a delta of `12 - 10`.
    press_root(&mut cx, &shell, "tab");
    cx.simulate_input("12");
    draw(&mut cx);
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([2.0, 0.0, 0.0]),
        "the preview and commit speak deltas"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "12"
    );

    // `=` flips back: the same 2 mm pending offset.
    press_root(&mut cx, &shell, "=");
    assert!(!shell.read_with(&cx, |shell, _| shell.pill.absolute()));
    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "2.00"
    );

    // A `=`-prefixed value is absolute for that field even in offset mode.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let input = shell.move_hud.as_ref().unwrap().inputs[0].clone();
            input.update(cx, |input, cx| input.set_text("=15", cx));
            shell.on_move_typed(cx);
        })
    });
    assert_eq!(
        shell.read_with(&cx, |shell, cx| shell.pending_translation(cx)),
        Some([5.0, 0.0, 0.0]),
        "`=15` snapped X to 15, a +5 offset from 10"
    );

    // `=` with a field focused toggles once: the form's key handler stops the event
    // before the shell root sees it.
    press_root(&mut cx, &shell, "tab");
    cx.simulate_keystrokes("=");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.pill.absolute()),
        "one toggle, not two"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, cx| dialog_field(shell, cx, "dx")),
        "15.00",
        "the pending +5 reads as X = 15"
    );
}

/// The Esc cascade reaches its last rung: a **clean** form does not swallow the key, so
/// Esc drops the selection and the form, triad and tape go with it.
#[gpui::test]
fn escape_at_rest_clears_the_selection(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "the selection shows the form at rest"
    );

    press_root(&mut cx, &shell, "escape");
    draw(&mut cx);

    let (selection, open) = shell.read_with(&cx, |shell, _| {
        (
            shell.model.lock().unwrap().selection.len(),
            shell.move_hud.is_some(),
        )
    });
    assert_eq!(selection, 0, "a clean Esc drops the selection");
    assert!(!open, "and the form goes with it");
    assert!(cx.debug_bounds("move-hud").is_none());
}

/// An absolute coordinate is shown to two decimals. Parsing the rounded text back
/// against an unrounded origin must not look like a pending move: at rest the form
/// stays an inspector (no `✗`/`✓`) and `✓`/`Enter` spends nothing.
#[gpui::test]
fn a_rounded_absolute_field_is_not_a_pending_move(cx: &mut TestAppContext) {
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

    // The anchor (bounding-box minimum) carries a fourth decimal of a millimetre.
    let mesh: StageMesh = serde_json::from_str(
        r#"{
            "entity": 7,
            "object": "Params",
            "positions": [[10.0, 20.004, 30.0], [11.0, 20.004, 30.0], [10.0, 21.004, 30.0]],
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
        })
    });
    draw(&mut cx);

    click_at(&mut cx, "move-mode");
    assert!(shell.read_with(&cx, |shell, _| shell.pill.absolute()));
    assert_eq!(
        fields(&shell, &cx),
        [10.0, 20.0, 30.0],
        "the fields show the origin rounded to two decimals"
    );
    assert!(
        !shell.read_with(&cx, |shell, cx| shell.move_is_pending(cx)),
        "the rounding residue is not a move"
    );
    assert!(
        cx.debug_bounds("dialog-accept").is_none(),
        "no ✓ at rest, even with an unrounded origin"
    );

    let committed = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.commit_session(cx))
    });
    assert!(!committed, "a resting form commits nothing");
    assert_eq!(
        fields(&shell, &cx),
        [10.0, 20.0, 30.0],
        "the reset keeps the absolute readout"
    );
}
