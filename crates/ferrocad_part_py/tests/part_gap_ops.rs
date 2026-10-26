//! The measured Part gaps: sewing, NURBS conversion, surface-bounded faces,
//! edge sampling, shell free edges, surface interpolation, exact compound
//! enumeration and cone/torus surface typing.
//!
//! Same boot as `part_shape_ops`: `ferrocad` + `Part` as built-ins, one core, one
//! OCCT backend. It needs OCCT (see `docs/architecture/occt-bundling.md`).

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
        panic!("the Part gap snippet failed:\n{source}");
    }
}

#[test]
fn sew_shape_joins_adjacent_faces_into_a_shell() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

def square(x0, x1):
    return Part.Face(Part.makePolygon([
        V(x0, 0, 0), V(x1, 0, 0), V(x1, 10, 0), V(x0, 10, 0), V(x0, 0, 0),
    ]))

sewn = Part.makeCompound([square(0, 10), square(10, 20)]).copy()
assert len(sewn.Shells) == 0, "a loose compound of faces has no shell"
assert sewn.sewShape() is None, "sewShape mutates in place and returns None"

shells = sewn.Shells
assert len(shells) == 1, f"sewing the shared edge gives one shell, got {len(shells)}"
assert len(shells[0].Faces) == 2
assert not shells[0].isNull()
"#,
        );
    });
}

#[test]
fn to_nurbs_rewrites_the_surface_as_a_b_spline() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

cylinder = Part.makeCylinder(2, 5)
lateral = [f for f in cylinder.Faces if f.Surface.TypeId == "Part::GeomCylinder"][0]
nurbs = lateral.toNurbs()

assert nurbs.ShapeType == "Face", f"toNurbs returned a {nurbs.ShapeType}"
assert nurbs.Faces[0].Surface.TypeId == "Part::GeomBSplineSurface", (
    f"the converted surface is {nurbs.Faces[0].Surface.TypeId}"
)
assert abs(nurbs.Area - lateral.Area) < 0.01 * lateral.Area, (
    f"{nurbs.Area} != {lateral.Area}"
)

box = Part.makeBox(2, 3, 4)
box_nurbs = box.toNurbs()
assert box_nurbs.ShapeType == "Solid"
assert abs(box_nurbs.Volume - box.Volume) < 1e-6
"#,
        );
    });
}

#[test]
fn a_face_can_be_built_on_an_elementary_surface() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# The planar form, with a second boundary wire.
box = Part.makeBox(20, 20, 10)
faces = list(box.Faces)
assert len(faces) == 6, f"{len(faces)} faces"
top = max(faces, key=lambda f: (f.BoundBox.ZMin, f.BoundBox.ZMax))
assert abs(top.BoundBox.ZMin - 10) < 1e-6 and abs(top.BoundBox.ZMax - 10) < 1e-6, (
    f"the selected face spans z {top.BoundBox.ZMin}..{top.BoundBox.ZMax}"
)
hole_wire = Part.Wire(Part.makePolygon([
    V(5, 5, 10), V(15, 5, 10), V(15, 15, 10), V(5, 15, 10), V(5, 5, 10),
]))
plain = Part.Face(top.Surface, [top.OuterWire])
assert abs(plain.Area - 400.0) < 1e-6, f"plain {plain.Area}, top {top.Area}"
unreversed = Part.Face(top.Surface, [top.OuterWire, hole_wire])
# OCCT's `Add` does not re-orient a boundary: a same-oriented wire adds area.
assert abs(unreversed.Area - 500.0) < 1e-6, f"unreversed {unreversed.Area}"
hole_wire.reverse()
with_hole = Part.Face(top.Surface, [top.OuterWire, hole_wire])
assert with_hole.Surface.TypeId == "Part::GeomPlane"
assert abs(with_hole.Area - (400.0 - 100.0)) < 1e-6, (
    f"the hole was not subtracted: Area = {with_hole.Area}"
)

# An open wire cannot be added as a hole: OCCT closes it against the outer
# boundary instead (upstream relies on Internal-oriented edges for that case,
# and `Shape.Orientation` is not writable here yet). The hole form above is the
# supported constructor use.

# A non-periodic cylindrical patch: an arc swept along +Z.
arc = Part.makeCircle(5, V(0, 0, 0), V(0, 0, 1), 10, 80)
patch = arc.extrude(V(0, 0, 10))
patch_face = patch.Faces[0]
assert patch_face.Surface.TypeId == "Part::GeomCylinder"
rebuilt = Part.Face(patch_face.Surface, [patch_face.OuterWire])
assert rebuilt.ShapeType == "Face", f"got a {rebuilt.ShapeType}"
assert rebuilt.Surface.TypeId == "Part::GeomCylinder"
assert abs(rebuilt.Area - patch_face.Area) < 1e-6, (
    f"{rebuilt.Area} != {patch_face.Area}; patch {patch_face.ParameterRange}, "
    f"rebuilt {rebuilt.ParameterRange}"
)

# The existing constructor still accepts a face-maker name.
unified = Part.Face(Part.makePolygon([
    V(0, 0, 0), V(1, 0, 0), V(1, 1, 0), V(0, 0, 0),
]), "Part::FaceMakerUnified")
assert unified.ShapeType == "Face"
"#,
        );
    });
}

#[test]
fn edge_discretize_samples_by_number_distance_and_tangency() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

circle = Part.makeCircle(5, V(0, 0, 0), V(0, 0, 1))

points = circle.discretize(8)
assert len(points) == 8, f"Number=8 gave {len(points)} points"
for point in points:
    assert abs(math.hypot(point.x, point.y) - 5.0) < 1e-9

points = circle.discretize(2.0)
assert len(points) > 4, f"Distance=2 gave {len(points)} points"
for point in points:
    assert abs(math.hypot(point.x, point.y) - 5.0) < 1e-9

points = circle.discretize(Angular=0.1, Curvature=0.1, Minimum=2)
assert len(points) >= 2, "the tangential sampler needs at least two points"

trimmed = circle.discretize(Number=3, First=0.0, Last=math.pi)
assert len(trimmed) == 3
assert abs(trimmed[0].x - 5.0) < 1e-9, "the first point is at parameter 0"
assert abs(trimmed[-1].x + 5.0) < 1e-9, "the last point is at parameter pi"

points = circle.discretize(Deflection=0.01)
assert len(points) > 8, "the deflection sampler adapts to the curvature"
"#,
        );
    });
}

#[test]
fn get_parameter_by_length_walks_the_edge() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

line = Part.makeLine(V(0, 0, 0), V(10, 0, 0))
assert abs(line.getParameterByLength(5.0) - 5.0) < 1e-9
assert abs(line.getParameterByLength(-2.5) - 7.5) < 1e-9

circle = Part.makeCircle(5, V(0, 0, 0), V(0, 0, 1))
middle = circle.valueAt(circle.getParameterByLength(circle.Length / 2.0))
assert abs(middle.x + 5.0) < 1e-6 and abs(middle.y) < 1e-6, (
    f"the half-length point is {middle}"
)

try:
    line.getParameterByLength(20.0)
    raise AssertionError("a length beyond the edge must raise")
except ValueError as error:
    assert "out of range" in str(error)
"#,
        );
    });
}

#[test]
fn a_shell_reports_its_free_edges() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

def square(x0, x1):
    return Part.Face(Part.makePolygon([
        V(x0, 0, 0), V(x1, 0, 0), V(x1, 10, 0), V(x0, 10, 0), V(x0, 0, 0),
    ]))

sewn = Part.makeCompound([square(0, 10), square(10, 20)]).copy()
sewn.sewShape()
shell = sewn.Shells[0]

free = shell.getFreeEdges()
assert free.ShapeType == "Compound", f"got a {free.ShapeType}"
assert len(free.Edges) == 6, (
    f"two squares sharing one edge have six free edges, got {len(free.Edges)}"
)
"#,
        );
    });
}

#[test]
fn bspline_surface_interpolates_a_grid() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

surface = Part.BSplineSurface()
surface.interpolate([
    [V(x, y, 0.1 * x * x + 0.065 * y * y) for y in (0, 5, 10, 15)]
    for x in (0, 5, 10, 15)
])

u0, u1, v0, v1 = surface.bounds()
assert u1 > u0 and v1 > v0, f"degenerate bounds {(u0, u1, v0, v1)}"
assert len(surface.UKnotSequence) >= 4, surface.UKnotSequence
assert len(surface.VKnotSequence) >= 4, surface.VKnotSequence
assert surface.UKnotSequence[0] >= u0 - 1e-12
assert surface.VKnotSequence[-1] <= v1 + 1e-12
assert not surface.isUPeriodic() and not surface.isVPeriodic()

# The regular-grid form: z values with explicit spacing.
height = Part.BSplineSurface()
height.interpolate([[x * x + y * y for y in range(4)] for x in range(4)], 0.0, 2.0, 0.0, 2.0)
u0, u1, v0, v1 = height.bounds()
assert u1 > u0 and v1 > v0

try:
    Part.BSplineSurface().interpolate([[V(0, 0, 0), V(1, 0, 0)]])
    raise AssertionError("a single row is not a surface")
except Part.OCCError:
    pass
"#,
        );
    });
}

#[test]
fn compounds_are_enumerated_exactly() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(1, 1, 1)
assert box.Compounds == [], "a solid has no compound sub-shape"
assert box.CompSolids == [], "a solid is not a comp-solid"

inner = Part.makeCompound([box])
flat = Part.makeCompound([box, Part.makeBox(1, 1, 1)])
compounds = flat.Compounds
assert len(compounds) == 1, f"got {len(compounds)} compounds"
assert compounds[0].isSame(flat), "the map starts at the shape itself"
assert inner.Compounds[0].isSame(inner)
"#,
        );
    });
}

#[test]
fn cone_and_torus_faces_expose_their_analytic_surface() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part

cone = Part.makeCone(2, 1, 4)
conical = [f for f in cone.Faces if f.Surface.TypeId == "Part::GeomCone"][0]
surface = conical.Surface
assert isinstance(surface, Part.Cone), type(surface)
assert abs(surface.Radius - 2.0) < 1e-9
assert 0.0 < abs(surface.SemiAngle) < math.pi / 2.0

u0, u1, v0, v1 = conical.ParameterRange
u, v = (u0 + u1) / 2.0, (v0 + v1) / 2.0
point = conical.valueAt(u, v)
u2, v2 = surface.parameter(point)
assert abs(v2 - v) < 1e-7, f"v {v} -> {v2}"
assert abs(u2 - u) < 1e-7 or abs(abs(u2 - u) - math.tau) < 1e-7, f"u {u} -> {u2}"
normal = conical.normalAt(u, v)
assert abs(normal.Length - 1.0) < 1e-9

torus = Part.makeTorus(5, 2)
face = torus.Faces[0]
assert face.Surface.TypeId == "Part::GeomToroid", face.Surface.TypeId
surface = face.Surface
assert isinstance(surface, Part.Toroid), type(surface)
assert abs(surface.MajorRadius - 5.0) < 1e-9
assert abs(surface.MinorRadius - 2.0) < 1e-9
assert surface.Center.Length < 1e-9
point = surface.value(0.0, 0.0)
assert abs(point.x - 7.0) < 1e-9 and abs(point.y) < 1e-9 and abs(point.z) < 1e-9

u0, u1, v0, v1 = face.ParameterRange
normal = face.normalAt((u0 + u1) / 2.0, (v0 + v1) / 2.0)
assert abs(normal.Length - 1.0) < 1e-9
"#,
        );
    });
}

#[test]
fn orientation_is_settable_and_keeps_the_geometry() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

box = Part.makeBox(2, 3, 4)
area = box.Area
assert box.Orientation == "Forward", box.Orientation

box.Orientation = "Reversed"
assert box.Orientation == "Reversed", box.Orientation
assert abs(box.Area - area) < 1e-9, "an orientation change must not change the geometry"

box.Orientation = "Forward"
assert box.Orientation == "Forward"

face = box.Faces[0]
face.Orientation = "Internal"
assert face.Orientation == "Internal", face.Orientation
face.Orientation = "Forward"
assert face.Orientation == "Forward"

try:
    box.Orientation = "Sideways"
    raise AssertionError("an unknown orientation must raise")
except Part.OCCError:
    pass
"#,
        );
    });
}

#[test]
fn bspline_surface_to_shape_builds_the_face() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

surface = Part.BSplineSurface()
surface.interpolate([
    [V(x, y, 0.1 * x * x + 0.065 * y * y) for y in (0, 5, 10, 15)]
    for x in (0, 5, 10, 15)
])
face = surface.toShape()

assert face.ShapeType == "Face", face.ShapeType
assert face.Area > 0.0, face.Area
box = face.BoundBox
assert abs(box.XMin) < 1e-6 and abs(box.XMax - 15.0) < 1e-6, (box.XMin, box.XMax)
assert abs(box.YMin) < 1e-6 and abs(box.YMax - 15.0) < 1e-6, (box.YMin, box.YMax)
assert len(face.Faces) == 1

try:
    Part.BSplineSurface().toShape()
    raise AssertionError("an empty surface has no shape")
except Part.OCCError:
    pass
"#,
        );
    });
}

#[test]
fn center_of_mass_follows_the_shape_type() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

def near(point, expected, what):
    assert abs(point.x - expected[0]) < 1e-6, (what, point)
    assert abs(point.y - expected[1]) < 1e-6, (what, point)
    assert abs(point.z - expected[2]) < 1e-6, (what, point)

face = Part.makePlane(10, 10)
near(face.CenterOfMass, (5, 5, 0), "face")
near(face.CenterOfGravity, (5, 5, 0), "face alias")

edge = Part.makeLine(V(0, 0, 0), V(10, 0, 0))
near(edge.CenterOfMass, (5, 0, 0), "edge")

solid = Part.makeBox(2, 2, 2)
near(solid.CenterOfMass, (1, 1, 1), "solid")

vertex = Part.Vertex(V(1, 2, 3))
near(vertex.CenterOfMass, (1, 2, 3), "vertex")
"#,
        );
    });
}
