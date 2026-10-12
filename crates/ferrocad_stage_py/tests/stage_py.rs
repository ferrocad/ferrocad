//! The T5 verification: the shared Python acceptance suite against an embedded CPython,
//! plus the native [`PyTool`] adapter a host would use to run a scripted tool.

mod common;

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict};

/// The Python acceptance suite, shared with the built-extension run.
fn acceptance_path() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/test_stage_py.py")
}

/// Run `tests/test_stage_py.py` (its `unittest` cases) against the embedded module. The
/// file is the single source of truth for the surface's behaviour.
#[test]
fn runs_the_python_acceptance_suite() {
    Python::with_gil(|py| {
        common::ferrocad_stage(py);

        let globals = PyDict::new(py);
        globals.set_item("__name__", "__stage_acceptance__").unwrap();
        globals.set_item("suite_path", acceptance_path()).unwrap();
        let code = CString::new(
            r#"
import importlib.util, unittest
spec = importlib.util.spec_from_file_location("test_stage_py", suite_path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
suite = unittest.defaultTestLoader.loadTestsFromModule(module)
result = unittest.TextTestRunner(verbosity=2).run(suite)
assert result.wasSuccessful(), f"failures={result.failures} errors={result.errors}"
"#,
        )
        .expect("the suite path has no interior NUL");
        py.run(code.as_c_str(), Some(&globals), None)
            .expect("the ferrocad.stage acceptance suite passes");
    });
}

/// A Python `Tool` subclass, driven through the native `ferrocad_stage::Tool` trait.
#[test]
fn the_native_adapter_drives_a_python_tool() {
    Python::with_gil(|py| {
        let module = common::ferrocad_stage(py);
        let globals = PyDict::new(py);
        globals.set_item("stage", &module).unwrap();
        let code = CString::new(
            r#"
class Recorder(stage.Tool):
    def __init__(self):
        super().__init__()
        self.seen = []
    def on_event(self, event):
        self.seen.append(event.position)
        item = stage.ShapeItem(0)
        item.set_shape([(0, 0, 0), (1, 0, 0)], None, None, [0, 1])
        self.add_item(item)
        return True

recorder = Recorder()
"#,
        )
        .unwrap();
        py.run(code.as_c_str(), Some(&globals), None).unwrap();

        let recorder = globals.get_item("recorder").unwrap().unwrap();
        let mut tool =
            ferrocad_stage_py::PyTool::new(py, recorder.clone().unbind()).expect("wrap the tool");

        use ferrocad_stage::{PointerEvent, PointerKind, Tool as _};
        let event = PointerEvent {
            kind: PointerKind::Move,
            position: [10, 20],
            button: None,
            hit: None,
        };
        tool.on_event(&event, None);

        assert_eq!(tool.items().len(), 1, "the Python tool added a transient item");
        let seen: Vec<(i32, i32)> = recorder.getattr("seen").unwrap().extract().unwrap();
        assert_eq!(seen, vec![(10, 20)], "the Python override saw the event");
    });
}
