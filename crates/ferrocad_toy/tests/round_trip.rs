//! End-to-end: the **toy workbench**, through the generic extension points.
//!
//! No kernel is linked. The four things a workbench does are exercised on a real
//! document:
//!
//! 1. create an object (`Toy.makeSlab`, one transaction);
//! 2. represent it (provider colour + display mode);
//! 3. manipulate it (the `HeightGrip` manipulator and the `TransformGizmo`);
//! 4. record it (undo/redo, and save → close → open).
//!
//! Every step follows the shell's path: selection → the edition's factory → a drag →
//! a neutral `InteractionCommit` → entity-to-object map → the app script's
//! `interaction` hook → a recomputed document → re-synced stage.

use std::sync::{Mutex, Once};

use ferrocad_geom::SelectionRef;
use ferrocad_gpui::python;
use ferrocad_gpui::{Contributions, DisplaySync, StageMesh};
use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_stage::screen::project_device;
use ferrocad_stage::{GizmoHandle, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};

/// The tests share one interpreter and one document registry.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the built-in `ferrocad` module and the app script before the interpreter
/// starts (`auto-initialize` starts it on the first GIL acquisition).
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        python::configure("ferrocad_toy", &[], &[], false);
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

fn meshes() -> Vec<StageMesh> {
    python::stage_meshes().expect("stage meshes")
}

fn find<'a>(meshes: &'a [StageMesh], object: &str) -> &'a StageMesh {
    meshes
        .iter()
        .find(|mesh| mesh.object == object)
        .unwrap_or_else(|| panic!("{object} is displayed"))
}

fn slab_count() -> usize {
    meshes()
        .iter()
        .filter(|mesh| mesh.object.starts_with("Slab"))
        .count()
}

/// An item's maximum z (its height above its base).
fn top_z(stage: &Stage, entity: u32) -> f64 {
    stage
        .item(entity)
        .expect("the item")
        .positions()
        .iter()
        .map(|position| f64::from(position[2]))
        .fold(f64::NEG_INFINITY, f64::max)
}

fn evaluate(code: &str) -> String {
    let result = python::evaluate(code).expect("the interpreter ran");
    assert!(result.ok, "{code} failed:\n{}", result.output);
    result.output
}

/// An item's minimum x.
fn min_x(stage: &Stage, entity: u32) -> f64 {
    stage
        .item(entity)
        .expect("the item")
        .positions()
        .iter()
        .map(|position| f64::from(position[0]))
        .fold(f64::INFINITY, f64::min)
}

/// A number the console printed (the facade prints `repr(value)`).
fn number(output: &str) -> f64 {
    output
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("expected a number, got {output:?}"))
}

#[test]
fn the_toy_workbench_creates_represents_manipulates_and_records() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    let camera = camera();

    // --- 1. Representation -------------------------------------------------------
    let displayed = meshes();
    let slab = find(&displayed, "Slab");
    let entity = slab.entity;
    assert_eq!(
        slab.color,
        Some([0.26, 0.55, 0.85, 1.0]),
        "the provider supplies the object's colour"
    );
    assert_eq!(slab.display.as_deref(), Some("shaded_edges"));

    let mut display = DisplaySync::new();
    let stage = display.build(displayed);
    assert_eq!(display.object_of(entity), Some("Slab"));
    let item = stage.item(entity).expect("the item");
    assert_eq!(item.surface_color(), [0.26, 0.55, 0.85, 1.0]);
    let height_before = top_z(&stage, entity);
    assert!((height_before - 20.0).abs() < 1e-6);

    // --- 2. Creation -------------------------------------------------------------
    // The console surface the toy registers with `add_namespace`.
    evaluate("Toy.makeSlab(30, 30, 10, 60)");
    let displayed = meshes();
    assert_eq!(slab_count(), 2, "a second slab was added");
    let created = displayed
        .iter()
        .find(|mesh| mesh.object != "Slab")
        .expect("the new slab");
    let created_min_x = created
        .positions
        .iter()
        .map(|position| f64::from(position[0]))
        .fold(f64::INFINITY, f64::min);
    assert!(
        created_min_x > 40.0,
        "the new slab sits at x = 60: min x {created_min_x}"
    );
    assert!(
        find(&displayed, "Slab").positions != created.positions,
        "the two slabs have their own geometry"
    );

    // Creation is one undoable transaction.
    evaluate("doc.undo()");
    assert_eq!(slab_count(), 1, "undo removes the new slab");
    evaluate("doc.redo()");
    assert_eq!(slab_count(), 2, "redo brings it back");
    evaluate("doc.undo()");

    // --- 3a. Manipulation: the height grip --------------------------------------
    let displayed = meshes();
    let entity = find(&displayed, "Slab").entity;
    let stage = display.build(displayed);
    let contributions = Contributions::new()
        .with_manipulator(ferrocad_toy::height_grip)
        .with_gizmo(ferrocad_toy::move_gizmo);

    let mut grip = contributions
        .manipulator_for(&SelectionRef::whole(entity), &stage)
        .expect("the toy factory arms a height grip");
    let anchor = grip.readout_anchor(&camera).expect("the grip projects");
    let grab = [anchor[0].round() as i32, anchor[1].round() as i32];
    assert!(grip.drag_begin(&camera, grab));
    assert!(grip.drag_update(&camera, [grab[0], grab[1] + 25]));
    let commit = grip.drag_end().expect("a commit");
    assert_eq!(commit.manipulator, "toy_height");

    let object = display.object_of(commit.target).expect("the object");
    let result = python::apply_interaction(&commit, object).expect("the hook ran");
    assert!(result.ok, "interaction failed: {}", result.error);

    let displayed = meshes();
    let stage = display.build(displayed);
    let height_after = top_z(&stage, entity);
    assert!(
        (height_after - commit.values[0]).abs() < 0.05,
        "expected {:.2}, measured {height_after:.2}",
        commit.values[0]
    );
    assert!(height_after > height_before + 1.0);

    // The edit is one undoable transaction.
    evaluate("doc.undo()");
    let displayed = meshes();
    let stage = display.build(displayed);
    assert!(
        (top_z(&stage, entity) - height_before).abs() < 1e-6,
        "undo restores the height"
    );

    // --- 3b. Manipulation: the gizmo --------------------------------------------
    let displayed = meshes();
    let entity = find(&displayed, "Slab").entity;
    let stage = display.build(displayed);
    let x_before = min_x(&stage, entity);
    let mut gizmo = contributions
        .gizmo_for(&SelectionRef::whole(entity), &stage)
        .expect("the toy factory arms a translate gizmo");
    assert_eq!(gizmo.target_id(), entity);
    gizmo.update(&camera);

    // Grab the middle of the X stem and drag it 30 px along +X.
    let stem = gizmo
        .origin()
        .add(&Vector3::new(gizmo.scale() * 0.5, 0.0, 0.0));
    let device = project_device(&camera, stem).expect("the stem projects");
    let grab = [device[0].round() as i32, device[1].round() as i32];
    assert!(gizmo.begin_drag(GizmoHandle::AxisX, &camera, grab));
    assert!(
        gizmo.drag_to(&camera, [grab[0] + 30, grab[1]]).is_some(),
        "the drag moved the gizmo"
    );
    let transform = gizmo.end_drag().expect("a gizmo commit");
    let commit = transform.to_interaction();
    assert_eq!(commit.manipulator, "transform");
    assert_eq!(commit.action, "apply");

    let result = python::apply_interaction(&commit, "Slab").expect("the hook ran");
    assert!(result.ok, "transform failed: {}", result.error);
    let x = number(&evaluate("doc.getObject('Slab').Placement.Base.x"));
    assert!(x > 0.0, "the slab moved along +X: {x}");

    // The re-synced mesh is at the new placement, and undo puts it back.
    let displayed = meshes();
    let stage = display.build(displayed);
    let x_after = min_x(&stage, entity);
    assert!(
        x_after > x_before + 5.0,
        "the displayed mesh moved: {x_before} → {x_after}"
    );
    evaluate("doc.undo()");
    let x = number(&evaluate("doc.getObject('Slab').Placement.Base.x"));
    assert!(x.abs() < 1e-9, "undo restores the placement: {x}");

    // --- 4. Persistence ----------------------------------------------------------
    let path = std::env::temp_dir().join("ferrocad_toy_round_trip.FCStd");
    let _ = std::fs::remove_file(&path);
    let path = path.to_string_lossy().to_string();
    evaluate(&format!("doc.saveAs({path:?})"));
    evaluate("FreeCAD.closeDocument('Toy')");
    evaluate(&format!("FreeCAD.openDocument({path:?})"));

    let displayed = meshes();
    let saved = find(&displayed, "Slab");
    let saved_entity = saved.entity;
    assert_eq!(
        saved.color,
        Some([0.26, 0.55, 0.85, 1.0]),
        "the colour persisted"
    );
    assert_eq!(saved.display.as_deref(), Some("shaded_edges"));
    let stage = DisplaySync::new().build(displayed);
    assert!(
        (top_z(&stage, saved_entity) - height_before).abs() < 1e-6,
        "the height persisted"
    );
}
