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
grid.adaptive = False
grid.minor_style = "lines"
grid.major_width = 4.0
grid.minor_width = 3.0
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
        "grid minor 2, grid major 20, grid adaptive off, grid minor style lines, \
         grid major width 4, grid minor width 3, grid radius 750"
    );
    assert_eq!(model.lock().unwrap().view.grid_minor, 2.0);
    assert_eq!(model.lock().unwrap().view.grid_major, 20.0);
    assert!(!model.lock().unwrap().view.grid_adaptive);
    assert_eq!(model.lock().unwrap().view.grid_major_width, 4.0);
    assert_eq!(model.lock().unwrap().view.grid_minor_width, 3.0);
    assert_eq!(model.lock().unwrap().view.grid_radius, Some(750.0));

    if !attached {
        eprintln!("no GPU adapter available; skipping the viewport step assertion");
        return;
    }
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    cx.update(|_window, cx| {
        assert_eq!(viewport.read(cx).grid_steps(), (2.0, 20.0));
        assert!(!viewport.read(cx).grid_adaptive());
        assert_eq!(
            viewport.read(cx).grid_stroke(),
            (ferrocad_stage::grid::MinorStyle::Lines, 4.0, 3.0),
            "the stroke reaches the viewport"
        );
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

/// G5: the `set` action's arbitrary frame reaches the viewport, which normalizes the
/// axes and labels the plane `Custom`; a degenerate frame is refused without moving
/// the plane. Also pins the datum-plane command's bridge report, which must not panic
/// when the base edition has no `add_datum_plane`. Skips without a GPU adapter.
#[gpui::test]
fn a_set_request_reaches_the_viewport(cx: &mut TestAppContext) {
    use crate::view::{ViewRequest, WorkplaneFrameRequest};

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
        eprintln!("no GPU adapter available; skipping set-workplane test");
        return;
    }
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());

    let request = ViewRequest {
        workplane: Some("set".to_string()),
        workplane_frame: Some(WorkplaneFrameRequest {
            origin: vec![10.0, 20.0, 30.0],
            u: vec![0.0, 1.0, 0.0],
            v: vec![0.0, 0.0, 2.0],
        }),
        ..Default::default()
    };
    let summary = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.apply_view(&request, cx))
    });
    assert_eq!(summary.unwrap(), "workplane: Custom");
    cx.update(|_window, cx| {
        let active = viewport.read(cx).active_workplane().clone();
        assert_eq!(active.plane.kind, ferrocad_stage::WorkplaneKind::Custom);
        assert_eq!(active.label, "Custom");
        assert!(active
            .plane
            .origin
            .is_equal(&ferrocad_types::Vector3::new(10.0, 20.0, 30.0), 1e-9));
        // `u × v` for `+Y` and `+Z` is `+X`, normalized away from the `v` length.
        assert!(active
            .plane
            .n
            .is_equal(&ferrocad_types::Vector3::new(1.0, 0.0, 0.0), 1e-9));
    });

    // The parser accepts a well-shaped frame even when its axes are parallel: the
    // viewport is the one that refuses it, and the active plane stays put.
    let degenerate = ViewRequest {
        workplane: Some("set".to_string()),
        workplane_frame: Some(WorkplaneFrameRequest {
            origin: vec![0.0, 0.0, 0.0],
            u: vec![1.0, 0.0, 0.0],
            v: vec![2.0, 0.0, 0.0],
        }),
        ..Default::default()
    };
    let summary = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.apply_view(&degenerate, cx))
    });
    assert_eq!(summary.unwrap(), "that frame is degenerate");
    cx.update(|_window, cx| {
        let origin = viewport.read(cx).active_workplane().plane.origin.clone();
        assert!(origin.is_equal(&ferrocad_types::Vector3::new(10.0, 20.0, 30.0), 1e-9));
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_SaveDatumPlane", cx))
    });
    let status = model.lock().unwrap().status.clone();
    assert!(status.starts_with("datum plane: "), "status: {status}");
}

/// G5: saving a datum plane reads the live frame, so a headless shell says it has none.
#[gpui::test]
fn saving_a_datum_plane_needs_a_viewport(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.run_command("App_SaveDatumPlane", cx)
        })
    });
    assert_eq!(model.lock().unwrap().status, "datum plane: no viewport");
}

/// G5: the datum-plane bridge sends the **live** frame, not a stale mirror. The base
/// edition has no `add_datum_plane` (the parametric one does), so the test installs a
/// stub that fails unless the payload carries this frame, and checks the object name
/// comes back. Skips without a GPU adapter.
#[gpui::test]
fn saving_a_datum_plane_sends_the_live_frame(cx: &mut TestAppContext) {
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
        eprintln!("no GPU adapter available; skipping datum-plane bridge test");
        return;
    }

    // The frame the stub must receive. `u × v = +X`, so the payload is unambiguous.
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            viewport.set_workplane_frame(
                ferrocad_types::Vector3::new(10.0, 20.0, 30.0),
                ferrocad_types::Vector3::new(0.0, 1.0, 0.0),
                ferrocad_types::Vector3::new(0.0, 0.0, 2.0),
            );
        });
    });
    let stub = r#"
import json
import ferrocad_shell

def _stub(payload):
    data = json.loads(payload)
    # The viewport normalizes the frame, so `v` arrives as a unit axis.
    assert data["origin"] == [10.0, 20.0, 30.0], data
    assert data["u"] == [0.0, 1.0, 0.0], data
    assert data["v"] == [0.0, 0.0, 1.0], data
    return json.dumps({"ok": True, "object": "DatumPlane001"})

ferrocad_shell.add_datum_plane = _stub
"#;
    let installed = cx.update(|_window, _cx| crate::python::evaluate(stub));
    assert!(installed.expect("the stub installs").ok, "stub failed");

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_SaveDatumPlane", cx))
    });
    assert_eq!(
        model.lock().unwrap().status,
        "datum plane: DatumPlane001",
        "the bridge received the live frame"
    );

    // Leave the entry module as the base edition had it.
    let _ = cx.update(|_window, _cx| crate::python::evaluate("del ferrocad_shell.add_datum_plane"));
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
    // The workplane command needs the face, so ask for the face scope: the default
    // `Select: Body` filter would report the whole slab.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_select_filter(SelectFilter::Face, cx));
    });
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

// ---------------------------------------------------------------------------
// W1: the triad hierarchy (workplanes note, §3.3)
// ---------------------------------------------------------------------------

/// A manipulator that claims `Face*` elements (the test's stand-in for Part's push/pull
/// stem). It draws and commits nothing: only the claiming rule is under test.
struct FaceClaim;

impl ferrocad_stage::Manipulator for FaceClaim {
    fn id(&self) -> &str {
        "face_claim"
    }

    fn items(&self) -> &[ferrocad_stage::ShapeItem] {
        &[]
    }

    fn hit(&self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
        false
    }

    fn drag_begin(&mut self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
        false
    }

    fn drag_update(&mut self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
        false
    }

    fn drag_end(&mut self) -> Option<ferrocad_stage::InteractionCommit> {
        None
    }

    fn drag_cancel(&mut self) -> bool {
        false
    }

    fn is_dragging(&self) -> bool {
        false
    }

    fn set_hovered(&mut self, _hovered: bool) -> bool {
        false
    }

    fn refresh(&mut self, _camera: &ferrocad_inventor::Camera) -> bool {
        false
    }
}

/// The `FaceClaim` factory (a plain `fn`: the `ManipulatorFactory` bound needs a
/// higher-ranked function, which a closure with elided lifetimes does not satisfy).
fn claim_face(
    selection: &ferrocad_geom::SelectionRef,
    _stage: &ferrocad_stage::Stage,
) -> Option<Box<dyn ferrocad_stage::Manipulator>> {
    selection
        .elements
        .first()
        .filter(|element| element.starts_with("Face"))
        .map(|_| Box::new(FaceClaim) as Box<dyn ferrocad_stage::Manipulator>)
}

/// A shell with a GPU viewport, one two-faced slab on the stage and a manipulator that
/// claims `Face*`. `None` without a GPU adapter. The viewport is read back through
/// `shell.viewport`, so its concrete renderer type stays unnamed here.
fn face_claim_shell(
    cx: &mut TestAppContext,
    model: Arc<Mutex<ShellModel>>,
) -> Option<(VisualTestContext, Entity<Shell>)> {
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut vcx).unwrap();
    let attached = vcx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_contributions(
                crate::Contributions::new()
                    .with_gizmo(crate::translate_gizmo)
                    .with_manipulator(claim_face),
            );
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    if !attached {
        eprintln!("no GPU adapter available; skipping triad-hierarchy test");
        return None;
    }
    // The stage the shell would build from its display meshes: a two-face slab.
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
    vcx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let stage = shell.display.build(vec![mesh]);
            // Through the shell, not the viewport: `Shell::set_stage` also keeps the
            // un-previewed document stage, which is where a transform session takes its
            // datum from.
            shell.set_stage(stage, cx);
            cx.notify();
        })
    });
    Some((vcx, shell))
}

// ---------------------------------------------------------------------------
// The Move HUD's drag session, driven through the viewport's own pointer path
// ---------------------------------------------------------------------------

/// Select `reference` the way a click does, so the shell re-arms the handles.
fn click_select(cx: &mut VisualTestContext, shell: &Entity<Shell>, reference: ferrocad_geom::SelectionRef) {
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

/// U4: Slot 1 is the body hand. A face click is reported as the whole body, so the
/// face-claiming manipulator is never offered and only the triad arms. Skips without a
/// GPU adapter.
#[gpui::test]
fn a_face_click_in_the_move_slot_arms_only_the_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "the app starts in the body hand"
    );
    click_select(
        &mut cx,
        &shell,
        ferrocad_geom::SelectionRef::element(7, "Face2"),
    );
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        let viewport = viewport.read(cx);
        assert!(
            !viewport.gizmos().is_empty(),
            "Slot 1 arms the body triad"
        );
        assert!(
            viewport.manipulators().is_empty(),
            "the face stem is unoffered in the Move hand"
        );
    });
}

/// W1 §3.3 → U4: Slot 2 hosts only the selection's manipulator, so a claimed face
/// arms the stem and the shared body triad is absent. Skips without a GPU adapter.
#[gpui::test]
fn a_claimed_sub_element_suppresses_the_shared_body_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    });
    click_select(
        &mut cx,
        &shell,
        ferrocad_geom::SelectionRef::element(7, "Face2"),
    );
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        let viewport = viewport.read(cx);
        assert!(
            viewport.gizmos().is_empty(),
            "Slot 2 hosts the stem, not the body triad"
        );
        assert_eq!(
            viewport.manipulators().len(),
            1,
            "the claiming manipulator is armed"
        );
    });
}

/// U4: the loadout replaced the claim-heuristic fallback. In Slot 2 an element no
/// manipulator claims leaves the hand empty (press `1` for the triad); it does not
/// silently arm the body control. Skips without a GPU adapter.
#[gpui::test]
fn an_unclaimed_sub_element_leaves_the_face_slot_empty(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    });
    click_select(
        &mut cx,
        &shell,
        ferrocad_geom::SelectionRef::element(7, "Edge1"),
    );
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        let viewport = viewport.read(cx);
        assert!(
            viewport.gizmos().is_empty(),
            "Slot 2 never falls back to the body triad"
        );
        assert!(
            viewport.manipulators().is_empty(),
            "nothing claims an edge face-manipulator slot"
        );
    });
}

/// U4: `Move` equips Slot 1, so it arms the triad even while a face is selected.
/// Skips without a GPU adapter.
#[gpui::test]
fn a_move_verb_arms_the_body_triad_on_a_face_selection(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    });
    let face = ferrocad_geom::SelectionRef::element(7, "Face2");
    click_select(&mut cx, &shell, face);
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        assert!(
            viewport.read(cx).gizmos().is_empty(),
            "the face hand has no triad"
        );
        assert_eq!(
            viewport.read(cx).manipulators().len(),
            1,
            "the face hand hosts the stem"
        );
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_Move", cx))
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.loadout.slot()),
        Slot::Move,
        "Move equips the body hand"
    );
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        assert!(
            !viewport.read(cx).gizmos().is_empty(),
            "the verb arms the triad"
        );
    });
}

// ---------------------------------------------------------------------------
// U5: hover-to-arm and the zero-click drag
// ---------------------------------------------------------------------------

/// U5: a handle press on the hover preview promotes its body **before** the session
/// opens, in one gesture. The preview is not a selection, so without this the commit
/// would find an empty facade selection.
#[gpui::test]
fn a_hover_preview_promotes_its_body_when_a_drag_begins(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, _model) = armed_window(cx);
    // The preview only exists while nothing is selected.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.clear_selection(cx)));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _| shell.hover_arm = Some(SelectionRef::whole(7)));
    });

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
        });
    });

    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().selection.clone()),
        vec![SelectionItem::new("Params")],
        "the press promoted the hovered body"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.hover_arm.is_none()),
        "the promoted body owns the handles"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_some()),
        "the same gesture opened the session"
    );
    assert!(shell.read_with(&cx, |shell, _| shell.model.lock().unwrap().gizmo_armed));

    // The release keeps the one-mode pending session.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Update, 4.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 4.0), cx);
        });
    });
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())),
        "the release leaves the move pending"
    );
}

/// U5: hover-to-arm belongs to the Move hand and to an empty selection. A latched
/// selection, a non-Move slot, a tool or a modal all own the pointer's meaning.
#[gpui::test]
fn hover_to_arm_needs_the_move_slot_and_an_empty_selection(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    assert!(shell.read_with(cx, |shell, _| shell.hover_arm_allowed()));
    shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::PushPull, cx));
    assert!(
        !shell.read_with(cx, |shell, _| shell.hover_arm_allowed()),
        "the face hand has no hover preview"
    );
    shell.update(cx, |shell, cx| shell.set_loadout_slot(Slot::Move, cx));
    assert!(shell.read_with(cx, |shell, _| shell.hover_arm_allowed()));
    shell.update(cx, |shell, cx| shell.select("Params", cx));
    assert!(
        !shell.read_with(cx, |shell, _| shell.hover_arm_allowed()),
        "a latched selection keeps its own handles"
    );
}

/// U6 fix: typing a delta in the Move pill moves the **gizmo**, not only the object.
/// The screenshot bug had the box at 10/10/10 with the triad stranded at the origin.
/// Skips without a GPU adapter.
#[gpui::test]
fn typing_a_move_moves_the_gizmo_with_the_object(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    let (drawn_before, origin_before) = cx.update(|_window, cx| {
        let gizmo = viewport.read(cx).gizmo().expect("the triad is armed");
        (gizmo.items()[0].positions()[0], gizmo.origin())
    });

    // The selection already opened the always-on Move form (and a clean session).
    draw(&mut cx);
    // The HUD opens unfocused: `Tab` enters the fields on the remembered axis, then
    // cycles X -> Y -> Z. Each typed `1` replaces that field's `0.00` seed.
    for _ in 0..3 {
        // `Tab` selects the field's seed, so `1` replaces it (0.00 -> 1, not 0.001).
        press_root(&mut cx, &shell, "tab");
        cx.simulate_input("1");
        draw(&mut cx);
    }

    let (origin, drawn_after, anchor, gizmo_anchor) = cx.update(|window, cx| {
        let viewport = viewport.read(cx);
        let gizmo = viewport.gizmo().expect("a gizmo");
        (
            gizmo.origin(),
            gizmo.items()[0].positions()[0],
            shell.read_with(cx, |shell, cx| shell.dialog_anchor(window, cx)),
            viewport.gizmo_anchor().expect("the triad projects"),
        )
    });
    assert!(
        (origin.x - origin_before.x - 1.0).abs() < 0.01
            && (origin.y - origin_before.y - 1.0).abs() < 0.01
            && (origin.z - origin_before.z - 1.0).abs() < 0.01,
        "the typed delta moved the gizmo frame from its anchor: {origin_before:?} -> {origin:?}"
    );
    assert!(
        (f64::from(drawn_after[0]) - f64::from(drawn_before[0]) - 1.0).abs() < 0.01,
        "the drawn triad follows the typed delta: {drawn_before:?} -> {drawn_after:?}"
    );
    assert_eq!(
        anchor, gizmo_anchor,
        "the HUD anchor is the moved gizmo, not a screen corner"
    );
}

/// U6 fix: the HUD anchors on the gizmo's projected origin, not on the pointer. The
/// old below-right offset put a low body's HUD into the dock clamp (bottom-right).
/// Skips without a GPU adapter.
#[gpui::test]
fn the_move_hud_anchors_to_the_gizmo_not_the_pointer(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    let gizmo = cx
        .update(|_window, cx| viewport.read(cx).gizmo_anchor())
        .expect("the triad projects");
    let anchor = cx.update(|window, cx| {
        shell.read_with(cx, |shell, cx| shell.dialog_anchor(window, cx))
    });
    assert_eq!(anchor, gizmo, "the HUD hangs off the gizmo origin");

    // A pointer parked in the bottom-right corner must not drag the HUD with it.
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    let far = point(
        canvas.origin.x + canvas.size.width - px(4.0),
        canvas.origin.y + canvas.size.height - px(4.0),
    );
    cx.simulate_mouse_move(far, None::<MouseButton>, gpui::Modifiers::default());
    draw(&mut cx);
    let after = cx.update(|window, cx| {
        shell.read_with(cx, |shell, cx| shell.dialog_anchor(window, cx))
    });
    assert_eq!(after, gizmo, "the pointer does not move the anchor");

    // The placement offset is up and to the right, so a low body keeps its HUD
    // beside the handles instead of in the bottom-right clamp.
    let viewport_size = cx.update(|window, _cx| window.viewport_size());
    let hud = size(px(420.0), px(60.0));
    let placed = place_hud(anchor, viewport_size, hud);
    assert!(
        placed.y < anchor.y,
        "the HUD sits above its anchor: {placed:?} vs {anchor:?}"
    );
}

/// U5/U6: the hover preview is hosted and dissolved by the state machine itself — the
/// pointer's report is the only untested leg, and it is `HoverReport`, already covered.
/// Skips without a GPU adapter.
#[gpui::test]
fn hover_arming_hosts_and_dissolves_the_preview(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    let body = ferrocad_geom::SelectionRef::whole(7);

    // The observer after this update sees no real pointer (the target was injected), so
    // the transitions are asserted inside the update that applies them.
    let (hovered, kept, left) = cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.apply_hover_arm(Some(&body), false, cx);
            let hosted = !viewport.read(cx).gizmos().is_empty();
            let idle = viewport
                .read(cx)
                .gizmo()
                .is_some_and(|gizmo| (gizmo.style().opacity - 0.30).abs() < 1e-6);
            let hovered = (
                shell.hover_arm.is_some(),
                hosted && idle,
                shell.model.lock().unwrap().selection.is_empty(),
            );

            // The gizmo consumes the pointer on its own handles: keep the preview.
            shell.apply_hover_arm(None, true, cx);
            let kept = shell.hover_arm.is_some();

            // Leaving dissolves it.
            shell.apply_hover_arm(None, false, cx);
            let left = (
                shell.hover_arm.is_none(),
                viewport.read(cx).gizmos().is_empty(),
            );
            (hovered, kept, left)
        })
    });
    assert_eq!(
        hovered,
        (true, true, true),
        "the hover is a live, idle-ink preview that selects nothing"
    );
    assert!(kept, "the pointer moving onto the handles keeps the preview");
    assert_eq!(left, (true, true), "leaving dissolves the idle triad");

    // A latched selection owns the handles: a body hover cannot replace or dim them.
    click_select(&mut cx, &shell, body.clone());
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.apply_hover_arm(Some(&body), false, cx));
    });
    assert!(
        shell.read_with(&cx, |shell, _| shell.hover_arm.is_none()),
        "a selection is not a hover preview"
    );
    let (hosted, opacity) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        (
            viewport.gizmos().len(),
            viewport.gizmo().map(|gizmo| gizmo.style().opacity),
        )
    });
    assert!(hosted > 0, "the latched selection keeps its triad");
    assert!(
        opacity.is_some_and(|opacity| (opacity - 1.0).abs() < 1e-6),
        "the selected triad keeps its full ink"
    );
}

/// Clearing the selection re-arms the canvas: the previous target's triad must not stay
/// hosted. A stale triad keeps consuming the pointer's hover, so hover-to-arm could not
/// materialise the idle preview until the pointer left and came back (or never). Skips
/// without a GPU adapter.
#[gpui::test]
fn clearing_the_selection_drops_the_stale_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());

    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));
    draw(&mut cx);
    let hosted = cx.update(|_window, cx| viewport.read(cx).gizmos().len());
    assert!(hosted > 0, "the selection hosts its triad");

    // Clear without a pointer move in between: the old handles must go now, not on the
    // next hover event.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.clear_selection(cx)));
    draw(&mut cx);
    let (hosted, armed) = cx.update(|_window, cx| {
        let hosted = viewport.read(cx).gizmos().len();
        let armed = shell.read_with(cx, |shell, _| shell.model.lock().unwrap().gizmo_armed);
        (hosted, armed)
    });
    assert_eq!(hosted, 0, "the old target's triad is gone");
    assert!(!armed, "and the model agrees");
}

/// U5: the hover preview is dimmed to the idle ink, and promotion restores the
/// selected triad's full opacity. Skips without a GPU adapter.
#[gpui::test]
fn the_idle_scale_dims_the_hosted_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().unwrap());
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    let opacity = |cx: &mut VisualTestContext| {
        cx.update(|_window, cx| {
            viewport
                .read(cx)
                .gizmo()
                .map(|gizmo| gizmo.style().opacity)
        })
    };
    assert!(
        opacity(&mut cx).is_some_and(|opacity| (opacity - 1.0).abs() < 1e-6),
        "a selection is drawn at full opacity"
    );

    cx.update(|_window, cx| {
        viewport.update(cx, |viewport, cx| {
            assert!(viewport.set_gizmo_idle_scale(Some(0.30)));
            cx.notify();
        });
    });
    assert!(
        opacity(&mut cx).is_some_and(|opacity| (opacity - 0.30).abs() < 1e-6),
        "the hover preview draws in the idle ink"
    );

    cx.update(|_window, cx| {
        viewport.update(cx, |viewport, cx| {
            assert!(viewport.set_gizmo_idle_scale(None));
            cx.notify();
        });
    });
    assert!(
        opacity(&mut cx).is_some_and(|opacity| (opacity - 1.0).abs() < 1e-6),
        "clearing the preview restores the selected ink"
    );
}

/// A deferred HUD drag must carry the triad with the previewed object, not leave it
/// at the datum it started from. Skips without a GPU adapter.
#[gpui::test]
fn a_deferred_hud_drag_moves_the_triad_with_the_object(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));
    let origin = |cx: &VisualTestContext| {
        shell.read_with(cx, |shell, cx| {
            shell
                .viewport
                .as_ref()
                .unwrap()
                .read(cx)
                .gizmos()
                .gizmos()[0]
                .origin()
        })
    };
    let start = origin(&cx);

    // The selection already opened the always-on Move form (and a clean session).
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_none_or(|session| !session.is_dirty())),
        "the HUD opens with a clean (not yet pending) session"
    );

    let preview = |phase, x: f64, y: f64, z: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, y, z),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0, 0.0, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::Update, 5.0, 0.0, 0.0), cx);
            shell.on_transform_preview(&preview(PreviewPhase::End, 5.0, 0.0, 0.0), cx);
        });
    });

    let end = origin(&cx);
    assert!(
        (end.x - (start.x + 5.0)).abs() < 1e-6 && (end.y - start.y).abs() < 1e-6,
        "the triad follows the previewed object: {start:?} -> {end:?}"
    );
}

/// The full Move-HUD session through the **viewport's own** pointer path: press a
/// translate arrow, tick, release. Each queued preview is routed the way the window
/// handlers route it. Releasing must leave the object previewed and the triad at the pose
/// the drag reached — the report is that both snap back to the datum on release. Skips
/// without a GPU adapter.
#[gpui::test]
fn a_move_hud_drag_survives_its_release(cx: &mut TestAppContext) {
    use ferrocad_stage::{GizmoHandle, GizmoTarget};

    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    // The selection already opened the always-on Move form (and a clean session).
    assert!(
        shell.read_with(&cx, |shell, _| shell
            .session
            .as_ref()
            .is_none_or(|session| !session.is_dirty())),
        "the HUD opens with a clean (not yet pending) session"
    );

    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().expect("a viewport"));
    let (start, grab, release, drawn_before) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let camera = viewport.camera().clone();
        let gizmo = &viewport.gizmos().gizmos()[0];
        let drawn_before = gizmo.items()[0].positions()[0];
        let origin = gizmo.origin();
        let device = ferrocad_stage::screen::project_device(&camera, origin)
            .expect("the triad projects");
        let mut hits = Vec::new();
        for dx in -140..=140 {
            for dy in -140..=140 {
                let point = [device[0].round() as i32 + dx, device[1].round() as i32 + dy];
                if gizmo.hit(&camera, point) == Some(GizmoTarget::Handle(GizmoHandle::AxisX)) {
                    hits.push(point);
                }
            }
        }
        // The middle of the hit region, not its edge: a one-pixel rounding difference
        // between the scan and the press must not miss the handle.
        let grab = *hits.get(hits.len() / 2).expect("the X arrow is pickable");
        let release = [grab[0] + 24, grab[1]];
        (
            origin,
            camera.viewport.to_logical(grab),
            camera.viewport.to_logical(release),
            drawn_before,
        )
    });

    // Press, tick, release through the viewport, feeding each queued preview to the
    // shell exactly as `InventorViewport::render`'s handlers do.
    let feed = |cx: &mut VisualTestContext, preview: Option<TransformPreview>| {
        let preview = preview.expect("a queued preview");
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.on_viewport_event(&ViewportEvent::TransformPreview(preview), cx)
            });
        });
    };
    let step = |cx: &mut VisualTestContext, at, down: bool| {
        let preview = cx.update(|_window, cx| {
            viewport.update(cx, |viewport, _cx| {
                let taken = viewport.handle_button(at, 1, down);
                assert!(taken, "the drag start and its release are both taken");
                viewport.take_transform_preview()
            })
        });
        feed(cx, preview);
    };

    step(&mut cx, grab, true);
    let update = cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            viewport.handle_pointer(release);
            viewport.take_transform_preview().expect("an update")
        })
    });
    feed(&mut cx, Some(update.clone()));
    step(&mut cx, release, false);
    // Paint a frame after the release: a later render must not undo the SESSION_HOLD
    // pose. The report was that the triad snapped back to the start of the drag.
    draw(&mut cx);

    let dragged = [
        update.total_transform.m[3],
        update.total_transform.m[7],
        update.total_transform.m[11],
    ];
    assert!(
        dragged[0].abs() > 1e-9,
        "the drag moved along X: {dragged:?}"
    );
    // The drawn handles, not only `origin()`, are at the released pose. A frame move
    // without a geometry rebuild was the "the triad snaps back" report.
    let drawn_after = cx.update(|_window, cx| {
        viewport.read(cx).gizmo().unwrap().items()[0].positions()[0]
    });
    assert!(
        (f64::from(drawn_after[0]) - f64::from(drawn_before[0]) - dragged[0]).abs() < 1e-4,
        "the drawn triad stays at the released pose: {drawn_before:?} -> {drawn_after:?} (drag {dragged:?})"
    );
    // A re-arm while the session is still pending rebuilds from the document stage;
    // the pending preview must be re-applied or the triad lands back at the old pose.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.rearm_selection(cx));
    });
    draw(&mut cx);
    let (rearmed_origin, rearmed_drawn) = cx.update(|_window, cx| {
        let gizmo = viewport.read(cx).gizmo().expect("the triad is still armed");
        (gizmo.origin(), gizmo.items()[0].positions()[0])
    });
    assert!(
        (rearmed_origin.x - (start.x + dragged[0])).abs() < 1e-6
            && (rearmed_origin.y - (start.y + dragged[1])).abs() < 1e-6,
        "a re-arm keeps the pending pose: {start:?} + {dragged:?} != {rearmed_origin:?}"
    );
    assert!(
        (f64::from(rearmed_drawn[0]) - f64::from(drawn_before[0]) - dragged[0]).abs() < 1e-4,
        "the re-armed triad draws at the pending pose: {rearmed_drawn:?}"
    );

    // 1. The triad sits where the drag ended, not back at the datum.
    cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let origin = viewport.gizmos().gizmos()[0].origin();
        assert!(
            (origin.x - (start.x + dragged[0])).abs() < 1e-6
                && (origin.y - (start.y + dragged[1])).abs() < 1e-6,
            "the triad stays at the dragged pose: {start:?} -> {origin:?} (drag {dragged:?})"
        );
        assert!(
            viewport.gizmos().gizmos()[0].display_offset().is_none(),
            "the draw-only offset is folded into the frame, so hit-testing agrees"
        );
    });
    // 2. The HUD's fields carry the drag, so accepting it repeats the move.
    let fields = shell.read_with(&cx, |shell, cx| {
        (
            dialog_field(shell, cx, "dx"),
            dialog_field(shell, cx, "dy"),
            dialog_field(shell, cx, "dz"),
        )
    });
    assert!(
        fields.0.parse::<f64>().is_ok_and(|dx| (dx - dragged[0]).abs() < 0.01),
        "the ΔX field is the drag, not zero: {fields:?} vs {dragged:?}"
    );
    // The field text is rounded to two decimals; the **commit** must use the exact
    // drag, or accepting would place the object a little off from what was previewed.
    let pending = shell
        .read_with(&cx, |shell, cx| shell.pending_translation(cx))
        .expect("a pending move");
    assert!(
        (pending[0] - dragged[0]).abs() < 1e-12,
        "the HUD keeps the exact delta for the commit: {pending:?} vs {dragged:?}"
    );
    // 4. The session is the uncommitted value behind that: it holds the same delta and
    //    the datum the gesture started from (the slab's world anchor, 7 sits at -0.5).
    let (session_delta, datum) = shell.read_with(&cx, |shell, _| {
        let session = shell.session.as_ref().expect("a live session");
        let delta = session.delta();
        (delta, session.datum())
    });
    assert!(
        (session_delta.x - dragged[0]).abs() < 1e-12,
        "the session holds the drag: {session_delta:?} vs {dragged:?}"
    );
    assert!(
        (datum.x - start.x).abs() < 1e-6 && (datum.y - start.y).abs() < 1e-6,
        "the session datum is the pose the drag started from: {datum:?} vs {start:?}"
    );
    // 3. The object is still previewed at the drag, not snapped back.
    let previewed = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let item = viewport
            .stage()
            .items()
            .iter()
            .find(|item| item.entity_id() == 7)
            .expect("the slab");
        let transform = item.temporary_transform().expect("a previewed object");
        (transform.m[3], transform.m[7], transform.m[11])
    });
    assert!(
        (previewed.0 - dragged[0]).abs() < 1e-9,
        "the object is still previewed where the drag ended: {previewed:?} vs {dragged:?}"
    );

    // 6. A **second** drag continues from the settled pose. The first release moved the
    //    interaction frame; a stale preview home would then draw the triad back at the
    //    document pose on the next press — the reported "it snaps back".
    let (settled, second_grab, second_release) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let camera = viewport.camera().clone();
        let gizmo = &viewport.gizmos().gizmos()[0];
        let origin = gizmo.origin();
        let device = ferrocad_stage::screen::project_device(&camera, origin)
            .expect("the settled triad projects");
        let mut hits = Vec::new();
        for dx in -140..=140 {
            for dy in -140..=140 {
                let point = [device[0].round() as i32 + dx, device[1].round() as i32 + dy];
                if gizmo.hit(&camera, point) == Some(GizmoTarget::Handle(GizmoHandle::AxisX)) {
                    hits.push(point);
                }
            }
        }
        let grab = *hits
            .get(hits.len() / 2)
            .expect("the settled X arrow is pickable");
        (
            origin,
            camera.viewport.to_logical(grab),
            camera.viewport.to_logical([grab[0] + 24, grab[1]]),
        )
    });
    step(&mut cx, second_grab, true);
    let (press_origin, press_offset) = cx.update(|_window, cx| {
        let gizmo = &viewport.read(cx).gizmos().gizmos()[0];
        (gizmo.origin(), gizmo.display_offset())
    });
    assert!(
        (press_origin.x - settled.x).abs() < 1e-9 && (press_origin.y - settled.y).abs() < 1e-9,
        "the press keeps the settled frame: {settled:?} -> {press_origin:?}"
    );
    assert_eq!(
        press_offset,
        Some(ferrocad_types::Vector3::zero()),
        "the draw-only offset is measured from where the triad is, not the document"
    );
    let update2 = cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            viewport.handle_pointer(second_release);
            viewport.take_transform_preview().expect("an update")
        })
    });
    let travelled = update2.total_transform.m[3];
    assert!(travelled.abs() > 1e-9, "the second drag moved: {travelled}");
    feed(&mut cx, Some(update2));
    step(&mut cx, second_release, false);
    draw(&mut cx);
    let (after_origin, after_drawn) = cx.update(|_window, cx| {
        let gizmo = &viewport.read(cx).gizmos().gizmos()[0];
        (gizmo.origin(), gizmo.items()[0].positions()[0])
    });
    assert!(
        (after_origin.x - (settled.x + travelled)).abs() < 1e-6,
        "the second release settles at the cumulative pose: {settled:?} + {travelled} != {after_origin:?}"
    );
    assert!(
        (f64::from(after_drawn[0]) - f64::from(drawn_before[0]) - dragged[0] - travelled).abs()
            < 1e-4,
        "the drawn triad carries both drags: {drawn_before:?} -> {after_drawn:?}"
    );

    // 5. Cancelling drops the live value and its preview: `Esc` is a rollback of the
    //    session, not an undo step (nothing was ever written to the document). The form
    //    itself stays, because it belongs to the Move hand.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.dismiss_move_hud(cx)));
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.is_none()),
        "the session is gone with the rollback"
    );
    let previewed = cx.update(|_window, cx| {
        viewport
            .read(cx)
            .stage()
            .items()
            .iter()
            .find(|item| item.entity_id() == 7)
            .and_then(|item| item.temporary_transform())
    });
    assert_eq!(previewed, None, "the ghost is gone with the session");
}

// ---------------------------------------------------------------------------
// U1: one mode — a release is pending, `Enter` commits, `Esc` drops
// ---------------------------------------------------------------------------

/// With no HUD open, a drag still leaves a pending session and a live preview; `Enter`
/// commits it and `Esc` drops it without writing. This is the behaviour change the
/// split-brain reports asked for. Skips without a GPU adapter.
#[gpui::test]
fn a_plain_drag_is_pending_until_enter_or_escape(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    let preview = |phase, x: f64| TransformPreview {
        target_id: 7,
        phase,
        total_transform: ferrocad_types::Matrix4::translation(x, 0.0, 0.0),
        value: GizmoValue::Linear { axis: GizmoAxis::X, raw: 0.0, applied: 0.0 },
        ring: None,
    };
    let drag = |cx: &mut VisualTestContext, x: f64| {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.on_transform_preview(&preview(PreviewPhase::Begin, 0.0), cx);
                shell.on_transform_preview(&preview(PreviewPhase::Update, x), cx);
                shell.on_transform_preview(&preview(PreviewPhase::End, x), cx);
            });
        });
    };
    let state = |cx: &mut VisualTestContext| {
        let viewport = shell.read_with(cx, |shell, _| shell.viewport.clone()).expect("a viewport");
        cx.update(|_window, cx| {
            let session = shell.read(cx).session.clone();
            let ghost = viewport
                .read(cx)
                .stage()
                .items()
                .iter()
                .find(|item| item.entity_id() == 7)
                .and_then(|item| item.temporary_transform());
            (session.map(|s| s.is_dirty()), ghost.is_some())
        })
    };

    // A release is not a commit: the session is dirty and the ghost is still drawn.
    drag(&mut cx, 4.0);
    assert_eq!(
        state(&mut cx),
        (Some(true), true),
        "the release leaves a pending move, not a committed one"
    );

    // Esc is the rollback: the pending move and its ghost go, nothing was written.
    let root = shell.read_with(&cx, |shell, _| shell.root_focus.clone());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert_eq!(
        state(&mut cx),
        (None, false),
        "Esc dropped the pending move without a commit"
    );

    // A second drag is a fresh session, and Enter spends it. (This runs last: the
    // harness's commit re-syncs the stage from the boot document, which is not the
    // synthetic slab, so the target only exists until the first commit.)
    drag(&mut cx, 4.0);
    assert_eq!(state(&mut cx), (Some(true), true));
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    assert_eq!(
        state(&mut cx),
        (None, false),
        "Enter checkpointed the move and released the ghost"
    );
}

// ---------------------------------------------------------------------------
// U3: the session's measuring tape and SESSION_HOLD
// ---------------------------------------------------------------------------

/// A pending session hands the viewport its tape, the release keeps both (SESSION_HOLD),
/// and the rollback drops them together. Skips without a GPU adapter.
#[gpui::test]
fn a_move_session_draws_the_tape_until_it_ends(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    click_select(&mut cx, &shell, ferrocad_geom::SelectionRef::whole(7));

    // The selection already opened the always-on Move form (and a clean session).
    let viewport = cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .clone()
            .expect("a viewport")
    });
    assert!(
        cx.update(|_window, cx| viewport.read(cx).transform_tape())
            .is_none(),
        "a clean session has no tape"
    );

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

    let tape = cx
        .update(|_window, cx| viewport.read(cx).transform_tape())
        .expect("the session's tape is drawn");
    assert_eq!(tape.delta().x, 5.0);
    assert!(
        (tape.current().x - (tape.datum().x + 5.0)).abs() < 1e-12,
        "the tape ends at the previewed origin"
    );
    assert!(
        shell.read_with(&cx, |shell, _| shell.session.as_ref().is_some_and(|s| s.is_dirty())),
        "SESSION_HOLD: the release left the session live"
    );

    // The rollback drops the session and its tape together.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.drop_session("move cancelled", cx);
        });
    });
    assert!(
        cx.update(|_window, cx| viewport.read(cx).transform_tape())
            .is_none(),
        "Esc's drop hides the tape"
    );
}

// ---------------------------------------------------------------------------
// W2: `Offset Workplane…` — the `+N` stem and its floating HUD
// ---------------------------------------------------------------------------

/// A windowed shell with a GPU viewport and the booted document, or `None` when no
/// adapter is available. The viewport is always read back through `shell.viewport`,
/// so its concrete renderer type stays unnamed here.
fn offset_shell(
    cx: &mut TestAppContext,
) -> Option<(VisualTestContext, Entity<Shell>, Arc<Mutex<ShellModel>>)> {
    let model = boot_shell();
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut vcx).unwrap();
    let attached = vcx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.enable_viewport(cx);
            shell.viewport.is_some()
        })
    });
    if !attached {
        eprintln!("no GPU adapter available; skipping the workplane stem test");
        return None;
    }
    Some((vcx, shell, model))
}

/// The armed manipulators' ids, in draw order.
fn armed_ids(cx: &mut VisualTestContext, shell: &Entity<Shell>) -> Vec<String> {
    cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .as_ref()
            .expect("a viewport")
            .read(cx)
            .manipulators()
            .iter()
            .map(|manipulator| manipulator.id().to_string())
            .collect()
    })
}

/// The active plane's transient offset.
fn active_offset(cx: &mut VisualTestContext, shell: &Entity<Shell>) -> f64 {
    cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .as_ref()
            .expect("a viewport")
            .read(cx)
            .active_workplane()
            .offset
    })
}

/// The active plane's origin.
fn active_origin(cx: &mut VisualTestContext, shell: &Entity<Shell>) -> ferrocad_types::Vector3 {
    cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .as_ref()
            .expect("a viewport")
            .read(cx)
            .active_workplane()
            .plane
            .origin
            .clone()
    })
}

/// Drag the armed `+N` stem `pixels` to the right through the viewport's own pointer
/// path, and take the commit it produced.
fn drag_stem(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    pixels: i32,
) -> Option<ferrocad_stage::InteractionCommit> {
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().expect("a viewport"));
    let (grab, release) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let camera = viewport.camera().clone();
        let plane = viewport.active_workplane().plane.clone();
        // The stem's own geometry against the live camera is exactly what the overlay
        // draws, so this is where the pointer must grab it.
        let (_, tip) = ferrocad_stage::WorkplaneStem::new(&plane)
            .handle_points(&camera)
            .expect("a projected stem");
        let grab = [tip[0].round() as i32, tip[1].round() as i32];
        let release = [grab[0] + pixels, grab[1]];
        (
            camera.viewport.to_logical(grab),
            camera.viewport.to_logical(release),
        )
    });
    cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            assert!(viewport.handle_button(grab, 1, true), "the stem is grabbable");
            viewport.handle_pointer(release);
            assert!(
                viewport.handle_button(release, 1, false),
                "releasing the stem commits"
            );
            viewport.take_interaction_commit()
        })
    })
}

/// Route a commit the way the window handlers do.
fn feed_commit(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    commit: ferrocad_stage::InteractionCommit,
) {
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.on_viewport_event(&ViewportEvent::Interaction(commit), cx)
        })
    });
}

/// Focus the shell root and press `Esc`, then repaint (the dismiss path the HUD uses).
fn press_escape(cx: &mut VisualTestContext, shell: &Entity<Shell>) {
    let root = shell.read_with(&*cx, |shell, _| shell.root_focus.clone());
    cx.update(|window, cx| window.focus(&root, cx));
    cx.simulate_keystrokes("escape");
    draw(cx);
}

/// W2: `App_OffsetWorkplane` arms the active plane's `+N` stem **and** opens its
/// floating one-field HUD, so the same offset can be dragged or typed. Skips without
/// a GPU adapter.
#[gpui::test]
fn offset_workplane_arms_the_stem_and_opens_a_floating_hud(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, _model)) = offset_shell(cx) else {
        return;
    };

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });

    assert_eq!(
        armed_ids(&mut cx, &shell),
        vec![ferrocad_stage::WORKPLANE_OFFSET_ID.to_string()],
        "the plane's `+N` stem is the armed handle"
    );
    let (open, floating, field, fields) = cx.update(|_window, cx| {
        let shell = shell.read(cx);
        let dialog = shell.dialog.as_ref().expect("the offset HUD");
        (
            true,
            dialog.spec.is_floating(),
            dialog_field(shell, cx, "offset"),
            dialog.spec.fields.len(),
        )
    });
    assert!(open && floating, "the offset HUD floats beside the stem");
    assert_eq!(fields, 1, "one quantity field");
    assert_eq!(field.parse::<f64>().unwrap_or(f64::NAN), 0.0, "seeded at zero");
    assert!(
        !cx.update(|_window, cx| shell.read(cx).session.is_some()),
        "an offset is applied as it is dragged, never held as a transform session"
    );
}

/// W2: a stem drag commits `workplane_offset`, the shell applies it to the active
/// plane itself (no document object lookup, no `interaction` hook), and the HUD's
/// field mirrors the session's total. Skips without a GPU adapter.
#[gpui::test]
fn a_stem_drag_offsets_the_plane_without_the_interaction_hook(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, model)) = offset_shell(cx) else {
        return;
    };

    // A `workplane_offset` commit must never reach the app script's hook.
    let stub = r#"
import ferrocad_shell
ferrocad_shell._w2_original_interaction = ferrocad_shell.interaction
ferrocad_shell._w2_calls = []
def _w2_recording_interaction(*args):
    ferrocad_shell._w2_calls.append(list(args))
    return '{"ok": true}'
ferrocad_shell.interaction = _w2_recording_interaction
"#;
    let installed = cx.update(|_window, _cx| crate::python::evaluate(stub));
    assert!(installed.expect("the stub installs").ok, "stub failed");

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });

    let before_origin = active_origin(&mut cx, &shell);
    let normal = cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .as_ref()
            .expect("a viewport")
            .read(cx)
            .active_workplane()
            .plane
            .n
    });
    let commit = drag_stem(&mut cx, &shell, 24).expect("the drag commits");

    assert_eq!(commit.manipulator, ferrocad_stage::WORKPLANE_OFFSET_ID);
    assert_eq!(commit.action, ferrocad_stage::WORKPLANE_OFFSET_ACTION);
    assert!(commit.element.is_none(), "the plane is not a sub-element");
    assert!(!commit.creates, "an offset creates nothing");
    assert_eq!(commit.values.len(), 1);
    let delta = commit.values[0];
    assert!(delta.abs() > 1e-6, "the drag travelled: {delta}");

    feed_commit(&mut cx, &shell, commit);

    // The app script's hook was never called...
    let restored = cx.update(|_window, _cx| {
        crate::python::evaluate(
            "import ferrocad_shell as s\n\
             calls = s._w2_calls\n\
             s.interaction = s._w2_original_interaction\n\
             del s._w2_original_interaction\n\
             del s._w2_calls\n\
             assert not calls, calls\n",
        )
    });
    assert!(
        restored.expect("the harness reads back").ok,
        "the offset commit reached the app script's interaction hook"
    );

    // ...and the plane moved along `+N`.
    assert_eq!(active_offset(&mut cx, &shell), delta, "the offset is the delta");
    let after_origin = active_origin(&mut cx, &shell);
    assert!(
        after_origin.is_equal(&before_origin.add(&normal.scale(delta)), 1e-9),
        "the origin moved along `+N`: {after_origin:?} from {before_origin:?}"
    );
    let status = model.lock().unwrap().status.clone();
    assert!(status.starts_with("workplane: "), "status: {status}");
    assert!(
        !status.contains("no document object"),
        "the commit never resolves an entity: {status}"
    );
    let field: f64 = cx.update(|_window, cx| dialog_field(shell.read(cx), cx, "offset"))
        .parse()
        .unwrap_or(f64::NAN);
    assert!(
        (field - delta).abs() < 0.01,
        "the HUD field follows the drag: {field} vs {delta}"
    );
}

/// W2: `Esc` with the HUD open restores the offset the drag started from and removes
/// the stem. Skips without a GPU adapter.
#[gpui::test]
fn escape_restores_the_pre_drag_workplane_offset(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, _model)) = offset_shell(cx) else {
        return;
    };

    // A session that starts from a non-zero offset, so "restore" is not just "zero".
    // The sample camera looks from `+(1, -1, 1)`, so a negative `+Z` offset stays in
    // front of the eye (and therefore projects).
    let seeded = crate::view::ViewRequest {
        workplane: Some("offset".to_string()),
        workplane_offset: Some(-7.0),
        ..Default::default()
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            let _ = shell.apply_view(&seeded, cx);
            shell.run_command("App_OffsetWorkplane", cx)
        })
    });
    let base = active_offset(&mut cx, &shell);
    assert_eq!(base, -7.0, "the session starts from a shifted plane");
    let commit = drag_stem(&mut cx, &shell, 24).expect("the drag commits");
    feed_commit(&mut cx, &shell, commit);
    assert_ne!(active_offset(&mut cx, &shell), base, "the drag moved the plane");

    press_escape(&mut cx, &shell);

    assert_eq!(
        active_offset(&mut cx, &shell),
        base,
        "Esc is a true cancel: the pre-drag offset is restored"
    );
    assert!(
        armed_ids(&mut cx, &shell).is_empty(),
        "the stem ends with the HUD"
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
        "Esc closes the HUD"
    );
}

/// W2: a drag still in flight applies nothing, so `Esc` during it leaves the plane at
/// the offset the drag began with (`Esc` cancels the drag, not just the HUD). Skips
/// without a GPU adapter.
#[gpui::test]
fn escape_during_a_stem_drag_keeps_the_pre_drag_offset(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, _model)) = offset_shell(cx) else {
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });
    let base = active_offset(&mut cx, &shell);

    // Press on the stem and move, but do not release.
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().expect("a viewport"));
    let (grab, move_to) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let camera = viewport.camera().clone();
        let plane = viewport.active_workplane().plane.clone();
        let (_, tip) = ferrocad_stage::WorkplaneStem::new(&plane)
            .handle_points(&camera)
            .expect("a projected stem");
        let grab = [tip[0].round() as i32, tip[1].round() as i32];
        (
            camera.viewport.to_logical(grab),
            camera.viewport.to_logical([grab[0] + 24, grab[1]]),
        )
    });
    let dragging = cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            assert!(viewport.handle_button(grab, 1, true));
            viewport.handle_pointer(move_to);
            viewport.manipulator(ferrocad_stage::WORKPLANE_OFFSET_ID)
                .is_some_and(|manipulator| manipulator.is_dragging())
        })
    });
    assert!(dragging, "the stem is mid-drag");

    // `Esc`: the viewport cancels the drag, the shell cancels the HUD.
    cx.update(|_window, cx| {
        viewport.update(cx, |viewport, _cx| {
            assert!(
                viewport.cancel_manipulator_drag(),
                "the drag is cancelled, not committed"
            );
            assert!(
                viewport.take_interaction_commit().is_none(),
                "a cancelled drag commits nothing"
            );
        })
    });
    press_escape(&mut cx, &shell);

    assert_eq!(
        active_offset(&mut cx, &shell),
        base,
        "a drag that never released applied nothing"
    );
    assert!(armed_ids(&mut cx, &shell).is_empty(), "the stem is gone");
}

/// W2: accepting the HUD keeps the offset the drag applied, and a typed value is the
/// session's offset as well. Skips without a GPU adapter.
#[gpui::test]
fn accepting_the_offset_hud_keeps_the_offset(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, _model)) = offset_shell(cx) else {
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });

    let commit = drag_stem(&mut cx, &shell, 24).expect("the drag commits");
    feed_commit(&mut cx, &shell, commit);
    let applied = active_offset(&mut cx, &shell);
    assert!(applied.abs() > 1e-6, "the drag applied an offset");

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.accept_dialog(cx))
    });

    assert_eq!(
        active_offset(&mut cx, &shell),
        applied,
        "accept keeps the offset the drag already applied"
    );
    assert!(
        cx.update(|_window, cx| shell.read(cx).dialog.is_none()),
        "the HUD closed"
    );
    assert!(
        armed_ids(&mut cx, &shell).is_empty(),
        "the stem ended with the session"
    );

    // The typed half applies the same offset the drag would: a second session starts
    // from `applied`, and typing `20` asks for 20 mm further along `+N`.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });
    let input = cx.update(|_window, cx| {
        shell
            .read(cx)
            .dialog
            .as_ref()
            .expect("the offset HUD")
            .inputs
            .first()
            .and_then(Option::as_ref)
            .cloned()
            .expect("one text field")
    });
    input.update(&mut cx, |input, cx| input.set_text("20".to_string(), cx));
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.accept_dialog(cx))
    });
    assert!(
        (active_offset(&mut cx, &shell) - (applied + 20.0)).abs() < 1e-6,
        "a typed value drives the same offset: {}",
        active_offset(&mut cx, &shell)
    );
    assert!(armed_ids(&mut cx, &shell).is_empty(), "the stem is gone");
}

/// W2: the offset session is two-way. A stem drag mirrors its live total into the HUD
/// field on **every tick** (not only on release), and typing a total moves the plane
/// and the stem with it. Skips without a GPU adapter.
#[gpui::test]
fn offsetting_a_workplane_drags_and_types_along_normal(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some((mut cx, shell, _model)) = offset_shell(cx) else {
        return;
    };

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_OffsetWorkplane", cx))
    });
    // Paint once so the viewport has adopted the window's real size: the drag points
    // below are device pixels, and a resize between the ticks would remap them.
    draw(&mut cx);
    let base = active_offset(&mut cx, &shell);
    let base_origin = active_origin(&mut cx, &shell);
    let normal = cx.update(|_window, cx| {
        shell
            .read(cx)
            .viewport
            .as_ref()
            .expect("a viewport")
            .read(cx)
            .active_workplane()
            .plane
            .n
    });

    // Press on the stem and travel half the drag, without releasing.
    let viewport = cx.update(|_window, cx| shell.read(cx).viewport.clone().expect("a viewport"));
    let (grab, mid, far) = cx.update(|_window, cx| {
        let viewport = viewport.read(cx);
        let camera = viewport.camera().clone();
        let plane = viewport.active_workplane().plane.clone();
        let (_, tip) = ferrocad_stage::WorkplaneStem::new(&plane)
            .handle_points(&camera)
            .expect("a projected stem");
        let grab = [tip[0].round() as i32, tip[1].round() as i32];
        (
            camera.viewport.to_logical(grab),
            camera.viewport.to_logical([grab[0] + 400, grab[1]]),
            camera.viewport.to_logical([grab[0] + 800, grab[1]]),
        )
    });

    // The drag tick: the viewport handler notifies the entity on a changed tick, which
    // is what wakes the shell's observation (the same path the camera mirror uses).
    let dragging = cx.update(|_window, cx| {
        viewport.update(cx, |viewport, cx| {
            assert!(viewport.handle_button(grab, 1, true), "the stem is grabbable");
            assert!(
                viewport.handle_pointer(mid),
                "the drag tick moved the stem"
            );
            cx.notify();
            viewport
                .manipulator(ferrocad_stage::WORKPLANE_OFFSET_ID)
                .is_some_and(|manipulator| manipulator.is_dragging())
        })
    });
    assert!(dragging, "the stem is mid-drag, not released");

    let mid_field: f64 = cx.update(|_window, cx| dialog_field(shell.read(cx), cx, "offset"))
        .parse()
        .unwrap_or(f64::NAN);
    assert!(
        mid_field > 0.0 && mid_field.is_finite(),
        "the field already tracks the drag before release: {mid_field}"
    );
    assert_eq!(
        active_offset(&mut cx, &shell),
        base,
        "an in-flight drag still applies nothing"
    );

    // Release: the commit applies the whole drag and the field keeps the total.
    let commit = cx
        .update(|_window, cx| {
            viewport.update(cx, |viewport, cx| {
                assert!(viewport.handle_pointer(far));
                assert!(
                    viewport.handle_button(far, 1, false),
                    "releasing the stem commits"
                );
                cx.notify();
                viewport.take_interaction_commit()
            })
        })
        .expect("the drag commits");
    feed_commit(&mut cx, &shell, commit);
    let dragged = active_offset(&mut cx, &shell) - base;
    assert!(
        dragged > mid_field,
        "the drag continued past the tick: {dragged} vs {mid_field}"
    );
    let released_field: f64 =
        cx.update(|_window, cx| dialog_field(shell.read(cx), cx, "offset"))
            .parse()
            .unwrap_or(f64::NAN);
    assert!(
        (released_field - dragged).abs() < 0.01,
        "the released field keeps the drag's total: {released_field} vs {dragged}"
    );
    // The mid-tick value is its own number, not a rounded copy of the release: the
    // field was written during the drag and written again when the drag ended.
    assert!(
        (released_field - mid_field).abs() > 0.02,
        "the field kept tracking after the mid tick: {mid_field} vs {released_field}"
    );

    // Typing the total is the other direction: the field drives the plane, and the
    // stem is re-armed on the plane it just moved.
    let typed = 5.0;
    let input = cx.update(|_window, cx| {
        shell
            .read(cx)
            .dialog
            .as_ref()
            .expect("the offset HUD")
            .inputs
            .first()
            .and_then(Option::as_ref)
            .cloned()
            .expect("one text field")
    });
    input.update(&mut cx, |input, cx| input.set_text(typed.to_string(), cx));
    // The dialog's change subscription is what a real keystroke calls; `set_text` is
    // programmatic and emits nothing, so drive the same entry point.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.preview_dialog(cx)));

    let typed_offset = active_offset(&mut cx, &shell);
    assert!(
        (typed_offset - (base + typed)).abs() < 1e-9,
        "typing drives the plane: {typed_offset}"
    );
    let origin = active_origin(&mut cx, &shell);
    assert!(
        origin.is_equal(&base_origin.add(&normal.scale(typed)), 1e-9),
        "the plane moved along `+N` by the typed value: {origin:?}"
    );
    let anchor = cx.update(|_window, cx| {
        viewport
            .read(cx)
            .manipulator(ferrocad_stage::WORKPLANE_OFFSET_ID)
            .and_then(|manipulator| manipulator.anchor())
    });
    assert!(
        anchor
            .expect("the stem has an anchor")
            .is_equal(&origin, 1e-9),
        "typing moves the stem with the plane"
    );
}
