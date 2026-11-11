//! Workbench scripts **from data**: the browser's loader (B19a).
//!
//! `wasm32` has no `os` and the page has no `mods/` tree to scan, so the browser cannot
//! call `load_workbenches`. It can `fetch`: the host decodes a pack and hands the sources
//! to `ferrocad_shell.load_workbench_sources`, which runs them with the same exec and
//! isolation rules as the filesystem loader. These tests prove the half that has nothing
//! to do with the network — a pack can register a command, and a broken script is
//! recorded instead of taking the others with it.

use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// Run `f` against a freshly booted interpreter on a large stack (RustPython's compiler
/// recurses; see `properties.rs` for the same requirement).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// Run the pack loader and decode its report.
fn load(host: &PywasmScriptHost, payload: &str) -> Value {
    let raw = host
        .call("load_workbench_sources", &[payload])
        .expect("the loader answers");
    serde_json::from_str(&raw).expect("the loader answers JSON")
}

/// The console's answer for one expression, or a panic quoting it.
fn evaluate(host: &PywasmScriptHost, code: &str) -> String {
    let raw = host.call("evaluate", &[code]).expect("the console runs");
    let answer: Value = serde_json::from_str(&raw).expect("evaluate answers JSON");
    assert_eq!(answer["ok"], true, "{answer}");
    answer["output"].as_str().unwrap_or_default().to_string()
}

#[test]
fn a_pack_registers_a_command_without_a_filesystem() {
    with_host(|host| {
        let report = load(
            host,
            r#"[
              {"name": "Demo", "script": "Init.py",
               "source": "import FreeCAD\nFreeCAD.demo_init = True\n"},
              {"name": "Demo", "script": "InitGui.py",
               "source": "import FreeCAD\nimport FreeCADGui\nclass _Run:\n    def Activated(self):\n        return {'status': 'demo ran'}\n    def GetResources(self):\n        return {'MenuText': 'Demo'}\nFreeCADGui.addCommand('Demo_Run', _Run())\n"}
            ]"#,
        );
        assert_eq!(
            report["loaded"],
            serde_json::json!(["Demo/Init.py", "Demo/InitGui.py"]),
            "both scripts ran, in pack order: {report}"
        );
        assert_eq!(report["errors"], serde_json::json!([]), "{report}");

        // The pack's command is in the facade's registry, and only the script can run
        // its `Activated` — which is what the `commands` contract will expose (B18a).
        let ids = evaluate(
            host,
            "import FreeCADGui\nprint(list(FreeCADGui.listCommands()))",
        );
        assert!(ids.contains("Demo_Run"), "the command registered: {ids}");
    });
}

#[test]
fn a_broken_pack_is_recorded_and_does_not_cost_the_others() {
    with_host(|host| {
        let report = load(
            host,
            r#"[
              {"name": "Bad", "script": "InitGui.py", "source": "raise RuntimeError('nope')\n"},
              {"name": "Good", "script": "InitGui.py",
               "source": "import FreeCAD\nFreeCAD.good_loaded = True\n"}
            ]"#,
        );
        assert_eq!(
            report["loaded"],
            serde_json::json!(["Good/InitGui.py"]),
            "the working mod still loaded: {report}"
        );
        let errors = report["errors"].as_array().expect("errors is a list");
        assert_eq!(errors.len(), 1, "{report}");
        assert_eq!(errors[0]["workbench"], "Bad");
        assert_eq!(errors[0]["script"], "InitGui.py");
        // The report has to name the cause even when the engine cannot format a
        // traceback: RustPython's frozen pylib answers "" from `format_exc()`, and an
        // empty report makes every failure look the same.
        let traceback = errors[0]["traceback"].as_str().expect("traceback is text");
        assert!(
            traceback.contains("RuntimeError") && traceback.contains("nope"),
            "the failure names the exception: {traceback:?}"
        );

        // The interpreter is still usable: a recorded failure is not a crashed host.
        assert!(evaluate(host, "1 + 1").contains('2'));
    });
}

#[test]
fn a_malformed_entry_is_skipped() {
    with_host(|host| {
        let report = load(
            host,
            r#"[
              "not an object",
              {"name": "", "script": "InitGui.py", "source": "pass\n"},
              {"name": "Odd", "script": "Init.py", "source": "pass\n"}
            ]"#,
        );
        assert_eq!(
            report["loaded"],
            serde_json::json!(["Odd/Init.py"]),
            "only the well-formed entry ran: {report}"
        );
        assert_eq!(report["errors"], serde_json::json!([]), "{report}");
    });
}
