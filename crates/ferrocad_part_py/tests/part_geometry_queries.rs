//! The geometry queries SimpleCAD drives through `Part`: `Shape.isClosed`,
//! `Shape.getTolerance`, `Shape.distToShape`, `Edge.valueAt`, `Face.valueAt` and
//! the BREP string round trip.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::exceptions::{PyNotImplementedError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule, PyTuple, PyType};

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

fn number(value: &Bound<'_, PyAny>) -> f64 {
    value.extract::<f64>().expect("a number")
}

fn component(value: &Bound<'_, PyAny>, name: &str) -> f64 {
    number(
        &value
            .getattr(name)
            .unwrap_or_else(|error| panic!("Vector.{name}: {error}")),
    )
}

fn point(value: &Bound<'_, PyAny>) -> Point {
    (
        component(value, "x"),
        component(value, "y"),
        component(value, "z"),
    )
}

fn close(first: Point, second: Point) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-7;
    near(first.0, second.0) && near(first.1, second.1) && near(first.2, second.2)
}

fn distance(first: Point, second: Point) -> f64 {
    let d = |a: f64, b: f64| a - b;
    (d(first.0, second.0).powi(2) + d(first.1, second.1).powi(2) + d(first.2, second.2).powi(2))
        .sqrt()
}

fn sub_shapes<'py>(shape: &Bound<'py, PyAny>, name: &str) -> Vec<Bound<'py, PyAny>> {
    shape
        .getattr(name)
        .unwrap_or_else(|error| panic!("Shape.{name}: {error}"))
        .try_iter()
        .unwrap_or_else(|error| panic!("iter(Shape.{name}): {error}"))
        .map(|item| item.expect("a sub-shape"))
        .collect()
}

fn overall_tolerance(shape: &Bound<'_, PyAny>) -> f64 {
    number(
        &shape
            .call_method0("getTolerance")
            .expect("Shape.getTolerance"),
    )
}

fn occ_error(py: Python<'_>) -> Bound<'_, PyType> {
    PyModule::import(py, "Part")
        .expect("import Part")
        .getattr("OCCError")
        .expect("Part.OCCError")
        .downcast_into::<PyType>()
        .expect("OCCError is a type")
}

#[test]
fn closed_shapes_report_their_shell() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");
        assert!(
            box_shape
                .call_method0("isClosed")
                .expect("Shape.isClosed")
                .extract::<bool>()
                .expect("isClosed as bool"),
            "a box solid is closed"
        );

        // A bare face has a free boundary even though its own surface is closed.
        let face = sub_shapes(&box_shape, "Faces")[0].clone();
        assert!(
            !face
                .call_method0("isClosed")
                .expect("Face.isClosed")
                .extract::<bool>()
                .expect("isClosed as bool"),
            "a bare face is open"
        );

        let empty = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()");
        let error = empty.call_method0("isClosed").expect_err("null isClosed");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn tolerance_is_the_positive_kernel_value() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");

        let tolerance = overall_tolerance(&box_shape);
        assert!(tolerance.is_finite() && tolerance > 0.0, "{tolerance}");
        // The explicit default spelling is the same query.
        let explicit = number(
            &box_shape
                .call_method1("getTolerance", (0, py.None()))
                .expect("getTolerance(0, None)"),
        );
        assert_eq!(tolerance, explicit);

        for kind in ["Faces", "Edges", "Vertexes"] {
            for element in sub_shapes(&box_shape, kind) {
                let value = overall_tolerance(&element);
                assert!(value.is_finite() && value > 0.0, "{kind}: {value}");
            }
        }

        // Upstream's non-default modes and shape-type filters are not computed
        // here; they must say so instead of returning the maximum.
        let error = box_shape
            .call_method1("getTolerance", (1,))
            .expect_err("getTolerance(1)");
        assert!(error.is_instance_of::<PyNotImplementedError>(py), "{error}");

        let vertex = part.getattr("Vertex").expect("Part.Vertex");
        let error = box_shape
            .call_method1("getTolerance", (0, vertex))
            .expect_err("getTolerance(0, Part.Vertex)");
        assert!(error.is_instance_of::<PyNotImplementedError>(py), "{error}");

        // The filter is also accepted as the `type` keyword, as upstream names it.
        let kwargs = PyDict::new(py);
        kwargs.set_item("mode", 0).unwrap();
        kwargs
            .set_item("type", part.getattr("Vertex").unwrap())
            .unwrap();
        let error = box_shape
            .call_method("getTolerance", (), Some(&kwargs))
            .expect_err("getTolerance(mode=0, type=Part.Vertex)");
        assert!(error.is_instance_of::<PyNotImplementedError>(py), "{error}");
    });
}

#[test]
fn two_separated_boxes_report_the_gap_and_its_endpoints() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");

        let first = part
            .call_method1("makeBox", (2.0, 2.0, 2.0))
            .expect("first box");
        let at_seven = vector.call1((7.0, 0.0, 0.0)).expect("a vector");
        let second = part
            .call_method1("makeBox", (2.0, 2.0, 2.0, &at_seven))
            .expect("second box");

        let result = first
            .call_method1("distToShape", (&second,))
            .expect("distToShape");
        let result = result.downcast::<PyTuple>().expect("a 3-tuple");
        assert_eq!(result.len(), 3);

        let reported = number(&result.get_item(0).unwrap());
        assert!((reported - 5.0).abs() < 1e-9, "{reported}");

        let pairs = result.get_item(1).unwrap();
        let pairs = pairs.downcast::<PyList>().expect("a list of pairs");
        assert_eq!(pairs.len(), 1);
        let pair = pairs.get_item(0).unwrap();
        let pair = pair.downcast::<PyTuple>().expect("a point pair");
        assert_eq!(pair.len(), 2);
        let on_first = point(&pair.get_item(0).unwrap());
        let on_second = point(&pair.get_item(1).unwrap());

        assert!((on_first.0 - 2.0).abs() < 1e-9, "{on_first:?}");
        assert!((on_second.0 - 7.0).abs() < 1e-9, "{on_second:?}");
        assert!(
            (on_first.1 - on_second.1).abs() < 1e-9,
            "{on_first:?} {on_second:?}"
        );
        assert!(
            (on_first.2 - on_second.2).abs() < 1e-9,
            "{on_first:?} {on_second:?}"
        );
        assert!((distance(on_first, on_second) - 5.0).abs() < 1e-9);

        let info = result.get_item(2).unwrap();
        assert!(
            info.downcast::<PyList>()
                .expect("the support info list")
                .is_empty(),
            "this slice has no support-type info"
        );

        // The pair follows the operand order.
        let swapped = second
            .call_method1("distToShape", (&first,))
            .expect("swapped distToShape");
        let swapped = swapped.downcast::<PyTuple>().expect("a 3-tuple");
        let swapped_pairs = swapped.get_item(1).unwrap();
        let swapped_pair = swapped_pairs
            .downcast::<PyList>()
            .expect("a list of pairs")
            .get_item(0)
            .unwrap();
        let swapped_pair = swapped_pair.downcast::<PyTuple>().expect("a point pair");
        assert!(close(point(&swapped_pair.get_item(0).unwrap()), on_second));
        assert!(close(point(&swapped_pair.get_item(1).unwrap()), on_first));

        // Only a Part.Shape is accepted, and a null shape cannot be measured.
        let error = first
            .call_method1("distToShape", (42,))
            .expect_err("distToShape(42)");
        assert!(error.is_instance_of::<PyTypeError>(py), "{error}");

        let empty = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()");
        let error = empty
            .call_method1("distToShape", (&first,))
            .expect_err("null distToShape");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = first
            .call_method1("distToShape", (&empty,))
            .expect_err("distToShape(null)");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn edge_value_at_maps_parameters_to_points() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("makeBox");
        let edge = sub_shapes(&box_shape, "Edges")[0].clone();

        let first: f64 = edge
            .getattr("FirstParameter")
            .expect("FirstParameter")
            .extract()
            .expect("f64");
        let last: f64 = edge
            .getattr("LastParameter")
            .expect("LastParameter")
            .extract()
            .expect("f64");
        let start = point(
            &edge
                .call_method1("valueAt", (first,))
                .expect("valueAt(first)"),
        );
        let end = point(
            &edge
                .call_method1("valueAt", (last,))
                .expect("valueAt(last)"),
        );
        assert!(
            (distance(start, end) - 1.0).abs() < 1e-9,
            "{start:?} {end:?}"
        );

        // A line's parameter is arc length, so the middle parameter is the midpoint.
        let middle = point(
            &edge
                .call_method1("valueAt", ((first + last) / 2.0,))
                .expect("valueAt(mid)"),
        );
        let expected = (
            (start.0 + end.0) / 2.0,
            (start.1 + end.1) / 2.0,
            (start.2 + end.2) / 2.0,
        );
        assert!(close(middle, expected), "{middle:?} != {expected:?}");

        // The endpoints are the edge's vertices.
        let vertices = [
            point(
                &edge
                    .call_method0("firstVertex")
                    .unwrap()
                    .getattr("Point")
                    .unwrap(),
            ),
            point(
                &edge
                    .call_method0("lastVertex")
                    .unwrap()
                    .getattr("Point")
                    .unwrap(),
            ),
        ];
        assert!(vertices.iter().any(|vertex| close(*vertex, start)));
        assert!(vertices.iter().any(|vertex| close(*vertex, end)));

        let empty_edge = part.getattr("Edge").unwrap().call0().expect("Part.Edge()");
        let error = empty_edge
            .call_method1("valueAt", (0.0,))
            .expect_err("empty valueAt");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn face_value_at_evaluates_a_plane() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");
        let box_face = sub_shapes(&box_shape, "Faces")[0].clone();
        // `Shape.Faces` currently yields a plain `Part.Shape` (the flat-face
        // typing gap), so build a real `Part.Face` from that boundary.
        let face = part
            .call_method1("Face", (&box_face,))
            .expect("Part.Face(box face)");

        let range: Vec<f64> = face
            .getattr("ParameterRange")
            .expect("ParameterRange")
            .extract()
            .expect("(u0, u1, v0, v1)");
        assert_eq!(range.len(), 4);

        // A plane is affine in (u, v), so the parameter corners map to the
        // rectangle's corners and the middle of the bounds to its centre.
        let mut corners = Vec::new();
        for (u, v) in [
            (range[0], range[2]),
            (range[1], range[2]),
            (range[1], range[3]),
            (range[0], range[3]),
        ] {
            corners.push(point(
                &face.call_method1("valueAt", (u, v)).expect("Face.valueAt"),
            ));
        }
        let vertices: Vec<Point> = sub_shapes(&box_face, "Vertexes")
            .iter()
            .map(|vertex| point(&vertex.getattr("Point").expect("Vertex.Point")))
            .collect();
        assert_eq!(vertices.len(), 4);
        for corner in &corners {
            assert!(
                vertices.iter().any(|vertex| close(*vertex, *corner)),
                "{corner:?} is not one of the face's corners {vertices:?}"
            );
        }

        let middle = point(
            &face
                .call_method1(
                    "valueAt",
                    ((range[0] + range[1]) / 2.0, (range[2] + range[3]) / 2.0),
                )
                .expect("Face.valueAt"),
        );
        let expected = (
            corners.iter().map(|corner| corner.0).sum::<f64>() / 4.0,
            corners.iter().map(|corner| corner.1).sum::<f64>() / 4.0,
            corners.iter().map(|corner| corner.2).sum::<f64>() / 4.0,
        );
        assert!(close(middle, expected), "{middle:?} != {expected:?}");

        let empty_face = part.getattr("Face").unwrap().call0().expect("Part.Face()");
        let error = empty_face
            .call_method1("valueAt", (0.0, 0.0))
            .expect_err("empty Face.valueAt");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn brep_strings_round_trip() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");
        let brep: Vec<u8> = box_shape
            .call_method0("exportBrepToString")
            .expect("exportBrepToString")
            .extract()
            .expect("BREP bytes");
        assert!(!brep.is_empty());

        // The empty shape loads in place (upstream's `TopoShape::importBrep`).
        let restored = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()");
        restored
            .call_method1("importBrepFromString", (brep.clone(),))
            .expect("importBrepFromString(bytes)");
        assert!(restored
            .call_method0("isClosed")
            .expect("isClosed")
            .extract::<bool>()
            .unwrap());
        let volume = number(&restored.getattr("Volume").expect("Volume"));
        assert!((volume - 24.0).abs() < 1e-9, "{volume}");

        // OCCT's BREP text is canonical, so re-exporting reproduces it.
        let again: Vec<u8> = restored
            .call_method0("exportBrepToString")
            .expect("re-export")
            .extract()
            .expect("BREP bytes");
        assert_eq!(again, brep);

        // A `str` (what upstream returns) is accepted too, and the call replaces
        // an existing shape rather than only filling a null one.
        let text = String::from_utf8(brep.clone()).expect("BREP text is UTF-8");
        let target = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("a small box");
        target
            .call_method1("importBrepFromString", (&text,))
            .expect("importBrepFromString(str)");
        let replaced = number(&target.getattr("Volume").expect("Volume"));
        assert!((replaced - 24.0).abs() < 1e-9, "{replaced}");

        // Invalid data is an OCCError; a non-string is a TypeError.
        let empty = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()");
        let error = empty
            .call_method1("importBrepFromString", ("not a brep",))
            .expect_err("invalid BREP");
        let occ = occ_error(py);
        assert!(error.is_instance(py, &occ), "{error}");

        let error = empty
            .call_method1("importBrepFromString", (42,))
            .expect_err("importBrepFromString(42)");
        assert!(error.is_instance_of::<PyTypeError>(py), "{error}");
    });
}
