//! Shell tests: a released manipulator holds its commit for the finish pair.
//!
//! A push/pull stem or a dress-up scrubber previews a value while it is dragged; the
//! release decides it but must not write it. These tests drive the release through the
//! viewport's event route and check that the document sees the edit exactly once, on
//! `✓` — never on `✗` — and that the pair is the Move form's own.

use super::*;

/// A `push_pull` commit as the stem produces it for the test stage's `Face1`.
fn push_pull_commit_at(distance: f64) -> InteractionCommit {
    let action = if distance < 0.0 { "pocket" } else { "extrude" };
    InteractionCommit::new("push_pull", 7, action)
        .with_element("Face1")
        .with_values(vec![distance])
        .with_readout(format!("{action} {:.2} mm", distance.abs()))
}

/// The commit a first drag at 3 mm produces.
fn push_pull_commit() -> InteractionCommit {
    push_pull_commit_at(3.0)
}

/// Route a commit the way the window's mouse-up handler does.
fn feed_commit(cx: &mut VisualTestContext, shell: &Entity<Shell>, commit: InteractionCommit) {
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_viewport_event(&ViewportEvent::Interaction(commit), cx)
        })
    });
}

/// Install (once) a recording `push_pull`/`extrude` provider and clear its log.
///
/// It claims only that pair, so the section's other tests — which register their own
/// providers in the same interpreter — still route their own commits.
fn install_recorder() {
    let script = r#"
import FerroCAD as _F
if not hasattr(_F, '_mf_calls'):
    _F._mf_calls = []
    def _mf_recorder(obj, manipulator, action, element, values):
        if manipulator != 'push_pull' or action != 'extrude':
            return None
        _F._mf_calls.append([obj.Name if obj is not None else '', element, list(values)])
        return {'ok': True}
    _F.register_interaction_provider(_mf_recorder)
_F._mf_calls.clear()
"#;
    let result = crate::python::evaluate(script).expect("install the recorder");
    assert!(result.ok, "{}", result.output);
}

/// Assert that the provider applied exactly `expected` commits.
///
/// The count is asserted **inside** the interpreter: `evaluate` reports success and the
/// script's own output, not an expression's value.
fn assert_applied(expected: usize) {
    let script = format!(
        "import FerroCAD as _F\nassert len(_F._mf_calls) == {expected}, _F._mf_calls"
    );
    let result = crate::python::evaluate(&script).expect("read the log");
    assert!(
        result.ok,
        "the provider applied the wrong number of commits:\n{}",
        result.output
    );
}

/// Assert that the last applied commit carried `expected` as its first value.
fn assert_last_value(expected: f64) {
    let script = format!(
        "import FerroCAD as _F\n\
         assert _F._mf_calls, 'no commit was applied'\n\
         value = _F._mf_calls[-1][2][0]\n\
         assert abs(value - {expected}) < 1e-9, (value, _F._mf_calls)"
    );
    let result = crate::python::evaluate(&script).expect("read the log");
    assert!(
        result.ok,
        "the applied value is not {expected}:\n{}",
        result.output
    );
}

/// A shell in the push/pull hand with the recorder installed.
fn push_pull_shell(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<Shell>,
    Arc<Mutex<ShellModel>>,
) {
    let (mut vcx, shell, model) = armed_window(cx);
    install_recorder();
    shell.update(&mut vcx, |shell, cx| {
        shell.activate_loadout_slot(Slot::PushPull, cx)
    });
    (vcx, shell, model)
}

/// A release holds the commit: nothing is written, the shell reports it as pending and
/// the `✗`/`✓` pair appears beside the readout.
#[gpui::test]
fn a_released_manipulator_waits_for_the_finish_pair(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = push_pull_shell(cx);
    let undo_before = undo_count();

    feed_commit(&mut cx, &shell, push_pull_commit());

    assert!(
        shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "the release left the commit pending"
    );
    let status = model.lock().unwrap().status.clone();
    assert!(
        status.contains("extrude 3.00 mm pending"),
        "the status names the held value: {status}"
    );
    assert_applied(0);
    assert_eq!(undo_count(), undo_before, "no undo entry was spent");

    draw(&mut cx);
    assert!(
        cx.debug_bounds("manipulator-finish").is_some(),
        "the pair appears with the pending value"
    );
    assert!(cx.debug_bounds("dialog-accept").is_some(), "✓ is offered");
    assert!(cx.debug_bounds("dialog-cancel").is_some(), "✗ is offered");

    // ✓ spends it once, and the pair retires with it.
    click_at(&mut cx, "dialog-accept");
    draw(&mut cx);
    assert!(
        !shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "the commit is spent"
    );
    assert_applied(1);
    assert!(
        cx.debug_bounds("manipulator-finish").is_none(),
        "the pair is gone at rest"
    );
}

/// `✗` drops the held commit: the provider never runs and the document keeps its state.
#[gpui::test]
fn discarding_a_released_manipulator_writes_nothing(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = push_pull_shell(cx);
    let undo_before = undo_count();

    feed_commit(&mut cx, &shell, push_pull_commit());
    draw(&mut cx);
    click_at(&mut cx, "dialog-cancel");
    draw(&mut cx);

    assert!(
        !shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "✗ drops the commit"
    );
    assert_applied(0);
    assert_eq!(undo_count(), undo_before, "nothing was written");
    assert!(
        cx.debug_bounds("manipulator-finish").is_none(),
        "the pair is gone"
    );
    let status = model.lock().unwrap().status.clone();
    assert!(status.contains("discarded"), "the status says so: {status}");
}

/// `Esc` is the same rollback as `✗`, and `Enter` is the same commit as `✓`.
#[gpui::test]
fn the_keyboard_cancels_and_accepts_a_held_manipulator(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = push_pull_shell(cx);

    feed_commit(&mut cx, &shell, push_pull_commit());
    press_root(&mut cx, &shell, "escape");
    assert!(
        !shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "Esc discards"
    );
    assert_applied(0);

    feed_commit(&mut cx, &shell, push_pull_commit());
    press_root(&mut cx, &shell, "enter");
    assert!(
        !shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "Enter applies"
    );
    assert_applied(1);
}

/// A pending manipulator owns the subject: another hand is refused until it is decided,
/// and the refusal names the held value.
#[gpui::test]
fn a_pending_manipulator_refuses_a_hand_switch(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = push_pull_shell(cx);
    feed_commit(&mut cx, &shell, push_pull_commit());

    shell.update(&mut cx, |shell, cx| {
        shell.activate_loadout_slot(Slot::Move, cx)
    });

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::PushPull,
        "the hand did not change"
    );
    let status = model.lock().unwrap().status.clone();
    assert!(
        status.contains("extrude 3.00 mm pending — ✓ or ✗ before you switch tools"),
        "the refusal names the value: {status}"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.interaction_is_pending()),
        "the commit is still held"
    );
}

/// Re-releasing the stem replaces the held commit: an adjustment made after the first
/// release is the value `✓` applies.
#[gpui::test]
fn a_second_release_replaces_the_held_commit(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = push_pull_shell(cx);

    feed_commit(&mut cx, &shell, push_pull_commit());
    feed_commit(&mut cx, &shell, push_pull_commit_at(5.0));

    let status = model.lock().unwrap().status.clone();
    assert!(
        status.contains("extrude 5.00 mm pending"),
        "the held value is the latest drag: {status}"
    );
    assert_applied(0);

    click_at(&mut cx, "dialog-accept");
    assert_applied(1);
    assert_last_value(5.0);
}
