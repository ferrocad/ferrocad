//! `Part::FaceMakerBuildFace` splits an annulus cut by connecting segments into
//! its bounded regions, the way upstream's `FaceMakerBuildFace` does.
//!
//! Runs in the same image as `part_curves`: `ferrocad` + `Part` as built-ins, one
//! core, one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use pyo3::prelude::*;

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

/// The upstream `testBuildFaceDirectMakeFace` geometry: two ellipses joined by
/// two radial segments, which bound the inner disc and the two halves of the
/// annulus.
const ANNULUS: &str = r#"
import math
from ferrocad import Vector
outer = Part.Ellipse(Vector(0, 0, 0), 100, 60)
inner = Part.Ellipse(Vector(0, 0, 0), 50, 30)
u1, u2 = -math.radians(15), math.radians(15)
wires = [
    Part.Wire([outer.toShape()]),
    Part.Wire([inner.toShape()]),
    Part.Wire([Part.LineSegment(outer.value(u1), inner.value(u1)).toShape()]),
    Part.Wire([Part.LineSegment(outer.value(u2), inner.value(u2)).toShape()]),
]
"#;

#[test]
fn build_face_splits_the_annulus() {
    boot();
    Python::with_gil(|py| {
        let part = py.import("Part").expect("import Part");
        let globals = pyo3::types::PyDict::new(py);
        globals.set_item("Part", &part).unwrap();
        let code = std::ffi::CString::new(ANNULUS).expect("no NUL");
        py.run(&code, Some(&globals), None)
            .expect("build the annulus");

        // Upstream's `testBuildFaceDirectMakeFace`: at least two faces (the two
        // halves of the annulus, plus the inner disc, for three in total).
        let face = part
            .call_method1(
                "makeFace",
                (
                    globals.get_item("wires").unwrap().unwrap(),
                    "Part::FaceMakerBuildFace",
                ),
            )
            .expect("makeFace(BuildFace)");
        assert!(!face.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        let faces = face.getattr("Faces").expect("Faces").len().unwrap();
        assert!(
            faces >= 2,
            "BuildFace should split the annulus, got {faces} face(s)"
        );
    });
}
