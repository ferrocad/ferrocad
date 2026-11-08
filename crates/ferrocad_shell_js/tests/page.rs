//! The JS half without a browser: the page is a fake, the protocol is real.
//!
//! `ferrocad_shell_js` used to be `#![cfg(target_family = "wasm")]`, so nothing in it
//! could be tested by `cargo test`. The [`Page`] trait is the seam that changes that:
//! the browser supplies `WebPage`, and these tests supply a struct whose answers they
//! write — which is also the shape a desktop engine has to fill.

use std::sync::{Arc, Mutex};

use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::events::{DocumentEvent, DocumentEventSink};
use ferrocad_shell_js::{JsScriptHost, Page, PageEvents, available};

/// A page whose every answer the test writes.
struct FakePage {
    active: bool,
    commands: String,
    evaluate: Mutex<Result<String, String>>,
    run: Mutex<Result<bool, String>>,
    /// What `stageMeshes` answers.
    stage: Mutex<Result<String, String>>,
    /// The `have` argument of each `stageMeshes` call, as it arrived.
    stage_have: Mutex<Vec<String>>,
    /// The ids `runCommand` was asked to run.
    ran: Mutex<Vec<String>>,
    /// The `(id, payload)` of each document event.
    events: Mutex<Vec<(String, String)>>,
    /// The doorway the page was handed, if it was.
    kernel: Mutex<Option<Arc<dyn Fn(String) -> String + Send + Sync>>>,
}

impl Default for FakePage {
    fn default() -> Self {
        Self {
            active: false,
            // A page with no commands is `[]`, not "" — the empty string is a parse
            // error, and this fake should not log one.
            commands: "[]".to_string(),
            evaluate: Mutex::new(Ok(String::new())),
            run: Mutex::new(Ok(true)),
            stage: Mutex::new(Err("the page has no stageMeshes hook".to_string())),
            stage_have: Mutex::new(Vec::new()),
            ran: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            kernel: Mutex::new(None),
        }
    }
}

impl Page for FakePage {
    fn active(&self) -> Result<bool, String> {
        Ok(self.active)
    }

    fn commands(&self) -> Result<String, String> {
        Ok(self.commands.clone())
    }

    fn evaluate(&self, _code: &str) -> Result<String, String> {
        self.evaluate.lock().unwrap().clone()
    }

    fn run_command(&self, id: &str) -> Result<bool, String> {
        self.ran.lock().unwrap().push(id.to_string());
        self.run.lock().unwrap().clone()
    }

    fn stage_meshes(&self, have: &str) -> Result<String, String> {
        self.stage_have.lock().unwrap().push(have.to_string());
        self.stage.lock().unwrap().clone()
    }

    fn on_document_event(&self, id: &str, payload: &str) -> Result<(), String> {
        self.events
            .lock()
            .unwrap()
            .push((id.to_string(), payload.to_string()));
        Ok(())
    }

    fn install_kernel(
        &self,
        call: Arc<dyn Fn(String) -> String + Send + Sync>,
    ) -> Result<(), String> {
        *self.kernel.lock().unwrap() = Some(call);
        Ok(())
    }
}

/// The host's document, as the doorway sees it.
#[derive(Default)]
struct FakeDocument {
    requests: Mutex<Vec<String>>,
}

impl DocumentApi for FakeDocument {
    fn call(&self, request: &str) -> String {
        self.requests.lock().unwrap().push(request.to_string());
        format!("{{\"ok\":true,\"request\":{request}}}")
    }
}

fn host(page: &Arc<FakePage>) -> JsScriptHost {
    JsScriptHost::new(erased(page))
}

/// Unsize a handle: the host takes trait objects, the tests keep the concrete type
/// so they can read what the page was told.
fn erased(page: &Arc<FakePage>) -> Arc<dyn Page> {
    let typed: Arc<FakePage> = Arc::clone(page);
    typed
}

fn erased_document(document: &Arc<FakeDocument>) -> Arc<dyn DocumentApi> {
    let typed: Arc<FakeDocument> = Arc::clone(document);
    typed
}

#[test]
fn a_page_without_its_module_is_not_available() {
    assert!(!available(&FakePage::default()));
    assert!(available(&FakePage {
        active: true,
        ..FakePage::default()
    }));
}

#[test]
fn the_page_commands_become_palette_rows() {
    let page = Arc::new(FakePage {
        commands: concat!(
            r#"[{"id":"Toy_Slab","label":"Create Slab","tooltip":"Add a slab"},"#,
            r#"{"id":"Toy_Bare"}]"#
        )
        .to_string(),
        ..FakePage::default()
    });
    let rows = host(&page).commands();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "Toy_Slab");
    assert_eq!(rows[0].menu, "Create Slab");
    assert_eq!(rows[0].tooltip, "Add a slab");
    // A page that gives no label is listed under its id.
    assert_eq!(rows[1].menu, "Toy_Bare");
    // The palette row is the shell's shape regardless of what the page sent.
    assert_eq!(rows[0].group, "JS");
    assert_eq!(rows[0].surface, "hidden");
    assert!(rows[0].enabled);
}

#[test]
fn a_broken_command_list_is_no_commands() {
    let page = Arc::new(FakePage {
        commands: "not json".to_string(),
        ..FakePage::default()
    });
    assert!(host(&page).commands().is_empty());
}

#[test]
fn an_id_the_page_did_not_list_is_not_mine() {
    let page = Arc::new(FakePage {
        commands: r#"[{"id":"Toy_Slab"}]"#.to_string(),
        ..FakePage::default()
    });
    let host = host(&page);

    assert!(host.run_command("Toy_Nope").is_none());
    assert!(
        page.ran.lock().unwrap().is_empty(),
        "an unlisted id must not reach the page"
    );
}

#[test]
fn a_listed_command_asks_the_page_and_reports_its_answer() {
    let page = Arc::new(FakePage {
        commands: r#"[{"id":"Toy_Slab"}]"#.to_string(),
        ..FakePage::default()
    });
    let host = host(&page);

    let ran = host.run_command("Toy_Slab").expect("a listed command");
    assert!(ran.ok);
    assert!(ran.status.contains("Toy_Slab"));
    assert_eq!(page.ran.lock().unwrap().as_slice(), ["Toy_Slab"]);

    *page.run.lock().unwrap() = Ok(false);
    let refused = host.run_command("Toy_Slab").expect("still listed");
    assert!(!refused.ok);
    assert!(refused.error.contains("refused"));

    *page.run.lock().unwrap() = Err("page threw".to_string());
    let threw = host.run_command("Toy_Slab").expect("still listed");
    assert!(!threw.ok);
    assert!(threw.error.contains("page threw"), "{}", threw.error);
}

#[test]
fn a_throw_is_a_result_not_a_panic() {
    let page = Arc::new(FakePage::default());
    *page.evaluate.lock().unwrap() = Ok("42".to_string());
    let ok = host(&page).evaluate("6 * 7").unwrap();
    assert!(ok.ok);
    assert_eq!(ok.output, "42");

    *page.evaluate.lock().unwrap() = Err("ReferenceError: x is not defined".to_string());
    let bad = host(&page).evaluate("x").unwrap();
    assert!(!bad.ok);
    assert_eq!(bad.output, "ReferenceError: x is not defined");
}

#[test]
fn the_display_hook_is_silent_until_it_works() {
    let page = Arc::new(FakePage::default());
    let host = host(&page);

    // A page that never answered is not a broken hook: the document's rows stand.
    assert!(host.stage_meshes(&[]).is_none());

    // Once it answers, it is the display hook — and its `have` argument is the
    // document's own rows, JSON-encoded.
    *page.stage.lock().unwrap() = Ok("[]".to_string());
    assert!(matches!(
        host.stage_meshes(&["Box", "Fillet"]),
        Some(Ok(ref meshes)) if meshes.is_empty()
    ));
    assert_eq!(
        page.stage_have.lock().unwrap().last().unwrap(),
        r#"["Box","Fillet"]"#
    );

    // From then on a failure is reported rather than quietly dropped.
    *page.stage.lock().unwrap() = Err("boom".to_string());
    match host.stage_meshes(&[]) {
        Some(Err(error)) => assert!(error.contains("boom"), "{error}"),
        _ => panic!("a working hook that breaks must report"),
    }
}

#[test]
fn the_doorway_reaches_the_hosts_document() {
    let page = Arc::new(FakePage::default());
    let document = Arc::new(FakeDocument::default());
    let _host = JsScriptHost::with_kernel(erased(&page), erased_document(&document));

    let call = page
        .kernel
        .lock()
        .unwrap()
        .clone()
        .expect("the page took the document");
    let answer = call(r#"{"op":"commands"}"#.to_string());

    assert!(answer.contains("\"ok\":true"), "{answer}");
    assert_eq!(
        document.requests.lock().unwrap().as_slice(),
        [r#"{"op":"commands"}"#]
    );
}

#[test]
fn a_page_that_refuses_the_doorway_still_works() {
    /// A page that registers commands and takes no document.
    struct CommandsOnly;

    impl Page for CommandsOnly {
        fn active(&self) -> Result<bool, String> {
            Ok(true)
        }
        fn commands(&self) -> Result<String, String> {
            Ok(r#"[{"id":"Toy_Slab"}]"#.to_string())
        }
        fn evaluate(&self, _code: &str) -> Result<String, String> {
            Ok(String::new())
        }
        fn run_command(&self, _id: &str) -> Result<bool, String> {
            Ok(true)
        }
    }

    let document = Arc::new(FakeDocument::default());
    let host = JsScriptHost::with_kernel(Arc::new(CommandsOnly), erased_document(&document));

    assert_eq!(host.commands().len(), 1);
}

#[test]
fn a_document_event_reaches_the_page_as_json() {
    let page = Arc::new(FakePage::default());
    let sink = PageEvents::new(erased(&page));
    let event = DocumentEvent::edited("Move", &["Box".to_string()]);

    sink.document_event(&event);
    // The host's own hook is the same route: one page, one encoding.
    host(&page).on_document_event(&event);

    let events = page.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    for (id, payload) in events.iter() {
        assert_eq!(id, "document.edited");
        assert!(payload.contains("\"transaction\":\"Move\""), "{payload}");
        assert!(payload.contains("\"Box\""), "{payload}");
    }
}
