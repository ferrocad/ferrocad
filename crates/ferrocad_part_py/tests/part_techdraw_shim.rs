//! The `python/TechDraw` shim that Draft's hatch object imports.
//!
//! FerroCAD does not implement the TechDraw workbench. `Draft.make_hatch` needs
//! exactly one function from it — `makeGeomHatch` — so `python/TechDraw` provides
//! it over the Part kernel, reading the standard PAT file shipped under
//! `python/Mod/TechDraw/PAT/`. This test runs that shim against a real face.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use std::sync::Once;

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

fn boot() {
    static BOOT: Once = Once::new();
    BOOT.call_once(|| {
        use ferrocad_part_py::Part as part_module;
        use ferrocad_py::ferrocad as ferrocad_module;
        pyo3::append_to_inittab!(ferrocad_module);
        pyo3::append_to_inittab!(part_module);
    });
}

#[test]
fn make_geom_hatch_fills_a_planar_face() {
    boot();
    Python::with_gil(|py| {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .parent()
            .expect("repository root");
        let sys = py.import("sys").expect("sys");
        sys.getattr("path")
            .unwrap()
            .call_method1("insert", (0, repo.join("python").to_str().unwrap()))
            .expect("sys.path.insert");

        let part = PyModule::import(py, "Part").expect("import Part");
        let techdraw = PyModule::import(py, "TechDraw").expect("import TechDraw");

        let box_shape = part
            .call_method1("makeBox", (50.0, 30.0, 1.0))
            .expect("makeBox");
        let faces = box_shape.getattr("Faces").expect("Faces");
        let faces = faces.downcast_into::<PyList>().expect("a list of faces");
        let face = faces
            .iter()
            .find(|face| {
                let normal = face.call_method1("normalAt", (0.0, 0.0)).unwrap();
                let z: f64 = normal.getattr("z").unwrap().extract().unwrap();
                z.abs() > 0.9
            })
            .expect("the box has a face normal to Z");

        let pat = repo.join("python/Mod/TechDraw/PAT/FCPAT.pat");
        let hatch = techdraw
            .call_method1(
                "makeGeomHatch",
                (&face, 1.0, "Horizontal5", pat.to_str().unwrap()),
            )
            .expect("TechDraw.makeGeomHatch");

        let bounds = hatch.getattr("BoundBox").expect("BoundBox");
        let x_length: f64 = bounds.getattr("XLength").unwrap().extract().unwrap();
        let y_length: f64 = bounds.getattr("YLength").unwrap().extract().unwrap();
        assert!((x_length - 50.0).abs() < 1e-6, "hatch XLength {x_length}");
        assert!((y_length - 30.0).abs() < 1e-6, "hatch YLength {y_length}");
        assert!(!hatch
            .call_method0("isNull")
            .unwrap()
            .extract::<bool>()
            .unwrap());
    });
}
