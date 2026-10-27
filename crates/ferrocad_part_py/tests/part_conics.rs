//! The remaining analytic curves end to end: `Part.Hyperbola`,
//! `Part.Parabola` and `Part.BezierCurve`, their arcs, and the exact B-spline
//! edges `toShape` builds from them.
//!
//! The conics are checked with coordinate-free invariants (the hyperbola's
//! difference of focal distances and the parabola's focus/directrix distance),
//! because the B-spline parameter is not the conic parameter.
//!
//! Same boot as the other Part tests (`ferrocad` + `Part` as built-ins, one OCCT
//! backend); it needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

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

type Point = (f64, f64, f64);

fn point(value: &Bound<'_, PyAny>) -> Point {
    let read = |name: &str| value.getattr(name).unwrap().extract::<f64>().unwrap();
    (read("x"), read("y"), read("z"))
}

fn sub(first: Point, second: Point) -> Point {
    (first.0 - second.0, first.1 - second.1, first.2 - second.2)
}

fn dot(first: Point, second: Point) -> f64 {
    first.0 * second.0 + first.1 * second.1 + first.2 * second.2
}

fn distance(first: Point, second: Point) -> f64 {
    dot(sub(first, second), sub(first, second)).sqrt()
}

fn close(first: Point, second: Point) -> bool {
    distance(first, second) <= 1e-9
}

fn number(object: &Bound<'_, PyAny>, name: &str) -> f64 {
    let value = object
        .getattr(name)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    value
        .extract::<f64>()
        .unwrap_or_else(|error| panic!("{name} as f64: {error}"))
}

fn at(object: &Bound<'_, PyAny>, parameter: f64) -> Point {
    point(
        &object
            .call_method1("value", (parameter,))
            .expect("value(u)"),
    )
}

fn occ_error(py: Python<'_>) -> Bound<'_, pyo3::types::PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<pyo3::types::PyType>()
        .expect("OCCError is a type")
}

fn type_name(object: &Bound<'_, PyAny>) -> String {
    object.get_type().name().unwrap().to_string()
}

/// The class name of the curve an edge stores.
fn edge_curve_type(edge: &Bound<'_, PyAny>) -> String {
    type_name(&edge.getattr("Curve").expect("Edge.Curve"))
}

#[test]
fn hyperbola_constructors_and_properties() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let hyperbola = part
            .getattr("Hyperbola")
            .expect("Part.Hyperbola")
            .call0()
            .expect("Hyperbola()");

        assert_eq!(number(&hyperbola, "MajorRadius"), 2.0);
        assert_eq!(number(&hyperbola, "MinorRadius"), 1.0);
        assert!(close(point(&hyperbola.getattr("Center").unwrap()), (0.0, 0.0, 0.0)));
        assert!(close(point(&hyperbola.getattr("Axis").unwrap()), (0.0, 0.0, 1.0)));
        assert!(close(at(&hyperbola, 0.0), (2.0, 0.0, 0.0)));
        let focal = 5.0f64.sqrt();
        assert!((number(&hyperbola, "Focal") - focal).abs() < 1e-12);
        assert!(close(
            point(&hyperbola.getattr("Focus1").unwrap()),
            (focal, 0.0, 0.0)
        ));
        assert!(close(
            point(&hyperbola.getattr("Focus2").unwrap()),
            (-focal, 0.0, 0.0)
        ));

        // `(Center, MajorRadius, MinorRadius)`.
        let offset = part
            .getattr("Hyperbola")
            .unwrap()
            .call1(((1.0, 2.0, 3.0), 4.0, 5.0))
            .expect("Hyperbola(center, 4, 5)");
        assert_eq!(number(&offset, "MajorRadius"), 4.0);
        assert_eq!(number(&offset, "MinorRadius"), 5.0);
        assert!(close(point(&offset.getattr("Center").unwrap()), (1.0, 2.0, 3.0)));
        assert!(close(at(&offset, 0.0), (5.0, 2.0, 3.0)));

        // `(S1, S2, Center)`: the major axis runs along Center → S1.
        let three = part
            .getattr("Hyperbola")
            .unwrap()
            .call1(((3.0, 0.0, 0.0), (0.0, 2.0, 0.0), (0.0, 0.0, 0.0)))
            .expect("Hyperbola(S1, S2, Center)");
        assert!((number(&three, "MajorRadius") - 3.0).abs() < 1e-12);
        assert!((number(&three, "MinorRadius") - 2.0).abs() < 1e-12);
        assert!(close(point(&three.getattr("Axis").unwrap()), (0.0, 0.0, 1.0)));
        assert!(close(at(&three, 0.0), (3.0, 0.0, 0.0)));

        // The copy constructor.
        let copy = part
            .getattr("Hyperbola")
            .unwrap()
            .call1((&hyperbola,))
            .expect("Hyperbola(hyperbola)");
        assert_eq!(number(&copy, "MajorRadius"), 2.0);
        assert_eq!(number(&copy, "MinorRadius"), 1.0);

        // The writable radii.
        hyperbola.setattr("MajorRadius", 6.0).expect("MajorRadius = 6");
        hyperbola.setattr("MinorRadius", 3.0).expect("MinorRadius = 3");
        assert_eq!(number(&hyperbola, "MajorRadius"), 6.0);
        assert_eq!(number(&hyperbola, "MinorRadius"), 3.0);
        assert!(close(at(&hyperbola, 0.0), (6.0, 0.0, 0.0)));

        // A non-positive radius is rejected.
        let error = hyperbola
            .setattr("MajorRadius", 0.0)
            .expect_err("a zero major radius is degenerate");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");

        // The hyperbola invariant: |dist(F1) - dist(F2)| == 2a.
        let major = number(&hyperbola, "MajorRadius");
        let focus1 = point(&hyperbola.getattr("Focus1").unwrap());
        let focus2 = point(&hyperbola.getattr("Focus2").unwrap());
        for parameter in [-2.0, -0.5, 0.0, 0.5, 2.0] {
            let value = at(&hyperbola, parameter);
            let difference = (distance(value, focus1) - distance(value, focus2)).abs();
            assert!(
                (difference - 2.0 * major).abs() < 1e-9,
                "point at {parameter} is off the hyperbola"
            );
        }
    });
}

#[test]
fn hyperbola_to_nurbs_and_shape_are_exact() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let hyperbola = part
            .getattr("Hyperbola")
            .unwrap()
            .call0()
            .expect("Hyperbola()");

        // The base curve has no edge without a finite range.
        let error = hyperbola
            .call_method0("toShape")
            .expect_err("an unbounded hyperbola needs a range");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");

        let spline = hyperbola
            .call_method1("toNurbs", (0.0, 1.0))
            .expect("toNurbs(0, 1)");
        assert_eq!(type_name(&spline), "BSplineCurve");
        assert_eq!(spline.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(spline.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 3);
        assert!(spline.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        assert_eq!(number(&spline, "FirstParameter"), 0.0);
        assert_eq!(number(&spline, "LastParameter"), 1.0);
        assert!(close(at(&spline, 0.0), at(&hyperbola, 0.0)));
        assert!(close(at(&spline, 1.0), at(&hyperbola, 1.0)));

        let major = number(&hyperbola, "MajorRadius");
        let focus1 = point(&hyperbola.getattr("Focus1").unwrap());
        let focus2 = point(&hyperbola.getattr("Focus2").unwrap());
        for parameter in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let value = at(&spline, parameter);
            let difference = (distance(value, focus1) - distance(value, focus2)).abs();
            assert!(
                (difference - 2.0 * major).abs() < 1e-9,
                "NURBS point at {parameter} is off the hyperbola"
            );
        }

        // `trim` returns the arc, and its conversions use the arc's range.
        let arc = hyperbola
            .call_method1("trim", (0.0, 1.0))
            .expect("hyperbola.trim(0, 1)");
        assert_eq!(type_name(&arc), "ArcOfHyperbola");
        assert_eq!(number(&arc, "FirstParameter"), 0.0);
        assert_eq!(number(&arc, "LastParameter"), 1.0);
        assert!(close(
            point(&arc.getattr("StartPoint").unwrap()),
            at(&hyperbola, 0.0)
        ));
        assert!(close(
            point(&arc.getattr("EndPoint").unwrap()),
            at(&hyperbola, 1.0)
        ));
        let arc_spline = arc.call_method0("toNurbs").expect("arc.toNurbs()");
        assert_eq!(number(&arc_spline, "FirstParameter"), 0.0);
        assert_eq!(number(&arc_spline, "LastParameter"), 1.0);
        assert!(close(at(&arc_spline, 0.0), at(&hyperbola, 0.0)));
        assert!(close(at(&arc_spline, 1.0), at(&hyperbola, 1.0)));

        // `toShape` builds the B-spline edge and reports that curve.
        let edge = arc.call_method0("toShape").expect("arc.toShape()");
        assert!(!edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert_eq!(edge_curve_type(&edge), "BSplineCurve");
        let arc_length = number(&edge, "Length");
        assert!(arc_length > 0.0);

        // The same from the base curve with an explicit range.
        let edge = hyperbola
            .call_method1("toShape", (0.0, 1.0))
            .expect("hyperbola.toShape(0, 1)");
        assert_eq!(edge_curve_type(&edge), "BSplineCurve");
        assert!((number(&edge, "Length") - arc_length).abs() < 1e-9);

        // The direct `ArcOfHyperbola(hyperbola, first, last)` constructor.
        let direct = part
            .getattr("ArcOfHyperbola")
            .unwrap()
            .call1((&hyperbola, 0.0, 1.0))
            .expect("ArcOfHyperbola(hyperbola, 0, 1)");
        assert_eq!(type_name(&direct.getattr("Hyperbola").unwrap()), "Hyperbola");
        assert!(close(
            point(&direct.getattr("EndPoint").unwrap()),
            at(&hyperbola, 1.0)
        ));
    });
}

#[test]
fn parabola_constructors_and_properties() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let parabola = part
            .getattr("Parabola")
            .expect("Part.Parabola")
            .call0()
            .expect("Parabola()");

        assert_eq!(number(&parabola, "Focal"), 1.0);
        assert_eq!(number(&parabola, "Parameter"), 2.0);
        assert!(close(point(&parabola.getattr("Center").unwrap()), (0.0, 0.0, 0.0)));
        assert!(close(point(&parabola.getattr("Axis").unwrap()), (0.0, 0.0, 1.0)));
        assert!(close(point(&parabola.getattr("Focus").unwrap()), (1.0, 0.0, 0.0)));
        assert!(close(at(&parabola, 0.0), (0.0, 0.0, 0.0)));
        assert!(close(at(&parabola, 1.0), (0.25, 1.0, 0.0)));

        // `(Focus, Center, Normal)`.
        let three = part
            .getattr("Parabola")
            .unwrap()
            .call1(((2.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 1.0)))
            .expect("Parabola(focus, center, normal)");
        assert!((number(&three, "Focal") - 2.0).abs() < 1e-12);
        assert!(close(point(&three.getattr("Focus").unwrap()), (2.0, 0.0, 0.0)));
        assert!(close(at(&three, 0.0), (0.0, 0.0, 0.0)));

        let copy = part
            .getattr("Parabola")
            .unwrap()
            .call1((&parabola,))
            .expect("Parabola(parabola)");
        assert_eq!(number(&copy, "Focal"), 1.0);

        parabola.setattr("Focal", 2.0).expect("Focal = 2");
        assert_eq!(number(&parabola, "Focal"), 2.0);
        assert!(close(point(&parabola.getattr("Focus").unwrap()), (2.0, 0.0, 0.0)));
        let error = parabola
            .setattr("Focal", 0.0)
            .expect_err("a zero focal length is degenerate");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");

        // `compute(p1, p2, p3)` fits `x = y^2` through these points.
        let computed = part
            .getattr("Parabola")
            .unwrap()
            .call0()
            .expect("Parabola()");
        computed
            .call_method1(
                "compute",
                ((0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (4.0, 2.0, 0.0)),
            )
            .expect("Parabola.compute");
        assert!((number(&computed, "Focal") - 0.25).abs() < 1e-9);
        assert!(close(point(&computed.getattr("Center").unwrap()), (0.0, 0.0, 0.0)));
        assert!(close(point(&computed.getattr("Focus").unwrap()), (0.25, 0.0, 0.0)));

        // The conic invariant: distance to the focus equals distance to the
        // directrix.
        let focal = number(&computed, "Focal");
        let focus = point(&computed.getattr("Focus").unwrap());
        let vertex = point(&computed.getattr("Center").unwrap());
        let axis = point(&computed.getattr("XAxis").unwrap());
        for parameter in [-2.0, -0.5, 0.0, 0.5, 2.0] {
            let value = at(&computed, parameter);
            let to_focus = distance(value, focus);
            let to_directrix =
                (dot(sub(value, vertex), axis) + focal).abs();
            assert!(
                (to_focus - to_directrix).abs() < 1e-9,
                "point at {parameter} is off the parabola"
            );
        }
    });
}

#[test]
fn parabola_to_nurbs_and_shape_are_exact() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let parabola = part
            .getattr("Parabola")
            .unwrap()
            .call0()
            .expect("Parabola()");

        let error = parabola
            .call_method0("toShape")
            .expect_err("an unbounded parabola needs a range");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");

        let spline = parabola
            .call_method1("toNurbs", (-1.0, 2.0))
            .expect("toNurbs(-1, 2)");
        assert_eq!(type_name(&spline), "BSplineCurve");
        assert_eq!(spline.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert!(!spline.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        assert_eq!(number(&spline, "FirstParameter"), -1.0);
        assert_eq!(number(&spline, "LastParameter"), 2.0);
        assert!(close(at(&spline, -1.0), at(&parabola, -1.0)));
        assert!(close(at(&spline, 2.0), at(&parabola, 2.0)));

        let focal = number(&parabola, "Focal");
        let focus = point(&parabola.getattr("Focus").unwrap());
        let vertex = point(&parabola.getattr("Center").unwrap());
        let axis = point(&parabola.getattr("XAxis").unwrap());
        for parameter in [-1.0, -0.25, 0.5, 1.25, 2.0] {
            let value = at(&spline, parameter);
            let to_focus = distance(value, focus);
            let to_directrix = (dot(sub(value, vertex), axis) + focal).abs();
            assert!(
                (to_focus - to_directrix).abs() < 1e-9,
                "B-spline point at {parameter} is off the parabola"
            );
        }

        let arc = parabola
            .call_method1("trim", (-1.0, 2.0))
            .expect("parabola.trim(-1, 2)");
        assert_eq!(type_name(&arc), "ArcOfParabola");
        assert_eq!(number(&arc, "FirstParameter"), -1.0);
        assert_eq!(number(&arc, "LastParameter"), 2.0);
        assert!(close(
            point(&arc.getattr("StartPoint").unwrap()),
            at(&parabola, -1.0)
        ));
        let arc_spline = arc.call_method0("toNurbs").expect("arc.toNurbs()");
        assert_eq!(number(&arc_spline, "FirstParameter"), -1.0);
        assert_eq!(number(&arc_spline, "LastParameter"), 2.0);
        assert!(close(at(&arc_spline, 2.0), at(&parabola, 2.0)));

        let edge = arc.call_method0("toShape").expect("arc.toShape()");
        assert!(!edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert_eq!(edge_curve_type(&edge), "BSplineCurve");
        let arc_length = number(&edge, "Length");
        assert!(arc_length > 0.0);

        let edge = parabola
            .call_method1("toShape", (-1.0, 2.0))
            .expect("parabola.toShape(-1, 2)");
        assert!((number(&edge, "Length") - arc_length).abs() < 1e-9);

        let direct = part
            .getattr("ArcOfParabola")
            .unwrap()
            .call1((&parabola, -1.0, 2.0))
            .expect("ArcOfParabola(parabola, -1, 2)");
        assert_eq!(type_name(&direct.getattr("Parabola").unwrap()), "Parabola");
        assert!(close(
            point(&direct.getattr("EndPoint").unwrap()),
            at(&parabola, 2.0)
        ));
    });
}

#[test]
fn bezier_curve_poles_weights_and_evaluation() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        // Upstream's default Geom_BezierCurve: two poles on +Z.
        let default = part
            .getattr("BezierCurve")
            .expect("Part.BezierCurve")
            .call0()
            .expect("BezierCurve()");
        assert_eq!(default.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 2);
        assert_eq!(default.getattr("Degree").unwrap().extract::<i32>().unwrap(), 1);
        assert_eq!(default.getattr("MaxDegree").unwrap().extract::<i32>().unwrap(), 25);
        assert!(close(at(&default, 0.0), (0.0, 0.0, 0.0)));
        assert!(close(at(&default, 1.0), (0.0, 0.0, 1.0)));

        let poles: Vec<Point> = vec![
            (0.0, 0.0, 0.0),
            (1.0, 2.0, 0.0),
            (2.0, 0.0, 0.0),
        ];
        let bezier = part
            .getattr("BezierCurve")
            .unwrap()
            .call1((poles.clone(),))
            .expect("BezierCurve(poles)");
        assert_eq!(bezier.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 3);
        assert_eq!(bezier.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert!(!bezier.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        assert!(!bezier.call_method0("isPeriodic").unwrap().extract::<bool>().unwrap());
        assert!(!bezier.call_method0("isClosed").unwrap().extract::<bool>().unwrap());
        assert!(close(point(&bezier.getattr("StartPoint").unwrap()), poles[0]));
        assert!(close(point(&bezier.getattr("EndPoint").unwrap()), poles[2]));
        assert!(close(at(&bezier, 0.5), (1.0, 1.0, 0.0)));

        // `getPole`/`setPole` use OCC's one-based indices.
        assert!(close(point(&bezier.call_method1("getPole", (2,)).unwrap()), poles[1]));
        bezier
            .call_method1("setPole", (2, (1.0, 3.0, 0.0)))
            .expect("setPole(2, pole)");
        assert!(close(
            point(&bezier.call_method1("getPole", (2,)).unwrap()),
            (1.0, 3.0, 0.0)
        ));
        let error = bezier
            .call_method1("getPole", (4,))
            .expect_err("pole index out of range");
        assert!(error.is_instance(py, &occ_error(py)), "{error}");

        let listed = bezier.call_method0("getPoles").expect("getPoles");
        let listed = listed.downcast::<PyList>().expect("a list of poles");
        assert_eq!(listed.len(), 3);
        assert!(close(point(&listed.get_item(1).unwrap()), (1.0, 3.0, 0.0)));

        // `setPoles` rebuilds the curve with unit weights.
        bezier
            .call_method1("setPoles", (poles.clone(),))
            .expect("setPoles");
        assert!(close(at(&bezier, 0.5), (1.0, 1.0, 0.0)));
        assert!(bezier
            .call_method0("getWeights")
            .unwrap()
            .extract::<Vec<f64>>()
            .unwrap()
            .iter()
            .all(|weight| (*weight - 1.0).abs() < 1e-12));

        // A rational quarter circle: weights `1, 1/sqrt(2), 1`.
        let circle: Vec<Point> = vec![
            (1.0, 0.0, 0.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 0.0),
        ];
        let rational = part
            .getattr("BezierCurve")
            .unwrap()
            .call1((circle, vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0]))
            .expect("rational BezierCurve");
        assert!(rational.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        let weight = rational
            .call_method1("getWeight", (2,))
            .expect("getWeight(2)")
            .extract::<f64>()
            .expect("a weight");
        assert!((weight - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        for parameter in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let value = at(&rational, parameter);
            assert!(
                (dot(value, value).sqrt() - 1.0).abs() < 1e-9,
                "point at {parameter} is off the unit circle"
            );
        }
    });
}

#[test]
fn bezier_trim_and_exact_conversions() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let poles: Vec<Point> = vec![
            (0.0, 0.0, 0.0),
            (1.0, 2.0, 0.0),
            (2.0, 0.0, 0.0),
        ];
        let bezier = part
            .getattr("BezierCurve")
            .unwrap()
            .call1((poles,))
            .expect("BezierCurve(poles)");

        // `trim` returns another Bezier curve over the requested range.
        let part_curve = bezier
            .call_method1("trim", (0.25, 0.75))
            .expect("bezier.trim(0.25, 0.75)");
        assert_eq!(type_name(&part_curve), "BezierCurve");
        assert_eq!(number(&part_curve, "FirstParameter"), 0.25);
        assert_eq!(number(&part_curve, "LastParameter"), 0.75);
        for parameter in [0.25, 0.4, 0.5, 0.6, 0.75] {
            assert!(close(at(&part_curve, parameter), at(&bezier, parameter)));
        }

        // `toNurbs`/`toBSpline` are the same clamped B-spline.
        let spline = bezier.call_method0("toNurbs").expect("bezier.toNurbs()");
        assert_eq!(type_name(&spline), "BSplineCurve");
        assert_eq!(spline.getattr("Degree").unwrap().extract::<i32>().unwrap(), 2);
        assert_eq!(spline.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 3);
        for parameter in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(close(at(&spline, parameter), at(&bezier, parameter)));
        }
        let alternate = bezier.call_method0("toBSpline").expect("bezier.toBSpline()");
        for parameter in [0.0, 0.5, 1.0] {
            assert!(close(at(&alternate, parameter), at(&bezier, parameter)));
        }

        // The exact edge, whose `Curve` is the B-spline that was built.
        let edge = bezier.call_method0("toShape").expect("bezier.toShape()");
        assert!(!edge.call_method0("isNull").unwrap().extract::<bool>().unwrap());
        assert_eq!(edge_curve_type(&edge), "BSplineCurve");
        assert!(number(&edge, "Length") > 0.0);

        let sub_edge = bezier
            .call_method1("toShape", (0.25, 0.75))
            .expect("bezier.toShape(0.25, 0.75)");
        let start = point(&sub_edge.getattr("Curve").unwrap().call_method1("value", (0.25,)).unwrap());
        assert!(close(start, at(&bezier, 0.25)));

        // A rational quarter circle keeps its arc length through the conversion.
        let circle: Vec<Point> = vec![
            (1.0, 0.0, 0.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 0.0),
        ];
        let rational = part
            .getattr("BezierCurve")
            .unwrap()
            .call1((circle, vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0]))
            .expect("rational BezierCurve");
        let edge = rational.call_method0("toShape").expect("toShape");
        let length = number(&edge, "Length");
        assert!(
            (length - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "quarter circle length is {length}"
        );

        // `segment` trims in place.
        bezier.call_method1("segment", (0.5, 1.0)).expect("segment");
        assert_eq!(number(&bezier, "FirstParameter"), 0.5);
        assert_eq!(number(&bezier, "LastParameter"), 1.0);
        assert!(close(at(&bezier, 1.0), (2.0, 0.0, 0.0)));
    });
}
