//! `Part.Vertex` as a shape and as a sequence, mirroring upstream's
//! `TestPartApp.TopoShapeTest` vertex cases.
//!
//! Runs in the same image as `part_curves`: `ferrocad` + `Part` as built-ins, one
//! core, one OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

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

/// Run a Python snippet against the booted `Part` module, reporting the Python
/// traceback when it fails.
fn run(py: Python<'_>, source: &str) {
    let code = CString::new(source).expect("the snippet has no NUL");
    let globals = PyDict::new(py);
    if let Err(error) = py.run(&code, Some(&globals), None) {
        error.print(py);
        panic!("the vertex snippet failed:\n{source}");
    }
}

#[test]
fn a_vertex_copy_is_the_same_shape() {
    boot();
    Python::with_gil(|py| {
        // Same assertions as upstream's `testTopoShapeVertex` and
        // `testTopoShapeVertexHashFollowsEquality`.
        run(
            py,
            r#"
import Part

box = Part.makeBox(10, 10, 10)
v1 = box.Vertexes[0]
v2 = Part.Vertex(v1)

assert isinstance(v2, Part.Shape)
assert v1 == v2
assert v2 == v1
assert v1.isSame(v2)
assert v1 in box.Vertexes
assert v2 in box.Vertexes
assert tuple(v1.Point) == tuple(v2.Point)

# The copy is a vertex at the same point, but a vertex of its own identity.
v3 = Part.Vertex(tuple(v2))
assert v3 != v1
assert v3 not in box.Vertexes
assert Part.Vertex(1, 2, 3) != Part.Vertex(1, 2, 3)

# Hash follows equality: one entry for the copy, eight for the box corners.
assert hash(v1) == hash(v2)
assert len({v1, v2}) == 1
assert len(set(box.Vertexes)) == 8
assert v2 in {v1: "found"}
"#,
        );
    });
}

#[test]
fn a_vertex_is_a_point_sequence() {
    boot();
    Python::with_gil(|py| {
        // Same assertions as upstream's `testTopoShapeVertexSequence`.
        run(
            py,
            r#"
import Part
import ferrocad as App

v = Part.Vertex(1, 2, 3)
assert len(v) == 3
assert tuple(v) == (1, 2, 3)
assert list(v) == [1.0, 2.0, 3.0]
assert v[0] == 1
assert v[2] == 3
assert v[-1] == 3
assert v[-3] == 1
assert App.Vector(v) == App.Vector(1, 2, 3)

try:
    v[3]
except IndexError:
    pass
else:
    raise AssertionError("v[3] must raise IndexError")

try:
    v[-4]
except IndexError:
    pass
else:
    raise AssertionError("v[-4] must raise IndexError")

try:
    tuple(Part.Vertex())
except RuntimeError:
    pass
else:
    raise AssertionError("tuple(Part.Vertex()) must raise RuntimeError")
"#,
        );
    });
}

#[test]
fn the_api_null_vertex_is_not_the_origin_vertex() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

assert repr(Part.Vertex(1, 2, 3)) == "<Part.Vertex (1, 2, 3)>"
assert repr(Part.Vertex()) == "<Part.Vertex()>"
assert repr(Part.Vertex(0, 0, 0)) == "<Part.Vertex (0, 0, 0)>"

# `Part.Vertex()` is the null vertex; explicit coordinates are a real one, even
# at the origin.
assert Part.Vertex().isNull()
assert not Part.Vertex(0, 0, 0).isNull()
assert Part.Vertex() == Part.Vertex()
assert Part.Vertex(0, 0, 0) != Part.Vertex()

# `toShape` preserves identity, so it copies the vertex rather than rebuilding it.
v = Part.Vertex(1, 2, 3)
assert v.toShape().isSame(v)
assert v.toShape() == v
"#,
        );
    });
}

#[test]
fn a_vertex_compares_only_with_vertices() {
    boot();
    Python::with_gil(|py| {
        // Same assertions as upstream's `testTopoShapeVertexComparesOnlyWithVertices`.
        run(
            py,
            r#"
import Part

v = Part.Vertex(1, 2, 3)
assert not (v == (1, 2, 3))
assert v != (1, 2, 3)
assert not (v == Part.Vertex())
assert v != Part.Vertex()

for other in (Part.Vertex(0, 0, 0), 0, "vertex"):
    try:
        v < other
    except TypeError:
        pass
    else:
        raise AssertionError("ordering a vertex must raise TypeError")
"#,
        );
    });
}

#[test]
fn vertexes_round_trip_through_the_kernel() {
    boot();
    Python::with_gil(|py| {
        // A direct check that the `Part.Vertex` objects the kernel hands out are
        // the ones the copy constructor and the sequence protocol see.
        let part = PyModule::import(py, "Part").expect("import Part");
        let box_ = part
            .call_method1("makeBox", (1.0, 1.0, 1.0))
            .expect("Part.makeBox");
        let vertexes = box_.getattr("Vertexes").expect("Vertexes");
        assert_eq!(vertexes.len().unwrap(), 8);

        let first = vertexes.get_item(0).expect("the first vertex");
        let copy = part
            .call_method1("Vertex", (&first,))
            .expect("Part.Vertex(vertex)");
        assert!(
            copy.call_method1("isSame", (&first,))
                .unwrap()
                .extract::<bool>()
                .unwrap(),
            "the copy shares the kernel shape"
        );
        assert_eq!(copy.len().unwrap(), 3, "a vertex is a 3-sequence");
        let values = copy
            .extract::<Vec<f64>>()
            .expect("a vertex extracts as a 3-sequence");
        assert_eq!(values.len(), 3);
    });
}
