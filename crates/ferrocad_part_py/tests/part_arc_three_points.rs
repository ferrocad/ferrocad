//! `Part.Arc(p1, p2, p3)`: the arc through three points (`GC_MakeArcOfCircle`),
//! alongside the `(circle, first, last)` form `Part.Arc` has always accepted.
//!
//! Same boot as the other Part tests (`ferrocad` + `Part` as built-ins, one OCCT
//! backend); it needs OCCT (see `docs/architecture/occt-bundling.md`).

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

fn number(object: &Bound<'_, PyAny>, name: &str) -> f64 {
    object
        .getattr(name)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .extract::<f64>()
        .unwrap_or_else(|error| panic!("{name} as f64: {error}"))
}

fn close(first: Point, second: Point) -> bool {
    let d = |a: f64, b: f64| (a - b).abs();
    d(first.0, second.0) <= 1e-9 && d(first.1, second.1) <= 1e-9 && d(first.2, second.2) <= 1e-9
}

fn occ_error(py: Python<'_>) -> Bound<'_, pyo3::types::PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<pyo3::types::PyType>()
        .expect("OCCError is a type")
}

#[test]
fn arc_through_three_points() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point_object = |x: f64, y: f64, z: f64| {
            vector
                .call1((x, y, z))
                .expect("a ferrocad.Vector")
                .unbind()
        };

        // A quarter arc of the unit-radius-5 circle around the origin.
        let a = point_object(5.0, 0.0, 0.0);
        let b = point_object(4.0, 3.0, 0.0);
        let c = point_object(0.0, 5.0, 0.0);

        let arc_class = part.getattr("Arc").expect("Part.Arc");
        let arc = arc_class
            .call1((&a, &b, &c))
            .expect("Part.Arc(a, b, c)");
        assert_eq!(arc.get_type().name().unwrap(), "ArcOfCircle");
        assert!((number(&arc, "Radius") - 5.0).abs() < 1e-9);

        let center = point(&arc.getattr("Center").expect("Center"));
        assert!(close(center, (0.0, 0.0, 0.0)), "centre is the circumcentre");

        let start = point(&arc.getattr("StartPoint").expect("StartPoint"));
        let end = point(&arc.getattr("EndPoint").expect("EndPoint"));
        assert!(close(start, (5.0, 0.0, 0.0)), "starts at the first point");
        assert!(close(end, (0.0, 5.0, 0.0)), "ends at the third point");

        // The parameter range is the arc through `b`, not the complement: the
        // mid-point of the range lies on the b-side of the circle.
        let first = number(&arc, "FirstParameter");
        let last = number(&arc, "LastParameter");
        let middle = point(
            &arc.call_method1("value", ((first + last) / 2.0,))
                .expect("value(mid)"),
        );
        assert!(
            middle.0 > 0.0 && middle.1 > 0.0,
            "the arc passes through the middle point: {middle:?}"
        );
        let through = point(
            &arc.call_method1("value", ((last - first).abs() / 2.0 + first,))
                .expect("value"),
        );
        assert!(
            close(through, (middle.0, middle.1, middle.2)),
            "value is continuous over the range"
        );

        // Single-sequence form, and `toShape()` gives a two-vertex edge whose
        // basis circle is the same one (`Draft.make_arc_3_points` uses exactly
        // this path).
        let triple = PyList::new(py, [&a, &b, &c]).unwrap();
        let from_sequence = arc_class
            .call1((triple,))
            .expect("Part.Arc([a, b, c])");
        assert!(
            (number(&from_sequence, "Radius") - 5.0).abs() < 1e-9,
            "the sequence form builds the same arc"
        );

        let edge = arc.call_method0("toShape").expect("toShape");
        let curve = edge.getattr("Curve").expect("edge Curve");
        assert_eq!(curve.get_type().name().unwrap(), "Circle");
        assert!((number(&curve, "Radius") - 5.0).abs() < 1e-9);
        let vertices = edge.getattr("Vertexes").expect("Vertexes");
        assert_eq!(vertices.len().unwrap(), 2);
        let first_vertex = point(&vertices.get_item(0).unwrap().getattr("Point").unwrap());
        let last_vertex = point(&vertices.get_item(1).unwrap().getattr("Point").unwrap());
        assert!(
            close(first_vertex, (5.0, 0.0, 0.0)) && close(last_vertex, (0.0, 5.0, 0.0)),
            "the edge runs from the first point to the third"
        );

        // The same quarter arc walked the other way. The plane normal comes from the
        // ordered points, so the parameterisation is always the one through the
        // middle point; the edge reconstruction (three points) must keep that arc,
        // not its complement.
        let reversed = arc_class
            .call1((
                point_object(0.0, 5.0, 0.0),
                point_object(4.0, 3.0, 0.0),
                point_object(5.0, 0.0, 0.0),
            ))
            .expect("Part.Arc(c, b, a)");
        let reversed_first = number(&reversed, "FirstParameter");
        let reversed_last = number(&reversed, "LastParameter");
        let mid = point(
            &reversed
                .call_method1("value", ((reversed_first + reversed_last) / 2.0,))
                .expect("value(mid)"),
        );
        assert!(
            mid.0 > 0.0 && mid.1 > 0.0,
            "the reversed arc also passes through the b-side: {mid:?}"
        );
        let reversed_edge = reversed.call_method0("toShape").expect("toShape");
        assert!(
            (number(&reversed_edge, "Length") - 5.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "the edge is the short (quarter) arc, not its complement"
        );
        let start = point(
            &reversed_edge
                .getattr("Vertexes")
                .unwrap()
                .get_item(0)
                .unwrap()
                .getattr("Point")
                .unwrap(),
        );
        assert!(close(start, (0.0, 5.0, 0.0)), "starts at the first point");
    });
}

#[test]
fn make_circle_with_an_angle_range_builds_an_arc_edge() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point_object = |x: f64, y: f64, z: f64| {
            vector
                .call1((x, y, z))
                .expect("a ferrocad.Vector")
                .unbind()
        };
        let make_circle = || {
            part.getattr("makeCircle")
                .expect("Part.makeCircle")
        };
        let full_length = 2.0 * std::f64::consts::PI * 5.0;

        // The default and the equal-angles form are full circles.
        let full = make_circle()
            .call1((5.0,))
            .expect("Part.makeCircle(5)");
        assert!(
            (number(&full, "Length") - full_length).abs() < 1e-6,
            "default makeCircle is a full circle"
        );
        let same = make_circle()
            .call1((
                5.0,
                point_object(0.0, 0.0, 0.0),
                point_object(0.0, 0.0, 1.0),
                0.0,
                0.0,
            ))
            .expect("Part.makeCircle(5, c, n, 0, 0)");
        assert!(
            (number(&same, "Length") - full_length).abs() < 1e-6,
            "equal angles keep the full circle"
        );
        let whole = make_circle()
            .call1((
                5.0,
                point_object(0.0, 0.0, 0.0),
                point_object(0.0, 0.0, 1.0),
                0.0,
                360.0,
            ))
            .expect("Part.makeCircle(5, c, n, 0, 360)");
        assert!(
            (number(&whole, "Length") - full_length).abs() < 1e-6,
            "0..360 is the full circle"
        );

        // The degrees range Draft's Circle proxy uses (0..90) makes an arc edge.
        let arc = make_circle()
            .call1((
                5.0,
                point_object(0.0, 0.0, 0.0),
                point_object(0.0, 0.0, 1.0),
                0.0,
                90.0,
            ))
            .expect("Part.makeCircle(5, c, n, 0, 90)");
        assert!(
            (number(&arc, "Length") - 5.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "the angle range trims the edge"
        );
        let curve = arc.getattr("Curve").expect("Curve");
        assert_eq!(curve.get_type().name().unwrap(), "Circle");
        assert!((number(&curve, "Radius") - 5.0).abs() < 1e-9);
        let vertices = arc.getattr("Vertexes").expect("Vertexes");
        assert_eq!(vertices.len().unwrap(), 2);
        let start = point(&vertices.get_item(0).unwrap().getattr("Point").unwrap());
        let end = point(&vertices.get_item(1).unwrap().getattr("Point").unwrap());
        assert!(close(start, (5.0, 0.0, 0.0)), "starts at angle 0");
        assert!(close(end, (0.0, 5.0, 0.0)), "ends at 90 degrees");
    });
}

#[test]
fn arc_keeps_the_circle_range_form_and_rejects_degenerate_points() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point_object = |x: f64, y: f64, z: f64| {
            vector
                .call1((x, y, z))
                .expect("a ferrocad.Vector")
                .unbind()
        };

        // The original `(circle, first, last)` form.
        let circle = part
            .getattr("Circle")
            .expect("Part.Circle")
            .call1((point_object(0.0, 0.0, 0.0), point_object(0.0, 0.0, 1.0), 2.0))
            .expect("Part.Circle");
        let arc = part
            .getattr("Arc")
            .expect("Part.Arc")
            .call1((&circle, 0.0, std::f64::consts::FRAC_PI_2))
            .expect("Part.Arc(circle, 0, pi/2)");
        assert!((number(&arc, "Radius") - 2.0).abs() < 1e-9);
        let start = point(&arc.getattr("StartPoint").expect("StartPoint"));
        let end = point(&arc.getattr("EndPoint").expect("EndPoint"));
        assert!(close(start, (2.0, 0.0, 0.0)));
        assert!(close(end, (0.0, 2.0, 0.0)));

        let occ_error_type = occ_error(py);
        let arc_class = part.getattr("Arc").expect("Part.Arc");

        // Collinear points.
        let collinear = arc_class.call1((
            point_object(0.0, 0.0, 0.0),
            point_object(1.0, 0.0, 0.0),
            point_object(2.0, 0.0, 0.0),
        ));
        match collinear {
            Ok(_) => panic!("collinear points must raise Part.OCCError"),
            Err(error) => assert!(
                error.is_instance(py, &occ_error_type),
                "collinear points raise Part.OCCError: {error}"
            ),
        }

        // Coincident points.
        let coincident = arc_class.call1((
            point_object(1.0, 1.0, 1.0),
            point_object(1.0, 1.0, 1.0),
            point_object(4.0, 5.0, 0.0),
        ));
        match coincident {
            Ok(_) => panic!("coincident points must raise Part.OCCError"),
            Err(error) => assert!(
                error.is_instance(py, &occ_error_type),
                "coincident points raise Part.OCCError: {error}"
            ),
        }
    });
}
