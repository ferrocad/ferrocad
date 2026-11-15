//! The Modify hand's element verbs, through the edition's own scripts.
//!
//! Three seams are pinned here, all headless:
//!
//! ```text
//! Delete/Backspace ──▶ App_Delete ──▶ FerroCAD.delete_selection
//!   ──▶ the Parametric delete provider ──▶ faces/edges removed, body healed, one undo
//!
//! a pick ──▶ FerroCAD.manipulator_specs ──▶ the edition's declared handle
//!   ──▶ InteractionCommit ──▶ the edition's `interaction` hook ──▶ one undo
//!
//! Part_Chamfer ──▶ the same feature the scrubber writes ──▶ its band re-opens as a chamfer
//! ```
//!
//! The disambiguation the mapping note asks for is asserted directly: the two compiled
//! manipulators own a planar face and an edge of a body that has faces, so the declared
//! handles answer exactly the picks those refuse — one pick, one handle.
//!
//! It needs OCCT, like the other edition round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_shell_python::selection::SelectionItem;
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

/// A fresh, empty document of this test's own, made active.
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

fn select(body: &str, elements: &[&str]) {
    let items = vec![SelectionItem::with_elements(
        body,
        elements.iter().map(|name| name.to_string()).collect(),
    )];
    python::set_selection(&items).expect("push the selection");
}

fn run(command: &str) -> String {
    let outcome = python::run_command(command).expect("the bridge");
    assert!(outcome.ok, "{command}: {}", outcome.error);
    outcome.status
}

fn faces() -> usize {
    evaluate("print(len(doc.getObject('Body').Shape.Faces))")
        .trim()
        .parse()
        .expect("a face count")
}

fn specs(object: &str, element: &str) -> Vec<serde_json::Value> {
    python::manipulator_specs(object, Some(element)).expect("the specs bridge")
}

#[test]
fn deleting_a_face_leaves_an_open_surface_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutDelete");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
print("ok")
"#,
    );

    select("Body", &["Face1"]);
    let status = run("App_Delete");
    assert!(status.contains("removed Face1"), "{status}");

    let after = evaluate(
        r#"
body = doc.getObject("Body")
assert body is not None, "the object survives: only its face went"
assert len(body.Shape.Faces) == 5, len(body.Shape.Faces)
assert not body.Shape.isClosed(), "five faces of a box are an open surface"
print("ok")
"#,
    );
    assert_eq!(after.trim(), "ok", "{after}");

    // One command, one transaction: a single undo restores the box.
    let undone = evaluate(
        r#"
doc.undo()
assert len(doc.getObject("Body").Shape.Faces) == 6, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn heal_refuses_a_result_that_cannot_close() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutHeal");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
print("ok")
"#,
    );

    select("Body", &["Face1"]);
    // A refused heal is a refusal, not a failure of the command: read the status.
    let refused = python::run_command("Part_HealFaces").expect("the bridge");
    let message = refused.status.to_lowercase();
    assert!(
        message.contains("cannot heal")
            || message.contains("opening")
            || message.contains("grow")
            || message.contains("close"),
        "{}",
        refused.status
    );
    assert_eq!(faces(), 6, "a refused heal leaves the body alone");
}

/// The heal SimpleCAD actually means: a pocket's floor and walls go, the body grows back.
#[test]
fn healing_a_pocket_closes_the_body_again() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutPocket");
    let pocket = evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
# Cut a 4 mm pocket into the top face and drop the splitter the cut leaves behind.
pocket = Part.makeBox(4, 4, 4, FreeCAD.Vector(3, 3, 6))
body.Shape = body.Shape.cut(pocket).removeSplitter()
doc.recompute()
assert len(body.Shape.Faces) == 11, len(body.Shape.Faces)
floor = next(
    index for index, face in enumerate(body.Shape.Faces, 1)
    if abs(face.CenterOfMass.z - 6.0) < 1e-6 and face.normalAt(0, 0).z > 0.9
)
walls = [
    index for index, face in enumerate(body.Shape.Faces, 1)
    if abs(face.Area - 16.0) < 1e-6 and face.CenterOfMass.z > 6.5
]
print(" ".join("Face%d" % index for index in [floor] + walls))
"#,
    );
    let names: Vec<&str> = pocket.split_whitespace().collect();
    assert_eq!(names.len(), 5, "a floor and four walls: {names:?}");
    select("Body", &names);
    let healed = python::run_command("Part_HealFaces").expect("the bridge");
    assert!(healed.ok, "{}", healed.status);

    let closed = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
assert body.Shape.isClosed(), "the pocket healed back into the box"
assert abs(body.Shape.Volume - 1000.0) < 1e-6, body.Shape.Volume
print("ok")
"#,
    );
    assert_eq!(closed.trim(), "ok", "{closed}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject("Body").Shape.Faces) == 11, len(doc.getObject("Body").Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

/// Two coplanar faces share a seam; deleting it is *merging* them, not removing anything.
#[test]
fn deleting_a_coplanar_edge_merges_the_faces() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutEdge");
    let shared = evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
V = FreeCAD.Vector
left = Part.Face(Part.makePolygon([V(0, 0, 0), V(10, 0, 0), V(10, 10, 0), V(0, 10, 0), V(0, 0, 0)]))
right = Part.Face(Part.makePolygon([V(10, 0, 0), V(20, 0, 0), V(20, 10, 0), V(10, 10, 0), V(10, 0, 0)]))
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeShell([left, right])
doc.recompute()
assert len(body.Shape.Faces) == 2, len(body.Shape.Faces)
# The seam is the edge at x = 10, y = 5.
seam = next(
    "Edge%d" % index
    for index, edge in enumerate(body.Shape.Edges, 1)
    if abs(edge.CenterOfMass.x - 10.0) < 1e-6 and abs(edge.CenterOfMass.y - 5.0) < 1e-6
)
print(seam)
"#,
    );
    select("Body", &[shared.trim()]);
    run("App_Delete");

    let merged = evaluate(
        r#"
body = doc.getObject("Body")
assert len(body.Shape.Faces) == 1, len(body.Shape.Faces)
assert len(body.Shape.Edges) == 4, len(body.Shape.Edges)
print("ok")
"#,
    );
    assert_eq!(merged.trim(), "ok", "{merged}");
}

/// One pick, one handle: the compiled families own what the declarations decline.
#[test]
fn the_declared_handles_answer_only_what_the_compiled_ones_refuse() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutSpecs");
    let curved = evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
box = doc.addObject("Part::Feature", "Box")
box.Shape = Part.makeBox(10, 10, 10)
cylinder = doc.addObject("Part::Feature", "Cylinder")
cylinder.Shape = Part.makeCylinder(5, 10)
ring = doc.addObject("Part::Feature", "Ring")
ring.Shape = Part.makeCircle(5)
doc.recompute()
wall = next(
    "Face%d" % index
    for index, face in enumerate(cylinder.Shape.Faces, 1)
    if not isinstance(face.Surface, Part.Plane)
)
print(wall)
"#,
    );
    let wall = curved.trim().to_string();
    // Arm the default verb, so the assertion does not depend on test order.
    run("SimpleCAD_Offset");

    // A planar face is push/pull's: the stem claims it, this provider declines it.
    assert!(
        specs("Box", "Face1").is_empty(),
        "a planar face belongs to the PushPullStem"
    );
    // An edge of a face-bearing body is the Fillet scrubber's.
    assert!(
        specs("Box", "Edge1").is_empty(),
        "a body's edge belongs to the DressUpScrubber"
    );

    let curved_specs = specs("Cylinder", &wall);
    assert_eq!(curved_specs.len(), 1, "{curved_specs:?}");
    assert_eq!(curved_specs[0]["action"], "offset");
    assert_eq!(curved_specs[0]["element"], wall.as_str());
    assert_eq!(curved_specs[0]["label"], "D");

    let edge_specs = specs("Ring", "Edge1");
    assert_eq!(edge_specs.len(), 1, "{edge_specs:?}");
    assert_eq!(edge_specs[0]["action"], "offset", "a faceless edge is ours");
}

#[test]
fn the_offset_handle_commits_in_one_step_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutOffset");
    let wall = evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
cylinder = doc.addObject("Part::Feature", "Cylinder")
cylinder.Shape = Part.makeCylinder(5, 10)
doc.recompute()
wall = next(
    "Face%d" % index
    for index, face in enumerate(cylinder.Shape.Faces, 1)
    if not isinstance(face.Surface, Part.Plane)
)
print(wall)
"#,
    );
    let wall = wall.trim().to_string();
    run("SimpleCAD_Offset");
    let spec = specs("Cylinder", &wall).remove(0);
    let action = spec["action"].as_str().expect("an action").to_string();

    let commit = ferrocad_stage::InteractionCommit::new(
        "simplecad_element",
        0,
        action.as_str(),
    )
    .with_element(wall.clone())
    .with_values([-1.0]);
    let applied = python::apply_interaction(&commit, "Cylinder").expect("the hook ran");
    assert!(applied.ok, "offset failed: {}", applied.error);

    let shrunk = evaluate(
        r#"
cylinder = doc.getObject("Cylinder")
width = cylinder.Shape.BoundBox.XLength
assert abs(width - 8.0) < 1e-6, width
print("ok")
"#,
    );
    assert_eq!(shrunk.trim(), "ok", "{shrunk}");

    let undone = evaluate(
        r#"
doc.undo()
width = doc.getObject("Cylinder").Shape.BoundBox.XLength
assert abs(width - 10.0) < 1e-6, width
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn the_elongate_handle_stretches_a_drawing_rim() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutElongate");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
rim = doc.addObject("Part::Feature", "Rim")
rim.Shape = Part.makeCircle(5)
doc.recompute()
print("ok")
"#,
    );
    run("SimpleCAD_Elongate");
    let spec = specs("Rim", "Edge1").remove(0);
    assert_eq!(spec["action"], "elongate");
    assert_eq!(spec["kind"], "axis");

    let commit = ferrocad_stage::InteractionCommit::new("simplecad_element", 0, "elongate")
        .with_element("Edge1")
        .with_values([2.0]);
    let applied = python::apply_interaction(&commit, "Rim").expect("the hook ran");
    assert!(applied.ok, "elongate failed: {}", applied.error);

    // The modelling layer draws the elongated profile as its own object, which is what
    // SimpleCAD's Elongate does: the source rim is the input, not the thing being edited.
    let stretched = evaluate(
        r#"
doc.recompute()
names = sorted(obj.Name for obj in doc.Objects)
assert names == ["Rim", "SimpleShape"], names
elongated = doc.getObject("SimpleShape").Shape
assert abs(elongated.BoundBox.XLength - 14.0) < 1e-6, elongated.BoundBox.XLength
assert abs(elongated.BoundBox.YLength - 10.0) < 1e-6, elongated.BoundBox.YLength
print("ok")
"#,
    );
    assert_eq!(stretched.trim(), "ok", "{stretched}");

    let undone = evaluate(
        r#"
doc.undo()
assert doc.getObject("SimpleShape") is None
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
    // Leave the default verb armed for any later case.
    run("SimpleCAD_Offset");
}

/// The drawing verb: an armed `Rotate` turns the picked arc about its own centre.
#[test]
fn the_rotate_handle_turns_a_drawing_arc() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutRotate");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
arc = doc.addObject("Part::Feature", "Arc")
arc.Shape = Part.makeCircle(10, FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(0, 0, 1), 0.0, 90.0)
doc.recompute()
print("ok")
"#,
    );
    run("SimpleCAD_RotateEdge");
    let spec = specs("Arc", "Edge1").remove(0);
    assert_eq!(spec["action"], "rotate");
    assert_eq!(spec["kind"], "ring");

    let commit = ferrocad_stage::InteractionCommit::new("simplecad_element", 0, "rotate")
        .with_element("Edge1")
        .with_values([45.0]);
    let applied = python::apply_interaction(&commit, "Arc").expect("the hook ran");
    assert!(applied.ok, "rotate failed: {}", applied.error);

    let turned = evaluate(
        r#"
import math
arc = doc.getObject("Arc")
edge = arc.Shape.Edges[0]
start = edge.valueAt(edge.FirstParameter)
assert abs(start.x - 7.0710678) < 1e-3, start
assert abs(start.y - 7.0710678) < 1e-3, start
assert abs(arc.Shape.Length - 5.0 * math.pi) < 1e-6, arc.Shape.Length
print("ok")
"#,
    );
    assert_eq!(turned.trim(), "ok", "{turned}");

    let undone = evaluate(
        r#"
arc = doc.getObject("Arc")
doc.undo()
# `undo` restores the stored shape; a recompute is what re-derives the view's copy.
doc.recompute()
edge = arc.Shape.Edges[0]
start = edge.valueAt(edge.FirstParameter)
assert abs(start.x - 10.0) < 1e-6, start
assert abs(start.y) < 1e-6, start
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
    // Leave the default verb armed for any later case.
    run("SimpleCAD_Offset");
}

/// A chamfer applied by a command is the scrubber's own feature, and re-opens as one.
#[test]
fn a_chamfer_command_applies_and_reopens_as_a_chamfer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    fresh_document("LoadoutChamfer");
    evaluate(
        r#"
import Part
doc = FreeCAD.ActiveDocument
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
print("ok")
"#,
    );

    select("Body", &["Edge1"]);
    run("Part_Chamfer");
    let band = evaluate(
        r#"
doc.recompute()
chamfer = doc.getObject("Body_Chamfer")
assert chamfer is not None, "the feature exists"
assert str(chamfer.TypeId) == "Part::Chamfer", chamfer.TypeId
base_edges = doc.getObject("Body").Shape.Edges
# The band is the one face whose every edge the feature added.
band = next(
    "Face%d" % index
    for index, face in enumerate(chamfer.Shape.Faces, 1)
    if face.Edges and all(
        not any(edge.isSame(base) for base in base_edges) for edge in face.Edges
    )
)
assert abs(float(chamfer.Distance) - float(chamfer.Distance2)) < 1e-9
print(band)
"#,
    );
    let band = band.trim().to_string();
    let reopened = specs("Body_Chamfer", &band);
    assert_eq!(reopened.len(), 1, "{reopened:?}");
    assert_eq!(
        reopened[0]["action"], "chamfer",
        "the band re-opens as the chamfer it is, not as a fillet"
    );
    assert_eq!(reopened[0]["label"], "D");

    // The feature is one transaction, so one undo removes it.
    let undone = evaluate(
        r#"
doc.undo()
assert doc.getObject("Body_Chamfer") is None
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}
