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
use ferrocad_shell_python::DisplaySync;
use ferrocad_shell_python::{PythonScriptHost, python};
use ferrocad_shell_rust::RustAppBackend;
use ferrocad_stage::{InteractionCommit, Stage, TRANSFORM_ID};
use ferrocad_types::Vector3;

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

/// The item's minimum x in world space: the corner a transform moves.
fn min_x(stage: &Stage, entity: u32) -> f64 {
    let item = stage.item(entity).expect("the item");
    // The drawn position, not the raw row: a host may keep the vertices in the frame
    // they were tessellated in and carry the move as the item's placement.
    let world = item.world_matrix();
    item.positions()
        .iter()
        .map(|position| {
            world
                .transform(&Vector3::new(
                    f64::from(position[0]),
                    f64::from(position[1]),
                    f64::from(position[2]),
                ))
                .x
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
fn a_manipulator_commit_routes_by_who_owns_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut backend = composed();

    let meshes = backend.stage_meshes().expect("the stage");
    let shown = meshes
        .iter()
        .find(|mesh| mesh.object == OBJECT)
        .expect("the edition's Box is drawn");
    let entity = shown.entity;
    assert!(
        shown.parts.iter().any(|part| part == "Edge1"),
        "the kernel names the box's edges"
    );

    let mut display = DisplaySync::new();
    let stage = display.build(meshes);
    let before = min_x(&stage, entity);
    assert!(
        before.abs() < 1e-6,
        "the seeded box sits at x = 0: {before}"
    );

    // 1. The shared transform gizmo is the **edition's**: the Part workbench registers a
    //    `transform`/`apply` provider, so the composition routes the commit to the
    //    script half and the document arm is only the fallback. The transaction's name
    //    is what makes that observable: the provider records `Move Box`, the document
    //    arm would record `Transform`.
    let mut matrix = [0.0f64; 16];
    for index in 0..4 {
        matrix[index * 5] = 1.0;
    }
    // Row-major (FreeCAD A11..A44): A14 is the x translation.
    matrix[3] = 12.0;
    let commit = InteractionCommit::new(TRANSFORM_ID, entity, "apply").with_values(matrix);
    let applied = backend
        .apply_interaction(&commit, OBJECT)
        .expect("the transform applied");
    assert!(applied.ok, "{}", applied.error);

    let stage = display.build(backend.stage_meshes().expect("the re-synced stage"));
    let moved = min_x(&stage, entity);
    assert!(
        (moved - (before + 12.0)).abs() < 0.05,
        "the document moved it by the commit's 12 mm: {before} → {moved}"
    );

    // Which half applied it is observable through the transaction's name: the
    // document arm records "Transform", the Part hook "Move Box".
    let undone = backend.run_command("App_Undo").expect("undo runs");
    assert!(
        undone.status.starts_with("undo: Move Box"),
        "the edition's provider applied the transform: {}",
        undone.status
    );
    assert!(backend.run_command("App_Redo").expect("redo runs").ok);

    // 2. A workbench manipulator is the script's: the document half does not know
    //    `dress_up`, so this passes only if it falls through to `PythonScriptHost`
    //    and on to the Part workbench's interaction provider.
    let commit = InteractionCommit::new("dress_up", entity, "fillet")
        .with_element("Edge1")
        .with_values([2.0])
        .with_readout("R 2.00 mm");
    let applied = backend
        .apply_interaction(&commit, OBJECT)
        .expect("the commit routed");
    assert!(
        applied.ok,
        "the script half applied the fillet: {}",
        applied.error
    );

    let objects: Vec<String> = backend
        .stage_meshes()
        .expect("the re-synced stage")
        .into_iter()
        .map(|mesh| mesh.object)
        .collect();
    assert!(
        objects.iter().any(|name| name == "Box_Fillet"),
        "the fillet is drawn: {objects:?}"
    );
    assert!(
        !objects.iter().any(|name| name == OBJECT),
        "the feature replaced its base: {objects:?}"
    );
}

/// A commit is a move: the rows the stage is built from must not change, only the
/// transform that places them. That is what lets `set_stage` patch the packet matrices
/// instead of re-emitting 1.24 M vertices and re-uploading them to the GPU.
#[test]
fn a_commit_moves_the_placement_not_the_rows() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut backend = composed();

    let before = backend.stage_meshes().expect("the stage syncs");
    let row_before = before
        .iter()
        .find(|mesh| mesh.object == OBJECT)
        .expect("the box is drawn")
        .clone();

    let mut matrix = [0.0f64; 16];
    for index in 0..4 {
        matrix[index * 5] = 1.0;
    }
    // Row-major (FreeCAD A11..A44): A14 is the x translation.
    matrix[3] = 12.0;
    let commit =
        InteractionCommit::new(TRANSFORM_ID, row_before.entity, "apply").with_values(matrix);
    let applied = backend
        .apply_interaction(&commit, OBJECT)
        .expect("the commit routes");
    assert!(applied.ok, "{}", applied.error);

    let after = backend.stage_meshes().expect("the stage re-syncs");
    let row_after = after
        .iter()
        .find(|mesh| mesh.object == OBJECT)
        .expect("the box is still drawn");

    assert_eq!(
        row_after.positions, row_before.positions,
        "a rigid move does not rewrite the vertices"
    );
    assert_eq!(row_after.triangles, row_before.triangles);
    let placement = row_after
        .placement
        .expect("the rows carry the move as a transform");
    assert!(
        (placement.to_matrix().m[3] - 12.0).abs() < 1e-6,
        "the move is the row's placement: {:?}",
        placement.to_matrix().m
    );
}

#[test]
fn the_document_half_answers_the_display_mode() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut backend = composed();

    // A workbench's convention is document data: `DisplayMode` is a property, so the
    // document half reads it instead of paying for a display-hook round-trip.
    let changed = backend
        .evaluate("FreeCAD.ActiveDocument.Box.DisplayMode = 'Wireframe'")
        .expect("the property is settable");
    assert!(changed.ok, "{changed:?}");
    let meshes = backend.stage_meshes().unwrap();
    let row = meshes
        .iter()
        .find(|mesh| mesh.object == OBJECT)
        .expect("the box is drawn");
    assert_eq!(row.display.as_deref(), Some("wireframe"));

    let changed = backend
        .evaluate("FreeCAD.ActiveDocument.Box.DisplayMode = 'ShadedWithEdges'")
        .unwrap();
    assert!(changed.ok);
    let meshes = backend.stage_meshes().unwrap();
    let row = meshes.iter().find(|mesh| mesh.object == OBJECT).unwrap();
    assert_eq!(row.display.as_deref(), Some("shaded_edges"));

    // A datum plane is a bounded translucent card, not a solid face.
    let plane = backend
        .add_datum_plane(
            Vector3::new(0.0, 0.0, 30.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .expect("a datum plane is created");
    let meshes = backend.stage_meshes().unwrap();
    let row = meshes
        .iter()
        .find(|mesh| mesh.object == plane)
        .unwrap_or_else(|| panic!("the datum plane {plane} is drawn"));
    assert_eq!(row.display.as_deref(), Some("glass"));
}

#[test]
fn a_sync_asks_the_hook_only_for_what_the_document_cannot_mesh() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();

    // Watch the display hook from Python's side. `_stage_mesh_rows` looks the module
    // attribute up at call time, so replacing it is enough.
    let installed = backend
        .evaluate(
            r#"
import FerroCAD
calls = []
_original = FerroCAD.display_mesh

def _spy(obj):
    calls.append(obj.Name)
    return _original(obj)

FerroCAD.display_mesh = _spy
"#,
        )
        .expect("the spy installs");
    assert!(installed.ok, "{installed:?}");

    // An object with no `Shape`: only a display hook can draw it.
    let added = backend
        .evaluate("FreeCAD.ActiveDocument.addObject('App::FeaturePython', 'Scripted')")
        .expect("the object is added");
    assert!(added.ok, "{added:?}");

    let meshes = backend.stage_meshes().unwrap();
    assert!(
        meshes.iter().any(|mesh| mesh.object == OBJECT),
        "the document meshed the box itself"
    );

    let seen = backend.evaluate("str(sorted(calls))").expect("read back");
    assert!(seen.ok, "{seen:?}");
    assert!(
        !seen.output.contains(OBJECT),
        "the display hook was asked for {OBJECT}: {}",
        seen.output
    );
    assert!(
        seen.output.contains("Scripted"),
        "the display hook still answers for a shape-less object: {}",
        seen.output
    );

    assert!(
        backend
            .evaluate("FerroCAD.display_mesh = _original")
            .unwrap()
            .ok
    );
}

#[test]
fn the_display_hook_tessellates_at_the_document_s_deflection() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let backend = composed();

    // A sphere, not the box: a flat-faced solid is twelve triangles at any setting, so
    // only a curved surface can answer differently.
    let added = backend
        .evaluate(
            r#"
doc = FreeCAD.ActiveDocument
ball = doc.addObject('Part::Sphere', 'Ball')
ball.Radius = 10.0
doc.recompute()
"#,
        )
        .expect("the sphere is added");
    assert!(added.ok, "{added:?}");

    // What the hook draws for it. `stage_meshes()` is the Python-side sync, which asks
    // the hook for everything — the path a host that cannot mesh the document takes.
    let hook_triangles = || -> usize {
        python::stage_meshes()
            .unwrap()
            .into_iter()
            .find(|mesh| mesh.object == "Ball")
            .expect("the hook draws the sphere")
            .triangles
            .len()
            / 3
    };

    let at_display = hook_triangles();

    // The invariant is that the hook tessellates at the deflection the document path
    // uses, and the only way to see a hook that quietly asked for something else is to
    // move the published number: the two would otherwise differ in the *cache key* and
    // nowhere else. Comparing the returned meshes cannot show it, because OCCT keeps a
    // triangulation that is already finer than the one asked for.
    let imported = backend.evaluate("import Part").expect("Part imports");
    assert!(imported.ok, "{imported:?}");
    let published = backend.evaluate("Part.DISPLAY_DEFLECTION").unwrap();
    assert!(published.ok, "{published:?}");
    let original = published.output.trim().to_string();

    let rebound = backend.evaluate("Part.DISPLAY_DEFLECTION = 0.01").unwrap();
    assert!(rebound.ok, "{rebound:?}");
    let finer = hook_triangles();
    assert!(
        finer > at_display,
        "the display hook ignored Part.DISPLAY_DEFLECTION ({at_display} triangles at \
         {original}, {finer} at 0.01): it must pass it to `Shape.mesh` so its memo entry \
         is the document's"
    );

    assert!(
        backend
            .evaluate(&format!("Part.DISPLAY_DEFLECTION = {original}"))
            .unwrap()
            .ok
    );
}
