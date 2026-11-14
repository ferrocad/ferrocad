//! The questions the shell cannot answer: the range the Part kernel completes, and the
//! value an operation the user re-opened already holds.
//!
//! The host proves a floor from the stage and nothing more — half a body's extent is *not*
//! a ceiling, because a single edge of a cube takes a fillet larger than that. The
//! edition's probe bisects a real `Shape.makeFillet` instead, so the row shows the radius
//! that stops working. It also reads the kernel's lineage: a cap face names the face it
//! grew from and a fillet's round names the edge, which is how a second gesture edits the
//! feature instead of stacking another one. These drive the probe through the facade
//! exactly as the shell does. They need OCCT.

use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register both built-in modules before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], false);
    });
    python::bootstrap().expect("boot the edition");
}

/// Run a script on the console surface (the app script's namespace).
fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

#[test]
fn the_edition_bisects_the_radius_the_kernel_completes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let output = evaluate(
        r#"
import Part, FerroCAD

class Holder:
    pass

# A 10 mm cube: the ceiling is the radius where makeFillet stops succeeding, which is a
# property of the edge and the faces, not of the body's half-extent.
box = Part.makeBox(10, 10, 10)
holder = Holder()
holder.Shape = box

bounds = FerroCAD.manipulator_probe(holder, 'Edge1', 'dress_up')
assert bounds is not None, 'the edition answers for an edge'
top = bounds['max']
assert bounds['min'] > 0, bounds
assert top is not None and 0 < top < 10, bounds

edge = box.getElement('Edge1')
box.makeFillet(top, [edge])          # the reported ceiling itself must succeed
try:
    box.makeFillet(top * 1.25, [edge])
except Exception:
    pass
else:
    raise AssertionError('the ceiling is not a boundary: %r' % (bounds,))
print(repr(bounds))
"#,
    );
    assert!(output.contains("'max'"), "{output}");
}

#[test]
fn an_applied_feature_is_reopened_not_stacked() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    // The round face is the one the kernel generated from the filleted edge; the probe
    // seeds the radius and the commit edits the feature instead of building a second one.
    let output = evaluate(
        r#"
import FreeCAD, FerroCAD
import ferrocad_parametric as part

doc = FreeCAD.ActiveDocument
fillet = doc.getObject('Fillet')
fillet.touch()
doc.recompute()

round = None
for name in list(fillet.Shape.mesh()['parts']):
    if not name.startswith('Face'):
        continue
    history = fillet.Shape.getElementHistory(name)
    if history and 'Edge1' in (history[2] or ()):
        round = name
assert round is not None, 'the round face carries the edge it came from'

probe = FerroCAD.manipulator_probe(fillet, round, 'dress_up')
assert probe['values'] == [fillet.Radius], probe
assert probe['max'] is not None and probe['max'] > fillet.Radius, probe

specs = FerroCAD.manipulator_specs(fillet, round)
assert specs, 'a declared handle re-opens a face selection'
assert specs[0]['id'] == 'dress_up' and specs[0]['kind'] == 'axis'
assert specs[0]['value'] == fillet.Radius, specs[0]
assert specs[0]['shape']['positions'], 'the grip has geometry to draw and hit'

before = len(doc.Objects)
assert part._interaction(fillet, 'dress_up', 'fillet', round, [4.0])['ok']
assert fillet.Radius == 4.0, fillet.Radius
assert len(doc.Objects) == before, 'the fillet was edited, not stacked'

box = part.makeBox(10, 10, 10, name='Reopen')
assert part._push_pull(box, 'Face1', 5.0)['ok']
fuse = doc.getObject('Reopen_Fuse')
caps = [
    name
    for name in list(fuse.Shape.mesh()['parts'])
    if part._is_extrusion_cap(fuse, name, box, 'Face1', 5.0)
]
assert len(caps) == 1, caps
cap = caps[0]
assert FerroCAD.manipulator_probe(fuse, cap, 'push_pull')['values'] == [5.0]

count = len(doc.Objects)
assert part._interaction(fuse, 'push_pull', 'extrude', cap, [7.0])['ok']
assert doc.getObject('Reopen_Extrusion').LengthFwd == 7.0
assert len(doc.Objects) == count, 'the extrusion was edited, not stacked'
assert FerroCAD.manipulator_probe(fuse, cap, 'push_pull')['values'] == [7.0]

# Crossing zero changes the operation: the same extrusion is rebuilt as a cut and the
# old boolean is retired, so the body still has one push/pull feature.
assert part._interaction(fuse, 'push_pull', 'pocket', cap, [-2.0])['ok']
tool = doc.getObject('Reopen_Extrusion')
assert tool.Reversed and tool.LengthFwd == 2.0, (tool.Reversed, tool.LengthFwd)
assert doc.getObject('Reopen_Fuse') is None, 'the fused boolean was retired'
assert doc.getObject('Reopen_Cut') is not None
print('reopened')
"#,
    );
    assert!(output.contains("reopened"), "{output}");
}

#[test]
fn a_push_pull_question_gets_no_kernel_bound() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    // A distance is unbounded (an extrusion can go as far as the user likes, and a cut
    // deeper than the body is a solid), so the edition answers "no opinion" rather than
    // inventing a range.
    let output = evaluate(
        r#"
import Part, FerroCAD

class Holder:
    pass

holder = Holder()
holder.Shape = Part.makeBox(10, 10, 10)
assert FerroCAD.manipulator_probe(holder, 'Face1', 'push_pull') is None
print('no bound')
"#,
    );
    assert!(output.contains("no bound"), "{output}");
}
