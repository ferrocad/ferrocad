//! RustPython as a desktop script half: a `ScriptHost` whose interpreter is elsewhere.
//!
//! The interesting part is that the shape works at all — the shell asks a `ScriptHost` on
//! its own thread, the VM lives on a 16 MiB worker, and only the console crosses. These
//! tests therefore cover the crossing, the constants answered locally, and the shutdown
//! (a worker that keeps its channel open deadlocks every drop, which is how the JS page's
//! first suite hung).

#![cfg(not(target_family = "wasm"))]

use ferrocad_python_host::{Contract, PythonRuntime};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::threaded;

#[test]
fn the_worker_boots_and_answers_the_console() {
    let host = threaded::live().expect("the interpreter boots");

    let answer = host.evaluate("6 * 7").expect("the console runs");
    assert!(answer.ok, "{}", answer.output);
    assert!(answer.output.contains("42"), "{}", answer.output);
}

#[test]
fn a_broken_line_is_a_result_not_a_panic() {
    let host = threaded::live().expect("the interpreter boots");

    let answer = host
        .evaluate("this is not python")
        .expect("the console runs");
    assert!(
        !answer.ok,
        "a syntax error is not a success: {}",
        answer.output
    );
}

#[test]
fn the_constants_are_answered_without_the_interpreter() {
    // Nothing but `evaluate` crosses the channel. Booting a second interpreter here to
    // compare would need a 16 MiB thread of its own — which is the reason this type
    // exists — so the answers are asserted directly instead.
    let host = threaded::live().expect("the interpreter boots");

    let hello = host.hello().expect("a greeting").expect("one, not silence");
    assert!(hello.contains("RustPython"), "{hello}");
    // The greeting tells the truth about the grant: the worker owns the registry, so
    // this half writes through the doorway (B18b item 5).
    assert!(hello.contains("read-write"), "{hello}");
    assert_eq!(
        host.document_grant(),
        ferrocad_shell_api::DocumentGrant::ReadWrite
    );
    // The contract is `FULL` now, so the script's registry answers the commands (this
    // used to assert an empty list while the preview reported `NONE`). The catalogue's
    // own verbs still fall through to the document half.
    assert!(
        !host.commands().is_empty(),
        "the shared command registry is the preview's"
    );
    assert!(
        host.run_command("App_Move").is_none(),
        "the document half owns the document verbs"
    );
    assert!(!host.tool_command("anything", "start"));
    let no_tool = host
        .tool("toy_polyline")
        .err()
        .expect("this half has no tools");
    assert!(no_tool.contains("toy_polyline"), "{no_tool}");
}

#[test]
fn a_python_tool_crosses_the_worker_channel() {
    let host = threaded::live().expect("the interpreter boots");
    let script = "\
import ferrocad_shell, ferrocad_stage
class Probe(ferrocad_stage.Tool):
    def __init__(self):
        self.seen = 0
        self.add_item(ferrocad_stage.ShapeItem(7))
    def on_event(self, event):
        self.seen += 1
        return True
    def status(self):
        return {'id': 'probe', 'label': 'Probe', 'active': True, 'points': self.seen, 'hint': 'h'}
    def cancel(self):
        return True
_probe = Probe()
ferrocad_shell.tool_bridge = lambda name: _probe
print('ready')
";
    let answer = host.evaluate(script).expect("the console runs");
    assert!(answer.ok, "{}", answer.output);

    let mut tool = host.tool("probe").expect("the worker builds the tool");
    assert_eq!(tool.items().len(), 1);
    assert_eq!(tool.status().points, 0);

    let event = ferrocad_stage::PointerEvent {
        kind: ferrocad_stage::PointerKind::Down,
        position: [4, 5],
        button: Some(ferrocad_stage::MouseButton::Left),
        hit: None,
        projection: None,
    };
    tool.on_event(&event, None);
    assert_eq!(tool.status().points, 1, "the event reached the worker's VM");
    assert!(tool.cancel());
}

#[test]
fn the_stack_the_worker_asks_for_is_the_documented_one() {
    // The value the interpreter's compiler needs; a smaller one aborts inside rustpython
    // rather than failing here, which is why it is a constant and not a parameter.
    assert_eq!(threaded::STACK, 16 << 20);
}

/// The desktop preview is the whole facade, not the document half's leftovers.
///
/// The worker boots `live_persistent`, so `view` answers the appearance the user
/// actually saved; that store was the one reason this contract had to stay `NONE`.
#[test]
fn the_desktop_preview_answers_the_whole_contract() {
    let runtime = threaded::ThreadedRuntime::live().expect("the worker boots");

    assert_eq!(runtime.contract(), Contract::FULL);
    assert!(
        runtime.has_function("interaction"),
        "the shared shell module answers the interaction hooks"
    );
    assert!(
        !runtime.has_function("add_datum_plane"),
        "datum geometry is the edition's, and this half installs no app script"
    );
}
