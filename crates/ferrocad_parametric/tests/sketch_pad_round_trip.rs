//! End-to-end P4 SK6: draw a sketch, dimension it, and watch the pad update.
//!
//! The `Sketch_Polyline` command's tool collects snapped points on the active
//! workplane; the edition's creating hook turns them into a constrained
//! `Sketcher::SketchObject` and the `Part::Extrusion` that links it. `Sketch_Dimension`
//! then adds a driving datum at the edge's current value, and `setDatum` resizes the
//! profile — the pad follows on the next recompute.
//!
//! It needs OCCT, like the other edition round trips.

use std::sync::{Mutex, Once};

use ferrocad_graphics::{Hit, PickId, Topology};
use ferrocad_shell_python::python;
use ferrocad_stage::{MouseButton, PointerEvent, PointerKind, PolylineTool, Snap, SnapKind, Tool};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

/// Four snapped clicks on the tilted plane x = 5, as the sketch tool would receive.
fn draw_square(tag: &str) -> ferrocad_stage::InteractionCommit {
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
    let mut commit = tool.finish().expect("four clicks finish a polyline");
    // This is the one thing the edition's `TaggedPolyline` wrapper does: the tool is
    // generic, and the id it commits under is the workbench's choice.
    commit.manipulator = tag.to_string();
    commit
}

#[test]
fn the_sketch_pad_follows_a_dimension() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();

    // --- Creation -------------------------------------------------------------
    let commit = draw_square("sketch_polyline");
    let result = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(result.ok, "sketch create failed: {}", result.error);

    let created = evaluate(
        r#"
sketch = doc.getObject("Sketch")
extrusion = doc.getObject("Sketch_Extrusion")
assert sketch is not None, "the sketch exists"
assert extrusion is not None, "the pad exists"
assert sketch.TypeId == "Sketcher::SketchObject", sketch.TypeId
assert sketch.GeometryCount == 4, sketch.GeometryCount
# Four shared corners, the first point anchored on the sketch origin (the
# autodetector stages that pair for both segments meeting there), and the two
# horizontal and two vertical autoconstraints.
assert sketch.ConstraintCount == 10, sketch.ConstraintCount
assert sketch.solve() == 2, "anchored; width and height are free"
assert sketch.RedundantConstraints == [6], "the second origin pair adds no rank"
assert not sketch.Visibility, "the sketch is hidden"
assert extrusion.Shape.ShapeType == "Solid", extrusion.Shape.ShapeType
box = extrusion.Shape.BoundBox
assert abs(box.XMin - 5.0) < 1e-6 and abs(box.XMax - 15.0) < 1e-6, (box.XMin, box.XMax)
assert abs(box.YLength - 10.0) < 1e-6, box.YLength
assert abs(box.ZLength - 10.0) < 1e-6, box.ZLength
print("ok")
"#,
    );
    assert_eq!(created.trim(), "ok", "{created}");

    // --- Dimension ------------------------------------------------------------
    let dimension = evaluate(
        r#"
import FreeCADGui
FreeCADGui.Selection.clearSelection()
FreeCADGui.Selection.addSelection(doc.getObject("Sketch"), "Edge1")
result = FreeCADGui.runCommand("Sketch_Dimension")
assert "Distance" in result["status"], result
assert "datum" in result["status"], result
sketch = doc.getObject("Sketch")
assert sketch.ConstraintCount == 11, sketch.ConstraintCount
assert sketch.solve() == 1, "one datum leaves one free parameter"
print("ok")
"#,
    );
    assert_eq!(dimension.trim(), "ok", "{dimension}");

    // Editing the datum resizes the profile; the linked pad follows on recompute.
    let resized = evaluate(
        r#"
sketch = doc.getObject("Sketch")
sketch.setDatum(10, 20.0)         # the datum the command just added
doc.recompute()
box = doc.getObject("Sketch_Extrusion").Shape.BoundBox
assert abs(box.YLength - 20.0) < 1e-6, box.YLength
print("ok")
"#,
    );
    assert_eq!(resized.trim(), "ok", "{resized}");

    // --- The commands stay distinct ------------------------------------------
    let commands = evaluate(
        r#"
assert FreeCADGui.runCommand("Sketch_Polyline") == {"tool": "sketch_polyline"}
assert FreeCADGui.runCommand("Part_Polyline") == {"tool": "polyline"}
assert FreeCADGui.Selection.getSelectionEx()[0].SubElementNames == ["Edge1"]
print("ok")
"#,
    );
    assert_eq!(commands.trim(), "ok", "{commands}");

    // --- Undo fences ---------------------------------------------------------
    let undone = evaluate(
        r#"
doc.undo()                         # the dimension
assert doc.getObject("Sketch").ConstraintCount == 10, doc.getObject("Sketch").ConstraintCount
doc.undo()                         # the creation: sketch and pad together
assert doc.getObject("Sketch") is None
assert doc.getObject("Sketch_Extrusion") is None
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}
