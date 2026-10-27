//! Shape identity, copying and the document `Shape`/`Placement` pair.
//!
//! These pin the semantics SimpleCAD's caches rely on:
//!
//! - `Shape.copy()` is a deep copy (`BRepBuilderAPI_Copy`): equal volume/area,
//!   but neither `isSame` nor `isEqual` and a different hash;
//! - `==` is `isEqual` (orientation included) while `hash` follows `isSame`
//!   (orientation ignored), so a hash bucket plus `isSame` finds one sub-shape;
//! - a document object's `Placement` moves its stored `Shape` without a
//!   recompute, and undo moves it back.
//!
//! Same boot as the other Part tests: `ferrocad` + `Part` as built-ins, one core,
//! one OCCT backend. Needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::PyDict;

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

/// Run a Python snippet against the booted modules, reporting the traceback when
/// it fails.
fn run(py: Python<'_>, source: &str) {
    let code = CString::new(source).expect("the snippet has no NUL");
    let globals = PyDict::new(py);
    if let Err(error) = py.run(&code, Some(&globals), None) {
        error.print(py);
        panic!("the identity snippet failed:\n{source}");
    }
}

#[test]
fn copy_is_a_deep_copy_with_the_same_geometry() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
import ferrocad as App

box = Part.makeBox(1, 2, 3)
box.Placement.Base = App.Vector(5, 0, 0)

copy = box.copy()
assert abs(copy.Volume - box.Volume) < 1e-9, (copy.Volume, box.Volume)
assert abs(copy.Area - box.Area) < 1e-9
assert not copy.isSame(box), "a deep copy has a new TShape"
assert not copy.isEqual(box)
assert copy != box
assert hash(copy) != hash(box), "a different TShape hashes differently"

# The accumulated placement travels with the copy, so the geometry is placed.
assert abs(copy.Placement.Base.x - 5) < 1e-12
assert abs(copy.BoundBox.XMin - 5) < 1e-9

# A null shape copies to a null shape instead of failing.
assert Part.Shape().copy().isNull()
"#,
        );
    });
}

#[test]
fn equality_is_is_equal_and_hash_follows_is_same() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 2, 2)

# The copy constructor shares the kernel handle: same identity and same hash.
wrapped = Part.Shape(box)
assert wrapped == box and box == wrapped
assert wrapped.isEqual(box) and wrapped.isSame(box)
assert hash(wrapped) == hash(box)
assert len({wrapped, box}) == 1

# A reversed shape is the same sub-shape (same TShape + location) but not equal.
reversed_box = box.reversed()
assert reversed_box.isSame(box)
assert hash(reversed_box) == hash(box), "orientation is not part of isSame"
assert reversed_box != box
assert not reversed_box.isEqual(box)

# A deep copy is a different TShape: neither same, equal, nor the same hash.
copy = box.copy()
assert copy != box and hash(copy) != hash(box)

# Null shapes are never equal, but comparing with a non-shape is False, not an
# exception (`NotImplemented` falls back to identity).
assert not (Part.Shape() == Part.Shape())
assert Part.Shape() != Part.Shape()
assert not (Part.Shape() == box)

# Sub-shape wrappers around one kernel element hash and compare alike, which is
# the `bucket = lookup.setdefault(hash(edge), [])` pattern.
edge = box.Edges[0]
same_edge = box.Edges[0]
assert hash(edge) == hash(same_edge)
assert edge == same_edge
assert edge in {same_edge: "found"}
"#,
        );
    });
}

#[test]
fn placement_copies_another_placement() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App

p = App.Placement()
p.Base = App.Vector(1, 2, 3)
p.Rotation.Axis = App.Vector(0, 0, 1)
p.Rotation.Angle = 0.5

q = App.Placement(p)
assert q is not p
assert (q.Base.x, q.Base.y, q.Base.z) == (1, 2, 3)
assert abs(q.Rotation.Angle - 0.5) < 1e-12
assert q.isSame(p, 1e-12)

# The copy is detached: mutating it leaves the source alone.
q.Base.x = 9
assert p.Base.x == 1
"#,
        );
    });
}

#[test]
fn bound_box_diagonal_length_covers_empty_point_box_and_rotated() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
import ferrocad as App

empty = App.BoundBox()
assert empty.DiagonalLength == 0
assert empty.XLength == 0 and empty.YLength == 0 and empty.ZLength == 0
center = empty.Center
assert (center.x, center.y, center.z) == (0, 0, 0)

point = App.BoundBox(1, 2, 3, 1, 2, 3)
assert point.DiagonalLength == 0

box = App.BoundBox(0, 0, 0, 3, 4, 12)
assert abs(box.DiagonalLength - 13) < 1e-12
center = box.Center
assert (center.x, center.y, center.z) == (1.5, 2, 6)

rotated = Part.makeBox(2, 2, 2)
rotated.Placement.Rotation.Angle = math.radians(45)
bounds = rotated.BoundBox
expected = math.sqrt(bounds.XLength ** 2 + bounds.YLength ** 2 + bounds.ZLength ** 2)
assert abs(bounds.DiagonalLength - expected) < 1e-9
assert abs(bounds.DiagonalLength - math.sqrt(20)) < 1e-9
"#,
        );
    });
}

#[test]
fn ordered_vertexes_follow_a_box_wire_and_an_open_polyline() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
import ferrocad as App

# A box face's outer wire is read from the B-Rep (no analytic entries).
wire = Part.makeBox(2, 2, 2).Faces[0].OuterWire
ordered = wire.OrderedVertexes
assert len(ordered) == 4, len(ordered)
points = [tuple(v.Point) for v in ordered]
assert len(set(points)) == 4, points
# Consecutive corners are an edge apart, including the closing pair (length 2).
for a, b in zip(points, points[1:] + points[:1]):
    assert abs(sum((x - y) ** 2 for x, y in zip(a, b)) - 4) < 1e-9, (a, b)
# The same vertices as `Vertexes`, just ordered.
assert {tuple(v.Point) for v in wire.Vertexes} == set(points)

# An open polygon reports its vertices end to end.
polyline = Part.makePolygon(
    [App.Vector(0, 0, 0), App.Vector(1, 0, 0), App.Vector(1, 1, 0)], False)
assert [tuple(v.Point) for v in polyline.OrderedVertexes] == [
    (0, 0, 0), (1, 0, 0), (1, 1, 0)]
"#,
        );
    });
}

#[test]
fn ancestors_and_elements_use_their_python_subclass() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 2, 2)
edge = box.Edges[0]
faces = box.ancestorsOfType(edge, Part.Face)
assert len(faces) == 2, len(faces)
assert all(isinstance(face, Part.Face) for face in faces), faces

# `getElement` also returns the typed wrapper.
assert isinstance(box.getElement("Edge1"), Part.Edge)
assert isinstance(box.getElement("Face1"), Part.Face)
assert isinstance(box.getElement("Vertex1"), Part.Vertex)
"#,
        );
    });
}

#[test]
fn a_document_shape_is_independent_of_mutating_the_source_wrapper() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
import ferrocad as App

doc = App.newDocument("IdentityUndo")
try:
    obj = doc.addObject("Part::Feature", "Feature")
    box = Part.makeBox(2, 3, 4)
    obj.Shape = box
    volume = obj.Shape.Volume

    # Mutating the source wrapper replaces its own handle; the stored shape is
    # untouched.
    box.Placement.Base = App.Vector(10, 0, 0)
    box.reverse()
    assert abs(obj.Shape.BoundBox.XMin) < 1e-9
    assert abs(obj.Shape.Volume - volume) < 1e-9

    # A view read back from the document is also detached.
    view = obj.Shape
    view.Placement.Base = App.Vector(100, 0, 0)
    view.reverse()
    assert abs(obj.Shape.BoundBox.XMin) < 1e-9

    # Moving the object bakes the placement into the stored shape immediately.
    doc.openTransaction("Move")
    obj.Placement = App.Placement(App.Vector(5, 0, 0), App.Rotation())
    doc.commitTransaction()
    assert abs(obj.Shape.BoundBox.XMin - 5) < 1e-9

    # Undo restores both the object placement and the shape it baked.
    assert doc.undo()
    assert abs(obj.Shape.BoundBox.XMin) < 1e-9
    assert abs(obj.Shape.Volume - volume) < 1e-9
finally:
    App.closeDocument("IdentityUndo")
"#,
        );
    });
}
