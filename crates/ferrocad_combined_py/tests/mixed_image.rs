//! The mixed-image guard: an edition's built-ins must not be replaced by the
//! on-disk `ferrocad_combined` extension.
//!
//! [`ferrocad_combined_py::register_builtins`] is how an edition binary (and this
//! test) composes `ferrocad`, `Part`, `Mesh` and `MeshPart`. `install()` is the
//! host-interpreter path used by `ferrocad_combined.abi3.so`; in the same process
//! it must refuse to publish a second core over the built-ins.
//!
//! It needs OCCT (see `docs/architecture/occt-bundling.md`).

use std::path::PathBuf;
use std::sync::Mutex;

use pyo3::exceptions::PyImportError;
use pyo3::prelude::*;

/// Register the built-in modules. Must run before the interpreter initialises
/// (`auto-initialize` does that on the first `Python::with_gil`).
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(ferrocad_combined_py::register_builtins);
}

/// Serialise the tests: the `Part.py` case rebinds `sys.modules["Part"]` for the
/// duration of one assertion, and the install case inspects that table.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn temp_probe_dir() -> PathBuf {
    std::env::temp_dir().join(format!("ferrocad_mixed_image_{}", std::process::id()))
}

#[test]
fn inittab_modules_are_detected_as_builtin() {
    boot();
    let _serial = serial();
    Python::with_gil(|py| {
        for name in ["ferrocad", "Part", "Mesh", "MeshPart"] {
            assert!(
                ferrocad_combined_py::is_builtin_module(py, name),
                "{name} should be a built-in in the edition image"
            );
        }
    });
}

#[test]
fn install_refuses_to_replace_the_edition_builtins() {
    boot();
    let _serial = serial();
    Python::with_gil(|py| {
        // An edition has already imported `Part` before anything can import the
        // on-disk extension.
        let part = py.import("Part").unwrap();

        let error = ferrocad_combined_py::install(py)
            .expect_err("install() must refuse an interpreter with built-in Part");
        assert!(error.is_instance_of::<PyImportError>(py), "{error}");
        let message = error.to_string();
        assert!(
            message.contains("already has ferrocad/Part/Mesh/MeshPart built in"),
            "{message}"
        );

        // The guard runs before `publish`, so the built-in is still the only image.
        let modules = py.import("sys").unwrap().getattr("modules").unwrap();
        let published = modules.get_item("Part").unwrap();
        assert_eq!(
            published.as_ptr(),
            part.as_ptr(),
            "install() replaced the built-in Part"
        );
    });
}

#[test]
fn part_py_file_module_is_not_detected_as_builtin() {
    boot();
    let _serial = serial();
    Python::with_gil(|py| {
        let directory = temp_probe_dir();
        std::fs::create_dir_all(&directory).unwrap();
        let shim = directory.join("Part.py");
        std::fs::write(&shim, "value = 1\n").unwrap();

        let sys = py.import("sys").unwrap();
        let path = sys.getattr("path").unwrap();
        let directory_string = directory.to_str().unwrap().to_owned();
        path.call_method1("insert", (0, directory_string.as_str()))
            .unwrap();

        // Load the shim the way the host interpreter's `Part` resolves: a file
        // module (`SourceFileLoader`), not `BuiltinImporter`. In this process the
        // built-in of the same name wins on `sys.path`, so the test binds the file
        // module in `sys.modules`, which is what `find_spec` consults first.
        let util = py.import("importlib.util").unwrap();
        let spec = util
            .call_method1("spec_from_file_location", ("Part", shim.to_str().unwrap()))
            .unwrap();
        let module = util.call_method1("module_from_spec", (&spec,)).unwrap();
        let modules = sys.getattr("modules").unwrap();
        modules.set_item("Part", &module).unwrap();

        let builtin = ferrocad_combined_py::is_builtin_module(py, "Part");

        modules.call_method1("pop", ("Part",)).unwrap();
        path.call_method1("remove", (directory_string.as_str(),))
            .unwrap();
        std::fs::remove_dir_all(&directory).ok();

        assert!(!builtin, "a Part.py shim must not look like a built-in");
    });
}
