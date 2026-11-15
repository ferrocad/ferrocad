//! The Part half over the kernel-free document backend: the sample builds, the
//! manipulator commits edit it, and the registry document is the one that changes.
//!
//! These lived in the document backend until the kernel was split out of it; they
//! stay here because every one of them reads `Part` geometry or a Part feature type.

use std::sync::{Arc, Mutex, MutexGuard};

use ferrocad_core::Document;
use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_doc::DocumentBackend;
use ferrocad_shell_rust::{PartEngine, TOLERANCE};
use ferrocad_stage::InteractionCommit;

/// The Part workbench's edge dress-up and push/pull manipulator ids.
const DRESS_UP: &str = "dress_up";
const PUSH_PULL: &str = "push_pull";

/// The registry is process-wide, so the shared tests take turns and start empty.
static REGISTRY: Mutex<()> = Mutex::new(());

fn registry() -> MutexGuard<'static, ferrocad_core::Application> {
    ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn clear_registry() -> MutexGuard<'static, ()> {
    let guard = REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
    let mut application = registry();
    for name in application.names() {
        application.close_document(&name);
    }
    drop(application);
    guard
}

fn shared() -> DocumentBackend {
    let mut backend = PartEngine::shared_backend();
    backend.bootstrap().expect("boot the sample");
    backend
}

fn handle() -> Arc<Mutex<Document>> {
    registry().get("Sample").expect("the sample is registered")
}

/// Whether `name` holds a shape, in the document the registry serves.
fn has_shape(document: &Document, name: &str) -> bool {
    document
        .get_by_name(name)
        .and_then(|id| document.object(id))
        .and_then(|object| object.properties.get("Shape"))
        .and_then(ferrocad_part::shape_of)
        .and_then(|property| property.shape())
        .is_some()
}

#[test]
fn the_sample_carries_the_part_body_and_its_fillet() {
    let _serial = clear_registry();
    let backend = shared();
    let tree = backend.model_tree().unwrap();
    let sample = tree
        .iter()
        .find(|node| node.name == "Sample")
        .expect("the sample document");
    assert!(
        sample.objects.iter().any(|object| object.name == "Box"),
        "the sample's body: {:?}",
        sample.objects
    );
    let document = handle();
    let document = document.lock().unwrap();
    assert!(has_shape(&document, "Box"), "the box built");
    let box_id = document.get_by_name("Box").unwrap();
    assert_eq!(
        document.object(box_id).unwrap().type_id,
        ferrocad_part::BOX_TYPE
    );
}

#[test]
fn the_sample_tessellates_through_the_engine() {
    let _serial = clear_registry();
    let backend = shared();
    let meshes = backend.stage_meshes().unwrap();
    assert!(!meshes.is_empty(), "the sample draws something");
    // The sample's fillet consumes the box, so the body that is drawn is the fillet.
    assert!(
        meshes.iter().any(|mesh| mesh.object == "Fillet"),
        "the body is in the stage: {:?}",
        meshes.iter().map(|mesh| &mesh.object).collect::<Vec<_>>()
    );
    assert!(TOLERANCE > 0.0);
}

#[test]
fn an_edit_and_its_undo_run_against_the_registry_document() {
    let _serial = clear_registry();
    let mut backend = shared();
    let handle = handle();
    let length = |document: &Document| {
        ferrocad_shell_doc::property_rows(
            document
                .object(document.get_by_name("Box").unwrap())
                .unwrap(),
        )
        .into_iter()
        .find(|row| row.name == "Length")
        .map(|row| row.value)
    };
    let original = length(&handle.lock().unwrap()).unwrap();

    assert!(backend.set_property("Box", "Length", "7").unwrap().ok);
    assert_ne!(length(&handle.lock().unwrap()).unwrap(), original);

    assert!(backend
        .run_command(ferrocad_shell_api::catalog::UNDO)
        .unwrap()
        .ok);
    // Read through the registry's own handle: the undo reached the document the
    // Python wrappers hold, not a copy of it.
    assert_eq!(length(&handle.lock().unwrap()).unwrap(), original);
}

#[test]
fn a_dress_up_commit_builds_the_feature_with_no_interpreter() {
    let _serial = clear_registry();
    let mut backend = shared();

    let commit = InteractionCommit::new(DRESS_UP, 1, "fillet")
        .with_element("Edge1")
        .with_values([2.0]);
    let result = backend.apply_interaction(&commit, "Box").unwrap();
    assert!(result.ok, "{}", result.error);

    let handle = handle();
    {
        let document = handle.lock().unwrap();
        let id = document.get_by_name("Box_Fillet").expect("the feature");
        assert_eq!(
            document.object(id).unwrap().type_id,
            ferrocad_part::FILLET_TYPE
        );
        assert!(has_shape(&document, "Box_Fillet"), "the fillet built");
        // The scrub's radius is the row the pane would show.
        match document.object(id).unwrap().properties.get("Radius") {
            Some(ferrocad_core::Property::Quantity(value)) => {
                assert!((value.value_mm() - 2.0).abs() < 1e-9)
            }
            other => panic!("a length radius, got {other:?}"),
        }
    }

    // The same commit again reuses the feature: a drag must not leave one per frame.
    // The guard is scoped: `apply_interaction` takes the same document lock.
    let commit = commit.with_values([3.0]);
    assert!(backend.apply_interaction(&commit, "Box").unwrap().ok);
    let document = handle.lock().unwrap();
    let fillets = document
        .object_ids()
        .iter()
        .filter(|id| {
            document
                .object(**id)
                .is_some_and(|object| object.type_id == ferrocad_part::FILLET_TYPE)
        })
        .count();
    // The sample's own `Fillet` plus `Box_Fillet`.
    assert_eq!(fillets, 2);
}

#[test]
fn a_chamfer_commit_writes_both_distances() {
    let _serial = clear_registry();
    let mut backend = shared();
    let commit = InteractionCommit::new(DRESS_UP, 1, "chamfer")
        .with_element("Edge1")
        .with_values([1.5, 0.5]);
    assert!(backend.apply_interaction(&commit, "Box").unwrap().ok);

    let handle = handle();
    let document = handle.lock().unwrap();
    let id = document.get_by_name("Box_Chamfer").expect("the feature");
    assert_eq!(
        document.object(id).unwrap().type_id,
        ferrocad_part::CHAMFER_TYPE
    );
    for (name, expected) in [("Distance", 1.5), ("Distance2", 0.5)] {
        match document.object(id).unwrap().properties.get(name) {
            Some(ferrocad_core::Property::Quantity(value)) => {
                assert!((value.value_mm() - expected).abs() < 1e-9, "{name}")
            }
            other => panic!("{name} is a length, got {other:?}"),
        }
    }
}

#[test]
fn a_push_pull_commit_extrudes_a_face_and_fuses_it_with_no_interpreter() {
    let _serial = clear_registry();
    let mut backend = shared();

    let commit = InteractionCommit::new(PUSH_PULL, 1, "extrude")
        .with_element("Face1")
        .with_values([3.0]);
    let result = backend.apply_interaction(&commit, "Box").unwrap();
    assert!(result.ok, "{}", result.error);

    let handle = handle();
    {
        let document = handle.lock().unwrap();
        let sweep = document.get_by_name("Box_Extrusion").expect("the sweep");
        assert_eq!(
            document.object(sweep).unwrap().type_id,
            ferrocad_part::EXTRUSION_TYPE
        );
        // The boolean is what is drawn; the sweep is scaffolding, so it is hidden.
        assert!(!ferrocad_core::is_visible(document.object(sweep).unwrap()));
        assert!(has_shape(&document, "Box_Extrusion"));

        let id = document.get_by_name("Box_Fuse").expect("the boolean");
        assert_eq!(
            document.object(id).unwrap().type_id,
            ferrocad_part::FUSE_TYPE
        );
        assert!(has_shape(&document, "Box_Fuse"));
        // The boolean consumes the body and links its tool.
        assert!(document
            .object(id)
            .unwrap()
            .properties
            .get("Tool")
            .is_some());
    }

    // Inward is a cut, with its own feature name (guard scoped: the commit locks).
    let commit = InteractionCommit::new(PUSH_PULL, 1, "pocket")
        .with_element("Face1")
        .with_values([-2.0]);
    assert!(backend.apply_interaction(&commit, "Box").unwrap().ok);
    let document = handle.lock().unwrap();
    let id = document.get_by_name("Box_Cut").expect("the cut");
    assert_eq!(document.object(id).unwrap().type_id, ferrocad_part::CUT_TYPE);
    assert!(has_shape(&document, "Box_Cut"));
}
