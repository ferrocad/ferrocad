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

/// `Edge.Orientation` must survive the shape that consumes the edge.
///
/// A `Part.Edge` carries the shape it was built from beside its base `PartShape`,
/// and `Part.Wire`/`Part.Face`/`replaceShape` read that cached entry. Setting the
/// flag only on the base left an edge that *reported* `Internal` but reached a new
/// wire as `Forward`, so an internal spur could never be built — or erased, which is
/// what SimpleCAD's `clean_solid_faces`/`erase_elements` do.
#[test]
fn an_edge_orientation_reaches_the_shapes_that_consume_it() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(20, 20, 10)
top = [f for f in box.Faces if abs(f.CenterOfMass.z - 10.0) < 1e-4][0]
spur = Part.makeLine(Part.Vertex(5, 5, 10), Part.Vertex(15, 15, 10))
spur.Orientation = "Internal"
assert spur.Orientation == "Internal"

wire = Part.Wire([spur])
assert wire.Edges[0].Orientation == "Internal", wire.Edges[0].Orientation

face = Part.Face(top.Surface, [top.Wires[0], wire])
assert len(face.Wires) == 2
internal = [e for e in face.Edges if e.Orientation == "Internal"]
assert len(internal) == 1, [e.Orientation for e in face.Edges]

# The spur is not part of the boundary: replacing the top face keeps twelve
# boundary edges plus the one internal edge.
replaced = box.replaceShape([(top, face)])
assert replaced.ShapeType == "Solid"
assert len(replaced.Edges) == 13, len(replaced.Edges)
assert len([e for f in replaced.Faces for e in f.Edges if e.Orientation == "Internal"]) == 1

# Erasing the spur leaves the box with its own edges and its own volume.
cleaned = replaced.removeShape(internal)
assert cleaned.ShapeType == "Solid", cleaned.ShapeType
assert cleaned.isValid()
assert len(cleaned.Faces) == 6, len(cleaned.Faces)
assert len(cleaned.Edges) == 12, len(cleaned.Edges)
assert abs(cleaned.Volume - box.Volume) < 1e-6, cleaned.Volume
assert not [e for f in cleaned.Faces for e in f.Edges if e.Orientation == "Internal"]
"#,
        );
    });
}

/// `Shape.fuse`/`cut`/`common` pass their `tolerance` to the kernel.
///
/// Upstream's `makeElementBoolean` treats `tol` as a signed fuzzy value: `0` leaves the
/// kernel's confusion tolerance, a positive value is used as given, and a negative one
/// asks for the automatic value. The seam used to drop the argument, so a caller could
/// not ask for a tolerant boolean at all.
#[test]
fn a_boolean_passes_its_tolerance_to_the_kernel() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

a = Part.makeBox(10, 10, 10)
b = Part.makeBox(10, 10, 10, App.Vector(10.0 + 1e-4, 0.0, 0.0))

# The 1e-4 gap is wider than the confusion tolerance: two bodies stay two.
assert len(a.fuse(b).Solids) == 2, len(a.fuse(b).Solids)
assert len(a.fuse(b, 0.0).Solids) == 2

# A fuzzy value wider than the gap bridges it.
assert len(a.fuse(b, 1e-2).Solids) == 1, len(a.fuse(b, 1e-2).Solids)
assert len(a.fuse(b, tolerance=1e-2).Solids) == 1
assert len(a.multiFuse([b], 1e-2).Solids) == 1

# `cut` and `common` take the same argument.
touching = Part.makeBox(10, 10, 10, App.Vector(5.0, 0.0, 0.0))
assert abs(a.cut(touching, 0.0).Volume - 500.0) < 1e-6, a.cut(touching, 0.0).Volume
assert abs(a.common(touching, 0.0).Volume - 500.0) < 1e-6
"#,
        );
    });
}

/// An edge's `Placement` reaches every consumer, not only the edge itself.
///
/// A `Part.Edge` keeps the shape it was built from in its cached entry beside the
/// baked base. `Placement` re-bakes the base; the entry used to keep the old
/// geometry, so `Part.Wire([edge])` rebuilt the edge at its pre-transform place.
/// SimpleCAD's rotate-edges path does exactly that (`e.copy(); e.Placement = …;
/// Part.Wire(...)`), and its crescent landed 180° from where it was dropped.
#[test]
fn an_edge_placement_reaches_the_shapes_that_consume_it() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

box = Part.makeBox(10, 10, 10)
edge = box.Edges[0]
turn = App.Placement(
    App.Vector(5, 5, 0),
    App.Rotation(App.Vector(0, 0, 1), 90),
    App.Vector(5, 5, 0),
)
edge.Placement = turn.multiply(edge.Placement)

def points(shape):
    return sorted(
        (round(v.Point.x, 6), round(v.Point.y, 6), round(v.Point.z, 6))
        for v in shape.Vertexes
    )

moved = [(15.0, 5.0, 0.0), (15.0, 5.0, 10.0)]
assert points(edge) == moved, points(edge)

# The wire built from the live edge sees the same geometry as the edge.
assert points(Part.Wire([edge])) == moved, points(Part.Wire([edge]))
assert points(Part.Wire([edge]).Edges[0]) == moved
assert points(Part.Wire(edge)) == moved

# `Edge.Curve` follows too: a moved circle reports its moved centre.
circle = Part.makeCircle(5, App.Vector(0, 0, 0), App.Vector(0, 0, 1))
circle.Placement = App.Placement(App.Vector(10, 0, 0), App.Rotation())
assert abs(circle.Curve.Center.x - 10.0) < 1e-6, circle.Curve.Center
"#,
        );
    });
}

/// A wire wrapped from a kernel shape counts its edges.
///
/// `Part.Wire([...])` carries the analytic entries it was built from, but a wire
/// reached through a shape (`Face.OuterWire`, or the `Shape` a `Part::Feature`
/// hands to Python) is only a B-Rep wrapper. The cache used to be the only source
/// for `__len__`, so such a wire reported `len() == 0` and — because Python derives
/// truthiness from `__len__` — was **falsy** despite having edges.
#[test]
fn a_wire_wrapped_from_a_shape_counts_its_edges() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(10, 10, 10)
outer = box.Faces[0].OuterWire
assert len(outer) == len(outer.Edges) == 4, (len(outer), len(outer.Edges))
assert bool(outer), "a four-edge wire must be truthy"
assert len(list(outer)) == 4, list(outer)

# The sequence and the property describe the same edges.
from_wire = [round(e.Length, 6) for e in outer]
from_edges = [round(e.Length, 6) for e in outer.Edges]
assert from_wire == from_edges, (from_wire, from_edges)

# A wire built with its own entries still reports them.
built = Part.Wire([Part.makeLine(Part.Vertex(0, 0, 0), Part.Vertex(1, 0, 0))])
assert len(built) == 1 and bool(built)
"#,
        );
    });
}

/// `Face.addWire` attaches a boundary as given (`BRep_Builder::Add`).
///
/// Upstream adds the wire without reversing or testing containment, so the caller
/// reverses a hole wire; the face's area then subtracts it. SimpleCAD's concentric
/// offset rebuilds a face one boundary at a time this way.
#[test]
fn a_face_accepts_another_boundary_wire() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math

import ferrocad as App
import Part

face = Part.Face(Part.makeCircle(10, App.Vector(0, 0, 0), App.Vector(0, 0, 1)))
assert len(face.Wires) == 1
assert abs(face.Area - math.pi * 100.0) < 1e-6, face.Area

hole = Part.Wire([Part.makeCircle(4, App.Vector(0, 0, 0), App.Vector(0, 0, 1))])
hole.reverse()
face.addWire(hole)

assert len(face.Wires) == 2, len(face.Wires)
assert face.isValid()
assert abs(face.Area - math.pi * (100.0 - 16.0)) < 1e-6, face.Area

# Upstream's signature takes a `Part.Wire`, not a bare edge.
try:
    face.addWire(Part.makeCircle(2, App.Vector(0, 0, 0), App.Vector(0, 0, 1)))
except TypeError:
    pass
else:
    raise AssertionError("Face.addWire accepted a Part.Edge")
"#,
        );
    });
}

/// A circle's analytic X axis is its edge's parameter 0.
///
/// `Part.makeCircle(..., normal)` builds the edge from the analytic circle. The
/// seam used to let `gp_Ax2` choose the X direction from the normal alone, so a
/// `-Z` circle's edge started half a turn from the `XAxis` the same circle
/// reported. Sewing two rims that should coincide then split the seam into two
/// arcs (`SimpleCAD`'s boss rebuild counted three circles instead of two).
#[test]
fn a_circles_x_axis_is_its_edges_parameter_zero() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

center = App.Vector(10, 10, 5)
for normal in ((0, 0, 1), (0, 0, -1), (0, 1, 0), (1, 0, 0)):
    circle = Part.Circle(center, App.Vector(*normal), 4)
    edge = circle.toShape()
    assert len(edge.Vertexes) == 1, (normal, len(edge.Vertexes))
    seam = edge.Vertexes[0].Point
    at_zero = circle.value(0)
    assert (seam - at_zero).Length < 1e-9, (normal, tuple(seam), tuple(at_zero))

    # The reported frame and the edge agree, so a caller can rely on XAxis.
    frame_point = center + circle.XAxis * circle.Radius
    assert (seam - frame_point).Length < 1e-6, (normal, tuple(seam), tuple(frame_point))

    # A rotated frame moves the seam with it.
    in_plane = App.Vector(1, 0, 0) if abs(normal[1]) > 0.5 else App.Vector(0, 1, 0)
    circle.XAxis = in_plane
    moved = circle.toShape()
    assert (moved.Vertexes[0].Point - circle.value(0)).Length < 1e-9
"#,
        );
    });
}

/// `Face.cutHoles` takes a list of boundary wires, including a reversed `Shape`.
///
/// Upstream is `BRepBuilderAPI_MakeFace(face).Add(wire)` for each item, and it
/// accepts any `Part.Shape` whose type is a wire — so `Wire.reversed()`, which
/// returns a plain `Shape`, is valid input.
#[test]
fn a_face_cuts_hole_wires() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math

import ferrocad as App
import Part

face = Part.Face(Part.Wire([Part.makeCircle(10, App.Vector(0, 0, 0), App.Vector(0, 0, 1))]))
hole = Part.Wire([Part.makeCircle(4, App.Vector(0, 0, 0), App.Vector(0, 0, 1))])
face.cutHoles([hole.reversed()])          # reversed() is a Part.Shape, not a Part.Wire
assert len(face.Wires) == 2, len(face.Wires)
assert face.isValid()
assert abs(face.Area - math.pi * (100.0 - 16.0)) < 1e-6, face.Area

for bad in ([Part.makeCircle(2)], [Part.Vertex(0, 0, 0)], "wire"):
    try:
        Part.Face(Part.Wire([Part.makeCircle(10)])).cutHoles(bad)
    except Exception:
        pass
    else:
        raise AssertionError("cutHoles accepted " + repr(bad))
"#,
        );
    });
}

/// The cylinder surface's upstream constructors.
///
/// `Part.Cylinder(circle)` takes its radius, centre, axis and `u = 0` direction
/// from the circle (SimpleCAD builds a fitted cylindrical face this way);
/// `Part.Cylinder(other)` copies.
#[test]
fn a_cylinder_can_be_built_from_a_circle() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

circle = Part.Circle(App.Vector(1, 2, 3), App.Vector(0, 0, -1), 5)
circle.XAxis = App.Vector(0, 1, 0)
cylinder = Part.Cylinder(circle)
assert abs(cylinder.Radius - 5) < 1e-12
assert (cylinder.Center - App.Vector(1, 2, 3)).Length < 1e-12
assert (cylinder.Axis - App.Vector(0, 0, -1)).Length < 1e-12

copy = Part.Cylinder(cylinder)
assert abs(copy.Radius - 5) < 1e-12
assert (copy.Axis - cylinder.Axis).Length < 1e-12

assert abs(Part.Cylinder().Radius - 1) < 1e-12
assert abs(Part.Cylinder(2.5).Radius - 2.5) < 1e-12

# The surface frames a face on the circle's own plane.
face = Part.Face(cylinder, Part.Wire([Part.makeCircle(5, App.Vector(1, 2, 3), App.Vector(0, 0, -1))]))
assert face.ShapeType == "Face"
"#,
        );
    });
}

/// The one-argument `Shape.tessellate` uses upstream's angular deflection.
///
/// FreeCAD derives `min(0.1, linear * 5 + 0.005)` from the linear value; OCCT's
/// own 0.5 rad default produced far coarser meshes (a 6 mm bore had a 0.04 mm
/// chord sag instead of 0.008), which SimpleCAD's glue profile could see.
#[test]
fn tessellate_defaults_to_upstreams_angular_deflection() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

def facets(**kwargs):
    shape = Part.makeCylinder(6, 10)
    return len(shape.tessellate(**kwargs)[1])

default = facets(deflection=0.15)
occt_default = facets(deflection=0.15, angular=0.5)
derived = facets(deflection=0.15, angular=0.1)
assert default == derived, (default, derived)
assert default > occt_default, (default, occt_default)

# A fine linear request tightens the angle further, before the 0.1 rad cap.
finer = facets(deflection=0.01)
assert finer == facets(deflection=0.01, angular=0.055), (finer, facets(deflection=0.01, angular=0.055))
"#,
        );
    });
}

/// `Placement` and `Shape` are one transaction, and a label travels with it.
///
/// A `Part::Feature` keeps the object's placement baked into its stored shape, so
/// a `Placement` write also rewrites `Shape`. That derived half used to be written
/// raw — invisible to the undo log — so an aborted move left the body where the
/// abandoned transaction had put it, and an undo reverted the placement while the
/// geometry stayed moved. A rename had the same shape of bug in the other
/// direction: `Label` was a field `set_label` wrote directly, so an undo restored
/// the shape but not the name, and a refused operation left its result renamed.
#[test]
fn a_placement_move_undoes_its_shape_and_a_label_undoes_with_it() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

doc = App.newDocument("PlacementUndo")
doc.UndoMode = 1
obj = doc.addObject("Part::Feature", "Block")
obj.Shape = Part.makeBox(10, 20, 30)
obj.Placement = App.Placement(App.Vector(70, 30, 40), App.Rotation(App.Vector(1, 2, 3), 27))
doc.recompute()
original = obj.Shape
count = doc.UndoCount

# An aborted move leaves both halves alone and adds no undo step.
doc.openTransaction("Rotate")
obj.Placement = App.Placement(App.Vector(0, 0, 0), App.Rotation(App.Vector(1, 2, 3), 48))
doc.abortTransaction()
assert obj.Shape.isSame(original), "an aborted move must leave the geometry alone"
assert (obj.Placement.Base - App.Vector(70, 30, 40)).Length < 1e-12, obj.Placement.Base
assert doc.UndoCount == count, (doc.UndoCount, count)

# A committed move is one undo step, and undo restores the same shape handle.
doc.openTransaction("Rotate")
obj.Placement = App.Placement(App.Vector(0, 0, 0), App.Rotation(App.Vector(1, 2, 3), 48))
doc.commitTransaction()
assert doc.UndoCount == count + 1, (doc.UndoCount, count)
assert not obj.Shape.isSame(original)
doc.undo()
assert obj.Shape.isSame(original), "undo must restore the shape that was moved"
assert (obj.Placement.Base - App.Vector(70, 30, 40)).Length < 1e-12, obj.Placement.Base
doc.redo()
assert obj.Placement.Base.Length < 1e-12, obj.Placement.Base

# The name is part of the transaction that sets it.
doc.openTransaction("Rename")
obj.Label = "Union"
doc.commitTransaction()
assert obj.Label == "Union"
doc.undo()
assert obj.Label == "Block", obj.Label
doc.redo()
assert obj.Label == "Union", obj.Label

# A refused transaction puts the label back too.
before = doc.UndoCount
doc.openTransaction("Rename")
obj.Label = "Refused"
doc.abortTransaction()
assert obj.Label == "Union", obj.Label
assert doc.UndoCount == before, (doc.UndoCount, before)

App.closeDocument(doc.Name)
"#,
        );
    });
}

/// An elementary surface is a value object: it can be filled in after it exists.
///
/// Upstream's `Geom_Cylinder` exposes `Radius`, `Center` and `Axis` as readable
/// *and writable* attributes, and `GeomSurface.toShape(u1, u2, v1, v2)` turns the
/// parameter band into a face. SimpleCAD's heal builds a hole wall exactly that
/// way (`surface = Part.Cylinder(); surface.Radius = …; surface.toShape(0, 2π, …)`),
/// so a read-only surface silently kept the tilted face it was meant to square.
#[test]
fn a_cylinder_surface_is_writable_and_bands_into_a_face() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

surface = Part.Cylinder()
surface.Radius = 1.7
surface.Center = App.Vector(-5, 6, 4)
surface.Axis = App.Vector(1, 0, 0)
assert abs(surface.Radius - 1.7) < 1e-12
assert (surface.Center - App.Vector(-5, 6, 4)).Length < 1e-12
assert (surface.Axis - App.Vector(1, 0, 0)).Length < 1e-12

band = surface.toShape(0, 6.283185307179586, 0, 9)
assert band.ShapeType == "Face", band.ShapeType
assert abs(band.Area - 2 * 3.141592653589793 * 1.7 * 9) < 1e-6, band.Area
assert isinstance(band.Surface, Part.Cylinder)
assert band.isValid()

# The frame's u = 0 direction survives an axis change when it can be projected.
surface.Axis = App.Vector(0, 1, 0)
assert abs(surface.Axis.y - 1.0) < 1e-12
assert abs((surface.Axis - App.Vector(0, 1, 0)).Length) < 1e-12
quarter = surface.toShape(0, 3.141592653589793 / 2, 0, 2)
assert abs(quarter.Area - (3.141592653589793 / 2) * 1.7 * 2) < 1e-6, quarter.Area

# A degenerate request is refused, not guessed.
for bad in ((1, 0, 0, 1), (0, 1, 1, 0)):
    try:
        surface.toShape(*bad)
    except Exception:
        pass
    else:
        raise AssertionError(f"toShape{bad} must be refused")
try:
    surface.toShape()
except Exception:
    pass
else:
    raise AssertionError("a cylinder has no natural bounds")
"#,
        );
    });
}

/// A placement write moves the geometry *from its un-moved base*.
///
/// `Shape.Placement` used to compose a delta onto the already-moved coordinates, so
/// returning to the identity left a float round-trip behind (5e-15 on a 10 mm cube —
/// enough to flip the longest-side tie-break SimpleCAD's `main_axis` uses for a
/// symmetric shape) and repeated drags compounded it. Upstream gets this for free by
/// keeping a standalone shape's placement as a `TopLoc_Location`; we keep the
/// un-moved geometry beside it, so clearing or replacing the placement hands the
/// original coordinates back untouched.
#[test]
fn a_placement_reset_is_bit_exact() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

def sides(shape):
    box = shape.BoundBox
    return (box.XLength, box.YLength, box.ZLength)

box = Part.makeBox(10, 10, 10)
assert sides(box) == (10.0, 10.0, 10.0), sides(box)

# A rotated copy returns to its un-moved geometry exactly.
box.Placement = App.Placement(App.Vector(80, -20, 30), App.Rotation(App.Vector(1, 2, 3), 27))
copy = box.copy()
copy.Placement = App.Placement()
assert sides(copy) == (10.0, 10.0, 10.0), sides(copy)
assert copy.Placement.Base.Length == 0.0, copy.Placement.Base

# And repeated drags transform from the base, not from the last result.
moving = Part.makeBox(10, 10, 10)
back = App.Vector(-1, -2, -3)
for _ in range(40):
    moving.translate(App.Vector(1, 2, 3))
    moving.translate(back)
assert sides(moving) == (10.0, 10.0, 10.0), sides(moving)
assert moving.Placement.Base.Length == 0.0, moving.Placement.Base
"#,
        );
    });
}

/// A rigid `Placement` write keeps the shape's native identity.
///
/// Upstream stores a standalone shape's placement as a `TopLoc_Location`, so a move
/// replaces no sub-shape handle: `isPartner` still recognises the moved shape as the
/// one it came from, and a style map (or a native cache) keyed on identity keeps
/// following it. Re-baking the coordinates made every handle new, which is why
/// SimpleCAD's material mapping lost the styles of a moved object.
#[test]
fn a_rigid_move_keeps_the_shapes_identity() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part

source = Part.makeBox(10, 10, 10)
moved = Part.Shape(source)
assert moved.isPartner(source)

moved.Placement = App.Placement(App.Vector(12, 25, 37), App.Rotation(App.Vector(1, 2, 3), 41))
assert moved.isPartner(source), "a rigid move must not replace a single handle"
assert not moved.isSame(source), "the location changed, so it is not the same shape"
assert abs(moved.Volume - 1000.0) < 1e-9, moved.Volume

# Clearing the placement is the original shape again, not merely an equal one.
moved.Placement = App.Placement()
assert moved.isSame(source)
assert moved.BoundBox.XLength == 10.0

# A translation is a location change too.
base = Part.makeBox(10, 10, 10)
translated = Part.Shape(base)
translated.translate(App.Vector(1, 2, 3))
assert translated.isPartner(base)
assert abs(translated.Volume - 1000.0) < 1e-9, translated.Volume

# A copy is not a partner: `copy()` is `BRepBuilderAPI_Copy`.
assert not source.copy().isPartner(source)
"#,
        );
    });
}
