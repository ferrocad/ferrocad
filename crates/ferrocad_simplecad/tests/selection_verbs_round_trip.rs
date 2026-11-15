//! The selection verbs: command → gate → pure function → one transaction.
//!
//! Every case builds a selection the way a click would, runs the `SimpleCAD_*` command
//! through the shell's registry, and asserts both the geometry and that one undo takes
//! it back. The refusals matter as much as the results: a bad selection must return a
//! sentence and leave the document untouched.
//!
//! The verbs call the reference package's **pure** half (`surfaces/offset.py`,
//! `surfaces/edit.py`, `document/operations.py`). `surfaces/operations.py` is
//! deliberately not used: it hands the job to a worker pool the embedded interpreter
//! cannot start.
//!
//! It needs OCCT and the `../simplecad` checkout on the path, like the other round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_shell_python::selection::SelectionItem;
use pyo3::types::PyAnyMethods;

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

fn fresh_document(name: &str) {
    evaluate(&format!(
        r#"
if FreeCAD.getDocument({name:?}):
    FreeCAD.closeDocument({name:?})
doc = FreeCAD.newDocument({name:?})
FreeCAD.setActiveDocument({name:?})
print("ok")
"#
    ));
}

fn select(items: &[(&str, &[&str])]) {
    let items = items
        .iter()
        .map(|(object, elements)| {
            SelectionItem::with_elements(
                *object,
                elements.iter().map(|name| name.to_string()).collect(),
            )
        })
        .collect::<Vec<_>>();
    python::set_selection(&items).expect("push the selection");
}

fn run(command: &str) -> (bool, String) {
    let outcome = python::run_command(command).expect("the bridge");
    (outcome.ok, outcome.status)
}

fn box_body() -> String {
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
print("ok")
"#,
    )
    .trim()
    .to_string()
}

#[test]
fn shell_hollows_the_body_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsShell");
    box_body();

    // Face6 is the top: the command leaves it open, so the walls get thickness.
    select(&[("Body", &["Face6"])]);
    let (ok, status) = run("SimpleCAD_Shell");
    assert!(ok, "{status}");

    let hollow = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 11, len(body.Shape.Faces)
assert body.Shape.Volume < 1000.0, body.Shape.Volume
assert body.Shape.isValid()
print("ok")
"#,
    );
    assert_eq!(hollow.trim(), "ok", "{hollow}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject("Body").Shape.Faces) == 6, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn draft_tilts_the_walls_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsDraft");
    box_body();

    // The four walls share one hinge, which is what the operation requires.
    select(&[("Body", &["Face1", "Face2", "Face3", "Face4"])]);
    let (ok, status) = run("SimpleCAD_Draft");
    assert!(ok, "{status}");

    let drafted = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
top = max(body.Shape.Faces, key=lambda face: face.CenterOfMass.z)
assert top.Area < 99.0, top.Area   # narrowed from 10 x 10
assert body.Shape.isValid()
print("ok")
"#,
    );
    assert_eq!(drafted.trim(), "ok", "{drafted}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
top = max(doc.getObject("Body").Shape.Faces, key=lambda face: face.CenterOfMass.z)
assert abs(top.Area - 100.0) < 1e-6, top.Area
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn fill_hole_closes_a_surface_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsFill");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeShell(Part.makeBox(10, 10, 10).Faces[1:])
doc.recompute()
assert len(body.Shape.Faces) == 5, len(body.Shape.Faces)
print("ok")
"#,
    );

    select(&[("Body", &[])]);
    let (ok, status) = run("SimpleCAD_FillHole");
    assert!(ok, "{status}");

    let closed = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
assert body.Shape.isClosed(), "the opening is closed"
print("ok")
"#,
    );
    assert_eq!(closed.trim(), "ok", "{closed}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject("Body").Shape.Faces) == 5, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn remove_rounds_takes_the_fillet_off_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsRounds");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
box = Part.makeBox(10, 10, 10)
body = doc.addObject("Part::Feature", "Body")
body.Shape = box.makeFillet(2.0, [box.Edges[0]])
doc.recompute()
assert len(body.Shape.Faces) == 7, len(body.Shape.Faces)
print("ok")
"#,
    );

    select(&[("Body", &[])]);
    let (ok, status) = run("SimpleCAD_RemoveRounds");
    assert!(ok, "{status}");

    let squared = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
assert abs(body.Shape.Volume - 1000.0) < 1e-6, body.Shape.Volume
print("ok")
"#,
    );
    assert_eq!(squared.trim(), "ok", "{squared}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject("Body").Shape.Faces) == 7, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn loft_forms_a_solid_between_two_wires() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsLoft");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
V = FreeCAD.Vector
low = doc.addObject("Part::Feature", "Low")
low.Shape = Part.makePolygon([V(0, 0, 0), V(10, 0, 0), V(10, 10, 0), V(0, 10, 0), V(0, 0, 0)])
high = doc.addObject("Part::Feature", "High")
high.Shape = Part.makePolygon([V(-5, -5, 10), V(15, -5, 10), V(15, 15, 10), V(-5, 15, 10), V(-5, -5, 10)])
doc.recompute()
print("ok")
"#,
    );

    select(&[("Low", &[]), ("High", &[])]);
    let (ok, status) = run("SimpleCAD_Loft");
    assert!(ok, "{status}");

    let lofted = evaluate(
        r#"
doc.recompute()
shapes = [obj for obj in doc.Objects if str(obj.TypeId) == "Part::Feature" and obj.Name not in ("Low", "High")]
assert len(shapes) == 1, [obj.Name for obj in doc.Objects]
solid = shapes[0].Shape
assert solid.ShapeType == "Solid", solid.ShapeType
assert solid.isValid() and solid.Volume > 0.0
print("ok")
"#,
    );
    assert_eq!(lofted.trim(), "ok", "{lofted}");

    let undone = evaluate(
        r#"
doc.undo()
assert len(doc.Objects) == 2, [obj.Name for obj in doc.Objects]
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn slice_splits_the_body_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsSlice");
    box_body();

    select(&[("Body", &[])]);
    let (ok, status) = run("SimpleCAD_Slice");
    assert!(ok, "{status}");

    let pieces = evaluate(
        r#"
doc.recompute()
assert len(doc.Objects) == 2, [obj.Name for obj in doc.Objects]
assert all(len(obj.Shape.Solids) == 1 for obj in doc.Objects), [
    len(obj.Shape.Solids) for obj in doc.Objects
]
print("ok")
"#,
    );
    assert_eq!(pieces.trim(), "ok", "{pieces}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.Objects) == 1, [obj.Name for obj in doc.Objects]
assert len(doc.getObject("Body").Shape.Faces) == 6
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn subdivide_divides_a_face_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsSubdivide");
    box_body();

    select(&[("Body", &["Face6"])]);
    let (ok, status) = run("SimpleCAD_Subdivide");
    assert!(ok, "{status}");

    let divided = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 14, len(body.Shape.Faces)   # 6 - 1 + 9
assert body.Shape.isValid()
print("ok")
"#,
    );
    assert_eq!(divided.trim(), "ok", "{divided}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject("Body").Shape.Faces) == 6, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn union_joins_two_bodies_into_one() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsUnion");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
left = doc.addObject("Part::Feature", "Left")
left.Shape = Part.makeBox(10, 10, 10)
right = doc.addObject("Part::Feature", "Right")
right.Shape = Part.makeBox(10, 10, 10, FreeCAD.Vector(5, 0, 0))
doc.recompute()
print("ok")
"#,
    );

    select(&[("Left", &[]), ("Right", &[])]);
    let (ok, status) = run("SimpleCAD_Union");
    assert!(ok, "{status}");

    let joined = evaluate(
        r#"
doc.recompute()
assert len(doc.Objects) == 1, [obj.Name for obj in doc.Objects]
assert abs(doc.Objects[0].Shape.Volume - 1500.0) < 1e-6, doc.Objects[0].Shape.Volume
print("ok")
"#,
    );
    assert_eq!(joined.trim(), "ok", "{joined}");
}

/// A selection the verb cannot serve is a sentence, not an exception, and writes nothing.
#[test]
fn a_bad_selection_is_refused_and_the_document_is_untouched() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsRefusal");
    box_body();

    // Unwrap wants exactly one face; two is a refusal.
    select(&[("Body", &["Face5", "Face6"])]);
    let (ok, status) = run("SimpleCAD_Unwrap");
    assert!(!ok, "two faces are not one: {status}");
    assert!(status.contains("exactly one face"), "{status}");

    // Shell wants faces; an edge is the wrong kind.
    select(&[("Body", &["Edge1"])]);
    let (ok, status) = run("SimpleCAD_Shell");
    assert!(!ok, "an edge is not a face: {status}");
    assert!(status.contains("faces only"), "{status}");

    // Union wants whole objects; faces are refused.
    select(&[("Body", &["Face5"]), ("Body", &["Face6"])]);
    let (ok, status) = run("SimpleCAD_Union");
    assert!(!ok, "{status}");
    assert!(status.contains("whole objects"), "{status}");

    // Nothing was written by any of them, and no undo step was spent.
    let untouched = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
assert len(doc.Objects) == 1, [obj.Name for obj in doc.Objects]
print("ok")
"#,
    );
    assert_eq!(untouched.trim(), "ok", "{untouched}");
}

/// The verbs take a value; the command passes its default, the function takes the number.
#[test]
fn a_verb_runs_at_the_value_it_is_given() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("VerbsValue");
    box_body();
    select(&[("Body", &["Face6"])]);

    let thicker = evaluate(
        r#"
import ferrocad_simplecad
result = ferrocad_simplecad._run_verb("Shell", 2.0)
assert result["ok"], result
print(result["status"])
"#,
    );
    assert!(thicker.contains("2.0 mm"), "{thicker}");
    let wall = evaluate(
        r#"
body = doc.getObject("Body")
assert body.Shape.Volume < 1000.0, body.Shape.Volume
# A 2 mm wall leaves less material than a 1 mm one.
assert body.Shape.Volume < 800.0, body.Shape.Volume
print("ok")
"#,
    );
    assert_eq!(wall.trim(), "ok", "{wall}");
}
