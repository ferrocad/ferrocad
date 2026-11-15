//! End-to-end SK4/SK5: a finished polyline becomes a wire and the extrusion that links
//! it, in one undoable transaction.
//!
//! The `Part_Polyline` command starts a native `polyline` tool whose points are
//! world-space (they were snapped on
//! the active workplane). The edition's creating-commit hook builds a parametric
//! `Part::FeaturePython` wire from them and a `Part::Extrusion` whose `Base` is that
//! wire; editing the wire's `Points` then moves the pad on the next recompute.
//!
//! It needs OCCT, like the other edition round trips.

use std::sync::{Mutex, Once};

use ferrocad_graphics::{Hit, PickId, Topology};
use ferrocad_shell_python::python;
use ferrocad_stage::{MouseButton, PointerEvent, PointerKind, PolylineTool, Snap, SnapKind, Tool};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the modules before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

/// Run a line on the console surface; a failing assertion is a test failure.
fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

#[test]
fn a_finished_polyline_creates_the_wire_and_its_extrusion_in_one_step() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();

    // Four snapped clicks on the tilted plane x = 5: the plane the tool would bind
    // from the first face press, and the `SnapContext` it would resolve against.
    let points: [[f32; 3]; 4] = [
        [5.0, 0.0, 0.0],
        [5.0, 10.0, 0.0],
        [5.0, 10.0, 10.0],
        [5.0, 0.0, 10.0],
    ];
    let mut tool = PolylineTool::new();
    for point in points {
        let event = PointerEvent {
            kind: PointerKind::Down,
            position: [0, 0],
            button: Some(MouseButton::Left),
            hit: Some(Hit {
                id: PickId(1),
                point,
                normal: [0.0, 0.0, 1.0],
                part: 0,
                element: Some(0),
                topology: Topology::Triangles,
                distance: 1.0,
            }),
            projection: None,
        };
        tool.on_event(
            &event,
            Some(Snap {
                point,
                kind: SnapKind::Point,
                local: None,
                major: false,
            }),
        );
    }
    let commit = tool.finish().expect("four clicks finish a polyline");
    // Every vertex survives the tool exactly: the commit carries the points it drew.
    assert_eq!(commit.values.len(), points.len() * 3);
    for (want, have) in points.iter().flatten().zip(&commit.values) {
        assert!(
            (*have - f64::from(*want)).abs() < 1e-6,
            "a snapped vertex moved: {want} -> {have}"
        );
    }

    let result = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(result.ok, "polyline create failed: {}", result.error);

    let created = evaluate(
        r#"
wire = doc.getObject("Polyline")
extrusion = doc.getObject("Polyline_Extrusion")
assert wire is not None, "the wire exists"
assert extrusion is not None, "the extrusion exists"
assert wire.TypeId == "Part::FeaturePython", wire.TypeId
assert wire.Shape.ShapeType == "Wire", wire.Shape.ShapeType
assert len(wire.Shape.Vertexes) == 4, len(wire.Shape.Vertexes)
# Every collected point is kept: the square is on the plane x = 5.
assert all(abs(v.Point.x - 5.0) < 1e-6 for v in wire.Shape.Vertexes), [
    tuple(v.Point) for v in wire.Shape.Vertexes
]
assert extrusion.Shape.ShapeType == "Solid", extrusion.Shape.ShapeType
box = extrusion.Shape.BoundBox
assert abs(box.XMin - 5.0) < 1e-6 and abs(box.XMax - 15.0) < 1e-6, (box.XMin, box.XMax)
assert abs(box.YLength - 10.0) < 1e-6, box.YLength
assert abs(box.ZLength - 10.0) < 1e-6, box.ZLength
# The wire is the pad's scaffolding, not a second body in the viewport.
assert not wire.Visibility, "the wire is hidden"
print("ok")
"#,
    );
    assert_eq!(created.trim(), "ok", "{created}");

    let undo_before: i64 = evaluate("doc.UndoCount").trim().parse().expect("UndoCount");

    // SK5's acceptance: editing the path updates the extrusion.
    let edited = evaluate(
        r#"
wire = doc.getObject("Polyline")
wire.Points = [5.0, 0.0, 0.0, 5.0, 20.0, 0.0, 5.0, 20.0, 10.0, 5.0, 0.0, 10.0]
doc.recompute()
box = doc.getObject("Polyline_Extrusion").Shape.BoundBox
assert abs(box.YLength - 20.0) < 1e-6, box.YLength
print("ok")
"#,
    );
    assert_eq!(edited.trim(), "ok", "{edited}");

    // One named transaction: one undo removes the wire and the pad together.
    let removed = evaluate(
        r#"
doc.undo()
assert doc.getObject("Polyline") is None, "the wire was in the transaction"
assert doc.getObject("Polyline_Extrusion") is None, "the pad was in the transaction"
print("ok")
"#,
    );
    assert_eq!(removed.trim(), "ok", "{removed}");
    let undo_after: i64 = evaluate("doc.UndoCount").trim().parse().expect("UndoCount");
    assert_eq!(undo_after, undo_before - 1, "the creation was one undo step");
}
