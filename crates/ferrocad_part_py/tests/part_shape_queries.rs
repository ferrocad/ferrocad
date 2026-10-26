//! The topology query surface SimpleCAD relies on: `Shape.countElement`,
//! `Edge.Degenerated`, `Edge.isSeam`, `ParameterRange` and `Shape.isEqual`.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

use std::f64::consts::TAU;

use pyo3::exceptions::{PyTypeError, PyValueError};
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

fn count_element(shape: &Bound<'_, PyAny>, kind: &str) -> usize {
    shape
        .call_method1("countElement", (kind,))
        .unwrap_or_else(|error| panic!("countElement({kind}): {error}"))
        .extract()
        .expect("countElement as int")
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

fn floats(value: &Bound<'_, PyAny>) -> Vec<f64> {
    value
        .try_iter()
        .expect("a tuple of floats")
        .map(|item| item.expect("a float").extract().expect("extract f64"))
        .collect()
}

#[test]
fn count_element_counts_the_named_topology() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");

        assert_eq!(count_element(&shape, "Face"), 6);
        assert_eq!(count_element(&shape, "Edge"), 12);
        assert_eq!(count_element(&shape, "Vertex"), 8);
        assert_eq!(count_element(&shape, "Solid"), 1);
        assert_eq!(count_element(&shape, "Shell"), 1);
        assert_eq!(count_element(&shape, "Compound"), 0);
        assert_eq!(count_element(&shape, "CompSolid"), 0);
        assert_eq!(count_element(&shape, "SubShape"), 1);
        assert_eq!(count_element(&shape, "Bogus"), 0);

        // Upstream takes a string; a type object is a TypeError, not a count.
        let error = shape
            .call_method1("countElement", (part.getattr("Face").unwrap(),))
            .expect_err("countElement(Part.Face)");
        assert!(error.is_instance_of::<PyTypeError>(py), "{error}");
    });
}

#[test]
fn degenerate_edges_are_reported() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cone = part
            .call_method1("makeCone", (5.0, 0.0, 10.0))
            .expect("makeCone with an apex");
        assert!(
            sub_shapes(&cone, "Edges").iter().any(|edge| {
                edge.getattr("Degenerated")
                    .expect("Edge.Degenerated")
                    .extract::<bool>()
                    .expect("Degenerated as bool")
            }),
            "a cone apex has a degenerate edge"
        );

        let box_shape = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("makeBox");
        assert!(
            sub_shapes(&box_shape, "Edges").iter().all(|edge| {
                !edge
                    .getattr("Degenerated")
                    .expect("Edge.Degenerated")
                    .extract::<bool>()
                    .expect("Degenerated as bool")
            }),
            "a box has no degenerate edge"
        );
    });
}

#[test]
fn seam_edges_name_their_face() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder = part
            .call_method1("makeCylinder", (5.0, 10.0))
            .expect("makeCylinder");
        let edges = sub_shapes(&cylinder, "Edges");
        let faces = sub_shapes(&cylinder, "Faces");
        let seam = edges.iter().any(|edge| {
            faces.iter().any(|face| {
                edge.call_method1("isSeam", (face,))
                    .expect("Edge.isSeam")
                    .extract::<bool>()
                    .expect("isSeam as bool")
            })
        });
        assert!(seam, "a full cylinder has a seam edge");

        let box_shape = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("makeBox");
        let box_edges = sub_shapes(&box_shape, "Edges");
        let box_faces = sub_shapes(&box_shape, "Faces");
        assert!(
            box_edges.iter().all(|edge| box_faces.iter().all(|face| {
                !edge
                    .call_method1("isSeam", (face,))
                    .expect("Edge.isSeam")
                    .extract::<bool>()
                    .expect("isSeam as bool")
            })),
            "a box has no seam edge"
        );

        // A null shape on either side is a ValueError.
        let empty_edge = part.getattr("Edge").unwrap().call0().expect("Part.Edge()");
        let error = empty_edge
            .call_method1("isSeam", (&faces[0],))
            .expect_err("empty edge isSeam");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");

        let empty_shape = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()");
        let error = edges[0]
            .call_method1("isSeam", (&empty_shape,))
            .expect_err("null face isSeam");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn parameter_ranges_describe_faces_and_edges() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let cylinder = part
            .call_method1("makeCylinder", (5.0, 10.0))
            .expect("makeCylinder");
        let face = sub_shapes(&cylinder, "Faces")[0].clone();
        let range = floats(&face.getattr("ParameterRange").expect("Face.ParameterRange"));
        assert_eq!(range.len(), 4, "a face reports (u0, u1, v0, v1)");
        assert!((range[1] - range[0] - TAU).abs() < 1e-6, "a full turn in U");
        assert!((range[3] - range[2] - 10.0).abs() < 1e-6, "the height in V");

        let box_shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");
        let edge = sub_shapes(&box_shape, "Edges")[0].clone();
        let range = floats(&edge.getattr("ParameterRange").expect("Edge.ParameterRange"));
        assert_eq!(range.len(), 2, "an edge reports (first, last)");
        assert!(range[1] > range[0], "{range:?}");
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
        assert_eq!((first, last), (range[0], range[1]));

        let error = part
            .getattr("Shape")
            .unwrap()
            .call0()
            .expect("Part.Shape()")
            .getattr("ParameterRange")
            .expect_err("null ParameterRange");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
    });
}

#[test]
fn is_equal_is_stricter_than_is_same() {
    boot();
    Python::with_gil(|py| {
        let part = PyModule::import(py, "Part").expect("import Part");
        let shape = part
            .call_method1("makeBox", (2.0, 3.0, 4.0))
            .expect("makeBox");
        let shape_class = part.getattr("Shape").unwrap();

        let copy = shape_class.call1((&shape,)).expect("Part.Shape(box)");
        assert!(
            shape
                .call_method1("isEqual", (&copy,))
                .expect("isEqual")
                .extract::<bool>()
                .unwrap()
        );

        let reversed = shape.call_method0("reversed").expect("Shape.reversed");
        assert!(
            shape
                .call_method1("isSame", (&reversed,))
                .expect("isSame")
                .extract::<bool>()
                .unwrap(),
            "reversing keeps the TShape"
        );
        assert!(
            !shape
                .call_method1("isEqual", (&reversed,))
                .expect("isEqual")
                .extract::<bool>()
                .unwrap(),
            "reversing changes the orientation"
        );

        // A null shape on either side is a ValueError, and a non-shape is a
        // TypeError.
        let empty = shape_class.call0().expect("Part.Shape()");
        let error = empty
            .call_method1("isEqual", (&shape,))
            .expect_err("null isEqual");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = shape
            .call_method1("isEqual", (&empty,))
            .expect_err("isEqual(null)");
        assert!(error.is_instance_of::<PyValueError>(py), "{error}");
        let error = shape
            .call_method1("isEqual", (42,))
            .expect_err("isEqual(int)");
        assert!(error.is_instance_of::<PyTypeError>(py), "{error}");
    });
}

/// Run a Python snippet against the booted `Part` module.
fn run(py: Python<'_>, source: &str) {
    use pyo3::types::PyDict;
    use std::ffi::CString;

    let code = CString::new(source).expect("the snippet has no NUL");
    let globals = PyDict::new(py);
    if let Err(error) = py.run(&code, Some(&globals), None) {
        error.print(py);
        panic!("the Part shape-query snippet failed:\n{source}");
    }
}

#[test]
fn hash_code_follows_is_same_and_accepts_the_upper_bound() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 3, 4)

# Stable within the process, and equal exactly when the TShape and location are
# (orientation ignored), like upstream's `ShapeMapHasher`.
assert box.hashCode() == box.hashCode()
assert box.hashCode() == Part.Shape(box).hashCode()
assert box.hashCode() == box.reversed().hashCode(), "orientation is not part of the hash"
assert box.hashCode() != box.copy().hashCode(), "a new TShape hashes differently"

# Sub-shape wrappers around one kernel element hash alike, which is how
# SimpleCAD keys its edge/face adjacency maps.
assert box.Edges[0].hashCode() == box.Edges[0].hashCode()
assert len({box.Edges[0].hashCode(), box.Edges[0].hashCode()}) == 1
assert len({edge.hashCode() for edge in box.Edges}) == 12

# `upper` is upstream's modulo bound (IntegerLast by default), so the value is
# non-negative and below it.
assert 0 <= box.hashCode() <= 2**31 - 1
assert 0 <= box.hashCode(10) < 10
assert box.hashCode(1) == 0

# A null shape hashes to 0, matching `hash(Part.Shape())`.
assert Part.Shape().hashCode() == 0
"#,
        );
    });
}

#[test]
fn is_partner_ignores_location_and_orientation() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 3, 4)

assert box.isPartner(box)
assert box.isPartner(Part.Shape(box))
assert box.isPartner(box.reversed()), "orientation is not part of the partner test"
assert box.isSame(box.reversed()) and box.isPartner(box.reversed())

# A deep copy has a new TShape, so it is not a partner even though its geometry
# is identical.
assert box.isPartner(box.copy()) is False
assert box.isSame(box.copy()) is False

# `isPartner` is weaker than `isSame`: a same-TShape handle with a different
# location would be a partner but not same. This seam bakes transforms into a
# new TShape, so the observable case here is a different shape.
other = Part.makeBox(5, 5, 5)
assert box.isPartner(other) is False

# Null shapes and non-shapes.
assert Part.Shape().isPartner(box) is False
assert box.isPartner(Part.Shape()) is False
try:
    box.isPartner(42)
    raise AssertionError("a non-shape argument must raise")
except TypeError:
    pass
"#,
        );
    });
}
