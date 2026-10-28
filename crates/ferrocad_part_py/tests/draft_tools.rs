//! In-process run of `tests/test_draft_tools.py`.
//!
//! A standalone pytest run has no `Part`, so that suite installs a documented
//! `Part.makePolygon` stub for its creation tests. Here `ferrocad`, `Part` and
//! `ferrocad.stage` share one interpreter (the app's composition), so the same
//! suite runs against the real kernel: the line/wire tools create a real
//! `Part::Feature` whose shape is a real wire.
//!
//! It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::ffi::CString;
use std::sync::Once;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Register the built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

fn repo() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..")
}

#[test]
fn runs_the_draft_tool_suite_with_the_real_part_kernel() {
    boot();
    Python::with_gil(|py| {
        ferrocad_stage_py::install(py).expect("install ferrocad.stage");
        let sys = py.import("sys").expect("sys");
        sys.getattr("path")
            .unwrap()
            .call_method1("insert", (0, format!("{}/python", repo())))
            .expect("sys.path.insert");

        let globals = PyDict::new(py);
        globals
            .set_item("__name__", "__draft_tools_acceptance__")
            .unwrap();
        globals
            .set_item("suite_path", format!("{}/tests/test_draft_tools.py", repo()))
            .unwrap();
        let code = CString::new(
            r#"
import importlib.util, unittest
spec = importlib.util.spec_from_file_location("test_draft_tools", suite_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
suite = unittest.defaultTestLoader.loadTestsFromModule(module)
result = unittest.TextTestRunner(verbosity=2).run(suite)
assert result.wasSuccessful(), f"failures={result.failures} errors={result.errors}"
"#,
        )
        .expect("the suite path has no interior NUL");
        py.run(code.as_c_str(), Some(&globals), None)
            .expect("the Draft tool suite passes against the real Part kernel");
    });
}
