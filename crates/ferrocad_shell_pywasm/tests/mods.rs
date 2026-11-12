//! Workbench scripts **from data**: the browser's loader (B19a).
//!
//! `wasm32` has no `os` and the page has no `mods/` tree to scan, so the browser cannot
//! call `load_workbenches`. It can `fetch`: the host decodes a pack and hands the sources
//! to `ferrocad_shell.load_workbench_sources`, which runs them with the same exec and
//! isolation rules as the filesystem loader. These tests prove the half that has nothing
//! to do with the network — a pack can register a command, and a broken script is
//! recorded instead of taking the others with it.

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_pywasm::PywasmScriptHost;

/// The overlay registry is process state; one test at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}
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

/// The **shipped** settings clients load here (B18b).
///
/// `mods/GridOverlay`, `AxisOverlay` and `GizmoOverlay` are pure-Python provider mods:
/// each imports `os`, `FreeCADGui`, `FerroCAD` and `FerroCADGui` at module scope, reads
/// its semantic store to fill the card, and declares a provider. Before B18b they all
/// stopped at `import os` here; now every name they need is installed, so the browser's
/// facade is exercised by real mods rather than synthetic ones. (The Coin/shader skins
/// read a resource file and need the overlay bridge, so they are separate.)
#[test]
fn the_shipped_settings_clients_load() {
    with_host(|host| {
        let report = load(
            host,
            &serde_json::json!([
                {"name": "GridOverlay", "script": "InitGui.py",
                 "source": include_str!("../../../mods/GridOverlay/InitGui.py")},
                {"name": "AxisOverlay", "script": "InitGui.py",
                 "source": include_str!("../../../mods/AxisOverlay/InitGui.py")},
                {"name": "GizmoOverlay", "script": "InitGui.py",
                 "source": include_str!("../../../mods/GizmoOverlay/InitGui.py")}
            ])
            .to_string(),
        );
        assert_eq!(
            report["loaded"],
            serde_json::json!([
                "GridOverlay/InitGui.py",
                "AxisOverlay/InitGui.py",
                "GizmoOverlay/InitGui.py"
            ]),
            "all three shipped settings clients ran: {report}"
        );

        // Each one registered its own command and claimed its feature.
        let commands = evaluate(
            host,
            "import FreeCADGui\nprint(sorted(FreeCADGui.listCommands()))",
        );
        for command in [
            "GridOverlay_Settings",
            "AxisOverlay_Settings",
            "GizmoOverlay_Settings",
        ] {
            assert!(
                commands.contains(command),
                "{command} registered: {commands}"
            );
        }
        let providers = evaluate(
            host,
            "import FerroCADGui\nprint([(m['feature'], m['backend']) for m in FerroCADGui.providers()])",
        );
        for feature in ["grid", "axis", "gizmo"] {
            assert!(
                providers.contains(&format!("('{feature}', 'ferrocad')")),
                "the {feature} feature has the native backend: {providers}"
            );
        }
    });
}

/// The **Coin** skins load too (B20).
///
/// `mods/GridOverlayCoin`, `AxisOverlayCoin` and `GizmoOverlayShader` import
/// `pivy.coin` at module scope and declare a provider; the shader tree itself is built on
/// activation, which is where the WGSL file is read — so loading (and registering) is the
/// part this host can prove without an addon resource directory.
#[test]
fn the_shipped_coin_skins_load() {
    with_host(|host| {
        let report = load(
            host,
            &serde_json::json!([
                {"name": "GridOverlayCoin", "script": "InitGui.py",
                 "source": include_str!("../../../mods/GridOverlayCoin/InitGui.py")},
                {"name": "AxisOverlayCoin", "script": "InitGui.py",
                 "source": include_str!("../../../mods/AxisOverlayCoin/InitGui.py")},
                {"name": "GizmoOverlayShader", "script": "InitGui.py",
                 "source": include_str!("../../../mods/GizmoOverlayShader/InitGui.py")}
            ])
            .to_string(),
        );
        let loaded = report["loaded"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            loaded.len(),
            3,
            "the three Coin skins ran: {report}"
        );
        assert_eq!(
            report["errors"],
            serde_json::json!([]),
            "no Coin skin raised: {report}"
        );

        let providers = evaluate(
            host,
            "import FerroCADGui\nprint([(m['feature'], m['backend']) for m in FerroCADGui.providers()])",
        );
        for backend in ["('grid', 'coin')", "('axis', 'coin')", "('gizmo', 'ferrocad-shader')"] {
            assert!(
                providers.contains(backend),
                "the Coin backend registered ({backend}): {providers}"
            );
        }
    });
}

/// The Coin grid skin, from its pack, all the way to a registered overlay (B20).
///
/// This is the payoff of shipping `Resources/` in a pack: the mod's `ShaderGrid` reads
/// `grid_overlay.wgsl` from its own `__file__` directory, builds the shader tree, and
/// hands it to the pull bridge — on a host with no filesystem.
#[test]
fn the_coin_grid_skin_builds_its_tree_from_the_pack() {
    let _guard = serial();
    ferrocad_inventor::overlays::clear();
    with_host(|host| {
        let pack = serde_json::json!({
            "name": "GridOverlayCoin",
            "version": 1,
            "files": {
                "Init.py": include_str!("../../../mods/GridOverlayCoin/Init.py"),
                "InitGui.py": include_str!("../../../mods/GridOverlayCoin/InitGui.py"),
                "Resources/grid_overlay.wgsl":
                    include_str!("../../../mods/GridOverlayCoin/Resources/grid_overlay.wgsl"),
            }
        })
        .to_string();
        let report = host.load_workbench_pack(&pack).expect("the pack loads");
        assert!(report.errors.is_empty(), "{report:?}");

        // The grid switch is the feature's, not the backend's: turn it on, then hand
        // the feature to the Coin backend, whose `activate` builds the shader tree —
        // including the WGSL it reads from the pack.
        let switched = evaluate(
            host,
            "import FerroCAD\nFerroCAD.Grid.enabled = True\nprint(App.ActiveDocument.Name)",
        );
        let outcome = host
            .set_provider("grid", "coin")
            .expect("the coin backend answers");
        assert!(outcome.ok, "{outcome:?} ({switched})");

        assert!(ferrocad_inventor::overlays::dirty(), "the tree was registered");
        let scenes = ferrocad_inventor::overlays::take_scenes();
        assert_eq!(scenes.len(), 1, "one overlay from the grid backend");
        let (queue, _directives) = ferrocad_inventor::compile(&scenes[0]);
        assert!(!queue.is_empty(), "the shader quad produced a packet");
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
