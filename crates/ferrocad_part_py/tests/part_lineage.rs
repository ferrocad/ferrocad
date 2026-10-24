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

/// The first forward-map key of `shape`'s lineage: a name an operation preserved.
fn mapped_name(shape: &Bound<'_, PyAny>) -> String {
    let part_shape = shape.extract::<PyRef<'_, PartShape>>().expect("PartShape");
    let lineage = part_shape.lineage().expect("the cut carries a lineage");
    lineage
        .map()
        .iter()
        .next()
        .map(|(name, _)| name.to_string())
        .expect("the cut has a forward map")
}

fn object_name(object: &Bound<'_, PyAny>) -> String {
    object.getattr("Name").unwrap().extract().unwrap()
}

/// A cut of a 1 mm box out of a 2 mm box, both at the origin, recomputed.
/// This is the primitive path's golden case: it produces both forward and
/// reverse lineage relations.
fn cut_document<'py>(
    ferrocad: &Bound<'py, PyModule>,
    name: &str,
) -> (Bound<'py, PyAny>, Bound<'py, PyAny>) {
    let doc = ferrocad
        .call_method1("newDocument", (name,))
        .expect("newDocument");
    for object_name in ["BoxA", "BoxB"] {
        let object = doc
            .call_method1("addObject", ("Part::Box", object_name))
            .expect("Part::Box");
        let size = if object_name == "BoxA" { 2.0 } else { 1.0 };
        for property in ["Length", "Width", "Height"] {
            object.setattr(property, size).expect("box dimension");
        }
    }
    let cut = doc
        .call_method1("addObject", ("Part::Cut", "Cut"))
        .expect("Part::Cut");
    let box_a = doc.call_method1("getObject", ("BoxA",)).unwrap();
    let box_b = doc.call_method1("getObject", ("BoxB",)).unwrap();
    cut.setattr("Base", box_a).expect("Cut.Base");
    cut.setattr("Tool", box_b).expect("Cut.Tool");
    doc.call_method0("recompute").expect("recompute");
    (doc, cut)
}

#[test]
fn a_retired_sub_element_name_fails_instead_of_moving() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let (_doc, cut) = cut_document(&ferrocad, "RustLineageResolve");

        let mapped = mapped_name(&cut.getattr("Shape").unwrap());
        let resolved = cut
            .call_method1("getSubObject", (&mapped, 1))
            .expect("getSubObject");
        assert_eq!(
            object_name(&resolved),
            "Cut",
            "a name in the lineage still resolves to its owner"
        );

        // A name the non-empty lineage does not contain is retired: it must not
        // silently select whatever face now has that ordinal.
        let retired = cut
            .call_method1("getSubObject", ("Face9999", 1))
            .expect("getSubObject");
        assert!(retired.is_none(), "a retired name does not resolve");

        ferrocad
            .call_method1("closeDocument", ("RustLineageResolve",))
            .expect("closeDocument");
    });
}

#[test]
fn a_shape_without_a_lineage_keeps_the_positional_fallback() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let doc = ferrocad
            .call_method1("newDocument", ("RustLineagePositional",))
            .expect("newDocument");
        let feature = doc
            .call_method1("addObject", ("Part::Feature", "Primitive"))
            .expect("Part::Feature");
        let shape = part
            .call_method1("makeBox", (2.0, 2.0, 2.0))
            .expect("makeBox");
        feature.setattr("Shape", shape).expect("Primitive.Shape");

        // No lineage: `Face1` is the first face, exactly as before L5.
        let resolved = feature
            .call_method1("getSubObject", ("Face1", 1))
            .expect("getSubObject");
        assert_eq!(object_name(&resolved), "Primitive");
        // ...and a face the box does not have is missing, not accepted blindly.
        let missing = feature
            .call_method1("getSubObject", ("Face9999", 1))
            .expect("getSubObject");
        assert!(missing.is_none());

        ferrocad
            .call_method1("closeDocument", ("RustLineagePositional",))
            .expect("closeDocument");
    });
}

#[test]
fn a_boolean_lineage_survives_an_fcstd_save_and_open() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let _part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let (doc, cut) = cut_document(&ferrocad, "RustLineageFcstd");
        let before = reverse_map(&cut.getattr("Shape").unwrap()).expect("reverse map");
        assert!(
            !before.is_empty(),
            "the cut carries a non-empty ElementReverseMap"
        );

        let path = std::env::temp_dir().join(format!(
            "ferrocad_part_lineage_{}.FCStd",
            std::process::id()
        ));
        doc.call_method1("saveAs", (path.to_string_lossy().as_ref(),))
            .expect("saveAs .FCStd");
        let original_name: String = doc.getattr("Name").unwrap().extract().unwrap();
        ferrocad
            .call_method1("closeDocument", (&original_name,))
            .expect("closeDocument");

        let reopened = ferrocad
            .call_method1("openDocument", (path.to_string_lossy().as_ref(),))
            .expect("openDocument .FCStd");
        let reopened_cut = reopened
            .call_method1("getObject", ("Cut",))
            .expect("getObject Cut");
        let after = reverse_map(&reopened_cut.getattr("Shape").unwrap()).expect("reverse map");
        assert_eq!(after, before, "ElementReverseMap survives the round trip");

        let reopened_name: String = reopened.getattr("Name").unwrap().extract().unwrap();
        ferrocad
            .call_method1("closeDocument", (&reopened_name,))
            .expect("closeDocument");
        let _ = std::fs::remove_file(&path);
    });
}
