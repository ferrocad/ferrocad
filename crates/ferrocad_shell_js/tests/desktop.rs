//! The desktop page: a workbench module, an embedded QuickJS, no browser.
//!
//! The module here is written the way `www/js-demo.js` is — it defines
//! `window.__ferrocadJs` from `install()` — so what these tests prove is that the
//! *same* script runs on the desktop, over the same document API, through the same
//! `JsScriptHost`.

#![cfg(feature = "desktop")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::events::DocumentEvent;
use ferrocad_shell_js::desktop::{EnginePage, js_modules};
use ferrocad_shell_js::{JsScriptHost, Page};

/// A workbench that registers two commands, answers the console, and reports what the
/// document told it.
const WORKBENCH: &str = r#"
const commands = [
  { id: "Toy_JS", label: "Create Slab", tooltip: "Add a slab" },
  { id: "Toy_Noop" },
];

const seen = [];

export function install() {
  // `window` is the bootstrap's alias for the global object, exactly as a page has it.
  window.__ferrocadJs = {
    active: () => true,
    commands: () => JSON.stringify(commands),
    evaluate(code) {
      if (code === "api") {
        return window.ferrocad.call('{"op":"ping"}');
      }
      // The ordering proof for a queued event: this answer is produced after it.
      if (code === "events") {
        return seen.join("|");
      }
      return `js: ${code}`;
    },
    runCommand: (id) => id === "Toy_JS",
    stageMeshes: (have) => JSON.stringify([]),
    onDocumentEvent(id, payload) {
      seen.push(`${id}:${payload}`);
    },
  };
}
"#;

/// A workbench directory laid out the way the Python side lays out `InitGui.py`.
fn workbench(dir: &str, source: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let mods = temp.path().join("mods");
    let entry = mods.join(dir).join("InitGui.js");
    std::fs::create_dir_all(entry.parent().unwrap()).expect("the workbench directory");
    std::fs::write(&entry, source).expect("the workbench module");
    (temp, entry)
}

/// The host's document, as the document API sees it.
#[derive(Default)]
struct FakeDocument {
    requests: Mutex<Vec<String>>,
}

impl DocumentApi for FakeDocument {
    fn call(&self, request: &str) -> String {
        self.requests.lock().unwrap().push(request.to_string());
        r#"{"ok":true,"op":"ping"}"#.to_string()
    }
}

fn page(entry: PathBuf) -> Arc<dyn Page> {
    Arc::new(EnginePage::new(vec![entry]).expect("the workbench boots"))
}

#[test]
fn a_workbench_module_registers_the_pages_commands() {
    let (_temp, entry) = workbench("Toy", WORKBENCH);
    let host = JsScriptHost::new(page(entry));

    let rows = host.commands();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "Toy_JS");
    assert_eq!(rows[0].menu, "Create Slab");
    assert_eq!(rows[1].menu, "Toy_Noop", "a row with no label uses its id");
}

#[test]
fn the_console_runs_in_the_module() {
    let (_temp, entry) = workbench("Toy", WORKBENCH);
    let host = JsScriptHost::new(page(entry));

    let answer = host.evaluate("6 * 7").expect("the console runs");
    assert!(answer.ok);
    assert_eq!(answer.output, "js: 6 * 7");

    let ran = host.run_command("Toy_JS").expect("a listed command");
    assert!(ran.ok);
    let refused = host.run_command("Toy_Noop").expect("still listed");
    assert!(!refused.ok, "the module answers false for its second id");
}

#[test]
fn the_document_api_is_ferrocad_call() {
    let (_temp, entry) = workbench("Toy", WORKBENCH);
    let document = Arc::new(FakeDocument::default());
    let host = JsScriptHost::with_kernel(page(entry), Arc::clone(&document) as Arc<dyn DocumentApi>);

    let answer = host.evaluate("api").expect("the console runs");
    assert!(answer.ok, "{}", answer.output);
    assert!(answer.output.contains("\"op\":\"ping\""), "{}", answer.output);
    assert_eq!(
        document.requests.lock().unwrap().as_slice(),
        [r#"{"op":"ping"}"#],
        "the module's call reached the host's document"
    );
}

#[test]
fn a_document_event_reaches_the_module() {
    let (_temp, entry) = workbench("Toy", WORKBENCH);
    let host = JsScriptHost::new(page(entry));

    // The event is queued, so the console line behind it is the ordering proof: the
    // engine answers jobs in order, and this one only runs once `events` has been set.
    host.on_document_event(&DocumentEvent::edited("Move", &["Box".to_string()]));
    let answer = host.evaluate("events").expect("the console runs");

    assert!(answer.ok, "{}", answer.output);
    assert!(answer.output.contains("document.edited"), "{}", answer.output);
    assert!(answer.output.contains("\"transaction\":\"Move\""), "{}", answer.output);
}

#[test]
fn a_module_that_throws_names_the_file() {
    let (_temp, entry) = workbench(
        "Broken",
        r#"export function install() { throw new Error("no commands here"); }"#,
    );

    let error = EnginePage::new(vec![entry]).err().expect("the boot fails");
    assert!(error.contains("InitGui.js"), "{error}");
    assert!(error.contains("no commands here"), "{error}");
}

#[test]
fn a_module_without_install_is_an_error() {
    let (_temp, entry) = workbench("Empty", "export const nothing = 1;");

    let error = EnginePage::new(vec![entry]).err().expect("the boot fails");
    assert!(error.contains("install"), "{error}");
}

#[test]
fn a_page_with_no_workbench_is_not_available() {
    let page = Arc::new(EnginePage::empty().expect("an empty engine"));
    assert!(!ferrocad_shell_js::available(&*page));

    let host = JsScriptHost::new(page);
    assert!(host.commands().is_empty());
}

#[test]
fn the_workbenches_are_the_ones_with_an_entry_file_in_name_order() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let mods = temp.path().join("mods");
    for name in ["Zeta", "Alpha"] {
        let entry = mods.join(name).join("InitGui.js");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, WORKBENCH).unwrap();
    }
    // A workbench with no entry file is not a JS workbench.
    std::fs::create_dir_all(mods.join("PythonOnly")).unwrap();
    std::fs::write(mods.join("PythonOnly").join("InitGui.py"), "").unwrap();

    let found = js_modules(&[mods]);
    let names: Vec<String> = found
        .iter()
        .map(|path| path.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Alpha", "Zeta"]);

    // And the page loads them in that order.
    let page = EnginePage::new(found.clone()).expect("both boot");
    assert_eq!(page.modules().len(), 2);
}
