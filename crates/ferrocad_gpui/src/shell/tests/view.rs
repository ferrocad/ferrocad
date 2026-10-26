//! Shell tests: view.

use super::*;

/// A4: a registered accelerator runs the same command path as a click, even
/// while a text field has focus (the chord is unhandled there and bubbles).
#[gpui::test]
fn a_shortcut_runs_the_command(cx: &mut TestAppContext) {
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
class _TestShortcut:
    def GetResources(self):
        return {"MenuText": "Shortcut", "Accel": "Ctrl+Shift+K"}
    def Activated(self):
        return {"status": "shortcut ran"}
if "Test_Shortcut" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Shortcut", _TestShortcut())
"#;
    let registered = cx.update(|_window, _cx| crate::python::evaluate(script));
    assert!(registered.unwrap().ok);
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

    // The console owns focus; Ctrl+Shift+K is not a text-editing chord, so it
    // bubbles to the Dev Tools root and runs the command — the same shell path as
    // a click, from either window.
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    show_dev_section(&mut cx, &shell, &mut dev_cx, dev_section::CONSOLE);
    let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
    dev_cx.update(|window, cx| window.focus(&console, cx));
    dev_cx.simulate_keystrokes("ctrl-shift-k");

    assert_eq!(model.lock().unwrap().status, "shortcut ran");
}

#[test]
fn the_pass_through_line_names_the_overlay_and_target() {
    let report = HoverReport {
        target: Some(SelectionRef::element(7, "Face1")),
        through: Some("navicube"),
    };
    assert_eq!(
        hover_status(&report, Some("Slab")),
        "clicked through: navicube → Slab"
    );
    assert_eq!(hover_status(&report, None), "clicked through: navicube");
    assert!(hover_status(&HoverReport::default(), Some("Slab")).is_empty());
}

/// A5: a `{"view": ...}` request flips the shell state, the mirrored facade
/// snapshot, and the command's check mark.
#[gpui::test]
fn a_view_request_toggles_the_grid_and_its_check_mark(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    assert!(!model.lock().unwrap().view.grid);
    assert!(command_enabled("App_ToggleGrid"));
    assert!(!command_checked("App_ToggleGrid"));

    cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx)));
    let (grid, status) = {
        let m = model.lock().unwrap();
        (m.view.grid, m.status.clone())
    };
    assert!(grid, "the request turned the grid on");
    assert_eq!(status, "grid on");
    assert!(
        command_checked("App_ToggleGrid"),
        "IsChecked reads the mirrored ViewState"
    );

    // A second run flips it back; the state is not a document edit.
    cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx)));
    assert!(!model.lock().unwrap().view.grid);
    assert!(!command_checked("App_ToggleGrid"));
}

/// G5: the axis cross is its own checkable flag, independent of the grid.
#[gpui::test]
fn the_axis_cross_is_toggled_separately(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    // Boot first: it puts the facade on `sys.path`, which the reset needs.
    reset_grid_state();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    assert!(model.lock().unwrap().view.axis_cross, "on by default");
    assert!(command_checked("App_ToggleAxisCross"));

    cx.update(|cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ToggleAxisCross", cx))
    });
    assert_eq!(model.lock().unwrap().status, "axis off");
    assert!(!model.lock().unwrap().view.axis_cross);
    assert!(!model.lock().unwrap().view.axis.enabled);
    assert!(!model.lock().unwrap().view.grid, "the grid is untouched");
    assert!(!command_checked("App_ToggleAxisCross"));
}

/// The parameterized `view.axis` request reaches the shell's view state; the bool
/// `axis_cross` stays the mirror of `axis.enabled`, and the deprecated alias still
/// drives the same flag.
#[gpui::test]
fn a_view_axis_request_reaches_the_state(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    let request: ViewRequest = serde_json::from_str(
        r##"{"axis": {"enabled": false, "color_x": "#010203", "length": 40.0, "width": 3.0}}"##,
    )
    .expect("a view.axis request");
    let summary = cx.update(|cx| {
        shell.update(cx, |shell, cx| shell.apply_view(&request, cx))
    });
    assert_eq!(summary.unwrap(), "axis off");

    let state = model.lock().unwrap().view.clone();
    assert!(!state.axis.enabled);
    assert!(!state.axis_cross, "the bool mirrors axis.enabled");
    assert_eq!(
        state.axis.colors[0],
        ferrocad_stage::grid::parse_srgb_hex("#010203").unwrap()
    );
    assert_eq!(state.axis.length, 40.0);
    assert_eq!(state.axis.width, 3.0);

    // The legacy alias still toggles the same flag (and the same mirror).
    let alias: ViewRequest = serde_json::from_str(r#"{"axis_cross": true}"#).unwrap();
    let summary = cx.update(|cx| {
        shell.update(cx, |shell, cx| shell.apply_view(&alias, cx))
    });
    assert_eq!(summary.unwrap(), "axis cross on");
    let state = model.lock().unwrap().view.clone();
    assert!(state.axis.enabled);
    assert!(state.axis_cross);
}

/// A non-native provider claim is reported when it changes *and* its provider
/// supplies no skin; the native backend is always hosted by the build.
#[test]
fn a_gizmo_backend_claim_is_reported_only_when_it_changes() {
    assert_eq!(
        gizmo_backend_note(ferrocad_stage::NATIVE_BACKEND, "ferrocad", false),
        None,
        "the unchanged native backend says nothing"
    );
    assert_eq!(gizmo_backend_note("shader", "shader", false), None);
    assert_eq!(
        gizmo_backend_note(
            ferrocad_stage::NATIVE_BACKEND,
            ferrocad_stage::SKIN_BACKEND,
            true
        ),
        None,
        "a supplied skin hosts the switch, so it is not a warning"
    );
    assert_eq!(
        gizmo_backend_note(
            ferrocad_stage::SKIN_BACKEND,
            ferrocad_stage::NATIVE_BACKEND,
            false
        ),
        None,
        "coming back to the native backend needs no warning"
    );

    let note = gizmo_backend_note(ferrocad_stage::NATIVE_BACKEND, "shader", false)
        .expect("a foreign backend with no skin is reported");
    assert!(note.contains("shader"), "{note}");
    assert!(note.contains("supplied no skin"), "{note}");
}

/// The skin channel's default: a plain boot (no mods) has no provider supplying a
/// gizmo skin, so the shell draws the native material.
#[test]
fn a_plain_boot_supplies_no_gizmo_skin() {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _model = boot_shell();
    assert_eq!(
        crate::python::provider_skin("gizmo").expect("the provider skin bridge"),
        None
    );
}

/// G1: a `view.gizmo` request restyles the workspace gizmo and reaches the
/// mirrored view state; a non-native backend with no supplied skin is stored but
/// reported as unhosted, because the native handles stay the event owner.
#[gpui::test]
fn a_view_gizmo_request_reaches_the_state_and_reports_the_backend(
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
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    let request: ViewRequest = serde_json::from_str(
        r##"{"gizmo": {"enabled": true, "backend": "ferrocad", "active_color": "#38bdf8", "inactive_color": "#94a3b8", "size": 96.0, "width": 2.0, "opacity": 0.8, "ghost_alpha": 0.5, "ring_snap_deg": 5.0, "show_readout": false}}"##,
    )
    .expect("a view.gizmo request");
    let summary = cx
        .update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.apply_view(&request, cx))
        })
        .expect("the request applies");
    assert!(summary.contains("gizmo style"), "{summary}");

    {
        let state = model.lock().unwrap().view.clone();
        let gizmo = &state.gizmo;
        assert!(gizmo.enabled);
        assert_eq!(gizmo.backend, ferrocad_stage::NATIVE_BACKEND);
        // The requested hexes are the documented defaults, so the 8-bit comparison
        // keeps the authored tokens rather than re-parsing them.
        let defaults = ferrocad_stage::GizmoStyle::default();
        assert_eq!(gizmo.active, defaults.active);
        assert_eq!(gizmo.inactive, defaults.inactive);
        assert_eq!(gizmo.inactive[3], 0.9, "the token's alpha is kept");
        assert_eq!(gizmo.size, 96.0);
        assert_eq!(gizmo.width, 2.0);
        assert_eq!(gizmo.opacity, 0.8);
        assert_eq!(gizmo.ghost_alpha, 0.5);
        assert_eq!(gizmo.ring_snap_deg, 5.0);
        assert!(!gizmo.show_readout);
    }

    // The claim is stored, but no provider declares `shader`, so it supplies no
    // skin: the shell says so, and the native manipulators remain the event owner.
    let claim: ViewRequest =
        serde_json::from_str(r#"{"gizmo": {"backend": "shader"}}"#).unwrap();
    let summary = cx
        .update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.apply_view(&claim, cx))
        })
        .expect("the claim applies");
    assert!(summary.contains("gizmo backend shader"), "{summary}");
    assert!(summary.contains("supplied no skin"), "{summary}");
    assert_eq!(model.lock().unwrap().view.gizmo.backend, "shader");
}

/// A5: a tool id resolves through the edition's registry; an unknown id reports
/// cleanly and starts nothing.
#[gpui::test]
fn a_tool_request_resolves_by_id(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));
    cx.update(|cx| {
        shell.update(cx, |shell, _| {
            shell.set_contributions(Contributions::new().with_tool("test_tool", || {
                Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
            }));
        })
    });

    let script = r#"
import FreeCADGui
class _TestTool:
    def Activated(self):
        return {"tool": "test_tool"}
class _TestMissingTool:
    def Activated(self):
        return {"tool": "no_such_tool"}
if "Test_Tool" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("Test_Tool", _TestTool())
    FreeCADGui.addCommand("Test_MissingTool", _TestMissingTool())
"#;
    let registered = cx.update(|_cx| crate::python::evaluate(script));
    assert!(registered.unwrap().ok);
    cx.update(|cx| shell.update(cx, |shell, cx| shell.refresh_commands(cx)));

    cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_Tool", cx)));
    assert_eq!(model.lock().unwrap().status, "tool: test_tool");
    let active = cx.update(|cx| shell.read(cx).active_tool.clone());
    assert_eq!(active.as_deref(), Some("test_tool"));

    cx.update(|cx| shell.update(cx, |shell, cx| shell.run_command("Test_MissingTool", cx)));
    assert_eq!(model.lock().unwrap().status, "no such tool no_such_tool");
}

/// A live tool session publishes a status the strip can draw, and the Cancel
/// handler ends it without a document edit. Headless: no viewport (and so no GPU)
/// is created.
#[gpui::test]
fn cancelling_a_tool_clears_the_strip_and_commits_nothing(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));
    cx.update(|cx| {
        shell.update(cx, |shell, _| {
            shell.set_contributions(Contributions::new().with_tool("test_polyline", || {
                Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
            }));
        })
    });

    let status = cx.update(|cx| {
        shell.update(cx, |shell, cx| shell.start_tool("test_polyline", cx))
    });
    assert_eq!(status, "tool: test_polyline");
    let status_line = model.lock().unwrap().status.clone();
    {
        let m = model.lock().unwrap();
        let tool = m.tool.as_ref().expect("the strip's status");
        assert!(tool.is_active());
        assert_eq!(tool.label, "Polyline");
        assert_eq!(tool.points, 0);
        assert!(tool.hint.contains("Esc to cancel"), "{}", tool.hint);
    }

    // The Cancel button's handler: the session ends and the strip's status is gone.
    assert!(cx.update(|cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))));
    assert!(model.lock().unwrap().tool.is_none(), "the strip is hidden");
    assert!(cx.update(|cx| shell.read(cx).active_tool.is_none()));
    assert!(
        !cx.update(|cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))),
        "a second cancel is a no-op"
    );
    // No InteractionCommit was produced: the shell's status line/console are intact.
    assert_eq!(model.lock().unwrap().status, status_line);
}

/// The strip builds and paints headlessly, and disappears after Cancel.
#[gpui::test]
fn the_tool_strip_renders_without_a_display(cx: &mut TestAppContext) {
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
        shell.update(cx, |shell, _| {
            shell.set_contributions(Contributions::new().with_tool("test_polyline", || {
                Box::new(ferrocad_stage::PolylineTool::new()) as Box<dyn ferrocad_stage::Tool>
            }));
        })
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.start_tool("test_polyline", cx);
        })
    });
    assert!(model.lock().unwrap().tool.is_some());

    // Paint the strip (the fake window has no display or GPU).
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    // Cancel through the same path as the button, then paint again without it.
    assert!(cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.cancel_tool(cx))));
    assert!(model.lock().unwrap().tool.is_none());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
