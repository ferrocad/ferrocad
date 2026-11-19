//! End-to-end: the **round primitives** — cylinder, sphere and cone from the Solid tier.
//!
//! The same two promises the Box tool's round trip pins, for the other SimpleCAD solids:
//!
//! ```text
//! Part_Cylinder ──▶ {"tool": "cylinder"}     (the command arms the tool)
//! FieldStrip ──▶ Tool::set_field("r")        (a typed value is the tool's value)
//!   ──▶ Enter ──▶ InteractionCommit {cylinder, create, [r, h, x, y, z, nx, ny, nz]}
//!   ──▶ the edition's `interaction` hook ──▶ one Part::Feature, one transaction
//! ```
//!
//! The axis travels with the commit, so a cylinder placed on a face grows out of that
//! face rather than out of the world's `+Z`. It needs OCCT, like the other edition round
//! trips.

use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_stage::gizmo::ManipulatorFrame;
use ferrocad_stage::{HoverTarget, PrimitiveTool, TargetKind, Tool};
use ferrocad_types::Vector3;

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

/// A fresh document, so the tests do not see each other's objects.
fn document(name: &str) {
    evaluate(&format!(
        r#"
if FreeCAD.getDocument("{name}"):
    FreeCAD.closeDocument("{name}")
doc = FreeCAD.newDocument("{name}")
FreeCAD.setActiveDocument("{name}")
print("ok")
"#
    ));
}

#[test]
fn the_solid_commands_arm_their_tools() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    document("PrimitiveToolsArm");

    for (command, tool) in [
        ("Part_Cylinder", "cylinder"),
        ("Part_Sphere", "sphere"),
        ("Part_Cone", "cone"),
    ] {
        let outcome = python::run_command(command).expect("the command ran");
        assert!(outcome.ok, "{command}: {}", outcome.error);
        assert_eq!(outcome.tool.as_deref(), Some(tool), "{command}: {outcome:?}");
    }
    let objects = evaluate("print(len(doc.Objects))");
    assert_eq!(objects.trim(), "0", "arming writes nothing");
}

#[test]
fn a_typed_radius_and_height_are_the_committed_cylinder() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    document("CylinderToolRoundTrip");

    let mut tool = PrimitiveTool::cylinder();
    assert_eq!(
        tool.status()
            .fields
            .iter()
            .map(|field| field.id.as_str())
            .collect::<Vec<_>>(),
        vec!["r", "h"]
    );
    assert!(tool.set_field("r", "5"), "the tool accepts 5 mm");
    assert!(tool.set_field("h", "20"));
    let commit = tool.finish().expect("Enter commits the cylinder");
    assert!(commit.creates);
    assert_eq!(commit.manipulator, "cylinder");
    assert_eq!(&commit.values[0..2], &[5.0, 20.0]);
    assert_eq!(&commit.values[5..8], &[0.0, 0.0, 1.0], "the world axis");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the cylinder create failed: {}", applied.error);

    let measured = evaluate(
        r#"
import Part
cylinder = doc.getObject("Cylinder")
assert cylinder is not None, [obj.Name for obj in doc.Objects]
bounds = cylinder.Shape.BoundBox
assert abs(bounds.XLength - 10.0) < 1e-6, bounds.XLength
assert abs(bounds.YLength - 10.0) < 1e-6, bounds.YLength
assert abs(bounds.ZLength - 20.0) < 1e-6, bounds.ZLength
print("ok")
"#,
    );
    assert_eq!(measured.trim(), "ok", "{measured}");

    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert doc.getObject("Cylinder") is None, [obj.Name for obj in doc.Objects]
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "one undo removes it: {undone}");
}

#[test]
fn the_axis_a_face_hands_over_is_the_one_the_object_grows_along() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    document("CylinderOnFace");

    // A placement frame whose normal is `+X`: the tool's commit carries it, and the
    // kernel builds the cylinder along it instead of the world's `+Z`.
    let mut tool = PrimitiveTool::cylinder();
    tool.hover(Some(&HoverTarget {
        frame: ManipulatorFrame {
            origin: Vector3::zero(),
            basis: [
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            ],
        },
        host: None,
        kind: TargetKind::Workplane,
    }));
    let commit = tool.finish().expect("a commit");
    assert_eq!(&commit.values[5..8], &[1.0, 0.0, 0.0], "the frame's normal");
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the cylinder create failed: {}", applied.error);

    let measured = evaluate(
        r#"
cylinder = doc.getObject("Cylinder")
assert cylinder is not None, [obj.Name for obj in doc.Objects]
bounds = cylinder.Shape.BoundBox
# Along +X the height runs, so that is the long side; the radius spans Y and Z.
assert abs(bounds.XLength - 20.0) < 1e-6, bounds.XLength
assert abs(bounds.YLength - 10.0) < 1e-6, bounds.YLength
assert abs(bounds.ZLength - 10.0) < 1e-6, bounds.ZLength
print("ok")
"#,
    );
    assert_eq!(measured.trim(), "ok", "{measured}");
}

#[test]
fn a_sphere_is_one_object_around_its_centre() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    document("SphereToolRoundTrip");

    let mut tool = PrimitiveTool::sphere();
    assert_eq!(
        tool.status()
            .fields
            .iter()
            .map(|field| field.id.as_str())
            .collect::<Vec<_>>(),
        vec!["r"]
    );
    assert!(tool.set_field("r", "10"));
    let commit = tool.finish().expect("Enter commits the sphere");
    assert_eq!(commit.manipulator, "sphere");
    assert_eq!(commit.values, vec![10.0, 0.0, 0.0, 0.0], "radius and centre");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the sphere create failed: {}", applied.error);

    let measured = evaluate(
        r#"
sphere = doc.getObject("Sphere")
assert sphere is not None, [obj.Name for obj in doc.Objects]
bounds = sphere.Shape.BoundBox
for length in (bounds.XLength, bounds.YLength, bounds.ZLength):
    assert abs(length - 20.0) < 1e-6, bounds
print("ok")
"#,
    );
    assert_eq!(measured.trim(), "ok", "{measured}");
}

#[test]
fn a_cone_commits_both_radii_and_refuses_a_solid_with_no_end() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    document("ConeToolRoundTrip");

    // A frustum: both ends open.
    let mut tool = PrimitiveTool::cone();
    assert!(tool.set_field("r1", "10"));
    assert!(tool.set_field("r2", "4"));
    assert!(tool.set_field("h", "20"));
    let commit = tool.finish().expect("Enter commits the frustum");
    assert_eq!(commit.manipulator, "cone");
    assert_eq!(&commit.values[0..3], &[10.0, 4.0, 20.0]);

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the cone create failed: {}", applied.error);
    let measured = evaluate(
        r#"
cone = doc.getObject("Cone")
assert cone is not None, [obj.Name for obj in doc.Objects]
bounds = cone.Shape.BoundBox
assert abs(bounds.XLength - 20.0) < 1e-6, bounds.XLength
assert abs(bounds.ZLength - 20.0) < 1e-6, bounds.ZLength
print("ok")
"#,
    );
    assert_eq!(measured.trim(), "ok", "{measured}");

    // Both radii at zero is no solid, and the tool refuses to commit it.
    let mut flat = PrimitiveTool::cone();
    assert!(flat.set_field("r1", "0"));
    assert_eq!(flat.finish(), None);
}
