//! Boot test: `Part` and `ferrocad` in one image, sharing one core.
//!
//! This is the composition the application performs: link both modules and register
//! them as built-ins, so `import Part` and `import FreeCAD` see the same document
//! registry and the same kernel. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// The tests share one interpreter and one document registry; run them one at a time so
/// `ActiveDocument` is not changed underneath a test.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        // One process, one core: both modules report the same application identity.
        let part_id: usize = part.getattr("_kernel_id").unwrap().extract().unwrap();
        let ferrocad_id: usize = ferrocad.getattr("_kernel_id").unwrap().extract().unwrap();
        assert_eq!(part_id, ferrocad_id, "Part and ferrocad were linked as separate cores");

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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

#[test]
fn the_shell_helpers_tessellate_and_dress_up_a_document_shape() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    Python::with_gil(|py| {
        // The shell library puts the facade on `sys.path`; do the same here.
        let facade = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python");
        py.import("sys")
            .unwrap()
            .getattr("path")
            .unwrap()
            .call_method1("insert", (0, facade.to_string_lossy().to_string()))
            .unwrap();
        let shell = PyModule::import(py, "ferrocad_parametric")
            .expect("import the edition's app script");

        // The edition registers Part's display/interaction hooks on `FerroCAD`.
        let ferrocad = PyModule::import(py, "FerroCAD").expect("import FerroCAD");
        assert!(ferrocad.hasattr("display_mesh").unwrap());
        assert!(ferrocad.hasattr("interaction").unwrap());

        // `bootstrap` adds the Box because Part is linked in this image.
        let _: String = shell
            .call_method0("bootstrap")
            .expect("bootstrap")
            .extract()
            .unwrap();

        let meshes: String = shell
            .call_method0("stage_meshes")
            .expect("stage_meshes")
            .extract()
            .unwrap();
        let meshes: serde_json::Value = serde_json::from_str(&meshes).unwrap();
        let box_mesh = meshes
            .as_array()
            .expect("an array")
            .iter()
            .find(|mesh| mesh["object"] == "Box")
            .expect("the Box is tessellated");
        let parts: Vec<&str> = box_mesh["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part.as_str())
            .collect();
        assert!(
            parts.contains(&"Edge1"),
            "the mesh names its edges: {parts:?}"
        );
        assert!(!box_mesh["edges"].as_array().unwrap().is_empty());

        // A manipulator commit: create the fillet, recompute, and re-read the display.
        let result: String = shell
            .call_method1(
                "interaction",
                ("Box", "dress_up", "fillet", "Edge1", "[2.0]"),
            )
            .expect("interaction")
            .extract()
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["ok"], true, "interaction: {result}");

        let after: String = shell
            .call_method0("stage_meshes")
            .unwrap()
            .extract()
            .unwrap();
        let after: serde_json::Value = serde_json::from_str(&after).unwrap();
        let objects: Vec<&str> = after
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|mesh| mesh["object"].as_str())
            .collect();
        assert!(
            !objects.contains(&"Box"),
            "the base is replaced by the feature: {objects:?}"
        );
        let feature = after
            .as_array()
            .unwrap()
            .iter()
            .find(|mesh| mesh["object"] == "Box_Fillet")
            .expect("the fillet is displayed");
        let faces = feature["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part.as_str().is_some_and(|name| name.starts_with("Face")))
            .count();
        assert!(faces > 6, "the fillet adds faces: {faces}");

        // Committing again reuses the feature.
        let again: String = shell
            .call_method1(
                "interaction",
                ("Box", "dress_up", "fillet", "Edge1", "[3.0]"),
            )
            .unwrap()
            .extract()
            .unwrap();
        let again: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert_eq!(again["ok"], true, "second interaction: {again}");
    });
}
