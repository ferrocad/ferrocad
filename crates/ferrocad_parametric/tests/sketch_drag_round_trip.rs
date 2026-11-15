//! End-to-end P4 SK6: drag a sketch vertex through the declared plane grip.
//!
//! The chain under test is the one the app runs:
//!
//! ```text
//! Sketcher::SketchObject ──▶ the edition's `manipulator_specs` provider
//!        ──▶ one `plane` handle per vertex ──▶ the shell's JSON transport
//!        ──▶ DeclarativeManipulator ──▶ InteractionCommit {sketch_drag, drag, [u, v]}
//!        ──▶ the app script's `interaction` hook ──▶ SketchObject.drag ──▶ recompute
//! ```
//!
//! The result a drag has to prove is not "the point moved" but "the *constraints* moved
//! it": a horizontal edge keeps its height, and the linked `Part::Extrusion` follows. It
//! needs OCCT, like the other edition round trips.

use std::sync::{Mutex, Once};

use ferrocad_graphics::{Camera, Hit, PickId, Projection, Topology, Viewport};
use ferrocad_shell_python::python;
use ferrocad_shell_python::workbench::manipulators::handle_from_value;
use ferrocad_stage::screen::project_device;
use ferrocad_stage::{
    DeclarativeKind, DeclarativeManipulator, Manipulator, MouseButton, PointerEvent, PointerKind,
    PolylineTool, Snap, SnapKind, Tool,
};
use ferrocad_types::{Placement, Rotation, Vector3};

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

/// A camera looking down at the world `XY` plane, 100 mm out, so screen `+x` is world
/// `+X` and screen `+y` is world `+Y`.
fn camera() -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -100.0), Rotation::identity()).to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 1000.0,
        },
        viewport: Viewport::new([400, 400], 1.0),
    }
}

/// The device point of a world position, rounded to a click.
fn device(camera: &Camera, point: Vector3) -> [i32; 2] {
    let projected = project_device(camera, point).expect("the point projects");
    [projected[0].round() as i32, projected[1].round() as i32]
}

/// Four snapped clicks on the plane x = 5, as the sketch tool would receive them.
///
/// The profile is `(y, z)` = `(0,0) (10,0) (10,10) (0,10)`, so the sketch's local axes
/// are world `+Y` (u) and world `+Z` (v) with the normal along `+X`.
fn draw_square() -> ferrocad_stage::InteractionCommit {
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
                normal: [1.0, 0.0, 0.0],
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
    commit.manipulator = "sketch_polyline".to_string();
    commit
}

#[test]
fn a_vertex_grip_drags_the_sketch_and_the_constraints_decide_where_it_lands() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();

    let result = python::apply_interaction(&draw_square(), "").expect("the hook ran");
    assert!(result.ok, "sketch create failed: {}", result.error);

    // --- The provider declares one plane grip per shape vertex -------------------
    let specs = python::manipulator_specs("Sketch", Some("Vertex2")).expect("the provider ran");
    assert_eq!(specs.len(), 1, "one grip for the named vertex: {specs:?}");
    let (target, handle) = handle_from_value(&specs[0], 7).expect("a valid declared handle");
    assert_eq!(handle.id, "sketch_drag");
    assert_eq!(handle.action, "drag");
    assert_eq!(handle.kind, DeclarativeKind::Plane, "a vertex drags in its plane");
    assert_eq!(handle.element.as_deref(), Some("Vertex2"));
    // The frame is the sketch's own: `u` is world +Y, `v` is world +Z, the normal +X.
    assert!((handle.frame.basis[0].y - 1.0).abs() < 1e-9, "{handle:?}");
    assert!((handle.frame.basis[1].z - 1.0).abs() < 1e-9, "{handle:?}");
    assert!((handle.frame.basis[2].x - 1.0).abs() < 1e-9, "{handle:?}");
    assert!((handle.frame.origin.y - 10.0).abs() < 1e-9, "Vertex2 is (5, 10, 0)");
    let mut grip = DeclarativeManipulator::new(target, handle);

    // --- The typed plane value is the same commit a drag produces -----------------
    //
    // `Vertex2` is segment 0's end: `Horizontal` fixes its height and nothing fixes the
    // width, so `+5` along `u` must take and `+3` along `v` must be refused.
    assert!(grip.set_values(&[5.0, 3.0]));
    let commit = grip.pending_commit().expect("a commit");
    assert_eq!(commit.values, vec![5.0, 3.0]);
    assert_eq!(commit.action, "drag");
    assert_eq!(commit.element.as_deref(), Some("Vertex2"));

    let applied = python::apply_interaction(&commit, "Sketch").expect("the hook ran");
    assert!(applied.ok, "the drag failed: {}", applied.error);

    let moved = evaluate(
        r#"
sketch = doc.getObject("Sketch")
assert sketch.solve() == 2, "a drag leaves the free degrees of freedom alone"
assert sketch.RedundantConstraints == [6], sketch.RedundantConstraints
box = doc.getObject("Sketch_Extrusion").Shape.BoundBox
# The width grew by 5 (u), the height did not (the Horizontal constraint refused v).
assert abs(box.YLength - 15.0) < 1e-6, box.YLength
assert abs(box.ZLength - 10.0) < 1e-6, box.ZLength
print("ok")
"#,
    );
    assert_eq!(moved.trim(), "ok", "{moved}");

    // --- A real gesture produces the same shape of commit ------------------------
    //
    // The pointer path is the stage's (unit-tested for the ray/plane maths); what this
    // asserts is that the *grip* answers a press on the vertex, walks the pointer in the
    // sketch's own plane, and hands the two coordinates to the same document hook. A
    // second sketch on the world `XY` plane lets the standard top-down camera be
    // face-on to it.
    let setup = evaluate(
        r#"
import Part, Sketcher
sketch = doc.addObject("Sketcher::SketchObject", "Flat")
sketch.addGeometry([Part.LineSegment(FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(10, 0, 0))], False)
sketch.addConstraint(Sketcher.Constraint("Horizontal", 0))
sketch.solve()
doc.recompute()
print("ok")
"#,
    );
    assert_eq!(setup.trim(), "ok", "{setup}");

    let specs = python::manipulator_specs("Flat", Some("Vertex2")).expect("the provider ran");
    assert_eq!(specs.len(), 1, "one grip for the flat sketch's second vertex");
    let (target, handle) = handle_from_value(&specs[0], 7).expect("a valid declared handle");
    assert_eq!(handle.kind, DeclarativeKind::Plane);
    let mut flat = DeclarativeManipulator::new(target, handle);

    let camera = camera();
    flat.update(&camera);
    let press = device(&camera, Vector3::new(10.0, 0.0, 0.0));
    assert!(flat.hit(&camera, press), "the grip is on Vertex2");
    assert!(flat.begin_drag(&camera, press));
    assert!(flat.drag_update(&camera, [press[0] + 30, press[1] + 20]));
    let gesture = flat.drag_end().expect("a commit");
    assert_eq!(gesture.element.as_deref(), Some("Vertex2"));
    assert_eq!(gesture.values.len(), 2, "a plane drag commits both coordinates");
    assert!(gesture.values[0] > 0.0, "{gesture:?}");
    assert!(gesture.values[1] > 0.0, "{gesture:?}");

    let applied = python::apply_interaction(&gesture, "Flat").expect("the hook ran");
    assert!(applied.ok, "the gesture failed: {}", applied.error);

    let moved = evaluate(
        r#"
sketch = doc.getObject("Flat")
assert sketch.solve() == 3, "one horizontal line keeps three degrees of freedom"
start, end = sketch.Geometry[0].Vertexes
# The drag was in `+x` and `+y`; the Horizontal constraint keeps the two heights equal,
# so the line is still horizontal and it grew along its own direction.
assert abs(start.Point.y - end.Point.y) < 1e-6, (start.Point, end.Point)
assert abs(end.Point.x - start.Point.x) > 10.0, (start.Point, end.Point)
print("ok")
"#,
    );
    assert_eq!(moved.trim(), "ok", "{moved}");
}
