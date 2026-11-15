//! End-to-end: the SimpleCAD edition's native rectangle, through the real registry.
//!
//! The chain under test is the one the app runs:
//!
//! ```text
//! two presses ──▶ RectangleTool::on_event ──▶ completed() ──▶ Tool::finish
//!   ──▶ InteractionCommit {rectangle, create, [12 corners]}
//!   ──▶ the app script's interaction hook ──▶ a Part::Feature face, one transaction
//! ```
//!
//! Both face builders are checked: `simplecad.core.geometry.rectangle` when the
//! sibling checkout is on the path (the edition's preferred path) and the equivalent
//! `Part` construction when it is not. They must produce the same face — same area,
//! same bounds — because the corners are already exact.
//!
//! It needs OCCT, like the other edition round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_stage::{
    MouseButton, PointerEvent, PointerKind, RectangleTool, Snap, SnapKind, Tool,
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

/// Press two world points through the real tool and return its commit.
fn draw(first: [f32; 3], second: [f32; 3]) -> Option<ferrocad_stage::InteractionCommit> {
    let mut tool = RectangleTool::new();
    for point in [first, second] {
        let event = PointerEvent {
            kind: PointerKind::Down,
            position: [0, 0],
            button: Some(MouseButton::Left),
            hit: None,
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
    tool.finish()
}

/// Drop any `Rectangle` an earlier case left behind.
///
/// The cases share one interpreter and one document, and the hook names its object
/// `Rectangle`, so each case starts from a clean slate rather than depending on the
/// order the harness happens to run them in.
fn clear_rectangle() {
    let _ = python::evaluate(
        "if doc.getObject('Rectangle') is not None:\n    doc.removeObject('Rectangle')",
    );
}

#[test]
fn a_two_click_rectangle_becomes_a_part_face() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_rectangle();

    // The edition registers the tool the shell would start, and it keeps the ids its
    // re-exported command set names: a command whose tool id does not resolve would
    // report "no such tool" the moment it is clicked.
    let contributions = ferrocad_simplecad::contributions();
    for id in [
        "rectangle",
        "polyline",
        "sketch_polyline",
        "workplane_points",
    ] {
        assert!(contributions.tool_for(id).is_some(), "`{id}` resolves");
    }

    let commit = draw([0.0, 0.0, 0.0], [10.0, 4.0, 0.0]).expect("the tool commits a rectangle");
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

#[test]
fn a_degenerate_second_click_commits_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();

    assert!(
        draw([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]).is_none(),
        "a click is not a rectangle"
    );
}

/// A vertical drag takes the vertical plane, and the face follows it.
#[test]
fn a_vertical_rectangle_lands_in_the_vertical_plane() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    clear_rectangle();

    let commit = draw([0.0, 0.0, 0.0], [6.0, 0.0, 3.0]).expect("the tool commits a rectangle");
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
