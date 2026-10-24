//! S6a: `Part.Shape` carries an operation's lineage and exposes its reverse map.
//!
//! The lineage is metadata beside the shape: these tests assert that a boolean
//! result carries one, that `ElementReverseMap` reports exactly the stored
//! relations, and that the forward map/version/content stay off.

use std::sync::{Mutex, Once};

use ferrocad_part_py::PartShape;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register both built-in modules. CPython initialisation is process-wide, so the
/// mutex below serialises the tests (parallel `with_gil` bootstraps abort).
fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

/// `Shape.ElementReverseMap` as sorted `(output, [inputs...])` pairs.
fn reverse_map(shape: &Bound<'_, PyAny>) -> PyResult<Vec<(String, Vec<String>)>> {
    let map = shape
        .getattr("ElementReverseMap")?
        .downcast_into::<PyDict>()?;
    let mut entries = Vec::new();
    for (output, inputs) in map.iter() {
        entries.push((output.extract::<String>()?, inputs.extract::<Vec<String>>()?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

#[test]
fn boolean_lineage_is_carried_and_copy_preserves_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let big = part
            .call_method1("makeBox", (2.0, 2.0, 2.0))
            .expect("box");
        let small = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("box");

        // A primitive has no operation history, so it carries no lineage and its
        // reverse map is empty.
        assert!(reverse_map(&big).expect("primitive reverse map").is_empty());
        assert!(big
            .extract::<PyRef<'_, PartShape>>()
            .expect("PartShape")
            .lineage()
            .is_none());

        // A boolean result carries the lineage folded from its operation history.
        let cut = big.call_method1("cut", (&small,)).expect("cut");
        let entries = reverse_map(&cut).expect("boolean reverse map");
        assert!(
            !entries.is_empty(),
            "a boolean result should carry a lineage"
        );

        // Every key and producer list comes from the stored lineage: the Python
        // view is the lineage's reverse map, nothing invented on top.
        let shape = cut.extract::<PyRef<'_, PartShape>>().expect("PartShape");
        let lineage = shape
            .lineage()
            .expect("the boolean result carries a lineage");
        let mut expected: Vec<(String, Vec<String>)> = lineage
            .reverse()
            .iter()
            .map(|(output, inputs)| (output.to_string(), inputs.to_vec()))
            .collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(entries, expected);

        // The rest of the element-map surface is unchanged.
        assert_eq!(
            cut.getattr("ElementMapVersion")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            ""
        );
        assert_eq!(
            cut.getattr("ElementMapSize")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            0
        );
        assert_eq!(
            cut.getattr("ElementMap")
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            cut.getattr("Content").unwrap().extract::<String>().unwrap(),
            "<ElementMap/>\n"
        );

        // `copy()` and the copy constructor preserve the lineage;
        // `copy(noElementMap=True)` returns a bare shape.
        let copied = cut.call_method0("copy").expect("Shape.copy");
        assert_eq!(reverse_map(&copied).unwrap(), entries);

        let bare = cut.call_method1("copy", (true,)).expect("Shape.copy(true)");
        assert!(reverse_map(&bare).unwrap().is_empty());

        let shape_class = part.getattr("Shape").unwrap();
        let wrapped = shape_class.call1((&cut,)).expect("Part.Shape(shape)");
        assert_eq!(reverse_map(&wrapped).unwrap(), entries);
    });
}
