//! The Part API gaps measured by the SimpleCAD and Draft conformance runs.
//!
//! Every test drives the public Python surface and asserts observable behaviour
//! (volumes, counts, coordinates, wrapper classes), not internals. The boot is the
//! same as `part_gap_ops`: `ferrocad` + `Part` as built-ins over one OCCT backend.
//!
//! It needs OCCT (see `docs/developer/architecture/occt-bundling.md`).

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
        panic!("the Part API snippet failed:\n{source}");
    }
}

/// Every test shares one interpreter and one kernel, so they run one at a time.
fn locked() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_face_reports_whether_a_parameter_pair_is_in_its_domain() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# A 10 by 10 face on z = 0; the kernel's surface frame centres u and v on it.
face = Part.Face(Part.makePolygon([V(0,0,0), V(10,0,0), V(10,10,0), V(0,10,0), V(0,0,0)]))
surface = face.Surface
centre = surface.parameter(V(5, 5, 0))
corner = surface.parameter(V(0, 0, 0))
assert face.isPartOfDomain(*centre) is True, "the centre is inside the face"
assert face.isPartOfDomain(*corner) is True, "a corner is on the boundary"
assert face.isPartOfDomain(centre[0] + 100, centre[1]) is False
assert face.isPartOfDomain(corner[0] - 1, corner[1]) is False
assert face.isPartOfDomain(centre[0], centre[1] + 100) is False

# A hole is outside the face's domain.
window = Part.Face(Part.makePolygon([V(4,4,0), V(6,4,0), V(6,6,0), V(4,6,0), V(4,4,0)]))
holed = Part.Face([face.OuterWire, window.OuterWire])
assert len(holed.Wires) == 2, len(holed.Wires)
inside_hole = holed.isPartOfDomain(*surface.parameter(V(5, 5, 0)))
assert inside_hole is False, "the hole is not part of the face's domain"
assert holed.isPartOfDomain(*surface.parameter(V(1, 1, 0))) is True
"#,
        );
    });
}

#[test]
fn fix_repairs_in_place_and_reports_validity() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

box = Part.makeBox(10, 10, 10)
volume = box.Volume
assert box.fix(1e-7, 1e-7, 1e-3) is True, "a valid box fixes cleanly"
assert box.isValid()
assert abs(box.Volume - volume) < 1e-6, "fixing a valid shape must not move it"

# A shell is fixed through its own tools (`ShapeFix_Shell`).
shell = box.Shells[0].copy()
assert shell.fix(1e-7, 1e-7, 1e-3) is True
assert shell.isValid()

# Upstream's signature allows the tolerances to be omitted.
assert Part.makeBox(1, 1, 1).fix(1e-7) is True
"#,
        );
    });
}

#[test]
fn a_b_spline_curve_and_its_conics_copy_independently() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

spline = Part.BSplineCurve()
spline.buildFromPolesMultsKnots([V(0,0,0), V(1,1,0), V(2,0,0)], [3, 3], [0, 1], False, 2)
clone = spline.copy()
assert clone is not spline, "copy() makes a new object"
assert clone.Degree == spline.Degree
assert [tuple(p) for p in clone.getPoles()] == [tuple(p) for p in spline.getPoles()]
assert clone.getKnots() == spline.getKnots()
assert clone.toShape().Length == spline.toShape().Length

# Mutating the copy leaves the source alone.
clone.setPole(2, V(5, 5, 0))
assert tuple(clone.getPole(2)) != tuple(spline.getPole(2))

circle = Part.Circle(V(1,2,3), V(0,0,1), 4)
radius = circle.Radius
copy = circle.copy()
copy.Radius = 7
assert circle.Radius == radius, "the circle copy is independent"
assert copy.Radius == 7

# `Geometry.copy` is what Draft's `edge.Curve.copy()` reaches.
edge = circle.toShape()
assert edge.Curve.copy().Radius == radius
"#,
        );
    });
}

#[test]
fn ellipse_center_axis_and_x_axis_are_writable() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

e = Part.Ellipse()
e.Center = V(3, 4, 5)
e.MajorRadius = 6
e.MinorRadius = 2
e.Axis = V(0, 1, 0)
e.XAxis = V(1, 0, 0)
assert tuple(e.Center) == (3, 4, 5), e.Center
assert tuple(e.Axis) == (0, 1, 0)
assert tuple(e.MajorAxis) == (1, 0, 0)
assert e.MajorRadius == 6 and e.MinorRadius == 2
assert tuple(e.toShape().Vertexes[0].Point) != (0, 0, 0)

# `Location` is the same point under upstream's newer name.
e.Location = V(1, 1, 1)
assert tuple(e.Center) == (1, 1, 1)

# `AngleXU` rotates the major axis in the plane; zero is the default frame.
assert abs(e.AngleXU) < 1e-12, e.AngleXU
e.AngleXU = math.pi / 2
assert abs(abs(e.AngleXU) - math.pi / 2) < 1e-9, e.AngleXU
assert abs(e.value(0).sub(e.Center).Length - e.MajorRadius) < 1e-9

# The same property exists on a circle.
c = Part.Circle()
c.Center = V(1, 2, 3)
c.Axis = V(0, 0, 1)
assert tuple(c.Center) == (1, 2, 3)
assert abs(c.AngleXU) < 1e-12
"#,
        );
    });
}

#[test]
fn curves_report_the_parameter_of_a_point() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

circle = Part.Circle(V(0,0,0), V(0,0,1), 2)
assert abs(circle.parameter(V(2, 0, 0))) < 1e-12
assert abs(circle.parameter(V(0, 2, 0)) - math.pi / 2) < 1e-12

ellipse = Part.Ellipse(V(0,0,0), 4, 1)
assert abs(ellipse.parameter(V(0, 1, 0)) - math.pi / 2) < 1e-12

segment = Part.LineSegment(V(0,0,0), V(0,0,10))
assert abs(segment.parameter(V(0,0,4)) - 4) < 1e-12

line = Part.Line(V(0,0,0), V(1,0,0))
assert abs(line.parameter(V(7,0,0)) - 7) < 1e-12
"#,
        );
    });
}

#[test]
fn make_offset_shape_and_fill_build_a_solid_from_a_face() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

face = Part.Face(Part.makePolygon([V(0,0,0), V(10,0,0), V(10,10,0), V(0,10,0), V(0,0,0)]))
area = face.Area
shell = face.makeOffsetShape(2.0, 1e-6)
assert shell.ShapeType in ("Shell", "Face", "Compound"), shell.ShapeType

solid = face.makeOffsetShape(2.0, 1e-6, fill=True)
assert solid.ShapeType == "Solid", f"fill=True closes the band into a solid, got {solid.ShapeType}"
assert abs(solid.Volume - area * 2.0) < 0.5 * area * 2.0, solid.Volume
assert solid.isValid()

# The solid offset of a solid keeps its volume in the right ballpark.
grow = Part.makeBox(10, 10, 10).makeOffsetShape(1.0, 1e-6)
assert grow.isValid()
assert grow.Volume > 1000.0
"#,
        );
    });
}

#[test]
fn defeature_removes_faces_and_heals_the_solid() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

block = Part.makeBox(40, 30, 20)
bored = block.cut(Part.makeCylinder(5, 40, V(20, 15, -10)))
assert len(bored.Solids) == 1
# The cylindrical wall is the only face without a plane.
wall = [f for f in bored.Faces if f.Surface.TypeId != "Part::GeomPlane"]
assert len(wall) == 1, [f.Surface.TypeId for f in bored.Faces]
healed = bored.defeaturing(wall)
assert healed.isValid(), "the healed shape is valid"
assert len(healed.Solids) == 1, f"the bore closes into one solid, got {healed.ShapeType}"
assert len(healed.Faces) < len(bored.Faces), "the wall and its rims are gone"
assert abs(healed.Volume - 40*30*20) < 1e-3, healed.Volume
"#,
        );
    });
}

#[test]
fn projections_project_a_wire_onto_a_face() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

face = Part.Face(Part.makePolygon([V(0,0,0), V(10,0,0), V(10,10,0), V(0,10,0), V(0,0,0)]))
wire = Part.makePolygon([V(2,2,5), V(8,2,5), V(8,8,5)])
parallel = face.makeParallelProjection(wire, V(0, 0, -1))
assert len(parallel.Edges) == 2, len(parallel.Edges)
for edge in parallel.Edges:
    for vertex in edge.Vertexes:
        assert abs(vertex.Point.z) < 1e-9, vertex.Point
    middle = edge.valueAt((edge.FirstParameter + edge.LastParameter) / 2)
    assert abs(middle.z) < 1e-9

perspective = face.makePerspectiveProjection(wire, V(5, 5, 20))
assert len(perspective.Edges) == 2
assert perspective.BoundBox.DiagonalLength > 0
"#,
        );
    });
}

#[test]
fn a_wire_lists_its_edges_in_connection_order_and_closes() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

corners = [V(0,0,0), V(10,0,0), V(10,10,0), V(0,10,0)]
edges = [Part.makeLine(corners[i], corners[(i + 1) % 4]) for i in range(4)]
wire = Part.Wire(edges)
assert wire.isClosed() is True
assert wire.Wires[0].isClosed() is True, "a wire and its own Wires[] agree"

ordered = wire.OrderedEdges
assert len(ordered) == 4
for index, edge in enumerate(ordered):
    start = corners[index]
    end = corners[(index + 1) % 4]
    points = {tuple(v.Point) for v in edge.Vertexes}
    assert tuple(start) in points and tuple(end) in points, (index, points)

# An edge stored in the opposite direction still chains (Draft's
# `test_offset_closed_with_reversed_edge`): the last edge runs d -> a.
reversed_edge = Part.makeLine(corners[0], corners[3])
closed = Part.Wire([edges[0], edges[1], edges[2], reversed_edge])
assert closed.isClosed() is True
assert len(closed.OrderedEdges) == 4

# An open chain reports the walk from one end.
open_wire = Part.makePolygon([V(0,0,0), V(4,0,0), V(4,3,0)])
assert open_wire.isClosed() is False
assert len(open_wire.OrderedEdges) == 2
"#,
        );
    });
}

#[test]
fn a_b_spline_face_exposes_its_surface_as_a_bspline_surface() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# A surface built from poles, trimmed by a wire drawn in its parameter plane.
poles = [
    [V(0,0,0), V(0,3,0), V(0,6,0)],
    [V(4,0,2), V(4,3,2), V(4,6,2)],
    [V(8,0,0), V(8,3,0), V(8,6,0)],
]
surface = Part.BSplineSurface()
surface.buildFromPolesMultsKnots(poles, [3,3], [3,3], [0,1], [0,1], False, False, 2, 2)
assert isinstance(surface, Part.BSplineSurface)
assert surface.UDegree == 2 and surface.VDegree == 2
assert surface.NbUPoles == 3 and surface.NbVPoles == 3
assert [tuple(p) for row in surface.getPoles() for p in row] == [tuple(p) for row in poles for p in row]
assert surface.getWeights() == [[1.0]*3]*3
assert surface.isURational() is False
assert hasattr(surface, "getPole") and hasattr(surface, "getUMultiplicities")

# `value` and `parameter` agree with each other.
u, v = surface.bounds()[:2]
point = surface.value(0.5, 0.5)
assert abs(surface.parameter(point)[0] - 0.5) < 1e-6, surface.parameter(point)
assert abs(surface.parameter(point)[1] - 0.5) < 1e-6

# `toShape` gives the untrimmed face; `Part.Face(surface, wire)` trims it.
face = surface.toShape()
assert face.ShapeType == "Face"
assert isinstance(face.Surface, Part.BSplineSurface)
assert face.Surface.UDegree == 2
assert abs(face.Surface.value(0.5, 0.5).sub(point).Length) < 1e-9

# A face read back from a B-spline surface reports the same poles.
assert len(face.Surface.getPoles()) == 3
"#,
        );
    });
}

#[test]
fn part_shape_accepts_a_list_of_shapes_and_geometry() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# Geometry objects are converted through `toShape`, as upstream's constructor does.
# Several entries are fused (`TopoShape::PyInit` → `makeElementBoolean("FUSE")`),
# not merely compounded, so arcs that meet at a point share the merged vertex.
arcs = [
    Part.Arc(V(0,0,0), V(2,-1,0), V(4,0,0)),
    Part.Arc(V(4,0,0), V(5,1.5,0), V(4,3,0)),
]
shape = Part.Shape(arcs)
assert shape.ShapeType == "Compound", shape.ShapeType
assert len(shape.Edges) == 2
assert len(shape.Vertexes) == 3, len(shape.Vertexes)  # the shared point is one vertex

lines = [Part.LineSegment(V(0,0,0), V(1,0,0)), Part.LineSegment(V(1,0,0), V(1,1,0))]
assert len(Part.Shape(lines).Edges) == 2
assert len(Part.Shape(lines).Vertexes) == 3

# A list of shapes and a single shape both work.
box = Part.makeBox(2, 2, 2)
compound = Part.Shape([box, Part.makeBox(2, 2, 2, V(5, 0, 0))])
assert compound.ShapeType == "Compound"
assert len(compound.Solids) == 2

single = Part.Shape(box)
assert single.isSame(box), "the single-shape form wraps the same handle"
assert Part.Shape([box]).isSame(box), "a one-element sequence is that shape"
assert Part.Shape().isNull()
"#,
        );
    });
}

#[test]
fn a_feature_shape_keeps_its_wrapper_and_placement() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part
from ferrocad import Vector as V

doc = App.newDocument("WrapperTest")
try:
    wire = Part.Wire([Part.makeLine(V(0,0,0), V(10,0,0)),
                      Part.makeLine(V(10,0,0), V(10,5,0))])
    obj = doc.addObject("Part::Feature", "Line")
    obj.Shape = wire
    doc.recompute()
    assert isinstance(obj.Shape, Part.Wire), type(obj.Shape)
    assert isinstance(obj.Shape, Part.Shape)

    face = Part.Face(Part.makePolygon([V(0,0,0), V(4,0,0), V(4,4,0), V(0,4,0), V(0,0,0)]))
    obj.Shape = face
    assert isinstance(obj.Shape, Part.Face), type(obj.Shape)

    box = doc.addObject("Part::Feature", "Box")
    box.Shape = Part.makeBox(2, 2, 2)
    assert isinstance(box.Shape, Part.Solid), type(box.Shape)

    # Upstream's `Feature::onChanged` bakes `Placement` into the stored `Shape`,
    # so the vertexes are in global coordinates and `Shape.Placement` is the
    # object's placement.
    placed = doc.addObject("Part::Feature", "Placed")
    placed.Shape = Part.makeBox(2, 2, 2)
    placed.Placement = App.Placement(V(5, 6, 7), App.Rotation())
    doc.recompute()
    assert tuple(placed.Shape.Placement.Base) == (5, 6, 7), placed.Shape.Placement
    minimum = min(tuple(v.Point) for v in placed.Shape.Vertexes)
    assert minimum == (5, 6, 7), minimum

    # A script assigning a world-space shape outside a recompute adopts the
    # shape's own (identity) transform, as upstream's `onChanged(Shape)` does.
    placed.Shape = Part.makeBox(2, 2, 2)
    assert tuple(placed.Placement.Base) == (0, 0, 0), placed.Placement
    minimum = min(tuple(v.Point) for v in placed.Shape.Vertexes)
    assert minimum == (0, 0, 0), minimum
finally:
    App.closeDocument(doc.Name)
"#,
        );
    });
}

#[test]
fn a_prism_document_object_extrudes_a_regular_polygon() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part
from ferrocad import Vector as V

doc = App.newDocument("PrismTest")
try:
    prism = doc.addObject("Part::Prism")
    assert prism.Polygon == 6 and prism.Circumradius == 2 and prism.Height == 10
    assert prism.FirstAngle == 0 and prism.SecondAngle == 0
    doc.recompute()
    assert prism.Shape.isValid()
    assert len(prism.Shape.Solids) == 1
    assert len(prism.Shape.Faces) == 8, len(prism.Shape.Faces)  # 6 sides, top and bottom

    prism.Polygon = 5
    prism.Circumradius = 4
    prism.Height = 3
    doc.recompute()
    assert len(prism.Shape.Faces) == 7, len(prism.Shape.Faces)
    # The circumradius reaches the vertexes.
    top = max(v.Point.z for v in prism.Shape.Vertexes)
    assert abs(top - 3) < 1e-9, top
    radial = max((v.Point.x**2 + v.Point.y**2) ** 0.5 for v in prism.Shape.Vertexes)
    assert abs(radial - 4) < 1e-9, radial

    # The placement moves the shape.
    prism.Placement = App.Placement(V(0, 0, 10), App.Rotation())
    doc.recompute()
    assert abs(min(v.Point.z for v in prism.Shape.Vertexes) - 10) < 1e-9
finally:
    App.closeDocument(doc.Name)
"#,
        );
    });
}

#[test]
fn make_polygon_skips_coincident_points() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# Draft's wire proxy builds `points + [points[0]]`, so a closed chain arrives
# with its first point repeated. OCCT's `BRepBuilderAPI_MakePolygon::Add`
# builds no edge for a point coincident with the previous one; the result must
# be a three-edge closed wire, not a degenerate closing segment.
pts = [V(0,0,0), V(2,2,0), V(2,4,0)]
wire = Part.makePolygon(pts + [pts[0]])
assert wire.ShapeType == "Wire", wire.ShapeType
assert len(wire.Edges) == 3, len(wire.Edges)
assert wire.isClosed()
assert abs(wire.Length - (2 * 2**0.5 + 2 + 20**0.5)) < 1e-9

repeat = Part.makePolygon([V(0,0,0), V(0,0,0), V(4,0,0), V(4,0,0), V(0,3,0)])
assert len(repeat.Edges) == 2, len(repeat.Edges)

# `closed=True` on a chain that already ends at the start adds no zero edge.
closed = Part.makePolygon([V(0,0,0), V(4,0,0), V(4,3,0), V(0,0,0)], True)
assert len(closed.Edges) == 3, len(closed.Edges)
assert closed.isClosed()
"#,
        );
    });
}

#[test]
fn transform_shape_bakes_an_affinity_into_the_geometry() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part
from ferrocad import Vector as V

# A non-uniform scale cannot be expressed by a gp_Trsf; with `checkScale=True`
# it must go through BRepBuilderAPI_GTransform and bake into the geometry,
# leaving Placement at identity (Draft.scale's part-feature path).
scale = App.Matrix()
scale.scale(V(2, 3, 1))
# A `gp_Trsf` would fold diag(2, 3, 1) into one uniform 1.817 scale; the
# `checkScale` path must apply each axis exactly.
x_line = Part.makeLine(V(0, 0, 0), V(2, 0, 0))
returned = x_line.transformShape(scale, False, True)
assert returned is x_line, "transformShape mutates and returns the shape"
assert x_line.Placement.Base.isEqual(V(0, 0, 0), 1e-9)
assert abs(max(v.Point.x for v in x_line.Vertexes) - 4) < 1e-6
y_line = Part.makeLine(V(0, 0, 0), V(0, 2, 0))
y_line.transformShape(scale, False, True)
assert abs(max(v.Point.y for v in y_line.Vertexes) - 6) < 1e-6

# `transformed` leaves the source alone and returns a new shape. Without
# `checkScale` a non-uniform matrix goes through `gp_Trsf`, which folds the three
# axes into one uniform scale (the same approximation upstream makes); asking it
# to check the scale gives the exact affinity.
source = Part.makeLine(V(0, 0, 0), V(2, 0, 0))
moved = source.transformed(scale, True, True)
assert abs(max(v.Point.x for v in source.Vertexes) - 2) < 1e-6, [tuple(v.Point) for v in source.Vertexes]
assert abs(max(v.Point.x for v in moved.Vertexes) - 4) < 1e-6, [tuple(v.Point) for v in moved.Vertexes]
assert abs(max(v.Point.y for v in moved.Vertexes)) < 1e-9
"#,
        );
    });
}

#[test]
fn a_part_feature_scaled_like_the_draft_tool_reports_scaled_vertexes() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import ferrocad as App
import Part
from ferrocad import Vector as V

base = V(3.5, 2.5, 0.0)
centre = V(2.0, 1.0, 0.0)
scale = V(2.0, 3.0, 1.0)
ends = [V(0,0,0), V(4,0,0), V(4,3,0), V(0,3,0)]
mids = [V(2,-0.5,0), V(4.5,1.5,0), V(2,3.5,0), V(-0.5,1.5,0)]
shape = Part.Shape([
    Part.Arc(ends[0], mids[0], ends[1]),
    Part.Arc(ends[1], mids[1], ends[2]),
    Part.Arc(ends[2], mids[2], ends[3]),
    Part.Arc(ends[3], mids[3], ends[0]),
])
assert len(shape.Vertexes) == 4, len(shape.Vertexes)

doc = App.newDocument("ScaleTest")
try:
    obj = doc.addObject("Part::Feature")
    obj.Shape = shape
    obj.Placement.Base = base
    doc.recompute()

    # `Draft.scale`'s non-parametric branch.
    pla = obj.Placement
    mtx = App.Matrix(pla.Matrix)
    mtx.move(-centre)
    mtx.scale(scale)
    mtx.move(centre)
    pla = App.Placement(mtx)
    mtx2 = App.Matrix()
    mtx2.move(-centre)
    mtx2.scale(scale)
    mtx2.move(centre)
    new_shape = obj.Shape.copy().transformShape(
        pla.Matrix.inverse().multiply(mtx2), False, True
    )
    assert new_shape.Placement.Base.isEqual(V(0, 0, 0), 1e-9)

    newobj = doc.addObject("Part::FeaturePython", "Scaled")
    newobj.Shape = Part.Compound([new_shape])
    newobj.Placement = pla
    doc.recompute()

    expected = [V(5, 5.5, 0), V(13, 5.5, 0), V(13, 14.5, 0), V(5, 14.5, 0)]
    got = [v.Point for v in newobj.Shape.Vertexes]
    assert len(got) == 4, len(got)
    for point, want in zip(got, expected):
        assert point.isEqual(want, 1e-6), (point, want)
finally:
    App.closeDocument(doc.Name)
"#,
        );
    });
}

#[test]
fn make_cylinder_accepts_a_partial_revolution() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

full = Part.makeCylinder(2, 10)
half = Part.makeCylinder(2, 10, V(0, 0, 0), V(0, 0, 1), 180)
assert abs(half.Volume - full.Volume / 2) < 1e-6 * full.Volume, (half.Volume, full.Volume)
quarter = Part.makeCylinder(2, 10, V(0, 0, 0), V(0, 0, 1), 90)
assert abs(quarter.Volume - full.Volume / 4) < 1e-6 * full.Volume
# A partial cylinder keeps one flat wall per radial end.
assert len(quarter.Faces) == 5, len(quarter.Faces)
"#,
        );
    });
}
