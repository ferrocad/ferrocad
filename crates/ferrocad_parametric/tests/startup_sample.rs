//! One startup document, loaded by both hosts.
//!
//! The desktop's app script opens `python/ferrocad_shell/Sample.FCStd` from disk;
//! `RustAppBackend` decodes the same archive from its embedded bytes (the browser's
//! path). This test boots both in one process and compares the document name, the
//! object tree, the object types and the dimensions a user sees in the property pane.

use std::sync::{Mutex, Once};

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_python::{PythonScriptHost, python};
use ferrocad_shell_rust::RustAppBackend;
use ferrocad_stage::{InteractionCommit, TRANSFORM_ID};

/// The interpreter and the document registry are process-wide.
static PYTHON: Mutex<()> = Mutex::new(());

/// The modules are built into the interpreter, which must happen before it starts.
static BOOT: Once = Once::new();

/// The desktop composition: `RustAppBackend` over the registry the interpreter fills.
fn desktop() -> RustAppBackend {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        // Headless: the parameter store stays in memory, so a test run never writes
        // the user's real configuration.
        python::configure("ferrocad_shell", &[], &[], false);
    });
    let mut backend =
        RustAppBackend::from_shared_registry().with_script_host(Box::new(PythonScriptHost));
    backend.bootstrap().expect("the desktop sample boots");
    backend
}

/// The sample's `(name, type_id)` rows, in document order.
fn tree(backend: &dyn AppBackend) -> Vec<(String, String)> {
    backend
        .model_tree()
        .expect("a model tree")
        .into_iter()
        .find(|document| document.name == "Sample")
        .expect("the sample document")
        .objects
        .into_iter()
        .map(|object| (object.name, object.type_id))
        .collect()
}

/// One property row's display value.
fn row(backend: &dyn AppBackend, object: &str, name: &str) -> String {
    backend
        .properties(object)
        .expect("properties answer")
        .into_iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("{object}.{name} is a row"))
        .value
}

/// The minimum x of the drawn Fillet, in world space.
fn min_x(stage: &[StageMesh]) -> f64 {
    stage
        .iter()
        .find(|mesh| mesh.object == "Fillet")
        .expect("the filleted body is drawn")
        .mesh
        .positions
        .iter()
        .map(|position| f64::from(position[0]))
        .fold(f64::INFINITY, f64::min)
}

#[test]
fn the_desktop_and_the_browser_boot_one_sample() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let desktop = desktop();
    let browser = RustAppBackend::new();

    assert_eq!(
        tree(&desktop),
        tree(&browser),
        "the two hosts list the same objects with the same types"
    );
    assert_eq!(
        tree(&desktop)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Params", "Derived", "Group", "Box", "Fillet"],
        "the canonical tree"
    );

    for (object, name) in [
        ("Box", "Length"),
        ("Box", "Width"),
        ("Box", "Height"),
        ("Box", "Placement"),
        ("Fillet", "Radius"),
        ("Fillet", "Placement"),
    ] {
        assert_eq!(
            row(&desktop, object, name),
            row(&browser, object, name),
            "{object}.{name} agrees across the hosts"
        );
    }
    assert_eq!(row(&desktop, "Box", "Length"), "10 mm");
    assert_eq!(row(&desktop, "Fillet", "Radius"), "3 mm");
    assert_eq!(
        row(&desktop, "Box", "Placement"),
        "(0.0, 0.0, 0.0)",
        "the box corner is the world origin"
    );
    assert_eq!(
        row(&desktop, "Fillet", "Placement"),
        "(0.0, 0.0, 0.0)",
        "the dress-up's own origin is the box corner too"
    );
    assert_eq!(
        row(&desktop, "Derived", "Result"),
        "20 mm",
        "the expression evaluated on both hosts"
    );
}

/// The browser's document half can move the visible dress-up feature itself.
///
/// The sample's visible body is `Fillet`, not the hidden `Box`; the browser installs
/// `RustAppBackend` with no script half, so its transform path is `apply_transform`.
/// This is the path the Web Move GIF exercises.
#[test]
fn the_browser_document_half_moves_the_dressed_up_body() {
    let mut browser = RustAppBackend::new();
    let before = browser.stage_meshes().expect("the stage syncs");
    assert!(
        min_x(&before).abs() < 1e-6,
        "the body starts with its corner at the origin"
    );
    let entity = before
        .iter()
        .find(|mesh| mesh.object == "Fillet")
        .expect("the filleted body is drawn")
        .entity;

    let mut matrix = [0.0f64; 16];
    for index in 0..4 {
        matrix[index * 5] = 1.0;
    }
    matrix[3] = 12.0;
    let commit = InteractionCommit::new(TRANSFORM_ID, entity, "apply").with_values(matrix);
    let applied = browser
        .apply_interaction(&commit, "Fillet")
        .expect("the transform routes");
    assert!(applied.ok, "{}", applied.error);

    let after = browser.stage_meshes().expect("the stage re-syncs");
    assert!(
        (min_x(&after) - 12.0).abs() < 1e-3,
        "the dress-up itself moved by 12 mm: {}",
        min_x(&after)
    );
}
