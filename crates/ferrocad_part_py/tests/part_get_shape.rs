//! `Part.getShape`: the object's shape, or a sub-element resolved by name, as a
//! `Part.Shape` (the call Draft's Facebinder and Hatch proxies make).
//!
//! Same boot as the other Part tests; it needs OCCT.

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

fn number(object: &Bound<'_, PyAny>, name: &str) -> f64 {
    object
        .getattr(name)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .extract::<f64>()
        .unwrap_or_else(|error| panic!("{name} as f64: {error}"))
}

fn shape_type(value: &Bound<'_, PyAny>) -> String {
    value
        .getattr("ShapeType")
        .unwrap()
        .extract::<String>()
        .unwrap()
}

fn occ_error(py: Python<'_>) -> Bound<'_, pyo3::types::PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<pyo3::types::PyType>()
        .expect("OCCError is a type")
}

#[test]
fn get_shape_returns_the_object_shape_and_named_sub_elements() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = ferrocad
            .call_method1("newDocument", ("GetShape",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::Feature", "Box"))
            .expect("addObject");
        object
            .setattr("Shape", part.call_method1("makeBox", (2.0, 2.0, 2.0)).unwrap())
            .expect("obj.Shape = makeBox");
        doc.call_method0("recompute").expect("recompute");

        let get_shape = part.getattr("getShape").expect("Part.getShape");

        // The whole object shape.
        let whole = get_shape
            .call1((&object,))
            .expect("Part.getShape(obj)");
        assert_eq!(shape_type(&whole), "Solid");
        assert!((number(&whole, "Volume") - 8.0).abs() < 1e-9);

        // A named face, the form `facebinder.py` and `hatch.py` use.
        let face = get_shape
            .call1((&object, "Face1"))
            .expect("Part.getShape(obj, 'Face1')");
        assert_eq!(shape_type(&face), "Face");
        assert!((number(&face, "Area") - 4.0).abs() < 1e-9);

        // The same with `needSubElement=True` and `retType=1`: both return a shape
        // (ours always carries the element map).
        let kwargs = PyDict::new(py);
        kwargs.set_item("needSubElement", true).unwrap();
        kwargs.set_item("retType", 1).unwrap();
        let edge = get_shape
            .call((&object, "Edge1"), Some(&kwargs))
            .expect("Part.getShape(obj, 'Edge1', needSubElement=True, retType=1)");
        assert_eq!(shape_type(&edge), "Edge");
        assert!((number(&edge, "Length") - 2.0).abs() < 1e-9);

        // `transform=True` applies the object's Placement to a local shape.
        object
            .setattr(
                "Placement",
                ferrocad
                    .getattr("Placement")
                    .unwrap()
                    .call1((ferrocad.getattr("Vector").unwrap().call1((10.0, 0.0, 0.0)).unwrap(),))
                    .unwrap(),
            )
            .expect("obj.Placement");
        let kwargs = PyDict::new(py);
        kwargs.set_item("transform", true).unwrap();
        let moved = get_shape
            .call((&object,), Some(&kwargs))
            .expect("Part.getShape(obj, transform=True)");
        let bound_box = moved.getattr("BoundBox").expect("BoundBox");
        assert!(
            (number(&bound_box, "XMin") - 10.0).abs() < 1e-9,
            "the placement moves the shape"
        );
        // Without `transform`, the local shape stays at the origin.
        let local = get_shape.call1((&object,)).expect("Part.getShape(obj)");
        let bound_box = local.getattr("BoundBox").expect("BoundBox");
        assert!((number(&bound_box, "XMin")).abs() < 1e-9);
    });
}

#[test]
fn get_shape_raises_occ_error_for_a_missing_object_or_sub_element() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = ferrocad
            .call_method1("newDocument", ("GetShapeErrors",))
            .expect("newDocument");
        let object = doc
            .call_method1("addObject", ("Part::Feature", "Box"))
            .expect("addObject");
        object
            .setattr("Shape", part.call_method1("makeBox", (2.0, 2.0, 2.0)).unwrap())
            .unwrap();
        doc.call_method0("recompute").unwrap();
        let get_shape = part.getattr("getShape").expect("Part.getShape");
        let error_type = occ_error(py);

        let missing = get_shape.call1((&object, "Face99"));
        match missing {
            Ok(_) => panic!("a missing sub-element must raise Part.OCCError"),
            Err(error) => assert!(
                error.is_instance(py, &error_type),
                "missing sub-element: {error}"
            ),
        }

        let no_object = get_shape.call1((py.None(),));
        match no_object {
            Ok(_) => panic!("a missing object must raise Part.OCCError"),
            Err(error) => assert!(
                error.is_instance(py, &error_type),
                "missing object: {error}"
            ),
        }

        let kwargs = PyDict::new(py);
        kwargs.set_item("retType", 7).unwrap();
        let bad_ret = get_shape.call((&object,), Some(&kwargs));
        match bad_ret {
            Ok(_) => panic!("an unsupported retType must raise Part.OCCError"),
            Err(error) => assert!(
                error.is_instance(py, &error_type),
                "unsupported retType: {error}"
            ),
        }
    });
}
