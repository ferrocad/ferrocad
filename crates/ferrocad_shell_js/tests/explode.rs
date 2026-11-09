//! The Explode package in `mods/`, loaded the way the app loads it.
//!
//! This is the first addon that *needed* the placement change: it reads each object's
//! `Placement` as text, rewrites the three base numbers and hands the rotation back
//! untouched. Collapse restores the string it replaced, so it is exact rather than a
//! division that drifts.

#![cfg(feature = "desktop")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_js::JsScriptHost;
use ferrocad_shell_js::desktop::EnginePage;

/// A document API that answers a canned document and records every call.
struct Scripted {
    document: String,
    requests: Mutex<Vec<serde_json::Value>>,
}

impl Scripted {
    fn new(document: &str) -> Self {
        Self {
            document: document.to_string(),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<serde_json::Value> {
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn writes(&self) -> Vec<serde_json::Value> {
        self.requests()
            .into_iter()
            .filter(|request| request["call"] == "set")
            .collect()
    }
}

impl DocumentApi for Scripted {
    fn call(&self, request: &str) -> String {
        let request: serde_json::Value = serde_json::from_str(request).expect("JSON in");
        let document = request["call"] == "document";
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(request);
        if document {
            self.document.clone()
        } else {
            r#"{"ok":true}"#.to_string()
        }
    }
}

/// A document with one object that has a placement (and a rotation) and one that does not.
const SAMPLE: &str = r#"{"ok":true,"result":{"name":"Sample","objects":[
    {"name":"Box","label":"Box","type_id":"Part::Box","entity":1,
     "properties":[{"name":"Placement","value":"(10.0, 0.0, 0.0; 0.0, 0.0, 1.0; 90.0)"}]},
    {"name":"Note","label":"Note","type_id":"App::FeaturePython","entity":2,
     "properties":[{"name":"Text","value":"hello"}]}
]}}"#;

/// A document where nothing has a placement.
const EMPTY: &str = r#"{"ok":true,"result":{"name":"Empty","objects":[
    {"name":"Note","label":"Note","type_id":"App::FeaturePython","entity":1,
     "properties":[{"name":"Text","value":"hello"}]}
]}}"#;

fn package() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/Explode/InitGui.js")
}

fn host(doorway: &Arc<Scripted>) -> JsScriptHost {
    let page = Arc::new(EnginePage::new(vec![package()]).expect("the package boots"));
    let api: Arc<dyn DocumentApi> = doorway.clone();
    JsScriptHost::with_kernel(page, api)
}

#[test]
fn exploding_moves_only_the_objects_that_have_a_placement() {
    let doorway = Arc::new(Scripted::new(SAMPLE));
    let host = host(&doorway);

    let ids: Vec<String> = host.commands().into_iter().map(|row| row.id).collect();
    assert!(ids.iter().any(|id| id == "Explode_Out"), "{ids:?}");
    assert!(ids.iter().any(|id| id == "Explode_In"), "{ids:?}");

    assert!(host.run_command("Explode_Out").expect("ours").ok);

    let writes = doorway.writes();
    assert_eq!(writes.len(), 1, "one movable object: {writes:?}");
    assert_eq!(writes[0]["object"], "Box");
    assert_eq!(writes[0]["property"], "Placement");
    // One object of one sits at angle 0, so it moves 40 mm along +X, and the rotation
    // tail is handed back exactly as it was read.
    assert_eq!(writes[0]["value"], "(50, 0, 0; 0.0, 0.0, 1.0; 90.0)");
}

#[test]
fn collapse_restores_the_exact_placement() {
    let doorway = Arc::new(Scripted::new(SAMPLE));
    let host = host(&doorway);

    // Twice: the second explode keeps the first original, so collapse still returns to
    // the document's own value rather than to an exploded one.
    assert!(host.run_command("Explode_Out").expect("ours").ok);
    assert!(host.run_command("Explode_Out").expect("ours").ok);
    assert!(host.run_command("Explode_In").expect("ours").ok);

    let writes = doorway.writes();
    assert_eq!(writes.len(), 3, "{writes:?}");
    assert_eq!(
        writes.last().expect("a restore")["value"],
        "(10.0, 0.0, 0.0; 0.0, 0.0, 1.0; 90.0)"
    );
}

#[test]
fn exploding_nothing_is_not_a_success() {
    let doorway = Arc::new(Scripted::new(EMPTY));
    let host = host(&doorway);

    assert!(!host.run_command("Explode_Out").expect("ours").ok);
    assert!(doorway.writes().is_empty(), "{:?}", doorway.writes());
}
