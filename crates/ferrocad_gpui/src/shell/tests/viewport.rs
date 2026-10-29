//! Shell tests: viewport.

use super::*;

/// The shell attaches and paints the 3D viewport headlessly. No CPython needed:
/// it builds a default model, attaches a GPU renderer when one is available
/// (else the placeholder), and forces a paint so the viewport render path runs.
#[gpui::test]
fn shell_paints_the_viewport(cx: &mut TestAppContext) {
    let model = Arc::new(Mutex::new(ShellModel::default()));
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();

    let attached = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    println!("shell viewport attached: {attached}");

    // Force a full paint (this is where `InventorViewport::render` draws and
    // reads the frame back). Works with the placeholder too when no GPU exists.
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

/// G2: the `App_ToggleGrid` view request reaches the viewport, and a viewport that
/// starts after the toggle picks the state up. Skips without a GPU adapter.
#[gpui::test]
fn a_grid_request_reaches_the_viewport(cx: &mut TestAppContext) {
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

    // Turn the grid on **before** the viewport exists: `enable_viewport` must apply
    // the mirrored view state on creation.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
    });

    let attached = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    if !attached {
        eprintln!("no GPU adapter available; skipping grid-to-viewport test");
        return;
    }

    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    cx.update(|_window, cx| {
        assert!(viewport.read(cx).grid_enabled(), "the boot state wins");
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
    });
    cx.update(|_window, cx| {
        assert!(!viewport.read(cx).grid_enabled(), "the toggle turns it off");
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_ToggleGrid", cx))
    });
    cx.update(|_window, cx| {
        assert!(viewport.read(cx).grid_enabled(), "and back on");
    });
}

/// G5: the grid preferences (steps and extent) are read from the parameter group by
/// `App_ReloadGridSettings` and reach the viewport without a restart.
#[gpui::test]
fn a_grid_settings_request_reaches_the_viewport(cx: &mut TestAppContext) {
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

    let attached = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });

    // A preference change goes through the one grid store; the command applies it.
    let script = r#"
import FerroCAD
grid = FerroCAD.Grid
grid.minor = 2.0
grid.major = 20.0
grid.radius = 750.0
"#;
    let evaluated = cx.update(|_window, _cx| crate::python::evaluate(script));
    assert!(evaluated.unwrap().ok);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("App_ReloadGridSettings", cx)
        })
    });

    assert_eq!(
        model.lock().unwrap().status,
        "grid minor 2, grid major 20, grid radius 750"
    );
    assert_eq!(model.lock().unwrap().view.grid_minor, 2.0);
    assert_eq!(model.lock().unwrap().view.grid_major, 20.0);
    assert_eq!(model.lock().unwrap().view.grid_radius, Some(750.0));

    if !attached {
        eprintln!("no GPU adapter available; skipping the viewport step assertion");
        return;
    }
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    cx.update(|_window, cx| {
        assert_eq!(viewport.read(cx).grid_steps(), (2.0, 20.0));
        assert_eq!(viewport.read(cx).grid_radius(), Some(750.0));
    });
}

/// G5: a workplane action is viewport work, so a headless shell reports it cleanly.
#[gpui::test]
fn a_workplane_request_needs_a_viewport(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("App_ActivateWorkplane", cx)
        })
    });
    assert_eq!(model.lock().unwrap().status, "view: no viewport");
}

/// G5: the `WP` menu's datum rows and `Set from selection` reach the viewport, and a
/// request that cannot bind says why without moving the plane. Skips without a GPU.
#[gpui::test]
fn the_workplane_rows_reach_the_viewport(cx: &mut TestAppContext) {
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

    let attached = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    if !attached {
        eprintln!("no GPU adapter available; skipping workplane-to-viewport test");
        return;
    }
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_WorkplaneXZ", cx))
    });
    assert_eq!(model.lock().unwrap().status, "workplane: XZ");
    cx.update(|_window, cx| {
        let active = viewport.read(cx).active_workplane().clone();
        assert_eq!(active.label, "XZ");
        assert!(active.plane.n.is_equal(&ferrocad_types::Vector3::new(0.0, -1.0, 0.0), 1e-9));
    });

    // Asking for the plane already active is reported, not faked as a change.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_WorkplaneXZ", cx))
    });
    assert_eq!(model.lock().unwrap().status, "workplane is already XZ");

    // Nothing planar is selected: the command explains itself and keeps the plane.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_SetWorkplane", cx))
    });
    assert_eq!(model.lock().unwrap().status, "select a planar face first");
    cx.update(|_window, cx| {
        assert_eq!(viewport.read(cx).active_workplane().label, "XZ");
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_WorkplaneXY", cx))
    });
    assert_eq!(model.lock().unwrap().status, "workplane: XY");
    cx.update(|_window, cx| {
        assert!(viewport.read(cx).active_workplane().is_default());
    });
}

/// G5/Tier 2: a **selected face** is what `Set from selection` binds. This is the whole
/// chain — the display element name, `entity_of`, `part_names`, the face frame — so it is
/// the test that catches a name that does not line up. Skips without a GPU adapter.
#[gpui::test]
fn a_selected_face_becomes_the_workplane(cx: &mut TestAppContext) {
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

    let attached = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    if !attached {
        eprintln!("no GPU adapter available; skipping face-selection test");
        return;
    }

    // A displayed slab with two named faces: the whole chain (display map, element
    // name, `entity_of`, `part_name`, face frame) runs for real, without depending on
    // which edition's startup document booted.
    let element = "Face2";
    let mesh: crate::display::StageMesh = serde_json::from_str(
        r#"{
            "entity": 7,
            "object": "Slab",
            "positions": [
                [-0.5, -0.5, 0.5], [0.0, -0.5, 0.5], [0.0, 0.5, 0.5], [-0.5, 0.5, 0.5],
                [0.0, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [0.0, 0.5, 0.5]
            ],
            "triangles": [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            "triangle_parts": [0, 0, 1, 1],
            "edges": [],
            "parts": ["Face1", "Face2"]
        }"#,
    )
    .expect("the test mesh");
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let stage = shell.display.build(vec![mesh]);
            if let Some(viewport) = shell.viewport.clone() {
                viewport.update(cx, |viewport, _cx| viewport.set_stage(stage));
            }
            cx.notify();
        })
    });

    // Select the face exactly as a single click does (the pick carries the element).
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.replace_selection(
                vec![SelectionItem::with_elements("Slab", vec![element.to_string()])],
                cx,
            )
        })
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_SetWorkplane", cx))
    });
    assert_eq!(model.lock().unwrap().status, format!("workplane: {element}"));
    cx.update(|_window, cx| {
        let active = shell
            .read(cx)
            .viewport
            .as_ref()
            .unwrap()
            .read(cx)
            .active_workplane()
            .clone();
        assert_eq!(active.label, element);
        assert!(!active.is_default());
        assert!((active.plane.origin.x - 0.25).abs() < 1e-9, "Face2's centroid");
    });
}
