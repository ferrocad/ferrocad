//! Shape-level modelling operations on `Part.Shape`: offset, thickness,
//! fillet/chamfer, clean, section, slice and General Fuse.
//!
//! Same boot as `part_unified`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

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

/// Run a Python snippet against the booted `Part` module, reporting the Python
/// traceback when it fails.
fn run(py: Python<'_>, source: &str) {
    let code = CString::new(source).expect("the snippet has no NUL");
    let globals = PyDict::new(py);
    if let Err(error) = py.run(&code, Some(&globals), None) {
        error.print(py);
        panic!("the shape-operation snippet failed:\n{source}");
    }
}

#[test]
fn a_face_offsets_in_its_own_plane() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(1, 2, 2)
face = box.Faces[0]

offset = face.makeOffset(0.5)
assert not offset.isNull(), "makeOffset returned a null shape"
assert offset.ShapeType == "Face", f"makeOffset returned a {offset.ShapeType}"
assert offset.Area > face.Area, "an outward offset must grow the face"
assert len(offset.Edges) == 8, "the rounded corners add four edges"
assert (
    offset.BoundBox.XLength > face.BoundBox.XLength
    or offset.BoundBox.YLength > face.BoundBox.YLength
    or offset.BoundBox.ZLength > face.BoundBox.ZLength
), "the offset must change the bounds"

offset2d = face.makeOffset2D(0.5)
assert not offset2d.isNull(), "makeOffset2D returned a null shape"
assert offset2d.Area > face.Area
assert len(offset2d.Edges) == 8

# A wire offsets too (upstream's `makeOffset2D` accepts a wire).
wire = face.OuterWire
offset_wire = wire.makeOffset2D(0.5)
assert not offset_wire.isNull()
assert offset_wire.ShapeType in ("Wire", "Compound")
"#,
        );
    });
}

#[test]
fn make_thickness_hollows_a_solid() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(1, 2, 2)
thickness = box.makeThickness(box.Faces[0:2], 0.1, 0.0001)
assert not thickness.isNull(), "makeThickness returned a null shape"
assert thickness.Volume > 0.0, f"a hollowed box must have volume, got {thickness.Volume}"
assert thickness.Volume < box.Volume, "the wall is thinner than the solid"

# Removing two faces opens the shell, so the result is a solid shell, not the
# original box.
assert len(thickness.Faces) > len(box.Faces)
"#,
        );
    });
}

#[test]
fn fillet_and_chamfer_round_a_box_edge() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(1, 2, 2)

fillet = box.makeFillet(0.1, box.Faces[0].Edges)
assert not fillet.isNull(), "makeFillet returned a null shape"
assert 0.0 < fillet.Volume < box.Volume, f"unexpected fillet volume {fillet.Volume}"
assert len(fillet.Faces) > len(box.Faces), "the rounds add faces"

chamfer = box.makeChamfer(0.1, box.Faces[0].Edges)
assert not chamfer.isNull(), "makeChamfer returned a null shape"
assert 0.0 < chamfer.Volume < box.Volume, f"unexpected chamfer volume {chamfer.Volume}"
assert len(chamfer.Faces) > len(box.Faces), "the bevels add faces"

# A single edge (not a list) is accepted, as upstream's getPyShapes does.
single = box.makeChamfer(0.1, box.Faces[0].Edges[0])
assert not single.isNull()

# Degenerate arguments are rejected instead of reaching OCCT.
for bad in (0.0, -1.0):
    try:
        box.makeFillet(bad, box.Faces[0].Edges)
    except Exception:
        pass
    else:
        raise AssertionError(f"makeFillet({bad}) should be rejected")
    try:
        box.makeChamfer(bad, box.Faces[0].Edges)
    except Exception:
        pass
    else:
        raise AssertionError(f"makeChamfer({bad}) should be rejected")

try:
    box.makeFillet(0.1, [])
except Exception:
    pass
else:
    raise AssertionError("makeFillet with no edges should be rejected")
"#,
        );
    });
}

#[test]
fn remove_splitter_unifies_a_fused_pair() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box1 = Part.makeBox(1, 2, 2)
box2 = Part.makeBox(2, 1, 2)
fused = box1.fuse(box2)
removed = fused.removeSplitter()

assert not removed.isNull(), "removeSplitter returned a null shape"
assert len(removed.Faces) <= len(fused.Faces), (
    f"removeSplitter added faces: {len(fused.Faces)} -> {len(removed.Faces)}"
)
assert abs(removed.Volume - fused.Volume) < 1e-6, "unification must not change volume"
assert not removed.isSame(fused), "removeSplitter must rebuild the shape"
"#,
        );
    });
}

#[test]
fn section_slice_and_slices() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
import ferrocad as App

box1 = Part.makeBox(1, 2, 2)
box2 = Part.makeBox(2, 1, 2)

section = box1.Faces[0].section(box2.Faces[3])
assert not section.isNull(), "section returned a null shape"

# Two overlapping solids meet along a curve.
solid_section = box1.section(box2)
assert not solid_section.isNull()
assert len(solid_section.Edges) > 0, "the solids must meet along edges"

wires = box1.slice(App.Vector(10, 10, 0), 1)
assert len(wires) >= 1, "slice returned no wires"
assert wires[0].ShapeType == "Wire"

slices = box1.slices(App.Vector(10, 10, 0), [0.2, 0.4])
assert not slices.isNull(), "slices returned a null shape"
assert len(slices.Wires) >= 2, f"expected sliced wires, got {len(slices.Wires)}"
"#,
        );
    });
}

#[test]
fn general_fuse_returns_the_result_and_split_map() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box1 = Part.makeBox(1, 2, 2)
box2 = Part.makeBox(2, 1, 2)

result, split = box1.generalFuse([box2])
assert len(split) == 2, f"one split list per input, got {len(split)}"
assert not result.isNull()
assert len(result.Solids) > 0, "General Fuse returns the split solids"
assert all(len(parts) > 0 for parts in split), "both boxes are split by the other"
assert all(not part.isNull() for parts in split for part in parts)
"#,
        );
    });
}

#[test]
fn optimal_bounds_and_sub_shape_lookup() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(1, 1, 1)

optimal = box.optimalBoundingBox(True, False)
assert optimal.isValid(), "AddOptimal must produce a box"
assert abs(optimal.XLength - 1.0) < 1e-9
assert abs(optimal.YLength - 1.0) < 1e-9
assert abs(optimal.ZLength - 1.0) < 1e-9

face = box.Faces[0]
assert box.Orientation == "Forward"
assert box.findSubShape(face) == ("Face", 1), box.findSubShape(face)
vertex = box.Vertexes[2]
assert box.findSubShape(vertex) == ("Vertex", 3)
assert box.findSubShape([vertex]) == [("Vertex", 3)]
assert box.findSubShape(Part.Vertex(5, 5, 5)) == (None, 0)

assert box.getElement("Face2").isSame(box.Faces[1])

# Upstream's `testTopoShapePart`: child shapes by name, and the faces that share
# this face's vertices (a box face's vertex set is unique, so only itself).
children = box.getChildShapes("Solid1")
assert len(children) == 1 and children[0].isSame(box.Solids[0])
shared = box.findSubShapesWithSharedVertex(box.Faces[1])
assert len(shared) == 1, len(shared)
assert shared[0].isSame(box.Faces[1])
named = box.findSubShapesWithSharedVertex(box.Faces[1], needName=True)
assert named[0][0] == "Face2", named

before = face.Orientation
reversed_face = face.reversed()
assert reversed_face.Orientation != before, (before, reversed_face.Orientation)
assert reversed_face.isSame(face), "IsSame ignores the orientation"
face.reverse()
assert face.Orientation != before
face.reverse()
assert face.Orientation == before

# A reversed copy still resolves to the same sub-shape (IsSame).
assert box.findSubShape(reversed_face) == ("Face", 1)
"#,
        );
    });
}
