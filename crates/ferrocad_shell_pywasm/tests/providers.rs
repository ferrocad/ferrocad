//! B18a step 5: the wasm facade's mod provider registry.
//!
//! `bootstrap.py` ports `python/FerroCADGui/__init__.py`'s provider section: registration,
//! the active backend per feature, the switch list the About card and Dev Tools read, the
//! skin channel and the activate/deactivate/refresh callbacks. The host exposes it through
//! `ScriptHost::provider_features`/`set_provider`/`refresh_providers`/`provider_skin`, so
//! these tests drive the unmodified `python/ferrocad_shell` through the shared host.
//!
//! Two divergences are recorded in the facade and asserted here: the selected backend is
//! in memory (no parameter store: B18b), and a declared settings schema is read for its
//! declared defaults only (writes are step 3).

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::types::ProviderFeature;
use ferrocad_shell_pywasm::PywasmScriptHost;

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

/// Run Python and assert it did not raise.
fn evaluate(host: &PywasmScriptHost, code: &str) -> String {
    let result = host.evaluate(code).expect("evaluate runs");
    assert!(result.ok, "{}", result.output);
    result.output
}

/// The feature `feature` among the registry's rows.
fn feature<'a>(features: &'a [ProviderFeature], name: &str) -> &'a ProviderFeature {
    features
        .iter()
        .find(|feature| feature.feature == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} is not registered: {:?}",
                features
                    .iter()
                    .map(|feature| &feature.feature)
                    .collect::<Vec<_>>()
            )
        })
}

/// Registration order, priority and a switch: the manifest list is what Dev Tools and the
/// header popover read, so it is the first thing to pin.
#[test]
fn a_registered_provider_is_listed_in_order_with_the_priority_active() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('demo', 'alpha', 'Alpha')
FerroCADGui.registerProvider('demo', 'beta', 'Beta', priority=5)
"#,
        );

        // The Python side first, so a difference between "not registered" and "the
        // bridge does not list it" is visible.
        let listed = evaluate(host, "FerroCADGui.providers('demo')");
        assert!(listed.contains("'backend': 'alpha'"), "{listed}");

        let features = host.provider_features();
        let demo = feature(&features, "demo");
        assert_eq!(demo.providers.len(), 2, "{demo:?}");
        assert_eq!(demo.providers[0].backend, "alpha");
        assert_eq!(demo.providers[0].label, "Alpha");
        assert!(!demo.providers[0].active, "the higher priority leads");
        assert_eq!(demo.providers[1].backend, "beta");
        assert!(demo.providers[1].active);
        assert_eq!(
            evaluate(host, "FerroCADGui.activeProvider('demo')"),
            "'beta'"
        );

        // A reload replaces the record without moving it.
        evaluate(
            host,
            "FerroCADGui.registerProvider('demo', 'alpha', 'Alpha 2')",
        );
        let features = host.provider_features();
        let demo = feature(&features, "demo");
        assert_eq!(demo.providers[0].label, "Alpha 2");
        assert_eq!(demo.providers[0].backend, "alpha", "same slot, same order");
        assert_eq!(demo.providers.len(), 2);
    });
}

/// The switch runs the outgoing backend's `deactivate` then the incoming `activate`, keeps
/// the choice (in memory), and its `view` claim reaches the shell. An unknown pair is a
/// result, not a crash.
#[test]
fn a_switch_runs_the_callbacks_and_carries_the_view_claim() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FreeCAD, FerroCADGui
FreeCAD.demo_log = []
def alpha_on():
    FreeCAD.demo_log.append('alpha on')
    return {'view': {'grid_overlay': True}, 'status': 'alpha ready'}
def alpha_off():
    FreeCAD.demo_log.append('alpha off')
def beta_on():
    FreeCAD.demo_log.append('beta on')
FerroCADGui.registerProvider('demo', 'alpha', 'Alpha', command='Demo_Show', activate=alpha_on, deactivate=alpha_off, skin='// alpha.wgsl\n')
FerroCADGui.registerProvider('demo', 'beta', 'Beta', activate=beta_on)
"#,
        );

        assert_eq!(
            host.provider_skin("demo").as_deref(),
            Some("// alpha.wgsl"),
            "the active backend's skin is what the host draws"
        );
        assert_eq!(
            feature(&host.provider_features(), "demo").providers[0]
                .command
                .as_deref(),
            Some("Demo_Show")
        );

        let switched = host
            .set_provider("demo", "alpha")
            .expect("the registry answers a switch");
        assert!(switched.ok, "{switched:?}");
        assert!(
            switched
                .status
                .contains("demo: alpha is the active provider"),
            "{}",
            switched.status
        );
        assert!(
            switched.status.contains("alpha ready"),
            "the callback's status is folded in: {}",
            switched.status
        );
        assert_eq!(
            switched.view.and_then(|view| view.grid_overlay),
            Some(true),
            "the activate callback's claim reaches the shell (the bootstrap switch)"
        );

        // Then to the other backend: the outgoing one stops before the incoming starts,
        // and the incoming one has no skin and no claim.
        let switched = host
            .set_provider("demo", "beta")
            .expect("the registry answers a switch");
        assert!(switched.ok, "{switched:?}");
        assert_eq!(
            evaluate(host, "FreeCAD.demo_log"),
            "['alpha on', 'alpha off', 'beta on']",
            "the outgoing backend stops before the incoming one starts"
        );
        assert_eq!(
            host.provider_skin("demo"),
            None,
            "beta supplied no skin: draw the native material"
        );
        assert!(feature(&host.provider_features(), "demo").providers[1].active);

        // And back: alpha's activate runs again, so its skin and claim return.
        let switched = host
            .set_provider("demo", "alpha")
            .expect("the registry answers a switch");
        assert!(switched.ok, "{switched:?}");
        assert_eq!(
            evaluate(host, "FreeCAD.demo_log"),
            "['alpha on', 'alpha off', 'beta on', 'alpha on']"
        );
        assert_eq!(host.provider_skin("demo").as_deref(), Some("// alpha.wgsl"));

        // A pair no backend provides is a result the shell can show.
        let refused = host
            .set_provider("demo", "nope")
            .expect("the bridge answers even a refusal");
        assert!(!refused.ok, "{refused:?}");
        assert!(refused.error.contains("nope"), "{}", refused.error);

        // And a feature nobody registered is not a crash either.
        assert_eq!(host.provider_skin("grid"), None);
        let none = host
            .set_provider("nobody", "x")
            .expect("the bridge answers");
        assert!(!none.ok);
        assert!(none.error.contains("nobody"), "{}", none.error);
    });
}

/// A provider-backed grid summary answers the shell: the active backend, its spacing row
/// (read from the declared schema — there is no store, so the default), and the switch
/// list. This is the row that made a provider-backed header possible.
#[test]
fn a_provider_backed_grid_summary_names_the_provider_and_its_spacing() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('grid', 'demo', 'Demo Grid', settings=[{'name': 'spacing', 'kind': 'quantity', 'unit': 'mm', 'default': 25.0}])
"#,
        );

        let summary = host.grid_summary().expect("the facade answers the summary");
        assert_eq!(summary.backend.as_deref(), Some("demo"));
        assert_eq!(summary.label.as_deref(), Some("Demo Grid"));
        assert_eq!(summary.spacing.as_deref(), Some("25 mm"));
        assert_eq!(summary.providers.len(), 1, "the popover's switch list");
        assert_eq!(summary.providers[0].backend, "demo");
        assert!(summary.providers[0].active);

        // The rows are the schema's, with the declared default, and since step 3 the
        // session store makes them editable.
        let rows = evaluate(host, "FerroCADGui.settings('grid')");
        assert!(rows.contains("'name': 'spacing'"), "{rows}");
        assert!(rows.contains("'value': '25 mm'"), "{rows}");
        assert!(
            rows.contains("'editable': True"),
            "a declared row writes to the session store: {rows}"
        );
    });
}

/// A fresh interpreter has no provider: the closed empty summary is what the document half
/// defers on (`DocumentBackend`'s `names_a_provider`), so it must stay empty rather than
/// inventing one.
#[test]
fn an_empty_registry_claims_nothing() {
    with_host(|host| {
        assert!(host.provider_features().is_empty());
        assert_eq!(host.provider_skin("grid"), None);
        let summary = host.grid_summary().expect("the facade still answers");
        assert_eq!(summary.backend, None);
        assert!(summary.providers.is_empty());
    });
}
