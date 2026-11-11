//! The wasm-Python scripting surface: [`ScriptHost`] over RustPython.
//!
//! The JS backend's composition ([`ferrocad_shell_js`]) is the shape here too: the
//! document half stays `ferrocad_shell_rust`'s `RustAppBackend`, and this crate is the
//! script half the shell asks for through `AppBackend::script_host()`:
//!
//! ```text
//! RustAppBackend (document)  +  PywasmScriptHost (this crate)  =  the pywasm backend
//! ```
//!
//! [`PywasmScriptHost::live`] is what the browser host composes: it reads the active
//! document from `ferrocad_core`'s registry on every call, so the console sees the
//! document the shell is showing (`FreeCAD.ActiveDocument`), including after a file is
//! opened. [`PywasmScriptHost::new`] boots over a fixed payload instead — the
//! geometry-free sample — which is what a host with no registry and the crate's own
//! test use.
//!
//! **The runtime is RustPython**, the second option in §6.3. It is a Python VM written
//! in Rust: no C ABI, no Pyodide download, no COOP/COEP, and — unlike Pyodide — it
//! compiles for `wasm32-unknown-unknown` with `host_env` off. §6.3's Pyodide row would
//! have needed a JS-mediated binding layer plus a ~10 MB runtime; MicroPython would
//! have been the least compatible with `python/`.
//!
//! **What is shared.** The scripts are literally `python/ferrocad_shell/__init__.py`,
//! `python/FreeCADGui/__init__.py`, `python/FerroCADGui/__init__.py` and the
//! `python/FerroCAD` package, included as source; the property rules are
//! `ferrocad_core::present` (see [`snapshot`]). What is *not* shared is the `*_py` PyO3
//! bindings: those are CPython extension modules and RustPython cannot load them, so the
//! bridge re-exposes exactly the rows the shell needs. §6.3 calls this the fork's
//! central trade-off, and it still holds.
//!
//! **What the facade does not do.** The document is read-only from Python: `setattr`
//! on an object raises rather than silently missing the document. Writes, recompute
//! and the `Shape` API are the document half's (`RustAppBackend`), and the shell's own
//! commands reach them. `bootstrap.py` supplies only what the shared modules cannot get
//! here: the host gaps, the document model, and the bricks they sit on (`ParamGet`, the
//! engine's `Quantity`, the `ferrocad` property rules, the parameter-file controls).
//!
//! **Why this crate also builds natively.** `ferrocad_shell_js`'s browser page is
//! `cfg(wasm)` because it is a `window` bridge, but the half above it is not; nor is
//! RustPython. This crate is plain Rust and its tests run on a normal
//! `cargo test` — the wasm target is proven separately by
//! `cargo check --target wasm32-unknown-unknown`. Both are in the acceptance.

use std::cell::RefCell;
use std::rc::Rc;

use ferrocad_inventor::Scene;
use ferrocad_python_host::{Contract, PythonHost, PythonRuntime};
use ferrocad_shell_api::DocumentGrant;
use ferrocad_stage::Tool;
use rustpython_vm::PyObjectRef;
use rustpython_vm::compiler::Mode;
use rustpython_vm::function::FuncArgs;
use rustpython_vm::{
    Context, Interpreter, PyRef, PyResult, Settings, VirtualMachine, builtins::PyBaseException,
    pymodule,
};

mod snapshot;

/// The desktop's RustPython host: a `ScriptHost` in front of a worker thread. Native
/// only — the browser boots the interpreter on its own thread and raises that stack as a
/// link argument instead.
#[cfg(not(target_family = "wasm"))]
pub mod threaded;

#[cfg(not(target_family = "wasm"))]
pub use threaded::ThreadedScriptHost;

/// What the console prints when RustPython is the script half.
pub(crate) const HELLO: &str =
    "FerroCAD shell · RustPython · wasm Python scripting (document read-only)";

/// The refusal a half with no tools answers with.
pub(crate) fn no_tool(name: &str) -> String {
    format!("pywasm: no tool {name} in this build")
}

/// The scripting module: `python/ferrocad_shell`, the **same source** the native
/// assembly runs. A change there is a change here, with no port in between.
const SHELL: &str = include_str!("../../../python/ferrocad_shell/__init__.py");

/// The generated built-in catalogue the module reads beside itself at import
/// (`_load_catalog(pathlib.Path(__file__).with_name("catalog.json"))`).
const CATALOG: &str = include_str!("../../../python/ferrocad_shell/catalog.json");

/// The `FreeCAD` surface over the bridge, and the modules `wasm32-unknown-unknown`
/// cannot import (see `bootstrap.py`).
const BOOTSTRAP: &str = include_str!("bootstrap.py");

/// The **shared** `FerroCADGui`: the same source the desktop's CPython half loads.
///
/// Installed over the bootstrap stubs once the bricks it needs (`ParamGet`, the engine's
/// `Quantity`, `os`) exist, so the provider registry, settings and view state cannot
/// drift from the desktop's (B18b).
const FERROCADGUI: &str = include_str!("../../../python/FerroCADGui/__init__.py");

/// The **shared** `FreeCADGui`: the command registry, selection mirror, dialogs and
/// document observers the desktop's CPython half loads.
///
/// It imports `os`, which is why bootstrap installs a synthetic one; with that it is the
/// same source on both hosts (B18b).
const FREECADGUI: &str = include_str!("../../../python/FreeCADGui/__init__.py");

/// The **shared** `FerroCAD` package: the semantic stores the desktop runs.
///
/// A package, not one source: `FerroCAD/__init__.py` lazily imports `._grid`, `._axis`
/// and `._gizmo`, and `._params` is the parameter-file control. All four are installed
/// so the store policy (schema-validated setters, the view mirror, reset) is one copy.
const FERROCAD_INIT: &str = include_str!("../../../python/FerroCAD/__init__.py");
const FERROCAD_PARAMS: &str = include_str!("../../../python/FerroCAD/_params.py");
const FERROCAD_GRID: &str = include_str!("../../../python/FerroCAD/_grid.py");
const FERROCAD_AXIS: &str = include_str!("../../../python/FerroCAD/_axis.py");
const FERROCAD_GIZMO: &str = include_str!("../../../python/FerroCAD/_gizmo.py");

/// Seed the console's namespace, the way `ferrocad_shell.bootstrap` does natively.
///
/// Here the *document* half boots, so the shell script's `bootstrap()` never runs and
/// the prompt would have no `FreeCAD`, `App` or `doc`. The values are the facade (and
/// the live document proxy), not snapshots, so they keep working after an open.
const NAMESPACE: &str = "\
import ferrocad_shell
import FreeCAD as _fc
ferrocad_shell.add_namespace('FreeCAD', _fc)
ferrocad_shell.add_namespace('App', _fc)
ferrocad_shell.add_namespace('doc', _fc.ActiveDocument)
";

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
    module.__dict__['__package__'] = name.rpartition('.')[0]
    module.__dict__['open'] = _open
    sys.modules[name] = module
    exec(compile(source, name + '.py', 'exec'), module.__dict__)


def _install_package(name, sources):
    # Install a package: submodules first, then `sources['__init__']`. The shared
    # modules use relative imports (`from ._params import ...`), which resolve through
    # sys.modules when `__path__` exists, so the package module is created first.
    module = type(sys)(name)
    module.__dict__['__name__'] = name
    module.__dict__['__file__'] = name + '/__init__.py'
    module.__dict__['__package__'] = name
    module.__dict__['__path__'] = []
    sys.modules[name] = module
    for key, source in sources.items():
        if key != '__init__':
            _install(name + '.' + key, source)
    exec(compile(sources['__init__'], name + '/__init__.py', 'exec'), module.__dict__)
    return module
";

/// Where the bridge reads the document from.
#[derive(Clone)]
enum DocumentSource {
    /// A payload supplied at boot: a host with no registry, and the test.
    Fixed(Rc<str>),
    /// The process-wide registry in `ferrocad_core`, read afresh on every call.
    Registry,
}

thread_local! {
    /// The document the bridge answers from. Set before the interpreter starts, read
    /// by the `_ferrocad` native module; thread-local because the VM is thread-bound.
    static DOCUMENT: RefCell<Option<DocumentSource>> = const { RefCell::new(None) };
}

/// The native module the script can see: the document rows, and the one value class the
/// shared Gui modules need from the engine.
#[pymodule]
mod _ferrocad {
    use ferrocad_core::quantity::Quantity;
    use ferrocad_core::unit::Unit;
    use rustpython_vm::builtins::{PyStrRef, PyType};
    use rustpython_vm::types::Constructor;
    use rustpython_vm::{Py, PyPayload, PyResult, VirtualMachine, pyclass};

    /// The active document's property rows, as JSON (`null` when none is open).
    #[pyfunction]
    fn document() -> rustpython_vm::PyResult<String> {
        Ok(crate::document_json())
    }

    /// `FreeCAD.Units.Quantity`, over `ferrocad_core`'s parser, normalisation and unit
    /// table.
    ///
    /// The shared `FerroCADGui` settings coercion uses exactly this much: construction
    /// from a number or a text expression, `Value`, `getValueAs` and `str`. Binding the
    /// engine type (rather than writing a second unit model in `bootstrap.py`) is what
    /// makes "5 cm" mean the same thing on both runtimes: a conversion it cannot make
    /// is refused here for the same reason and with the same words as on the desktop.
    #[pyattr]
    #[pyclass(name = "Quantity", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    struct PyQuantity {
        inner: Quantity,
    }

    impl Constructor for PyQuantity {
        type Args = rustpython_vm::function::FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let mut positional = args.args;
            match positional.len() {
                0 => Ok(Self {
                    inner: Quantity::dimensionless(0.0),
                }),
                1 => {
                    let value = positional.remove(0);
                    let inner = if let Some(copy) = value.downcast_ref::<Self>() {
                        copy.inner
                    } else if let Ok(text) = value.try_to_value::<String>(vm) {
                        Quantity::parse(&text).map_err(|error| vm.new_value_error(error))?
                    } else {
                        Quantity::dimensionless(value.try_into_value::<f64>(vm)?)
                    };
                    Ok(Self { inner })
                }
                2 => {
                    let number = positional[0].clone().try_into_value::<f64>(vm)?;
                    let symbol = positional[1].try_to_value::<String>(vm)?;
                    let unit = Unit::parse(&symbol)
                        .ok_or_else(|| vm.new_value_error(format!("unknown unit '{symbol}'")))?;
                    Ok(Self {
                        inner: Quantity::new(number, unit),
                    })
                }
                _ => Err(vm.new_type_error("Quantity() takes 0, 1 or 2 arguments")),
            }
        }
    }

    #[pyclass(with(Constructor))]
    #[allow(non_snake_case)]
    impl PyQuantity {
        /// FreeCAD's `.Value`: the number in this quantity's own unit.
        #[pygetset]
        fn Value(&self) -> f64 {
            self.inner.value()
        }

        /// The value expressed in `unit`; a different signature is an error, as it is
        /// natively.
        #[pymethod]
        fn getValueAs(&self, unit: String, vm: &VirtualMachine) -> PyResult<Self> {
            let unit = Unit::parse(&unit)
                .ok_or_else(|| vm.new_value_error(format!("unknown unit '{unit}'")))?;
            if unit.sig != self.inner.sig() {
                return Err(vm.new_value_error("incompatible unit for getValueAs"));
            }
            Ok(Self {
                inner: self.inner.in_unit(unit),
            })
        }

        /// The display string (`"10 mm"`), from the engine's canonical formatting.
        #[pymethod]
        fn __str__(&self, vm: &VirtualMachine) -> PyStrRef {
            vm.ctx.new_str(self.inner.user_string())
        }
    }
}

/// The bridge's payload: the document the Rust side built, rendered by [`snapshot`].
///
/// `null` when the source is the registry and no document is open, which the facade
/// turns into `FreeCAD.ActiveDocument is None` rather than a sample nobody installed.
fn document_json() -> String {
    let source = DOCUMENT.with(|cell| cell.borrow().clone());
    match source {
        Some(DocumentSource::Fixed(payload)) => payload.to_string(),
        Some(DocumentSource::Registry) | None => snapshot::active(),
    }
}

/// The pywasm backend's script half: RustPython behind the shared `ScriptHost`.
///
/// The seam itself (all 35 methods, the JSON marshalling and the contract) is
/// [`ferrocad_python_host::PythonHost`], the same type the embedded CPython half wraps;
/// this type only names the engine.
pub type PywasmScriptHost = PythonHost<RustPythonRuntime>;

/// Boot over a fixed payload: the geometry-free sample document.
///
/// This is the crate's own test and any host with no document registry. The browser host
/// uses [`live`] instead, so its facade follows the document the shell actually holds.
pub fn new() -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::new().map(PythonHost::new)
}

/// Boot over the process-wide registry, read on every call.
///
/// The document half must be the shared-registry one
/// (`RustAppBackend::from_shared_registry`) or the registry holds nothing and the facade
/// reports no active document. That pairing is what makes `FreeCAD.ActiveDocument` and
/// the model tree the same object, and what keeps the cascade current after an open.
pub fn live() -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::live().map(PythonHost::new)
}

/// Boot over a payload rendered from a document the caller built.
///
/// For a host that owns a document outside the registry, and for tests that want a known
/// shape without touching process-wide state.
pub fn over(payload: impl Into<String>) -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::over(payload).map(PythonHost::new)
}

/// RustPython as a [`PythonRuntime`]: the interpreter, and what its facade answers.
///
/// The facade is **the desktop's own Python**, installed from source: `FreeCADGui`,
/// `FerroCADGui` and the `FerroCAD` package. `bootstrap.py` supplies only the host gaps,
/// the document model and the four bricks they need, so there is one implementation of
/// the Gui app on both runtimes. The [`Contract`] names what that facade answers:
///
/// - `view` — `appearance_view`, `grid_summary`, `axis_summary` over the shared
///   `FerroCAD.Grid`/`Axis`/`Gizmo` stores. An unset parameter-group value falls back to
///   the shell's mirrored `ViewState`, which is what is drawn.
/// - `view_state` — `set_view_state`, the push a checkable command's `IsChecked` reads.
/// - `commands` — `commands`, `run_command` and the two dialog verbs over the shared
///   `FreeCADGui._COMMANDS` registry the shell module fills at import (`App_ToggleGrid`,
///   `App_ToggleAxisCross`, `App_ToggleGizmo`, `App_ViewWorkplane`, `View_GridSettings`,
///   `Dev_ToggleTools`, and every workbench command a mod registers).
/// - `selection` — `set_selection`/`take_selection_requests` over the shared
///   `FreeCADGui.Selection`: the shell pushes what it has selected, a script's
///   `addSelection`/`removeSelection`/`clearSelection` updates that snapshot and queues
///   the write for the shell to drain. `Document.UndoCount`/`RedoCount` answer from the
///   payload's counts, so `App_Undo`, `App_Redo`, `App_Delete`, `App_Move`,
///   `App_Rotate` and `App_ToggleVisibility` are judged by their real predicates.
/// - `providers` — `provider_features`, `set_provider`, `refresh_providers` and
///   `provider_skin` over the shared registry. The selected backend persists in the
///   dict-backed `ParamGet` (the process, not a file), so a reload starts from priority.
/// - `settings` — `extra_properties`/`set_extra_property` over the
///   `Settings/<Feature>` namespace: the provider selector row and the active
///   provider's schema. A written value goes through `FreeCAD.ParamGet` (the
///   dict-backed store, so it is session-only — B18b), coercion and range/choice
///   checks are the native ones, `resetAppearance` drops the written values, and a
///   write returns the provider's `refresh` view claim.
/// - `tree` — `model_tree`, filtered to the synthetic `Settings` root the shared
///   provider registry lists. The documents themselves stay the document half's, so
///   nothing is shown twice.
/// - `interaction` — `interaction` and `manipulator_specs` over the shared
///   `FerroCAD.register_interaction_provider`/`register_manipulator_provider`
///   registries, so a Python mod that takes over a commit is answered here on both
///   hosts. `add_datum_plane` is **not** this half's: the edition's app script exports
///   it and this runtime installs none, so [`PythonRuntime::has_function`] reports it
///   absent and the document half answers.
pub struct RustPythonRuntime {
    interpreter: Interpreter,
}

impl RustPythonRuntime {
    /// Boot over the geometry-free sample document.
    pub fn new() -> Result<Self, String> {
        Self::over(snapshot::render(&snapshot::sample()))
    }

    /// Boot over the process-wide registry, read on every call.
    pub fn live() -> Result<Self, String> {
        Self::boot(DocumentSource::Registry)
    }

    /// Boot over a payload rendered from a document the caller built.
    pub fn over(payload: impl Into<String>) -> Result<Self, String> {
        let payload: Rc<str> = Rc::from(payload.into());
        Self::boot(DocumentSource::Fixed(payload))
    }

    /// Start one interpreter over `source`.
    ///
    /// The interpreter and its stdlib are per-host, so two hosts never share Python
    /// state.
    fn boot(source: DocumentSource) -> Result<Self, String> {
        DOCUMENT.with(|cell| *cell.borrow_mut() = Some(source));

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
                    "{LOADER}\nSHELL_FILES = {{'catalog.json': SHELL_CATALOG}}\n\
                     _install('FreeCADGui', FREECADGUI_SOURCE)\n\
                     _install_package('FerroCAD', eval(FERROCAD_FILES_SOURCE))\n\
                     _install('FerroCADGui', FERROCADGUI_SOURCE)\n\
                     _install('ferrocad_shell', SHELL_SOURCE)"
                );
                // A Python dict of the package's sources. `{:?}` renders a Rust string
                // as a quoted literal (newlines and quotes escaped), which is also a
                // Python one.
                let ferro_files = format!(
                    "{{'__init__': {FERROCAD_INIT:?}, '_params': {FERROCAD_PARAMS:?}, '_grid': {FERROCAD_GRID:?}, '_axis': {FERROCAD_AXIS:?}, '_gizmo': {FERROCAD_GIZMO:?}}}"
                );
                run_source(
                    vm,
                    "<install>",
                    &install,
                    &[
                        ("SHELL_SOURCE", SHELL),
                        ("SHELL_CATALOG", CATALOG),
                        ("FREECADGUI_SOURCE", FREECADGUI),
                        ("FERROCADGUI_SOURCE", FERROCADGUI),
                        ("FERROCAD_FILES_SOURCE", ferro_files.as_str()),
                    ],
                )?;
                run_source(vm, "<namespace>", NAMESPACE, &[])
            })();
            boot
        })?;

        Ok(Self { interpreter })
    }
}

impl PythonRuntime for RustPythonRuntime {
    fn call(&self, function: &str, args: &[&str]) -> Result<String, String> {
        self.interpreter.enter(|vm| {
            let answer = (|| -> PyResult<String> {
                let module = vm.import("ferrocad_shell", 0)?;
                let name = vm.ctx.intern_str(function);
                let function = module.get_attr(name, vm)?;
                let args: Vec<PyObjectRef> = args.iter().map(|arg| vm.new_pyobj(*arg)).collect();
                let value = function.call(
                    FuncArgs {
                        args,
                        kwargs: Default::default(),
                    },
                    vm,
                )?;
                value.try_to_value::<String>(vm)
            })();
            answer.map_err(|error| exception_text(vm, &error))
        })
    }

    /// There is no app script here: `ferrocad_shell` is the only Python module this
    /// runtime installs, so the edition's `add_datum_plane` is absent by construction.
    fn has_function(&self, function: &str) -> bool {
        self.interpreter.enter(|vm| {
            let name = vm.ctx.intern_str(function);
            vm.import("ferrocad_shell", 0)
                .and_then(|module| module.get_attr(name, vm))
                .is_ok()
        })
    }

    /// The presentation, selection, provider and settings half of the Gui app, the
    /// model tree's `Settings` root, and a mod's interaction hooks: the shell's own view
    /// commands run here, a script's selection writes reach it, a mod's providers are
    /// listed and switched, a provider's settings card is answered, and a manipulator a
    /// mod declared commits through the shared registry (see the type docs).
    fn contract(&self) -> Contract {
        Contract {
            tree: true,
            view: true,
            view_state: true,
            commands: true,
            selection: true,
            providers: true,
            settings: true,
            interaction: true,
            ..Contract::NONE
        }
    }

    /// Read-only: `_ferrocad` exposes one function, and it reads.
    fn document_grant(&self) -> DocumentGrant {
        DocumentGrant::Read
    }

    /// The console's greeting: this is an interpreter, unlike the plain Rust half.
    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(HELLO.to_string()))
    }

    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String> {
        Err(no_tool(name))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        // `ferrocad_shell` builds its fallback scene through `pivy.coin`, which the
        // facade does not supply; the document's own meshes are the browser's path.
        Ok(Scene::default())
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
