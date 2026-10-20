//! Boot test: `Part` and `ferrocad` in one image, sharing one core.
//!
//! This is the composition the application performs: link both modules and register
//! them as built-ins, so `import Part` and `import FreeCAD` see the same document
//! registry and the same kernel. It needs OCCT (see `docs/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Register both built-in modules. Must run before the interpreter initialises
/// (`auto-initialize` does that on the first `Python::with_gil`).
///
/// `append_to_inittab` panics once an interpreter is running, so the registration is
/// guarded: whichever test boots first appends both modules, and the others reuse them.
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

#[test]
fn part_shares_one_core_with_ferrocad() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        // `Part.makeBox` produces a `Part.Shape`.
        let shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("Part.makeBox");
        assert_eq!(
            shape.get_type().name().unwrap(),
            "Shape",
            "Part.makeBox must return a Part.Shape"
        );

        // A `Part::Feature` in a document round-trips it through the converter hook:
        // `obj.Shape = shape` stores a `Part::PropertyPartShape`, `obj.Shape` reads it
        // back as a `Part.Shape`. This only works if both modules are in one image.
        let doc = ferrocad
            .call_method1("newDocument", ("PartDemo",))
            .expect("newDocument");
        let obj = doc
            .call_method1("addObject", ("Part::Feature", "Box"))
            .expect("addObject Part::Feature");

        // Default: an empty (null) shape.
        let before = obj.getattr("Shape").expect("obj.Shape (default)");
        assert!(
            before.call_method0("isNull").unwrap().extract::<bool>().unwrap(),
            "a fresh Part::Feature has a null Shape"
        );

        obj.setattr("Shape", &shape).expect("obj.Shape = shape");

        let after = obj.getattr("Shape").expect("obj.Shape");
        assert_eq!(after.get_type().name().unwrap(), "Shape");
        assert!(
            !after.call_method0("isNull").unwrap().extract::<bool>().unwrap(),
            "the assigned shape is no longer null"
        );
        assert!(
            shape.call_method1("isSame", (&after,)).unwrap().extract::<bool>().unwrap(),
            "the round-tripped shape is the same kernel object"
        );

        // The shape is real geometry: it serialises to non-empty BREP.
        let brep: Vec<u8> = after
            .call_method0("exportBrepToString")
            .expect("exportBrepToString")
            .extract()
            .expect("BREP bytes");
        assert!(!brep.is_empty(), "an OCCT box must serialise to BREP");
    });
}

#[test]
fn a_fillet_feature_rebuilds_through_python() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        let doc = ferrocad
            .call_method1("newDocument", ("FilletDemo",))
            .expect("newDocument");
        let base = doc
            .call_method1("addObject", ("Part::Feature", "Box"))
            .expect("addObject Part::Feature");
        let shape = part
            .call_method1("makeBox", (10.0, 10.0, 10.0))
            .expect("Part.makeBox");
        base.setattr("Shape", &shape).expect("box.Shape");

        // `Part::Fillet` is a registered document-object type; `Base` is a LinkSub.
        let fillet = doc
            .call_method1("addObject", ("Part::Fillet", "Fillet"))
            .expect("addObject Part::Fillet");
        let subs = pyo3::types::PyList::new(py, ["Edge1"]).unwrap();
        fillet
            .setattr("Base", (&base, subs))
            .expect("fillet.Base = (box, [\"Edge1\"])");
        fillet.setattr("Radius", 2.0).expect("fillet.Radius = 2.0");

        doc.call_method0("recompute").expect("recompute");

        let output = fillet.getattr("Shape").expect("fillet.Shape");
        assert_eq!(output.get_type().name().unwrap(), "Shape");
        assert!(
            !output
                .call_method0("isNull")
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the fillet feature computed a shape"
        );
    });
}
