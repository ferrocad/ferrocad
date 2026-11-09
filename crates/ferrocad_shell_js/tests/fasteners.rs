//! The Fasteners package in `mods/`, loaded the way the app loads it.
//!
//! The point is not the five sizes: it is that a package in the repository is a *real*
//! workbench — the host evaluates `InitGui.js`, reads its commands, and a command writes
//! through the document API. A regression in the module contract fails here.

#![cfg(feature = "desktop")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_js::JsScriptHost;
use ferrocad_shell_js::desktop::EnginePage;

/// A document API that records what the package asked for and grants it.
#[derive(Default)]
struct Recorder {
    requests: Mutex<Vec<String>>,
}

impl DocumentApi for Recorder {
    fn call(&self, request: &str) -> String {
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(request.to_string());
        r#"{"ok":true}"#.to_string()
    }
}

/// `mods/Fasteners/InitGui.js`, from this crate's directory.
fn package() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/Fasteners/InitGui.js")
}

#[test]
fn the_fastener_package_registers_and_writes_through_the_document_api() {
    let page = Arc::new(EnginePage::new(vec![package()]).expect("the package boots"));
    let recorder = Arc::new(Recorder::default());
    let api: Arc<dyn DocumentApi> = recorder.clone();
    let host = JsScriptHost::with_kernel(page, api);

    let ids: Vec<String> = host.commands().into_iter().map(|row| row.id).collect();
    assert!(ids.iter().any(|id| id == "Fastener_M6x20"), "{ids:?}");

    assert!(
        host.run_command("Fastener_M6x20")
            .expect("the package's own id")
            .ok,
        "the command ran"
    );

    let requests = recorder
        .requests
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(requests.len(), 1, "{requests:?}");
    let request: serde_json::Value = serde_json::from_str(&requests[0]).expect("JSON");
    assert_eq!(request["call"], "create");
    assert_eq!(request["type"], "Part::Cylinder");
    assert_eq!(request["name"], "ISO4762_M6x20");
    assert_eq!(request["properties"]["Radius"], "3 mm");
    assert_eq!(request["properties"]["Height"], "20 mm");
}

#[test]
fn an_id_the_package_does_not_own_is_not_its() {
    let page = Arc::new(EnginePage::new(vec![package()]).expect("the package boots"));
    let recorder = Arc::new(Recorder::default());
    let api: Arc<dyn DocumentApi> = recorder.clone();
    let host = JsScriptHost::with_kernel(page, api);

    assert!(host.run_command("Somebody_Else").is_none());
    assert!(
        recorder
            .requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    );
}
