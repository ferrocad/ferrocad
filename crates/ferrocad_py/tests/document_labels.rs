//! Document label lookups (`Document.getObjectsByLabel`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Register `ferrocad` as a built-in module and start CPython.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::prepare_freethreaded_python();
    });
}

#[test]
fn objects_are_found_by_label_not_name() {
    boot();
    Python::with_gil(|py| {
        let source = r#"
import ferrocad as App

doc = App.newDocument("Labels")
a = doc.addObject("App::FeatureTest", "Box")
b = doc.addObject("App::FeatureTest", "Box001")
assert a.Name == "Box" and a.Label == "Box", (a.Name, a.Label)
assert b.Name == "Box001" and b.Label == "Box001"

# Two objects may share a label; the lookup is ordered and exact.
b.Label = "Box"
found = doc.getObjectsByLabel("Box")
assert [o.Name for o in found] == ["Box", "Box001"], [o.Name for o in found]
assert doc.getObjectsByLabel("Box001") == [], "label is not the name"
assert doc.getObjectsByLabel("missing") == []

App.closeDocument("Labels")
"#;
        let code = CString::new(source).expect("the snippet has no NUL");
        let globals = PyDict::new(py);
        if let Err(error) = py.run(&code, Some(&globals), None) {
            error.print(py);
            panic!("the label snippet failed");
        }
    });
}
