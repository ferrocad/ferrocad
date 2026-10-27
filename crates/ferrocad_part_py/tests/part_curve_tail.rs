//! The tail of the Part curve surface: the exact helix (`makeHelix` /
//! `makeLongHelix`) and the full `Part.Geom2d` class family.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::f64::consts::{PI, TAU};

use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple, PyType};

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

fn vector2<'py>(py: Python<'py>, x: f64, y: f64) -> Bound<'py, PyAny> {
    PyModule::import(py, "ferrocad")
        .expect("import ferrocad")
        .getattr("Vector2d")
        .expect("ferrocad.Vector2d")
        .call1((x, y))
        .expect("a Vector2d")
}

fn number(value: &Bound<'_, PyAny>, what: &str) -> f64 {
    value
        .extract::<f64>()
        .unwrap_or_else(|_| panic!("{what} as f64"))
}

fn pair(value: &Bound<'_, PyAny>) -> (f64, f64) {
    (
        number(&value.getattr("x").expect("Vector2d.x"), "x"),
        number(&value.getattr("y").expect("Vector2d.y"), "y"),
    )
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what} was {actual}, expected {expected}"
    );
}

fn length(shape: &Bound<'_, PyAny>) -> f64 {
    number(&shape.getattr("Length").expect("Shape.Length"), "Length")
}

fn shape_type(shape: &Bound<'_, PyAny>) -> String {
    shape
        .getattr("ShapeType")
        .expect("Shape.ShapeType")
        .extract::<String>()
        .expect("ShapeType as str")
}

fn is_valid(shape: &Bound<'_, PyAny>) -> bool {
    shape
        .call_method0("isValid")
        .expect("Shape.isValid")
        .extract::<bool>()
        .expect("isValid as bool")
}

fn isinstance(py: Python<'_>, value: &Bound<'_, PyAny>, class: &Bound<'_, PyAny>) -> bool {
    py.import("builtins")
        .expect("import builtins")
        .getattr("isinstance")
        .expect("builtins.isinstance")
        .call1((value, class))
        .expect("isinstance")
        .extract::<bool>()
        .expect("isinstance as bool")
}

fn edge_points(edge: &Bound<'_, PyAny>) -> Vec<(f64, f64, f64)> {
    let vertexes = edge.getattr("Vertexes").expect("Edge.Vertexes");
    let mut points = Vec::new();
    for index in 0..vertexes.len().expect("vertex count") {
        let vertex = vertexes.get_item(index).expect("a vertex");
        points.push((
            number(&vertex.getattr("X").expect("Vertex.X"), "X"),
            number(&vertex.getattr("Y").expect("Vertex.Y"), "Y"),
            number(&vertex.getattr("Z").expect("Vertex.Z"), "Z"),
        ));
    }
    points
}

fn points_meet(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-7 && (a.1 - b.1).abs() < 1e-7 && (a.2 - b.2).abs() < 1e-7
}

fn bounds(shape: &Bound<'_, PyAny>) -> [f64; 6] {
    let bound_box = shape.getattr("BoundBox").expect("Shape.BoundBox");
    let value = |name: &str| {
        number(
            &bound_box
                .getattr(name)
                .unwrap_or_else(|_| panic!("BoundBox.{name}")),
            name,
        )
    };
    [
        value("XMin"),
        value("YMin"),
        value("ZMin"),
        value("XMax"),
        value("YMax"),
        value("ZMax"),
    ]
}

fn occ_error_type(py: Python<'_>) -> Bound<'_, PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<PyType>()
        .expect("OCCError is a type")
}

#[test]
fn make_helix_is_the_exact_analytic_edge() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let turn = ((TAU * 2.0).powi(2) + 1.0).sqrt();

        // The cylindrical helix starts at (radius, 0, 0) and rises by `height`.
        let helix = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0))
            .expect("Part.makeHelix(1, 4, 2)");
        assert_eq!(shape_type(&helix), "Edge");
        assert!(is_valid(&helix));
        assert!(
            (length(&helix) - 4.0 * turn).abs() < 1e-4,
            "cylinder helix length was {}, expected {}",
            length(&helix),
            4.0 * turn
        );
        let points = edge_points(&helix);
        assert_eq!(points.len(), 2);
        assert!(
            points.iter().any(|point| points_meet(*point, (2.0, 0.0, 0.0))),
            "one end is (2, 0, 0): {points:?}"
        );
        assert!(
            points.iter().any(|point| points_meet(*point, (2.0, 0.0, 4.0))),
            "the other end is (2, 0, 4): {points:?}"
        );

        // Handedness: a quarter turn reaches +Y one way and -Y the other.
        let right = part
            .call_method1("makeHelix", (1.0, 0.25, 2.0))
            .expect("right-handed quarter helix");
        let left = part
            .call_method1("makeHelix", (1.0, 0.25, 2.0, -1.0, true))
            .expect("left-handed quarter helix");
        let right_bounds = bounds(&right);
        let left_bounds = bounds(&left);
        assert_close(right_bounds[4], 2.0, "right helix ymax");
        assert_close(left_bounds[1], -2.0, "left helix ymin");
        assert!(
            !right
                .call_method1("isSame", (&left,))
                .expect("Shape.isSame")
                .extract::<bool>()
                .expect("isSame as bool"),
            "the two helices are different geometry"
        );

        // Old-style conical helix: the segment ends at `v = height`, so the
        // axial rise is `height * cos(angle)` and the radius grows by
        // `height * sin(angle)` (OCCT's cone `v` is along the generator).
        let old_style = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0, 30.0))
            .expect("old-style conical helix");
        let old_bounds = bounds(&old_style);
        assert_close(
            old_bounds[5],
            4.0 * 30.0_f64.to_radians().cos(),
            "old-style axial height",
        );
        assert_close(
            old_bounds[3],
            2.0 + 4.0 * 30.0_f64.to_radians().sin(),
            "old-style end radius",
        );

        // New style (`pvertHeight`): the segment ends at `height / cos(angle)`,
        // so the axial rise is exactly `height`.
        let new_style = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0, 30.0, false, true))
            .expect("new-style conical helix");
        let new_bounds = bounds(&new_style);
        assert_close(new_bounds[5], 4.0, "new-style axial height");
        assert_close(
            new_bounds[3],
            2.0 + 4.0 * 30.0_f64.to_radians().tan(),
            "new-style end radius",
        );

        // Upstream's errors, verbatim.
        let error = part
            .call_method1("makeHelix", (0.0, 4.0, 2.0))
            .expect_err("a zero pitch is rejected");
        assert!(error.to_string().contains("Pitch of helix too small"), "{error}");
        let error = part
            .call_method1("makeHelix", (1.0, 0.0, 2.0))
            .expect_err("a zero height is rejected");
        assert!(error.to_string().contains("Height of helix too small"), "{error}");
        let error = part
            .call_method1("makeHelix", (1.0, 4.0, 0.0))
            .expect_err("a zero radius is rejected");
        assert!(error.to_string().contains("Radius of helix too small"), "{error}");
        let error = part
            .call_method1("makeHelix", (1.0, -4.0, 2.0))
            .expect_err("a sign mismatch is rejected");
        assert!(
            error
                .to_string()
                .contains("Pitch and height of helix not compatible"),
            "{error}"
        );
        let error = part
            .call_method1("makeHelix", (1.0, 4.0, 2.0, 1.0e-6))
            .expect_err("a sub-confusion angle is rejected");
        assert!(error.to_string().contains("Angle of helix too small"), "{error}");
    });
}

#[test]
fn make_long_helix_splits_at_the_whole_turns() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let turn = ((TAU * 2.0).powi(2) + 1.0).sqrt();

        let wire = part
            .call_method1("makeLongHelix", (1.0, 4.0, 2.0))
            .expect("Part.makeLongHelix(1, 4, 2)");
        assert_eq!(shape_type(&wire), "Wire");
        assert!(is_valid(&wire));
        assert!(
            (length(&wire) - 4.0 * turn).abs() < 1e-4,
            "long helix length was {}, expected {}",
            length(&wire),
            4.0 * turn
        );

        let edges = wire.getattr("Edges").expect("Wire.Edges");
        assert_eq!(edges.len().expect("edge count"), 4, "one edge per whole turn");
        let mut pieces = Vec::new();
        for index in 0..edges.len().unwrap() {
            let edge = edges.get_item(index).expect("an edge");
            assert!(is_valid(&edge));
            assert!(
                (length(&edge) - turn).abs() < 1e-3,
                "each piece was {}, expected one whole turn {turn}",
                length(&edge)
            );
            pieces.push(edge);
        }
        // The pieces chain, and their boundaries are the whole turns.
        for pair in pieces.windows(2) {
            assert!(
                edge_points(&pair[0])
                    .iter()
                    .any(|a| edge_points(&pair[1]).iter().any(|b| points_meet(*a, *b))),
                "consecutive pieces share an endpoint"
            );
        }
        let mut heights: Vec<f64> = pieces
            .iter()
            .flat_map(|edge| edge_points(edge).into_iter().map(|point| point.2))
            .collect();
        heights.sort_by(|a, b| a.total_cmp(b));
        heights.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        for (index, height) in heights.iter().enumerate() {
            assert_close(*height, index as f64, "turn boundary height");
        }

        // A part turn adds a fifth, shorter piece.
        let part_wire = part
            .call_method1("makeLongHelix", (1.0, 4.5, 2.0))
            .expect("Part.makeLongHelix(1, 4.5, 2)");
        assert_eq!(
            part_wire
                .getattr("Edges")
                .expect("Wire.Edges")
                .len()
                .expect("edge count"),
            5,
            "four whole turns plus the part turn"
        );
        assert!(
            (length(&part_wire) - 4.5 * turn).abs() < 1e-4,
            "long helix with a part turn was {}, expected {}",
            length(&part_wire),
            4.5 * turn
        );

        // The same split works for a left-handed helix.
        let left = part
            .call_method1("makeLongHelix", (1.0, 4.0, 2.0, -1.0, true))
            .expect("left-handed long helix");
        assert_eq!(
            left.getattr("Edges")
                .expect("Wire.Edges")
                .len()
                .expect("edge count"),
            4,
            "one edge per whole turn, left-handed"
        );
        assert!(
            !wire
                .call_method1("isSame", (&left,))
                .expect("Shape.isSame")
                .extract::<bool>()
                .expect("isSame as bool"),
            "the two long helices are different geometry"
        );

        // Less than one turn: the wire is the single exact edge.
        let short = part
            .call_method1("makeLongHelix", (1.0, 0.5, 2.0))
            .expect("Part.makeLongHelix(1, 0.5, 2)");
        assert_eq!(
            short
                .getattr("Edges")
                .expect("Wire.Edges")
                .len()
                .expect("edge count"),
            1,
            "a sub-turn helix is not split"
        );
        assert!(
            (length(&short) - 0.5 * turn).abs() < 1e-4,
            "sub-turn long helix length was {}, expected {}",
            length(&short),
            0.5 * turn
        );

        // A conical long helix advances the cone's generator by one pitch per
        // turn (upstream's `v`), so its axial rise is `height * cos(angle)` and
        // its radius grows by `height * sin(angle)`.
        let conical = part
            .call_method1("makeLongHelix", (1.0, 4.0, 2.0, 10.0))
            .expect("Part.makeLongHelix(1, 4, 2, 10)");
        assert_eq!(
            conical
                .getattr("Edges")
                .expect("Wire.Edges")
                .len()
                .expect("edge count"),
            4,
            "one edge per whole turn on the cone"
        );
        let conical_bounds = bounds(&conical);
        assert_close(
            conical_bounds[5],
            4.0 * 10.0_f64.to_radians().cos(),
            "conical long helix axial height",
        );
        assert_close(
            conical_bounds[3],
            2.0 + 4.0 * 10.0_f64.to_radians().sin(),
            "conical long helix end radius",
        );

        // Upstream's errors.
        let error = part
            .call_method1("makeLongHelix", (0.0, 4.0, 2.0))
            .expect_err("a zero pitch is rejected");
        assert!(error.to_string().contains("Pitch of helix too small"), "{error}");
        let error = part
            .call_method1("makeLongHelix", (1.0, 0.0, 2.0))
            .expect_err("a zero height is rejected");
        assert!(error.to_string().contains("Height of helix too small"), "{error}");
        let error = part
            .call_method1("makeLongHelix", (1.0, 4.0, 0.0))
            .expect_err("a zero radius is rejected");
        assert!(error.to_string().contains("Radius of helix too small"), "{error}");
    });
}

/// The whole `Part.Geom2d` class set of `contracts/part-api.json`.
#[test]
fn geom2d_registers_the_whole_class_set() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        for name in [
            "Geometry2d",
            "Curve2d",
            "Conic2d",
            "Line2d",
            "Line2dSegment",
            "ArcOfConic2d",
            "Circle2d",
            "ArcOfCircle2d",
            "Ellipse2d",
            "ArcOfEllipse2d",
            "Hyperbola2d",
            "ArcOfHyperbola2d",
            "Parabola2d",
            "ArcOfParabola2d",
            "BezierCurve2d",
            "BSplineCurve2d",
            "OffsetCurve2d",
        ] {
            assert!(
                geom2d.getattr(name).is_ok(),
                "Part.Geom2d.{name} is registered"
            );
        }
    });
}

#[test]
fn geom2d_line_segment_and_circle_families() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        let class = |name: &str| geom2d.getattr(name).unwrap_or_else(|_| panic!("Geom2d.{name}"));
        let occ = occ_error_type(py);

        // --- Line2d -------------------------------------------------------
        let line = class("Line2d").call0().expect("Line2d()");
        assert_close(
            number(&line.getattr("FirstParameter").unwrap(), "FirstParameter"),
            -1e100,
            "line first parameter",
        );
        assert_close(
            number(&line.getattr("LastParameter").unwrap(), "LastParameter"),
            1e100,
            "line last parameter",
        );
        assert_eq!(pair(&line.call_method1("value", (2.0,)).unwrap()), (2.0, 0.0));
        assert_eq!(pair(&line.getattr("Location").unwrap()), (0.0, 0.0));
        assert_eq!(pair(&line.getattr("Direction").unwrap()), (1.0, 0.0));
        let trimmed = line
            .call_method1("toShape", (0.0, 3.0))
            .expect("Line2d.toShape(0, 3)");
        assert_close(length(&trimmed), 3.0, "trimmed line length");
        let error = line.call_method0("toShape").expect_err("an infinite line has no edge");
        assert!(error.is_instance(py, &occ), "{error}");

        // --- Line2dSegment ------------------------------------------------
        let segment = class("Line2dSegment")
            .call1(((0.0, 0.0), (3.0, 4.0)))
            .expect("Line2dSegment((0,0), (3,4))");
        assert_close(
            number(&segment.getattr("FirstParameter").unwrap(), "FirstParameter"),
            0.0,
            "segment first parameter",
        );
        assert_close(
            number(&segment.getattr("LastParameter").unwrap(), "LastParameter"),
            5.0,
            "segment last parameter",
        );
        assert_eq!(
            pair(&segment.getattr("StartPoint").unwrap()),
            (0.0, 0.0)
        );
        assert_eq!(pair(&segment.getattr("EndPoint").unwrap()), (3.0, 4.0));
        assert_eq!(
            pair(&segment.call_method1("value", (2.5,)).unwrap()),
            (1.5, 2.0)
        );
        let edge = segment.call_method0("toShape").expect("segment.toShape()");
        assert!(is_valid(&edge));
        assert_close(length(&edge), 5.0, "segment edge length");
        let pcurve = edge
            .call_method1("curveOnSurface", (0,))
            .expect("curveOnSurface(0)");
        assert!(!pcurve.is_none(), "the edge carries its pcurve");
        assert_eq!(pcurve.get_type().name().unwrap(), "Line2dSegment");
        assert_eq!(pair(&pcurve.getattr("StartPoint").unwrap()), (0.0, 0.0));
        segment
            .call_method1("setParameterRange", (1.0, 3.0))
            .expect("setParameterRange");
        assert_eq!(
            pair(&segment.getattr("StartPoint").unwrap()),
            (0.6, 0.8)
        );

        // --- Circle2d (now a Conic2d/Curve2d/Geometry2d) -------------------
        let circle = class("Circle2d")
            .call1(((1.0, 1.0), 2.0))
            .expect("Circle2d((1,1), 2)");
        assert!(isinstance(py, &circle, &class("Conic2d")));
        assert!(isinstance(py, &circle, &class("Curve2d")));
        assert!(isinstance(py, &circle, &class("Geometry2d")));
        assert_close(
            number(&circle.getattr("Radius").unwrap(), "Radius"),
            2.0,
            "circle radius",
        );
        assert_eq!(pair(&circle.getattr("Location").unwrap()), (1.0, 1.0));
        assert_eq!(
            pair(&circle.call_method1("value", (0.0,)).unwrap()),
            (3.0, 1.0)
        );
        let quarter = pair(&circle.call_method1("value", (PI / 2.0,)).unwrap());
        assert_close(quarter.0, 1.0, "circle value(pi/2).x");
        assert_close(quarter.1, 3.0, "circle value(pi/2).y");
        assert!(
            circle.getattr("Closed").unwrap().extract::<bool>().unwrap(),
            "a circle is closed"
        );
        assert!(
            circle.getattr("Periodic").unwrap().extract::<bool>().unwrap(),
            "a circle is periodic"
        );
        assert_close(
            number(&circle.getattr("Eccentricity").unwrap(), "Eccentricity"),
            0.0,
            "circle eccentricity",
        );
        assert_close(
            number(&circle.call_method0("length").unwrap(), "length"),
            TAU * 2.0,
            "circle length",
        );
        let circle_edge = circle.call_method0("toShape").expect("circle.toShape()");
        assert!(is_valid(&circle_edge));
        assert_close(length(&circle_edge), TAU * 2.0, "circle edge length");
        let copied = circle.call_method0("copy").expect("copy()");
        assert_eq!(copied.get_type().name().unwrap(), "Circle2d");
        assert_eq!(
            pair(&copied.call_method1("value", (0.0,)).unwrap()),
            (3.0, 1.0)
        );

        // --- ArcOfCircle2d ------------------------------------------------
        let arc = class("ArcOfCircle2d")
            .call1((&circle, 0.0, PI / 2.0))
            .expect("ArcOfCircle2d(circle, 0, pi/2)");
        assert_eq!(pair(&arc.getattr("StartPoint").unwrap()), (3.0, 1.0));
        let end = pair(&arc.getattr("EndPoint").unwrap());
        assert_close(end.0, 1.0, "arc end x");
        assert_close(end.1, 3.0, "arc end y");
        assert_close(
            number(&arc.getattr("Radius").unwrap(), "Radius"),
            2.0,
            "arc radius",
        );
        assert_close(
            number(&arc.call_method0("length").unwrap(), "length"),
            PI,
            "quarter-circle arc length",
        );
        let arc_edge = arc.call_method0("toShape").expect("arc.toShape()");
        assert!(is_valid(&arc_edge));
        assert_close(length(&arc_edge), PI, "arc edge length");

        // Three points through the arc.
        let three = class("ArcOfCircle2d")
            .call1(((1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)))
            .expect("ArcOfCircle2d through three points");
        let three_start = pair(&three.getattr("StartPoint").unwrap());
        assert_close(three_start.0, 1.0, "three-point arc start x");
        assert_close(three_start.1, 0.0, "three-point arc start y");
        let three_end = pair(&three.getattr("EndPoint").unwrap());
        assert_close(three_end.0, -1.0, "three-point arc end x");
        assert_close(three_end.1, 0.0, "three-point arc end y");

        // Upstream's `getCircleCenter` is still there, with its ValueError.
        let center = class("Circle2d")
            .call_method1(
                "getCircleCenter",
                ((0.0, 0.0), (2.0, 0.0), (1.0, 1.0)),
            )
            .expect("getCircleCenter");
        let center = pair(&center);
        assert_close(center.0, 1.0, "circle centre x");
        assert_close(center.1, 0.0, "circle centre y");
        let error = class("Circle2d")
            .call_method1("getCircleCenter", ((0.0, 0.0), (1.0, 1.0), (2.0, 2.0)))
            .expect_err("collinear points");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn geom2d_conics_and_offset_families() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        let class = |name: &str| geom2d.getattr(name).unwrap_or_else(|_| panic!("Geom2d.{name}"));
        let occ = occ_error_type(py);

        // --- Ellipse2d: `(Center, Major, Minor)` puts the major axis on +Y ---
        let ellipse = class("Ellipse2d")
            .call1(((0.0, 0.0), 4.0, 2.0))
            .expect("Ellipse2d((0,0), 4, 2)");
        assert!(isinstance(py, &ellipse, &class("Conic2d")));
        assert_close(
            number(&ellipse.getattr("MajorRadius").unwrap(), "MajorRadius"),
            4.0,
            "ellipse major radius",
        );
        assert_close(
            number(&ellipse.getattr("MinorRadius").unwrap(), "MinorRadius"),
            2.0,
            "ellipse minor radius",
        );
        let (x_axis, y_axis) = (
            pair(&ellipse.getattr("XAxis").unwrap()),
            pair(&ellipse.getattr("YAxis").unwrap()),
        );
        assert_close(x_axis.0, 0.0, "ellipse XAxis.x");
        assert_close(x_axis.1, 1.0, "ellipse XAxis.y");
        assert_close(y_axis.0, -1.0, "ellipse YAxis.x");
        assert_close(y_axis.1, 0.0, "ellipse YAxis.y");
        let at_zero = pair(&ellipse.call_method1("value", (0.0,)).unwrap());
        assert_close(at_zero.0, 0.0, "ellipse value(0).x");
        assert_close(at_zero.1, 4.0, "ellipse value(0).y");
        assert_close(
            number(&ellipse.getattr("Focal").unwrap(), "Focal"),
            (16.0f64 - 4.0).sqrt(),
            "ellipse focal distance",
        );
        assert_close(
            number(&ellipse.getattr("Eccentricity").unwrap(), "Eccentricity"),
            (1.0 - 4.0 / 16.0f64).sqrt(),
            "ellipse eccentricity",
        );
        assert_close(
            number(&ellipse.call_method0("length").unwrap(), "length"),
            19.376_896_441_094_08,
            "ellipse circumference",
        );
        let ellipse_edge = ellipse.call_method0("toShape").expect("ellipse.toShape()");
        assert!(is_valid(&ellipse_edge));
        // `Shape.Length` goes through the kernel's linear properties, which
        // approximate a long ellipse; the curve's own `length()` above is the
        // exact perimeter.
        assert!(
            (length(&ellipse_edge) - 19.376_896_441_094_08).abs() < 0.05,
            "ellipse edge length was {}",
            length(&ellipse_edge)
        );

        // The default ellipse is major 2 / minor 1 on +X.
        let default_ellipse = class("Ellipse2d").call0().expect("Ellipse2d()");
        assert_close(
            number(&default_ellipse.getattr("MajorRadius").unwrap(), "MajorRadius"),
            2.0,
            "default ellipse major radius",
        );
        assert_close(
            pair(&default_ellipse.getattr("XAxis").unwrap()).0,
            1.0,
            "default ellipse XAxis.x",
        );
        // A major radius below the minor one is rejected, as `gp_Elips2d`.
        let error = class("Ellipse2d")
            .call1(((0.0, 0.0), 1.0, 2.0))
            .expect_err("major < minor is rejected");
        assert!(error.is_instance(py, &occ), "{error}");

        // --- ArcOfEllipse2d ------------------------------------------------
        let arc = class("ArcOfEllipse2d")
            .call1((&ellipse, 0.0, PI / 2.0))
            .expect("ArcOfEllipse2d(ellipse, 0, pi/2)");
        let start = pair(&arc.getattr("StartPoint").unwrap());
        assert_close(start.0, 0.0, "ellipse arc start x");
        assert_close(start.1, 4.0, "ellipse arc start y");
        let end = pair(&arc.getattr("EndPoint").unwrap());
        assert_close(end.0, -2.0, "ellipse arc end x");
        assert_close(end.1, 0.0, "ellipse arc end y");
        let arc_edge = arc.call_method0("toShape").expect("arc.toShape()");
        assert!(is_valid(&arc_edge));
        let basis = arc.getattr("Ellipse").expect("ArcOfEllipse2d.Ellipse");
        assert_eq!(basis.get_type().name().unwrap(), "Ellipse2d");
        assert_close(
            number(&basis.getattr("MajorRadius").unwrap(), "MajorRadius"),
            4.0,
            "arc basis major radius",
        );

        // --- Hyperbola2d ---------------------------------------------------
        let hyperbola = class("Hyperbola2d").call0().expect("Hyperbola2d()");
        assert_eq!(
            pair(&hyperbola.call_method1("value", (0.0,)).unwrap()),
            (2.0, 0.0)
        );
        assert_close(
            number(&hyperbola.getattr("Focal").unwrap(), "Focal"),
            5.0f64.sqrt(),
            "hyperbola focal distance",
        );
        assert_close(
            number(&hyperbola.getattr("Eccentricity").unwrap(), "Eccentricity"),
            5.0f64.sqrt() / 2.0,
            "hyperbola eccentricity",
        );
        let error = hyperbola
            .call_method0("toShape")
            .expect_err("a hyperbola is unbounded");
        assert!(error.is_instance(py, &occ), "{error}");
        let hyperbola_edge = hyperbola
            .call_method1("toShape", (0.0, 1.0))
            .expect("hyperbola.toShape(0, 1)");
        assert!(is_valid(&hyperbola_edge));
        assert!(
            (length(&hyperbola_edge) - 1.653_946_232_263_27).abs() < 1e-4,
            "hyperbola edge length was {}",
            length(&hyperbola_edge)
        );

        let arc = class("ArcOfHyperbola2d")
            .call1((&hyperbola, 0.0, 1.0))
            .expect("ArcOfHyperbola2d(hyperbola, 0, 1)");
        let arc_end = pair(&arc.getattr("EndPoint").unwrap());
        assert_close(arc_end.0, 2.0 * 1.0f64.cosh(), "hyperbola arc end x");
        assert_close(arc_end.1, 1.0f64.sinh(), "hyperbola arc end y");
        assert!(is_valid(&arc.call_method0("toShape").expect("arc.toShape()")));

        // --- Parabola2d ----------------------------------------------------
        let parabola = class("Parabola2d").call0().expect("Parabola2d()");
        assert_close(
            number(&parabola.getattr("Focal").unwrap(), "Focal"),
            1.0,
            "parabola focal length",
        );
        assert_close(
            number(&parabola.getattr("Parameter").unwrap(), "Parameter"),
            2.0,
            "parabola parameter",
        );
        assert_close(
            number(&parabola.getattr("Eccentricity").unwrap(), "Eccentricity"),
            1.0,
            "parabola eccentricity",
        );
        let at_two = pair(&parabola.call_method1("value", (2.0,)).unwrap());
        assert_close(at_two.0, 1.0, "parabola value(2).x");
        assert_close(at_two.1, 2.0, "parabola value(2).y");
        let error = parabola
            .call_method0("toShape")
            .expect_err("a parabola is unbounded");
        assert!(error.is_instance(py, &occ), "{error}");
        let parabola_edge = parabola
            .call_method1("toShape", (-1.0, 1.0))
            .expect("parabola.toShape(-1, 1)");
        assert!(is_valid(&parabola_edge));
        assert!(
            (length(&parabola_edge) - 2.080_457_638_869_05).abs() < 1e-4,
            "parabola edge length was {}",
            length(&parabola_edge)
        );

        let arc = class("ArcOfParabola2d")
            .call1((&parabola, -1.0, 1.0))
            .expect("ArcOfParabola2d(parabola, -1, 1)");
        let start = pair(&arc.getattr("StartPoint").unwrap());
        assert_close(start.0, 0.25, "parabola arc start x");
        assert_close(start.1, -1.0, "parabola arc start y");
        let end = pair(&arc.getattr("EndPoint").unwrap());
        assert_close(end.0, 0.25, "parabola arc end x");
        assert_close(end.1, 1.0, "parabola arc end y");
        assert!(is_valid(&arc.call_method0("toShape").expect("arc.toShape()")));
    });
}

#[test]
fn geom2d_offset_bezier_and_abstract_classes() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let geom2d = part.getattr("Geom2d").expect("Part.Geom2d");
        let class = |name: &str| geom2d.getattr(name).unwrap_or_else(|_| panic!("Geom2d.{name}"));

        // --- OffsetCurve2d: an exact offset evaluation ---------------------
        let line = class("Line2d")
            .call1(((0.0, 0.0), (1.0, 0.0)))
            .expect("Line2d through (0,0) along +X");
        let offset = class("OffsetCurve2d")
            .call1((&line, 1.0))
            .expect("OffsetCurve2d(line, 1)");
        assert_close(
            number(&offset.getattr("OffsetValue").unwrap(), "OffsetValue"),
            1.0,
            "offset value",
        );
        assert_eq!(
            pair(&offset.call_method1("value", (2.0,)).unwrap()),
            (2.0, 1.0),
            "offset of the X axis line is one unit to the left"
        );
        let basis = offset.getattr("BasisCurve").expect("BasisCurve");
        assert_eq!(basis.get_type().name().unwrap(), "Line2d");
        let error = offset
            .call_method0("toShape")
            .expect_err("no exact offset edge is bound");
        assert!(error.is_instance_of::<PyNotImplementedError>(py), "{error}");

        // --- BezierCurve2d --------------------------------------------------
        let bezier = class("BezierCurve2d").call0().expect("BezierCurve2d()");
        assert_eq!(bezier.get_type().name().unwrap(), "BezierCurve2d");
        assert_close(
            number(&bezier.getattr("Degree").unwrap(), "Degree"),
            1.0,
            "default Bezier degree",
        );
        assert_eq!(
            pair(&bezier.getattr("StartPoint").unwrap()),
            (0.0, 0.0)
        );
        assert_eq!(pair(&bezier.getattr("EndPoint").unwrap()), (0.0, 1.0));
        let midpoint = pair(&bezier.call_method1("value", (0.5,)).unwrap());
        assert_close(midpoint.0, 0.0, "Bezier midpoint x");
        assert_close(midpoint.1, 0.5, "Bezier midpoint y");
        assert_close(
            length(&bezier.call_method0("toShape").expect("Bezier.toShape()")),
            1.0,
            "default Bezier length",
        );
        let poles = PyTuple::new(
            py,
            [
                vector2(py, 0.0, 0.0),
                vector2(py, 1.0, 0.0),
                vector2(py, 1.0, 1.0),
            ],
        )
        .unwrap();
        bezier
            .call_method1("setPoles", (poles,))
            .expect("setPoles");
        assert_close(
            number(&bezier.getattr("Degree").unwrap(), "Degree"),
            2.0,
            "Bezier degree after setPoles",
        );
        assert_eq!(
            bezier
                .call_method1("getPoles", ())
                .expect("getPoles")
                .len()
                .unwrap(),
            3
        );
        assert_eq!(
            pair(&bezier.call_method1("getPole", (2,)).unwrap()),
            (1.0, 0.0)
        );
        assert!(
            !bezier
                .call_method0("isRational")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        // `segment(u1, u2)` trims in place.
        bezier
            .call_method1("segment", (0.0, 0.5))
            .expect("segment(0, 0.5)");
        let trimmed = pair(&bezier.call_method1("value", (0.5,)).unwrap());
        assert_close(trimmed.0, 0.75, "trimmed Bezier end x");
        assert_close(trimmed.1, 0.25, "trimmed Bezier end y");

        // --- The abstract bases cannot be instantiated ----------------------
        for (name, message) in [
            ("Geometry2d", "'Geometry'"),
            ("Curve2d", "'Curve2d'"),
            ("Conic2d", "'Conic2d'"),
            ("ArcOfConic2d", "'ArcOfConic2d'"),
        ] {
            let error = class(name)
                .call0()
                .expect_err("an abstract geometry cannot be created");
            assert!(error.is_instance_of::<PyRuntimeError>(py), "{name}: {error}");
            assert!(
                error.to_string().contains(message),
                "{name}: {error} does not mention {message}"
            );
        }

        // --- BSplineCurve2d is a real kernel curve ---------------------------
        let bspline = class("BSplineCurve2d").call0().expect("BSplineCurve2d()");
        assert!(isinstance(py, &bspline, &class("Curve2d")));
        // Before a curve is built the empty class reports an honest error.
        let occ = occ_error_type(py);
        let error = bspline
            .call_method1("value", (0.5,))
            .expect_err("an unbuilt B-spline has no value");
        assert!(error.is_instance(py, &occ), "{error}");

        let points = PyTuple::new(
            py,
            [vector2(py, 0.0, 0.0), vector2(py, 1.0, 1.0)],
        )
        .unwrap();
        bspline
            .call_method1("interpolate", (points,))
            .expect("interpolate(points)");
        // `GeomAPI_Interpolate` parameterises by chord length, so the domain is
        // `[0, sqrt(2)]`; evaluate at the ends and the middle of that range.
        let first = number(&bspline.getattr("FirstParameter").expect("FirstParameter"), "first");
        let last = number(&bspline.getattr("LastParameter").expect("LastParameter"), "last");
        assert_close(first, 0.0, "interpolated first parameter");
        assert_close(last, 2f64.sqrt(), "interpolated last parameter");
        let start = pair(&bspline.call_method1("value", (first,)).expect("value(first)"));
        assert_close(start.0, 0.0, "interpolated start x");
        assert_close(start.1, 0.0, "interpolated start y");
        let end = pair(&bspline.call_method1("value", (last,)).expect("value(last)"));
        assert_close(end.0, 1.0, "interpolated end x");
        assert_close(end.1, 1.0, "interpolated end y");
        let middle = pair(
            &bspline
                .call_method1("value", (0.5 * (first + last),))
                .expect("value(middle)"),
        );
        assert_close(middle.0, 0.5, "interpolated midpoint x");
        assert_close(middle.1, 0.5, "interpolated midpoint y");
        assert_eq!(
            bspline
                .getattr("NbPoles")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            2
        );

        // Knot insertion rebuilds the curve with the same geometry.
        let middle_parameter = 0.5 * (first + last);
        bspline
            .call_method1("insertKnot", (middle_parameter,))
            .expect("insertKnot");
        assert_eq!(
            bspline
                .getattr("NbPoles")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            3
        );
        let inserted = pair(
            &bspline
                .call_method1("value", (middle_parameter,))
                .expect("value(middle)"),
        );
        assert_close(inserted.0, 0.5, "midpoint x after insertKnot");
        assert_close(inserted.1, 0.5, "midpoint y after insertKnot");

        // And it lifts onto a surface as a real edge.
        let cylinder = part
            .getattr("Cylinder")
            .expect("Part.Cylinder")
            .call0()
            .expect("Part.Cylinder()");
        let edge = bspline
            .call_method1("toShape", (&cylinder,))
            .expect("toShape(surface)");
        assert_eq!(shape_type(&edge), "Edge");
    });
}
