//! The base FerroCAD application (the general edition).
//!
//! It is the smallest possible edition: it calls the shared shell library with
//! the base app's configuration (its Python entry module, and later its workbench
//! directories). A redistribution such as *FerroCAD: Architecture* is the same
//! shape with different scripts and mods (see `docs/repackaging.md`).
//!
//! Run it with `cargo run -p ferrocad` (needs a display); `cargo install
//! ferrocad` produces the same binary with its Python payload embedded.

fn main() {
    // Register the PyO3 extension as a built-in module, so `import ferrocad`
    // resolves without a separate `ferrocad.abi3.so` on disk. `#[pymodule]`
    // expands `ferrocad_py::ferrocad` into a hidden module, which the macro
    // needs in scope by name.
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);

    // Find the Python facade and workbenches: a checkout, an installer payload,
    // or the copy embedded in this binary (unpacked on first run).
    let payload = ferrocad::payload::resolve();

    let config = ferrocad_gpui::HostConfig {
        app_name: "FerroCAD".to_string(),
        entry_module: "ferrocad_shell".to_string(),
        python_paths: payload.python_paths,
        mods_paths: payload.mods_paths,
    };
    ferrocad_gpui::run_with(config);
}
