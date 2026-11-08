//! A wasm workbench, hosted and sandboxed, without a browser.
//!
//! The fixture is written as `wat` so the ABI is visible: the bundle imports the doorway,
//! exports the five entry points the loader calls, and packs every answer as
//! `len << 32 | ptr` in its own memory.

use std::sync::{Arc, Mutex};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::DocumentEvent;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::types::EvalResult;
use ferrocad_shell_api::HostChain;
use ferrocad_shell_wasm::{Capability, WasmAddonHost, announce_to, chain, packs, read_only};

/// The command list must be exactly 64 bytes: the bundle answers with a static buffer.
const COMMANDS: &str =
    r#"[{"id":"Toy_Wasm","label":"Create Slab","tooltip":"Add a slab"}]"#;

fn fixture() -> String {
    fixture_sending(r#"{"op":"ping"}"#)
}

/// The same module, sending `request` to the doorway whenever a command runs.
fn fixture_sending(request: &str) -> String {
    assert_eq!(COMMANDS.len(), 64, "the fixture's length constant");
    format!(
        r#"
(module
  (import "ferrocad" "call" (func $call (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{commands}")
  (data (i32.const 1200) "[]")
  (data (i32.const 1300) "{request}")
  (global $bump (mut i32) (i32.const 4096))

  (func (export "ferrocad_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (local.get $ptr) (i32.add (local.get $len) (i32.const 8))))
    (local.get $ptr))

  (func (export "ferrocad_dealloc") (param i32) (param i32))

  (func $pack (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $len)) (i64.const 32))
      (i64.extend_i32_u (local.get $ptr))))

  (func (export "ferrocad_commands") (result i64)
    (call $pack (i32.const 1024) (i32.const 64)))

  (func (export "ferrocad_run") (param $ptr i32) (param $len i32) (result i32)
    ;; Exercise the doorway once per run: the host must have granted it.
    (drop (call $call (i32.const 1300) (i32.const {request_len})))
    (i32.const 1))

  (func (export "ferrocad_stage_meshes") (param $ptr i32) (param $len i32) (result i64)
    (call $pack (i32.const 1200) (i32.const 2)))
)
"#,
        commands = COMMANDS.replace('"', "\\\""),
        request = request.replace('"', "\\\""),
        request_len = request.len(),
    )
}

/// A bundle that never returns: the sandbox's fuel budget has to stop it.
fn runaway() -> String {
    format!(
        r#"
(module
  (import "ferrocad" "call" (func $call (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{commands}")
  (func (export "ferrocad_alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "ferrocad_dealloc") (param i32) (param i32))
  (func (export "ferrocad_commands") (result i64)
    (i64.or (i64.shl (i64.const 64) (i64.const 32)) (i64.const 1024)))
  (func (export "ferrocad_run") (param i32 i32) (result i32)
    (loop $spin (br $spin))
    (i32.const 1))
  (func (export "ferrocad_stage_meshes") (param i32 i32) (result i64) (i64.const 0))
)
"#,
        commands = COMMANDS.replace('"', "\\\""),
    )
}

/// The host's document, as the doorway sees it.
#[derive(Default)]
struct FakeDocument {
    requests: Mutex<Vec<String>>,
}

impl DocumentApi for FakeDocument {
    fn call(&self, request: &str) -> String {
        self.requests.lock().unwrap().push(request.to_string());
        r#"{"ok":true}"#.to_string()
    }
}

fn bundle() -> (WasmAddonHost, Arc<FakeDocument>) {
    let document = Arc::new(FakeDocument::default());
    let typed: Arc<FakeDocument> = Arc::clone(&document);
    let erased: Arc<dyn DocumentApi> = typed;
    let host = WasmAddonHost::new(fixture().as_bytes(), erased).expect("the bundle instantiates");
    (host, document)
}

#[test]
fn a_bundle_registers_its_commands() {
    let (host, _) = bundle();
    let rows = host.commands();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "Toy_Wasm");
    assert_eq!(rows[0].menu, "Create Slab");
    assert_eq!(rows[0].group, "Wasm");
}

#[test]
fn a_listed_command_runs_through_the_doorway() {
    let (host, document) = bundle();

    let answer = host.run_command("Toy_Wasm").expect("the bundle's own id");
    assert!(answer.ok, "{:?}", answer.error);

    // The bundle called the doorway once, and only text crossed.
    assert_eq!(
        document.requests.lock().unwrap().as_slice(),
        [r#"{"op":"ping"}"#]
    );
}

#[test]
fn an_id_the_bundle_did_not_register_is_not_its() {
    let (host, document) = bundle();

    assert!(host.run_command("Toy_Other").is_none());
    assert!(
        document.requests.lock().unwrap().is_empty(),
        "an unlisted id must not reach the bundle"
    );
}

#[test]
fn the_display_hook_answers_the_document() {
    let (host, _) = bundle();

    match host.stage_meshes(&["Box"]) {
        Some(Ok(meshes)) => assert!(meshes.is_empty(), "the fixture adds nothing"),
        _ => panic!("the fixture has a display hook"),
    }
}

#[test]
fn a_runaway_bundle_traps_instead_of_hanging() {
    let document: Arc<dyn DocumentApi> = Arc::new(FakeDocument::default());
    // A small budget: the trap is the assertion, not how long 50 million instructions take.
    let host = WasmAddonHost::with_fuel(runaway().as_bytes(), document, 10_000)
        .expect("the bundle instantiates");

    let answer = host
        .run_command("Toy_Wasm")
        .expect("the id is in the bundle's list");
    assert!(!answer.ok, "an infinite loop is not a success");
    assert!(!answer.error.is_empty(), "the trap has to be reported");
}

#[test]
fn a_read_only_door_refuses_writes_and_passes_reads() {
    let document = Arc::new(FakeDocument::default());
    let typed: Arc<FakeDocument> = Arc::clone(&document);
    let erased: Arc<dyn DocumentApi> = typed;
    let door = read_only(erased);

    // Reads go through.
    let read = door.call(r#"{"call":"document"}"#);
    assert!(read.contains("\"ok\":true"), "{read}");
    let rows = door.call(r#"{"call":"properties","object":"Toy"}"#);
    assert!(rows.contains("\"ok\":true"), "{rows}");

    // Writes are refused before the document sees them.
    for request in [
        r#"{"call":"create","name":"Toy","type":"App::FeaturePython"}"#,
        r#"{"call":"set","object":"Toy","property":"Height","value":"30 mm"}"#,
        r#"{"call":"recompute"}"#,
    ] {
        let refused = door.call(request);
        assert!(refused.contains("read-only"), "{request} → {refused}");
        assert!(refused.contains("\"ok\":false"), "{refused}");
    }

    assert_eq!(
        document.requests.lock().unwrap().len(),
        2,
        "only the two reads reached the document"
    );
}

#[test]
fn a_bundle_is_read_only_unless_it_asks_for_more() {
    // The fixture never calls the manifest path (it is instantiated from bytes), which
    // is the safe default: no manifest, no writes.
    let (host, _) = bundle();
    assert_eq!(host.capability(), Capability::Read);
    assert!(host.hello().unwrap().unwrap().contains("read-only"));

    assert_eq!(Capability::parse("read"), Ok(Capability::Read));
    assert_eq!(Capability::parse("read-only"), Ok(Capability::Read));
    assert_eq!(Capability::parse("read-write"), Ok(Capability::Write));
    assert_eq!(Capability::parse("write"), Ok(Capability::Write));
    assert!(Capability::parse("admin").is_err());
}

#[test]
fn something_that_is_not_wasm_is_named() {
    let document: Arc<dyn DocumentApi> = Arc::new(FakeDocument::default());

    let error = WasmAddonHost::new(b"this is not a module", document)
        .err()
        .expect("garbage is not a bundle");
    assert!(error.contains("wasm"), "{error}");
}

/// A script half that is not a bundle: it stands in for CPython.
#[derive(Default)]
struct Primary {
    /// What the doorway announced, and which listeners ran, in order.
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl ScriptHost for Primary {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: true,
            output: format!("primary: {code}"),
        })
    }

    fn load_workbenches(&self) -> Result<ferrocad_shell_api::types::WorkbenchReport, String> {
        Ok(Default::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("no tool {name}"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(Recorder {
            log: Arc::clone(&self.events),
            who: "primary",
        })
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo {
            id: "Primary_One".to_string(),
            menu: "One".to_string(),
            ..Default::default()
        }]
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        (id == "Primary_One").then(|| CommandOutcome {
            ok: true,
            status: "primary ran it".to_string(),
            ..Default::default()
        })
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        self.events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event.id);
    }
}

/// A second script half, so a chain has something to be a chain of. It answers its own
/// command and its own console, which is what makes the ordering assertions meaningful.
#[derive(Default)]
struct Secondary {
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl ScriptHost for Secondary {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: true,
            output: format!("secondary: {code}"),
        })
    }

    fn load_workbenches(&self) -> Result<ferrocad_shell_api::types::WorkbenchReport, String> {
        Ok(Default::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("no tool {name}"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(Recorder {
            log: Arc::clone(&self.events),
            who: "secondary",
        })
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo {
            id: "Second_Two".to_string(),
            menu: "Two".to_string(),
            ..Default::default()
        }]
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        (id == "Second_Two").then(|| CommandOutcome {
            ok: true,
            status: "second ran it".to_string(),
            ..Default::default()
        })
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        self.events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event.id);
    }
}

/// A listener that records that it was asked, and changes nothing.
struct Recorder {
    log: Arc<Mutex<Vec<&'static str>>>,
    who: &'static str,
}

impl EventListener for Recorder {
    fn on_event(
        &mut self,
        _event: &Event,
        _hit: Option<&Hit>,
        _scene: &mut Scene,
        _directives: &Directives,
    ) -> bool {
        self.log
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(self.who);
        false
    }
}

#[test]
fn addons_are_composed_behind_the_primary() {
    let (addon, _) = bundle();
    let host = chain(vec![Arc::new(Primary::default())], vec![addon]);

    // The palette is the primary's rows, then the bundle's.
    let rows = host.commands();
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, ["Primary_One", "Toy_Wasm"]);

    // The primary answers first; the bundle answers what it registered.
    assert!(host.run_command("Primary_One").expect("primary's").ok);
    assert!(host.run_command("Toy_Wasm").expect("bundle's").ok);
    assert!(host.run_command("Nobody").is_none());

    // The console is the primary's, which is the point of composing rather than replacing.
    let answer = host.evaluate("1 + 1").expect("the console runs");
    assert_eq!(answer.output, "primary: 1 + 1");
}

/// Two script halves and a bundle, in the order a chain sees them.
fn composed() -> (HostChain, Arc<Primary>, Arc<Secondary>, Arc<FakeDocument>) {
    let first = Arc::new(Primary::default());
    let second = Arc::new(Secondary::default());
    let (addon, document) = bundle();
    let hosts: Vec<Arc<dyn ScriptHost>> = vec![first.clone(), second.clone()];
    (chain(hosts, vec![addon]), first, second, document)
}

#[test]
fn a_chain_merges_in_load_order() {
    let (host, _, _, _) = composed();

    // Every host's rows, in order: the halves first, then the bundle.
    let ids: Vec<String> = host.commands().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, ["Primary_One", "Second_Two", "Toy_Wasm"]);

    // The first host to answer wins, and a subordinate's id still reaches it.
    assert_eq!(
        host.run_command("Second_Two")
            .expect("the second half's id")
            .status,
        "second ran it"
    );
    assert!(host.run_command("Nobody").is_none());

    // The console is the primary's: one stdin and one history.
    assert_eq!(
        host.evaluate("1 + 1").expect("the console").output,
        "primary: 1 + 1"
    );
}

#[test]
fn a_chain_fans_out_the_document_hooks() {
    let (host, first, second, _) = composed();

    host.on_document_event(&DocumentEvent::edited("Create Toy", &["Toy".to_string()]));

    // Not "the primary heard it": every host does, so a subordinate can keep its own
    // state in step with a document it did not write.
    assert_eq!(
        first.events.lock().unwrap().as_slice(),
        [DocumentEvent::EDITED]
    );
    assert_eq!(
        second.events.lock().unwrap().as_slice(),
        [DocumentEvent::EDITED]
    );
}

#[test]
fn a_chain_runs_every_event_listener() {
    let (host, first, second, _) = composed();

    let mut listener = host.event_listener();
    let changed = listener.on_event(
        &Event::MouseMove { position: [0, 0] },
        None,
        &mut Scene::default(),
        &Directives::default(),
    );

    assert!(!changed, "neither fixture edits the scene");
    assert_eq!(first.events.lock().unwrap().as_slice(), ["primary"]);
    assert_eq!(second.events.lock().unwrap().as_slice(), ["secondary"]);
}

#[test]
fn a_chain_of_one_is_just_that_host() {
    // The single-half case must not pay for the generalisation: one listener is handed
    // back unwrapped, and `new` is still the same constructor the desktop calls.
    let script = Arc::new(Primary::default());
    let host = chain(vec![script.clone()], vec![]);

    assert_eq!(host.commands().len(), 1);
    assert_eq!(
        host.evaluate("2 + 2").expect("the console").output,
        "primary: 2 + 2"
    );
    assert!(chain(vec![], vec![]).is_empty());
}

#[test]
fn an_addons_own_edit_is_announced_to_the_primary() {
    // The desktop's composition: the primary, the bundles, and a doorway whose sink is
    // that primary. `with_addons` builds exactly this, so what is asserted here is the
    // rule — an addon's `create` is a document edit like the shell's own.
    {
        // A bundle cannot create with no document open, and that refusal reads exactly
        // like a broken ABI.
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
        registry.new_document(Some("Announced"));
    }

    let script = Arc::new(Primary::default());
    let primary: Arc<dyn ScriptHost + Send + Sync> = script.clone();
    let document: Arc<dyn DocumentApi> = Arc::new(ferrocad_doc_api::DocApi::with_events(
        announce_to(Arc::clone(&primary)),
    ));

    // A read-write manifest, because the bundle is asking to create.
    let temp = tempfile::tempdir().expect("a temporary directory");
    let path = temp.path().join("toy.wasm");
    std::fs::write(
        &path,
        fixture_sending(r#"{"call":"create","name":"Toy","type":"App::FeaturePython"}"#),
    )
    .expect("the bundle");
    std::fs::write(
        temp.path().join("toy.json"),
        r#"{"document": "read-write"}"#,
    )
    .expect("the manifest");
    let addon = WasmAddonHost::open(&path, Arc::clone(&document)).expect("the bundle instantiates");

    let host = chain(vec![primary], vec![addon]);
    let ran = host.run_command("Toy_Wasm").expect("the bundle's own id");
    assert!(ran.ok, "{:?}", ran.error);

    // The write was the bundle's own, and the primary heard about it through the same
    // sink the shell's edits use.
    let events = script
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(events.as_slice(), [DocumentEvent::EDITED, DocumentEvent::RECOMPUTED]);
}

#[test]
fn an_addon_only_composition_has_no_console() {
    let host = chain(vec![], vec![]);

    let answer = host.evaluate("1 + 1").expect("evaluate reports, it does not fail");
    assert!(!answer.ok);
    assert!(answer.output.contains("no console"), "{}", answer.output);
    assert!(host.commands().is_empty());
}

#[test]
fn the_packs_are_the_directories_with_an_addon_in_name_order() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let mods = temp.path().join("mods");
    for name in ["Zeta", "Alpha"] {
        let entry = mods.join(name).join("addon.wasm");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, fixture()).unwrap();
    }
    // A workbench with no bundle is not a wasm workbench.
    std::fs::create_dir_all(mods.join("PythonOnly")).unwrap();
    std::fs::write(mods.join("PythonOnly").join("InitGui.py"), "").unwrap();

    let found = packs(&[mods]);
    let names: Vec<String> = found
        .iter()
        .map(|path| {
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["Alpha", "Zeta"]);
}
