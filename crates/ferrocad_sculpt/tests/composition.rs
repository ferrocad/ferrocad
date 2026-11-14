//! The Sculpt edition composes: one interpreter, the built-in modules, the app
//! script's commands and the bundle's document model.
//!
//! It boots the way the desktop boots — `ferrocad_combined_py::register_builtins`
//! before any interpreter starts, then `python::configure("ferrocad_sculpt", …)` —
//! so the script, the modules and the engine share one `ferrocad_core` registry.
//! Importing the `.abi3.so` modules instead would give the test a second core and
//! measure nothing.
//!
//! Two things are under test: the edition's own startup sample (a cage, its skin
//! and the body that nests them) and the command surface the app script registers.
//! Both need OCCT.

use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// The modules must be registered before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        // `gui = true` mirrors the desktop: `FreeCAD.GuiUp` is set, so the app
        // script's command registration runs the same path. No window or GPU is
        // involved (this is the headless console surface).
        python::configure("ferrocad_sculpt", &[], &[], true);
    });
    python::bootstrap().expect("boot the edition");
}

/// Run a script on the console surface and return its output.
fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

/// A fresh document, so each test starts from a known one.
const FRESH_DOCUMENT: &str = r#"
import FreeCAD
for name in ("SculptEdition", "SculptEdition2"):
    if FreeCAD.getDocument(name) is not None:
        FreeCAD.closeDocument(name)
"#;

#[test]
fn the_edition_boots_a_sculpted_sample() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    let output = evaluate(
        r#"
import FreeCAD
from Sculpt import role_of
sample = FreeCAD.getDocument("Sample")
assert sample is not None, "the canonical sample did not open"
cage = sample.getObject("SculptCage")
skin = sample.getObject("SculptSkin")
body = sample.getObject("SculptBody")
assert cage is not None and skin is not None and body is not None, sample.Objects
assert cage.TypeId == "Mesh::FeaturePython", cage.TypeId
assert skin.TypeId == "Part::FeaturePython", skin.TypeId
assert role_of(body, cage) == "support", role_of(body, cage)
assert role_of(body, skin) == "base", role_of(body, skin)
assert body.Shape.isValid(), body.getStatusString()
assert body.Shape.ShapeType == "Solid", body.Shape.ShapeType
assert body.Shape.Volume > 0.0, body.Shape.Volume
print("sample ok", round(body.Shape.Volume, 3))
"#,
    );
    assert!(output.contains("sample ok"), "{output}");
}

#[test]
fn the_commands_are_registered_and_run_headless() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    evaluate(FRESH_DOCUMENT);
    let output = evaluate(
        r#"
import FreeCAD, FreeCADGui

expected = [
    "Sculpt_MakeCage", "Sculpt_MakeSkin", "Sculpt_NestInBody",
    "Sculpt_LevelUp", "Sculpt_LevelDown", "Sculpt_AttachProxies",
]
registered = FreeCADGui.listCommands()
for name in expected:
    assert name in registered, name
assert "Part_Box" in registered, "the Parametric command set did not arrive"

# The create -> skin -> nest flow, with one undo step per command.
doc = FreeCAD.newDocument("SculptEdition")
created = FreeCADGui.runCommand("Sculpt_MakeCage")
cage = doc.getObject(created["object"])
assert cage is not None, created
assert cage.Mesh.CountFacets > 0, cage.Mesh.CountFacets

skinned = FreeCADGui.runCommand("Sculpt_MakeSkin")
skin = doc.getObject(skinned["object"])
assert skin.isValid(), skin.getStatusString()

nested = FreeCADGui.runCommand("Sculpt_NestInBody")
body = doc.getObject(nested["body"])
assert nested["roles"] == ["support", "base"], nested["roles"]
assert body.Shape.ShapeType == "Solid", body.Shape.ShapeType
assert body.Shape.isValid(), body.getStatusString()

# Undo is the document's: every command is one transaction, so one undo removes
# the nesting and the body it created.
doc.undo()
doc.recompute()
assert doc.getObject("SculptBody") is None, [o.Name for o in doc.Objects]
assert doc.getObject("Skin") is not None, "the earlier commands stay undone-in"

# Levels and the attach command on their own document.
doc = FreeCAD.newDocument("SculptEdition2")
cage = doc.getObject(FreeCADGui.runCommand("Sculpt_MakeCage")["object"])
before = int(cage.Levels)
up = FreeCADGui.runCommand("Sculpt_LevelUp")
assert int(cage.Levels) == before + 1, (before, cage.Levels, up)
down = FreeCADGui.runCommand("Sculpt_LevelDown")
assert int(cage.Levels) == before, (before, cage.Levels, down)

attached = FreeCADGui.runCommand("Sculpt_AttachProxies")
assert attached["objects"] == [], attached
print("commands ok", len(expected))
"#,
    );
    assert!(output.contains("commands ok 6"), "{output}");
}
