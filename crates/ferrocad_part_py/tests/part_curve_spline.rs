//! `Part.BSplineCurve` pole/knot construction, the analytic curve transforms and
//! `Part.Ellipse.toShape()`, end to end.
//!
//! These are the behaviours upstream's `PartTestBSplineCurve` and
//! `PartTestCurveToNurbs` exercise. It runs in the same image as `part_boot`
//! (`ferrocad` + `Part` as built-ins, one OCCT backend) and needs OCCT (see
//! `docs/developer/architecture/occt-bundling.md`).

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

fn occ_error(py: Python<'_>) -> Bound<'_, pyo3::types::PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<pyo3::types::PyType>()
        .expect("OCCError is a type")
}

/// Upstream's `PartTestBSplineCurve.setUp`: a quadratic through three poles.
fn three_pole_spline<'py>(part: &Bound<'py, PyModule>, vector: &Bound<'py, PyAny>) -> Bound<'py, PyAny> {
    let poles = PyList::new(
        part.py(),
        [
            vector.call1((0.0, 0.0, 0.0)).unwrap(),
            vector.call1((1.0, 1.0, 0.0)).unwrap(),
            vector.call1((2.0, 0.0, 0.0)).unwrap(),
        ],
    )
    .expect("pole list");
    let spline = part
        .getattr("BSplineCurve")
        .expect("Part.BSplineCurve")
        .call0()
        .expect("Part.BSplineCurve()");
    spline
        .call_method1("buildFromPoles", (poles,))
        .expect("buildFromPoles(poles)");
    spline
}

#[test]
fn build_from_poles_mults_knots_reads_its_state_back() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");

        // The upstream `PartTestBSplineCurve` NURBS.
        let poles = PyList::new(
            py,
            [
                vector.call1((0.0, 0.0, 0.0)).unwrap(),
                vector.call1((1.0, 1.0, 0.0)).unwrap(),
                vector.call1((2.0, 0.0, 0.0)).unwrap(),
                vector.call1((1.0, -1.0, 0.0)).unwrap(),
            ],
        )
        .expect("pole list");
        let nurbs = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("Part.BSplineCurve()");
        nurbs
            .call_method1(
                "buildFromPolesMultsKnots",
                (poles, (3, 1, 3), (0.0, 0.5, 1.0), false, 2),
            )
            .expect("buildFromPolesMultsKnots(poles, (3, 1, 3), (0, 0.5, 1), False, 2)");

        assert_eq!(nurbs.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(nurbs.getattr("NbPoles").unwrap().extract::<i32>().unwrap(), 4);
        assert_eq!(nurbs.getattr("NbKnots").unwrap().extract::<i32>().unwrap(), 3);
        assert_eq!(
            nurbs.getattr("KnotSequence").unwrap().extract::<Vec<f64>>().unwrap(),
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0]
        );
        assert_eq!(
            nurbs.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(),
            0.0
        );
        assert_eq!(
            nurbs.getattr("LastParameter").unwrap().extract::<f64>().unwrap(),
            1.0
        );
        assert_eq!(
            nurbs.getattr("Continuity").unwrap().extract::<String>().unwrap(),
            "C1"
        );

        // `getPoles`/`getWeights`/`getPolesAndWeights` mirror the input.
        let poles_back = nurbs.call_method0("getPoles").expect("getPoles");
        assert_eq!(poles_back.len().unwrap(), 4);
        assert_eq!(
            nurbs.call_method0("getWeights").unwrap().extract::<Vec<f64>>().unwrap(),
            vec![1.0, 1.0, 1.0, 1.0]
        );
        let first = nurbs.call_method1("getPole", (1,)).expect("getPole(1)");
        assert!(first.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert_eq!(
            nurbs.call_method1("getMultiplicity", (2,)).unwrap().extract::<i32>().unwrap(),
            1
        );
        // `getResolution(0.5)` is upstream's "must not crash" probe.
        let resolution = nurbs
            .call_method1("getResolution", (0.5,))
            .expect("getResolution(0.5)")
            .extract::<f64>()
            .unwrap();
        assert!(resolution > 0.0);
    });
}

#[test]
fn a_pole_spline_reports_properties_and_mutates() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let spline = three_pole_spline(&part, &vector);

        // Upstream's `testProperties`, for the 3-pole quadratic: the degree
        // defaults to 3 but is capped at `len(poles) - 1`.
        assert_eq!(spline.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(spline.getattr("NbPoles").unwrap().extract::<i32>().unwrap(), 3);
        assert_eq!(spline.getattr("NbKnots").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(
            spline.getattr("FirstUKnotIndex").unwrap().extract::<i32>().unwrap(),
            1
        );
        assert_eq!(
            spline.getattr("LastUKnotIndex").unwrap().extract::<i32>().unwrap(),
            2
        );
        assert_eq!(
            spline.getattr("Continuity").unwrap().extract::<String>().unwrap(),
            "CN",
            "the quadratic has no inner knot"
        );
        assert_eq!(
            spline.getattr("KnotSequence").unwrap().extract::<Vec<f64>>().unwrap(),
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]
        );
        let end = spline.getattr("EndPoint").unwrap();
        assert!((end.getattr("x").unwrap().extract::<f64>().unwrap() - 2.0).abs() < 1e-9);

        // Upstream's `testSetters`: a copy is mutated independently, and the
        // periodicity conversion is a real OCCT restructure.
        let copy = py
            .import("copy")
            .expect("import copy")
            .call_method1("copy", (spline.clone(),))
            .expect("copy.copy(spline)");
        copy.call_method1("setKnot", (1, 0.1)).expect("setKnot(1, 0.1)");
        assert!(
            (copy.call_method1("getKnot", (1,)).unwrap().extract::<f64>().unwrap() - 0.1).abs()
                < 1e-12
        );
        assert_eq!(
            spline.call_method1("getKnot", (1,)).unwrap().extract::<f64>().unwrap(),
            0.0,
            "the original is untouched"
        );
        copy.call_method0("setPeriodic").expect("setPeriodic()");
        assert!(copy.call_method0("isPeriodic").unwrap().extract::<bool>().unwrap());
        copy.call_method0("setNotPeriodic").expect("setNotPeriodic()");
        assert!(!copy.call_method0("isPeriodic").unwrap().extract::<bool>().unwrap());

        // `setPole` moves a control point.
        spline
            .call_method1("setPole", (1, vector.call1((1.0, 0.0, 0.0)).unwrap()))
            .expect("setPole(1, (1,0,0))");
        let moved = spline.call_method1("getPole", (1,)).unwrap();
        assert!((moved.getattr("x").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9);
    });
}

#[test]
fn a_bad_knot_vector_is_an_occ_error() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let poles = PyList::new(
            py,
            [
                vector.call1((0.0, 0.0, 0.0)).unwrap(),
                vector.call1((1.0, 1.0, 0.0)).unwrap(),
            ],
        )
        .expect("pole list");
        let spline = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("Part.BSplineCurve()");

        // Three knots but two multiplicities: upstream's length check.
        let error = spline
            .call_method1(
                "buildFromPolesMultsKnots",
                (poles, (3, 3), (0.0, 0.5, 1.0), false, 2),
            )
            .expect_err("knot/mult length mismatch");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");
    });
}

#[test]
fn circle_and_ellipse_transform_in_place() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let matrix = ferrocad.getattr("Matrix").expect("ferrocad.Matrix");

        // Upstream's `testCircleToNurbs` matrix: three axis rotations.
        let rotation = matrix.call0().expect("Matrix()");
        rotation.call_method1("rotateX", (1.0,)).expect("rotateX(1)");
        rotation.call_method1("rotateY", (1.0,)).expect("rotateY(1)");
        rotation.call_method1("rotateZ", (1.0,)).expect("rotateZ(1)");

        let circle = part.getattr("Circle").expect("Part.Circle").call0().expect("Circle()");
        circle.setattr("Radius", 5.0).expect("Radius = 5");
        let start = circle.call_method1("toShape", ()).expect("toShape");
        assert!(!start.call_method0("isNull").unwrap().extract::<bool>().unwrap());

        circle
            .call_method1("transform", (&rotation,))
            .expect("Circle.transform(matrix)");
        assert_eq!(circle.getattr("Radius").unwrap().extract::<f64>().unwrap(), 5.0);
        let normal = circle.getattr("Axis").unwrap();
        // A rotation moves +Z off the global axis, so the circle is no longer in
        // the XY plane and its centre left the origin.
        assert!(normal.getattr("z").unwrap().extract::<f64>().unwrap().abs() < 0.999);
        let centre = circle.getattr("Center").unwrap();
        assert!(centre.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        assert!(centre.getattr("y").unwrap().extract::<f64>().unwrap().abs() < 1e-9);

        // An ellipse keeps both radii (they scale uniformly, `1.0` here).
        let ellipse = part
            .getattr("Ellipse")
            .expect("Part.Ellipse")
            .call1((vector.call1((0.0, 0.0, 0.0)).unwrap(), 5.0, 3.0))
            .expect("Ellipse(center, 5, 3)");
        ellipse
            .call_method1("transform", (&rotation,))
            .expect("Ellipse.transform(matrix)");
        assert_eq!(ellipse.getattr("MajorRadius").unwrap().extract::<f64>().unwrap(), 5.0);
        assert_eq!(ellipse.getattr("MinorRadius").unwrap().extract::<f64>().unwrap(), 3.0);
        let start = ellipse.call_method1("value", (0.0,)).unwrap();
        assert!(start.getattr("z").unwrap().extract::<f64>().unwrap().abs() > 1e-6);
    });
}

#[test]
fn a_non_similarity_transform_is_rejected() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let matrix = ferrocad.getattr("Matrix").expect("ferrocad.Matrix");

        // A shear: `Base.Matrix` can express it, `gp_Trsf` cannot.
        let shear = matrix.call0().expect("Matrix()");
        shear.setattr("A12", 0.5).expect("A12 = 0.5");
        let circle = part.getattr("Circle").expect("Part.Circle").call0().expect("Circle()");
        let error = circle
            .call_method1("transform", (&shear,))
            .expect_err("a shear cannot transform a circle");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");
    });
}

#[test]
fn ellipse_to_shape_builds_an_exact_ellipse() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");

        let ellipse = part
            .getattr("Ellipse")
            .expect("Part.Ellipse")
            .call1((vector.call1((0.0, 0.0, 0.0)).unwrap(), 100.0, 60.0))
            .expect("Ellipse(center, 100, 60)");
        let edge = ellipse.call_method0("toShape").expect("Ellipse.toShape");
        assert!(!edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        let curve = edge.getattr("Curve").expect("Edge.Curve");
        assert_eq!(curve.get_type().name().unwrap(), "Ellipse");
        assert_eq!(curve.getattr("MajorRadius").unwrap().extract::<f64>().unwrap(), 100.0);
        assert_eq!(curve.getattr("MinorRadius").unwrap().extract::<f64>().unwrap(), 60.0);

        // A wire of the one closed edge, as upstream's face-maker test builds.
        let wire = part
            .getattr("Wire")
            .expect("Part.Wire")
            .call1((&edge,))
            .expect("Part.Wire([edge])");
        assert!(wire.call_method0("isClosed").unwrap().extract::<bool>().unwrap());
    });
}
