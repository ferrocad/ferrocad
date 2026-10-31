//! Shell tests: the U4 tool loadout (slots, the `Select ▾` filter, and the bar).
//!
//! The slot decides which control may arm and the filter decides the scope the tree,
//! the facade and the arm seam see. These tests drive the shell, not the GPU: the
//! `arm` seam is exercised in `viewport.rs` where a viewport and a claiming
//! manipulator are available.

use super::*;

/// Slot 1 hosts both handle sets, so its bar label names the one the user will get.
#[test]
fn slot_labels_name_the_hand_they_equip() {
    assert_eq!(Slot::Move.label_for(GizmoMode::Translate), "Move");
    assert_eq!(Slot::Move.label_for(GizmoMode::Rotate), "Rotate");
    assert_eq!(Slot::Move.label_for(GizmoMode::Universal), "Move");
    assert_eq!(Slot::PushPull.label_for(GizmoMode::Rotate), "Push/Pull");
    assert_eq!(Slot::Fillet.label_for(GizmoMode::Rotate), "Fillet");
    assert_eq!(Slot::Idle.label_for(GizmoMode::Rotate), "Idle");
    assert_eq!(
        Slot::Idle.status_label(GizmoMode::Rotate),
        "idle",
        "the status line keeps its lower-case idle"
    );
    assert_eq!(
        Slot::ALL,
        [Slot::Move, Slot::PushPull, Slot::Fillet, Slot::Idle],
        "the pointer is the last pill"
    );
}

/// Select `reference` the way a click does, so the shell re-applies the loadout filter
/// and re-arms.
fn click(cx: &mut VisualTestContext, shell: &Entity<Shell>, reference: SelectionRef) {
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_selection(
                &SelectionEvent::Picked {
                    reference,
                    anchor: None,
                    click_count: 1,
                },
                cx,
            )
        })
    });
}

/// The primary selection item (the shell's reported scope).
fn selection(shell: &Entity<Shell>, cx: &VisualTestContext) -> Vec<SelectionItem> {
    shell.read_with(cx, |shell, _| shell.model.lock().unwrap().selection.clone())
}

/// A face click in Slot 1 is a **body** click: the loadout's default filter strips the
/// sub-element from the reported selection, so the triad has a whole object to move.
#[gpui::test]
fn a_face_click_selects_the_body_in_the_move_slot(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout),
        Loadout::default(),
        "the app starts in Slot 1 with its Body filter"
    );

    click(&mut cx, &shell, SelectionRef::element(7, "Face1"));
    assert_eq!(
        selection(&shell, &cx),
        vec![SelectionItem::new("Params")],
        "Select: Body reports the whole body, never Face1"
    );
}

/// Slot 2 is the face hand: the same click keeps `Face1`. The header's `Select ▾` can
/// override the slot's filter independently, and the store shows the override.
#[gpui::test]
fn the_slot_sets_the_filter_and_select_overrides_it(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.filter()),
        SelectFilter::Face
    );
    click(&mut cx, &shell, SelectionRef::element(7, "Face1"));
    assert_eq!(
        selection(&shell, &cx),
        vec![SelectionItem::with_elements("Params", vec!["Face1".into()])],
        "Slot 2 keeps the face it will extrude"
    );

    // A slot change resets the filter to the slot's default...
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::Move, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.filter()),
        SelectFilter::Body
    );
    assert!(!shell.read_with(&cx, |shell, _| shell.loadout.overridden()));

    // ...and `Select ▾` overrides it without leaving the Move hand.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("select:face", cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| (shell.loadout.slot(), shell.loadout.filter())),
        (Slot::Move, SelectFilter::Face),
        "the override changes the filter, not the hand"
    );
    assert!(shell.read_with(&cx, |shell, _| shell.loadout.overridden()));
    click(&mut cx, &shell, SelectionRef::element(7, "Face1"));
    assert_eq!(
        selection(&shell, &cx),
        vec![SelectionItem::with_elements("Params", vec!["Face1".into()])],
        "the override survives and keeps the face"
    );

    // Selecting `Auto` is the pre-loadout behaviour, and the exact spelling is case
    // insensitive so a script or a console line can use it.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("select:auto", cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.filter()),
        SelectFilter::Auto
    );
}

/// The wheel's verbs equip a hand like the slots do: `Rotate` and `Move` both land in
/// Slot 1 (the triad), and `Move` keeps forcing the arrows.
#[gpui::test]
fn the_wheel_verbs_equip_the_move_slot(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::Fillet, cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Fillet
    );

    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.run_command("App_Rotate", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| (shell.loadout.slot(), shell.gizmo_mode)),
        (Slot::Move, GizmoMode::Rotate),
        "Rotate equips the body hand in its ring mode"
    );

    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.run_command("App_Move", cx)));
    assert_eq!(
        shell.read_with(&cx, |shell, _| (shell.loadout.slot(), shell.gizmo_mode)),
        (Slot::Move, GizmoMode::Translate),
        "Move equips the body hand and its arrows"
    );
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.cancel_dialog(cx)));
}

/// The bar is the discoverable, clickable view of the same store: one pill per slot,
/// and a click equips it exactly as its key does.
#[gpui::test]
fn the_loadout_bar_renders_and_equips_a_slot(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);

    assert!(
        cx.debug_bounds("loadout-bar").is_some(),
        "the bar is drawn over the canvas"
    );
    for (key, selector) in [
        ("0", "loadout-0"),
        ("1", "loadout-1"),
        ("2", "loadout-2"),
        ("3", "loadout-3"),
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "slot {key} has a pill");
    }

    click_at(&mut cx, "loadout-2");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::PushPull,
        "a click equips the same slot as `2`"
    );
    // The pointer pill shares the same handler; its own key path is covered by
    // `the_move_and_pointer_slots_are_separate_hands`. Do not click the last pill here:
    // a test-run command registry can widen the centred action dock over it, while the
    // base app's dock stays clear of the bar.
    draw(&mut cx);
    cx.run_until_parked();
    click_at(&mut cx, "loadout-1");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move
    );
}

/// A pending move is explicit state, so a slot change is **refused** while it is
/// unfinished: the drag is neither committed nor dropped and no undo entry appears.
/// Finishing it with `Enter` releases the lock and `2` then equips the face hand.
#[gpui::test]
fn switching_the_slot_waits_for_a_pending_move(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    give_params_a_placement();

    let preview = |phase, x: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, 0.0, 0.0),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 5.0), cx);
        });
    });
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the release left a pending move"
    );

    let before = undo_count();
    // `2` is the slot key, not a digit typed into the Move HUD, even though a gizmo is
    // armed: the loadout claims 0–3. With a move pending it is refused.
    press_root(&mut cx, &shell, "2");

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "the hand does not change under a pending move"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the pending move is neither committed nor dropped"
    );
    assert_eq!(undo_count(), before, "nothing was written");
    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .contains("move pending"),
        "the status says why the key did nothing"
    );

    // The dimmed bar is inert too: a click cannot change the hand either.
    click_at(&mut cx, "loadout-2");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "the bar does not equip a hand under a pending move"
    );

    // `Enter` spends the move; the lock lifts and `2` goes through.
    press_root(&mut cx, &shell, "enter");
    assert_eq!(undo_count(), before + 1, "the move committed once");
    press_root(&mut cx, &shell, "2");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::PushPull,
        "with the move finished the key equips the face hand"
    );
}

/// `1` toggles Slot 1's two hands; `0` is the pointer, a separate slot that keeps the
/// selection but hosts no handles or form.
#[gpui::test]
fn the_move_and_pointer_slots_are_separate_hands(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    let mode = |cx: &VisualTestContext| shell.read_with(cx, |shell, _| shell.gizmo_mode);
    assert_eq!(mode(&cx), GizmoMode::Translate);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "the armed hand shows its form"
    );

    // 1 → Rotate: the same slot, now hosting rings.
    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    let (slot, open, armed, status) = shell.read_with(&cx, |shell, _| {
        let model = shell.model.lock().unwrap();
        (
            shell.loadout.slot(),
            shell.move_hud.is_some(),
            model.gizmo_armed,
            model.status.clone(),
        )
    });
    assert_eq!(slot, Slot::Move, "Slot 1 stays equipped");
    assert_eq!(mode(&cx), GizmoMode::Rotate, "the press swaps to Rotate");
    assert!(open, "the rotate form is the same docked row");
    assert!(armed, "the rings are hosted");
    assert_eq!(status, "tool: Rotate", "the status names the hand");

    // 1 → Move: the same key toggles back instead of cycling into idle.
    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    assert_eq!(mode(&cx), GizmoMode::Translate);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone()),
        "tool: Move"
    );

    // 0 → Idle: its own slot. The selection stays, the handles and form leave.
    press_root(&mut cx, &shell, "0");
    draw(&mut cx);
    let (slot, open, armed, selection, status) = shell.read_with(&cx, |shell, _| {
        let model = shell.model.lock().unwrap();
        (
            shell.loadout.slot(),
            shell.move_hud.is_some(),
            model.gizmo_armed,
            model.selection.len(),
            model.status.clone(),
        )
    });
    assert_eq!(slot, Slot::Idle, "0 equips the pointer, not a paused Move");
    assert!(!open, "the pointer hides the form");
    assert!(!armed, "the pointer hosts no handles");
    assert_eq!(selection, 1, "the selection is kept for inspection");
    assert_eq!(status, "tool: idle", "the pointer names its own state");
    assert!(cx.debug_bounds("move-hud").is_none());

    // 1 → Move: entering Slot 1 from the pointer starts at the arrows.
    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    assert_eq!(mode(&cx), GizmoMode::Translate);
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "Move brings the row back"
    );
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone()),
        "tool: Move"
    );
}
