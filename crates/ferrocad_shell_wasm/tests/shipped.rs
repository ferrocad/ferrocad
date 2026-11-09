//! The bundle the browser ships, run by the desktop's interpreter.
//!
//! `cargo test -p ferrocad_shell_wasm` cannot build the bundle — it is a wasm artifact of
//! another crate — so this file reads `FERROCAD_ADDON_WASM` and skips when it is unset.
//! CI builds `ferrocad_addon_toy` for `wasm32-unknown-unknown` and sets it, which is what
//! turns "the desktop runs the bundle the browser ships, not a second build of it" into a
//! checked claim. The other suite uses hand-written `wat` fixtures; this one uses the real
//! artifact, with the real `DocApi` behind the document API.

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

/// A copy of the bundle with `addon.json` beside it.
fn with_manifest(source: &PathBuf, document: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("a temporary directory");
    std::fs::copy(source, temp.path().join("addon.wasm")).expect("the bundle copy");
    std::fs::write(
        temp.path().join("addon.json"),
        format!(r#"{{"document": "{document}"}}"#),
    )
    .expect("the manifest");
    temp
}

/// The shipped bundle, when the environment names one.
fn bundle() -> Option<PathBuf> {
    match std::env::var("FERROCAD_ADDON_WASM") {
        Ok(path) => Some(PathBuf::from(path)),
        Err(_) => {
            eprintln!("FERROCAD_ADDON_WASM is unset: skipping the shipped-bundle tests");
            None
        }
    }
}

fn host(path: &PathBuf) -> WasmAddonHost {
    // The real document API, over a clean registry with one empty document: a bundle cannot
    // create an object with no document open, and that failure would read as a broken ABI.
    {
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
        registry.new_document(Some("Shipped"));
    }
    let document: Arc<dyn DocumentApi> = Arc::new(ferrocad_doc_api::DocApi::new());
    WasmAddonHost::open(path, document).expect("the shipped bundle instantiates")
}

#[test]
fn the_shipped_toy_bundle_registers_its_commands() {
    let Some(path) = bundle() else {
        return;
    };
    let _serial = serial();
    let ids: Vec<String> = host(&path).commands().into_iter().map(|row| row.id).collect();

    assert!(ids.iter().any(|id| id == "Toy_Slab"), "{ids:?}");
    assert!(ids.iter().any(|id| id == "Toy_Taller"), "{ids:?}");
}

#[test]
fn the_shipped_bundle_cannot_write_without_a_manifest() {
    let Some(path) = bundle() else {
        return;
    };
    let _serial = serial();
    let host = host(&path);
    assert_eq!(host.capability(), Capability::Read);

    let refused = host.run_command("Toy_Slab").expect("the bundle's own command");
    assert!(!refused.ok, "a read-only bundle cannot create its object");
    assert!(refused.error.contains("read-only"), "{}", refused.error);

    // And nothing was drawn: the hook lists the document's toy objects, and there is none.
    match host.stage_meshes(&[]) {
        Some(Ok(meshes)) => assert!(meshes.is_empty(), "no object was created"),
        _ => panic!("the fixture has a display hook"),
    }
}

#[test]
fn the_shipped_bundle_creates_and_draws_through_the_document_api() {
    let Some(source) = bundle() else {
        return;
    };
    let _serial = serial();
    // The toy workbench creates and edits, so it asks for writes — and the manifest is
    // what grants them.
    let bundle = with_manifest(&source, "read-write");
    let host = host(&bundle.path().join("addon.wasm"));
    assert_eq!(host.capability(), Capability::Write);
    assert_eq!(
        host.document_grant(),
        ferrocad_shell_api::DocumentGrant::ReadWrite
    );

    // The command creates its object through `ferrocad.call`; the document half is the
    // one that answers, so a failure here is the document API, not the ABI.
    let ran = host.run_command("Toy_Slab").expect("the bundle's own command");
    assert!(ran.ok, "{:?}", ran.error);

    // And the display hook draws what it made.
    match host.stage_meshes(&[]) {
        Some(Ok(meshes)) => {
            assert!(!meshes.is_empty(), "the slab has a mesh");
            assert!(
                meshes.iter().any(|mesh| !mesh.positions.is_empty()),
                "the mesh has vertices"
            );
        }
        Some(Err(error)) => panic!("the hook failed: {error}"),
        None => panic!("the bundle has a display hook"),
    }
}
