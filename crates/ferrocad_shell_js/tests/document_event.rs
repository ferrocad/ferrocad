//! A document event emitted from inside a document API call.
//!
//! This is the normal path, not an edge case: a package's command writes through
//! `ferrocad.call`, the document answers, and the host announces the edit back to the
//! page that made it. The announcement is *queued* for exactly this reason — answering
//! it there would re-enter the QuickJS runtime — but queueing must not wait for the
//! call that is still in flight, or the page and the caller wait for each other.
//!
//! The regression this pins: `RoundCommand` held the engine lock while waiting for its
//! answer, and `fire` (what queues an event) needed the same lock. Every addon command
//! that wrote anything hung the window.

#![cfg(feature = "desktop")]

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_js::desktop::EnginePage;
use ferrocad_shell_js::{JsScriptHost, Page};

/// A package whose one command writes through the document API.
const MODULE: &str = r#"
export function install() {
  window.__ferrocadJs = {
    active: () => true,
    commands: () => JSON.stringify([{ id: "Echo_Write", label: "Write" }]),
    evaluate: (code) => `js: ${code}`,
    runCommand(id) {
      if (id !== "Echo_Write") return false;
      return JSON.parse(window.ferrocad.call('{"call":"document"}')).ok === true;
    },
    stageMeshes: () => "[]",
  };
}
"#;

/// A document API that announces an edit from inside the call, as `DocApi` does.
struct Announce(Arc<EnginePage>);

impl DocumentApi for Announce {
    fn call(&self, _request: &str) -> String {
        // The sink `js_page()` installs is `PageEvents`, which calls this on the page.
        let _ = self.0.on_document_event("document.edited", "{}");
        r#"{"ok":true}"#.to_string()
    }
}

#[test]
fn a_write_that_announces_itself_does_not_wait_for_itself() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let entry = temp.path().join("InitGui.js");
    std::fs::write(&entry, MODULE).expect("the package");

    let engine = Arc::new(EnginePage::new(vec![entry]).expect("the page boots"));
    let page: Arc<dyn Page> = engine.clone();
    let api: Arc<dyn DocumentApi> = Arc::new(Announce(Arc::clone(&engine)));
    let host = JsScriptHost::with_kernel(page, api);

    // The command used to hang here. A watchdog keeps a regression a failing test
    // rather than a stuck suite.
    let (done, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(host.run_command("Echo_Write"));
    });

    let outcome = answer
        .recv_timeout(Duration::from_secs(10))
        .expect("the page answered instead of waiting for itself");
    assert!(outcome.expect("the package's own id").ok, "the command ran");
}
