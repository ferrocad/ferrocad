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
            if let Some(viewport) = shell.viewport.clone() {
                viewport.update(cx, |viewport, _cx| viewport.set_stage(stage));
            }
            cx.notify();
        })
    });
    Some((vcx, shell))
}

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

/// W1 §3.3: a sub-element that a manipulator claims owns the handles, so the shared body
/// triad steps aside. Skips without a GPU adapter.
#[gpui::test]
fn a_claimed_sub_element_suppresses_the_shared_body_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
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
            "the face's stem owns the handles, so the body triad yields"
        );
        assert_eq!(
            viewport.manipulators().len(),
            1,
            "the claiming manipulator is armed"
        );
    });
}

/// W1 §3.3.2: an element **no** manipulator claims keeps the body triad, so a click never
/// leaves the user without handles. Skips without a GPU adapter.
#[gpui::test]
fn an_unclaimed_sub_element_keeps_the_body_triad(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    click_select(
        &mut cx,
        &shell,
        ferrocad_geom::SelectionRef::element(7, "Edge1"),
    );
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        let viewport = viewport.read(cx);
        assert!(
            !viewport.gizmos().is_empty(),
            "an unclaimed element falls back to the body triad"
        );
        assert!(viewport.manipulators().is_empty());
    });
}

/// W1 §3.3.3: `Move` is an explicit request for the body control, so it arms the triad
/// even while a face is selected. Skips without a GPU adapter.
#[gpui::test]
fn a_move_verb_arms_the_body_triad_on_a_face_selection(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let Some((mut cx, shell)) = face_claim_shell(cx, model) else {
        return;
    };
    let face = ferrocad_geom::SelectionRef::element(7, "Face2");
    click_select(&mut cx, &shell, face);
    cx.update(|_window, cx| {
        assert!(shell
            .read(cx)
            .viewport
            .as_ref()
            .unwrap()
            .read(cx)
            .gizmos()
            .is_empty());
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_Move", cx))
    });
    cx.update(|_window, cx| {
        let viewport = shell.read(cx).viewport.clone().unwrap();
        assert!(
            !viewport.read(cx).gizmos().is_empty(),
            "the verb overrides the element suppression"
        );
    });
}
