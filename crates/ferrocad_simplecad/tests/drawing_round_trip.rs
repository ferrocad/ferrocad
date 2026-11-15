//! End-to-end: the SimpleCAD edition's drawing tools, through the real registry.
//!
//! The chain under test is the one the app runs:
//!
//! ```text
//! presses ──▶ Tool::on_event ──▶ completed() ──▶ Tool::finish
//!   ──▶ InteractionCommit {<tool>, create, [world geometry]}
//!   ──▶ the app script's interaction hook ──▶ a Part::Feature, one transaction
//! ```
//!
//! Each tool is taken from [`ferrocad_simplecad::contributions`] — the registry the
//! window installs — so a command whose id the edition forgot is a failure here rather
//! than a `no such tool` in the UI. Both face builders are checked where they can
//! differ: `simplecad.core.geometry` when the sibling checkout is on the path (the
//! edition's preferred path) and the equivalent `Part` construction when it is not.
//!
//! It needs OCCT, like the other edition round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_stage::{
    MouseButton, PointerEvent, PointerKind, PolygonTool, Snap, SnapKind, Tool,
};
use pyo3::types::PyAnyMethods;

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

static BOOT: Once = Once::new();

fn sibling_checkout() -> Option<std::path::PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../simplecad");
    candidate
        .is_dir()
        .then(|| candidate.canonicalize().unwrap_or(candidate))
}

fn boot() {
    BOOT.call_once(|| {
        // The built-ins must be registered before the interpreter starts, and the
        // checkout must be importable before the edition's script loads, so the order
        // is: inittab, then the path (which starts the interpreter), then the script.
        ferrocad_combined_py::register_builtins();
        if let Some(checkout) = sibling_checkout() {
            pyo3::Python::with_gil(|py| {
                let sys = py.import("sys").expect("sys");
                sys.getattr("path")
                    .expect("path")
                    .call_method1("insert", (0, checkout.to_string_lossy().to_string()))
                    .expect("insert the checkout on sys.path");
            });
        }
        python::configure("ferrocad_simplecad", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

/// Drive one tool from the edition's registry: a move and a press per point.
///
/// The move comes first because a tool that measures a sweep (the arc) reads it from
/// the pointer's motion — the same sequence the viewport delivers.
fn draw(tool: &str, points: &[[f32; 3]]) -> Option<ferrocad_stage::InteractionCommit> {
    let mut session = ferrocad_simplecad::contributions()
        .tool_for(tool)
        .unwrap_or_else(|| panic!("the edition registers `{tool}`"));
    press_all(&mut *session, points)
}

/// The same presses against a tool the test built itself (a polygon with a side count).
fn press_all(tool: &mut dyn Tool, points: &[[f32; 3]]) -> Option<ferrocad_stage::InteractionCommit> {
    for point in points {
        let snap = Some(Snap {
            point: *point,
            kind: SnapKind::Point,
            local: None,
            major: false,
        });
        for kind in [PointerKind::Move, PointerKind::Down] {
            let event = PointerEvent {
                kind,
                position: [0, 0],
                button: (kind == PointerKind::Down).then_some(MouseButton::Left),
                hit: None,
                projection: None,
            };
            tool.on_event(&event, snap);
        }
    }
    tool.finish()
}

/// Drop an object an earlier case left behind.
///
/// The cases share one interpreter and one document, and the hook names its objects
/// after the tool, so each case starts from a clean slate rather than depending on the
/// order the harness happens to run them in.
fn clear_object(name: &str) {
    let _ = python::evaluate(&format!(
        "if doc.getObject({name:?}) is not None:\n    doc.removeObject({name:?})"
    ));
}

#[test]
fn a_two_click_rectangle_becomes_a_part_face() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_object("Rectangle");

    // The edition registers the tool the shell would start, and it keeps the ids its
    // re-exported command set names: a command whose tool id does not resolve would
    // report "no such tool" the moment it is clicked.
    let contributions = ferrocad_simplecad::contributions();
    for id in [
        "rectangle",
        "circle",
        "polygon",
        "arc",
        "polyline",
        "sketch_polyline",
        "workplane_points",
    ] {
        assert!(contributions.tool_for(id).is_some(), "`{id}` resolves");
    }

    let commit = draw("rectangle", &[[0.0, 0.0, 0.0], [10.0, 4.0, 0.0]])
        .expect("the tool commits a rectangle");
    assert!(commit.creates, "a rectangle creates rather than edits");
    assert_eq!(commit.manipulator, "rectangle");
    assert_eq!(commit.action, "create");
    assert_eq!(commit.values.len(), 12, "four world corners");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "rectangle create failed: {}", applied.error);

    let made = evaluate(
        r#"
import ferrocad_simplecad
assert ferrocad_simplecad._simplecad_geometry is not None, (
    "the sibling checkout is on the path, so SimpleCAD's own builder runs"
)
made = doc.getObject("Rectangle")
assert made is not None, "the object exists"
assert made.TypeId == "Part::Feature", made.TypeId
face = made.Shape
assert face.ShapeType == "Face", face.ShapeType
assert face.isValid(), made.getStatusString()
assert abs(face.Area - 40.0) < 1e-6, face.Area
box = face.BoundBox
assert abs(box.XMin) < 1e-9 and abs(box.XMax - 10.0) < 1e-9, (box.XMin, box.XMax)
assert abs(box.YMin) < 1e-9 and abs(box.YMax - 4.0) < 1e-9, (box.YMin, box.YMax)
print("ok")
"#,
    );
    assert_eq!(made.trim(), "ok", "{made}");

    // The same corners through the `Part` builder — the path the script takes when the
    // SimpleCAD package is absent. The face must be identical, because the corners are
    // exact; only the construction differs.
    let equivalent = evaluate(
        r#"
import Part
values = [0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 4.0, 0.0, 0.0, 4.0, 0.0]
points = [FreeCAD.Vector(*values[i:i + 3]) for i in range(0, 12, 3)]
direct = Part.Face(Part.makePolygon(points + [points[0]]))
made = doc.getObject("Rectangle").Shape
assert direct.isValid()
assert abs(direct.Area - made.Area) < 1e-9, (direct.Area, made.Area)
print("ok")
"#,
    );
    assert_eq!(equivalent.trim(), "ok", "{equivalent}");

    // One undo removes the face: the hook writes in one named transaction.
    let undone = evaluate(
        r#"
doc.undo()
assert doc.getObject("Rectangle") is None
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

/// A circle is a disc: centre and rim in, one planar face out.
#[test]
fn a_two_click_circle_becomes_a_disc() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_object("Circle");

    let commit = draw("circle", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0]]).expect("a circle commit");
    assert_eq!(commit.values.len(), 9, "centre, rim, normal");
    assert_eq!(commit.readout, "Ø 10.0 mm");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "circle create failed: {}", applied.error);

    let made = evaluate(
        r#"
import math
made = doc.getObject("Circle")
assert made is not None, "the object exists"
face = made.Shape
assert face.ShapeType == "Face", face.ShapeType
assert face.isValid(), made.getStatusString()
assert abs(face.Area - 25.0 * math.pi) < 1e-6, face.Area
box = face.BoundBox
assert abs(box.XMin + 5.0) < 1e-9 and abs(box.XMax - 5.0) < 1e-9, (box.XMin, box.XMax)
assert abs(box.ZLength) < 1e-9, box.ZLength
print("ok")
"#,
    );
    assert_eq!(made.trim(), "ok", "{made}");

    // The `Part` path builds the same disc from the same numbers.
    let equivalent = evaluate(
        r#"
import Part
edge = Part.makeCircle(5.0, FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(0, 0, 1))
direct = Part.Face(Part.Wire([edge]))
made = doc.getObject("Circle").Shape
assert direct.isValid()
assert abs(direct.Area - made.Area) < 1e-9, (direct.Area, made.Area)
print("ok")
"#,
    );
    assert_eq!(equivalent.trim(), "ok", "{equivalent}");
}

/// The polygon's side count is the user's: six by default, four when the tool is set.
#[test]
fn a_polygon_is_a_regular_face_with_the_committed_sides() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_object("Polygon");

    let commit = draw("polygon", &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]])
        .expect("the default polygon commits");
    assert_eq!(commit.values[9], 6.0, "upstream's default is six sides");
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "polygon create failed: {}", applied.error);

    let hexagon = evaluate(
        r#"
import math
made = doc.getObject("Polygon")
face = made.Shape
assert face.ShapeType == "Face", face.ShapeType
assert face.isValid(), made.getStatusString()
expected = 300.0 * math.sin(math.pi / 3.0)   # (6/2) r² sin(2π/6), r = 10
assert abs(face.Area - expected) < 1e-6, (face.Area, expected)
assert len(face.Edges) == 6, len(face.Edges)
print("ok")
"#,
    );
    assert_eq!(hexagon.trim(), "ok", "{hexagon}");

    // The side count travels through the commit, not through a module-level setting:
    // a four-sided tool on the same document makes a square.
    clear_object("Polygon");
    let mut square = PolygonTool::new();
    assert!(square.set_sides(4));
    let commit = press_all(&mut square, &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]])
        .expect("the four-sided polygon commits");
    assert_eq!(commit.values[9], 4.0);
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "polygon create failed: {}", applied.error);
    let square = evaluate(
        r#"
made = doc.getObject("Polygon")
face = made.Shape
assert abs(face.Area - 200.0) < 1e-6, face.Area   # a square of side 10√2
assert len(face.Edges) == 4, len(face.Edges)
print("ok")
"#,
    );
    assert_eq!(square.trim(), "ok", "{square}");
}

/// An arc is an edge, not a face: the three points name the circle it lies on.
#[test]
fn a_three_click_arc_becomes_a_circular_edge() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_object("Arc");

    let commit = draw(
        "arc",
        &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
    )
    .expect("an arc commit");
    assert_eq!(commit.values.len(), 9, "start, mid, end");
    assert_eq!(commit.readout, "90.0° · R 10.0 mm");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "arc create failed: {}", applied.error);

    let made = evaluate(
        r#"
import math
import Part
made = doc.getObject("Arc")
assert made is not None, "the object exists"
edge = made.Shape
assert edge.ShapeType == "Edge", edge.ShapeType
assert edge.isValid(), made.getStatusString()
assert isinstance(edge.Curve, Part.Circle), edge.Curve
assert abs(edge.Length - 5.0 * math.pi) < 1e-6, edge.Length
box = edge.BoundBox
assert abs(box.XMax - 10.0) < 1e-6, (box.XMin, box.XMax)
assert abs(box.YMax - 10.0) < 1e-6, (box.YMin, box.YMax)
print("ok")
"#,
    );
    assert_eq!(made.trim(), "ok", "{made}");

    // One undo removes the edge: the hook writes in one named transaction.
    let undone = evaluate(
        r#"
doc.undo()
assert doc.getObject("Arc") is None
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

/// A click in place names no radius and commits nothing — for every round tool.
#[test]
fn a_degenerate_click_commits_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();

    assert!(
        draw("rectangle", &[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]).is_none(),
        "a click is not a rectangle"
    );
    assert!(
        draw("circle", &[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]).is_none(),
        "a click is not a circle"
    );
    assert!(
        draw("polygon", &[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]).is_none(),
        "a click is not a polygon"
    );
    assert!(
        draw("arc", &[[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [5.0, 0.0, 0.0]]).is_none(),
        "an arc with no sweep is not an arc"
    );
}

/// A vertical drag takes the vertical plane, and the face follows it.
#[test]
fn a_vertical_rectangle_lands_in_the_vertical_plane() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_object("Rectangle");

    let commit = draw("rectangle", &[[0.0, 0.0, 0.0], [6.0, 0.0, 3.0]])
        .expect("the tool commits a rectangle");
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "{}", applied.error);
    let check = evaluate(
        r#"
made = doc.getObject("Rectangle")
assert made is not None, "the object exists"
face = made.Shape
assert abs(face.Area - 18.0) < 1e-6, face.Area
box = face.BoundBox
# The drag had no `y` component, so the box lies in world `XZ`.
assert abs(box.YLength) < 1e-9, box.YLength
assert abs(box.XLength - 6.0) < 1e-9, box.XLength
assert abs(box.ZLength - 3.0) < 1e-9, box.ZLength
print("ok")
"#,
    );
    assert_eq!(check.trim(), "ok", "{check}");
}
