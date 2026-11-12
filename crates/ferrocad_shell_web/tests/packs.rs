//! A fetched mod pack, end to end through the backend (B19c).
//!
//! The browser has no filesystem, so `load_workbenches` cannot run there. The page
//! `fetch`es `www/mods/<Name>.json` instead — one pack document as `tools/mods_pack.py`
//! writes it — and hands it to [`AppBackend::load_workbench_pack`], which the script
//! half flattens to the loader's entries. These tests prove the Rust half of that chain
//! without a network: the same `ferrocad_shell_web::backend` the window composes, one
//! RustPython host, and a hand-written pack.
//!
//! What is *not* here is the refresh a shell needs after loading: `ShellModel.commands`
//! and `Shell::shortcuts` are snapshots taken at boot, and the seam deliberately does not
//! reach into the UI. The next slice (the page's `fetch`) calls
//! `ferrocad_shell_core::app::load_workbench_pack(cx, pack)` and then, once per batch,
//! `ferrocad_shell_core::Shell::refresh_after_mods` — see B19c in
//! `docs/project/tasks/backend-parity.md`.

use std::sync::{Mutex, MutexGuard};

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_web::{Runtime, backend};

/// The document registry is process-wide and `?runtime=pywasm` reads it, so these tests
/// take turns: a `bootstrap()` in one replaces the active document under another.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

/// Run `f` on a freshly booted backend, on a large stack.
///
/// RustPython's compiler recurses; `ferrocad_shell_web`'s own tests and
/// `ferrocad_shell_pywasm`'s use the same 16 MiB. The browser gets it from
/// `ferrocad_web_gpui`'s link argument.
fn with_backend<R: Send + 'static>(f: impl FnOnce(Box<dyn AppBackend>) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut composed = backend(&[Runtime::Pywasm]).expect("the interpreter boots");
            // The real order: the window boots, then the page fetches packs after the
            // first frame. Nothing in the pack path needs the document, but a mod that
            // reads `FreeCAD.ActiveDocument` at import does.
            composed.bootstrap().expect("the sample boots");
            f(composed)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// A pack whose `InitGui.py` registers a command, exactly as a hand-written mod would.
///
/// The file keys are out of order on purpose: the flattening, not the JSON, sets
/// `Init.py` before `InitGui.py`.
const PACK: &str = r#"{
  "name": "Demo",
  "version": 1,
  "files": {
    "InitGui.py": "import FreeCAD\nimport FreeCADGui\nclass _Run:\n    def Activated(self):\n        return {'status': 'demo ran'}\n    def GetResources(self):\n        return {'MenuText': 'Demo', 'Pixmap': 'demo'}\nFreeCADGui.addCommand('Demo_Run', _Run())\n",
    "Init.py": "import FreeCAD\nFreeCAD.demo_init = True\n"
  }
}"#;

/// A pack that ships a `Resources/` file and reads it at import, as the Coin skins do.
///
/// The mod computes the path from its own `__file__`, which the loader names
/// `<name>/<script>` — so the pack's `Resources/shader.wgsl` has to arrive at
/// `Shaded/Resources/shader.wgsl` for the same script to work on a host with no
/// filesystem.
const RESOURCE_PACK: &str = r#"{
  "name": "Shaded",
  "version": 1,
  "files": {
    "Init.py": "import FreeCAD\nFreeCAD.shaded_init = True\n",
    "InitGui.py": "import os\nimport FreeCAD\nimport FreeCADGui\nhere = os.path.dirname(os.path.abspath(__file__))\nwith open(os.path.join(here, 'Resources', 'shader.wgsl')) as handle:\n    FreeCAD.shaded_shader = handle.read()\nclass _Run:\n    def Activated(self):\n        return {'status': 'shaded ran'}\n    def GetResources(self):\n        return {'MenuText': 'Shaded', 'Pixmap': 'shaded'}\nFreeCADGui.addCommand('Shaded_Run', _Run())\n",
    "Resources/shader.wgsl": "@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }\n"
  }
}"#;

/// The pack's resource is installed where the mod's `__file__` points, so its `open`
/// finds it — and an absent one is still `FileNotFoundError`.
#[test]
fn a_pack_installs_its_resources_where_the_mod_reads_them() {
    let _guard = serial();
    with_backend(|backend| {
        let report = backend
            .load_workbench_pack(RESOURCE_PACK)
            .expect("a pack with a resource loads");
        assert!(report.errors.is_empty(), "{report:?}");

        let script = [
            "import FreeCAD",
            "print(FreeCAD.shaded_shader.strip())",
            "try:",
            "    open('Shaded/Resources/absent.wgsl')",
            "except FileNotFoundError:",
            "    print('absent')",
        ]
        .join("\n");
        let answer = backend.evaluate(&script).expect("evaluate");
        assert!(answer.ok, "{}", answer.output);
        assert!(
            answer.output.contains("@fragment fn fs()"),
            "the mod read its shader through open(): {}",
            answer.output
        );
        assert!(
            answer.output.contains("absent"),
            "an uninstalled path is still FileNotFoundError: {}",
            answer.output
        );

        // The patch is transparent: a real file still opens. That is the desktop preview,
        // which imports `python/` from disk and keeps `host_env`.
        let manifest = std::env::current_dir()
            .expect("a working directory")
            .join("Cargo.toml");
        let script = format!(
            "print(open({manifest:?}, encoding='utf-8').read().startswith('[package]'))",
            manifest = manifest.to_string_lossy()
        );
        let answer = backend.evaluate(&script).expect("evaluate");
        assert!(answer.ok, "{}", answer.output);
        assert_eq!(answer.output.trim(), "True", "a real file still opens");
    });
}

/// A pack with one good script and one that raises.
const BROKEN_PACK: &str = r#"{
  "name": "Mixed",
  "version": 1,
  "files": {
    "Init.py": "import FreeCAD\nFreeCAD.mixed_init = True\n",
    "InitGui.py": "raise RuntimeError('this mod is broken')\n"
  }
}"#;

fn ids(backend: &dyn AppBackend) -> Vec<String> {
    backend
        .commands()
        .expect("the command list")
        .into_iter()
        .map(|command| command.id)
        .collect()
}

#[test]
fn a_fetched_pack_runs_both_scripts_and_registers_its_command() {
    let _serial = serial();
    with_backend(|backend| {
        let report = backend
            .load_workbench_pack(PACK)
            .expect("a well-formed pack loads");
        assert_eq!(
            report.loaded,
            vec!["Demo/Init.py".to_string(), "Demo/InitGui.py".to_string()],
            "both entry scripts ran, Init.py first"
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        // The registered command reaches the palette: the same `commands()` the shell
        // reads, which is what a mod is for.
        assert!(
            ids(&*backend).contains(&"Demo_Run".to_string()),
            "the pack's command is in the list: {:?}",
            ids(&*backend)
        );
    });
}

#[test]
fn a_raising_script_is_recorded_and_does_not_cost_the_other() {
    let _serial = serial();
    with_backend(|backend| {
        let report = backend
            .load_workbench_pack(BROKEN_PACK)
            .expect("a well-formed pack with a broken script is still Ok");
        assert_eq!(report.loaded, vec!["Mixed/Init.py".to_string()]);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        let error = &report.errors[0];
        assert_eq!(error.workbench, "Mixed");
        assert_eq!(error.script, "InitGui.py");

        // A recorded failure is not a crashed host: the interpreter still answers.
        let console = backend.evaluate("1 + 1").expect("the console runs");
        assert!(console.ok, "{}", console.output);
    });
}

#[test]
fn a_malformed_pack_is_an_error() {
    let _serial = serial();
    with_backend(|backend| {
        let error = backend.load_workbench_pack("this is not JSON").unwrap_err();
        assert!(error.starts_with("mod pack: "), "{error}");

        let error = backend
            .load_workbench_pack(r#"{"name": "Fasteners", "version": 1}"#)
            .unwrap_err();
        assert!(error.contains("files"), "the missing map is named: {error}");
    });
}
