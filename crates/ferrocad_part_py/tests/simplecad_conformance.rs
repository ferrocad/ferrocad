//! SimpleCAD conformance: run its unit suite against our modules.
//!
//! SimpleCAD imports `Part`, `Mesh`, `MeshPart` and `FreeCAD` and expects them to
//! share the same process-wide core, which a standalone extension module cannot
//! provide (each `.so` would link its own `ferrocad_core`). This test supplies the
//! one image the application uses — `ferrocad` + `Part` + `Mesh` + `MeshPart` as
//! built-ins — and then runs `tools/conformance.py --simplecad` in-process, so
//! there is a single harness and a single report.
//!
//! The checkout is resolved exactly as the harness does it: `SIMPLECAD_DIR`, then
//! `FERROCAD_SIMPLECAD`, then the sibling `../simplecad`. When none exists the
//! test skips, so CI stays hermetic. The report is a **measurement**, not an
//! assertion: only the presence of the harness summary is checked.
//!
//! ```sh
//! export FERROCAD_SIMPLECAD=$PWD/../simplecad
//! cargo test -p ferrocad_part_py --test simplecad_conformance -- --nocapture
//! ```
//!
//! `FERROCAD_SIMPLECAD_FILES` narrows the run to one or more file stems, which is
//! how a single module can be isolated.
//!
//! It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).
//!
//! SimpleCAD runs curved-surface and offset operations in an isolated
//! `scripts/shape_worker.py` **subprocess**. A child does not inherit this
//! interpreter's built-in modules, so the in-process image must export the same
//! `PYTHONPATH`/library path a shell would set. Without that, every worker exits 1
//! with `ModuleNotFoundError: FreeCAD`, and SimpleCAD reports it as
//! “The kernel stopped unexpectedly on this geometry (code 1)”.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};

/// Register the built-in modules. Must run before the interpreter initialises.
fn boot() {
    use ferrocad_mesh_py::Mesh as mesh_module;
    use ferrocad_mesh_py::MeshPart as meshpart_module;
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
    pyo3::append_to_inittab!(mesh_module);
    pyo3::append_to_inittab!(meshpart_module);
}

/// The SimpleCAD checkout, in the harness's own order: `SIMPLECAD_DIR`,
/// `FERROCAD_SIMPLECAD`, then the sibling `../simplecad`.
fn simplecad_root(repo: &Path) -> PathBuf {
    let path = std::env::var_os("SIMPLECAD_DIR")
        .or_else(|| std::env::var_os("FERROCAD_SIMPLECAD"))
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.parent().unwrap_or(repo).join("simplecad"));
    path.canonicalize().unwrap_or(path)
}

/// Export the paths a SimpleCAD worker subprocess needs.
///
/// The parent test registers `ferrocad`, `Part`, `Mesh` and `MeshPart` as built-ins;
/// a subprocess can only import the packaged `python/` tree and the shared OCCT
/// libraries. Prepend (do not replace) whatever the caller already set, so a
/// developer's own environment keeps working.
fn export_child_paths(py: Python<'_>, repo: &Path) -> PyResult<()> {
    let os = py.import("os")?;
    let environ = os.getattr("environ")?;
    let separator = if cfg!(windows) { ";" } else { ":" };

    let python_dir = repo.join("python");
    if python_dir.is_dir() {
        prepend_path(&environ, "PYTHONPATH", &python_dir, separator)?;
    }

    if cfg!(windows) {
        let dll_dir = repo.join(".occt/prefix/bin");
        if dll_dir.is_dir() {
            prepend_path(&environ, "PATH", &dll_dir, separator)?;
        }
    } else {
        let lib_dir = repo.join(".occt/prefix/lib");
        if lib_dir.is_dir() {
            prepend_path(&environ, "LD_LIBRARY_PATH", &lib_dir, separator)?;
            prepend_path(&environ, "DYLD_LIBRARY_PATH", &lib_dir, separator)?;
        }
    }
    Ok(())
}

/// Prepend `directory` to a path-list variable unless it is already there.
fn prepend_path(
    environ: &Bound<'_, PyAny>,
    name: &str,
    directory: &Path,
    separator: &str,
) -> PyResult<()> {
    let wanted = directory.to_string_lossy().into_owned();
    let current: String = environ
        .get_item(name)
        .ok()
        .and_then(|value| value.extract().ok())
        .unwrap_or_default();
    if current.split(separator).any(|part| part == wanted) {
        return Ok(());
    }
    let joined = if current.is_empty() {
        wanted
    } else {
        format!("{wanted}{separator}{current}")
    };
    environ.set_item(name, joined)
}

/// Run `tools/conformance.py --simplecad` and return what it printed.
fn run_harness(py: Python<'_>, harness: &Path, root: &Path) -> PyResult<String> {
    let sys = py.import("sys")?;
    let io = py.import("io")?;

    // The harness locates the repo from `__file__` and runs from `__name__`.
    let mut args = vec![
        harness.to_string_lossy().into_owned(),
        "--simplecad".to_string(),
        "--simplecad-root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    if let Ok(files) = std::env::var("FERROCAD_SIMPLECAD_FILES") {
        args.push("--files".to_string());
        args.extend(files.split_whitespace().map(str::to_owned));
    }
    let argv = PyList::new(py, args)?;
    sys.setattr("argv", argv)?;

    let globals = PyDict::new(py);
    globals.set_item("__file__", harness.to_string_lossy().into_owned())?;
    globals.set_item("__name__", "__main__")?;

    // `Python::run` writes to the real fd via Python's `sys.stdout`; redirect it so
    // Rust can print the report itself (visible under `--nocapture`).
    let buf = io.call_method0("StringIO")?;
    let saved = sys.getattr("stdout")?;
    sys.setattr("stdout", &buf)?;

    let source = std::fs::read_to_string(harness).expect("read tools/conformance.py");
    let code = CString::new(source).expect("script has no NUL");
    let result = py.run(&code, Some(&globals), None);

    sys.setattr("stdout", saved)?;
    let text: String = buf.call_method0("getvalue")?.extract()?;

    match result {
        Ok(()) => {}
        Err(err) if err.is_instance_of::<pyo3::exceptions::PySystemExit>(py) => {
            let code = err.value(py).getattr("code").ok();
            let ok = code
                .as_ref()
                .map(|c| c.is_none() || c.extract::<i32>().unwrap_or(1) == 0)
                .unwrap_or(true);
            assert!(ok, "conformance harness exited with {code:?}");
        }
        Err(err) => panic!("conformance harness failed to run: {err}"),
    }
    Ok(text)
}

#[test]
fn simplecad_conformance() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = simplecad_root(&repo);
    if !root.is_dir() {
        eprintln!(
            "SimpleCAD checkout not found at {}; skipping SimpleCAD conformance",
            root.display()
        );
        return;
    }

    let harness = repo.join("tools/conformance.py");

    boot();
    Python::with_gil(|py| {
        export_child_paths(py, &repo).expect("export worker paths");
        let report = run_harness(py, &harness, &root).expect("run conformance");
        println!("\n{report}");
        assert!(
            report.contains("summary:"),
            "the harness produced no summary:\n{report}"
        );
    });
}
