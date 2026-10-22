//! # ferrocad
//!
//! The base FerroCAD application. The binary (`cargo install ferrocad`) boots an
//! embedded CPython interpreter and opens the `bite-gpui` shell; the engine is
//! [`ferrocad_core`](https://crates.io/crates/ferrocad_core) and the shell library
//! is [`ferrocad_gpui`](https://crates.io/crates/ferrocad_gpui).
//!
//! ## The payload
//!
//! The app is useless without its Python facade and workbenches. A development
//! checkout has them in the workspace; `cargo install` does not, so the payload
//! is embedded into the binary and unpacked on first run. See [`payload`] for the
//! resolution order and the packaging story in `docs/architecture/distribution.md`.
//!
//! ## Editions
//!
//! [`run_as`] is the entry point an **edition** (a thin binary that links extra
//! workbench modules and brands the window) calls. An edition registers its own
//! built-in modules first, then delegates here; see `docs/architecture/repackaging.md`.

use ferrocad_gpui::HostConfig;

pub mod payload;

/// The workbench contributions an edition registers (its manipulators).
pub use ferrocad_gpui::Contributions;

/// Register the core `ferrocad` PyO3 extension as a built-in module, so `import
/// ferrocad` resolves without a separate `ferrocad.abi3.so` on disk. `#[pymodule]`
/// expands `ferrocad_py::ferrocad` into a hidden module, which the macro needs in
/// scope by name.
///
/// Must run before the interpreter initialises.
pub fn register_core_module() {
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
}

/// Boot the embedded interpreter and open the shell window (the base app).
pub fn run() {
    run_as("FerroCAD");
}

/// Like [`run`], but with an edition's `app_name` (the window title) and its Python
/// entry module.
///
/// An edition calls this after registering its own built-in modules, so they share
/// the process-wide core the shell boots (see `docs/architecture/occt-integration.md` §4). The
/// entry module is the edition's **app script**: it re-exports the base shell's data
/// functions and registers its workbenches' module hooks (see
/// `docs/architecture.md` §9).
pub fn run_as(app_name: &str) {
    run_edition(app_name, "ferrocad_shell");
}

/// [`run_as`] with an explicit entry module (the edition's app script).
pub fn run_edition(app_name: &str, entry_module: &str) {
    run_edition_with(app_name, entry_module, base_contributions());
}

/// The general edition's contributions: the generic transform handle.
///
/// A whole-object selection arms a translate gizmo, so the shipped app has an
/// interactive move tool out of the box (the dock's `Move` arms it for the current
/// selection). Workbench manipulators — the Part edge dress-up arc, a height grip —
/// stay with the edition that links them, which is why this is a default and not a
/// global registration in the shell.
fn base_contributions() -> Contributions {
    Contributions::new().with_gizmo(ferrocad_gpui::translate_gizmo)
}

/// [`run_edition`] with the workbench contributions the edition registers.
///
/// An edition that links a workbench with a manipulator (Part's edge dress-up) passes
/// its factories here, so the shell can host them without naming the workbench. An
/// edition with no manipulators passes [`Contributions::default`].
pub fn run_edition_with(
    app_name: &str,
    entry_module: &str,
    contributions: Contributions,
) {
    register_core_module();
    let mut config = app_config(app_name, entry_module);
    config.contributions = contributions;
    ferrocad_gpui::run_with(config);
}

/// Boot the interpreter and the payload, then exit without opening a window.
///
/// This backs the app's `--check-init` flag: it loads the native dependencies,
/// the built-in `ferrocad` module, the `FreeCAD` facade and the workbenches
/// exactly as a normal start does, and reports success. It needs no display or
/// GPU, so the release workflow runs it on an unpacked artifact to catch a broken
/// layout before shipping (see `docs/architecture/distribution.md`).
pub fn check_init() -> Result<(), String> {
    register_core_module();
    ferrocad_gpui::check_init(base_config("FerroCAD"))
}

/// The [`HostConfig`] an edition boots with, after resolving its payload.
fn base_config(app_name: &str) -> HostConfig {
    app_config(app_name, "ferrocad_shell")
}

fn app_config(app_name: &str, entry_module: &str) -> HostConfig {
    let payload = payload::resolve();
    HostConfig {
        app_name: app_name.to_string(),
        entry_module: entry_module.to_string(),
        python_paths: payload.python_paths,
        mods_paths: payload.mods_paths,
        contributions: Contributions::default(),
    }
}
