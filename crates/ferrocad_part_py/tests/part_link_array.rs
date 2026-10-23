//! `Part::LinkArray*` document objects, end to end.
//!
//! Same one-image boot as `part_conformance`: `ferrocad` + `Part` registered as
//! built-ins. CPython initialisation and document globals are process-wide, so a
//! `Mutex` serialises the tests (parallel `with_gil` bootstraps abort the process).

use std::sync::{Mutex, Once};

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


