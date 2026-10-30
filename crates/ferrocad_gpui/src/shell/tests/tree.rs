//! Shell tests: the model tree's `Space`-to-hide and the datum plane.
//!
//! The tree is a Dev Tools pane, so its key path is the one that passes
//! `cancel_dialog = true` to [`Shell::handle_key`]. `Space` there toggles the
//! selected object's `Visibility` in one undo step; the main window keeps the
//! object-verb wheel on `Space`.

use super::*;

/// Rebuild the display map from the document (what `sync_stage` does when there
/// is a viewport; this test stays GPU-free).
fn rebuild_display(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|_window, cx| {
        shell.update(cx, |shell, _cx| {
            let _ = shell.build_stage();
        })
    });
}

/// The undo depth of the active document.
fn undo_count() -> usize {
    let result = crate::python::evaluate("FreeCAD.ActiveDocument.getAvailableUndos()")
        .expect("read the undo count");
    assert!(result.ok, "{}", result.output);
    result.output.trim().parse().expect("an integer undo count")
}

/// `true`/`false` for one object's `Visibility`.
fn visible(object: &str) -> bool {
    let name = serde_json::to_string(object).unwrap();
    let script = format!("FreeCAD.ActiveDocument.getObject({name}).Visibility");
    let result = crate::python::evaluate(&script).expect("read Visibility");
    assert!(result.ok, "{}", result.output);
    result.output.trim() == "True"
}

/// Create a `Part::DatumPlane` in the active document; `None` when this checkout
/// has no combined Part extension (the test then skips).
fn create_datum_plane(u: [f64; 3], v: [f64; 3]) -> Option<String> {
    let payload = serde_json::json!({
        "origin": [10.0, 20.0, 30.0],
        "u": u,
        "v": v,
    });
    let payload = serde_json::to_string(&payload.to_string()).unwrap();
    let script = format!(
        r#"
import json
import ferrocad_parametric
reply = json.loads(ferrocad_parametric.add_datum_plane({payload}))
print(reply["object"] if reply.get("ok") else "")
"#
    );
    let Ok(result) = crate::python::evaluate(&script) else {
        return None;
    };
    if !result.ok {
        // No combined Part extension (or no OCCT on the library path): this
        // checkout cannot create the object, so the test skips like the other
        // optional-artifact suites instead of failing on the environment.
        eprintln!("cannot create a Part::DatumPlane here; skipping:\n{}", result.output);
        return None;
    }
    let name = result.output.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// `Space` on a tree selection hides and shows it, one undo step each time, and
/// the toggle reaches the display path.
#[gpui::test]
fn space_in_the_tree_toggles_visibility_in_one_undo_step(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell, model) = palette_window(cx);

    // A real Part object, so the display hook (and the host's entity map) is
    // exercised: the tree row hides something that is actually drawn.
    let Some(name) = create_datum_plane([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]) else {
        eprintln!("no Part display provider; skipping tree-visibility test");
        return;
    };
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.refresh(cx)));
    rebuild_display(&shell, &mut cx);
    assert!(
        cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_some()),
        "the datum plane is drawn before it is hidden"
    );

    // A tree click is the shell's `select`, not a viewport pick.
    cx.update(|_window, cx| shell.update(cx, |shell, cx| shell.select(&name, cx)));
    let mut dev_cx = open_dev_tools(&mut cx, &shell);
    let before = undo_count();

    dev_cx.simulate_keystrokes("space");
    draw(&mut cx);

    assert_eq!(model.lock().unwrap().status, format!("hide {name}"));
    assert!(!visible(&name), "Space hid the object");
    assert_eq!(undo_count(), before + 1, "exactly one undo step");
    rebuild_display(&shell, &mut cx);
    assert!(
        cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_none()),
        "the hidden object left the display path"
    );

    // A second `Space` shows it again, again in one step.
    dev_cx.simulate_keystrokes("space");
    draw(&mut cx);

    assert_eq!(model.lock().unwrap().status, format!("show {name}"));
    assert!(visible(&name), "Space showed the object again");
    assert_eq!(undo_count(), before + 2, "one step per toggle");
    rebuild_display(&shell, &mut cx);
    assert!(
        cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_some()),
        "the object is drawn again"
    );

    // Each toggle is one undoable edit: undo the show, then the hide (which also
    // removes the property the first toggle added).
    let undo = crate::python::evaluate("FreeCAD.ActiveDocument.undo()").expect("undo");
    assert!(undo.ok, "{}", undo.output);
    assert!(!visible(&name), "undo restored the hidden state");
    rebuild_display(&shell, &mut cx);
    assert!(cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_none()));

    let undo = crate::python::evaluate("FreeCAD.ActiveDocument.undo()").expect("undo");
    assert!(undo.ok, "{}", undo.output);
    assert!(
        visible(&name),
        "undo removed the added property and restored the view visibility"
    );
    rebuild_display(&shell, &mut cx);
    assert!(cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_some()));
}

/// G5/Tier 2: `Set from selection` binds a real datum plane's `Face1` frame.
///
/// This runs the whole chain — the derived shape's element names, the display
/// map, `entity_of`, `part_names` and the face frame — on a `Part::DatumPlane`
/// rather than a hand-built mesh. Skips without a GPU adapter.
#[gpui::test]
fn a_datum_plane_face_binds_the_workplane(cx: &mut TestAppContext) {
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
        eprintln!("no GPU adapter available; skipping datum-plane bind test");
        return;
    }

    // A tilted frame, so the bound plane cannot pass as the world XY plane.
    let Some(name) = create_datum_plane([1.0, 0.0, 0.0], [0.0, 0.6, 0.8]) else {
        eprintln!("no Part display provider; skipping datum-plane bind test");
        return;
    };
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.refresh(cx);
            shell.sync_stage(cx);
        })
    });
    if !cx.update(|_window, cx| shell.read(cx).display.entity_of(&name).is_some()) {
        eprintln!("the datum plane is not displayed; skipping bind test");
        return;
    }

    // Select the object's one named face exactly as a single click does.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.replace_selection(
                vec![SelectionItem::with_elements(&name, vec!["Face1".to_string()])],
                cx,
            )
        })
    });
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.run_command("App_SetWorkplane", cx))
    });
    assert_eq!(model.lock().unwrap().status, "workplane: Face1");

    cx.update(|_window, cx| {
        let active = shell
            .read(cx)
            .viewport
            .as_ref()
            .unwrap()
            .read(cx)
            .active_workplane()
            .clone();
        assert_eq!(active.label, "Face1");
        assert!(!active.is_default());
        let origin = active.plane.origin;
        assert!(
            (origin.x - 10.0).abs() < 1e-6
                && (origin.y - 20.0).abs() < 1e-6
                && (origin.z - 30.0).abs() < 1e-6,
            "the datum's placement origin: {origin:?}"
        );
        // u × v for (1,0,0) and (0,0.6,0.8).
        let normal = active.plane.n;
        assert!(
            normal.x.abs() < 1e-6
                && (normal.y + 0.8).abs() < 1e-6
                && (normal.z - 0.6).abs() < 1e-6,
            "the datum's normal: {normal:?}"
        );
    });
}
