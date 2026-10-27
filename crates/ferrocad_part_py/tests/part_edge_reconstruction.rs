//! The boundary-reconstruction seams SimpleCAD's surface and mesh paths hit:
//! a 2-D curve lifted onto its surface, a shell sewn from independently built
//! faces, a bull's-eye face from several loops, a compound's centre of mass, and
//! double-precision tessellation.
//!
//! Same boot as `part_gap_ops`: `ferrocad` + `Part` as built-ins, one core, one
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
        panic!("the Part edge-reconstruction snippet failed:\n{source}");
    }
}

#[test]
fn a_2d_line_lifted_onto_a_cylinder_lies_on_it() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part

shape = Part.makeCylinder(10, 20)
face = next(f for f in shape.Faces if isinstance(f.Surface, Part.Cylinder))

# `toShape(surface)` must project the 2-D line onto the surface, as upstream's
# `BRepBuilderAPI_MakeEdge(Geom2d_Curve, Geom_Surface)` does. Before this was
# fixed the edge was the 2-D image at z = 0, so it left the face entirely.
segment = Part.Geom2d.Line2dSegment((0.5, 2.0), (2.0, 15.0))
edge = segment.toShape(face.Surface)
expected = math.hypot((2.0 - 0.5) * 10.0, 15.0 - 2.0)
assert abs(edge.Length - expected) < 1e-6, f"{edge.Length} != {expected}"

# Every point of the edge is on the surface...
for step in range(9):
    u = edge.FirstParameter + (edge.LastParameter - edge.FirstParameter) * step / 8
    point = edge.valueAt(u)
    assert abs(math.hypot(point.x, point.y) - 10.0) < 1e-6, (step, point)

# ...so the exact coverage test SimpleCAD applies to a drawn line holds.
assert abs(face.common(edge).Length - edge.Length) < 1e-6, "the edge left the face"

# A 2-D segment that crosses the seam stays on the surface there: the surface
# image of u = tau - 0.2 .. tau + 0.2 is the 4 mm arc through u = 0.
seam = Part.Geom2d.Line2dSegment((math.tau - 0.2, 10.0),
                                 (math.tau + 0.2, 10.0)).toShape(face.Surface)
assert abs(seam.Length - 4.0) < 1e-6, seam.Length
assert all(vertex.Point.x > 9.0 for vertex in seam.Vertexes), [tuple(v.Point) for v in seam.Vertexes]

# A plane keeps the exact analytic edge type.
plane = Part.Plane()
flat = Part.Geom2d.Line2dSegment((0.0, 0.0), (3.0, 4.0)).toShape(plane)
assert flat.Curve.TypeId == "Part::GeomLine", flat.Curve.TypeId
assert abs(flat.Length - 5.0) < 1e-9, flat.Length
"#,
        );
    });
}

#[test]
fn a_wire_on_a_sphere_bounds_a_spherical_face() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part

# The upstream `BRep_tests.testEdgeSplitFace` wire: four 2-D segments lifted
# onto one sphere, one of them split, joined into a wire and made into a face.
coords = [(0.5, -0.5), (1.0, -0.5), (1.0, 0.5), (0.5, 0.5)]
points = list(coords)
points.append(points[0])
sphere = Part.Sphere()
edges = [Part.Geom2d.Line2dSegment(points[i - 1], points[i]).toShape(sphere)
         for i in range(1, len(points))]
split = edges[0].split(0.25)
wire = Part.Wire(split.Edges + edges[1:])
assert wire.isClosed(), "the wire is not closed"

for maker in (None, "Part::FaceMakerSimple"):
    face = Part.Face(wire, maker) if maker else Part.Face(wire)
    assert face.isValid(), maker
    assert face.Surface.TypeId == "Part::GeomSphere", (maker, face.Surface.TypeId)
    assert face.Area > 0.0, face.Area
"#,
        );
    });
}

#[test]
fn make_shell_sews_independently_built_faces() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# Six separately built square faces: they share geometry, never topology. That is
# exactly what a mesh's flat regions and SimpleCAD's planar reconstruction hand
# to `Part.makeShell`, and upstream re-sews the shell when it is not valid.
def square(points):
    return Part.Face(Part.makePolygon([V(*point) for point in points]
                                      + [V(*points[0])]))

quads = [
    [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)],
    [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)],
    [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)],
    [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)],
    [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)],
    [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
]
faces = [square([(10 * x, 10 * y, 10 * z) for x, y, z in quad]) for quad in quads]
assert all(face.isValid() for face in faces), "the source faces are valid"

shell = Part.makeShell(faces)
assert shell.isValid(), "a sewn box shell must be valid"
assert shell.isClosed(), "a sewn box shell must be closed"
assert len(shell.Faces) == 6, len(shell.Faces)
assert len(shell.Shells) == 1, len(shell.Shells)

solid = Part.makeSolid(shell)
assert solid.isValid() and solid.isClosed(), (solid.isValid(), solid.isClosed())
assert abs(solid.Volume - 1000.0) < 1e-6, solid.Volume

# An assembly that cannot be one shell keeps the sewn result rather than failing.
pair = Part.makeShell(faces[:2])
assert pair.isValid(), "two disjoint faces still sew into valid geometry"
assert not pair.isClosed()
"#,
        );
    });
}

#[test]
fn bullseye_nests_wires_and_keeps_disconnected_regions_apart() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

def square(x0, y0, x1, y1):
    return Part.makePolygon([V(x0, y0, 0), V(x1, y0, 0), V(x1, y1, 0),
                             V(x0, y1, 0), V(x0, y0, 0)])

outer, inner = square(0, 0, 10, 10), square(2, 2, 8, 8)

# A hole is a wire inside a face.
annulus = Part.makeFace([outer, inner], "Part::FaceMakerBullseye")
assert annulus.isValid(), "the annulus is valid"
assert len(annulus.Faces) == 1, len(annulus.Faces)
assert abs(annulus.Area - 64.0) < 1e-6, annulus.Area

# An island inside a hole is a face of its own.
island = Part.makeFace([outer, inner, square(3, 3, 4, 4)], "Part::FaceMakerBullseye")
assert island.isValid(), "the island compound is valid"
assert len(island.Faces) == 2, len(island.Faces)
assert abs(island.Area - 65.0) < 1e-6, island.Area

# Disconnected regions are separate faces, not one degenerate face. This is the
# cap `slicing.py` builds from a cut plane's several loops.
apart = Part.makeFace([square(0, 0, 1, 1), square(5, 5, 6, 6)], "Part::FaceMakerBullseye")
assert apart.isValid(), "disconnected regions must not make an invalid face"
assert len(apart.Faces) == 2, len(apart.Faces)
assert abs(apart.Area - 2.0) < 1e-6, apart.Area

# The plain constructor keeps upstream's first-wire/rest-are-holes build.
plain = Part.Face([outer, inner])
assert abs(plain.Area - 64.0) < 1e-6, plain.Area
"#,
        );
    });
}

#[test]
fn a_compound_of_faces_reports_its_surface_centre() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# `Part::getShapeProperties`: the property regime follows what the shape holds,
# so a compound of faces measures its surface instead of a zero-mass volume.
face = Part.Face(Part.Plane(V(20, 0, 0), V(1, 0, 0)), Part.makePolygon([
    V(20, 0, 0), V(20, 30, 0), V(20, 30, 20), V(20, 0, 20), V(20, 0, 0),
]))
assert abs(face.Area - 600.0) < 1e-6, face.Area
compound = Part.makeCompound([face])
centre = compound.CenterOfMass
assert abs(centre.x - 20.0) < 1e-6, tuple(centre)
assert abs(centre.y - 15.0) < 1e-6, tuple(centre)
assert abs(centre.z - 10.0) < 1e-6, tuple(centre)

# A boolean that keeps only this face reports the same centre.
other = Part.makeCompound([face.copy()])
common = compound.common(other)
assert len(common.Faces) == 1, len(common.Faces)
centre = common.CenterOfMass
assert abs(centre.x - 20.0) < 1e-6, tuple(centre)
assert abs(centre.y - 15.0) < 1e-6, tuple(centre)

# The principal axes are the face's own, not a zero-mass default.
properties = common.PrincipalProperties
assert sum(properties["Moments"]) > 0.0, properties["Moments"]
"#,
        );
    });
}

#[test]
fn tessellate_returns_the_shapes_own_vertex_coordinates() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# Upstream returns `Base.Vector` points; a rounded (single-precision) node is no
# longer the shape's vertex, and mesh tools that weld the two fail on the gap.
plane = Part.Plane(V(1e-3, 2e-3, 3e-3), V(0, 0, 1))
wire = Part.makePolygon([V(0.1, 0.2, 0), V(1.1, 0.2, 0), V(1.1, 1.2, 0),
                         V(0.1, 1.2, 0), V(0.1, 0.2, 0)])
face = Part.Face(plane, wire)
points, facets = face.tessellate(0.01)
assert facets, "the face tessellates"
vertices = [tuple(vertex.Point) for vertex in face.Vertexes]
for point in points:
    nearest = min(max(abs(point[i] - vertex[i]) for i in range(3)) for vertex in vertices)
    assert nearest < 1e-12, (tuple(point), nearest)
"#,
        );
    });
}
