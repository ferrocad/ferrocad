//! End-to-end: the **Box tool** — the zero-mouse primitive and C2's field strip.
//!
//! Two promises are pinned here, both through the paths the window uses:
//!
//! ```text
//! Part_Box ──▶ {"tool": "box"}          (the command arms the tool; Search still runs it)
//! FieldStrip ──▶ Tool::set_field("w")   (a typed value is the tool's value)
//!   ──▶ Enter ──▶ InteractionCommit {box, create, [w, l, h, x, y, z]}
//!   ──▶ the edition's `interaction` hook ──▶ one Part::Feature, one transaction
//! ```
//!
//! It needs OCCT, like the other edition round trips.

use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_stage::{BoxTool, Tool};

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

#[test]
fn the_box_command_arms_the_tool_instead_of_creating_immediately() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    evaluate(
        r#"
if FreeCAD.getDocument("BoxToolArms"):
    FreeCAD.closeDocument("BoxToolArms")
doc = FreeCAD.newDocument("BoxToolArms")
FreeCAD.setActiveDocument("BoxToolArms")
print("ok")
"#,
    );

    // The command stays registered (Search and the app menu reach it), and what it does
    // is arm the tool: the old behaviour created a box the moment it was clicked, which
    // is exactly the button the creation pill replaced.
    let outcome = python::run_command("Part_Box").expect("the command ran");
    assert!(outcome.ok, "{}", outcome.error);
    assert_eq!(outcome.tool.as_deref(), Some("box"), "{outcome:?}");
    let objects = evaluate("print(len(doc.Objects))");
    assert_eq!(objects.trim(), "0", "arming writes nothing");
}

#[test]
fn a_typed_width_resizes_the_committed_box() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    evaluate(
        r#"
if FreeCAD.getDocument("BoxToolFields"):
    FreeCAD.closeDocument("BoxToolFields")
doc = FreeCAD.newDocument("BoxToolFields")
FreeCAD.setActiveDocument("BoxToolFields")
print("ok")
"#,
    );

    // The tool is registered by the edition's own contributions (`Part_Box` names it).
    let mut tool = BoxTool::new();
    assert_eq!(tool.size(), [20.0, 20.0, 20.0]);
    assert_eq!(
        tool.status()
            .fields
            .iter()
            .map(|field| field.value.as_str())
            .collect::<Vec<_>>(),
        vec!["20.00", "20.00", "20.00"]
    );

    // A typed width is the tool's value, and the commit carries it.
    assert!(tool.set_field("w", "30"), "the tool accepts 30 mm");
    let commit = tool.finish().expect("Enter commits the box");
    assert!(commit.creates);
    assert_eq!(commit.manipulator, "box");
    assert_eq!(&commit.values[0..3], &[30.0, 20.0, 20.0]);

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the box create failed: {}", applied.error);

    let measured = evaluate(
        r#"
import Part
box = doc.getObject("Box")
assert box is not None, [obj.Name for obj in doc.Objects]
bounds = box.Shape.BoundBox
assert abs(bounds.XLength - 30.0) < 1e-9, bounds.XLength
assert abs(bounds.YLength - 20.0) < 1e-9, bounds.YLength
assert abs(bounds.ZLength - 20.0) < 1e-9, bounds.ZLength
print("ok")
"#,
    );
    assert_eq!(measured.trim(), "ok", "{measured}");

    // One undo removes it: the hook writes in one named transaction.
    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert doc.getObject("Box") is None, [obj.Name for obj in doc.Objects]
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn the_zero_mouse_path_places_the_box_at_the_workplane_origin() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    evaluate(
        r#"
if FreeCAD.getDocument("BoxToolZeroMouse"):
    FreeCAD.closeDocument("BoxToolZeroMouse")
doc = FreeCAD.newDocument("BoxToolZeroMouse")
FreeCAD.setActiveDocument("BoxToolZeroMouse")
print("ok")
"#,
    );

    // No pointer at all: the tool is armed, the strip's numbers are the input, and
    // `Enter` commits at the placement origin (the world/workplane origin here).
    let mut tool = BoxTool::new();
    assert_eq!(tool.status().points, 0, "nothing is placed");
    let commit = tool.finish().expect("Enter commits with no click");
    assert_eq!(&commit.values[3..6], &[0.0, 0.0, 0.0]);

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the box create failed: {}", applied.error);
    let placed = evaluate(
        r#"
box = doc.getObject("Box")
origin = box.Placement.Base
assert abs(origin.x) < 1e-9 and abs(origin.y) < 1e-9 and abs(origin.z) < 1e-9, origin
print("ok")
"#,
    );
    assert_eq!(placed.trim(), "ok", "{placed}");
}
