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
//! resolution order and the packaging story in `docs/distribution.md`.
//!
//! ## Editions
//!
//! [`run_as`] is the entry point an **edition** (a thin binary that links extra
//! workbench modules and brands the window) calls. An edition registers its own
//! built-in modules first, then delegates here; see `docs/repackaging.md`.

use ferrocad_gpui::HostConfig;

pub mod payload;

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

/// Like [`run`], but with an edition's `app_name` (the window title).
///
/// An edition calls this after registering its own built-in modules, so they share
/// the process-wide core the shell boots (see `docs/occt-integration.md` §4).
pub fn run_as(app_name: &str) {
    register_core_module();

    let payload = payload::resolve();
    let config = HostConfig {
        app_name: app_name.to_string(),
        entry_module: "ferrocad_shell".to_string(),
        python_paths: payload.python_paths,
        mods_paths: payload.mods_paths,
    };
    ferrocad_gpui::run_with(config);
}
