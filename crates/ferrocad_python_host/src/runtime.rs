//! What differs between the interpreters: how to call one, and what it supports.
//!
//! Everything a Python script half does that is not "ask the shell module a question"
//! lives here, so [`PythonHost`](crate::PythonHost) can answer the whole
//! [`ScriptHost`](ferrocad_shell_api::app::ScriptHost) surface once.
//!
//! The three groups below are the *whole* difference between the embedded CPython and
//! the RustPython half:
//!
//! 1. **[`call`](PythonRuntime::call)** — the one primitive. CPython enters the GIL and
//!    calls the module; RustPython calls a function on its `VirtualMachine`. Everything
//!    else in the shell contract is JSON in, JSON out, so it is written once.
//! 2. **The boot and file hooks** (`bootstrap`, `open_startup_document`, `open_path`,
//!    `save_path`, `activate_document`, `mods_roots`) — an interpreter with no
//!    filesystem cannot read a `.FCStd` or scan a `mods/` tree, and a console-only
//!    interpreter has no app script to run.
//! 3. **The typed hooks** (`stage_meshes`, `tool`, `inventor_scene`,
//!    `event_listener`) — these hand Python objects over rather than JSON text, which
//!    is where a binding layer is worth the cost (the display path is measured: 1.3 s
//!    of text against 35 ms of typed rows on a 1.24 M-triangle document).
//!
//! [`Contract`] is the fourth difference, and it is deliberately a *value*: which
//! groups of the shell contract this facade can answer. Asking a facade a question it
//! cannot answer would raise on the hot paths (the model tree and the selection mirror
//! are refreshed constantly), so an unsupported group is not called at all and the
//! document half answers, exactly as it did before the halves shared a host.

use std::path::PathBuf;

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::DocumentGrant;
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::types::{Bootstrap, OpenedDocument};
use ferrocad_stage::Tool;

/// The groups of the shell contract a facade implements.
///
/// One flag per group rather than per function: a facade that has `model_tree` has the
/// selection mirror too, because both come from the same app script. [`Self::FULL`] is
/// the embedded CPython facade; RustPython grows a flag as its facade gains the group
/// (B18a), and it now answers every one of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contract {
    /// `model_tree` — the synthetic `Settings` root and its children.
    pub tree: bool,
    /// `set_selection` / `take_selection_requests` — the facade's selection mirror.
    pub selection: bool,
    /// `grid_summary`, `axis_summary`, `appearance_view`, and the GPU preference.
    pub view: bool,
    /// `set_view_state` — the presentation state the Gui app reads.
    pub view_state: bool,
    /// `properties`/`set_property` on the `Settings/<Feature>` namespace.
    pub settings: bool,
    /// `provider_features`, `set_provider`, `refresh_providers`, `provider_skin`.
    pub providers: bool,
    /// `commands`, `run_command`, and the dialogs a command opens.
    pub commands: bool,
    /// `interaction`, `manipulator_specs`, `add_datum_plane` — the workbench hooks.
    pub interaction: bool,
}

impl Contract {
    /// A facade with nothing: the document half answers everything.
    pub const NONE: Self = Self {
        tree: false,
        selection: false,
        view: false,
        view_state: false,
        settings: false,
        providers: false,
        commands: false,
        interaction: false,
    };

    /// The app script's whole surface, as the embedded CPython facade has it.
    pub const FULL: Self = Self {
        tree: true,
        selection: true,
        view: true,
        view_state: true,
        settings: true,
        providers: true,
        commands: true,
        interaction: true,
    };
}

/// One interpreter, as the shell's script half needs it.
///
/// The methods have defaults wherever a facade can legitimately have nothing to say — an
/// interpreter with no filesystem, or one that does not run an app script — so a new
/// engine only writes the parts it can actually answer.
pub trait PythonRuntime {
    /// Call `function` in the shell module with positional string arguments, and return
    /// its raw answer. Most callers want [`PythonHost::json`](crate::PythonHost::json);
    /// the raw form is for the two helpers that return a bare string (`hello`,
    /// `gpu_adapter`) and for tests.
    fn call(&self, function: &str, args: &[&str]) -> Result<String, String>;

    /// Whether the entry module defines `function`.
    ///
    /// [`Contract`] is per group, but one function in the `interaction` group belongs
    /// to the *edition's* app script rather than to `ferrocad_shell`:
    /// `add_datum_plane` is implemented by `python/ferrocad_parametric`, so a facade
    /// that has the interaction hooks (a mod's manipulators) can still have no datum
    /// hook. Answering yes by default keeps a runtime that cannot introspect its
    /// module from silently dropping a capability.
    fn has_function(&self, function: &str) -> bool {
        let _ = function;
        true
    }

    /// Which contract groups this facade implements. Defaults to none.
    fn contract(&self) -> Contract {
        Contract::NONE
    }

    /// Run the app script's `bootstrap()` and report the document it created.
    ///
    /// `None` means this half has no app script: the shell then keeps the document the
    /// document half booted.
    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        Ok(None)
    }

    /// `--open`: open the startup document and drop the sample the boot made.
    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        let _ = boot;
        Ok(None)
    }

    /// Open a saved document from a path the host picked.
    fn open_path(&self, path: &std::path::Path) -> Option<Result<OpenedDocument, String>> {
        let _ = path;
        None
    }

    /// Save the active document to a path the host picked.
    fn save_path(&self, path: &std::path::Path) -> Option<Result<(), String>> {
        let _ = path;
        None
    }

    /// Make one open document active.
    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        let _ = name;
        None
    }

    /// Install a fetched pack's non-entry files so a script can `open()` them.
    ///
    /// A mod reads its shader or template through
    /// `open(os.path.join(os.path.dirname(os.path.abspath(__file__)), ...))`, and the
    /// loader names the entry `<name>/<script>` — so a pack's resource key
    /// `Resources/a.wgsl` is installed at `<name>/Resources/a.wgsl`. A runtime with a
    /// real filesystem (the desktop's scanned tree) leaves this a no-op: the files are
    /// already where the path points.
    fn install_pack_files(&self, name: &str, files: &[(String, String)]) -> Result<(), String> {
        let _ = (name, files);
        Ok(())
    }

    /// The workbench roots this facade can scan, or `None` when it has no filesystem.
    ///
    /// `Some` is passed to the shell's `load_workbenches`; `None` answers "nothing was
    /// loaded" without crossing into an interpreter that cannot read a directory.
    fn mods_roots(&self) -> Option<Vec<PathBuf>> {
        None
    }

    /// The console's greeting line.
    fn hello(&self) -> Result<Option<String>, String>;

    /// What this half may do to the document.
    ///
    /// The default is read-write: a half compiled into the host shares the document it
    /// was linked with. A half whose binding layer exposes only reads overrides it.
    fn document_grant(&self) -> DocumentGrant {
        DocumentGrant::ReadWrite
    }

    /// The display hook's rows, or `None` when this facade has no display hook.
    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        let _ = have;
        None
    }

    /// Build a Python-backed interactive tool.
    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String>;

    /// Send a session verb (`cancel`/`finish`) to a Python-backed tool.
    fn tool_command(&self, name: &str, verb: &str) -> bool;

    /// The `pivy.coin` scene a script built, if this facade can build one.
    fn inventor_scene(&self) -> Result<Scene, String>;

    /// The listener that forwards viewport events into the script.
    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(NoopScriptListener)
    }
}

/// The listener a runtime installs when it has no event callbacks.
///
/// Registering nothing is not the same as registering a no-op: the shell asks whether a
/// listener is present. This one is the honest answer for a facade without `pivy`.
pub struct NoopScriptListener;

impl EventListener for NoopScriptListener {
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
