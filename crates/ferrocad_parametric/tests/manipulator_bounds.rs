//! The one question the shell cannot answer: the range the Part kernel completes.
//!
//! The host proves a floor from the stage and nothing more — half a body's extent is *not*
//! a ceiling, because a single edge of a cube takes a fillet larger than that. The
//! edition's bounds provider bisects a real `Shape.makeFillet` instead, so the row shows
//! the radius that stops working. This drives the provider through the facade exactly as
//! the shell does. It needs OCCT.

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

bounds = FerroCAD.manipulator_bounds(holder, 'Edge1', 'dress_up')
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
assert FerroCAD.manipulator_bounds(holder, 'Face1', 'push_pull') is None
print('no bound')
"#,
    );
    assert!(output.contains("no bound"), "{output}");
}
