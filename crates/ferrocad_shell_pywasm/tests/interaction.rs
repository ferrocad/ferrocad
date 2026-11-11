//! B18a steps 6 and 7: the wasm half grows interaction and the model tree.
//!
//! Both flags ride the *shared* sources, not a port. `python/FerroCAD/__init__.py`
//! already owns `register_interaction_provider`/`register_manipulator_provider` and
//! `python/ferrocad_shell/__init__.py` already builds the synthetic `Settings` root,
//! so the flags only decide whether the document half defers to them here.
//!
//! What the flags must *not* claim is the edition's datum hook: `add_datum_plane` is
//! exported by the app script (`python/ferrocad_parametric`), and this runtime installs
//! no app script, so the answer stays the document half's (`None`).

use ferrocad_core::Vector3;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::PywasmScriptHost;
use ferrocad_stage::InteractionCommit;

/// Run `f` against a freshly booted interpreter on a large stack (RustPython's compiler
/// recurses; see `properties.rs` for the same requirement).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// Run `code` in the console and return what it printed, or panic quoting the error.
fn evaluate(host: &PywasmScriptHost, code: &str) -> String {
    let raw = host.call("evaluate", &[code]).expect("the console runs");
    let answer: serde_json::Value = serde_json::from_str(&raw).expect("evaluate answers JSON");
    assert_eq!(answer["ok"], true, "{answer}");
    answer["output"].as_str().unwrap_or_default().to_string()
}

/// A provider a mod registers is the one that applies its commit; the sample document's
/// `Box` is the object it receives.
#[test]
fn a_mod_provider_owns_the_commit() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCAD

seen = []

def provider(obj, manipulator, action, element, values):
    if manipulator != 'push_pull':
        return None
    seen.append([getattr(obj, 'Name', None), action, element, values])
    return {'ok': True, 'error': ''}

FerroCAD.register_interaction_provider(provider)
"#,
        );

        let mut commit = InteractionCommit::new("push_pull", 0, "extrude");
        commit.values = vec![4.0];
        let result = host
            .apply_interaction(&commit, "Box")
            .expect("the provider owns this commit");
        assert!(result.ok, "the provider's answer: {result:?}");

        let seen = evaluate(host, "print(seen[-1])");
        assert!(seen.contains("Box"), "the object was resolved: {seen}");
        assert!(seen.contains("extrude"), "the action crossed: {seen}");
        assert!(seen.contains("4.0"), "the values crossed: {seen}");
    });
}

/// Nobody registered a provider, so the commit is not this half's: `None` is what sends
/// it to the document half (the generic gizmo in a build with no edition mod).
#[test]
fn without_a_provider_the_document_half_answers() {
    with_host(|host| {
        let commit = InteractionCommit::new("transform", 0, "apply");
        assert!(
            host.apply_interaction(&commit, "Box").is_none(),
            "no provider owns the commit"
        );
        assert_eq!(
            host.manipulator_specs("Box", None)
                .unwrap_or_default()
                .len(),
            0,
            "and it declares no handles"
        );
    });
}

/// A declared handle reaches the shell in provider order, which is what the viewport
/// draws for the armed selection.
///
/// The shell passes the **object's name**, not a document object (see
/// `ferrocad_shell.manipulator_specs`); a handle is declarative data, so the name is what
/// the viewport resolves it against.
#[test]
fn a_manipulator_provider_declares_handles() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCAD

def provider(obj, element):
    return [{'kind': 'translate', 'label': 'Move', 'object': obj}]

FerroCAD.register_manipulator_provider(provider)
"#,
        );

        let specs = host
            .manipulator_specs("Box", None)
            .expect("the registry answers");
        assert_eq!(specs.len(), 1, "one declared handle: {specs:?}");
        assert_eq!(specs[0]["kind"], "translate");
        assert_eq!(specs[0]["object"], "Box", "the resolved object is passed");
    });
}

/// The tree appends the synthetic `Settings` root once a provider registers a feature —
/// and only then: with no provider the shared `model_tree` lists documents alone, which
/// the document half already builds.
#[test]
fn the_tree_grows_the_settings_root_with_a_provider() {
    with_host(|host| {
        assert!(
            host.extra_tree_nodes().is_empty(),
            "no provider is registered, so the script owns no node"
        );

        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('grid', 'demo', 'Demo Grid', settings=[
    {'name': 'minor', 'kind': 'int', 'default': 10},
])
"#,
        );

        let nodes = host.extra_tree_nodes();
        let settings = nodes
            .iter()
            .find(|node| node.name == "Settings")
            .unwrap_or_else(|| panic!("the Settings root: {nodes:?}"));
        assert!(!settings.active, "Settings is not a document");
        assert_eq!(
            settings
                .objects
                .iter()
                .map(|object| object.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Settings/grid"],
            "one node per registered feature"
        );
    });
}

/// `add_datum_plane` belongs to the edition's app script, which this runtime does not
/// install: even with `interaction` on, the hook is absent and the document half answers
/// instead of a missing-attribute error being reported as a failed plane.
#[test]
fn the_datum_hook_stays_the_document_halves() {
    with_host(|host| {
        assert!(
            host.add_datum_plane(
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .is_none(),
            "no app script, no datum geometry"
        );
    });
}
