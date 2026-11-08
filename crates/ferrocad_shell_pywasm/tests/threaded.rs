//! RustPython as a desktop script half: a `ScriptHost` whose interpreter is elsewhere.
//!
//! The interesting part is that the shape works at all — the shell asks a `ScriptHost` on
//! its own thread, the VM lives on a 16 MiB worker, and only the console crosses. These
//! tests therefore cover the crossing, the constants answered locally, and the shutdown
//! (a worker that keeps its channel open deadlocks every drop, which is how the JS page's
//! first suite hung).

#![cfg(not(target_family = "wasm"))]

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::{ThreadedScriptHost, threaded};

#[test]
fn the_worker_boots_and_answers_the_console() {
    let host = ThreadedScriptHost::live().expect("the interpreter boots");

    let answer = host.evaluate("6 * 7").expect("the console runs");
    assert!(answer.ok, "{}", answer.output);
    assert!(answer.output.contains("42"), "{}", answer.output);
}

#[test]
fn a_broken_line_is_a_result_not_a_panic() {
    let host = ThreadedScriptHost::live().expect("the interpreter boots");

    let answer = host.evaluate("this is not python").expect("the console runs");
    assert!(!answer.ok, "a syntax error is not a success: {}", answer.output);
}

#[test]
fn the_constants_are_answered_without_the_interpreter() {
    // Nothing but `evaluate` crosses the channel. Booting a second interpreter here to
    // compare would need a 16 MiB thread of its own — which is the reason this type
    // exists — so the answers are asserted directly instead.
    let host = ThreadedScriptHost::live().expect("the interpreter boots");

    let hello = host.hello().expect("a greeting").expect("one, not silence");
    assert!(hello.contains("RustPython"), "{hello}");
    assert!(host.commands().is_empty());
    assert!(host.run_command("App_Delete").is_none());
    assert!(!host.tool_command("anything", "start"));
    let no_tool = host.tool("toy_polyline").err().expect("the spike has no tools");
    assert!(no_tool.contains("toy_polyline"), "{no_tool}");
}

#[test]
fn the_stack_the_worker_asks_for_is_the_documented_one() {
    // The value the interpreter's compiler needs; a smaller one aborts inside rustpython
    // rather than failing here, which is why it is a constant and not a parameter.
    assert_eq!(threaded::STACK, 16 << 20);
}
