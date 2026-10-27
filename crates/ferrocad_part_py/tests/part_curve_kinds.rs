//! `Edge.Curve` for the richer analytic kinds: elliptical/conic arcs, Bezier,
//! B-spline and offset curves.
//!
//! Every case builds a real kernel edge from the analytic curve and asks
//! `Edge.Curve` for the upstream class, with the edge's bounds read back from
//! the kernel. Same boot as the other Part tests (`ferrocad` + `Part` as
//! built-ins, one OCCT backend); it needs OCCT (see
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

fn type_name(object: &Bound<'_, PyAny>) -> String {
    object.get_type().name().unwrap().to_string()
}

fn number(object: &Bound<'_, PyAny>, name: &str) -> f64 {
    object
        .getattr(name)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .extract::<f64>()
        .unwrap_or_else(|error| panic!("{name} as f64: {error}"))
}

#[test]
fn edge_curve_reports_the_conic_arc_bases() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let edge_class = part.getattr("Edge").expect("Part.Edge");

        // Hyperbola: `Edge.Curve` is the basis `Part.Hyperbola`, with the arc's
        // span available on the edge itself.
        let hyperbola = part.getattr("Hyperbola").unwrap().call0().expect("Hyperbola()");
        let arc = hyperbola
            .call_method1("trim", (0.0, 1.0))
            .expect("hyperbola.trim(0, 1)");
        let edge = edge_class.call1((&arc,)).expect("Part.Edge(arc)");
        assert_eq!(type_name(&edge.getattr("Curve").unwrap()), "Hyperbola");
        assert!((number(&edge, "FirstParameter") - 0.0).abs() < 1e-9);
        assert!((number(&edge, "LastParameter") - 1.0).abs() < 1e-9);

        // Parabola.
        let parabola = part.getattr("Parabola").unwrap().call0().expect("Parabola()");
        let arc = parabola
            .call_method1("trim", (-1.0, 1.0))
            .expect("parabola.trim(-1, 1)");
        let edge = edge_class.call1((&arc,)).expect("Part.Edge(arc)");
        assert_eq!(type_name(&edge.getattr("Curve").unwrap()), "Parabola");
        assert!((number(&edge, "FirstParameter") - -1.0).abs() < 1e-9);
        assert!((number(&edge, "LastParameter") - 1.0).abs() < 1e-9);

        // Elliptical arc: the basis `Part.Ellipse` keeps the radii, the bounds
        // live on the edge.
        let ellipse = part.getattr("Ellipse").unwrap().call0().expect("Ellipse()");
        let arc = part
            .getattr("ArcOfEllipse")
            .unwrap()
            .call1((&ellipse, 0.0, std::f64::consts::FRAC_PI_2))
            .expect("ArcOfEllipse(ellipse, 0, pi/2)");
        let edge = edge_class.call1((&arc,)).expect("Part.Edge(arc)");
        let curve = edge.getattr("Curve").unwrap();
        assert_eq!(type_name(&curve), "Ellipse");
        assert!((number(&curve, "MajorRadius") - 1.0).abs() < 1e-9);
        assert!((number(&edge, "FirstParameter") - 0.0).abs() < 1e-9);
        assert!(
            (number(&edge, "LastParameter") - std::f64::consts::FRAC_PI_2).abs() < 1e-9
        );
    });
}

#[test]
fn edge_curve_reports_bezier_and_bspline_curves() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let edge_class = part.getattr("Edge").expect("Part.Edge");

        let poles = PyList::new(
            py,
            [
                vector.call1((0.0, 0.0, 0.0)).unwrap(),
                vector.call1((1.0, 2.0, 0.0)).unwrap(),
                vector.call1((2.0, 0.0, 0.0)).unwrap(),
            ],
        )
        .expect("pole list");

        // A Bezier edge reports a `Part.BezierCurve` with its poles.
        let bezier = part
            .getattr("BezierCurve")
            .unwrap()
            .call1((poles.clone(),))
            .expect("BezierCurve(poles)");
        let edge = edge_class.call1((&bezier,)).expect("Part.Edge(bezier)");
        let curve = edge.getattr("Curve").unwrap();
        assert_eq!(type_name(&curve), "BezierCurve");
        assert_eq!(curve.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 3);
        assert!((number(&curve, "FirstParameter") - 0.0).abs() < 1e-9);
        assert!((number(&curve, "LastParameter") - 1.0).abs() < 1e-9);

        // A B-spline edge reports a `Part.BSplineCurve` with its own data.
        let spline = part
            .getattr("BSplineCurve")
            .unwrap()
            .call0()
            .expect("BSplineCurve()");
        spline
            .call_method1("buildFromPoles", (poles,))
            .expect("buildFromPoles(poles)");
        let edge = edge_class.call1((&spline,)).expect("Part.Edge(spline)");
        let curve = edge.getattr("Curve").unwrap();
        assert_eq!(type_name(&curve), "BSplineCurve");
        assert_eq!(curve.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(curve.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 3);
    });
}

#[test]
fn edge_curve_reports_an_offset_curve() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let circle = part.getattr("Circle").unwrap().call0().expect("Circle()");
        let offset = part
            .getattr("OffsetCurve")
            .unwrap()
            .call1((&circle, 0.5, (0.0, 0.0, 1.0)))
            .expect("OffsetCurve(circle, 0.5, +Z)");
        assert_eq!(type_name(&offset.getattr("BasisCurve").unwrap()), "Circle");
        assert!((number(&offset, "Offset") - 0.5).abs() < 1e-9);

        let edge = offset.call_method0("toShape").expect("offset.toShape()");
        let curve = edge.getattr("Curve").unwrap();
        assert_eq!(type_name(&curve), "OffsetCurve");
        assert!((number(&curve, "Offset") - 0.5).abs() < 1e-9);
        assert_eq!(type_name(&curve.getattr("BasisCurve").unwrap()), "Circle");
        let first = number(&edge, "FirstParameter");
        let last = number(&edge, "LastParameter");
        assert!((last - first - std::f64::consts::TAU).abs() < 1e-9);
    });
}
