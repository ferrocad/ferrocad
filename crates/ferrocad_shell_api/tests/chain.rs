//! The composition rule, on its own: a chain of hosts behind one primary.
//!
//! `ferrocad_shell_wasm` exercises the same type through real wasm bundles and
//! `ferrocad_shell_web` through real interpreters, which is slow and feature-dependent.
//! These are the two rules that are easy to regress and not obvious from a call site:
//! who owns the console, and who decides whether the chain has a display hook.

use std::sync::{Arc, Mutex};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::types::EvalResult;
use ferrocad_shell_api::{DocumentEvent, HostChain, ScriptHost};

/// A host that answers its own console line and command, and can offer a display hook.
struct Stub {
    name: &'static str,
    /// Whether `stage_meshes` has a hook. It always returns no rows: the point is whether
    /// the *composition* counts an empty answer as having one.
    hook: bool,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    fn new(name: &'static str, hook: bool) -> Self {
        Self {
            name,
            hook,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ScriptHost for Stub {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        Ok(EvalResult {
            ok: true,
            output: format!("{}: {code}", self.name),
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
        struct Quiet;
        impl EventListener for Quiet {
            fn on_event(
                &mut self,
                _event: &Event,
                _hit: Option<&Hit>,
                _scene: &mut Scene,
                _directives: &Directives,
            ) -> bool {
                false
            }
        }
        Box::new(Quiet)
    }

    fn stage_meshes(&self, _have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        self.hook.then(|| Ok(Vec::new()))
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo {
            id: self.name.to_string(),
            menu: self.name.to_string(),
            ..Default::default()
        }]
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        (id == self.name).then(|| CommandOutcome {
            ok: true,
            status: format!("{} ran it", self.name),
            ..Default::default()
        })
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event.id.to_string());
    }
}

fn chain(stubs: &[&Arc<Stub>]) -> HostChain {
    HostChain::new(
        stubs
            .iter()
            .map(|stub| Arc::clone(stub) as Arc<dyn ScriptHost>)
            .collect(),
    )
}

#[test]
fn the_first_host_is_the_primary() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    // One console, and it is the first host's.
    assert_eq!(chain.evaluate("x").expect("console").output, "first: x");
    assert_eq!(chain.len(), 2);
    assert!(!chain.is_empty());
}

#[test]
fn commands_concatenate_and_the_first_answer_wins() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    let ids: Vec<String> = chain.commands().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, ["first", "second"]);
    assert_eq!(
        chain.run_command("second").expect("the subordinate").status,
        "second ran it"
    );
    assert!(chain.run_command("nobody").is_none());
}

#[test]
fn the_primary_decides_whether_the_chain_has_a_display_hook() {
    // A primary with no hook: the backend's own rows stay, even if a subordinate offers an
    // (empty) hook. This is the rule the desktop's addon composition had, kept intact.
    let bare = Arc::new(Stub::new("bare", false));
    let hooked = Arc::new(Stub::new("hooked", true));
    assert!(chain(&[&bare, &hooked]).stage_meshes(&[]).is_none());

    // A primary that answers `Ok` — even with no rows — means the chain has a hook.
    match chain(&[&hooked, &bare]).stage_meshes(&[]) {
        Some(Ok(rows)) => assert!(rows.is_empty()),
        other => panic!("the primary's answer decides: {other:?}"),
    }

    // Nothing installed at all is still no hook.
    assert!(HostChain::new(Vec::new()).stage_meshes(&[]).is_none());
}

#[test]
fn every_host_hears_the_document() {
    let first = Arc::new(Stub::new("first", true));
    let second = Arc::new(Stub::new("second", true));
    let chain = chain(&[&first, &second]);

    chain.on_document_event(&DocumentEvent::edited("Create", &["Toy".to_string()]));

    // A subordinate keeps its own state in step with a document it did not write.
    assert_eq!(first.seen.lock().unwrap().as_slice(), ["document.edited"]);
    assert_eq!(second.seen.lock().unwrap().as_slice(), ["document.edited"]);
}
