//! `Part::PropertyTopoShapeList`, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).
//!
//! Mirrors upstream `src/Mod/Part/parttests/TopoShapeListTest.py`: a dynamic
//! property on an `App::FeaturePython`, three boxes, persistence through
//! `saveAs`/`openDocument`, then a transaction and `undo`.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    static BOOT: std::sync::Once = std::sync::Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// A per-test scratch file under the system temp directory.
fn scratch_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ferrocad_shape_list_{}_{tag}.json",
        std::process::id()
    ))
}

/// The volumes of `obj.Shapes`, in order.
fn volumes_of(shapes: &Bound<'_, PyAny>) -> Vec<f64> {
    (0..shapes.len().expect("a shape list"))
        .map(|index| {
            shapes
                .get_item(index)
                .expect("a shape entry")
                .getattr("Volume")
                .expect("Shape.Volume")
                .extract::<f64>()
                .expect("volume as f64")
        })
        .collect()
}

fn assert_volumes(shapes: &Bound<'_, PyAny>, expected: &[f64]) {
    let volumes = volumes_of(shapes);
    assert_eq!(volumes.len(), expected.len(), "entry count: {volumes:?}");
    for (index, (got, want)) in volumes.iter().zip(expected).enumerate() {
        assert!(
            (got - want).abs() < 1e-7,
            "entry {index} has volume {got}, expected {want}"
        );
    }
}

/// An `App::FeaturePython` with a `Shapes` property and three assigned boxes.
fn object_with_three_boxes<'py>(
    app: &Bound<'py, PyModule>,
    part: &Bound<'py, PyModule>,
    doc_name: &str,
) -> (Bound<'py, PyAny>, Bound<'py, PyAny>) {
    let doc = app
        .call_method1("newDocument", (doc_name,))
        .expect("newDocument");
    let obj = doc
        .call_method1("addObject", ("App::FeaturePython", "TestObject"))
        .expect("addObject App::FeaturePython");
    obj.call_method1("addProperty", ("Part::PropertyTopoShapeList", "Shapes"))
        .expect("addProperty Part::PropertyTopoShapeList");

    let boxes: Vec<_> = [1.0, 2.0, 3.0]
        .iter()
        .map(|height| {
            part.call_method1("makeBox", (1.0, 1.0, *height))
                .expect("Part.makeBox")
        })
        .collect();
    let list = PyList::new(part.py(), &boxes).expect("box list");
    obj.setattr("Shapes", &list).expect("obj.Shapes = [box, box, box]");
    (doc, obj)
}

#[test]
fn a_shape_list_property_holds_ordered_shapes() {
    boot();
    Python::with_gil(|py| {
        let app = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let part = PyModule::import(py, "Part").expect("import Part");

        let (doc, obj) = object_with_three_boxes(&app, &part, "ShapeListOrdered");
        let shapes = obj.getattr("Shapes").expect("obj.Shapes");
        assert_volumes(&shapes, &[1.0, 2.0, 3.0]);

        // Reassigning fewer entries keeps them in order and in count.
        let shorter = PyList::new(
            py,
            [
                shapes.get_item(1).expect("entry 1"),
                shapes.get_item(2).expect("entry 2"),
            ],
        )
        .expect("two-entry list");
        obj.setattr("Shapes", &shorter).expect("obj.Shapes = two boxes");
        assert_volumes(&obj.getattr("Shapes").unwrap(), &[2.0, 3.0]);

        // The empty list is still a shape list (not, say, a bool list).
        obj.setattr("Shapes", PyList::empty(py))
            .expect("obj.Shapes = []");
        assert_eq!(obj.getattr("Shapes").unwrap().len().unwrap(), 0);

        let name: String = doc.getattr("Name").unwrap().extract().unwrap();
        app.call_method1("closeDocument", (&name,))
            .expect("closeDocument");
    });
}

#[test]
fn a_shape_list_property_persists_and_undoes() {
    boot();
    Python::with_gil(|py| {
        let app = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let part = PyModule::import(py, "Part").expect("import Part");

        let file = scratch_file("persist");
        let (doc, obj) = object_with_three_boxes(&app, &part, "ShapeListPersist");
        assert_volumes(&obj.getattr("Shapes").unwrap(), &[1.0, 2.0, 3.0]);

        doc.call_method1("saveAs", (file.to_string_lossy().as_ref(),))
            .expect("saveAs");
        let name: String = doc.getattr("Name").unwrap().extract().unwrap();
        app.call_method1("closeDocument", (&name,))
            .expect("closeDocument");

        let reopened = app
            .call_method1("openDocument", (file.to_string_lossy().as_ref(),))
            .expect("openDocument");
        let reopened_obj = reopened
            .call_method1("getObject", ("TestObject",))
            .expect("getObject TestObject");
        assert_volumes(&reopened_obj.getattr("Shapes").unwrap(), &[1.0, 2.0, 3.0]);

        // Two entries inside a transaction, then undo restores all three.
        let shapes = reopened_obj.getattr("Shapes").unwrap();
        let two = PyList::new(
            py,
            [
                shapes.get_item(1).expect("entry 1"),
                shapes.get_item(2).expect("entry 2"),
            ],
        )
        .expect("two-entry list");
        reopened
            .call_method1("openTransaction", ("Change shapes",))
            .expect("openTransaction");
        reopened_obj
            .setattr("Shapes", &two)
            .expect("obj.Shapes = two boxes");
        reopened
            .call_method0("commitTransaction")
            .expect("commitTransaction");
        assert_volumes(&reopened_obj.getattr("Shapes").unwrap(), &[2.0, 3.0]);

        reopened.call_method0("undo").expect("undo");
        assert_volumes(&reopened_obj.getattr("Shapes").unwrap(), &[1.0, 2.0, 3.0]);

        reopened.call_method0("redo").expect("redo");
        assert_volumes(&reopened_obj.getattr("Shapes").unwrap(), &[2.0, 3.0]);

        let name: String = reopened.getattr("Name").unwrap().extract().unwrap();
        app.call_method1("closeDocument", (&name,))
            .expect("closeDocument");
        let _ = std::fs::remove_file(&file);
    });
}
