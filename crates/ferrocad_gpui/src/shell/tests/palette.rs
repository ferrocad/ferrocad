//! Shell tests: palette.

use super::*;

/// Ctrl+K opens the palette, and the query filters case-insensitively over the
/// ids (and the labels/tooltips, via the same helper).
#[gpui::test]
fn ctrl_k_opens_the_palette_and_filters_the_commands(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);

    open_palette_with_keyboard(&mut cx, &shell);
    assert!(cx.debug_bounds("palette").is_some());
    assert!(cx.debug_bounds("palette-input").is_some());
    // Nothing is filtered yet, so the whole registry is listed, including a
    // disabled command (it is visible but greyed, like the dock).
    assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
    assert!(cx.debug_bounds("palette-row-App_Delete").is_some());

    // Case-insensitive label match: only the grid command survives.
    cx.simulate_input("GRID");
    draw(&mut cx);
    assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
    assert!(cx.debug_bounds("palette-row-App_Delete").is_none());
    assert!(cx.debug_bounds("palette-row-App_ToggleAxisCross").is_none());

    // The id is searched too, case-insensitively.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("dev_toggletools");
    draw(&mut cx);
    assert!(cx.debug_bounds("palette-row-Dev_ToggleTools").is_some());
    assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_none());

    // A query with no matches shows the empty state instead of a blank panel.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("zzzz-no-such-command");
    draw(&mut cx);
    assert!(cx.debug_bounds("palette-empty").is_some());
}

/// The key path is shared by both windows: Ctrl+K typed while Dev Tools has
/// focus opens the same palette (drawn in the main window), and Esc there closes
/// it too.
#[gpui::test]
fn ctrl_k_opens_the_palette_from_dev_tools(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);
    let mut dev_cx = open_dev_tools(&mut cx, &shell);

    dev_cx.simulate_keystrokes("ctrl-k");
    assert!(
        shell.read_with(&cx, |shell, _| shell.palette.is_some()),
        "Ctrl+K in Dev Tools opens the shared palette"
    );
    draw(&mut cx);
    assert!(cx.debug_bounds("palette").is_some());

    dev_cx.simulate_keystrokes("escape");
    assert!(
        shell.read_with(&cx, |shell, _| shell.palette.is_none()),
        "Esc in Dev Tools closes the palette"
    );
}

/// Down moves the cursor over the filtered rows and Enter runs the highlighted
/// enabled command (the same `run_command` path a click uses).
#[gpui::test]
fn the_palette_arrows_move_the_selection_and_enter_runs_it(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);
    reset_grid_state();

    open_palette_with_keyboard(&mut cx, &shell);
    // Registry order: App_ToggleGrid, App_ToggleAxisCross, Dev_ToggleTools.
    cx.simulate_input("toggle");
    draw(&mut cx);
    assert!(cx.debug_bounds("palette-row-App_ToggleGrid").is_some());
    assert!(cx.debug_bounds("palette-row-App_ToggleAxisCross").is_some());

    let before = model.lock().unwrap().view.clone();
    cx.simulate_keystrokes("down");
    draw(&mut cx);
    cx.simulate_keystrokes("enter");
    draw(&mut cx);

    let after = model.lock().unwrap().view.clone();
    assert_eq!(before.grid, after.grid, "the first row (the grid) did not run");
    assert_ne!(
        before.axis_cross, after.axis_cross,
        "the selected row (the axis cross) ran"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.palette.is_none()),
        "running a command closes the palette"
    );
    assert!(cx.debug_bounds("palette").is_none());
    reset_grid_state();
}

/// Esc closes the palette and hands focus back to whatever held it when Ctrl+K
/// opened it.
#[gpui::test]
fn escape_closes_the_palette_and_restores_focus(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = palette_window(cx);
    let root = shell.read_with(&cx, |shell, _| shell.root_focus.clone());

    open_palette_with_keyboard(&mut cx, &shell);
    cx.simulate_keystrokes("escape");
    draw(&mut cx);

    assert!(
        shell.read_with(&cx, |shell, _| shell.palette.is_none()),
        "Esc closes the palette"
    );
    assert!(cx.debug_bounds("palette").is_none());
    let focused = cx.update(|window, cx| window.focused(cx));
    assert_eq!(focused, Some(root), "focus returns to the opener");
}

/// A disabled command stays listed, but Enter does not run it (and does not close
/// the palette): `App_Delete` is inactive with nothing selected.
#[gpui::test]
fn a_disabled_palette_command_does_not_run(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);

    open_palette_with_keyboard(&mut cx, &shell);
    cx.simulate_input("Delete");
    draw(&mut cx);
    assert!(cx.debug_bounds("palette-row-App_Delete").is_some());

    let objects_before: usize = model
        .lock()
        .unwrap()
        .tree
        .iter()
        .map(|doc| doc.objects.len())
        .sum();
    cx.simulate_keystrokes("enter");
    draw(&mut cx);

    let status = model.lock().unwrap().status.clone();
    assert!(status.contains("disabled"), "status: {status}");
    assert!(
        shell.read_with(&cx, |shell, _| shell.palette.is_some()),
        "a disabled command leaves the palette open"
    );
    let objects_after: usize = model
        .lock()
        .unwrap()
        .tree
        .iter()
        .map(|doc| doc.objects.len())
        .sum();
    assert_eq!(objects_before, objects_after, "the document is untouched");
}
