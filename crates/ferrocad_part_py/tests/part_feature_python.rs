//! `Part::FeaturePython` / `Part::Part2DObjectPython` proxy behaviour.
//!
//! These exercise the P2 contract end to end: a Python proxy is stored by
//! `addObject`, its `execute` runs on recompute (writing `obj.Shape`), its
//! `onChanged` follows a property write, and a native save/load keeps both the
//! proxy and the shape. A document whose proxy class cannot be imported loads
//! readable but invalid.
//!
//! Needs OCCT (see `docs/developer/architecture/occt-bundling.md`) and the one image that
//! links `ferrocad` + `Part`.

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// A proxy whose `execute` makes a box and whose `onChanged` records calls.
///
/// It is defined in a synthetic module (registered in `sys.modules`) so the
/// restore path can import it by `__module__`/class name.
const PROXY_SOURCE: &str = r#"
class Proxy:
    def __init__(self):
        self.executes = 0
        self.changes = []

    def execute(self, obj):
        self.executes += 1
        obj.Shape = Part.makeBox(1.0, 2.0, 3.0)

    def onChanged(self, obj, prop):
        self.changes.append(prop)

    def dumps(self):
        return {"executes": self.executes}

    def loads(self, state):
        if isinstance(state, dict):
            self.executes = state.get("executes", 0)


class AttachProxy:
    def attach(self, obj):
        obj.addProperty("App::PropertyInteger", "Attached")
"#;

/// Install `name` as an importable module holding `Proxy`; returns the class.
fn install_proxy_module<'py>(
    py: Python<'py>,
    name: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let module = PyModule::new(py, name)?;
    module.setattr("Part", py.import("Part")?)?;
    let code = CString::new(PROXY_SOURCE).unwrap();
    py.run(&code, Some(&module.dict()), None)?;
    py.import("sys")?
        .getattr("modules")?
        .downcast::<PyDict>()?
        .set_item(name, &module)?;
    module.getattr("Proxy")
}

fn null_shape(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isNull")
        .unwrap()
        .extract::<bool>()
        .unwrap()
}

#[test]
fn a_proxy_recomputes_and_sees_property_changes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let proxy_class = install_proxy_module(py, "feature_proxy_test").expect("proxy module");
        let proxy = proxy_class.call0().expect("Proxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("FeaturePythonDemo",))
            .expect("newDocument");
        let feature = doc
            .call_method1(
                "addObject",
                ("Part::FeaturePython", "Feature", &proxy, py.None()),
            )
            .expect("addObject Part::FeaturePython");

        // `addObject(objProxy=...)` stored the proxy and `obj.Proxy` returns it.
        let stored = feature.getattr("Proxy").expect("obj.Proxy");
        assert!(stored.is(&proxy), "addObject must store the object proxy");

        // The default Shape is null; a recompute runs the proxy, which writes it.
        assert!(null_shape(&feature.getattr("Shape").unwrap()));
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            1,
            "the proxy executed exactly once"
        );
        assert!(
            !null_shape(&feature.getattr("Shape").unwrap()),
            "the proxy's Shape is visible after recompute"
        );

        // A clean recompute does not execute the proxy again.
        doc.call_method0("recompute").expect("recompute again");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            1
        );

        // A property write notifies `onChanged`; the object is then dirty.
        feature
            .call_method1("addProperty", ("App::PropertyInteger", "Count"))
            .expect("addProperty");
        feature.setattr("Count", 7).expect("obj.Count = 7");
        let changes: Vec<String> = proxy
            .getattr("changes")
            .unwrap()
            .extract()
            .expect("changes list");
        assert_eq!(
            changes.last().map(String::as_str),
            Some("Count"),
            "the property write notifies onChanged: {changes:?}"
        );
        // The proxy's own `obj.Shape = ...` also goes through onChanged, exactly as
        // upstream's `DocumentObject::onChanged`.
        assert!(changes.contains(&"Shape".to_string()), "{changes:?}");

        doc.call_method0("recompute").expect("recompute after change");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            2,
            "a property change makes the feature execute again"
        );
    });
}

#[test]
fn a_native_round_trip_keeps_the_proxy_and_the_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let proxy_class = install_proxy_module(py, "feature_proxy_roundtrip").expect("proxy module");
        let proxy = proxy_class.call0().expect("Proxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("FeaturePythonRoundtrip",))
            .expect("newDocument");
        let _feature = doc
            .call_method1("addObject", ("Part::FeaturePython", "Feature", &proxy, py.None()))
            .expect("addObject");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            proxy.getattr("executes").unwrap().extract::<i64>().unwrap(),
            1
        );

        let path = std::env::temp_dir().join("ferrocad_feature_python.ferrocad");
        let path = path.to_string_lossy().to_string();
        doc.call_method1("saveAs", (&path,)).expect("saveAs native");
        ferrocad
            .call_method1("closeDocument", ("FeaturePythonRoundtrip",))
            .expect("closeDocument");

        let doc = ferrocad
            .call_method1("openDocument", (&path,))
            .expect("open native");
        let feature = doc
            .call_method1("getObject", ("Feature",))
            .expect("getObject");
        assert!(!feature.is_none(), "the object is present");
        assert!(
            !feature.getattr("Proxy").unwrap().is_none(),
            "the native round trip keeps the proxy"
        );
        assert!(
            !null_shape(&feature.getattr("Shape").unwrap()),
            "the persisted Shape is readable after load"
        );

        doc.call_method0("recompute").expect("recompute after load");
        let restored = feature.getattr("Proxy").expect("restored proxy");
        assert_eq!(
            restored
                .getattr("executes")
                .unwrap()
                .extract::<i64>()
                .unwrap(),
            2,
            "the restored proxy kept its state and executed on recompute"
        );
        assert!(
            !null_shape(&feature.getattr("Shape").unwrap()),
            "the proxy rebuilt the Shape after load"
        );

        ferrocad
            .call_method1("closeDocument", ("FeaturePythonRoundtrip",))
            .expect("closeDocument");
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn a_missing_proxy_loads_readable_but_invalid() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let proxy_class = install_proxy_module(py, "feature_proxy_ghost").expect("proxy module");
        let proxy = proxy_class.call0().expect("Proxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("FeaturePythonGhost",))
            .expect("newDocument");
        doc.call_method1(
            "addObject",
            ("Part::FeaturePython", "Feature", &proxy, py.None()),
        )
        .expect("addObject");
        doc.call_method0("recompute").expect("recompute");

        let path = std::env::temp_dir().join("ferrocad_feature_python_ghost.ferrocad");
        let path = path.to_string_lossy().to_string();
        doc.call_method1("saveAs", (&path,)).expect("saveAs");
        ferrocad
            .call_method1("closeDocument", ("FeaturePythonGhost",))
            .expect("closeDocument");

        // Make the proxy class unavailable, as a document opened without the
        // workbench module that defines it would be.
        py.import("sys")
            .unwrap()
            .getattr("modules")
            .unwrap()
            .downcast::<PyDict>()
            .unwrap()
            .del_item("feature_proxy_ghost")
            .expect("forget the proxy module");

        let doc = ferrocad
            .call_method1("openDocument", (&path,))
            .expect("open without the proxy");
        let feature = doc
            .call_method1("getObject", ("Feature",))
            .expect("getObject");
        assert!(!feature.is_none(), "the object is present");
        // Readable: the Shape was persisted. No proxy: honest `None`.
        assert!(!null_shape(&feature.getattr("Shape").unwrap()));
        assert!(feature.getattr("Proxy").unwrap().is_none());
        assert!(
            feature
                .getattr("State")
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap()
                .iter()
                .any(|state| state == "Invalid"),
            "a proxy that could not be restored leaves the object invalid"
        );

        ferrocad
            .call_method1("closeDocument", ("FeaturePythonGhost",))
            .expect("closeDocument");
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn add_object_calls_the_proxy_attach_when_asked() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let module = PyModule::new(py, "feature_proxy_attach_test").unwrap();
        module.setattr("Part", py.import("Part").unwrap()).unwrap();
        let code = CString::new(PROXY_SOURCE).unwrap();
        py.run(&code, Some(&module.dict()), None).unwrap();
        let proxy = module
            .getattr("AttachProxy")
            .unwrap()
            .call0()
            .expect("AttachProxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("FeaturePythonAttach",))
            .expect("newDocument");
        let obj = doc
            .call_method1(
                "addObject",
                (
                    "Part::FeaturePython",
                    "Attached",
                    &proxy,
                    py.None(),
                    true,
                ),
            )
            .expect("addObject attach=true");
        let properties: Vec<String> = obj
            .getattr("PropertiesList")
            .unwrap()
            .extract()
            .unwrap();
        assert!(
            properties.iter().any(|name| name == "Attached"),
            "attach=True calls proxy.attach: {properties:?}"
        );
        ferrocad
            .call_method1("closeDocument", ("FeaturePythonAttach",))
            .expect("closeDocument");
    });
}

#[test]
fn an_fcstd_round_trip_keeps_the_shape_but_not_an_incompatible_proxy() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let proxy_class = install_proxy_module(py, "feature_proxy_fcstd").expect("proxy module");
        let proxy = proxy_class.call0().expect("Proxy()");

        let doc = ferrocad
            .call_method1("newDocument", ("FeaturePythonFcstd",))
            .expect("newDocument");
        doc.call_method1(
            "addObject",
            ("Part::FeaturePython", "Feature", &proxy, py.None()),
        )
        .expect("addObject");
        doc.call_method0("recompute").expect("recompute");

        let path = std::env::temp_dir().join("ferrocad_feature_python.FCStd");
        let path = path.to_string_lossy().to_string();
        doc.call_method1("saveAs", (&path,)).expect("saveAs FCStd");
        ferrocad
            .call_method1("closeDocument", ("FeaturePythonFcstd",))
            .expect("closeDocument");

        // FerroCAD's proxy payload is its own `{"attrs", "proxy"}` envelope, not
        // upstream's `<Python value>` proxy pickle, so the writer reports it as
        // dropped instead of writing a misleading value. The Shape is still the
        // `.FCStd` `Part::PropertyPartShape` member, so it survives.
        let doc = ferrocad
            .call_method1("openDocument", (&path,))
            .expect("open FCStd");
        let feature = doc
            .call_method1("getObject", ("Feature",))
            .expect("getObject");
        assert!(!feature.is_none(), "the object is present");
        assert!(!null_shape(&feature.getattr("Shape").unwrap()));
        assert!(feature.getattr("Proxy").unwrap().is_none());

        doc.call_method0("recompute").expect("recompute");
        assert!(feature
            .getattr("State")
            .unwrap()
            .extract::<Vec<String>>()
            .unwrap()
            .iter()
            .any(|state| state == "Invalid"));

        ferrocad
            .call_method1("closeDocument", ("FeaturePythonFcstd",))
            .expect("closeDocument");
        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn part2d_python_has_the_feature_surface() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let doc = ferrocad
            .call_method1("newDocument", ("Part2DPython",))
            .expect("newDocument");
        let obj = doc
            .call_method1("addObject", ("Part::Part2DObjectPython", "Sketchy"))
            .expect("addObject Part::Part2DObjectPython");
        assert!(
            obj.getattr("PropertiesList")
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap()
                .iter()
                .any(|name| name == "Shape"),
            "a Part2D Python object has a Shape"
        );
        assert!(obj.getattr("Proxy").unwrap().is_none());
        // Recomputing without a proxy is an honest failure, not a silent success.
        doc.call_method0("recompute").expect("recompute");
        assert!(
            obj.getattr("State")
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap()
                .iter()
                .any(|state| state == "Invalid"),
            "no proxy leaves the object invalid"
        );
    });
}

#[test]
fn a_python_feature_without_an_interpreter_is_invalid() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // No interpreter is started here; this exercises the pure-Rust path.
    use ferrocad_core::Document;
    use ferrocad_part_py::feature_python::{register, FEATURE_PYTHON_TYPE};

    ferrocad_part::register(std::sync::Arc::new(ferrocad_occt::OcctBackend::new()));
    register();
    ferrocad_core::object_registry::set_python_executor_available(false);

    let mut doc = Document::new();
    let id = doc.add_object("Feature", FEATURE_PYTHON_TYPE);
    doc.touch(id, false);
    let _ = doc.recompute();
    let object = doc.object(id).expect("the object exists");
    assert!(object.invalid, "without an interpreter the object is invalid");
    assert!(
        object.error.as_deref().unwrap_or_default().contains("interpreter"),
        "the failure names the missing interpreter: {:?}",
        object.error
    );
    assert!(!doc.python_pending(id));

    // The flag is process-wide; leave it as the other tests expect it.
    ferrocad_core::object_registry::set_python_executor_available(true);
}
