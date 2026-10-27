//! Shell tests: toolbar.

use super::*;

/// T0/T6: every command the registry currently reports is on exactly one surface,
/// and the shipped metadata matches the documented mapping.
#[gpui::test]
fn every_registered_command_lands_on_exactly_one_surface(_cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _model = boot_shell();

    // A mod that predates the metadata must still appear, on the dock.
    let script = r#"
import FreeCADGui
class _Unannotated:
    def Activated(self):
        return {"status": "unannotated ran"}
if "Test_Unannotated" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Unannotated", _Unannotated())
"#;
    let registered = crate::python::evaluate(script).expect("evaluate");
    assert!(registered.ok, "{}", registered.output);

    let commands = crate::python::commands().expect("the command list");
    let groups = crate::commands::group_by_surface(&commands);
    assert_eq!(
        groups.all().len(),
        commands.len(),
        "every command lands on one surface"
    );
    let mut ids: Vec<&str> = groups.all().iter().map(|command| command.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), commands.len(), "no command is on two surfaces");

    let find = |id: &str| {
        commands
            .iter()
            .find(|command| command.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    use crate::commands::Surface;
    assert_eq!(find("App_ToggleGrid").surface(), Surface::Grid);
    assert_eq!(find("App_ToggleAxisCross").surface(), Surface::Axis);
    assert_eq!(find("App_ViewWorkplane").surface(), Surface::Workplane);
    assert_eq!(find("Mods_Settings").surface(), Surface::Mods);
    assert_eq!(find("App_Delete").surface(), Surface::Radial);
    assert_eq!(find("App_Move").surface(), Surface::Radial);
    assert_eq!(
        find("Dev_ToggleTools").surface(),
        Surface::Hidden,
        "the Dev Tools toggle is chrome, not a workbench action"
    );
    assert_eq!(
        find("View_GridSettings").parent(),
        Some("App_ToggleGrid")
    );
    assert_eq!(
        find("App_ReloadGridSettings").parent(),
        Some("App_ToggleGrid")
    );
    assert_eq!(
        find("App_ActivateWorkplane").parent(),
        Some("App_ViewWorkplane")
    );
    assert_eq!(find("Test_Unannotated").surface(), Surface::Dock);
}

/// T1/T2: the header split button's label runs the primary command; the caret
/// opens the popover without running it; `Esc` and an outside click dismiss it.
#[gpui::test]
fn the_grid_split_button_runs_the_label_and_the_caret_opens(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
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

    // The controls exist in the floating pill at the canvas' top-left, ordered
    // left to right. This is a compact cluster, not a full-width header row.
    let width = cx.update(|window, _| window.viewport_size().width);
    let pane = cx.debug_bounds("main-viewport").expect("the viewport pane");
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    let header = cx.debug_bounds("viewport-header").expect("the control pill");
    let grid = cx.debug_bounds("grid-label").expect("the Grid label");
    let axis = cx.debug_bounds("axis-label").expect("the Axes label");
    let wp = cx.debug_bounds("wp-label").expect("the WP label");
    assert!(grid.origin.x < axis.origin.x);
    assert!(axis.origin.x < wp.origin.x);
    assert!(
        cx.debug_bounds("mods-label").is_none(),
        "the Mods control left the header"
    );
    assert!(
        cx.debug_bounds("mods-caret").is_none(),
        "the Mods caret left the header"
    );
    assert!(
        header.origin.x < width * 0.5,
        "the pill sits at the leading edge (at {:?}, width {width:?})",
        header.origin.x
    );
    assert!(
        header.size.width < canvas.size.width,
        "the pill is compact, not a full-width header row (pill {:?}, canvas {:?})",
        header.size.width,
        canvas.size.width
    );
    assert!(
        header.origin.x >= canvas.origin.x
            && header.origin.x < canvas.origin.x + px(16.)
            && header.origin.y >= canvas.origin.y
            && header.origin.y < canvas.origin.y + px(16.),
        "the pill floats over the canvas' top-left (pill {:?}, canvas {:?}, pane {:?})",
        header.origin,
        canvas.origin,
        pane.origin
    );
    for control in [&grid, &axis, &wp] {
        assert!(
            header.contains(&control.center()),
            "the pill owns every control"
        );
    }
    assert!(
        cx.debug_bounds("grid-state-dot").is_some(),
        "the Grid control carries a persistent on/off dot"
    );

    // The caret opens the grid popover and does not toggle the grid.
    assert!(!model.lock().unwrap().view.grid);
    click_at(&mut cx, "grid-caret");
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
        "the caret opened the popover"
    );
    assert!(!model.lock().unwrap().view.grid, "opening ran no command");
    draw(&mut cx);
    let popover = cx.debug_bounds("grid-popover").expect("the grid popover");
    let caret = cx.debug_bounds("grid-caret").expect("the caret");
    assert!(
        popover.origin.y >= caret.origin.y + caret.size.height,
        "the popover drops below the control (popover {:?}, caret {:?})",
        popover.origin,
        caret.origin
    );
    assert!(
        popover.origin.x + popover.size.width <= width + px(8.),
        "the popover stays inside the window (right edge {:?})",
        popover.origin.x + popover.size.width
    );
    assert!(
        cx.debug_bounds("grid-row-ferrocad-provider:grid:ferrocad").is_some(),
        "the provider switch rows lead the popover"
    );
    assert!(cx.debug_bounds("grid-row-ferrocad-provider:grid:coin").is_some());
    assert!(
        cx.debug_bounds("grid-row-View_GridSettings").is_some(),
        "the popover lists the grid surface's children"
    );
    assert!(cx.debug_bounds("grid-row-App_ReloadGridSettings").is_some());
    assert!(
        cx.debug_bounds("grid-row-GridOverlay_Settings").is_none(),
        "a dev-only backend settings command is filtered from the popover"
    );
    let providers_row = cx
        .debug_bounds("grid-row-ferrocad-provider:grid:ferrocad")
        .expect("the provider row");
    let settings_row = cx
        .debug_bounds("grid-row-View_GridSettings")
        .expect("the settings row");
    assert!(
        providers_row.origin.y < settings_row.origin.y,
        "provider rows come before the generic settings rows"
    );
    assert!(
        cx.debug_bounds("grid-hairline-2").is_some(),
        "the provider group and the config group have a hairline"
    );

    // Esc dismisses it and leaves the grid alone.
    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
    assert!(!model.lock().unwrap().view.grid);

    // A right-click on the label opens the popover and never runs the command.
    let label = cx.debug_bounds("grid-label").expect("the Grid label");
    cx.simulate_mouse_down(
        label.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        label.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::default(),
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
        "a right-click opened the popover"
    );
    assert!(!model.lock().unwrap().view.grid, "and ran nothing");
    cx.simulate_keystrokes("escape");

    // An outside click dismisses it too (the viewport keeps the event: the
    // popover has no backdrop).
    draw(&mut cx);
    click_at(&mut cx, "grid-caret");
    assert!(cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
    draw(&mut cx);
    cx.simulate_click(gpui::point(px(200.), px(300.)), gpui::Modifiers::default());
    assert!(
        !cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
        "an outside click dismissed the popover"
    );

    // One popover at a time: opening the workplane menu replaces the grid one.
    draw(&mut cx);
    click_at(&mut cx, "grid-caret");
    draw(&mut cx);
    click_at(&mut cx, "wp-caret");
    assert!(cx.update(|_window, cx| shell.read(cx).popover_open("wp")));
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
    draw(&mut cx);
    assert!(cx.debug_bounds("wp-popover").is_some());
    assert!(cx.debug_bounds("grid-popover").is_none());
    assert!(cx.debug_bounds("wp-row-App_ActivateWorkplane").is_some());
    assert!(cx.debug_bounds("wp-row-App_ClearWorkplane").is_some());

    // The label runs the primary command in one click and closes the menu.
    click_at(&mut cx, "grid-label");
    assert!(model.lock().unwrap().view.grid, "the label toggled the grid");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
    click_at(&mut cx, "grid-label");
    assert!(!model.lock().unwrap().view.grid, "and back off");

    // The Axes control is a split button; its label runs the checkable command.
    assert!(model.lock().unwrap().view.axis_cross);
    click_at(&mut cx, "axis-label");
    assert!(!model.lock().unwrap().view.axis_cross);
}

/// T1/T4: the `Axes` label runs `App_ToggleAxisCross`; the caret opens the axis
/// popover without toggling. The popover lists both backend settings rows and the
/// reload command, and the control keeps a persistent state dot.
#[gpui::test]
fn the_axis_split_button_runs_the_label_and_the_caret_opens(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    // The Python facade owns `App_ReloadAxisSettings` (axis surface, parent
    // `App_ToggleAxisCross`). Until that registration lands, install the same
    // metadata so this test still exercises the popover wiring; when the real
    // command is present this branch is skipped and the real one is asserted.
    let reload_registered = crate::python::commands()
        .expect("the command list")
        .iter()
        .any(|command| command.id == "App_ReloadAxisSettings");
    if !reload_registered {
        let script = r#"
import FreeCADGui
class _TestReloadAxisSettings:
    def GetResources(self):
        return {
            "MenuText": "Reload axis settings",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
        }
    def IsActive(self):
        return True
    def Activated(self):
        return {"status": "axis settings reloaded"}
if "App_ReloadAxisSettings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("App_ReloadAxisSettings", _TestReloadAxisSettings())
"#;
        let registered = crate::python::evaluate(script).expect("register the stand-in");
        assert!(registered.ok, "{}", registered.output);
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.refresh_commands(cx));
        });
    }
    draw(&mut cx);

    // The split button carries its own persistent on/off dot.
    assert!(
        cx.debug_bounds("axis-state-dot").is_some(),
        "the Axes control carries a state dot"
    );
    assert!(cx.debug_bounds("axis-label").is_some());
    assert!(cx.debug_bounds("axis-caret").is_some());
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));

    // The caret opens the popover and does not toggle the cross.
    assert!(model.lock().unwrap().view.axis_cross, "on by default");
    click_at(&mut cx, "axis-caret");
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("axis")),
        "the caret opened the axis popover"
    );
    assert!(
        model.lock().unwrap().view.axis_cross,
        "opening ran no command"
    );
    draw(&mut cx);
    assert!(cx.debug_bounds("axis-popover").is_some());
    assert!(
        cx.debug_bounds("axis-row-ferrocad-provider:axis:ferrocad").is_some(),
        "the native provider switch row leads the popover"
    );
    assert!(
        cx.debug_bounds("axis-row-ferrocad-provider:axis:coin").is_some(),
        "the Coin provider switch row is offered"
    );
    assert!(
        cx.debug_bounds("axis-row-AxisOverlay_Settings").is_none(),
        "the native backend's settings command is dev-only"
    );
    assert!(
        cx.debug_bounds("axis-row-AxisOverlayCoin_Settings").is_none(),
        "the Coin backend's settings command is dev-only"
    );
    assert!(
        cx.debug_bounds("axis-row-App_ReloadAxisSettings").is_some(),
        "the axis surface's reload command"
    );
    assert!(
        cx.debug_bounds("axis-row-View_AxisSettings").is_some(),
        "the axis surface's settings command"
    );
    let providers_row = cx
        .debug_bounds("axis-row-ferrocad-provider:axis:ferrocad")
        .expect("the provider row");
    let reload_row = cx
        .debug_bounds("axis-row-App_ReloadAxisSettings")
        .expect("the reload row");
    let settings_row = cx
        .debug_bounds("axis-row-View_AxisSettings")
        .expect("the settings row");
    assert!(
        providers_row.origin.y < settings_row.origin.y
            && providers_row.origin.y < reload_row.origin.y,
        "provider rows come before the generic settings rows"
    );

    // Esc dismisses it and leaves the cross alone.
    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));
    assert!(model.lock().unwrap().view.axis_cross);

    // The label runs the primary command and closes the menu.
    click_at(&mut cx, "axis-label");
    assert!(
        !model.lock().unwrap().view.axis_cross,
        "the label toggled the cross"
    );
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("axis")));
    click_at(&mut cx, "axis-label");
    assert!(model.lock().unwrap().view.axis_cross, "and back on");

    // Leave the shared interpreter on the default cross for later tests.
    if !reload_registered {
        let cleanup = crate::python::evaluate(
            "import FreeCADGui\nFreeCADGui.removeCommand('App_ReloadAxisSettings')",
        )
        .expect("remove the stand-in");
        assert!(cleanup.ok, "{}", cleanup.output);
    }
    reset_grid_state();
}

/// The snap filter set is spatial ambient state, so it lives beside Grid/Axes/WP
/// in the header, not in the action dock. The label is the global switch and the
/// popover is the six-mode checklist with de-noised labels.
#[gpui::test]
fn the_snap_control_lives_in_the_header_and_not_the_dock(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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

    // The control is the last item in the spatial pill, after WP.
    let header = cx.debug_bounds("viewport-header").expect("the control pill");
    let wp = cx.debug_bounds("wp-label").expect("the WP label");
    let snap = cx.debug_bounds("snap-label").expect("the Snap label");
    assert!(wp.origin.x < snap.origin.x, "Snap follows WP");
    assert!(
        header.contains(&snap.center()),
        "the pill owns the Snap control"
    );
    assert!(
        cx.debug_bounds("snap-state-dot").is_some(),
        "the Snap control carries a persistent on/off dot"
    );

    // The six modes are on the snap surface and parented to the switch, so the
    // dock does not render them any more.
    let command_ids: Vec<String> = {
        let model = model.lock().unwrap();
        model.commands.iter().map(|command| command.id.clone()).collect()
    };
    for id in [
        "Draft_Snap_Lock",
        "Draft_Snap_Grid",
        "Draft_Snap_Endpoint",
        "Draft_Snap_Midpoint",
        "Draft_Snap_Center",
        "Draft_Snap_Origin",
        "Draft_Snap_Vertex",
    ] {
        assert!(
            command_ids.iter().any(|known| known.as_str() == id),
            "{id} is registered"
        );
    }
    {
        let model = model.lock().unwrap();
        let lock = crate::commands::find(&model.commands, "Draft_Snap_Lock")
            .expect("the lock command");
        assert_eq!(lock.surface(), crate::commands::Surface::Snap);
        let grid_mode = crate::commands::find(&model.commands, "Draft_Snap_Grid")
            .expect("the Grid mode");
        assert_eq!(grid_mode.surface(), crate::commands::Surface::Snap);
        assert_eq!(grid_mode.parent(), Some("Draft_Snap_Lock"));
        assert_eq!(grid_mode.short_label(), "Grid");
    }
    assert!(
        cx.debug_bounds("dock-Draft_Snap_Grid").is_none(),
        "the snap modes left the action dock"
    );
    assert!(
        cx.debug_bounds("dock-App_Delete").is_none(),
        "the selection verbs are radial slots, not dock rows"
    );
    assert!(
        cx.debug_bounds("dock-search").is_some(),
        "the dock still carries the global search slot"
    );

    // Opening the caret does not toggle the switch.
    fn snapping_enabled() -> bool {
        let result = crate::python::evaluate(
            "import FreeCADGui\nprint(FreeCADGui.Snapper.isEnabled())",
        )
        .expect("read the snap switch");
        assert!(result.ok, "{}", result.output);
        result.output.trim() == "True"
    }
    let before = snapping_enabled();
    click_at(&mut cx, "snap-caret");
    assert!(cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
    assert_eq!(snapping_enabled(), before, "the caret ran no command");

    // A checkable row toggles in place: the menu stays open and the tick follows.
    fn mode_enabled(mode: &str) -> bool {
        let result = crate::python::evaluate(&format!(
            "import FreeCADGui\nprint(FreeCADGui.Snapper.isEnabled({mode:?}))"
        ))
        .expect("read the snap mode");
        assert!(result.ok, "{}", result.output);
        result.output.trim() == "True"
    }
    draw(&mut cx);
    // Every mode row carries the pill's left-aligned status dot; the old
    // right-aligned checkmark is gone.
    for dot in [
        "snap-row-Draft_Snap_Grid-dot",
        "snap-row-Draft_Snap_Endpoint-dot",
        "snap-row-Draft_Snap_Midpoint-dot",
        "snap-row-Draft_Snap_Center-dot",
        "snap-row-Draft_Snap_Origin-dot",
        "snap-row-Draft_Snap_Vertex-dot",
    ] {
        assert!(cx.debug_bounds(dot).is_some(), "{dot} exists");
    }
    assert!(
        cx.debug_bounds("snap-row-Draft_Snap_Grid").is_some(),
        "the Grid mode is a row"
    );
    let grid_mode = mode_enabled("Grid");
    click_at(&mut cx, "snap-row-Draft_Snap_Grid");
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("snap")),
        "toggling a checkable row keeps the menu open"
    );
    assert_eq!(mode_enabled("Grid"), !grid_mode, "the tick follows the mask");
    click_at(&mut cx, "snap-row-Draft_Snap_Grid");
    assert_eq!(mode_enabled("Grid"), grid_mode, "and back");

    // Closing stays explicit: Esc dismisses the menu and runs no command.
    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
    assert_eq!(snapping_enabled(), before, "Esc ran no command");

    // The label is the global switch, and each click flips it.
    click_at(&mut cx, "snap-label");
    assert_eq!(snapping_enabled(), !before);
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("snap")));
    click_at(&mut cx, "snap-label");
    assert_eq!(snapping_enabled(), before, "and back");
}

/// A row that is not a toggle is a one-shot action, so it still dismisses the
/// menu (the checkable-row exception must not leak into actions).
#[gpui::test]
fn a_one_shot_popover_row_still_closes_the_menu(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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

    click_at(&mut cx, "grid-caret");
    draw(&mut cx);
    assert!(cx.update(|_window, cx| shell.read(cx).popover_open("grid")));
    assert!(
        cx.debug_bounds("grid-row-App_ReloadGridSettings").is_some(),
        "the reload row is on the grid surface"
    );
    // A provider row is a toggle (it shows the active backend); a one-shot row
    // has no status dot.
    assert!(
        cx.debug_bounds("grid-row-ferrocad-provider:grid:ferrocad-dot")
            .is_some(),
        "the provider row carries a status dot"
    );
    assert!(
        cx.debug_bounds("grid-row-App_ReloadGridSettings-dot")
            .is_none(),
        "a one-shot action has no dot"
    );

    click_at(&mut cx, "grid-row-App_ReloadGridSettings");
    assert!(
        !cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
        "a one-shot row dismisses the menu"
    );
}

/// The canvas owns the full pane: there is no full-width header row above it, and
/// the environment controls float as a compact pill over its top-left corner.
#[gpui::test]
fn the_canvas_starts_at_the_pane_top_and_the_controls_float_over_it(
    cx: &mut TestAppContext,
) {
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
    draw(&mut cx);

    let pane = cx.debug_bounds("main-viewport").expect("the viewport pane");
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    let pill = cx.debug_bounds("viewport-header").expect("the control pill");

    // The canvas is the pane's first row: no header pushes the scene down.
    assert_eq!(
        canvas.origin.y, pane.origin.y,
        "the canvas starts at the pane top (canvas {canvas:?}, pane {pane:?})"
    );
    assert_eq!(
        canvas.size.height, pane.size.height,
        "the canvas fills the pane (canvas {canvas:?}, pane {pane:?})"
    );

    // The pill overlays the canvas' top-left corner, inside it and compact.
    assert!(
        pill.origin.x >= canvas.origin.x
            && pill.origin.x < canvas.origin.x + px(16.)
            && pill.origin.y >= canvas.origin.y
            && pill.origin.y < canvas.origin.y + px(16.),
        "the pill floats at the canvas' top-left (pill {pill:?}, canvas {canvas:?})"
    );
    assert!(
        pill.origin.x + pill.size.width <= canvas.origin.x + canvas.size.width
            && pill.origin.y + pill.size.height <= canvas.origin.y + canvas.size.height,
        "the pill stays inside the canvas (pill {pill:?}, canvas {canvas:?})"
    );
    assert!(
        pill.size.width < canvas.size.width,
        "the pill is not a full-width header row (pill {pill:?}, canvas {canvas:?})"
    );
}

/// T4: the Grid popover reports the active provider and spacing above its
/// command rows, and follows a provider switch.
#[gpui::test]
fn the_grid_popover_reports_the_active_provider_and_spacing(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
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

    click_at(&mut cx, "grid-caret");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("grid-row-provider").is_some(),
        "the popover reports the provider above its commands"
    );
    assert!(
        cx.debug_bounds("grid-row-spacing").is_some(),
        "and the active provider's spacing"
    );
    assert!(
        cx.debug_bounds("grid-row-no-provider").is_none(),
        "there is a provider"
    );
    {
        let summary = cx
            .update(|_window, cx| shell.read(cx).grid_summary.clone())
            .expect("opening the popover fetched the summary");
        assert_eq!(summary.feature, "grid");
        assert_eq!(summary.backend.as_deref(), Some("ferrocad"));
        assert_eq!(summary.label.as_deref(), Some("FerroCAD (native)"));
        assert_eq!(summary.spacing.as_deref(), Some("10 mm"));
        assert!(
            summary
                .providers
                .iter()
                .any(|provider| provider.backend == "coin" && !provider.active),
            "the other backend is listed but not active"
        );
    }
    // A settings command is an **action**, not a checkable state row (the
    // provider rows above it carry the active dot).
    {
        let model = model.lock().unwrap();
        let checked = |id: &str| {
            model
                .commands
                .iter()
                .find(|command| command.id == id)
                .map(|command| command.checked)
                .unwrap_or(false)
        };
        assert!(
            !checked("GridOverlay_Settings"),
            "the settings row is an action, not a checked mode"
        );
        assert!(!checked("GridOverlayCoin_Settings"));
    }

    // Switching the provider through the Settings node's selector refreshes
    // the cached summary, and an open popover follows it.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.commit_property("Settings/Grid.provider", "Coin (WGSL)", cx);
        });
    });
    draw(&mut cx);
    {
        let summary = cx
            .update(|_window, cx| shell.read(cx).grid_summary.clone())
            .expect("the settings edit refreshed the summary");
        assert_eq!(summary.backend.as_deref(), Some("coin"));
        assert_eq!(summary.label.as_deref(), Some("Coin (WGSL)"));
    }
    assert!(
        cx.debug_bounds("grid-row-provider").is_some(),
        "the provider row is still there after the switch"
    );

    // Leave the shared interpreter on the default provider for later tests.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.commit_property("Settings/Grid.provider", "FerroCAD (native)", cx);
        });
    });
    reset_grid_state();
}

/// T4: a provider row in the Grid/Axes popover switches the active backend
/// through the synthetic id and applies the returned view; no registry command
/// is involved.
#[gpui::test]
fn provider_rows_switch_the_active_backend(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
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

    // The feature owns the switch: turn the grid on explicitly. A provider hook
    // selects an implementation and claims the drawing; it must not change
    // visibility, so the overlay assertion below is about drawing, not enabling.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
    });
    draw(&mut cx);
    assert!(
        model.lock().unwrap().view.grid,
        "the grid is on before the provider switch"
    );

    // Grid: the Coin row switches the backend and closes the menu.
    click_at(&mut cx, "grid-caret");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("grid-row-ferrocad-provider:grid:coin")
            .is_some(),
        "the coin backend is offered"
    );
    click_at(&mut cx, "grid-row-ferrocad-provider:grid:coin");
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("grid")),
        "a provider choice is a state change, so the popover stays open"
    );
    {
        let summary = cx
            .update(|_window, cx| shell.read(cx).grid_summary.clone())
            .expect("the switch refreshed the summary");
        assert_eq!(summary.backend.as_deref(), Some("coin"));
        assert!(
            summary
                .providers
                .iter()
                .any(|provider| provider.backend == "coin" && provider.active),
            "the summary follows the switch"
        );
    }
    {
        let m = model.lock().unwrap();
        assert!(
            m.status.contains("coin is the active provider"),
            "the provider status is reported: {}",
            m.status
        );
        assert!(
            m.view.grid_overlay,
            "the provider's overlay claim was applied"
        );
        assert!(
            m.view.grid,
            "switching a provider must not change the visibility switch"
        );
    }

    // Switch back through the same intercept.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("ferrocad-provider:grid:ferrocad", cx)
        })
    });
    {
        let summary = cx
            .update(|_window, cx| shell.read(cx).grid_summary.clone())
            .expect("the switch back refreshed the summary");
        assert_eq!(summary.backend.as_deref(), Some("ferrocad"));
        assert!(!model.lock().unwrap().view.grid_overlay);
    }

    // Axes: the Coin row does the same on the axis feature.
    click_at(&mut cx, "axis-caret");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("axis-row-ferrocad-provider:axis:coin")
            .is_some(),
        "the coin axis backend is offered"
    );
    click_at(&mut cx, "axis-row-ferrocad-provider:axis:coin");
    assert!(
        cx.update(|_window, cx| shell.read(cx).popover_open("axis")),
        "the axis provider is a state row too, so the popover stays open"
    );
    {
        let summary = cx
            .update(|_window, cx| shell.read(cx).axis_summary.clone())
            .expect("the switch refreshed the axis summary");
        assert_eq!(summary.backend.as_deref(), Some("coin"));
    }
    assert!(
        model.lock().unwrap().view.axis.overlay,
        "the axis provider's claim was applied"
    );

    // Leave the shared interpreter on the default providers for later tests.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("ferrocad-provider:axis:ferrocad", cx)
        })
    });
    assert!(!model.lock().unwrap().view.axis.overlay);
    reset_grid_state();
}

/// T5/S2: `View_GridSettings` asks the shell to select the synthetic
/// `Settings/Grid` node, and the existing Properties pane renders its rows.
/// A settings entry in a header menu is an **action** row: clicking it closes
/// the menu, opens the card for that feature, and leaves the selection alone.
#[gpui::test]
fn a_settings_menu_row_closes_the_popover(cx: &mut TestAppContext) {
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
        shell.update(cx, |shell, cx| shell.select("Params", cx))
    });
    draw(&mut cx);

    click_at(&mut cx, "gizmo-caret");
    draw(&mut cx);
    assert!(cx.debug_bounds("gizmo-popover").is_some());

    let settings_row = if cx
        .debug_bounds("gizmo-row-GizmoOverlay_Settings")
        .is_some()
    {
        "gizmo-row-GizmoOverlay_Settings"
    } else {
        "gizmo-row-GizmoOverlayShader_Settings"
    };
    click_at(&mut cx, settings_row);
    draw(&mut cx);

    assert!(
        cx.debug_bounds("gizmo-popover").is_none(),
        "choosing a settings entry closes the menu"
    );
    let node = cx.update(|_window, cx| {
        shell
            .read(cx)
            .settings_card
            .as_ref()
            .map(|card| card.node.clone())
    });
    assert_eq!(
        node.as_deref(),
        Some("Settings/Gizmo"),
        "the card opened for the gizmo"
    );
    let selected = model.lock().unwrap().selected.clone();
    assert_eq!(
        selected.as_deref(),
        Some("Params"),
        "the object stayed selected"
    );
}

/// `View_GridSettings` opens the floating card **without** changing the
/// selection: a settings panel is a tool, so the edited object keeps its
/// selection and its gizmo.
#[gpui::test]
fn view_grid_settings_opens_the_card_without_selecting(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
    // The command returns the directive; `FreeCADGui._payload` must forward it.
    let forwarded = crate::commands::run("View_GridSettings").expect("run the command");
    assert_eq!(
        forwarded.select.as_deref(),
        Some("Settings/Grid"),
        "the bridge forwards the command's select directive"
    );

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
        shell.update(cx, |shell, cx| shell.select("Params", cx))
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });

    {
        let m = model.lock().unwrap();
        assert_eq!(
            m.selected.as_deref(),
            Some("Params"),
            "opening settings keeps the object selected"
        );
        assert!(
            m.selection.iter().any(|item| item.is("Params")),
            "the selection list is untouched"
        );
        assert!(
            m.status.contains("Params"),
            "no settings node replaced the status: {}",
            m.status
        );
    }
    let (node, rows) = cx.update(|_window, cx| {
        let shell = shell.read(cx);
        let card = shell.settings_card.as_ref().expect("the settings card");
        (card.node.clone(), card.rows.clone())
    });
    assert_eq!(node, "Settings/Grid", "the card edits the requested node");
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert!(names.contains(&"provider"), "rows: {names:?}");
    assert!(names.contains(&"minor"), "rows: {names:?}");
    assert!(
        cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
        "the select directive is not the modal fallback"
    );

    // The modeless card is the visible settings surface (A3/T5): it renders one
    // field per editable row, without opening the Dev Tools window.
    draw(&mut cx);
    let card = cx.debug_bounds("settings-card").expect("the settings card");
    assert_eq!(card.size.width, px(380.), "the card is 380 px wide");
    assert!(cx.debug_bounds("settings-card-header").is_some());
    assert!(cx.debug_bounds("settings-card-close").is_some());
    let keys = cx.update(|_window, cx| {
        let mut keys: Vec<String> = shell.read(cx).prop_inputs.keys().cloned().collect();
        keys.sort();
        keys
    });
    assert!(
        cx.debug_bounds("prop-enum-Settings/Grid.provider-select")
            .is_some(),
        "the provider row is a select box"
    );
    assert!(
        cx.debug_bounds("prop-bool-Settings/Grid.enabled-toggle")
            .is_some(),
        "the enabled row is a toggle switch"
    );
    assert!(
        keys.iter().any(|key| key == "Settings/Grid.minor"),
        "fields: {keys:?}"
    );

    // The header drags the card; the shell root follows the pointer.
    let header = cx
        .debug_bounds("settings-card-header")
        .expect("the card header");
    let start = header.center();
    cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        start + point(px(40.), px(30.)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        start + point(px(40.), px(30.)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    let moved = cx.update(|_window, cx| {
        shell
            .read(cx)
            .settings_card
            .as_ref()
            .expect("the card is still open")
            .position
    });
    assert_eq!(
        moved,
        point(px(260.), px(126.)),
        "the header drag moved the card"
    );

    // Selecting another object does not close the panel: it is dismissed only
    // by its own ✕ or Esc.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.select("Derived", cx))
    });
    assert!(
        cx.update(|_window, cx| shell.read(cx).settings_card.is_some()),
        "a selection change leaves the settings panel open"
    );

    // The close button and Esc both dismiss it without losing the selection.
    click_at(&mut cx, "settings-card-close");
    assert!(cx.update(|_window, cx| shell.read(cx).settings_card.is_none()));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("settings-card").is_some());
    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert!(
        cx.update(|_window, cx| shell.read(cx).settings_card.is_none()),
        "Esc dismisses the modeless card"
    );
    reset_grid_state();
}

/// The settings rows are *painted*, not merely built: both the card and the Dev
/// Tools property pane go through `properties_panel`, and the field elements must
/// have real bounds inside the scroll area.
#[gpui::test]
fn the_settings_rows_are_painted(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);

    let area = cx
        .debug_bounds("dev-properties-area")
        .expect("the settings card's scroll area is painted");
    assert!(area.size.height > px(0.), "area bounds {area:?}");
    let field = cx
        .debug_bounds("prop-Settings/Grid.provider")
        .expect("the provider field is painted");
    assert!(
        field.size.width > px(0.) && field.size.height > px(0.),
        "field bounds {field:?}"
    );

    // The same pane in Dev Tools must not collapse either. It is selection
    // driven, so select the settings node there explicitly.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.select("Settings/Grid", cx))
    });
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::PROPERTIES);
    let area = dev_cx
        .debug_bounds("dev-properties-area")
        .expect("the Dev Tools properties panes is painted");
    assert!(area.size.height > px(0.), "Dev Tools area bounds {area:?}");
    let field = dev_cx
        .debug_bounds("prop-Settings/Grid.provider")
        .expect("the Dev Tools settings field is painted");
    assert!(
        field.size.height > px(0.),
        "Dev Tools field bounds {field:?}"
    );

    reset_grid_state();
}

/// A colour row shows a swatch; clicking it expands the palette, and a pick
/// commits through the same property path as a typed hex.
#[gpui::test]
fn a_colour_setting_opens_the_palette_and_commits(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);

    assert!(
        cx.debug_bounds("color-Settings/Grid.color-swatch")
            .is_some(),
        "the colour row has a swatch"
    );
    assert!(
        cx.debug_bounds("color-Settings/Grid.color-palette")
            .is_none()
    );

    click_at(&mut cx, "color-Settings/Grid.color-swatch");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("color-Settings/Grid.color-palette")
            .is_some(),
        "the swatch opens the palette"
    );

    click_at(&mut cx, "color-Settings/Grid.color-pick-ef4444");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("color-Settings/Grid.color-palette")
            .is_none(),
        "picking closes the palette"
    );
    let stored = cx.update(|_window, cx| {
        shell.read(cx).settings_card.as_ref().and_then(|card| {
            card.rows
                .iter()
                .find(|row| row.name == "color")
                .map(|row| row.value.clone())
        })
    });
    assert_eq!(stored.as_deref(), Some("#ef4444"), "the pick was committed");

    reset_grid_state();
}

/// A bounded numeric setting opens the slider overlay; the drag commits on
/// release. The mapping is logarithmic, so 75 % of 8..400 is well above the
/// arithmetic midpoint.
#[gpui::test]
fn a_bounded_setting_opens_the_slider_and_commits(cx: &mut TestAppContext) {
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
        shell.update(cx, |shell, cx| shell.select("Settings/Gizmo", cx))
    });
    draw(&mut cx);

    assert!(
        cx.debug_bounds("slider-Settings/Gizmo.size-trigger")
            .is_some(),
        "the bounded row has a slider trigger"
    );
    click_at(&mut cx, "slider-Settings/Gizmo.size-trigger");
    draw(&mut cx);

    let track = cx
        .debug_bounds("slider-Settings/Gizmo.size-track")
        .expect("the slider overlay is rendered");
    let at = point(
        track.left() + track.size.width * 0.75,
        track.center().y,
    );
    cx.simulate_mouse_down(at, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Left, gpui::Modifiers::default());
    draw(&mut cx);

    assert!(
        cx.debug_bounds("row-popup").is_none(),
        "the slider closes on release"
    );
    let stored = model
        .lock()
        .unwrap()
        .properties
        .iter()
        .find(|row| row.name == "size")
        .map(|row| row.value.clone())
        .expect("the size row");
    let size: f64 = stored
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .parse()
        .expect("a number");
    assert!(
        size > 100.0 && size < 400.0,
        "75 % of 8..400 on a log scale, got {stored:?}"
    );

    // Leave the shared interpreter on its default for the other tests.
    let _ = crate::python::evaluate("import FerroCAD\nFerroCAD.Gizmo.reset()");
}

/// A command dialog renders a bool as a toggle switch and an enum as a select
/// box whose dropdown updates the dialog's pending value.
#[gpui::test]
fn a_dialog_uses_the_toggle_and_select_box(cx: &mut TestAppContext) {
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

    let script = r#"
import FreeCADGui
class _TestForm:
    def GetResources(self):
        return {"MenuText": "Form", "Group": "Edit", "CmdType": "ForEdit"}
    def IsActive(self):
        return True
    def Activated(self):
        return {
            "title": "Test form",
            "fields": [
                {"name": "on", "label": "On", "kind": "bool", "value": False},
                {"name": "mode", "label": "Mode", "kind": "enum",
                 "values": ["fast", "safe"], "value": "safe"},
            ],
            "on_accept": lambda values: {"status": "form ok"},
        }
if "Test_Form" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Form", _TestForm())
"#;
    assert!(crate::python::evaluate(script).unwrap().ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.refresh_commands(cx);
            shell.run_command("Test_Form", cx);
        })
    });
    draw(&mut cx);

    assert!(
        cx.debug_bounds("dialog-bool-0-toggle").is_some(),
        "a bool field is a toggle"
    );
    assert!(
        cx.debug_bounds("dialog-enum-1-select").is_some(),
        "an enum field is a select box"
    );

    click_at(&mut cx, "dialog-bool-0-toggle");
    draw(&mut cx);
    let flag = cx.update(|_window, cx| {
        matches!(
            shell.read(cx).dialog.as_ref().and_then(|d| d.states.first()),
            Some(ValueState::Flag(true))
        )
    });
    assert!(flag, "the toggle flipped the dialog flag");

    click_at(&mut cx, "dialog-enum-1-select");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("choose-dialog-enum-1-choices").is_some(),
        "the dropdown lists the enum values"
    );
    click_at(&mut cx, "choose-dialog-enum-1-choice-0");
    draw(&mut cx);
    let choice = cx.update(|_window, cx| {
        match shell.read(cx).dialog.as_ref().and_then(|d| d.states.get(1)) {
            Some(ValueState::Choice(choice)) => Some(*choice),
            _ => None,
        }
    });
    assert_eq!(choice, Some(0), "the dropdown set the dialog choice");
}

/// A boolean setting is a toggle switch; a click commits the opposite value.
#[gpui::test]
fn a_bool_row_is_a_toggle_that_commits(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);

    let before = shell.read_with(&cx, |shell, _| {
        shell.settings_card.as_ref().and_then(|card| {
            card.rows
                .iter()
                .find(|row| row.name == "enabled")
                .map(|row| row.value.clone())
        })
    });
    assert_eq!(before.as_deref(), Some("False"), "the grid starts off");
    assert!(
        cx.debug_bounds("prop-bool-Settings/Grid.enabled-toggle")
            .is_some(),
        "a bool row is a toggle"
    );

    click_at(&mut cx, "prop-bool-Settings/Grid.enabled-toggle");
    draw(&mut cx);
    let after = shell.read_with(&cx, |shell, _| {
        shell.settings_card.as_ref().and_then(|card| {
            card.rows
                .iter()
                .find(|row| row.name == "enabled")
                .map(|row| row.value.clone())
        })
    });
    assert_eq!(after.as_deref(), Some("True"), "the toggle committed true");

    reset_grid_state();
}

/// An enumeration setting is a select box; a pick commits through the same
/// property path as typing.
#[gpui::test]
fn an_enum_row_is_a_select_box_that_commits(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);

    assert!(
        cx.debug_bounds("prop-enum-Settings/Grid.provider-select")
            .is_some(),
        "the provider row is a select box"
    );
    click_at(&mut cx, "prop-enum-Settings/Grid.provider-select");
    draw(&mut cx);
    assert!(
        cx.debug_bounds("choose-Settings/Grid.provider-choices")
            .is_some(),
        "the dropdown lists the providers"
    );

    // The providers are listed in registration order: native first, Coin second.
    click_at(&mut cx, "choose-Settings/Grid.provider-choice-1");
    draw(&mut cx);
    let backend = cx.update(|_window, cx| {
        shell
            .read(cx)
            .grid_summary
            .as_ref()
            .and_then(|summary| summary.backend.clone())
    });
    assert_eq!(backend.as_deref(), Some("coin"), "the pick switched the provider");

    // Leave the shared registry on the native backend for the other tests.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("ferrocad-provider:grid:ferrocad", cx)
        })
    });
    reset_grid_state();
}

/// A settings pane with more rows than its height is scrollable: the rows must
/// not be compressed into the viewport (which would clip them instead of
/// overflowing), and a wheel over it must move the tracked handle.
#[gpui::test]
fn a_crowded_settings_pane_scrolls(cx: &mut TestAppContext) {
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

    // The card's last row must be visible inside its scroll viewport; if it is
    // clipped, the rows were compressed instead of overflowing.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.select("Settings/Axis", cx))
    });
    draw(&mut cx);
    let area = cx
        .debug_bounds("dev-properties-area")
        .expect("the card's scroll area");
    let last = cx
        .debug_bounds("prop-command-App_ResetAxisAppearance")
        .expect("the card's last row");
    let card = cx.debug_bounds("settings-card").expect("the card");
    assert_eq!(card.size.height, px(420.), "the card keeps its height");
    assert!(
        area.size.height < card.size.height,
        "the viewport stays inside the card: card {:?}, area {:?}",
        card.size.height,
        area.size.height
    );
    let max = cx.update(|_window, cx| shell.read(cx).dev_scroll.settings.max_offset().y);
    assert!(
        max > px(0.),
        "the axis rows overflow the viewport (max {max:?})"
    );
    assert!(
        last.bottom() > area.bottom(),
        "the last row starts below the viewport: {:?} vs {:?}",
        last.bottom(),
        area.bottom()
    );

    // A wheel over the card scrolls it.
    cx.simulate_mouse_move(area.center(), None, gpui::Modifiers::default());
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: area.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-80.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(&mut cx);
    let offset = cx.update(|_window, cx| shell.read(cx).dev_scroll.settings.offset().y);
    assert!(
        offset < px(0.),
        "the wheel over the card scrolled it (offset {offset:?})"
    );

    // The default Dev Tools layout keeps Commands, Model and Properties open,
    // so the property pane is short and its settings rows must overflow.
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.select("Settings/Gizmo", cx))
    });
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let max = cx.update(|_window, cx| shell.read(cx).dev_scroll.properties.max_offset().y);
    assert!(
        max > px(0.),
        "the settings rows overflow the pane, so it is scrollable (max {max:?})"
    );

    let area = dev_cx
        .debug_bounds("dev-properties-area")
        .expect("the properties pane");
    dev_cx.simulate_mouse_move(area.center(), None, gpui::Modifiers::default());
    dev_cx.simulate_event(gpui::ScrollWheelEvent {
        position: area.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    let offset = cx.update(|_window, cx| shell.read(cx).dev_scroll.properties.offset().y);
    assert!(
        offset < px(0.),
        "a wheel over the pane scrolled it (offset {offset:?})"
    );
}

/// P0.8: a click/focus inside the card must not fall through to the canvas and
/// clear the selection (which would drop the card).
#[gpui::test]
fn clicking_a_settings_field_keeps_the_card_open(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    reset_grid_state();
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
        shell.update(cx, |shell, cx| shell.run_command("View_GridSettings", cx))
    });
    draw(&mut cx);

    let field = cx
        .debug_bounds("prop-Settings/Grid.minor")
        .expect("the minor field");
    cx.simulate_mouse_down(
        field.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        field.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(&mut cx);

    {
        let m = model.lock().unwrap();
        assert!(
            m.selected.is_none(),
            "opening settings does not select a node"
        );
    }
    let rows = cx.update(|_window, cx| {
        shell
            .read(cx)
            .settings_card
            .as_ref()
            .map(|card| card.rows.clone())
            .unwrap_or_default()
    });
    assert!(
        rows.iter().any(|row| row.name == "minor"),
        "the settings rows are still loaded"
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).settings_card.is_some()),
        "the card stays open while a field is edited"
    );
    reset_grid_state();
}

/// S2: editing a `Settings/Grid` row through the property editor commits to
/// the provider store, and the pane re-reads the row.
#[gpui::test]
fn editing_a_settings_row_updates_the_properties(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    // The modeless card renders the same property editor; selecting the node
    // opens it and builds one field per editable row.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.select("Settings/Grid", cx);
            cx.notify();
        });
    });
    draw(&mut cx);

    let field = shell.read_with(&cx, |shell, _| {
        shell
            .prop_inputs
            .get("Settings/Grid.minor")
            .expect("the Minor spacing field")
            .state
            .clone()
    });
    cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("25 mm");
    cx.simulate_keystrokes("enter");

    let minor = shell.read_with(&cx, |shell, _| {
        shell
            .model
            .lock()
            .unwrap()
            .properties
            .iter()
            .find(|row| row.name == "minor")
            .map(|row| row.value.clone())
    });
    assert_eq!(minor.as_deref(), Some("25 mm"));
    assert_eq!(model.lock().unwrap().status, "set Settings/Grid.minor");
    // The store, not just the mirrored row, changed.
    let stored = crate::python::properties("Settings/Grid")
        .expect("settings rows")
        .into_iter()
        .find(|row| row.name == "minor")
        .map(|row| row.value);
    assert_eq!(stored.as_deref(), Some("25 mm"));
    reset_grid_state();
}

/// T7: the app menu sits in the **title bar** and is the discoverable way back
/// into Dev Tools — the window starts closed and the command is hidden from every
/// other surface. The viewport header keeps only the environment controls.
///
/// The headless test platform always reports server-side decorations, so
/// `window_frame_with_leading` returns the content unchanged and paints no title
/// bar. This renders the same `title_bar` element the frame composes, with the
/// shell's real menu, so the popover and its rows can be clicked. The frame's own
/// client-decoration branch stays visual-only (see
/// `docs/project/record/tasks/toolbar-chrome.md` §10).
#[gpui::test]
fn the_app_menu_opens_dev_tools(cx: &mut TestAppContext) {
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

    // The menu left the viewport header; Grid/Axes/WP did not. The test
    // platform draws server-side decorations, so the frame's fallback leading bar
    // (below the platform title bar) is where the menu lives here.
    let header = cx.debug_bounds("viewport-header").expect("the viewport header");
    assert!(cx.debug_bounds("grid-label").is_some());
    let menu = cx
        .debug_bounds("app-menu")
        .expect("the app menu in the leading bar");
    assert!(
        !header.contains(&menu.center()),
        "the app menu is no longer in the viewport header"
    );
    assert!(
        cx.debug_bounds("leading-bar").is_some(),
        "server decorations get the leading-bar fallback"
    );

    // A second window whose content is only the title bar (see `TitleBarHarness`).
    let size = cx.update(|window, _| window.viewport_size());
    let shell_for_bar = shell.clone();
    let bar = cx.cx.open_window(size, move |_window, _cx| TitleBarHarness {
        shell: shell_for_bar.clone(),
    });
    let mut bar_cx = VisualTestContext::from_window(bar.into(), &cx);
    draw(&mut bar_cx);

    let title_bar = bar_cx.debug_bounds("titlebar").expect("the title bar");
    let menu = bar_cx.debug_bounds("app-menu").expect("the app menu");
    assert!(
        title_bar.contains(&menu.center()),
        "the app menu is inside the title bar (menu {menu:?}, bar {title_bar:?})"
    );
    assert!(
        menu.origin.x < title_bar.size.width * 0.5,
        "the app menu leads the title bar (at {:?})",
        menu.origin.x
    );
    assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

    // It lists Dev Tools, which no other surface offers.
    click_at(&mut bar_cx, "app-menu");
    assert!(cx.update(|_window, cx| shell.read(cx).popover_open("app")));
    draw(&mut bar_cx);
    let popover = bar_cx.debug_bounds("app-popover").expect("the app popover");
    assert!(
        popover.origin.y > title_bar.center().y,
        "the popover drops below the title bar (at {:?})",
        popover.origin.y
    );
    assert!(bar_cx.debug_bounds("app-row-Dev_ToggleTools").is_some());

    // Esc closes it and opens nothing.
    let root = cx.update(|_window, cx| shell.read(cx).root_focus_handle());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("app")));
    assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

    // Running the row opens the second window and dismisses the menu.
    draw(&mut bar_cx);
    click_at(&mut bar_cx, "app-menu");
    draw(&mut bar_cx);
    click_at(&mut bar_cx, "app-row-Dev_ToggleTools");
    assert!(!cx.update(|_window, cx| shell.read(cx).popover_open("app")));
    assert!(cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));
    assert!(
        cx.windows()
            .iter()
            .any(|handle| handle.downcast::<DevTools>().is_some()),
        "the Dev Tools window exists"
    );
}

/// The status bar's literal bug is the discoverable toggle: it opens and closes
/// Dev Tools without going through the title-bar menu.
#[gpui::test]
fn the_status_bar_bug_toggles_dev_tools(cx: &mut TestAppContext) {
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

    let bug = cx
        .debug_bounds("dev-tools-bug")
        .expect("the status bar bug");
    let viewport = cx.debug_bounds("main-viewport").expect("the viewport");
    assert!(
        bug.center().y > viewport.center().y,
        "the bug sits in the bottom strip (bug {bug:?}, viewport {viewport:?})"
    );
    assert!(!cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)));

    click_at(&mut cx, "dev-tools-bug");
    assert!(
        cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)),
        "the bug opens Dev Tools"
    );
    assert!(
        cx.windows()
            .iter()
            .any(|handle| handle.downcast::<DevTools>().is_some()),
        "the Dev Tools window exists"
    );

    click_at(&mut cx, "dev-tools-bug");
    assert!(
        !cx.update(|_window, cx| shell.read(cx).dev_tools_open(cx)),
        "a second click closes it"
    );
}

/// P5: a collapsed Dev Tools pane keeps its slot at zero height and paints no
/// body, so it leaves neither a paint sliver nor an invisible hit area. An open
/// pane keeps a real body; toggling the header moves the same slot between the
/// two states.
#[gpui::test]
fn a_collapsed_dev_pane_is_zero_height_and_paints_no_body(cx: &mut TestAppContext) {
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
    let mut dev_cx = open_dev_tools(&mut cx, &shell);

    // The console starts collapsed: its slot takes no height and its panel is
    // not painted.
    let collapsed = dev_cx
        .debug_bounds("dev-body-console")
        .expect("a collapsed pane keeps a zero-height slot");
    assert_eq!(
        collapsed.size.height,
        px(0.),
        "the collapsed console pane has no height (bounds {collapsed:?})"
    );
    assert!(
        dev_cx.debug_bounds("console-panel").is_none(),
        "a collapsed pane paints no body"
    );

    // Commands is open: a real body with a real height.
    let expanded = dev_cx
        .debug_bounds("dev-body-commands")
        .expect("an open pane has a body slot");
    assert!(
        expanded.size.height > px(0.),
        "the open commands pane keeps its height (bounds {expanded:?})"
    );
    assert!(
        dev_cx.debug_bounds("dev-command-list").is_some(),
        "the open pane paints its body"
    );

    // Opening the console turns the same slot into a painted body; closing it
    // returns the slot to zero and drops the body again.
    click_at(&mut dev_cx, "dev-header-console");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let opened = dev_cx
        .debug_bounds("dev-body-console")
        .expect("the opened pane has a body slot");
    assert!(
        opened.size.height > px(0.),
        "an opened pane gets its height back (bounds {opened:?})"
    );
    assert!(dev_cx.debug_bounds("console-panel").is_some());

    click_at(&mut dev_cx, "dev-header-console");
    dev_cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let collapsed = dev_cx
        .debug_bounds("dev-body-console")
        .expect("the collapsed pane keeps a slot after a second click");
    assert_eq!(
        collapsed.size.height,
        px(0.),
        "collapsing again returns the pane to zero height (bounds {collapsed:?})"
    );
    assert!(
        dev_cx.debug_bounds("console-panel").is_none(),
        "the collapsed pane no longer paints its body"
    );
}
