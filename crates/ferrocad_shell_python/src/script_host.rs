//! The CPython half of the [`ScriptHost`] seam.
//!
//! [`RustAppBackend`](ferrocad_shell_rust::RustAppBackend) is the document half; this is
//! the script half alone, so the two compose into the desktop's engine:
//!
//! ```text
//! RustAppBackend::from_shared_registry()
//!     .with_script_host(Box::new(PythonScriptHost))
//! ```
//!
//! Both halves then point at the one document in `ferrocad_core::application`, so
//! `FreeCAD.ActiveDocument` in a workbench and the model tree in the shell are the same
//! object. The document answers the tree, the rows of real objects, the stage and the
//! edits; this answers what a script owns — the synthetic `Settings/<Feature>` nodes, the
//! provider registry, the commands a workbench registered, its dialogs, and the console.
//!
//! The seam itself is [`ferrocad_python_host::PythonHost`], the same type the RustPython
//! half wraps; all this module adds is [`CpythonRuntime`] — how to call the *embedded*
//! interpreter, and which parts of the shell contract its facade implements. See
//! `docs/project/tasks/backend-parity.md` B18.
//!
//! # What is deliberately *not* here
//!
//! The five merged methods of `RustAppBackend` (`model_tree`, `properties`,
//! `set_property`, `dialog_accept`, `dialog_cancel`) are not implemented as `AppBackend`
//! calls here. The document half owns them and asks this half only for the part it cannot
//! answer, through [`extra_tree_nodes`], [`extra_properties`] and [`set_extra_property`] —
//! the hooks that keep a node the document has never heard of (`Settings/Grid`)
//! resolvable without turning every property read into a Python round-trip.
//!
//! [`extra_tree_nodes`]: ferrocad_shell_api::app::ScriptHost::extra_tree_nodes
//! [`extra_properties`]: ferrocad_shell_api::app::ScriptHost::extra_properties
//! [`set_extra_property`]: ferrocad_shell_api::app::ScriptHost::set_extra_property

use std::path::PathBuf;

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_python_host::{Contract, PythonHost, PythonRuntime};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::types::{Bootstrap, OpenedDocument};

use crate::backend::{python_tool, python_tool_command};
use crate::events::PythonEventCallbacks;
use crate::python;

/// The embedded interpreter, as a [`PythonRuntime`]: how to call one function, and what
/// the `FreeCAD` facade behind it implements.
///
/// Stateless: the interpreter, its facades and the document registry are all
/// process-wide, so a runtime is a handle rather than a holder.
#[derive(Debug, Default, Clone, Copy)]
pub struct CpythonRuntime;

impl CpythonRuntime {
    /// A runtime over the already-`configure`d interpreter.
    pub const fn new() -> Self {
        Self
    }
}

impl PythonRuntime for CpythonRuntime {
    fn call(&self, function: &str, args: &[&str]) -> Result<String, String> {
        python::call_raw(function, args)
    }

    /// The app script decides: `ferrocad_parametric` exports `add_datum_plane`, the
    /// geometry-free base script does not.
    fn has_function(&self, function: &str) -> bool {
        python::has_function(function)
    }

    /// The real `FreeCAD`/`FreeCADGui` facade: every group of the shell contract answers.
    fn contract(&self) -> Contract {
        Contract::FULL
    }

    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        python::bootstrap().map(Some)
    }

    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        python::open_startup_document(boot)
    }

    fn open_path(&self, path: &std::path::Path) -> Option<Result<OpenedDocument, String>> {
        Some(python::open_document(path))
    }

    fn save_path(&self, path: &std::path::Path) -> Option<Result<(), String>> {
        Some(python::save_document(path))
    }

    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        Some(python::activate_document(name))
    }

    fn mods_roots(&self) -> Option<Vec<PathBuf>> {
        Some(python::mods_dirs())
    }

    fn hello(&self) -> Result<Option<String>, String> {
        python::hello().map(Some)
    }

    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        Some(python::stage_meshes_skipping(have))
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        python_tool(name)
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        python_tool_command(name, verb)
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        python::inventor_scene()
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(PythonEventCallbacks)
    }
}

/// The embedded interpreter as a [`ScriptHost`](ferrocad_shell_api::app::ScriptHost).
pub type PythonScriptHost = PythonHost<CpythonRuntime>;

/// The unit value, so the host keeps its old spelling at call sites (`Box::new(PythonScriptHost)`).
///
/// A unit struct used to be both the type and the value; the alias above is only the type,
/// and this is the value. `PythonScriptHost::new()` still works through the alias.
#[allow(non_upper_case_globals)]
pub const PythonScriptHost: PythonScriptHost = PythonHost::new(CpythonRuntime);
