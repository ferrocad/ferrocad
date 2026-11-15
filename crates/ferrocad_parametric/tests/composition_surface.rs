//! The composition's surface: what the desktop backend answers.
//!
//! The desktop is one `AppBackend`: `DocumentBackend` for the document,
//! `PythonScriptHost` for everything a script owns. This file began as the B0 parity
//! ledger, comparing that composition with the retired `PythonAppBackend`
//! (`docs/project/tasks/backend-parity.md`). B5 deleted the legacy type, so each
//! comparison is now an assertion: the expectations the ledger pinned are what these
//! tests keep.
//!
//! What is pinned: the command list (catalogue metadata plus the script's live state),
//! the model tree and its document switch, the routed command outcomes, the script's
//! semantic stores, the property rows, an edit and its undo, a transform commit
//! through the edition's interaction provider, and (B6) the agreement of the two
//! script halves over one document.
//!
//! # Why this crate
//!
//! The harness has to look like an **edition** (`ferrocad_combined_py::register_builtins`):
//! `ferrocad`, `Part`, `Mesh` and `MeshPart` are this binary's modules, so the
//! interpreter and the document half share one `ferrocad_core` — and therefore one
//! document registry. Importing them from `python/*.abi3.so` instead would give the
//! test a second core, and every read below would describe an empty document. It is
//! also the crate whose CI job has the kernel.
//!
//! # One core
//!
//! [`the_test_image_has_one_core`] asserts that (`ferrocad._kernel_id` ==
//! `ferrocad_core::application_id()`).

use std::collections::BTreeMap;
use std::sync::{Mutex, Once};

use ferrocad_shell_api::app::{AppBackend, ScriptHost};
use ferrocad_shell_api::catalog;
use ferrocad_shell_api::commands::CommandOutcome;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::PropRow;
use ferrocad_shell_api::view::ViewState;
use ferrocad_shell_python::{PythonScriptHost, python};
use ferrocad_shell_rust::{DocumentBackend, PartEngine};
use ferrocad_stage::{InteractionCommit, TRANSFORM_ID};

/// One interpreter and one document registry per process, shared by the tests.
static PYTHON: Mutex<()> = Mutex::new(());

/// The extension must be in the interpreter's built-in table before it starts.
static BOOT: Once = Once::new();

/// The edition's interaction provider, registered once (it lives in the interpreter).
static PROVIDER: Once = Once::new();

/// The document the app script's `bootstrap()` opens.
const DOCUMENT: &str = "Sample";

/// A `FeaturePython` object in that document, with a `Length` and an expression.
const OBJECT: &str = "Params";

/// The property-driven object: `Result` is `Params.Length * 2`.
const DERIVED: &str = "Derived";

/// Close every document an earlier scenario left behind.
const CLOSE_ALL: &str = "\
for _name in list(FreeCAD.listDocuments()):
    FreeCAD.closeDocument(_name)
";

/// Reset the three semantic switches the Gui toggles write.
///
/// They live in the process-wide parameter store, so a test that asserts a toggle's
/// effect has to start from a known state.
const RESET_TOGGLES: &str = "\
import FerroCAD
FerroCAD.Grid.enabled = False
FerroCAD.Axis.enabled = False
FerroCAD.Gizmo.enabled = False
";

/// A `transform`/`apply` provider, the kind an edition registers.
///
/// The app script ships no edition mod, so an unowned transform commit would fall to
/// the document's own arm. A provider is what `FerroCAD.register_interaction_provider`
/// is for, and it is what makes the commit's *transaction name* the mod's.
const REGISTER_TRANSFORM_PROVIDER: &str = "\
import FreeCAD
import FerroCAD
if not getattr(FerroCAD, '_surface_transform', False):
    def _surface_transform(obj, manipulator, action, element, values):
        if manipulator != 'transform' or action != 'apply' or len(values) != 16:
            return None
        doc = obj.Document
        doc.openTransaction('Move %s' % obj.Name)
        try:
            obj.Placement = FreeCAD.Placement(FreeCAD.Matrix(*values)) * obj.Placement
            doc.recompute()
            doc.commitTransaction()
        except Exception as exc:
            doc.abortTransaction()
            return {'ok': False, 'error': str(exc)}
        return {'ok': True}
    FerroCAD.register_interaction_provider(_surface_transform)
    FerroCAD._surface_transform = True
";

/// The composition, booted the way the desktop boots it, on a fresh document.
///
/// The caller holds [`PYTHON`]: the interpreter and the registry are process-wide.
/// `bootstrap()` is what starts the interpreter and makes the sample document, so it
/// runs before the cleanup; the second call then makes the sample the only document.
fn fresh() -> DocumentBackend {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        // Headless (`GuiUp == 0`): the parameter store stays in memory, so a test
        // run never writes the user's real configuration.
        python::configure("ferrocad_shell", &[], &[], false);
    });
    let mut backend =
        PartEngine::shared_backend().with_script_host(Box::new(PythonScriptHost));
    backend.bootstrap().expect("the composition boots");
    // After the boot, because `bootstrap` is what puts `python/` on `sys.path`.
    PROVIDER.call_once(|| {
        let registered = python::evaluate(REGISTER_TRANSFORM_PROVIDER).expect("evaluate");
        assert!(
            registered.ok,
            "registering the transform provider failed: {}",
            registered.output
        );
    });
    let cleared = python::evaluate(CLOSE_ALL).expect("evaluate");
    assert!(cleared.ok, "close-all failed: {}", cleared.output);
    let booted = backend.bootstrap().expect("the composition re-boots");
    assert_eq!(
        booted.document, DOCUMENT,
        "the app script made the document"
    );
    backend
}

/// Evaluate `code` through the bridge and print the result, trimmed.
fn print(code: &str) -> String {
    python::evaluate(&format!("print({code})"))
        .expect("evaluate")
        .output
        .trim()
        .to_string()
}

/// One property row's value, or `None` when the row is absent.
fn row_value(backend: &DocumentBackend, object: &str, name: &str) -> Option<String> {
    backend
        .properties(object)
        .expect("properties answer")
        .into_iter()
        .find(|row| row.name == name)
        .map(|row| row.value)
}

/// Whether `Params.Length` is the only property a script can turn into a row, etc.
fn length_is(backend: &DocumentBackend, value: &str) -> bool {
    row_value(backend, OBJECT, "Length")
        .unwrap_or_default()
        .contains(value)
}

/// The test image must have one `ferrocad_core`.
///
/// `register_builtins()` makes `import ferrocad` resolve to the module linked into
/// this binary. If it resolved to an `.so` instead (`ferrocad.abi3.so`, or
/// `ferrocad_combined.abi3.so`), the interpreter would own a second registry and
/// every read below would describe an empty document.
#[test]
fn the_test_image_has_one_core() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    // `fresh()` is what boots the interpreter and puts `python/` on `sys.path`.
    drop(fresh());
    let printed = python::evaluate("import ferrocad\nprint(ferrocad._kernel_id)")
        .expect("evaluate");
    assert!(printed.ok, "{}", printed.output);
    assert_eq!(
        printed.output.trim(),
        ferrocad_core::application_id().to_string(),
        "the interpreter's `ferrocad` is a different core from this binary's: \
         `import ferrocad` is loading a .so, not the module built into the test binary",
    );
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Rows by id, so a test names what it asserts.
fn command_rows(backend: &DocumentBackend) -> BTreeMap<String, ferrocad_shell_api::commands::CommandInfo> {
    backend
        .commands()
        .expect("commands answer")
        .into_iter()
        .map(|command| (command.id.clone(), command))
        .collect()
}

#[test]
fn the_command_list_is_the_catalogue_plus_the_script() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let backend = fresh();
    let rows = command_rows(&backend);

    assert_eq!(
        rows.len(),
        backend.commands().unwrap().len(),
        "no id is listed twice"
    );

    // The catalogue is the source of every built-in's metadata.
    for def in catalog::builtin() {
        let row = rows
            .get(def.id)
            .unwrap_or_else(|| panic!("{} is missing from the command list", def.id));
        assert_eq!(row.menu, def.menu, "{} menu", def.id);
        assert_eq!(row.tooltip, def.tooltip, "{} tooltip", def.id);
        assert_eq!(row.accel, def.accel, "{} accel", def.id);
        assert_eq!(row.surface, def.surface, "{} surface", def.id);
        assert_eq!(row.parent, def.parent, "{} parent", def.id);
        assert_eq!(row.wheel, def.wheel, "{} wheel", def.id);
        assert_eq!(row.dev_only, def.dev_only, "{} dev_only", def.id);
    }

    // Live state is the script's (`IsActive`): a fresh document has no history and
    // nothing selected, so every edit verb is greyed.
    for id in [
        catalog::UNDO,
        catalog::REDO,
        catalog::MOVE,
        catalog::ROTATE,
        catalog::TOGGLE_VISIBILITY,
    ] {
        assert!(
            !rows[id].enabled,
            "{id} is disabled with nothing to act on"
        );
    }

    // A script verb the catalogue does not list reaches the palette with its own
    // metadata, and Draft's snap row is Draft's (label, surface, check state).
    let delete = rows
        .get("App_Delete")
        .expect("the app script's own verb is listed");
    assert_eq!(delete.menu, "Delete");
    assert_eq!(delete.surface, "radial");
    let snap = rows
        .get(catalog::SNAP_LOCK)
        .expect("Draft's snap switch is listed");
    assert_eq!(snap.menu, "Snap Lock");
    assert_eq!(snap.surface, "snap");
    assert!(snap.checkable);
}

// ---------------------------------------------------------------------------
// The model tree
// ---------------------------------------------------------------------------

/// `(document, active, [(object, type)])`, in the order the tree reports.
type Tree = Vec<(String, bool, Vec<(String, String)>)>;

fn tree_rows(backend: &DocumentBackend) -> Tree {
    backend
        .model_tree()
        .expect("the tree answers")
        .into_iter()
        .map(|document| {
            (
                document.name,
                document.active,
                document
                    .objects
                    .into_iter()
                    .map(|object| (object.name, object.type_id))
                    .collect(),
            )
        })
        .collect()
}

/// The tree's document rows: everything but the synthetic `Settings` namespace.
///
/// The namespace is not a document — the shell never follows it and it is never active —
/// and since B13 the document half offers it even in a build with no provider registered
/// (`Settings/grid` is answered from the shell's view state), so a test about documents
/// has to ask for the documents. `the_settings_root_is_not_a_document` pins the other half.
fn documents(backend: &DocumentBackend) -> Tree {
    tree_rows(backend)
        .into_iter()
        .filter(|(name, _, _)| name != "Settings")
        .collect()
}

#[test]
fn the_settings_root_is_not_a_document() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let backend = fresh();
    let tree = tree_rows(&backend);

    let settings: Vec<&(String, bool, Vec<(String, String)>)> =
        tree.iter().filter(|(name, _, _)| name == "Settings").collect();
    assert_eq!(settings.len(), 1, "one synthetic root: {tree:?}");
    let (_, active, objects) = settings[0];
    assert!(!*active, "the synthetic root is never a document");
    assert_eq!(
        objects,
        &vec![(
            "Settings/grid".to_string(),
            "FerroCAD::Settings".to_string()
        )],
        "the document half's grid card is the one feature it owns"
    );
}

#[test]
fn the_tree_lists_the_sample_document() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let backend = fresh();
    let tree = documents(&backend);

    assert_eq!(tree.len(), 1, "{tree:?}");
    let (name, active, objects) = &tree[0];
    assert_eq!(name, DOCUMENT);
    assert!(active, "the booted document is the active one");
    let names: Vec<&str> = objects.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Params", "Derived", "Group", "Box", "Fillet"], "{objects:?}");
    let types: BTreeMap<&str, &str> = objects
        .iter()
        .map(|(name, type_id)| (name.as_str(), type_id.as_str()))
        .collect();
    assert_eq!(types["Params"], "App::FeaturePython");
    assert_eq!(types["Group"], "App::DocumentObjectGroup");
}

#[test]
fn the_tree_lists_every_document_and_switches() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let mut backend = fresh();

    // A second document; `newDocument` makes it the active one.
    let created = python::evaluate("FreeCAD.newDocument('Second')").expect("evaluate");
    assert!(created.ok, "{}", created.output);

    let tree = documents(&backend);
    assert_eq!(
        tree.iter()
            .map(|(name, active, _)| (name.as_str(), *active))
            .collect::<Vec<_>>(),
        vec![("Sample", false), ("Second", true)],
        "every document is listed, in name order, and the active one is marked"
    );

    // Switching back is one call; the tree follows and the rows are the document's.
    backend
        .activate_document(DOCUMENT)
        .expect("activate_document answers");
    assert_eq!(
        documents(&backend)
            .iter()
            .map(|(name, active, _)| (name.as_str(), *active))
            .collect::<Vec<_>>(),
        vec![(DOCUMENT, true), ("Second", false)],
        "the switch moved the active mark"
    );
    assert!(
        length_is(&backend, "10"),
        "the active document's properties are readable after the switch"
    );
}

// ---------------------------------------------------------------------------
// Command outcomes
// ---------------------------------------------------------------------------

fn run(backend: &mut DocumentBackend, id: &str) -> CommandOutcome {
    backend.run_command(id).expect("run_command answers")
}

/// The state one outcome requests, applied the way the shell applies it.
fn applied(outcome: &CommandOutcome) -> ViewState {
    let mut state = ViewState::default();
    if let Some(view) = outcome.view.as_ref() {
        state.apply(view).expect("the shell accepts the request");
    }
    state
}

#[test]
fn the_routed_commands_report_their_work() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let mut backend = fresh();
    let reset = python::evaluate(RESET_TOGGLES).expect("evaluate");
    assert!(reset.ok, "{}", reset.output);

    // The four presentation verbs run in the script and return the semantic order.
    assert_eq!(applied(&run(&mut backend, catalog::TOGGLE_GRID)).grid, true);
    assert_eq!(
        applied(&run(&mut backend, catalog::TOGGLE_AXIS_CROSS))
            .axis
            .enabled,
        true
    );
    assert_eq!(
        applied(&run(&mut backend, catalog::TOGGLE_GIZMO))
            .gizmo
            .enabled,
        true
    );
    let workplane = run(&mut backend, catalog::VIEW_WORKPLANE);
    assert!(workplane.ok);
    assert_eq!(
        workplane.view.as_ref().and_then(|view| view.workplane.as_deref()),
        Some("view")
    );

    // The edit verbs hand the shell a session.
    let move_ = run(&mut backend, catalog::MOVE);
    assert!(move_.ok);
    assert_eq!(move_.gizmo.as_deref(), Some(catalog::MOVE_GIZMO));
    assert!(move_.dialog.is_some(), "Move opens its form");
    assert!(move_.token.is_some(), "Move mints a dialog token");
    assert_eq!(run(&mut backend, catalog::ROTATE).gizmo.as_deref(), Some("rotate"));
    assert_eq!(
        run(&mut backend, catalog::FIT)
            .view
            .as_ref()
            .and_then(|view| view.fit),
        Some(true)
    );

    // Nothing acts on an empty document or selection.
    assert!(!run(&mut backend, catalog::UNDO).ok, "nothing to undo");
    assert!(!run(&mut backend, catalog::REDO).ok, "nothing to redo");
    let hidden = run(&mut backend, catalog::TOGGLE_VISIBILITY);
    assert!(!hidden.ok);
    assert!(hidden.error.contains("nothing selected"), "{}", hidden.error);

    // A script verb the catalogue does not list routes to the registry, and its
    // answer is the script's (here: the status, not a document edit).
    let delete = run(&mut backend, "App_Delete");
    assert!(delete.ok, "{}", delete.error);
    assert!(delete.status.contains("nothing selected"), "{}", delete.status);
}

/// B2 made the toggles the script's, so the store the providers read is the one the
/// command writes.
#[test]
fn a_toggle_writes_the_script_store() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let mut backend = fresh();
    let reset = python::evaluate(RESET_TOGGLES).expect("evaluate");
    assert!(reset.ok, "{}", reset.output);
    assert_eq!(print("FerroCAD.Grid.enabled"), "False");

    assert!(run(&mut backend, catalog::TOGGLE_GRID).ok);
    assert_eq!(print("FerroCAD.Grid.enabled"), "True");

    // The other two toggles write their own stores.
    assert!(run(&mut backend, catalog::TOGGLE_AXIS_CROSS).ok);
    assert_eq!(print("FerroCAD.Axis.enabled"), "True");
    assert!(run(&mut backend, catalog::TOGGLE_GIZMO).ok);
    assert_eq!(print("FerroCAD.Gizmo.enabled"), "True");
}

// ---------------------------------------------------------------------------
// Property rows
// ---------------------------------------------------------------------------

#[test]
fn the_property_rows_are_the_editor_rows() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let backend = fresh();
    let rows = backend.properties(OBJECT).expect("properties answer");

    // `Label` is a document-object field, not a `PropertiesList` entry, and the
    // editor shows it first.
    let label = &rows[0];
    assert_eq!(label.name, "Label");
    assert_eq!(label.value, OBJECT);
    assert_eq!(label.kind, "text");
    assert_eq!(label.type_id, "App::PropertyString");
    assert!(label.editable);

    let length = rows
        .iter()
        .find(|row| row.name == "Length")
        .expect("the sample has a Length");
    assert_eq!(length.kind, "quantity");
    assert!(length.value.contains("10"), "{}", length.value);
    assert!(length.editable);

    // An enumeration carries its choices, or the select box could only misspell one.
    let mode = rows
        .iter()
        .find(|row| row.name == "Mode")
        .expect("the sample has a Mode");
    assert_eq!(mode.kind, "enum");
    assert_eq!(mode.choices, ["Fast", "Accurate"]);

    // An expression-driven property is read-only: a recompute would overwrite an edit.
    let result = backend
        .properties(DERIVED)
        .expect("properties answer")
        .into_iter()
        .find(|row| row.name == "Result")
        .expect("Derived has a Result");
    assert!(!result.editable, "an expression owns the value");
    assert_eq!(result.value, "20 mm", "Params.Length * 2");
}

// ---------------------------------------------------------------------------
// An edit and its undo
// ---------------------------------------------------------------------------

#[test]
fn an_edit_and_its_undo_round_trip() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let mut backend = fresh();

    let set = backend
        .set_property(OBJECT, "Length", "25 mm")
        .expect("set_property answers");
    assert!(set.ok, "set_property failed: {}", set.error);
    assert!(length_is(&backend, "25"));
    assert_eq!(
        row_value(&backend, DERIVED, "Result").as_deref(),
        Some("50 mm"),
        "recompute propagated the edit to the expression"
    );

    assert!(run(&mut backend, catalog::UNDO).ok);
    assert!(length_is(&backend, "10"), "undo restored the value");
    assert_eq!(
        row_value(&backend, DERIVED, "Result").as_deref(),
        Some("20 mm"),
        "and the expression recomputed"
    );
    assert!(run(&mut backend, catalog::REDO).ok);
    assert!(length_is(&backend, "25"), "redo re-applied it");
}

// ---------------------------------------------------------------------------
// A transform commit
// ---------------------------------------------------------------------------

#[test]
fn a_transform_commit_runs_the_editions_provider() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let mut backend = fresh();
    // `Params` has no `Placement`; the gesture needs one.
    let added = python::evaluate(
        "\
obj = FreeCAD.ActiveDocument.getObject('Params')
if 'Placement' not in obj.PropertiesList:
    obj.addProperty('App::PropertyPlacement', 'Placement', 'Base', 'Placement')
",
    )
    .expect("evaluate");
    assert!(added.ok, "addProperty failed: {}", added.output);

    let mut commit = InteractionCommit::new(TRANSFORM_ID, 0, "apply");
    // A +10 mm translation, row-major 4x4.
    commit.values = vec![
        1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let applied = backend
        .apply_interaction(&commit, OBJECT)
        .expect("apply_interaction answers");
    assert!(applied.ok, "{}", applied.error);

    let placement = row_value(&backend, OBJECT, "Placement").unwrap_or_default();
    assert!(
        placement.starts_with("(10"),
        "the delta composed on the left: {placement}"
    );

    // The transaction name is what names the executor: the provider records
    // `Move Params`; the document's own arm would record `Transform`.
    assert_eq!(
        print("FreeCAD.ActiveDocument.UndoNames[0]"),
        "Move Params",
        "the edition's provider applied the commit"
    );
}

// ---------------------------------------------------------------------------
// The two script halves
// ---------------------------------------------------------------------------

/// One `properties()` row, projected to the fields both halves emit.
type ScriptRow = (String, String, String, String, String, bool, Vec<String>);

fn cpython_script_rows(object: &str) -> Vec<ScriptRow> {
    python::properties(object)
        .expect("the CPython bridge answers")
        .into_iter()
        .map(|row: PropRow| {
            (
                row.name,
                row.type_id,
                row.value,
                row.status,
                row.kind,
                row.editable,
                row.choices,
            )
        })
        .collect()
}

fn wasm_script_rows(json: &str) -> Vec<ScriptRow> {
    let text = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .expect("properties() returns a JSON array")
        .iter()
        .map(|row| {
            (
                text(row, "name"),
                text(row, "type"),
                text(row, "value"),
                text(row, "status"),
                text(row, "kind"),
                row.get("editable")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                row.get("choices")
                    .and_then(|value| value.as_array())
                    .map(|choices| {
                        choices
                            .iter()
                            .filter_map(|choice| choice.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// B6: CPython and RustPython run the same `python/ferrocad_shell`, over one document.
///
/// A **capability matrix**, not equality: the wasm facade answers the console, the
/// property rows, the Gui app's presentation half, its selection mirror and the mod
/// provider registry (B18a steps 1–5), and nothing else — no tools, no filesystem, no
/// document writes. Each row below says which of those it is.
#[test]
fn the_two_script_halves_agree() {
    let _guard = PYTHON.lock().unwrap_or_else(|error| error.into_inner());
    let _backend = fresh();

    // CPython: the same bridge functions the shell asks for.
    let cpython_rows = cpython_script_rows(OBJECT);
    assert!(
        cpython_rows.iter().any(|row| row.0 == "Length"),
        "there are rows to compare: {cpython_rows:?}"
    );
    let cpython_name = python::evaluate("FreeCAD.ActiveDocument.Name").expect("evaluate");
    assert!(cpython_name.ok, "{}", cpython_name.output);
    let cpython_commands = python::commands().expect("the registry answers");
    let cpython_delete = PythonScriptHost
        .run_command("App_Delete")
        .expect("the script owns App_Delete");
    let cpython_hello = PythonScriptHost
        .hello()
        .expect("hello answers")
        .expect("CPython greets");
    // B18a steps 5 and 3: the provider registry and the settings card it feeds are shared
    // policy, so the same registration renders the same rows. Register, read and drop it
    // again, leaving the process-wide CPython facade as it was found. The schema uses a
    // `get` hook and no setter on purpose: a computed row reads no parameter store, so
    // the comparison writes nothing to the developer's own FreeCAD parameters.
    let cpython_card = {
        let setup = python::evaluate(
            "import FerroCADGui\nFerroCADGui.registerProvider('B18aStep5', 'half', 'Step 5', settings=[{'name': 'half', 'kind': 'text', 'default': '', 'get': lambda: 'step 5'}])",
        )
        .expect("the facade registers a provider");
        assert!(setup.ok, "{}", setup.output);
        let feature = PythonScriptHost
            .provider_features()
            .into_iter()
            .find(|feature| feature.feature == "B18aStep5");
        let rows = PythonScriptHost.extra_properties("Settings/B18aStep5");
        // A computed row with no setter is read-only; the refusal touches no store.
        let refused = PythonScriptHost.set_extra_property("Settings/B18aStep5", "half", "x");
        let cleanup = python::evaluate("FerroCADGui.unregisterProvider('B18aStep5', 'half')")
            .expect("the facade drops a provider");
        assert!(cleanup.ok, "{}", cleanup.output);
        (feature, rows, refused)
    };

    // RustPython over the same registry. It compiles recursively, so it gets its own
    // big-stack thread (as `ferrocad_shell_pywasm/tests/live.rs` does); the document
    // half already booted CPython, so this host adopts `Shell` rather than building a
    // sample of its own.
    let wasm = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let host = ferrocad_shell_pywasm::live().expect("RustPython boots over the registry");
            (
                host.call("properties", &[OBJECT]).expect("the shell script runs"),
                host.call("evaluate", &["FreeCAD.ActiveDocument.Name"])
                    .expect("evaluate runs"),
                host.commands(),
                host.run_command("App_Delete"),
                host.hello().expect("hello answers").expect("RustPython greets"),
                {
                    // B18a step 4: with the shell's selection pushed down, the
                    // predicate is judged and the command reaches the document — which
                    // is where the read-only half refuses it.
                    host.on_selection_changed(&[SelectionItem::new(OBJECT)]);
                    host.run_command("App_Delete")
                },
                {
                    // B18a steps 5 and 3: the same registration as CPython's, through
                    // the ported registry; drop it so the interpreter is left clean.
                    let setup = host
                        .evaluate(
                            "import FerroCADGui\nFerroCADGui.registerProvider('B18aStep5', 'half', 'Step 5', settings=[{'name': 'half', 'kind': 'text', 'default': '', 'get': lambda: 'step 5'}])",
                        )
                        .expect("register runs");
                    assert!(setup.ok, "{}", setup.output);
                    let feature = host
                        .provider_features()
                        .into_iter()
                        .find(|feature| feature.feature == "B18aStep5");
                    let rows = host.extra_properties("Settings/B18aStep5");
                    let refused = host.set_extra_property("Settings/B18aStep5", "half", "x");
                    let cleanup = host
                        .evaluate("FerroCADGui.unregisterProvider('B18aStep5', 'half')")
                        .expect("unregister runs");
                    assert!(cleanup.ok, "{}", cleanup.output);
                    (feature, rows, refused)
                },
            )
        })
        .expect("a worker thread")
        .join()
        .expect("the thread did not panic");

    // Same: one script source, one document, one presenter — the rows agree field for
    // field, `Label` included (the bridge synthesizes it) and the status names too.
    assert_eq!(
        wasm_script_rows(&wasm.0),
        cpython_rows,
        "the script-visible property rows agree"
    );

    // Same: the console reads the same document, in the same namespace.
    let wasm_name: serde_json::Value =
        serde_json::from_str(&wasm.1).expect("evaluate returns JSON");
    assert_eq!(wasm_name["ok"], true, "{wasm_name}");
    assert_eq!(wasm_name["output"], cpython_name.output);

    // Intentional: the greeting names the VM, because the console should say which
    // one is behind it.
    assert!(cpython_hello.contains("Python"), "{cpython_hello}");
    assert!(wasm.4.contains("RustPython"), "{}", wasm.4);

    // Same registry, different reach: `bootstrap.py` ports `FreeCADGui._command_list`
    // (B18a step 2), so the shell module's own registrations are listed by both halves.
    assert!(
        cpython_commands.len() >= catalog::builtin().len(),
        "CPython lists the catalogue"
    );
    assert!(
        wasm.2.iter().any(|command| command.id == "App_ToggleGrid"),
        "the wasm half lists the ids the shell registered: {:?}",
        wasm.2.iter().map(|command| &command.id).collect::<Vec<_>>()
    );

    // The selection mirror is real (B18a step 4), so an empty selection is judged the
    // same on both halves: `App_Delete` reports its own early-out, where it used to
    // fail on a missing `FreeCADGui.Selection`.
    assert!(cpython_delete.ok, "the script owns App_Delete");
    let wasm_idle = wasm
        .3
        .expect("the registry lists App_Delete, so the script answers it");
    assert!(wasm_idle.ok, "{wasm_idle:?}");
    assert!(
        wasm_idle.status.contains("nothing selected"),
        "{}",
        wasm_idle.status
    );

    // With a selection the delete reaches the document and stops at the doorway's one
    // missing verb: `removeObject` is not part of it (B18b item 5 wrote create/set/
    // recompute). The command catches its own failure and reports it as a status, so
    // `ok` is not the field to read. The mirror answered, which is what B18a step 4 added.
    let wasm_selected = wasm.5.expect("the registry still lists App_Delete");
    assert!(
        wasm_selected.status.contains("removeObject"),
        "the refusal names the missing verb: {wasm_selected:?}"
    );
    assert!(
        !wasm_selected.status.contains("Selection"),
        "the mirror answered, so it cannot be the reason: {wasm_selected:?}"
    );

    // Same: a registered provider is listed with the same manifest by both halves
    // (B18a step 5), which is what lets the About card and Dev Tools name a mod's
    // providers on either runtime.
    let (cpython_provider, cpython_rows, cpython_refusal) = cpython_card;
    let (wasm_provider, wasm_rows, wasm_refusal) = wasm.6;
    let cpython_provider = cpython_provider.expect("CPython lists the provider it registered");
    let wasm_provider = wasm_provider.expect("the wasm half lists the provider it registered");
    assert_eq!(cpython_provider.providers.len(), 1, "{cpython_provider:?}");
    assert_eq!(wasm_provider.providers.len(), 1, "{wasm_provider:?}");
    assert_eq!(
        wasm_provider.providers[0].backend, cpython_provider.providers[0].backend,
        "the same backend id"
    );
    assert_eq!(
        wasm_provider.providers[0].label, cpython_provider.providers[0].label,
        "the same label"
    );
    assert!(wasm_provider.providers[0].active);
    assert!(cpython_provider.providers[0].active);

    // Same: the settings card the registration produces (B18a step 3). The selector row
    // and the schema row agree field for field, so a card cannot render differently on
    // the two runtimes, and a computed row with no setter is refused by both.
    let cpython_rows = cpython_rows.expect("CPython renders the card");
    let wasm_rows = wasm_rows.expect("the wasm half renders the card");
    let shape = |rows: &[PropRow]| {
        rows.iter()
            .map(|row| {
                (
                    row.name.clone(),
                    row.value.clone(),
                    row.kind.clone(),
                    row.editable,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(&wasm_rows),
        shape(&cpython_rows),
        "the settings card agrees"
    );
    let cpython_refusal = cpython_refusal.expect("CPython answers the write");
    let wasm_refusal = wasm_refusal.expect("the wasm half answers the write");
    assert!(!cpython_refusal.ok, "{cpython_refusal:?}");
    assert!(!wasm_refusal.ok, "{wasm_refusal:?}");
    assert!(
        cpython_refusal.error.contains("read-only"),
        "{}",
        cpython_refusal.error
    );
    assert_eq!(wasm_refusal.error, cpython_refusal.error);
}
