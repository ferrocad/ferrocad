//! The composed desktop backend: `RustAppBackend` for the document, the embedded
//! interpreter's `PythonScriptHost` for everything a script owns.
//!
//! This is the acceptance for the S9 merge, and it runs the way the edition runs:
//! the `ferrocad`/`Part` modules are **built into this binary**
//! (`ferrocad_combined_py::register_builtins`), so the interpreter and the host
//! share one `ferrocad_core` — and therefore one document registry. Importing the
//! modules from `python/*.abi3.so` instead would give the test a second core, and
//! the composition would be measuring nothing.
//!
//! The edition's app script boots a document named `Shell` with a 20 mm `Box`, so
//! these use the real geometry rather than a stub.

use std::sync::{Mutex, Once};

use ferrocad_shell_api::app::{AppBackend, ScriptHost};
use ferrocad_shell_api::catalog;
use ferrocad_shell_python::{PythonScriptHost, python};
use ferrocad_shell_rust::RustAppBackend;

/// One interpreter and one document registry per process, shared by the tests.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// The modules are built into the interpreter, which must happen before it starts.
static BOOT: Once = Once::new();

/// The document the edition's `bootstrap()` creates.
const DOCUMENT: &str = "Shell";

/// The solid that document carries.
const OBJECT: &str = "Box";

/// The sample's live `App::PropertyLength` (the edition's box bakes its length at
/// creation, so it has no settable one).
const SAMPLE: &str = "Params";

/// The composed backend, booted the way the desktop boots it.
///
/// The `ScriptHost` half owns the boot: `backend.bootstrap()` starts the interpreter,
/// imports the app script and creates the document in the shared registry, and only
/// then does the document half read it.
fn composed() -> RustAppBackend {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], true);
    });
    let mut backend =
        RustAppBackend::from_shared_registry().with_script_host(Box::new(PythonScriptHost));
    let boot = backend.bootstrap().expect("the composed backend boots");
    assert_eq!(boot.document, DOCUMENT, "the script half made the document");
    backend
}

/// Register a provider so the synthetic `Settings/<Feature>` node exists.
///
/// The edition registers none — a workbench does — so the test supplies one.
const REGISTER_GRID: &str = "\
import FerroCADGui; FerroCADGui.registerProvider(\
    'grid', 'test-backend', 'Test',\
    settings=[{'name': 'spacing', 'kind': 'quantity', 'default': '10 mm'}],\
)";

/// The value of one property row, or `None` when the row is absent.
fn row(backend: &RustAppBackend, object: &str, name: &str) -> Option<String> {
    backend
        .properties(object)
        .expect("properties answer")
        .into_iter()
        .find(|row| row.name == name)
        .map(|row| row.value)
}

#[test]
fn the_tree_has_one_copy_of_every_node() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();
    let tree = backend.model_tree().unwrap();

    // `python::model_tree` answers for the whole application — the Settings root and
    // every document. If the host passed the documents through as well, each one
    // would appear twice: that duplication is what the merge has to avoid.
    let names: Vec<&str> = tree.iter().map(|node| node.name.as_str()).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        names.len(),
        "a node is listed twice: {names:?}"
    );

    let count = tree.iter().filter(|node| node.name == DOCUMENT).count();
    assert_eq!(count, 1, "the document is listed {count} times");

    let document = tree.iter().find(|node| node.name == DOCUMENT).unwrap();
    assert!(
        document.objects.iter().any(|object| object.name == OBJECT),
        "the document half built the tree: {document:?}"
    );
}

#[test]
fn settings_rows_are_python_and_document_rows_are_rust() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();
    let registered = backend
        .evaluate(REGISTER_GRID)
        .expect("the registration runs");
    assert!(registered.ok, "a provider registers: {registered:?}");

    // The synthetic root exists exactly once, and it is the only thing the script
    // contributes to the tree.
    let tree = backend.model_tree().unwrap();
    let settings: Vec<_> = tree.iter().filter(|node| node.name == "Settings").collect();
    assert_eq!(settings.len(), 1, "one Settings root: {tree:?}");
    assert!(
        settings[0]
            .objects
            .iter()
            .any(|object| object.name == "Settings/grid"),
        "a registered feature is a Settings node: {:?}",
        settings[0]
    );

    // A script-owned node: the rows come from Python, because the document has never
    // heard of `Settings/grid`.
    let rows = backend.properties("Settings/grid").unwrap();
    assert!(
        rows.iter().any(|row| row.name == "provider"),
        "the selector row is Python's: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.name == "spacing"),
        "the schema row is Python's: {rows:?}"
    );

    // A real object: read straight from the document.
    assert!(
        backend
            .properties(SAMPLE)
            .unwrap()
            .iter()
            .any(|row| row.name == "Length"),
        "the document answers for its own objects: {:?}",
        backend.properties(SAMPLE).unwrap()
    );

    // The boundary itself, stated as a property of the host rather than inferred
    // from the rows above.
    let host = PythonScriptHost;
    assert!(host.extra_properties(SAMPLE).is_none());
    assert!(host.extra_properties("Settings/grid").is_some());
    assert!(host.set_extra_property(SAMPLE, "Length", "1").is_none());
    assert!(
        host.set_extra_property("Settings/grid", "spacing", "5 mm")
            .is_some()
    );
}

#[test]
fn commands_are_listed_once() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();
    let commands = backend.commands().unwrap();

    // The app script registers a `FreeCADGui` class for every catalogue id, so the
    // raw registry holds them all; the merge must not list any id twice.
    let mut ids: Vec<&str> = commands.iter().map(|command| command.id.as_str()).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "a command id is listed twice");

    assert_eq!(
        commands
            .iter()
            .filter(|command| command.id == "App_Move")
            .count(),
        1,
        "a built-in is the catalogue's, once"
    );
    // Merging must not *drop* the catalogue either: it is additive.
    for def in catalog::builtin() {
        assert!(
            commands.iter().any(|command| command.id == def.id),
            "{} disappeared from the palette",
            def.id
        );
    }
    // The edition's own workbench verbs come through from the script half — the
    // Part box the test document carries is created by one of them.
    assert!(
        commands
            .iter()
            .any(|command| command.id.starts_with("Part_")),
        "the workbench's verbs are registered"
    );
}

#[test]
fn a_python_edit_is_visible_to_the_rust_half() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();

    let before = row(&backend, SAMPLE, "Length").expect("the sample has a Length");
    assert_ne!(before, "25 mm", "the sample does not start at 25 mm");

    // The write happens entirely in Python, through `FreeCAD.ActiveDocument`.
    let result = backend
        .evaluate("doc = FreeCAD.ActiveDocument; doc.Params.Length = '25 mm'; doc.recompute()")
        .unwrap();
    assert!(result.ok, "{result:?}");

    // The Rust half reads the same document out of the registry, with no
    // invalidation step and no second copy to refresh.
    assert_eq!(row(&backend, SAMPLE, "Length").as_deref(), Some("25 mm"));

    // And the geometry half agrees: the stage is built from the document the Python
    // write landed in, not from a stale copy of it.
    let meshes = backend.stage_meshes().unwrap();
    assert!(
        meshes.iter().any(|mesh| mesh.object == OBJECT),
        "the stage draws the edited document: {:?}",
        meshes
            .iter()
            .map(|mesh| mesh.object.as_str())
            .collect::<Vec<_>>()
    );
    assert!(!backend.save_document().unwrap().is_empty());
}
