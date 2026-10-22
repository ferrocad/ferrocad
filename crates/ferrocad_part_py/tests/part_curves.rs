//! The analytic half of `Part`: curves, edges and wires, end to end.
//!
//! Runs in the same image as `part_boot`: `ferrocad` + `Part` as built-ins, one
//! core, one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use pyo3::prelude::*;
use pyo3::types::{PyList, PyModule};

/// Register both built-in modules before the interpreter initialises.
fn boot() {
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
}

#[test]
fn curves_edges_and_wires_round_trip() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let point = |x: f64, y: f64, z: f64| {
            vector
                .call1((x, y, z))
                .expect("a ferrocad.Vector")
                .unbind()
        };

        let p1 = point(0.0, 0.0, 0.0);
        let p2 = point(4.0, 0.0, 0.0);

        // Part.LineSegment → toShape() → Edge.Curve is the basis Line.
        let segment = part
            .getattr("LineSegment")
            .expect("Part.LineSegment")
            .call1((&p1, &p2))
            .expect("LineSegment(0,0,0 -> 4,0,0)");
        let start = segment.getattr("StartPoint").expect("StartPoint");
        assert!(start.getattr("x").unwrap().extract::<f64>().unwrap().abs() < 1e-9);
        let edge = segment.call_method0("toShape").expect("toShape");
        let shape_class = part.getattr("Shape").expect("Part.Shape");
        assert!(
            edge.is_instance(&shape_class).unwrap(),
            "Part.Edge is a Part.Shape subclass, as upstream"
        );
        let curve = edge.getattr("Curve").expect("Curve");
        assert_eq!(
            curve.get_type().name().unwrap(),
            "Line",
            "a segment's basis curve is an infinite Line, as upstream"
        );

        // Part.Arc is the Part.ArcOfCircle class.
        let arc = part.getattr("Arc").expect("Part.Arc");
        let arc_of_circle = part.getattr("ArcOfCircle").expect("Part.ArcOfCircle");
        assert_eq!(arc.as_ptr(), arc_of_circle.as_ptr());

        // makeCircle → an edge whose Curve is a Circle with the radius.
        let circle_edge = part
            .call_method1("makeCircle", (2.0,))
            .expect("Part.makeCircle(2)");
        let circle = circle_edge.getattr("Curve").expect("circle edge Curve");
        assert_eq!(circle.get_type().name().unwrap(), "Circle");
        assert!((circle.getattr("Radius").unwrap().extract::<f64>().unwrap() - 2.0).abs() < 1e-9);

        // makePolygon(closed) → a wire of four edges that closes.
        let points = PyList::new(
            py,
            [
                point(0.0, 0.0, 0.0),
                point(4.0, 0.0, 0.0),
                point(4.0, 4.0, 0.0),
                point(0.0, 4.0, 0.0),
            ],
        )
        .unwrap();
        let wire = part
            .call_method1("makePolygon", (points, true))
            .expect("Part.makePolygon(points, True)");
        assert!(
            wire.call_method0("isClosed")
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the wire closes"
        );
        assert_eq!(wire.getattr("Edges").unwrap().len().unwrap(), 4);
        assert!(
            wire.is_instance(&shape_class).unwrap(),
            "Part.Wire is a Part.Shape subclass too"
        );
        assert_eq!(wire.len().unwrap(), 4);
        let listed = py
            .import("builtins")
            .unwrap()
            .getattr("list")
            .unwrap()
            .call1((&wire,))
            .expect("list(wire) iterates the edges");
        assert_eq!(listed.len().unwrap(), 4, "the sequence protocol iterates");
        let wire_shape = wire.call_method0("toShape").expect("wire.toShape");
        assert!(
            !wire_shape
                .call_method0("isNull")
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "make_wire produced a shape"
        );

        // Part.Vertex and Shape.Vertexes
        let vertex = part
            .call_method1("Vertex", (1.0, 2.0, 3.0))
            .expect("Part.Vertex(1, 2, 3)");
        let vertex_point = vertex.getattr("Point").expect("Vertex.Point");
        assert!(
            (vertex_point.getattr("x").unwrap().extract::<f64>().unwrap() - 1.0).abs() < 1e-9
        );
        assert_eq!(vertex.getattr("X").unwrap().extract::<f64>().unwrap(), 1.0);

        // Part.makeFace(square wire) → a planar face with four edges.
        let face = part
            .call_method1("makeFace", (&wire,))
            .expect("Part.makeFace(wire)");
        assert_eq!(face.getattr("Faces").unwrap().len().unwrap(), 1);
        assert_eq!(face.getattr("Edges").unwrap().len().unwrap(), 4);
        assert!(
            face.is_instance(&shape_class).unwrap(),
            "Part.Face is a Part.Shape subclass"
        );
        let face_from_class = part.call_method1("Face", (&wire,)).expect("Part.Face(wire)");
        assert_eq!(face_from_class.getattr("Edges").unwrap().len().unwrap(), 4);

        // A solid's Edges come from the kernel's own enumeration.
        let cube = part.call_method1("makeBox", (1.0, 1.0, 1.0)).unwrap();
        assert_eq!(cube.getattr("Edges").unwrap().len().unwrap(), 12);
        assert_eq!(cube.getattr("Faces").unwrap().len().unwrap(), 6);
        assert_eq!(cube.getattr("Vertexes").unwrap().len().unwrap(), 8);
        let corner = cube.getattr("Vertexes").unwrap().get_item(0).unwrap();
        assert!(corner.getattr("Point").is_ok(), "Vertexes carry Point");

        // An infinite line has no edge; the failure is a Part.OCCError.
        let occ = part.getattr("OCCError").expect("Part.OCCError");
        let line = part
            .getattr("Line")
            .unwrap()
            .call1((point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)))
            .expect("Part.Line");
        let error = line.call_method0("toShape").expect_err("a line has no edge");
        let occ_type = occ.downcast::<pyo3::types::PyType>().unwrap();
        assert!(error.is_instance(py, occ_type));
    });
}

#[test]
fn bspline_interpolation_builds_an_edge() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let vector = ferrocad.getattr("Vector").expect("ferrocad.Vector");
        let points = PyList::new(
            py,
            [
                vector.call1((0.0, 0.0, 0.0)).unwrap(),
                vector.call1((10.0, 0.0, 5.0)).unwrap(),
                vector.call1((10.0, 10.0, -3.0)).unwrap(),
                vector.call1((0.0, 10.0, 4.0)).unwrap(),
            ],
        )
        .unwrap();
        let curve = part.getattr("BSplineCurve").unwrap().call0().expect("Part.BSplineCurve");
        curve
            .call_method1("interpolate", (points, true))
            .expect("BSplineCurve.interpolate(points, PeriodicFlag=True)");
        let edge = curve.call_method0("toShape").expect("BSplineCurve.toShape");
        assert!(
            !edge
                .call_method0("isNull")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        assert!(edge.getattr("Length").unwrap().extract::<f64>().unwrap() > 0.0);
        let wire = part.getattr("Wire").unwrap().call1((&edge,)).expect("Part.Wire(edge)");
        assert_eq!(wire.len().unwrap(), 1);

        // The control-point constructor produces a B-spline too.
        let poles = PyList::new(
            py,
            [
                vector.call1((0.0, 0.0, 0.0)).unwrap(),
                vector.call1((10.0, 0.0, 5.0)).unwrap(),
                vector.call1((10.0, 10.0, -3.0)).unwrap(),
                vector.call1((0.0, 10.0, 4.0)).unwrap(),
            ],
        )
        .unwrap();
        let by_poles = part
            .getattr("BSplineCurve")
            .unwrap()
            .call1((poles,))
            .expect("Part.BSplineCurve(poles)");
        let pole_edge = by_poles.call_method0("toShape").expect("toShape from poles");
        assert!(pole_edge.getattr("Length").unwrap().extract::<f64>().unwrap() > 0.0);
    });
}
