//! The spring bundle the desktop ships, run by the desktop's own wasm host.
//!
//! `cargo test -p ferrocad_shell_wasm` cannot build the bundle — it is a wasm artifact of
//! another crate — so this file reads `FERROCAD_SPRING_WASM` and skips when it is unset.
//! CI builds `ferrocad_addon_spring` for `wasm32-unknown-unknown` and sets it, which is
//! what turns "the bundle a mod directory carries runs in the app" into a checked claim.
//! The unit tests in the crate drive the same logic over the same `DocApi`; this one
//! drives the compiled artifact through the sandbox.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_wasm::{Capability, WasmAddonHost};

/// One test at a time: the registry, and the document the API serves, are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// The bundle, when the environment names one.
fn bundle() -> Option<PathBuf> {
    match std::env::var("FERROCAD_SPRING_WASM") {
        Ok(path) => Some(PathBuf::from(path)),
        Err(_) => {
            eprintln!("FERROCAD_SPRING_WASM is unset: skipping the spring-bundle test");
            None
        }
    }
}

/// A copy of the bundle with `addon.json` beside it, because the spring creates objects.
fn with_manifest(source: &PathBuf) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("a temporary directory");
    std::fs::copy(source, temp.path().join("addon.wasm")).expect("the bundle copy");
    std::fs::write(
        temp.path().join("addon.json"),
        r#"{"document": "read-write"}"#,
    )
    .expect("the manifest");
    temp
}

/// The real document API over a clean registry with one empty document: a bundle cannot create
/// an object with no document open, and that failure would read as a broken ABI.
fn host(path: &PathBuf) -> WasmAddonHost {
    {
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
        registry.new_document(Some("Bundle"));
    }
    let document: Arc<dyn DocumentApi> = Arc::new(ferrocad_doc_api::DocApi::new());
    WasmAddonHost::open(path, document).expect("the spring bundle instantiates")
}

#[test]
fn the_spring_bundle_creates_its_parameters_and_draws_the_helix() {
    let Some(source) = bundle() else {
        return;
    };
    let _serial = serial();
    let temp = with_manifest(&source);
    let host = host(&temp.path().join("addon.wasm"));
    assert_eq!(host.capability(), Capability::Write);

    let ids: Vec<String> = host.commands().into_iter().map(|row| row.id).collect();
    assert!(ids.iter().any(|id| id == "Spring_Insert"), "{ids:?}");
    assert!(ids.iter().any(|id| id == "Spring_Tighten"), "{ids:?}");

    // Nothing to draw before the object exists.
    match host.stage_meshes(&[]) {
        Some(Ok(meshes)) => assert!(meshes.is_empty(), "no spring yet"),
        _ => panic!("the fixture has a display hook"),
    }

    let created = host
        .run_command("Spring_Insert")
        .expect("the bundle's own command");
    assert!(created.ok, "{:?}", created.error);

    // The hook draws what the document cannot: the object is parameters only.
    match host.stage_meshes(&[]) {
        Some(Ok(meshes)) => {
            assert_eq!(meshes.len(), 1, "one spring");
            let mesh = &meshes[0];
            assert_eq!(mesh.object, "Spring");
            assert!(!mesh.positions.is_empty(), "the helix has points");
            assert!(!mesh.edges.is_empty(), "the helix has segments");
            assert_eq!(mesh.display.as_deref(), Some("wireframe"));
        }
        Some(Err(error)) => panic!("the hook failed: {error}"),
        None => panic!("the bundle has a display hook"),
    }

    // Tightening reads the `Pitch` the insert declared, so it also proves the object and
    // its parameters crossed the document API, not just the ABI.
    let tightened = host
        .run_command("Spring_Tighten")
        .expect("the bundle's own command");
    assert!(tightened.ok, "{:?}", tightened.error);
}
