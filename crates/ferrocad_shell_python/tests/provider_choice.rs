//! A provider choice survives a restart, for a backend only the page lists.
//!
//! This is the end-to-end shape of the bug: pick *Axis Cross (JS)*, restart, and the
//! native cross was back. The store is the composition's (the desktop's `FreeCAD`
//! parameter file, `localStorage` in the browser), the choice is written when the
//! selector switches, and it is re-applied when the script half boots — the point where
//! `python/` has joined `sys.path` and the mods have registered their providers.

#![cfg(feature = "js")]

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_python::{Runtime, compose_all};

/// The axis providers as `(backend, active)`, the way the popover reads them.
fn axis_providers(backend: &dyn AppBackend) -> Vec<(String, bool)> {
    backend
        .provider_features()
        .expect("the registry answers")
        .into_iter()
        .find(|feature| feature.feature == "axis")
        .map(|feature| {
            feature
                .providers
                .into_iter()
                .map(|provider| (provider.backend, provider.active))
                .collect()
        })
        .unwrap_or_default()
}

/// The repository's `mods/` directory, so the page loads the real axis workbench.
fn mods_dir() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../mods")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_js_provider_choice_survives_a_restart() {
    let params = std::env::temp_dir().join("ferrocad-provider-choice.json");
    let _ = std::fs::remove_file(&params);
    // SAFETY: this test binary holds one test, so nothing else reads the environment
    // while these are set. The parameter file is redirected to a temporary path before
    // the interpreter boots, so a test run can never touch the user's own store.
    unsafe {
        std::env::set_var("FERROCAD_PARAM_FILE", &params);
        std::env::set_var("FERROCAD_MODS_PATH", mods_dir());
    }

    let mut backend = compose_all(&[Runtime::Cpython, Runtime::Js], &[]).expect("composes");
    backend.bootstrap().expect("the halves boot");
    assert!(
        !axis_providers(backend.as_ref())
            .iter()
            .any(|(backend, active)| backend == "js" && *active),
        "the JS cross starts unloaded"
    );

    let switched = backend
        .set_provider("axis", "js")
        .expect("the switch answers");
    assert!(switched.ok, "{switched:?}");
    assert!(
        axis_providers(backend.as_ref())
            .iter()
            .any(|(backend, active)| backend == "js" && *active),
        "the page's backend is the one loaded"
    );
    let store = std::fs::read_to_string(&params).expect("the choice was written to the store");
    assert!(
        store.contains("\"axis\"") && store.contains("\"js\""),
        "the store holds the choice: {store}"
    );

    // A restart: a new composition reads the store back and loads the page's backend
    // without a click.
    drop(backend);
    let mut restarted = compose_all(&[Runtime::Cpython, Runtime::Js], &[]).expect("recomposes");
    restarted.bootstrap().expect("the halves boot again");
    assert!(
        axis_providers(restarted.as_ref())
            .iter()
            .any(|(backend, active)| backend == "js" && *active),
        "the stored backend came back: {:?}",
        axis_providers(restarted.as_ref())
    );
}
