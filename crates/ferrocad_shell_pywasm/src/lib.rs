//! The wasm-Python scripting surface: [`ScriptHost`] over RustPython (S8 **spike**).
//!
//! The JS backend's composition ([`ferrocad_shell_js`]) is the shape here too: the
//! document half stays `ferrocad_shell_rust`'s `RustAppBackend`, and this crate is the
//! script half the shell asks for through `AppBackend::script_host()`:
//!
//! ```text
//! RustAppBackend (document)  +  PywasmScriptHost (this crate)  =  the pywasm backend
//! ```
//!
//! **The runtime is RustPython**, the second option in §6.3. It is a Python VM written
//! in Rust: no C ABI, no Pyodide download, no COOP/COEP, and — unlike Pyodide — it
//! compiles for `wasm32-unknown-unknown` with `host_env` off. §6.3's Pyodide row would
//! have needed a JS-mediated binding layer plus a ~10 MB runtime; MicroPython would
//! have been the least compatible with `python/`.
//!
//! **What is shared.** The script is literally
//! `python/ferrocad_shell/__init__.py`, included as source, and the property rules are
//! `ferrocad_core::present` (see [`snapshot`]). What is *not* shared is the `*_py` PyO3
//! bindings: those are CPython extension modules and RustPython cannot load them, so
//! the bridge re-exposes exactly the rows the shell needs. §6.3 calls this the fork's
//! central trade-off, and the spike confirms it.
//!
//! **Why this crate also builds natively.** `ferrocad_shell_js` is `cfg(wasm)`
//! because its only implementation is a `window` bridge. RustPython has no such
//! dependency, so this crate is plain Rust and its test runs `properties()` on a
//! normal `cargo test` — the wasm target is proven separately by
//! `cargo check --target wasm32-unknown-unknown`. Both are in the acceptance.

use std::cell::RefCell;

use ferrocad_inventor::{Event, EventListener, Hit, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};
use rustpython_vm::compiler::Mode;
use rustpython_vm::{
    Context, Interpreter, PyRef, PyResult, Settings, VirtualMachine, builtins::PyBaseException,
    pymodule,
};

mod snapshot;

/// The scripting module: `python/ferrocad_shell`, the **same source** the native
/// assembly runs. A change there is a change here, with no port in between.
const SHELL: &str = include_str!("../../../python/ferrocad_shell/__init__.py");

/// The generated built-in catalogue the module reads beside itself at import
/// (`_load_catalog(pathlib.Path(__file__).with_name("catalog.json"))`).
const CATALOG: &str = include_str!("../../../python/ferrocad_shell/catalog.json");

/// The `FreeCAD` surface over the bridge, and the modules `wasm32-unknown-unknown`
/// cannot import (see `bootstrap.py`).
const BOOTSTRAP: &str = include_str!("bootstrap.py");

/// Loads [`SHELL`] as the module `ferrocad_shell`.
///
/// The module object is created here rather than by `import` because the source is
/// embedded in the binary, not on a filesystem the interpreter can see. The embedded
/// [`CATALOG`] is served through a module-local `open`, so the shell's import-time
/// `_load_catalog` reads the same table the native assembly ships.
const LOADER: &str = "\
import sys

class _Reader:
    def __init__(self, text):
        self._text = text

    def read(self, size=-1):
        return self._text if size is None or size < 0 else self._text[:size]

    def readline(self):
        return self._text

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

def _open(path, *args, **kwargs):
    key = str(path)
    if key in SHELL_FILES:
        return _Reader(SHELL_FILES[key])
    raise FileNotFoundError(key)

def _install(name, source):
    module = type(sys)(name)
    module.__dict__['__name__'] = name
    module.__dict__['__file__'] = name + '.py'
    module.__dict__['open'] = _open
    sys.modules[name] = module
    exec(compile(source, name + '.py', 'exec'), module.__dict__)
    return module
";

thread_local! {
    /// The document the bridge answers from. Set before the interpreter starts, read
    /// by the `_ferrocad` native module; thread-local because the VM is thread-bound.
    static DOCUMENT_JSON: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The one native function the script can see.
#[pymodule]
mod _ferrocad {
    /// The sample document's property rows, as JSON.
    #[pyfunction]
    fn document() -> rustpython_vm::PyResult<String> {
        Ok(crate::document_json())
    }
}

/// The bridge's payload: the document the Rust side built, rendered by [`snapshot`].
fn document_json() -> String {
    DOCUMENT_JSON.with(|cell| cell.borrow().clone())
}

/// The pywasm backend's script half: one interpreter, booted with
/// `python/ferrocad_shell` loaded.
pub struct PywasmScriptHost {
    interpreter: Interpreter,
}

impl PywasmScriptHost {
    /// Boot the interpreter over the sample document.
    ///
    /// The document is the sample's *content* (`Box` + `Fillet`), rendered by
    /// [`snapshot`]; the interpreter and its stdlib are per-host, so two hosts never
    /// share Python state.
    pub fn new() -> Result<Self, String> {
        let payload = snapshot::render(&snapshot::sample());
        DOCUMENT_JSON.with(|cell| *cell.borrow_mut() = payload);

        let interpreter = Interpreter::builder(Settings::default())
            .add_native_modules(
                // `rustpython-stdlib` built without `host_env`: `json`, `sys` and the
                // rest, minus the `os`/`_io` file layer the target has no POSIX
                // module for.
                &rustpython_stdlib::stdlib_module_defs(Context::genesis()),
            )
            .add_native_module(_ferrocad::module_def(Context::genesis()))
            // The frozen Python half of the standard library (`types`, `json`, …).
            .add_frozen_modules(rustpython_pylib::FROZEN_STDLIB)
            .build();

        interpreter.enter(|vm| {
            let boot = (|| -> Result<(), String> {
                run_source(vm, "<bootstrap>", BOOTSTRAP, &[])?;
                // The loader defines `_open`/`_install`; they have to run in the same
                // scope as the call, because `run_source` gives each source a fresh one.
                let install = format!(
                    "{LOADER}\nSHELL_FILES = {{'catalog.json': SHELL_CATALOG}}\n_install('ferrocad_shell', SHELL_SOURCE)"
                );
                run_source(
                    vm,
                    "<install>",
                    &install,
                    &[("SHELL_SOURCE", SHELL), ("SHELL_CATALOG", CATALOG)],
                )
            })();
            boot
        })?;

        Ok(Self { interpreter })
    }

    /// Call a module-level function of `python/ferrocad_shell` with one string
    /// argument, returning its JSON string.
    pub fn call(&self, function: &str, argument: &str) -> Result<String, String> {
        self.interpreter.enter(|vm| {
            let answer = (|| -> PyResult<String> {
                let module = vm.import("ferrocad_shell", 0)?;
                let name = vm.ctx.intern_str(function);
                let function = module.get_attr(name, vm)?;
                let value = function.call((vm.new_pyobj(argument),), vm)?;
                value.try_to_value::<String>(vm)
            })();
            answer.map_err(|error| exception_text(vm, &error))
        })
    }

    /// `ferrocad_shell.properties`: the property rows for one object, as JSON.
    ///
    /// This is the S8 acceptance: the same function the native assembly calls, over
    /// the sample document, answered by a Python interpreter.
    pub fn properties(&self, object: &str) -> Result<String, String> {
        self.call("properties", object)
    }
}

/// Compile and run `source`, with `inject` as globals.
fn run_source(
    vm: &VirtualMachine,
    name: &str,
    source: &str,
    inject: &[(&str, &str)],
) -> Result<(), String> {
    let code = vm
        .compile(source, Mode::Exec, name.to_owned())
        .map_err(|error| format!("{name}: {error}"))?;
    let scope = vm.new_scope_with_builtins();
    for (key, value) in inject {
        scope
            .globals
            .set_item(*key, vm.new_pyobj(*value), vm)
            .map_err(|error| exception_text(vm, &error))?;
    }
    vm.run_code_obj(code, scope)
        .map_err(|error| exception_text(vm, &error))?;
    Ok(())
}

/// A `PyBaseException` as text, for the seam's `String` errors.
fn exception_text(vm: &VirtualMachine, error: &PyRef<PyBaseException>) -> String {
    let mut text = String::new();
    match vm.write_exception(&mut text, error) {
        Ok(()) => text,
        Err(_) => format!("{error:?}"),
    }
}

impl ScriptHost for PywasmScriptHost {
    /// `ferrocad_shell.evaluate`: the console line, in the shell's own namespace.
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        let answer = self.call("evaluate", code)?;
        let value: serde_json::Value =
            serde_json::from_str(&answer).map_err(|error| format!("evaluate: {error}"))?;
        Ok(EvalResult {
            ok: value["ok"].as_bool().unwrap_or(false),
            output: value["output"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        // The frozen stdlib has no filesystem to scan; a real host would pass the
        // mods it loaded into the interpreter (`ferrocad_shell.add_namespace`).
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("pywasm: no tool {name} in the spike"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        // `ferrocad_shell` builds its fallback scene through `pivy.coin`, which the
        // spike does not supply; the document's own meshes are the browser's path.
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        // The `SoEventCallback` path needs `pivy`; until then the viewport forwards
        // nothing, exactly like a document-only backend.
        Box::new(NoopScriptListener)
    }

    fn commands(&self) -> Vec<CommandInfo> {
        // The shell's catalogue is `ferrocad_shell.commands()`, which the backend
        // asks for directly; a script adds no extra ids here yet.
        Vec::new()
    }

    fn run_command(&self, _id: &str) -> Option<CommandOutcome> {
        None
    }
}

/// Forwards nothing: the spike supplies no `pivy` event callbacks.
struct NoopScriptListener;

impl EventListener for NoopScriptListener {
    fn on_event(
        &mut self,
        _event: &Event,
        _hit: Option<&Hit>,
        _scene: &mut Scene,
        _directives: &ferrocad_inventor::Directives,
    ) -> bool {
        false
    }
}
