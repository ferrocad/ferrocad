//! End-to-end S4.5: a face selection's push/pull stem drives the edition's document.
//!
//! The selection names one planar face, the edition's factory
//! (`ferrocad_part_gui::push_pull_stem_for`) returns the stem, a drag produces a neutral
//! `InteractionCommit`, and the app script's `push_pull` hook applies it. This is the
//! same route SimpleCAD's push/pull takes: no separate command and no second geometry
//! code path. It needs OCCT.

use std::sync::{Mutex, Once};

use ferrocad_geom::SelectionRef;
use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_part_gui::push_pull_stem_for;
use ferrocad_shell_python::DisplaySync;
use ferrocad_shell_python::python;
use ferrocad_stage::Manipulator;
use ferrocad_types::{Placement, Rotation, Vector3};

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

fn camera() -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -100.0), Rotation::identity()).to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 1000.0,
        },
        viewport: Viewport::new([400, 400], 1.0),
    }
}

/// Run a line on the console surface (the app script's namespace).
fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

#[test]
fn the_push_pull_stem_drives_the_document_in_one_transaction() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let mut display = DisplaySync::new();
    let stage = display.build(meshes.clone());

    // A displayed solid with a planar `Face1`: the sample's box. A curved face has no
    // stem, so the factory returning `Some` is itself the planarity check.
    let (object, entity, mut stem) = meshes
        .iter()
        .filter(|mesh| mesh.parts.iter().any(|part| part == "Face1"))
        .find_map(|mesh| {
            push_pull_stem_for(&SelectionRef::element(mesh.entity, "Face1"), &stage)
                .map(|stem| (mesh.object.clone(), mesh.entity, stem))
        })
        .expect("the sample has a planar face to push/pull");
    assert_eq!(stem.target_id(), entity);
    assert_eq!(stem.face_name(), "Face1");

    // Grab the stem's projected tip and drag further out along its own normal.
    let camera = camera();
    let anchor = Manipulator::anchor_device(&stem, &camera).expect("the face projects");
    let tip = Manipulator::readout_anchor(&stem, &camera).expect("the tip projects");
    let (dx, dy) = (tip[0] - anchor[0], tip[1] - anchor[1]);
    let length = dx.hypot(dy);
    assert!(length > 1.0, "the stem is long enough to grab: {length}");
    let grab = [tip[0].round() as i32, tip[1].round() as i32];
    let target = [
        grab[0] + (dx / length * 20.0).round() as i32,
        grab[1] + (dy / length * 20.0).round() as i32,
    ];
    assert!(
        Manipulator::drag_begin(&mut stem, &camera, grab),
        "the tip is grabbable"
    );
    assert!(
        Manipulator::drag_update(&mut stem, &camera, target),
        "the drag moved the stem"
    );
    let commit = Manipulator::drag_end(&mut stem).expect("a commit");

    assert_eq!(commit.manipulator, "push_pull");
    assert_eq!(commit.action, "extrude");
    assert_eq!(commit.element.as_deref(), Some("Face1"));
    assert!(
        commit.values[0] > 0.0,
        "dragging along the normal is a positive extrusion: {}",
        commit.values[0]
    );

    // The shell's route: commit → entity map → the app script's `interaction` hook.
    let result = python::apply_interaction(&commit, &object).expect("the hook ran");
    assert!(result.ok, "push/pull failed: {}", result.error);

    let after = python::stage_meshes().expect("stage meshes");
    let names: Vec<&str> = after.iter().map(|mesh| mesh.object.as_str()).collect();
    let feature = format!("{object}_Fuse");
    assert!(
        names.contains(&feature.as_str()),
        "the fused result is displayed: {names:?}"
    );

    // One undo step: the edition wraps the extrusion and the boolean in a single
    // transaction, so the base returns and the feature disappears together.
    evaluate("doc.undo()\ndoc.recompute()");
    let restored = python::stage_meshes().expect("stage meshes");
    let names: Vec<&str> = restored.iter().map(|mesh| mesh.object.as_str()).collect();
    assert!(
        !names.contains(&feature.as_str()),
        "one undo removes the boolean: {names:?}"
    );
    assert!(
        names.contains(&object.as_str()),
        "the base is back: {names:?}"
    );
}
