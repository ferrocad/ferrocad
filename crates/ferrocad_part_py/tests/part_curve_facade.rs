//! `Part.makeFace` boundary sources and `BSplineCurve.buildFromPoles`, end to end.
//!
//! Same boot as `part_curves`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

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

/// A `ferrocad.Vector` at `(x, y, z)`.
fn vector<'py>(
    ferrocad: &Bound<'py, PyModule>,
    (x, y, z): (f64, f64, f64),
) -> Bound<'py, PyAny> {
    ferrocad
        .getattr("Vector")
        .expect("ferrocad.Vector")
        .call1((x, y, z))
        .expect("a ferrocad.Vector")
}

/// A closed wire through `points`, each an `(x, y)` pair on the XY plane.
fn polygon<'py>(part: &Bound<'py, PyModule>, points: &[(f64, f64)]) -> Bound<'py, PyAny> {
    let ferrocad = PyModule::import(part.py(), "ferrocad").expect("import ferrocad");
    let mut items = Vec::with_capacity(points.len() + 1);
    for &(x, y) in points.iter().chain(points.first()) {
        items.push(vector(&ferrocad, (x, y, 0.0)));
    }
    let list = PyList::new(part.py(), &items).expect("point list");
    part.call_method1("makePolygon", (&list,))
        .expect("Part.makePolygon(points)")
}

fn length(bound_box: &Bound<'_, PyAny>, prefix: &str) -> f64 {
    bound_box
        .getattr(format!("{prefix}Length"))
        .expect("bound box length")
        .extract::<f64>()
        .expect("length as f64")
}

fn bound_lengths(shape: &Bound<'_, PyAny>) -> (f64, f64, f64) {
    let bound_box = shape.getattr("BoundBox").expect("Shape.BoundBox");
    (
        length(&bound_box, "X"),
        length(&bound_box, "Y"),
        length(&bound_box, "Z"),
    )
}

fn count(shape: &Bound<'_, PyAny>, attribute: &str) -> usize {
    shape
        .getattr(attribute)
        .expect("Shape sub-shape list")
        .len()
        .expect("sub-shape count")
}

#[test]
fn make_face_accepts_a_face_and_uses_its_outer_wire() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cube = part
            .call_method1("makeBox", (10.0, 20.0, 30.0))
            .expect("Part.makeBox");
        let source = cube
            .getattr("Faces")
            .expect("Shape.Faces")
            .get_item(0)
            .expect("a box face");
        let source_type = source.get_type().name().expect("type name").to_string();
        assert!(
            source_type == "PartShape" || source_type == "Shape",
            "Shape.Faces yields PartShape objects, got {source_type}"
        );
        let expected = bound_lengths(&source);

        // Upstream's `testAppPartMakeFace`: a face is a valid `makeFace` argument.
        let face = part
            .call_method1("makeFace", (&source,))
            .expect("Part.makeFace(box.Faces[0])");
        assert!(
            !face
                .call_method0("isNull")
                .expect("isNull")
                .extract::<bool>()
                .expect("bool"),
            "makeFace(face) must produce a face"
        );
        assert_eq!(count(&face, "Faces"), 1, "one face");
        assert_eq!(count(&face, "Edges"), 4, "the box face's four boundary edges");
        let actual = bound_lengths(&face);
        for (kind, got, want) in [("X", actual.0, expected.0), ("Y", actual.1, expected.1), ("Z", actual.2, expected.2)] {
            assert!(
                (got - want).abs() < 1e-6,
                "the rebuilt face must keep the source face's {kind} extent (got {got}, want {want})"
            );
        }

        // The same face through the `Part.Face` class, `noElementMap` accepted.
        let by_class = part
            .call_method1("Face", (&source,))
            .expect("Part.Face(box.Faces[0])");
        assert_eq!(count(&by_class, "Edges"), 4);
        let without_map = part
            .call_method1("makeFace", (&source, "Part::FaceMakerBullseye", true))
            .expect("Part.makeFace(face, ..., noElementMap=True)");
        assert_eq!(count(&without_map, "Edges"), 4);
    });
}

#[test]
fn make_face_still_accepts_a_compound_of_wires() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let outer = polygon(&part, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let hole = polygon(&part, &[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)]);
        let children = PyList::new(py, [&outer, &hole]).expect("wire list");
        let compound = part
            .getattr("Compound")
            .expect("Part.Compound")
            .call1((&children,))
            .expect("Part.Compound([outer, hole])");

        let face = part
            .call_method1("makeFace", (&compound,))
            .expect("Part.makeFace(compound of wires)");
        assert!(!face
            .call_method0("isNull")
            .expect("isNull")
            .extract::<bool>()
            .expect("bool"));
        assert_eq!(count(&face, "Faces"), 1);
        assert_eq!(count(&face, "Edges"), 8, "outer and hole boundaries");
        let area = face.getattr("Area").expect("Area").extract::<f64>().expect("area");
        assert!(
            (area - 84.0).abs() < 1e-6,
            "10x10 outer minus a 4x4 hole is 84, got {area}"
        );

        // A bare wire keeps working unchanged.
        let from_wire = part
            .call_method1("makeFace", (&outer,))
            .expect("Part.makeFace(wire)");
        assert_eq!(count(&from_wire, "Edges"), 4);
    });
}

#[test]
fn build_from_poles_builds_a_bspline_edge() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");

        // The upstream `BRep_tests.testProject` poles: a zig-zag of 18 points.
        let poles: Vec<_> = (0..18)
            .map(|index| vector(&ferrocad, (index as f64, (index % 2) as f64, 0.0)))
            .collect();
        let poles = PyList::new(py, &poles).expect("pole list");
        let curve = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("Part.BSplineCurve()");
        curve
            .call_method1("buildFromPoles", (&poles, false, 1))
            .expect("BSplineCurve.buildFromPoles(poles, False, 1)");
        let edge = curve.call_method0("toShape").expect("BSplineCurve.toShape");
        assert!(
            !edge
                .call_method0("isNull")
                .expect("isNull")
                .extract::<bool>()
                .expect("bool"),
            "buildFromPoles must produce an edge"
        );
        let edge_length = edge.getattr("Length").expect("Length").extract::<f64>().expect("length");
        assert!(edge_length > 0.0, "the edge must have length, got {edge_length}");

        // Interpolation is untouched.
        let through = PyList::new(
            py,
            [
                vector(&ferrocad, (0.0, 0.0, 0.0)),
                vector(&ferrocad, (10.0, 0.0, 5.0)),
                vector(&ferrocad, (10.0, 10.0, -3.0)),
                vector(&ferrocad, (0.0, 10.0, 4.0)),
            ],
        )
        .expect("point list");
        let interpolated = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("Part.BSplineCurve()");
        interpolated
            .call_method1("interpolate", (&through, true))
            .expect("BSplineCurve.interpolate(points, PeriodicFlag=True)");
        let interpolated_edge = interpolated
            .call_method0("toShape")
            .expect("BSplineCurve.toShape");
        assert!(
            interpolated_edge
                .getattr("Length")
                .expect("Length")
                .extract::<f64>()
                .expect("length")
                > 0.0
        );
    });
}

#[test]
fn build_from_poles_rejects_fewer_than_two_poles() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let ferrocad = PyModule::import(py, "ferrocad").expect("import ferrocad");
        let poles = PyList::new(py, [vector(&ferrocad, (0.0, 0.0, 0.0))]).expect("pole list");
        let curve = part
            .getattr("BSplineCurve")
            .expect("Part.BSplineCurve")
            .call0()
            .expect("Part.BSplineCurve()");
        let error = curve
            .call_method1("buildFromPoles", (&poles, false, 1))
            .expect_err("one pole is not a curve");
        let occ = part
            .getattr("OCCError")
            .expect("Part.OCCError")
            .downcast_into::<pyo3::types::PyType>()
            .expect("OCCError is a type");
        assert!(
            error.is_instance(py, &occ),
            "a one-pole spline is a Part.OCCError, got {error}"
        );
    });
}
