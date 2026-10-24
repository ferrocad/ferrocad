//! `Part.ShapeFix` and `Part.ChFi2d`, end to end.
//!
//! Runs in the same image as `part_boot`: `ferrocad` + `Part` as built-ins, one
//! core, one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).
//!
//! These are the Rust-side checks that the new submodules are really wired to
//! OCCT; the upstream `TestPartApp` cases in `part_conformance` remain the
//! authority on FreeCAD parity.

use pyo3::exceptions::PyTypeError;
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

fn is_type_error(py: Python<'_>, error: &PyErr) -> bool {
    error.is_instance_of::<PyTypeError>(py)
}

#[test]
fn shape_fix_wraps_each_occt_fixer() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape_fix = part.getattr("ShapeFix").expect("Part.ShapeFix");

        for name in ["Root", "Shape", "Solid", "Shell", "Face", "Wire", "Edge"] {
            assert!(shape_fix.hasattr(name).unwrap(), "Part.ShapeFix.{name}");
        }

        // Root: the real ShapeFix_Root precision/tolerance limiters.
        let root = shape_fix.getattr("Root").unwrap().call0().unwrap();
        root.setattr("Precision", 0.0).unwrap();
        assert_eq!(root.getattr("Precision").unwrap().extract::<f64>().unwrap(), 0.0);
        root.setattr("MinTolerance", 0.0).unwrap();
        assert_eq!(
            root.getattr("MinTolerance").unwrap().extract::<f64>().unwrap(),
            0.0
        );
        root.setattr("MaxTolerance", 0.5).unwrap();
        assert_eq!(
            root.getattr("MaxTolerance").unwrap().extract::<f64>().unwrap(),
            0.5
        );
        let limited = root
            .call_method1("limitTolerance", (0.25,))
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!((limited - 0.25).abs() < 1e-12);

        // The constructors reject a list, as upstream's typed parsers do.
        assert!(is_type_error(py, &shape_fix.getattr("Root").unwrap().call1((PyList::empty(py),)).unwrap_err()));
        assert!(is_type_error(py, &shape_fix.getattr("Wire").unwrap().call1((PyList::empty(py),)).unwrap_err()));
        assert!(is_type_error(py, &shape_fix.getattr("Edge").unwrap().call1((PyList::empty(py),)).unwrap_err()));

        // A real planar face, fixed by ShapeFix_Shape.
        let plane = part.getattr("Plane").unwrap().call0().unwrap();
        let face = plane
            .call_method1("toShape", (-1.0, 1.0, -1.0, 1.0))
            .unwrap();
        let fixer = shape_fix.getattr("Shape").unwrap().call1((&face,)).unwrap();
        fixer.call_method1("init", (&face,)).unwrap();
        // A valid face needs no fixes: `perform()` reports whether *anything* was
        // changed, so `false` is the success case here.
        fixer.call_method0("perform").unwrap();
        let fixed = fixer.call_method0("shape").unwrap();
        assert!(
            fixed
                .is_instance(&part.getattr("Shape").unwrap())
                .unwrap(),
            "ShapeFix.Shape.shape() returns a Part.Shape"
        );

        // Each tool accessor returns the matching fixer object.
        assert_eq!(
            fixer.call_method0("fixFaceTool").unwrap().get_type().name().unwrap(),
            "Face"
        );
        assert_eq!(
            fixer.call_method0("fixWireTool").unwrap().get_type().name().unwrap(),
            "Wire"
        );
        assert_eq!(
            fixer.call_method0("fixEdgeTool").unwrap().get_type().name().unwrap(),
            "Edge"
        );

        // ShapeFix_Wire over the face's outer wire: real edge count and state.
        let outer = face.getattr("OuterWire").unwrap();
        let wire_fixer = shape_fix
            .getattr("Wire")
            .unwrap()
            .call1((&outer, &face, 1e-7))
            .unwrap();
        assert_eq!(
            wire_fixer
                .call_method0("numberOfEdges")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            4
        );
        assert!(wire_fixer.call_method0("isLoaded").unwrap().extract::<bool>().unwrap());
        assert!(wire_fixer.call_method0("isReady").unwrap().extract::<bool>().unwrap());
        let edge_tool = wire_fixer.call_method0("fixEdgeTool").unwrap();
        // `face.Edge1` is a Part.Edge; both pcurve overloads are accepted, and a
        // bare face (not an edge) is a TypeError.
        let edge1 = face.getattr("Edge1").unwrap();
        edge_tool
            .call_method1("fixRemovePCurve", (&edge1, &face))
            .unwrap();
        edge_tool
            .call_method1(
                "fixRemovePCurve",
                (&edge1, face.getattr("Surface").unwrap(), face.getattr("Placement").unwrap()),
            )
            .unwrap();
        assert!(is_type_error(
            py,
            &edge_tool.call_method1("fixRemovePCurve", (&face,)).unwrap_err()
        ));
    });
}

#[test]
fn chfi2d_fillets_and_chamfers() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point = |x: f64, y: f64, z: f64| {
            vector.call1((x, y, z)).expect("a Vector").unbind()
        };

        let edge1 = part
            .call_method1("makeLine", (point(0.0, 0.0, 0.0), point(0.0, 10.0, 0.0)))
            .unwrap();
        let edge2 = part
            .call_method1("makeLine", (point(0.0, 10.0, 0.0), point(10.0, 10.0, 0.0)))
            .unwrap();
        let wire = part
            .call_method1("Wire", (PyList::new(py, [&edge1, &edge2]).unwrap(),))
            .unwrap();
        let plane = part.getattr("Plane").unwrap().call0().unwrap();
        let chfi2d = part.getattr("ChFi2d").expect("Part.ChFi2d");

        // FilletAlgo: a quarter-circle of radius 1 at the corner.
        let fillet = chfi2d
            .getattr("FilletAlgo")
            .unwrap()
            .call1((&edge1, &edge2, &plane))
            .unwrap();
        assert!(fillet
            .call_method1("perform", (1.0,))
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert_eq!(
            fillet
                .call_method1("numberOfResults", (point(0.0, 10.0, 0.0),))
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            1
        );
        let result = fillet
            .call_method1("result", (point(0.0, 10.0, 0.0),))
            .unwrap();
        assert_eq!(result.len().unwrap(), 3);
        let curve = result.get_item(0).unwrap().getattr("Curve").unwrap();
        let radius: f64 = curve.getattr("Radius").unwrap().extract().unwrap();
        assert!((radius - 1.0).abs() < 1e-9, "fillet radius is 1.0");

        // AnaFilletAlgo: the analytic form gives the same radius.
        let ana = chfi2d
            .getattr("AnaFilletAlgo")
            .unwrap()
            .call1((&edge1, &edge2, &plane))
            .unwrap();
        assert!(ana
            .call_method1("perform", (1.0,))
            .unwrap()
            .extract::<bool>()
            .unwrap());
        let ana_result = ana.call_method0("result").unwrap();
        let ana_radius: f64 = ana_result
            .get_item(0)
            .unwrap()
            .getattr("Curve")
            .unwrap()
            .getattr("Radius")
            .unwrap()
            .extract()
            .unwrap();
        assert!((ana_radius - 1.0).abs() < 1e-9, "analytic fillet radius is 1.0");

        // ChamferAPI over a wire: a straight chamfer edge.
        let chamfer = chfi2d
            .getattr("ChamferAPI")
            .unwrap()
            .call1((&wire,))
            .unwrap();
        assert!(chamfer.call_method0("perform").unwrap().extract::<bool>().unwrap());
        let chamfer_result = chamfer.call_method1("result", (1.0, 1.0)).unwrap();
        let curve = chamfer_result
            .get_item(0)
            .unwrap()
            .getattr("Curve")
            .unwrap();
        assert_eq!(curve.get_type().name().unwrap(), "Line");

        // A single edge is not a wire for the ChamferAPI constructor, and a
        // bare `perform()` without a radius is a TypeError.
        assert!(is_type_error(
            py,
            &chfi2d.getattr("ChamferAPI").unwrap().call1((&edge1,)).unwrap_err()
        ));
        assert!(is_type_error(py, &fillet.call_method0("perform").unwrap_err()));
        // Without a shape, FilletAlgo.perform raises (OCCT raises).
        let empty = chfi2d.getattr("FilletAlgo").unwrap().call0().unwrap();
        assert!(empty.call_method1("perform", (1.0,)).is_err());
    });
}
