//! `Part.Circle`/`Part.Ellipse` → NURBS, the behaviours upstream's
//! `PartTestCurveToNurbs` exercises: `toNurbs`, `toBSpline`, and `trim` followed
//! by `toNurbs` on the resulting arc.
//!
//! Upstream's `toNurbs` maps the parameter onto a rational B-spline whose
//! parameter is not the conic angle (the whole-circle branch is a cubic with
//! knots `0, π, 2π`), so the contract is the start point plus "every evaluated
//! point is on the conic": the curve is exact, not sampled.

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule};

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

fn sub(first: Point, second: Point) -> Point {
    (first.0 - second.0, first.1 - second.1, first.2 - second.2)
}

fn dot(first: Point, second: Point) -> f64 {
    first.0 * second.0 + first.1 * second.1 + first.2 * second.2
}

fn length(value: Point) -> f64 {
    dot(value, value).sqrt()
}

fn point(value: &Bound<'_, PyAny>) -> Point {
    let read = |name: &str| value.getattr(name).unwrap().extract::<f64>().unwrap();
    (read("x"), read("y"), read("z"))
}

fn at(object: &Bound<'_, PyAny>, parameter: f64) -> Point {
    point(&object.call_method1("value", (parameter,)).expect("value(u)"))
}

/// Upstream's `PartTestCurveToNurbs` matrix: three axis rotations.
fn rotation(py: Python<'_>) -> Bound<'_, PyAny> {
    let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
    let matrix = ferrocad.getattr("Matrix").expect("ferrocad.Matrix");
    let rotation = matrix.call0().expect("Matrix()");
    rotation.call_method1("rotateX", (1.0,)).expect("rotateX(1)");
    rotation.call_method1("rotateY", (1.0,)).expect("rotateY(1)");
    rotation.call_method1("rotateZ", (1.0,)).expect("rotateZ(1)");
    rotation
}

/// Assert that `curve`'s points lie on the circle `center`/`radius`.
fn assert_on_circle(curve: &Bound<'_, PyAny>, center: Point, radius: f64) {
    for parameter in [0.0, 0.3, 1.5, 3.0, 5.0] {
        let offset = sub(at(curve, parameter), center);
        assert!(
            (length(offset) - radius).abs() < 1e-9,
            "point at {parameter} is {} from the centre, not {radius}",
            length(offset)
        );
    }
}

/// Assert that `ellipse`'s points lie on the ellipse with the given frame.
fn assert_on_ellipse(
    curve: &Bound<'_, PyAny>,
    center: Point,
    normal: Point,
    major_axis: Point,
    major: f64,
    minor: f64,
) {
    let minor_axis = (
        normal.1 * major_axis.2 - normal.2 * major_axis.1,
        normal.2 * major_axis.0 - normal.0 * major_axis.2,
        normal.0 * major_axis.1 - normal.1 * major_axis.0,
    );
    for parameter in [0.0, 0.3, 1.5, 3.0, 5.0] {
        let offset = sub(at(curve, parameter), center);
        let u = dot(offset, major_axis) / major;
        let v = dot(offset, minor_axis) / minor;
        assert!(
            (u * u + v * v - 1.0).abs() < 1e-9,
            "point at {parameter} is off the ellipse"
        );
        assert!(dot(offset, normal).abs() < 1e-9, "point at {parameter} left the plane");
    }
}

#[test]
fn circle_to_nurbs_is_the_exact_conic() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let rotation = rotation(py);

        let circle = part.getattr("Circle").unwrap().call0().expect("Circle()");
        circle.setattr("Radius", 5.0).expect("Radius = 5");
        circle
            .call_method1("transform", (&rotation,))
            .expect("Circle.transform(matrix)");
        let center = point(&circle.getattr("Center").unwrap());

        // Upstream: `assertEqual(circle.value(0), nurbs.value(0))`, and the
        // whole-circle branch is the rational cubic upstream constructs.
        let nurbs = circle.call_method0("toNurbs").expect("circle.toNurbs()");
        assert!(nurbs.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        assert_eq!(nurbs.getattr("Degree").unwrap().extract::<i32>().unwrap(), 3);
        assert_eq!(nurbs.getattr("NbPoles").unwrap().extract::<usize>().unwrap(), 7);
        assert_eq!(nurbs.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(), 0.0);
        assert_eq!(
            nurbs.getattr("LastParameter").unwrap().extract::<f64>().unwrap(),
            std::f64::consts::TAU
        );
        assert!(length(sub(at(&circle, 0.0), at(&nurbs, 0.0))) < 1e-12);
        assert_on_circle(&nurbs, center, 5.0);

        // `circle.trim(0, 2).toNurbs()` keeps the basis parameterisation, so
        // parameter 0 is still `circle.value(0)`.
        let arc = circle.call_method1("trim", (0.0, 2.0)).expect("circle.trim(0, 2)");
        assert_eq!(arc.get_type().name().unwrap(), "ArcOfCircle");
        assert_eq!(arc.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(), 0.0);
        assert_eq!(arc.getattr("LastParameter").unwrap().extract::<f64>().unwrap(), 2.0);
        let trimmed = arc.call_method0("toNurbs").expect("arc.toNurbs()");
        assert!(length(sub(at(&circle, 0.0), at(&trimmed, 0.0))) < 1e-12);
        assert_on_circle(&trimmed, center, 5.0);

        // `toBSpline` is upstream's approximation; the start point is still exact.
        let spline = circle.call_method0("toBSpline").expect("circle.toBSpline()");
        assert!(length(sub(at(&circle, 0.0), at(&spline, 0.0))) < 1e-7);
    });
}

#[test]
fn ellipse_to_nurbs_is_the_exact_conic() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let rotation = rotation(py);

        let ellipse = part.getattr("Ellipse").unwrap().call0().expect("Ellipse()");
        ellipse.setattr("MajorRadius", 5.0).expect("MajorRadius = 5");
        ellipse.setattr("MinorRadius", 3.0).expect("MinorRadius = 3");
        ellipse
            .call_method1("transform", (&rotation,))
            .expect("Ellipse.transform(matrix)");
        let center = point(&ellipse.getattr("Center").unwrap());
        let normal = point(&ellipse.getattr("Axis").unwrap());
        let major_axis = point(&ellipse.getattr("MajorAxis").unwrap());

        let nurbs = ellipse.call_method0("toNurbs").expect("ellipse.toNurbs()");
        assert!(nurbs.call_method0("isRational").unwrap().extract::<bool>().unwrap());
        // Upstream's whole-ellipse conversion keeps the parameter on [0, 2].
        assert_eq!(nurbs.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(), 0.0);
        assert_eq!(nurbs.getattr("LastParameter").unwrap().extract::<f64>().unwrap(), 2.0);
        assert!(length(sub(at(&ellipse, 0.0), at(&nurbs, 0.0))) < 1e-12);
        assert_on_ellipse(&nurbs, center, normal, major_axis, 5.0, 3.0);

        let arc = ellipse.call_method1("trim", (0.0, 2.0)).expect("ellipse.trim(0, 2)");
        assert_eq!(arc.get_type().name().unwrap(), "ArcOfEllipse");
        assert_eq!(arc.getattr("MajorRadius").unwrap().extract::<f64>().unwrap(), 5.0);
        let trimmed = arc.call_method0("toNurbs").expect("arc.toNurbs()");
        assert!(length(sub(at(&ellipse, 0.0), at(&trimmed, 0.0))) < 1e-12);
        assert_on_ellipse(&trimmed, center, normal, major_axis, 5.0, 3.0);

        let spline = ellipse.call_method0("toBSpline").expect("ellipse.toBSpline()");
        assert!(length(sub(at(&ellipse, 0.0), at(&spline, 0.0))) < 1e-7);
    });
}

#[test]
fn an_arc_to_nurbs_honours_an_explicit_range() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");

        let circle = part.getattr("Circle").unwrap().call0().expect("Circle()");
        circle.setattr("Radius", 2.0).expect("Radius = 2");
        let center = point(&circle.getattr("Center").unwrap());
        let arc = circle.call_method1("trim", (0.5, 2.5)).expect("circle.trim(0.5, 2.5)");

        // The range defaults to the arc's own parameters.
        let nurbs = arc.call_method0("toNurbs").expect("arc.toNurbs()");
        assert_eq!(nurbs.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(), 0.5);
        assert_eq!(nurbs.getattr("LastParameter").unwrap().extract::<f64>().unwrap(), 2.5);
        assert!(length(sub(at(&circle, 0.5), at(&nurbs, 0.5))) < 1e-9);
        assert_on_circle(&nurbs, center, 2.0);

        // An explicit range is honoured over the same support.
        let sub_range = arc
            .call_method1("toNurbs", (1.0, 2.0))
            .expect("arc.toNurbs(1, 2)");
        assert_eq!(sub_range.getattr("FirstParameter").unwrap().extract::<f64>().unwrap(), 1.0);
        assert_eq!(sub_range.getattr("LastParameter").unwrap().extract::<f64>().unwrap(), 2.0);
        assert!(length(sub(at(&circle, 1.0), at(&sub_range, 1.0))) < 1e-9);
        assert_on_circle(&sub_range, center, 2.0);

        // The whole circle only takes upstream's seven-pole branch for [0, 2π];
        // an explicit partial range uses the trimmed conversion.
        let partial = circle
            .call_method1("toNurbs", (0.0, 1.0))
            .expect("circle.toNurbs(0, 1)");
        assert!(length(sub(at(&circle, 1.0), at(&partial, 1.0))) < 1e-9);
        assert_on_circle(&partial, center, 2.0);
    });
}
