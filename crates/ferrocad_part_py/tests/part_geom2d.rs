//! `Part.Geom2d` and the `Part.Cylinder` surface: the slice of upstream's
//! `Geom2d_tests.py` that the Part module now answers.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::PyModule;

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

#[test]
fn cylinder_is_a_surface_with_a_radius() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder_class = part.getattr("Cylinder").expect("Part.Cylinder");

        // Upstream's default is center (0,0,0), axis +Z, radius 1.
        let cylinder = cylinder_class.call0().expect("Part.Cylinder()");
        assert!(
            (cylinder
                .getattr("Radius")
                .unwrap()
                .extract::<f64>()
                .unwrap()
                - 1.0)
                .abs()
                < 1e-12
        );
        let center = cylinder.getattr("Center").expect("Cylinder.Center");
        for axis in ["x", "y", "z"] {
            assert!(center.getattr(axis).unwrap().extract::<f64>().unwrap().abs() < 1e-12);
        }
        let axis = cylinder.getattr("Axis").expect("Cylinder.Axis");
        assert!((axis.getattr("z").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-12);

        // The radius / center / axis keywords are this slice's extension.
        let sized = cylinder_class
            .call1((2.0,))
            .expect("Part.Cylinder(2.0)");
        assert!(
            (sized.getattr("Radius").unwrap().extract::<f64>().unwrap() - 2.0).abs() < 1e-12
        );

        // It is a surface description, not a B-Rep shape: no shape members are
        // faked. The document primitive stays `Part.makeCylinder`.
        assert!(
            cylinder.getattr("Faces").is_err(),
            "Part.Cylinder is a surface, not a shape"
        );
        assert!(part.getattr("makeCylinder").is_ok());
    });
}

#[test]
fn line2dsegment_to_shape_with_and_without_surface() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector2d = ferrocad.getattr("Vector2d").expect("ferrocad.Vector2d");

        // The upstream test's points, as FreeCAD.Base.Vector2d objects.
        let p1 = vector2d.call1((1.0, -1.0)).expect("Vector2d(1, -1)");
        let p2 = vector2d.call1((2.0, -1.0)).expect("Vector2d(2, -1)");

        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        let segment_class = geom2d
            .getattr("Line2dSegment")
            .expect("Part.Geom2d.Line2dSegment");
        let l12 = segment_class
            .call1((&p1, &p2))
            .expect("Line2dSegment(p1, p2)");

        // (u, v) tuples are accepted too.
        segment_class
            .call1(((0.0, 0.0), (1.0, 1.0)))
            .expect("Line2dSegment((u, v), (u, v))");

        // No surface: a real edge in the XY plane that still reports its pcurve.
        let e1 = l12.call_method0("toShape").expect("toShape()");
        assert!(
            !e1.call_method0("isNull")
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "toShape() produced an edge"
        );
        let edge_class = part.getattr("Edge").expect("Part.Edge");
        assert!(
            e1.is_instance(&edge_class).unwrap(),
            "toShape() returns a Part.Edge"
        );
        let pcurve = e1
            .call_method1("curveOnSurface", (0,))
            .expect("curveOnSurface(0)");
        assert!(!pcurve.is_none(), "a Geom2d edge carries its pcurve");
        assert_eq!(pcurve.get_type().name().unwrap(), "Line2dSegment");
        let start = pcurve.getattr("StartPoint").expect("pcurve.StartPoint");
        assert!((start.getattr("x").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-12);
        assert!((start.getattr("y").unwrap().extract::<f64>().unwrap() + 1.0).abs() < 1e-12);

        // Only index 0 exists; anything else is None rather than an error.
        assert!(
            e1.call_method1("curveOnSurface", (3,))
                .unwrap()
                .is_none(),
            "curveOnSurface(3) is None"
        );

        // With a surface the same edge is returned and the pcurve is still there
        // (the surface is accepted, not projected in this slice).
        let surf = part
            .getattr("Cylinder")
            .expect("Part.Cylinder")
            .call0()
            .expect("Part.Cylinder()");
        let e2 = l12
            .call_method1("toShape", (&surf,))
            .expect("toShape(surface)");
        assert!(
            !e2.call_method0("isNull")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        assert!(
            !e2.call_method1("curveOnSurface", (0,))
                .expect("curveOnSurface(0) with a surface")
                .is_none(),
            "toShape(surface) keeps the pcurve"
        );

        // A bogus surface argument is a TypeError, not a silent success.
        let error = l12
            .call_method1("toShape", (42,))
            .expect_err("toShape(42) is not a surface");
        assert!(
            error.is_instance_of::<pyo3::exceptions::PyTypeError>(py),
            "expected TypeError, got {error}"
        );
    });
}

#[test]
fn bspline2d_insert_knot_raises_type_error() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let curve = part
            .getattr("Geom2d")
            .expect("Part.Geom2d")
            .getattr("BSplineCurve2d")
            .expect("Part.Geom2d.BSplineCurve2d")
            .call0()
            .expect("BSplineCurve2d()");

        // Upstream's test calls this with an extra argument; its
        // `PyArg_ParseTuple("d|id", …)` rejects it as a TypeError, and so does
        // our signature.
        let error = curve
            .call_method1("insertKnot", (0.5, 1, 0.01, 2))
            .expect_err("upstream's Geom2d_tests expects a TypeError here");
        assert!(
            error.is_instance_of::<pyo3::exceptions::PyTypeError>(py),
            "expected TypeError, got {error}"
        );

        // A well-formed call fails loudly too: no 2D B-spline kernel exists, so
        // successfully doing nothing would be a lie.
        let error = curve
            .call_method1("insertKnot", (0.5,))
            .expect_err("insertKnot is not implemented");
        assert!(
            !error.is_instance_of::<pyo3::exceptions::PyTypeError>(py),
            "an in-range call should fail as a Part.OCCError, got {error}"
        );
    });
}
