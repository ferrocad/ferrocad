//! One Python image for the whole app surface.
//!
//! FerroCAD's Python surface is four modules (`ferrocad`, `Part`, `Mesh`,
//! `MeshPart`) over one process-wide core. Linking them as four separate
//! extension modules does **not** produce that image: each `.so` statically links
//! its own `ferrocad_core`, so `Part.makeBox` and `App.newDocument` would see
//! different document registries and different kernel state.
//!
//! This crate is the composition root that links all four together:
//!
//! - [`register_builtins`] appends them with `PyImport_AppendInittab`, for a host
//!   that starts the interpreter itself (the edition binaries and the in-process
//!   test harness). It **must** run before the interpreter initialises.
//! - [`install`] builds the same module objects and publishes them in
//!   `sys.modules`, for a host Python that is already running. The standalone
//!   `ferrocad_image` extension calls it during its own module init; the
//!   `python/Part.py`, `python/Mesh.py` and `python/MeshPart.py` shims import
//!   that extension, so a plain `import Part` still resolves.
//!
//! Importing `ferrocad_image` therefore makes all four modules available and
//! sharing one core. The shims never create a second copy: [`install`] marks the
//! modules it publishes and is idempotent.

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Attribute set on every module [`install`] publishes, so a second call (from
/// another shim, or `FreeCAD` after `Part`) does not rebuild the image.
const MARKER: &str = "__ferrocad_shared_image__";

/// Append the four modules to the interpreter's built-in table.
///
/// Must be called before the interpreter initialises (before the first
/// `Python::with_gil`). The edition binaries and the in-process conformance
/// harness call it from `main` / the test boot.
pub fn register_builtins() {
    use ferrocad_mesh_py::{Mesh as mesh_module, MeshPart as meshpart_module};
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
    pyo3::append_to_inittab!(mesh_module);
    pyo3::append_to_inittab!(meshpart_module);
}

/// Publish the four modules into `sys.modules` in an already-running interpreter.
///
/// This is the runtime counterpart of [`register_builtins`], for a host that
/// imports `ferrocad_image` instead of embedding the modules. It is idempotent:
/// modules this function published carry [`MARKER`] and are left alone.
///
/// A module that was imported from a *different* `.so` (for example a standalone
/// `Part.abi3.so`) is replaced, because it cannot share this image's core.
pub fn install(py: Python<'_>) -> PyResult<()> {
    let modules = py.import("sys")?.getattr("modules")?;
    if installed(&modules) {
        return Ok(());
    }
    publish(py, &modules, "ferrocad", ferrocad_py::ferrocad)?;
    publish(py, &modules, "Part", ferrocad_part_py::Part)?;
    publish(py, &modules, "Mesh", ferrocad_mesh_py::Mesh)?;
    publish(py, &modules, "MeshPart", ferrocad_mesh_py::MeshPart)?;
    Ok(())
}

/// Whether `sys.modules` already holds the image this crate published.
fn installed(modules: &Bound<'_, pyo3::types::PyAny>) -> bool {
    match modules.get_item("Part") {
        Ok(module) => module.getattr(MARKER).is_ok(),
        Err(_) => false,
    }
}

/// Build one module, mark it, and install it under `name`.
fn publish(
    py: Python<'_>,
    modules: &Bound<'_, pyo3::types::PyAny>,
    name: &str,
    init: fn(&Bound<'_, PyModule>) -> PyResult<()>,
) -> PyResult<()> {
    let module = PyModule::new(py, name)?;
    init(&module)?;
    module.add(MARKER, true)?;
    modules.set_item(name, &module)?;
    Ok(())
}

/// The standalone `ferrocad_image` extension.
///
/// Creating a module object and calling another `#[pymodule]` init is the only way
/// to add top-level modules after CPython has started, which is the situation an
/// imported extension module is in. All four are built by this same `.so`, so they
/// share one core.
#[pymodule]
pub fn ferrocad_image(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    install(py)?;
    let modules = py.import("sys")?.getattr("modules")?;
    for name in ["ferrocad", "Part", "Mesh", "MeshPart"] {
        m.add(name, modules.get_item(name)?)?;
    }
    Ok(())
}
