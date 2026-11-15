//! End-to-end: a drawing tool **anchored on a face**, through the real registry.
//!
//! The chain under test is slice C4 of `docs/project/research/simplecad-loadout-mapping.md`:
//!
//! ```text
//! hover ──▶ HoverTarget { face's frame, host: Face6 } ──▶ the tool's ghost + session plane
//!   ──▶ InteractionCommit {<tool>, create, [world geometry], host }
//!   ──▶ the app script's interaction hook ──▶ draw_profile ──▶ the host's face divides
//! ```
//!
//! Two things are asserted that the separate `WP` and `face` paths could each pass alone:
//!
//! * the committed profile is **in the face's plane** (the tool drew it there, because the
//!   first press bound the hovered frame), and
//! * the commit names the host, so the script **divides** the host face instead of adding
//!   a second object — one face more, the same object count, one undo.
//!
//! It needs OCCT, like the other edition round trips.

use std::path::Path;
use std::sync::{Mutex, Once};

use ferrocad_shell_python::python;
use ferrocad_stage::{
    CreationHost, HoverTarget, ManipulatorFrame, MouseButton, PointerEvent, PointerKind, Snap,
    SnapKind, TargetKind,
};
use ferrocad_types::Vector3;
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

/// A fresh document holding one 10 mm box named `Body`, made active.
fn box_document(name: &str) {
    evaluate(&format!(
        r#"
if FreeCAD.getDocument({name:?}):
    FreeCAD.closeDocument({name:?})
doc = FreeCAD.newDocument({name:?})
FreeCAD.setActiveDocument({name:?})
import Part
body = doc.addObject("Part::Feature", "Body")
body.Shape = Part.makeBox(10, 10, 10)
doc.recompute()
assert len(body.Shape.Faces) == 6, len(body.Shape.Faces)
print("ok")
"#
    ));
}

fn faces() -> usize {
    evaluate("print(len(doc.getObject('Body').Shape.Faces))")
        .trim()
        .parse()
        .expect("a face count")
}

fn objects() -> usize {
    evaluate("print(len(doc.Objects))")
        .trim()
        .parse()
        .expect("an object count")
}

/// The placement the viewport resolves over the box's top face (`Face6`, `z = 10`).
fn top_face_target() -> HoverTarget {
    HoverTarget {
        frame: ManipulatorFrame {
            origin: Vector3::new(5.0, 5.0, 10.0),
            basis: [
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
            ],
        },
        host: Some(CreationHost::element(1, "Face6")),
        kind: TargetKind::Face,
    }
}

/// Hover, then press each point: the sequence the viewport delivers.
fn draw_on_face(
    tool: &str,
    target: &HoverTarget,
    points: &[[f32; 3]],
) -> Option<ferrocad_stage::InteractionCommit> {
    let mut session = ferrocad_simplecad::contributions()
        .tool_for(tool)
        .unwrap_or_else(|| panic!("the edition registers `{tool}`"));
    session.hover(Some(target));
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
            session.on_event(&event, snap);
        }
    }
    session.finish()
}

/// The host resolution the shell performs before the commit reaches the script.
fn resolve_host(commit: &mut ferrocad_stage::InteractionCommit, object: &str) {
    let host = commit.host.as_mut().expect("the commit names a host");
    assert!(host.object.is_none(), "the stage never fills a document name");
    host.object = Some(object.to_string());
}

#[test]
fn a_circle_anchored_on_a_face_divides_that_face_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    box_document("FaceHostedCircle");
    assert_eq!(faces(), 6);
    assert_eq!(objects(), 1);

    // The circle is drawn on the top face: centre (5, 5, 10), rim 2 mm out.
    let mut commit = draw_on_face(
        "circle",
        &top_face_target(),
        &[[5.0, 5.0, 10.0], [7.0, 5.0, 10.0]],
    )
    .expect("a circle commit");
    assert!(commit.creates, "a drawing tool creates");
    assert_eq!(
        &commit.values[6..9],
        &[0.0, 0.0, 1.0],
        "the face's own normal, not a world-plane rule"
    );
    assert_eq!(
        commit.host.as_ref().and_then(|host| host.element_name()),
        Some("Face6"),
        "the first press bound the hovered face"
    );

    resolve_host(&mut commit, "Body");
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the imprint failed: {}", applied.error);

    // The face divided: one more face, still one object, and the disc is a sub-face of
    // the box rather than a second profile sitting on it.
    assert_eq!(faces(), 7, "Face6 divided into an inner and an outer face");
    assert_eq!(objects(), 1, "an imprint adds no object");

    // One named transaction: undo puts the square back.
    let undone = evaluate(
        r#"
doc.undo()
doc.recompute()
assert len(doc.getObject('Body').Shape.Faces) == 6, len(doc.getObject('Body').Shape.Faces)
print("ok")
"#,
    );
    assert_eq!(undone.trim(), "ok", "{undone}");
}

#[test]
fn a_rectangle_anchored_on_a_face_divides_that_face() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    box_document("FaceHostedRectangle");
    assert_eq!(faces(), 6);

    let mut commit = draw_on_face(
        "rectangle",
        &top_face_target(),
        &[[2.0, 2.0, 10.0], [6.0, 5.0, 10.0]],
    )
    .expect("a rectangle commit");
    // Square to the face's axes, not the drag's: every corner is on `z = 10`.
    for corner in commit.values.chunks_exact(3) {
        assert_eq!(corner[2], 10.0, "{corner:?}");
    }
    resolve_host(&mut commit, "Body");
    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the imprint failed: {}", applied.error);

    assert_eq!(faces(), 7);
    assert_eq!(objects(), 1);
}

#[test]
fn without_a_host_the_same_commit_still_makes_a_separate_object() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    box_document("FaceHostedUnhosted");

    // A workplane hover: the tool draws in the drag's world plane and names no body.
    let plane = HoverTarget::of_workplane(&ferrocad_stage::Workplane::world());
    let commit = draw_on_face("circle", &plane, &[[20.0, 0.0, 0.0], [25.0, 0.0, 0.0]])
        .expect("a circle commit");
    assert!(commit.host.is_none(), "the workplane is not a body");

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(applied.ok, "the create failed: {}", applied.error);
    assert_eq!(faces(), 6, "the box is untouched");
    assert_eq!(objects(), 2, "a new Part::Feature appeared");
}

#[test]
fn a_host_naming_a_missing_face_is_refused_and_writes_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    box_document("FaceHostedRefusal");

    let mut commit = draw_on_face(
        "circle",
        &top_face_target(),
        &[[5.0, 5.0, 10.0], [7.0, 5.0, 10.0]],
    )
    .expect("a circle commit");
    resolve_host(&mut commit, "Body");
    // The body went away between the press and the commit: the element cannot resolve.
    commit.host.as_mut().expect("a host").element = Some("Face99".to_string());

    let applied = python::apply_interaction(&commit, "").expect("the hook ran");
    assert!(!applied.ok, "a bad element is a refusal");
    assert!(!applied.error.is_empty(), "the refusal says why");
    assert_eq!(faces(), 6, "the document is untouched");
    assert_eq!(objects(), 1);
}
