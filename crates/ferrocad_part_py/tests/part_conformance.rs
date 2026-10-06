//! Part workbench conformance: run the upstream Part tests against our modules.
//!
//! The upstream Part tests need `import Part` **and** `import FreeCAD` to see the
//! same process-wide core, which a standalone extension module cannot provide
//! (each `.so` would link its own `ferrocad_core`). This test supplies the one
//! image the application uses — `ferrocad` + `Part` as built-ins — and then runs
//! `tools/conformance.py --part` in-process, so there is a single harness and a
//! single report.
//!
//! It is skipped unless `FERROCAD_UPSTREAM` points at a `freecad-upstream`
//! checkout (the report is a measurement, not an assertion):
//!
//! ```sh
//! export FERROCAD_UPSTREAM=$PWD/../freecad-upstream
//! cargo test -p ferrocad_part_py --test part_conformance -- --nocapture
//! ```
//!
//! It needs OCCT (see `docs/occt-bundling.md`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// Register both built-in modules. Must run before the interpreter initialises.
fn boot() {
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
}

/// Run `tools/conformance.py --part` and return what it printed.
fn run_harness(py: Python<'_>, harness: &std::path::Path, root: &str) -> PyResult<String> {
    let sys = py.import("sys")?;
    let io = py.import("io")?;

    // The harness locates the repo from `__file__` and runs from `__name__`.
    let argv = PyList::new(
        py,
        [
            harness.to_string_lossy().into_owned(),
            "--root".to_string(),
            root.to_string(),
            "--part".to_string(),
        ],
    )?;
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
fn upstream_part_conformance() {
    let Ok(root) = std::env::var("FERROCAD_UPSTREAM") else {
        eprintln!("FERROCAD_UPSTREAM is not set; skipping Part conformance");
        return;
    };

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let harness = repo.join("tools/conformance.py");

    boot();
    Python::with_gil(|py| {
        let report = run_harness(py, &harness, &root).expect("run conformance");
        println!("\n{report}");
        assert!(
            report.contains("summary:"),
            "the harness produced no summary:\n{report}"
        );
    });
}
