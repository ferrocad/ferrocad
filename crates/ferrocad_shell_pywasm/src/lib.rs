//! The wasm-Python scripting surface: [`ScriptHost`] over RustPython.
//!
//! The JS backend's composition ([`ferrocad_shell_js`]) is the shape here too: the
//! document half stays `ferrocad_shell_rust`'s `DocumentBackend`, and this crate is the
//! script half the shell asks for through `AppBackend::script_host()`:
//!
//! ```text
//! DocumentBackend (document)  +  PywasmScriptHost (this crate)  =  the pywasm backend
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
//! compiles for `wasm32-unknown-unknown` with `host_env` off. A **native** build turns
//! `host_env` on (a target-scoped feature, B18b item 3), which gives the desktop's
//! interpreter the real `os`/`_io` layer and the POSIX stdlib; the browser keeps the
//! feature off and `bootstrap.py`'s substitutes. §6.3's Pyodide row would have needed a
//! JS-mediated binding layer plus a ~10 MB runtime; MicroPython would have been the
//! least compatible with `python/`.
//!
//! **What is shared.** The scripts are literally `python/ferrocad_shell/__init__.py`,
//! `python/FreeCAD/` (the App facade), `python/FreeCADGui/__init__.py`,
//! `python/FerroCADGui/__init__.py` and the `python/FerroCAD` package. A native host
//! imports that tree **from disk**, so an edit is seen by the next boot without a
//! rebuild; the browser (no filesystem) installs the embedded copy through the loader.
//! The property rules are `ferrocad_core::present` (see [`snapshot`]). What is *not*
//! shared is the `*_py` PyO3 bindings: those are CPython extension modules and
//! RustPython cannot load them, so [`native`] binds the same engine types with the
//! `rustpython-vm` grammar. §6.3 calls this the fork's central trade-off, and it still
//! holds.
//!
//! **What the facade does not do.** The document is read-only from Python: creating or
//! opening one and `setattr` on an object raise rather than silently missing the
//! document. Writes, recompute and the `Shape` API are the document half's
//! (`DocumentBackend`), and the shell's own commands reach them. `bootstrap.py` supplies
//! only the gaps a `host_env`-off build has (`io`, `contextlib`, `pathlib`, `traceback`,
//! `importlib`, `os`, `tempfile`) and the wiring; with the feature on, its `_missing`
//! probes leave the real modules in place.
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
};

mod snapshot;

/// The `ferrocad` native module `python/FreeCAD` binds to `ferrocad_core`.
mod native;
// The `pivy.coin` facade over `ferrocad_inventor`: an adapter, not a port (B18b).
mod native_coin;

/// `Part`, `Mesh` and `MeshPart` over the OCCT kernel (B18b item 2).
mod native_shapes;

/// `ferrocad_stage` for RustPython: tools, shapes, snapping and the gizmo (B18b item 2).
mod native_stage;

/// `ferrocad_ui_py` for RustPython: the `.ui` proxy tree behind `PySideUic.loadUi`.
mod native_ui;

/// Where a native host imports `python/**` from (B18b item 3).
///
/// Only a build with `host_env` can import from disk, so the browser never uses this;
/// a wasm target falls back to the embedded copy. `None` means no tree was found and the
/// embedded copy is installed instead (a release binary without `python/` beside it).
#[cfg(not(target_family = "wasm"))]
mod python_tree {
    use std::path::{Path, PathBuf};

    /// `FERROCAD_PYTHON_DIR`, else a tree above the executable (a `cargo run` and an
    /// installed binary beside `python/` both resolve). The working directory is
    /// deliberately **not** searched: launching from an untrusted directory must not
    /// hand every package in it to the interpreter. In a dev build the manifest
    /// directory is the last resort, so `CARGO_TARGET_DIR` outside the tree still finds
    /// the sources the crate was compiled beside.
    pub(super) fn find() -> Option<PathBuf> {
        if let Ok(value) = std::env::var("FERROCAD_PYTHON_DIR") {
            let path = PathBuf::from(value);
            if is_tree(&path) {
                return Some(path);
            }
        }
        let mut roots = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            let mut cursor = exe.parent();
            while let Some(dir) = cursor {
                roots.push(dir.to_path_buf());
                cursor = dir.parent();
            }
        }
        #[cfg(debug_assertions)]
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
            roots.push(Path::new(manifest).to_path_buf());
        }
        roots.into_iter().find_map(|root| {
            [root.join("python"), root.join("ferrocad").join("python")]
                .into_iter()
                .find(|candidate| is_tree(candidate))
        })
    }

    /// Every package the boot imports, so a half-copied tree is refused rather than
    /// booting into an `ImportError`.
    fn is_tree(path: &Path) -> bool {
        [
            "FreeCAD",
            "FreeCADGui",
            "FerroCAD",
            "FerroCADGui",
            "ferrocad_shell",
        ]
        .iter()
        .all(|package| path.join(package).join("__init__.py").is_file())
            && path.join("ferrocad_shell").join("catalog.json").is_file()
    }
}

/// The desktop's RustPython host: a `ScriptHost` in front of a worker thread. Native
/// only — the browser boots the interpreter on its own thread and raises that stack as a
/// link argument instead.
#[cfg(not(target_family = "wasm"))]
pub mod threaded;

#[cfg(not(target_family = "wasm"))]
pub use threaded::ThreadedScriptHost;

/// What the console prints when RustPython is the script half.
///
/// The grant is part of the greeting because it is the first thing a user needs to know
/// about this console: a host that owns the registry writes, a fixed-payload host (the
/// crate's sample, a test) does not.
pub(crate) const HELLO_READ_ONLY: &str =
    "FerroCAD shell · RustPython · wasm Python scripting (document read-only)";
pub(crate) const HELLO_READ_WRITE: &str =
    "FerroCAD shell · RustPython · wasm Python scripting (document read-write)";

pub(crate) fn hello() -> String {
    let greeting = if document_writes() {
        HELLO_READ_WRITE
    } else {
        HELLO_READ_ONLY
    };
    greeting.to_string()
}

/// The refusal a half with no tools answers with.
///
/// Native-only: the browser's `RustPythonRuntime::tool` now answers through the stage
/// binding, and only the desktop worker still has a "no such tool" reply.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn no_tool(name: &str) -> String {
    format!("pywasm: no tool {name} in this build")
}

/// The scripting module: `python/ferrocad_shell`, the **same source** the native
/// assembly runs. A change there is a change here, with no port in between.
///
/// This is the copy the **browser** installs. A native host imports the file from disk
/// (B18b item 3), so the constant is the fallback for a release without `python/`.
const SHELL: &str = include_str!("../../../python/ferrocad_shell/__init__.py");

/// The generated built-in catalogue the module reads beside itself at import
/// (`_load_catalog(pathlib.Path(__file__).with_name("catalog.json"))`).
const CATALOG: &str = include_str!("../../../python/ferrocad_shell/catalog.json");

/// The `FreeCAD` surface over the bridge, and the modules `wasm32-unknown-unknown`
/// cannot import (see `bootstrap.py`).
const BOOTSTRAP: &str = include_str!("bootstrap.py");

/// The **shared** `FreeCAD` package: documents, parameters, units and Base values.
///
/// The desktop's App facade, installed rather than re-implemented; its `ferrocad`
/// dependency is the native module in `native.rs` (B18b).
const FREECAD_INIT: &str = include_str!("../../../python/FreeCAD/__init__.py");
const FREECAD_BASE: &str = include_str!("../../../python/FreeCAD/Base.py");
const FREECAD_UNITS: &str = include_str!("../../../python/FreeCAD/Units.py");
const FREECAD_CONSOLE: &str = include_str!("../../../python/FreeCAD/Console.py");
const FREECAD_VIEW_STYLE: &str = include_str!("../../../python/FreeCAD/_view_style.py");

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

/// The **shared** Draft control surface: the snap bar, `Snapper` and the snap commands.
///
/// `FreeCADGui` imports it lazily (`from . import draft`) and names `draft.py` as the
/// reason the Draft tool works; installing it here is what makes
/// `ferrocad_shell.tool_bridge('draft')` and `FreeCADGui.Snapper` real on this host too
/// (the stage extension it needs is `native_stage`).
const FREECADGUI_DRAFT: &str = include_str!("../../../python/FreeCADGui/draft.py");

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
import Part as _part
import Mesh as _mesh
import MeshPart as _meshpart
import ferrocad as _ferrocad
import ferrocad_stage as _stage
# `ferrocad_stage.install()` attaches this on CPython; mirror it here so
# `from ferrocad import stage` resolves on both runtimes.
_ferrocad.stage = _stage
ferrocad_shell.add_namespace('FreeCAD', _fc)
ferrocad_shell.add_namespace('App', _fc)
ferrocad_shell.add_namespace('doc', _fc.ActiveDocument)
ferrocad_shell.add_namespace('Part', _part)
ferrocad_shell.add_namespace('Mesh', _mesh)
ferrocad_shell.add_namespace('MeshPart', _meshpart)
";

/// Registers `pivy` / `pivy.coin`, the modules a Coin workbench imports.
///
/// The native module is `coin`; upstream reaches it as `pivy.coin`, so the package is
/// the same two-entry alias the PyO3 facade's `install()` creates.
const PIVY: &str = "\
import sys
import types as _types
import coin as _coin
_pivy = _types.ModuleType('pivy')
_pivy.__dict__['coin'] = _coin
sys.modules['pivy'] = _pivy
sys.modules['pivy.coin'] = _coin
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
    /// by the `ferrocad` native module; thread-local because the VM is thread-bound.
    static DOCUMENT: RefCell<Option<DocumentSource>> = const { RefCell::new(None) };
}

/// Where the Python parameter store persists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParameterStore {
    /// Process memory only: the browser, and every embedded host.
    Memory,
    /// The facade's file: `FERROCAD_PARAM_FILE`, else the platform config path.
    File,
}

/// The file `python/FreeCAD._parameter_file_path` resolves to, computed the same way:
/// an explicit `FERROCAD_PARAM_FILE`, else `XDG_CONFIG_HOME`/`~/.config` (or `APPDATA`).
fn parameter_file() -> Option<String> {
    if let Ok(path) = std::env::var("FERROCAD_PARAM_FILE") {
        if !path.trim().is_empty() {
            return Some(path);
        }
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| format!("{home}/.config"))
        })
        .or_else(|| std::env::var("APPDATA").ok())?;
    Some(format!("{base}/ferrocad/parameters.json"))
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

/// Whether this host's document is the process-wide one the doorway writes.
///
/// A [`DocumentSource::Fixed`] host answers from a payload nobody can edit: the
/// documented object there is a projection, not the registry's. Writes would land in the
/// registry (a *different* document), so the native module refuses them instead, and
/// [`RustPythonRuntime::document_grant`] reports `Read` for that host.
pub(crate) fn document_writes() -> bool {
    matches!(
        DOCUMENT.with(|cell| cell.borrow().clone()),
        Some(DocumentSource::Registry)
    )
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

/// [`new`] with the facade's file-backed parameter store (native; for tests).
pub fn new_persistent() -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::new_persistent().map(PythonHost::new)
}

/// Boot over the process-wide registry, read on every call.
///
/// The document half must be the shared-registry one
/// (`DocumentBackend::from_shared_registry`) or the registry holds nothing and the facade
/// reports no active document. That pairing is what makes `FreeCAD.ActiveDocument` and
/// the model tree the same object, and what keeps the cascade current after an open.
///
/// The parameter store is **memory-only**: this is what the browser gets (it has no
/// filesystem), and what a test gets (it must not touch the user's config). A desktop
/// preview that wants the app's store uses [`live_persistent`].
pub fn live() -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::live().map(PythonHost::new)
}

/// [`live`] with the facade's **file-backed** parameter store.
///
/// For the desktop `--script-host=pywasm`, so the preview reads the same store the
/// CPython half writes (`FERROCAD_PARAM_FILE`, else the platform config file). The
/// browser never takes this path — it has no filesystem.
pub fn live_persistent() -> Result<PywasmScriptHost, String> {
    RustPythonRuntime::live_persistent().map(PythonHost::new)
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
/// The facade is **the desktop's own Python**, installed from source: `FreeCAD`,
/// `FreeCADGui`, `FerroCADGui` and the `FerroCAD` package, over the native module in
/// [`native`]. `bootstrap.py` supplies only the host gaps, so there is one implementation
/// of the App and Gui facades on both runtimes. The [`Contract`] names what that facade
/// answers:
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
///   parameter store: the process for a memory host, the app's file for a desktop one.
/// - `settings` — `extra_properties`/`set_extra_property` over the
///   `Settings/<Feature>` namespace: the provider selector row and the active
///   provider's schema. A written value goes through `FreeCAD.ParamGet` (memory or the
///   host's file, per `ParameterStore`), coercion and range/choice checks are the
///   native ones, `resetAppearance` drops the written values, and a write returns the
///   provider's `refresh` view claim.
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
    interpreter: Rc<Interpreter>,
}

impl RustPythonRuntime {
    /// Boot over the canonical sample document.
    pub fn new() -> Result<Self, String> {
        let sample = snapshot::sample()?;
        Self::over(snapshot::render(&sample))
    }

    /// [`new`](Self::new) with the facade's file-backed parameter store.
    pub fn new_persistent() -> Result<Self, String> {
        let sample = snapshot::sample()?;
        Self::boot(
            DocumentSource::Fixed(Rc::from(snapshot::render(&sample))),
            ParameterStore::File,
        )
    }

    /// Boot over the process-wide registry, read on every call.
    pub fn live() -> Result<Self, String> {
        Self::boot(DocumentSource::Registry, ParameterStore::Memory)
    }

    /// [`live`](Self::live) with the facade's file-backed parameter store.
    ///
    /// The browser has no filesystem (`host_env` is off), so the store stays in memory
    /// there; a native host resolves the app's parameter file.
    pub fn live_persistent() -> Result<Self, String> {
        Self::boot(DocumentSource::Registry, ParameterStore::File)
    }

    /// Boot over a payload rendered from a document the caller built.
    pub fn over(payload: impl Into<String>) -> Result<Self, String> {
        let payload: Rc<str> = Rc::from(payload.into());
        Self::boot(DocumentSource::Fixed(payload), ParameterStore::Memory)
    }

    /// [`over`](Self::over) with the facade's file-backed parameter store.
    pub fn over_persistent(payload: impl Into<String>) -> Result<Self, String> {
        let payload: Rc<str> = Rc::from(payload.into());
        Self::boot(DocumentSource::Fixed(payload), ParameterStore::File)
    }

    /// Start one interpreter over `source`.
    ///
    /// The interpreter and its stdlib are per-host, so two hosts never share Python
    /// state.
    fn boot(source: DocumentSource, store: ParameterStore) -> Result<Self, String> {
        DOCUMENT.with(|cell| *cell.borrow_mut() = Some(source));
        // No POSIX module on wasm, so no file even if the caller asked for one.
        let store = if cfg!(target_family = "wasm") {
            ParameterStore::Memory
        } else {
            store
        };
        let parameter_file = match store {
            ParameterStore::File => parameter_file(),
            ParameterStore::Memory => None,
        };

        let interpreter = Interpreter::builder(Settings::default())
            .add_native_modules(
                // `rustpython-stdlib`: `json`, `sys` and the rest. On a native build
                // `host_env` adds the `os`/`_io` file layer; on wasm it is off and
                // `bootstrap.py` supplies the names the shared facade touches.
                &rustpython_stdlib::stdlib_module_defs(Context::genesis()),
            )
            .add_native_module(native::module_def(Context::genesis()))
            // `pivy.coin`: the scene-graph facade the Coin workbenches import, over the
            // same `ferrocad_inventor` the CPython facade binds. Installed as a `pivy`
            // package below, so `from pivy import coin` works here too.
            .add_native_module(native_coin::coin_module_def(Context::genesis()))
            // The 3D kernel a Python script builds with: one `ferrocad_geom::Shape`,
            // the same OCCT backend the document half registered.
            .add_native_module(native_shapes::part_module_def(Context::genesis()))
            .add_native_module(native_shapes::mesh_module_def(Context::genesis()))
            .add_native_module(native_shapes::mesh_part_module_def(Context::genesis()))
            // The interactive stage: tools, `ShapeItem`, snapping and the gizmo, so a
            // mod's tool and a script's preview are the ones the viewport draws.
            .add_native_module(native_stage::stage_module_def(Context::genesis()))
            // The typed form tree `FreeCADGui.PySideUic.loadUi` imports: a workbench's
            // `.ui` panel is the same proxy tree on both runtimes.
            .add_native_module(native_ui::ui_module_def(Context::genesis()))
            // The frozen Python half of the standard library (`types`, `json`, …).
            .add_frozen_modules(rustpython_pylib::FROZEN_STDLIB)
            .build();

        #[cfg(not(target_family = "wasm"))]
        let python_tree = python_tree::find();
        #[cfg(target_family = "wasm")]
        let python_tree: Option<std::path::PathBuf> = None;

        interpreter.enter(|vm| {
            let boot = (|| -> Result<(), String> {
                run_source(vm, "<bootstrap>", BOOTSTRAP, &[])?;
                let store = match store {
                    ParameterStore::Memory => "memory",
                    ParameterStore::File => "file",
                };
                // A native host with a `python/` tree imports the shared facade **from
                // disk**, so an edit is picked up by the next boot without a rebuild
                // (B18b item 3; this is what `host_env` buys). The browser has no
                // filesystem, so it installs the embedded copy through the loader.
                let install = match &python_tree {
                    Some(tree) => format!(
                        "import sys\n\
                         sys.path.insert(0, {tree:?})\n\
                         import FreeCAD\n\
                         sys.modules['_ferrocad_bootstrap'].configure(FreeCAD, {store:?}, {parameter:?})\n\
                         import FreeCADGui\n\
                         import FerroCAD\n\
                         import FerroCADGui\n\
                         import ferrocad_shell",
                        parameter = parameter_file.as_deref().unwrap_or(""),
                    ),
                    // The loader defines `_open`/`_install`; they have to run in the
                    // same scope as the call, because `run_source` gives each source a
                    // fresh one.
                    None => format!(
                        "{LOADER}\nSHELL_FILES = {{'catalog.json': SHELL_CATALOG}}\n\
                         _install_package('FreeCAD', eval(FREECAD_FILES_SOURCE))\n\
                         sys.modules['_ferrocad_bootstrap'].configure(sys.modules['FreeCAD'], PARAMETER_STORE, PARAMETER_FILE)\n\
                         _install('FreeCADGui', FREECADGUI_SOURCE)\n\
                         _install('FreeCADGui.draft', FREECADGUI_DRAFT_SOURCE)\n\
                         _install_package('FerroCAD', eval(FERROCAD_FILES_SOURCE))\n\
                         _install('FerroCADGui', FERROCADGUI_SOURCE)\n\
                         _install('ferrocad_shell', SHELL_SOURCE)"
                    ),
                };
                // A Python dict of each package's sources. `{:?}` renders a Rust string
                // as a quoted literal (newlines and quotes escaped), which is also a
                // Python one. Unused when the tree was imported from disk.
                let ferro_files = format!(
                    "{{'__init__': {FERROCAD_INIT:?}, '_params': {FERROCAD_PARAMS:?}, '_grid': {FERROCAD_GRID:?}, '_axis': {FERROCAD_AXIS:?}, '_gizmo': {FERROCAD_GIZMO:?}}}"
                );
                let freecad_files = format!(
                    "{{'__init__': {FREECAD_INIT:?}, 'Base': {FREECAD_BASE:?}, 'Units': {FREECAD_UNITS:?}, 'Console': {FREECAD_CONSOLE:?}, '_view_style': {FREECAD_VIEW_STYLE:?}}}"
                );
                run_source(
                    vm,
                    "<install>",
                    &install,
                    &[
                        ("SHELL_SOURCE", SHELL),
                        ("SHELL_CATALOG", CATALOG),
                        ("FREECADGUI_SOURCE", FREECADGUI),
                        ("FREECADGUI_DRAFT_SOURCE", FREECADGUI_DRAFT),
                        ("FERROCADGUI_SOURCE", FERROCADGUI),
                        ("FERROCAD_FILES_SOURCE", ferro_files.as_str()),
                        ("FREECAD_FILES_SOURCE", freecad_files.as_str()),
                        ("PARAMETER_STORE", store),
                        ("PARAMETER_FILE", parameter_file.as_deref().unwrap_or("")),
                    ],
                )?;
                run_source(vm, "<pivy>", PIVY, &[])?;
                run_source(vm, "<namespace>", NAMESPACE, &[])
            })();
            boot
        })?;

        Ok(Self {
            interpreter: Rc::new(interpreter),
        })
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

    /// Install a fetched pack's resources where the loader's `__file__` points.
    ///
    /// The interpreter has no filesystem here (`host_env` is off on wasm), so the host
    /// keeps the text and `bootstrap.py`'s `open` serves it.
    fn install_pack_files(&self, name: &str, files: &[(String, String)]) -> Result<(), String> {
        crate::native::install_pack_files(name, files);
        Ok(())
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

    /// Read-write when the host installed the process-wide document (B18b item 5).
    ///
    /// The gateway is the native module: `Document.addObject`/`recompute`, a property
    /// assignment and the transaction no-ops all call `ferrocad_doc_api`'s doorway over
    /// the registry — the same calls the JS and bundle halves make, so a script's write
    /// is the one the model tree and the stage read afterwards.
    ///
    /// A [`DocumentSource::Fixed`](DocumentSource) host (the crate's sample, and a test)
    /// keeps `Read`: its document is a payload nobody else can see, so the doorway would
    /// write somewhere else.
    fn document_grant(&self) -> DocumentGrant {
        if crate::document_writes() {
            DocumentGrant::ReadWrite
        } else {
            DocumentGrant::Read
        }
    }

    /// The console's greeting: this is an interpreter, unlike the plain Rust half.
    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(crate::hello()))
    }

    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String> {
        self.interpreter.enter(|vm| {
            let bridge = (|| -> PyResult<PyObjectRef> {
                let module = vm.import("ferrocad_shell", 0)?;
                let function = module.get_attr(vm.ctx.intern_str("tool_bridge"), vm)?;
                function.call((vm.new_pyobj(name),), vm)
            })();
            match bridge {
                Ok(object) => native_stage::extract_tool(self.interpreter.clone(), object)
                    .map_err(|error| exception_text(vm, &error)),
                Err(error) => Err(exception_text(vm, &error)),
            }
        })
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        self.interpreter.enter(|vm| {
            let answer = vm
                .import("ferrocad_shell", 0)
                .and_then(|module| module.get_attr(vm.ctx.intern_str("tool_command"), vm))
                .and_then(|function| function.call((vm.new_pyobj(name), vm.new_pyobj(verb)), vm));
            match answer {
                Ok(value) => value.is_true(vm).unwrap_or(false),
                Err(error) => {
                    vm.print_exception(&error);
                    false
                }
            }
        })
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
