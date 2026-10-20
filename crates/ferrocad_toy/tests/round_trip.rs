//! End-to-end: a **toy** workbench's manipulator, through the generic registry.
//!
//! No kernel is linked. The document object is an `App::FeaturePython` with a
//! `Height` length property; the app script generates its display mesh and implements
//! the `toy_height` interaction; the manipulator is `ferrocad_toy::height_grip`,
//! contributed exactly as an edition contributes one.
//!
//! The test follows the shell's path: selection → factory → drag → neutral commit →
//! entity-to-object map → the app script's `interaction` hook → re-sync.

use std::sync::{Mutex, Once};

use ferrocad_geom::SelectionRef;
use ferrocad_gpui::python;
use ferrocad_gpui::{Contributions, DisplaySync};
use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_stage::Stage;
use ferrocad_types::{Placement, Rotation, Vector3};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the built-in `ferrocad` module and the app script before the interpreter
/// starts (`auto-initialize` starts it on the first GIL acquisition).
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        python::configure("ferrocad_toy", &[], &[]);
    });
    python::bootstrap().expect("boot the toy app");
}

fn camera() -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -200.0), Rotation::identity()).to_matrix(),
        projection: Projection::Perspective {
            fov_y: 45.0f64.to_radians(),
            near: 0.1,
            far: 1000.0,
        },
        viewport: Viewport::new([400, 400], 1.0),
    }
}

/// The item's maximum z (the slab's height above its base).
fn top_z(stage: &Stage, entity: u32) -> f64 {
    stage
        .item(entity)
        .expect("the item")
        .positions()
        .iter()
        .map(|position| f64::from(position[2]))
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn a_toy_manipulator_drives_a_document_property() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let slab = meshes
        .iter()
        .find(|mesh| mesh.object == "Slab")
        .expect("the Slab is displayed");
    let entity = slab.entity;
    assert!(
        slab.parts.is_empty(),
        "the toy mesh is untagged, so a click selects the whole object"
    );

    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    assert_eq!(display.object_of(entity), Some("Slab"));
    let before = top_z(&stage, entity);
    assert!((before - 20.0).abs() < 1e-6, "the slab starts 20 mm tall");

    // What the shell does on a click: ask the edition's factories.
    let contributions = Contributions::new().with_manipulator(ferrocad_toy::height_grip);
    let mut grip = contributions
        .manipulator_for(&SelectionRef::whole(entity), &stage)
        .expect("the toy factory arms a height grip");
    assert_eq!(grip.id(), "toy_height");

    // Drag the grip up 25 device pixels.
    let camera = camera();
    let anchor = grip.readout_anchor(&camera).expect("the grip projects");
    let grab = [anchor[0].round() as i32, anchor[1].round() as i32];
    assert!(grip.drag_begin(&camera, grab));
    assert!(grip.drag_update(&camera, [grab[0], grab[1] + 25]));
    let commit = grip.drag_end().expect("a commit");

    assert_eq!(commit.manipulator, "toy_height");
    assert_eq!(commit.target, entity);
    assert_eq!(commit.action, "set");
    assert!(commit.values[0] > before, "the drag grew the height");

    // The shell's route: commit → entity map → the app script's `interaction` hook.
    let object = display.object_of(commit.target).expect("the object");
    let result = python::apply_interaction(&commit, object).expect("the hook ran");
    assert!(result.ok, "interaction failed: {}", result.error);

    // The document changed, and the provider regenerated the mesh from `Height`.
    let meshes = python::stage_meshes().expect("stage meshes");
    let stage = display.build(meshes);
    let after = top_z(&stage, entity);
    assert!(
        (after - commit.values[0]).abs() < 0.05,
        "expected {:.2}, measured {after:.2}",
        commit.values[0]
    );
    assert!(after > before + 1.0, "the slab is visibly taller");
}
