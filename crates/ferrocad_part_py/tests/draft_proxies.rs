//! Draft M2 acceptance: the two pending modules load against the proxy layer.
//!
//! `drafttests/test_creation.py` and `test_array.py` are the first real
//! consumers of `Part::FeaturePython` / `Part::Part2DObjectPython`: every Draft
//! object is a Python proxy that writes `obj.Shape`. This test runs them through
//! `tools/conformance.py --draft` in the one image that links `ferrocad` + `Part`
//! and prints the counts. It fails only on a **load** error, not on a test
//! failure: the numbers are the measurement, not a gate.
//!
//! Skipped unless `FERROCAD_UPSTREAM` points at a `freecad-upstream` checkout.
//! Needs OCCT.

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

fn boot() {
    use std::sync::Once;

    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// Run `tools/conformance.py --draft --files ...` and return what it printed.
fn run_harness(py: Python<'_>, harness: &std::path::Path, root: &str) -> PyResult<String> {
    let sys = py.import("sys")?;
    let io = py.import("io")?;

    let args = vec![
        harness.to_string_lossy().into_owned(),
        "--root".to_string(),
        root.to_string(),
        "--draft".to_string(),
        "--files".to_string(),
        "test_creation".to_string(),
        "test_array".to_string(),
    ];
    sys.setattr("argv", PyList::new(py, args)?)?;

    let globals = PyDict::new(py);
    globals.set_item("__file__", harness.to_string_lossy().into_owned())?;
    globals.set_item("__name__", "__main__")?;

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
fn draft_creation_and_array_modules_load() {
    let Ok(root) = std::env::var("FERROCAD_UPSTREAM") else {
        eprintln!("FERROCAD_UPSTREAM is not set; skipping the Draft load check");
        return;
    };

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let harness = repo.join("tools/conformance.py");

    boot();
    Python::with_gil(|py| {
        let report = run_harness(py, &harness, &root).expect("run conformance");
        println!("\n{report}");
        assert!(report.contains("summary:"), "no summary:\n{report}");
        for stem in ["test_creation", "test_array"] {
            assert!(
                !report.contains(&format!("{stem}.py: LOAD ERROR")),
                "{stem} must load:\n{report}"
            );
        }
    });
}
