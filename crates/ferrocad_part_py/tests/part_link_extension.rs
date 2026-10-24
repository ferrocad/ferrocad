//! `App::LinkExtensionPython` on a Python feature: `addExtension` adds the link
//! property surface, `configLinkProperty` validates and records the source
//! properties, and the extension survives a native doc/save/load round trip.
//!
//! Same boot as the other Part tests; the Draft `test_array` case is the reason
//! this exists (Draft's `draftlink.py` calls exactly this pair).

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// `hasattr` without exceptions. An empty `PropertyLink` reads as `None` but the
/// attribute still exists, which is what Draft's `hasattr` checks test.
fn has(object: &Bound<'_, PyAny>, name: &str) -> bool {
    object.hasattr(name).unwrap_or(false)
}

#[test]
fn link_extension_adds_the_link_surface_and_configures_sources() {
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let doc = ferrocad
            .call_method1("newDocument", ("LinkExtension",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::FeaturePython", "Array"))
            .expect("addObject");

        // Draft's arrays add `Base` before attaching the link extension.
        object
            .call_method1("addProperty", ("App::PropertyLink", "Base", "Objects"))
            .expect("add Base");
        object
            .call_method1("addExtension", ("App::LinkExtensionPython",))
            .expect("addExtension");

        assert!(object
            .call_method1("hasExtension", ("App::LinkExtensionPython",))
            .unwrap()
            .extract::<bool>()
            .unwrap());
        for name in [
            "LinkedObject",
            "LinkTransform",
            "LinkPlacement",
            "Scale",
            "ScaleVector",
            "LinkMode",
            "LinkCopyOnChange",
            "ShowElement",
            "ElementCount",
            "PlacementList",
            "ElementList",
            "ColoredElements",
        ] {
            assert!(has(&object, name), "the extension adds {name}");
        }

        // `configLinkProperty("Placement", LinkedObject="Base")`: the object's
        // `Base` property is the link target. This is draftlink.py:97.
        let links = PyDict::new(py);
        links.set_item("LinkedObject", "Base").unwrap();
        object
            .call_method("configLinkProperty", ("Placement",), Some(&links))
            .expect("configLinkProperty(Placement, LinkedObject=Base)");

        // `configLinkProperty(ElementCount="Count")` (array.py:331).
        object
            .call_method1(
                "addProperty",
                ("App::PropertyInteger", "Count", "Array"),
            )
            .unwrap();
        let links = PyDict::new(py);
        links.set_item("ElementCount", "Count").unwrap();
        object
            .call_method("configLinkProperty", (), Some(&links))
            .expect("configLinkProperty(ElementCount=Count)");

        // A named source property that does not exist is an error, not a no-op.
        let links = PyDict::new(py);
        links.set_item("LinkedObject", "Missing").unwrap();
        let error = object
            .call_method("configLinkProperty", (), Some(&links))
            .expect_err("a missing source property must raise");
        assert!(
            error.to_string().contains("Missing"),
            "the error names the property: {error}"
        );
    });
}

#[test]
fn link_extension_survives_a_native_round_trip() {
    boot();
    Python::with_gil(|py| {
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _part = PyModule::import(py, "Part").expect("import Part");
        let doc = ferrocad
            .call_method1("newDocument", ("LinkExtensionRoundtrip",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::FeaturePython", "Array"))
            .expect("addObject");
        object
            .call_method1("addProperty", ("App::PropertyLink", "Base", "Objects"))
            .unwrap();
        object
            .call_method1("addExtension", ("App::LinkExtensionPython",))
            .unwrap();
        object
            .setattr("LinkTransform", true)
            .expect("LinkTransform is a real property");

        let path = std::env::temp_dir().join("ferrocad_link_extension.ferrocad");
        let path = path.to_string_lossy().to_string();
        doc.call_method1("saveAs", (&path,)).expect("saveAs native");
        ferrocad
            .call_method1("closeDocument", ("LinkExtensionRoundtrip",))
            .expect("closeDocument");

        let doc = ferrocad
            .call_method1("openDocument", (&path,))
            .expect("open native");
        let restored = doc
            .call_method1("getObject", ("Array",))
            .expect("getObject");
        assert!(!restored.is_none(), "the object is present");
        assert!(
            restored
                .call_method1("hasExtension", ("App::LinkExtensionPython",))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the extension set persists"
        );
        assert!(
            restored
                .getattr("LinkTransform")
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the extension property value persists"
        );
    });
}
