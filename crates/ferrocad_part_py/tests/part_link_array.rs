//! `Part::LinkArray*` document objects, end to end.
//!
//! Same one-image boot as `part_conformance`: `ferrocad` + `Part` registered as
//! built-ins. CPython initialisation and document globals are process-wide, so a
//! `Mutex` serialises the tests (parallel `with_gil` bootstraps abort the process).

use std::sync::{Mutex, Once};

use pyo3::exceptions::{PyAttributeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

fn vector<'py>(
    module: &Bound<'py, PyAny>,
    x: f64,
    y: f64,
    z: f64,
) -> PyResult<Bound<'py, PyAny>> {
    module.getattr("Vector")?.call1((x, y, z))
}

fn placements<'py>(array: &Bound<'py, PyAny>) -> PyResult<Vec<(f64, f64, f64)>> {
    let list = array.getattr("PlacementList")?;
    let mut out = Vec::new();
    for placement in list.try_iter()? {
        let base = placement?.getattr("Base")?;
        out.push((
            base.getattr("x")?.extract()?,
            base.getattr("y")?.extract()?,
            base.getattr("z")?.extract()?,
        ));
    }
    Ok(out)
}

fn distance(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// Set `ElementList[index].Suppressed`.
fn set_suppressed(array: &Bound<'_, PyAny>, index: usize, suppressed: bool) -> PyResult<()> {
    let elements = array.getattr("ElementList")?.downcast_into::<PyList>()?;
    elements.get_item(index)?.setattr("Suppressed", suppressed)
}

fn suppressed_indices(array: &Bound<'_, PyAny>) -> PyResult<Vec<usize>> {
    let elements = array.getattr("ElementList")?.downcast_into::<PyList>()?;
    let mut indices = Vec::new();
    for (index, element) in elements.iter().enumerate() {
        if element.getattr("Suppressed")?.extract::<bool>()? {
            indices.push(index);
        }
    }
    Ok(indices)
}

fn status(array: &Bound<'_, PyAny>) -> PyResult<String> {
    array.call_method0("getStatusString")?.extract()
}

fn new_document<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    // Importing `Part` runs its module init, which registers the Part object
    // types (including the link arrays) on the shared kernel.
    PyModule::import(py, "Part")?;
    PyModule::import(py, "ferrocad")?.call_method1("newDocument", (name,))
}

fn close_document(py: Python<'_>, name: &str) {
    let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
    ferrocad
        .call_method1("closeDocument", (name,))
        .expect("closeDocument");
}

#[test]
fn point_array_deduplicates_vertices_and_offsets_from_the_source() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = ferrocad
            .call_method1("newDocument", ("RustLinkArrayPoint",))
            .expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let points = doc
            .call_method1("addObject", ("Part::Feature", "Points"))
            .expect("add Points feature");

        // A compound of three vertices; the second is duplicated to exercise the
        // kernel-tolerance deduplication.
        let vertices = PyList::new(
            py,
            [
                part.call_method1("Vertex", (vector(&ferrocad, 2.0, 3.0, 4.0).unwrap(),))
                    .unwrap(),
                part.call_method1("Vertex", (vector(&ferrocad, 2.0, 3.0, 4.0).unwrap(),))
                    .unwrap(),
                part.call_method1("Vertex", (vector(&ferrocad, 12.0, 3.0, 4.0).unwrap(),))
                    .unwrap(),
                part.call_method1("Vertex", (vector(&ferrocad, 12.0, 13.0, 4.0).unwrap(),))
                    .unwrap(),
            ],
        )
        .unwrap();
        let compound = part
            .call_method1("makeCompound", (vertices,))
            .expect("makeCompound");
        points.setattr("Shape", compound).expect("points.Shape");

        let array = doc
            .call_method1("addObject", ("Part::LinkArrayPoint", "Array"))
            .expect("add LinkArrayPoint");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("ShowElement", false).expect("ShowElement");
        array.setattr("PointObject", &points).expect("PointObject");

        doc.call_method0("recompute").expect("recompute");

        assert_eq!(
            array
                .call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );
        assert_eq!(
            array.getattr("ElementCount").unwrap().extract::<i64>().unwrap(),
            3
        );

        // The placements are offsets from the first vertex (the default origin).
        let result = placements(&array).unwrap();
        assert_eq!(result.len(), 3);
        assert!(distance(result[0], (0.0, 0.0, 0.0)) < 1e-9);
        assert!(distance(result[1], (10.0, 0.0, 0.0)) < 1e-9);
        assert!(distance(result[2], (10.0, 10.0, 0.0)) < 1e-9);

        // Unsetting the point object is silent and empties the array.
        array.setattr("PointObject", py.None()).expect("clear PointObject");
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(
            array
                .call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );
        assert_eq!(
            array.getattr("ElementCount").unwrap().extract::<i64>().unwrap(),
            0
        );

        ferrocad
            .call_method1("closeDocument", ("RustLinkArrayPoint",))
            .expect("closeDocument");
    });
}

#[test]
fn path_array_follows_connected_edges() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = ferrocad
            .call_method1("newDocument", ("RustLinkArrayPath",))
            .expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let path = doc
            .call_method1("addObject", ("Part::Feature", "Path"))
            .expect("add Path feature");

        let points = PyList::new(
            py,
            [
                vector(&ferrocad, 0.0, 0.0, 0.0).unwrap(),
                vector(&ferrocad, 10.0, 0.0, 0.0).unwrap(),
                vector(&ferrocad, 10.0, 10.0, 0.0).unwrap(),
            ],
        )
        .unwrap();
        let wire = part.call_method1("makePolygon", (points,)).expect("makePolygon");
        path.setattr("Shape", wire).expect("path.Shape");

        let array = doc
            .call_method1("addObject", ("Part::LinkArrayPath", "Array"))
            .expect("add LinkArrayPath");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("ShowElement", false).expect("ShowElement");
        let subs = PyList::new(py, ["Edge1", "Edge2"]).unwrap();
        array.setattr("Path", (&path, subs)).expect("Path");
        array.setattr("Count", 5).expect("Count");

        doc.call_method0("recompute").expect("recompute");

        assert_eq!(
            array
                .call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );
        assert_eq!(
            array.getattr("ElementCount").unwrap().extract::<i64>().unwrap(),
            5
        );
        let expected = [
            (0.0, 0.0, 0.0),
            (5.0, 0.0, 0.0),
            (10.0, 0.0, 0.0),
            (10.0, 5.0, 0.0),
            (10.0, 10.0, 0.0),
        ];
        let result = placements(&array).unwrap();
        assert_eq!(result.len(), expected.len());
        for (placement, point) in result.iter().zip(expected) {
            assert!(distance(*placement, point) < 1e-7);
        }

        ferrocad
            .call_method1("closeDocument", ("RustLinkArrayPath",))
            .expect("closeDocument");
    });
}

#[test]
fn linear_array_rejects_collapsing_suppressed_elements() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustLinkArrayCollapse").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayLinear", "Array"))
            .expect("add LinkArrayLinear");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("Occurrences", 3).expect("Occurrences");
        array.setattr("Occurrences2", 3).expect("Occurrences2");
        doc.call_method0("recompute").expect("recompute");

        set_suppressed(&array, 1, true).expect("suppress element 1");

        // Before the array executes again, the collapse guard reports the value
        // rejection the upstream `onChanged` hook raises.
        let error = array
            .setattr("ShowElement", false)
            .expect_err("collapse must be rejected");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        assert!(array.getattr("ShowElement").unwrap().extract::<bool>().unwrap());

        // The recompute refreshes `ShowElement`'s immutable status, so the same
        // write now fails the property-immutability check as `AttributeError`.
        doc.call_method0("recompute").expect("recompute");
        let error = array
            .setattr("ShowElement", false)
            .expect_err("collapse must stay rejected");
        assert!(error.is_instance_of::<PyAttributeError>(py), "{error}");
        assert!(array.getattr("ShowElement").unwrap().extract::<bool>().unwrap());
        // The explicit setter maps the same rejection to the same exception.
        let error = array
            .call_method1("setPropertyByName", ("ShowElement", false))
            .expect_err("setPropertyByName must stay rejected");
        assert!(error.is_instance_of::<PyAttributeError>(py), "{error}");

        // Restoring the element makes collapsing legal again.
        set_suppressed(&array, 1, false).expect("restore element 1");
        doc.call_method0("recompute").expect("recompute");
        array.setattr("ShowElement", false).expect("collapse allowed");
        assert!(!array.getattr("ShowElement").unwrap().extract::<bool>().unwrap());
        doc.call_method0("recompute").expect("recompute");
        assert_eq!(status(&array).unwrap(), "Valid");

        close_document(py, "RustLinkArrayCollapse");
    });
}

#[test]
fn circular_array_reports_the_execute_error_message() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let doc = new_document(py, "RustLinkArrayError").expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayCircular", "Array"))
            .expect("add LinkArrayCircular");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("NumberCircles", 2).expect("NumberCircles");
        array
            .setattr("RadialDistance", 1_000_000.0)
            .expect("RadialDistance");
        array
            .setattr("TangentialDistance", 0.001)
            .expect("TangentialDistance");
        let subs = PyList::new(py, ["Y_Axis"]).unwrap();
        array
            .setattr("BinormalAxis", (py.None(), subs))
            .expect("BinormalAxis");

        doc.call_method0("recompute").expect("recompute");

        let status = status(&array).unwrap();
        assert!(status.starts_with("Invalid"), "{status}");
        assert!(status.contains("10000"), "{status}");

        close_document(py, "RustLinkArrayError");
    });
}

#[test]
fn restore_marks_registered_objects_for_recompute() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let name = "RustLinkArrayRestore";
        let doc = new_document(py, name).expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayLinear", "Array"))
            .expect("add LinkArrayLinear");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("Occurrences", 3).expect("Occurrences");
        array.setattr("Occurrences2", 3).expect("Occurrences2");
        doc.call_method0("recompute").expect("recompute");

        // The resize is pending, so element 5 still has the old (1, 2) position.
        array.setattr("Occurrences2", 4).expect("Occurrences2");
        set_suppressed(&array, 5, true).expect("suppress element 5");

        let tempfile = PyModule::import(py, "tempfile").expect("import tempfile");
        let directory: String = tempfile
            .call_method0("mkdtemp")
            .expect("mkdtemp")
            .extract()
            .expect("path");
        let path = format!("{directory}/array.FCStd");
        doc.call_method1("saveAs", (&path,)).expect("saveAs");
        close_document(py, name);

        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let reopened = ferrocad
            .call_method1("openDocument", (&path,))
            .expect("openDocument");
        let array = reopened
            .call_method1("getObject", ("Array",))
            .expect("getObject");

        // `from_saved` marks registered types for execution, so this recompute
        // re-derives the suppression from the persisted (1, 2) position.
        reopened.call_method0("recompute").expect("recompute");
        assert_eq!(status(&array).unwrap(), "Valid");
        assert_eq!(suppressed_indices(&array).unwrap(), vec![6]);

        close_document(py, name);
        let shutil = PyModule::import(py, "shutil").expect("import shutil");
        shutil
            .call_method1("rmtree", (&directory,))
            .expect("rmtree");
    });
}

#[test]
fn linear_array_reports_each_grid_element() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let _ = &part;

        let doc = ferrocad
            .call_method1("newDocument", ("RustLinkArrayLinear",))
            .expect("newDocument");
        let source = doc
            .call_method1("addObject", ("Part::Box", "Source"))
            .expect("add Source box");
        let array = doc
            .call_method1("addObject", ("Part::LinkArrayLinear", "Array"))
            .expect("add LinkArrayLinear");
        array.setattr("LinkedObject", &source).expect("LinkedObject");
        array.setattr("Occurrences", 3).expect("Occurrences");
        array.setattr("Occurrences2", 3).expect("Occurrences2");
        array.setattr("ShowElement", false).expect("ShowElement");

        doc.call_method0("recompute").expect("recompute");

        assert_eq!(
            array
                .call_method0("getStatusString")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Valid"
        );
        assert_eq!(
            array.getattr("ElementCount").unwrap().extract::<i64>().unwrap(),
            9
        );

        ferrocad
            .call_method1("closeDocument", ("RustLinkArrayLinear",))
            .expect("closeDocument");
    });
}
