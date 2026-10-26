//! End-to-end: the **Part edition's** dress-up arc, through the generic registry.
//!
//! This is the real edition: `Part` and `ferrocad` share one image (one core), the app
//! script registers the display and interaction providers, and the manipulator is the
//! one `main` contributes. It needs OCCT.
//!
//! The tests follow the shell's path: selection → the edition's factory → a drag →
//! neutral `InteractionCommit` → entity-to-object map → the app script's `interaction`
//! hook → a recomputed document → re-sync.
//!
//! The first test covers the Part edge dress-up arc (a `Part::Fillet`); the second
//! covers the shared translate gizmo, so the edition's composition of
//! `ferrocad::base_contributions()` with the Part manipulator keeps its move handle.

use std::sync::{Mutex, Once};

use ferrocad_geom::SelectionRef;
use ferrocad_gpui::DisplaySync;
use ferrocad_gpui::python;
use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_part_gui::{DressUpMode, dress_up_scrubber_for};
use ferrocad_stage::screen::project_device;
use ferrocad_stage::{GizmoHandle, Manipulator, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register both built-in modules before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        // The edition registers the combined image's four modules; the test boots
        // the same image so Part and the shell share one core.
        ferrocad_image_py::register_builtins();
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

/// An item's minimum x — the world-space corner the gizmo is armed at.
fn min_x(stage: &Stage, entity: u32) -> f64 {
    stage
        .item(entity)
        .expect("the item")
        .positions()
        .iter()
        .map(|position| f64::from(position[0]))
        .fold(f64::INFINITY, f64::min)
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

/// The Part edition starts from `ferrocad::base_contributions()`, not
/// `Contributions::default()`, so composing its fillet manipulator must not drop the
/// shared move gizmo. This is the regression that motivated the fix: the triad was
/// simply absent, and a test that only checked `result.ok` after a hand-built commit
/// would never have noticed.
#[test]
fn the_part_move_gizmo_translates_the_box_and_undoes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();

    let meshes = python::stage_meshes().expect("stage meshes");
    let box_mesh = meshes
        .iter()
        .find(|mesh| mesh.object == "Box")
        .expect("the seeded Box is displayed");
    let entity = box_mesh.entity;

    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    assert_eq!(display.object_of(entity), Some("Box"));

    // The edition's composition (`main` does this): base contributions plus the
    // Part dress-up factory. A whole-object selection must still arm the gizmo.
    let contributions =
        ferrocad::base_contributions().with_manipulator(ferrocad_part_gui::fillet_manipulator);
    let mut gizmo = contributions
        .gizmo_for(&SelectionRef::whole(entity), &stage)
        .expect("the base composition arms the move gizmo for a whole body");
    assert_eq!(gizmo.target_id(), entity);

    // A click on the Part box resolves to a *named* element, because the OCCT
    // tessellation tags `Face1`/`Edge1`/…; the toy's mesh is one untagged part. That
    // selection must arm the triad too, or the edition shows no gizmo at all — the
    // regression this test exists for.
    let face = contributions
        .gizmo_for(&SelectionRef::element(entity, "Face1"), &stage)
        .expect("a face click arms the triad for its body");
    assert_eq!(face.target_id(), entity);

    let camera = camera();
    gizmo.update(&camera);

    let x_before = min_x(&stage, entity);
    assert!(
        x_before.abs() < 1e-6,
        "the seeded 20 mm box sits at x = 0: {x_before}"
    );

    // Grab the middle of the X stem and drag it 30 px along +X. The camera looks
    // along +Z, so X projects across the viewport and the stem is grabbable.
    let stem = gizmo
        .origin()
        .add(&Vector3::new(gizmo.scale() * 0.5, 0.0, 0.0));
    let device = project_device(&camera, stem).expect("the X stem projects");
    let grab = [device[0].round() as i32, device[1].round() as i32];
    assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera, grab));
    assert!(
        gizmo.drag_to(&camera, [grab[0] + 30, grab[1]]).is_some(),
        "the drag moved the gizmo"
    );
    let commit = gizmo.end_drag().expect("a gizmo commit").to_interaction();

    assert_eq!(commit.manipulator, "transform");
    assert_eq!(commit.action, "apply");
    assert_eq!(commit.values.len(), 16, "a row-major 4x4 transform");
    // Row-major (FreeCAD A11..A44): A14/A24/A34 are the translation.
    let delta_x = commit.values[3];
    assert!(
        delta_x > 1.0,
        "the drag asks for a visible +X move: {delta_x}"
    );

    // The shell's route: commit → entity map → the app script's `interaction` hook.
    let object = display.object_of(commit.target).expect("the object");
    let result = python::apply_interaction(&commit, object).expect("the hook ran");
    assert!(result.ok, "move failed: {}", result.error);

    // The re-synced box is at the dragged placement, by the transform's own delta.
    let meshes = python::stage_meshes().expect("stage meshes");
    let stage = display.build(meshes);
    let x_after = min_x(&stage, entity);
    assert!(
        (x_after - (x_before + delta_x)).abs() < 0.05,
        "the box moved by the commit's {delta_x:.3}: {x_before} → {x_after}"
    );
    assert!(
        x_after > x_before + 1.0,
        "the displayed geometry actually moved: {x_before} → {x_after}"
    );

    // The app hook wraps the move in one named transaction, so one undo puts the
    // body back where it was.
    evaluate("doc.undo()");
    let meshes = python::stage_meshes().expect("stage meshes");
    let stage = display.build(meshes);
    let x_restored = min_x(&stage, entity);
    assert!(
        (x_restored - x_before).abs() < 1e-6,
        "undo restores the placement: {x_restored} vs {x_before}"
    );
}
