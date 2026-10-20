//! End-to-end: the **Part edition's** dress-up arc, through the generic registry.
//!
//! This is the real edition: `Part` and `ferrocad` share one image (one core), the app
//! script registers the display and interaction providers, and the manipulator is the
//! one `main` contributes. It needs OCCT.
//!
//! The test follows the shell's path: selection → the edition's factory → a drag →
//! neutral `InteractionCommit` → entity-to-object map → the app script's `interaction`
//! hook → a recomputed `Part::Fillet` → re-sync.

use std::sync::{Mutex, Once};

use ferrocad_geom::SelectionRef;
use ferrocad_gpui::DisplaySync;
use ferrocad_gpui::python;
use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_part_gui::{DressUpMode, dress_up_scrubber_for};
use ferrocad_stage::Manipulator;
use ferrocad_types::{Placement, Rotation, Vector3};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register both built-in modules before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        use ferrocad_part_py::Part as part_module;
        pyo3::append_to_inittab!(part_module);
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

#[test]
fn the_part_fillet_manipulator_drives_the_document() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let box_mesh = meshes
        .iter()
        .find(|mesh| mesh.object == "Box")
        .expect("the seeded Box is displayed");
    let entity = box_mesh.entity;
    assert!(
        box_mesh.parts.iter().any(|part| part == "Edge1"),
        "the kernel names the box's edges"
    );

    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    assert_eq!(display.object_of(entity), Some("Box"));

    // The edition's factory, on an edge selection (the rule Part uses).
    let mut scrubber = dress_up_scrubber_for(
        &SelectionRef::element(entity, "Edge1"),
        &stage,
        DressUpMode::Fillet,
    )
    .expect("a scrubber over Edge1");

    // Grab the radius tick and drag outwards.
    let camera = camera();
    let anchor = scrubber.screen_anchor(&camera).expect("the edge projects");
    let apex = Manipulator::readout_anchor(&scrubber, &camera).expect("the radius tick projects");
    let (dx, dy) = (apex[0] - anchor[0], apex[1] - anchor[1]);
    let length = dx.hypot(dy).max(1e-9);
    let grab = [anchor[0].round() as i32, anchor[1].round() as i32];
    let target = [
        grab[0] + (dx / length * 25.0).round() as i32,
        grab[1] + (dy / length * 25.0).round() as i32,
    ];
    assert!(scrubber.begin_drag(&camera, grab), "the tick is grabbable");
    assert!(
        scrubber.drag_to(&camera, target).is_some(),
        "the scrub moved the radius"
    );
    let commit = Manipulator::drag_end(&mut scrubber).expect("a commit");

    assert_eq!(commit.manipulator, "dress_up");
    assert_eq!(commit.action, "fillet");
    assert_eq!(commit.element.as_deref(), Some("Edge1"));
    assert!(commit.values[0] > 0.0, "a positive radius");

    // The shell's route: commit → entity map → the app script's `interaction` hook.
    let object = display.object_of(commit.target).expect("the object");
    let result = python::apply_interaction(&commit, object).expect("the hook ran");
    assert!(result.ok, "interaction failed: {}", result.error);

    // The feature replaced its base, and it has the fillet's extra faces.
    let meshes = python::stage_meshes().expect("stage meshes");
    let objects: Vec<&str> = meshes.iter().map(|mesh| mesh.object.as_str()).collect();
    assert!(
        objects.contains(&"Box_Fillet"),
        "the fillet is displayed: {objects:?}"
    );
    assert!(
        !objects.contains(&"Box"),
        "the base is replaced by the feature: {objects:?}"
    );
    let feature = meshes
        .iter()
        .find(|mesh| mesh.object == "Box_Fillet")
        .expect("the fillet mesh");
    let faces = feature
        .parts
        .iter()
        .filter(|part| part.starts_with("Face"))
        .count();
    assert!(faces > 6, "the fillet adds faces: {faces}");

    let stage = display.build(python::stage_meshes().expect("stage meshes"));
    assert!(
        stage.items().iter().all(|item| item.entity_id() != entity),
        "the old box entity is gone"
    );
}
