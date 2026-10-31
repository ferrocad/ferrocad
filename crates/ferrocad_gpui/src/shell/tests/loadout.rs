//! Shell tests: the U4 tool loadout (slots, the `Select ▾` filter, and the bar).
//!
//! The slot decides which control may arm and the filter decides the scope the tree,
//! the facade and the arm seam see. These tests drive the shell, not the GPU: the
//! `arm` seam is exercised in `viewport.rs` where a viewport and a claiming
//! manipulator are available.

use super::*;

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
    for key in ["1", "2", "3"] {
        assert!(
            cx.debug_bounds(match key {
                "1" => "loadout-1",
                "2" => "loadout-2",
                _ => "loadout-3",
            })
            .is_some(),
            "slot {key} has a pill"
        );
    }

    click_at(&mut cx, "loadout-2");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::PushPull,
        "a click equips the same slot as `2`"
    );
    click_at(&mut cx, "loadout-1");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move
    );
}

/// The boot object is an `App::FeaturePython`, which has no `Placement`. Give it one
/// so `_move_selection` can write a real transaction and the undo count is meaningful.
fn give_params_a_placement() {
    let result = crate::python::evaluate(
        r#"
import FreeCAD
obj = FreeCAD.ActiveDocument.getObject('Params')
if obj is not None and 'Placement' not in obj.PropertiesList:
    obj.addProperty('App::PropertyPlacement', 'Placement', 'Base', 'Placement')
"#,
    )
    .expect("add Placement to the boot object");
    assert!(result.ok, "{}", result.output);
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
    // armed: the loadout claims 1/2/3. With a move pending it is refused.
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

/// Pressing the **active** Move hand pauses it: the selection stays, but the triad and
/// the form leave the canvas so the model can be inspected. Pressing it again arms the
/// same hand, and a new selection re-arms it on its own.
#[gpui::test]
fn pressing_the_active_move_hand_pauses_and_rearms_it(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "the armed hand shows its form"
    );

    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    let (slot, open, armed, selection) = shell.read_with(&cx, |shell, _| {
        let model = shell.model.lock().unwrap();
        (
            shell.loadout.slot(),
            shell.move_hud.is_some(),
            model.gizmo_armed,
            model.selection.len(),
        )
    });
    assert_eq!(slot, Slot::Move, "the hand stays equipped");
    assert!(!open, "pausing hides the Move form");
    assert!(!armed, "a paused hand hosts no handles");
    assert_eq!(selection, 1, "the selection is kept for inspection");
    assert!(cx.debug_bounds("move-hud").is_none());

    // The same key arms it again.
    press_root(&mut cx, &shell, "1");
    draw(&mut cx);
    assert!(
        shell.read_with(&cx, |shell, _| shell.move_hud.is_some()),
        "pressing the hand again brings its form back"
    );
    assert!(cx.debug_bounds("move-hud").is_some());
    assert!(
        !shell.read_with(&cx, |shell, _| shell.move_disarmed),
        "the hand is armed, not paused"
    );
}
