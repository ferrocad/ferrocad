//! The kernel regimes SimpleCAD drives: typed `copy()` wrappers, non-planar
//! surface normals/derivatives, `Face.Surface.bounds()`, `Face.curveOnSurface`,
//! `Wire.discretize` and the real 2-D B-spline.
//!
//! Every test drives the public Python surface and asserts observable behaviour
//! (coordinates, classes, distances), not internals. The boot is the same as the
//! other Part tests: `ferrocad` + `Part` as built-ins over one OCCT backend, so it
//! needs OCCT (see `docs/architecture/occt-bundling.md`).

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
        panic!("the Part kernel-regime snippet failed:\n{source}");
    }
}

/// Every test shares one interpreter and one kernel, so they run one at a time.
fn locked() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn copy_keeps_the_typed_wrapper() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import Part
from ferrocad import Vector as V

# A wire copy stays a `Part.Wire`, so Draft's `wire.fixWire()` and
# `wire.OrderedEdges` are still reachable after a copy.
wire = Part.makeBox(4, 5, 6).Faces[0].OuterWire
copy = wire.copy()
assert isinstance(copy, Part.Wire), type(copy)
assert len(copy.OrderedEdges) == 4, len(copy.OrderedEdges)
assert len(copy.OrderedVertexes) == 4, len(copy.OrderedVertexes)
assert abs(copy.Length - wire.Length) < 1e-9
assert copy.isSame(wire) is False, "copy is a new TShape"
assert copy.fixWire() is None, "fixWire mutates in place"

# A solid copy stays a `Part.Solid`; a face copy a `Part.Face`.
box = Part.makeBox(2, 2, 2)
solid = box.copy()
assert isinstance(solid, Part.Solid), type(solid)
assert abs(solid.Volume - 8.0) < 1e-9

face = box.Faces[0]
face_copy = face.copy()
assert isinstance(face_copy, Part.Face), type(face_copy)
u1, u2, v1, v2 = face_copy.ParameterRange
# `normalAt` is a `Part.Face` method: losing the wrapper would hide it.
normal = face_copy.normalAt((u1 + u2) / 2, (v1 + v2) / 2)
assert abs(normal.Length - 1.0) < 1e-9

# A copy of an edge is still a `Part.Edge`, with `discretize`.
edge = box.Edges[0].copy()
assert isinstance(edge, Part.Edge), type(edge)
assert len(edge.discretize(Number=3)) == 3

# The empty shape stays the base class, as upstream's null `TopoShape`.
assert Part.Shape().copy().isNull()
"#,
        );
    });
}

#[test]
fn non_planar_surfaces_report_normals_and_derivatives() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V

def lateral(shape, type_id):
    return next(f for f in shape.Faces if f.Surface.TypeId == type_id)

# A cylinder's lateral face: the normal is the radial direction, perpendicular
# to both partial derivatives, at any (u, v) — not only on a plane.
face = lateral(Part.makeCylinder(5, 20), 'Part::GeomCylinder')
u, v = 1.2, 7.0
point = face.valueAt(u, v)
normal = face.normalAt(u, v)
radius = math.hypot(point.x, point.y)
assert abs(normal.x * 5 / radius - point.x / radius) < 1e-7, normal
assert abs(normal.z) < 1e-7, normal
assert abs(normal.Length - 1) < 1e-9

du, dv = face.derivative1At(u, v)
assert abs(normal.dot(du)) < 1e-7, normal.dot(du)
assert abs(normal.dot(dv)) < 1e-7, normal.dot(dv)
tangent_u, tangent_v = face.tangentAt(u, v)
assert abs(tangent_u.dot(du.normalize()) - 1) < 1e-9
assert abs(tangent_v.dot(dv.normalize()) - 1) < 1e-9
# The second derivatives are defined on a curved surface.
face.derivative2At(u, v)
# Principal curvatures (OCCT's algebraic pair): `0` along the generator and
# `±1/5` around the circumference, so the magnitudes are {0.2, 0}.
minimum, maximum = face.curvatureAt(u, v)
small, large = sorted(abs(value) for value in (minimum, maximum))
assert abs(small) < 1e-9 and abs(large - 0.2) < 1e-9, (minimum, maximum)

# The surface's own (untrimmed) bounds, as `Geom_Surface::Bounds` reports.
bounds = face.Surface.bounds()
assert abs(bounds[0]) < 1e-12 and abs(bounds[1] - math.tau) < 1e-12, bounds
# A cylinder is unbounded along its axis: `Precision::Infinite()`, i.e. a
# magnitude the drawing bounds check can never trip.
assert abs(bounds[2]) > 1e300 and abs(bounds[3]) > 1e300, bounds

# A sphere: radial normal, latitude bounds, 1/4 curvature everywhere.
sphere = lateral(Part.makeSphere(4), 'Part::GeomSphere')
point = sphere.valueAt(0.3, 0.4)
normal = sphere.normalAt(0.3, 0.4)
assert (normal * 4 - point).Length < 1e-7, normal
bounds = sphere.Surface.bounds()
assert abs(bounds[2] + math.pi / 2) < 1e-12, bounds
assert abs(bounds[3] - math.pi / 2) < 1e-12, bounds
minimum, maximum = sphere.curvatureAt(0.3, 0.4)
assert abs(abs(maximum) - 0.25) < 1e-9, maximum
assert abs(abs(minimum) - 0.25) < 1e-9, minimum

# A cone and a torus.
cone = lateral(Part.makeCone(2, 1, 5), 'Part::GeomCone')
assert abs(cone.normalAt(0.5, 2.0).Length - 1) < 1e-9
bounds = cone.Surface.bounds()
assert abs(bounds[1] - math.tau) < 1e-12, bounds

torus = lateral(Part.makeTorus(5, 1), 'Part::GeomToroid')
bounds = torus.Surface.bounds()
assert abs(bounds[3] - math.tau) < 1e-12, bounds
normal = torus.normalAt(0.4, 0.9)
assert abs(normal.Length - 1) < 1e-9

# A reversed face flips the normal, as upstream's `Tools::getNormal` does.
u, v = 1.2, 7.0
before = face.normalAt(u, v)
face.reverse()
after = face.normalAt(u, v)
assert (before + after).Length < 1e-7, (before, after)
"#,
        );
    });
}

#[test]
fn face_curve_on_surface_reports_the_pcurve() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
import ferrocad

def lateral(shape, type_id):
    return next(f for f in shape.Faces if f.Surface.TypeId == type_id)

face = lateral(Part.makeCylinder(5, 20), 'Part::GeomCylinder')
surface = face.Surface
assert len(face.Edges) >= 3, len(face.Edges)
for edge in face.Edges:
    data = face.curveOnSurface(edge)
    assert data is not None, "a face's own edge has a pcurve on it"
    curve, first, last = data
    # The pcurve shares the edge's parameterisation: `curve.value(t)` is the
    # surface's `(u, v)` of the 3D point at edge parameter `t`.
    uv = curve.value(first)
    expected = surface.parameter(edge.valueAt(edge.FirstParameter))
    assert abs(uv.x - expected[0]) < 1e-7, (uv, expected)
    assert abs(uv.y - expected[1]) < 1e-7, (uv, expected)
    # And it spans the edge's own range.
    assert abs(first - edge.FirstParameter) < 1e-9
    assert abs(last - edge.LastParameter) < 1e-9

# An edge that is not on this face has no pcurve there.
foreign = Part.makeBox(1, 1, 1).Edges[0]
assert face.curveOnSurface(foreign) is None

# A face's edges always have one; an edge built by `Geom2d` keeps its record.
segment = Part.Geom2d.Line2dSegment(ferrocad.Vector2d(0, 0), ferrocad.Vector2d(1, 1))
assert segment.toShape().curveOnSurface(0) is not None
"#,
        );
    });
}

#[test]
fn wire_discretize_samples_along_the_whole_wire() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part

circle = Part.makeCircle(5)
wire = Part.Wire(circle)
assert wire.ShapeType == 'Wire'

# `Number=n` gives n points at uniform arc length, all on the circle.
points = wire.discretize(Number=12)
assert len(points) == 12, len(points)
for point in points:
    assert abs(math.hypot(point.x, point.y) - 5) < 1e-7, point

# `Distance=d` covers the wire's length: a 10π circumference every π is 11.
points = wire.discretize(Distance=math.pi)
assert len(points) == 11, len(points)

# A positional int/float, and the `Angular` form, work as on an edge.
assert len(wire.discretize(8)) == 8
assert len(wire.discretize(Angular=0.2, Curvature=0.01, Minimum=4)) >= 4
"#,
        );
    });
}

#[test]
fn bspline2d_is_a_real_kernel_curve() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector2d as V2

spline = Part.Geom2d.BSplineCurve2d()

# Before `interpolate`/`buildFromPoles…`, every query is upstream's null handle:
# an `OCCError`, never an invented answer.
try:
    spline.value(0.5)
except Part.OCCError:
    pass
else:
    raise AssertionError('an unbuilt B-spline must not answer value()')

# `buildFromPolesMultsKnots` validates the description the way the 3D class does.
spline.buildFromPolesMultsKnots([V2(0, 0), V2(2, 0)], [2, 2], [0, 1], False, 1)
assert spline.NbPoles == 2, spline.NbPoles
assert spline.NbKnots == 2, spline.NbKnots
assert spline.Degree == 1, spline.Degree
assert spline.MaxDegree == 25
assert list(spline.KnotSequence) == [0, 0, 1, 1], list(spline.KnotSequence)
assert list(spline.getKnots()) == [0, 1], list(spline.getKnots())
assert list(spline.getMultiplicities()) == [2, 2]
assert list(spline.getWeights()) == [1, 1]
assert not spline.isRational()
assert not spline.isPeriodic()
middle = spline.value(0.5)
assert abs(middle.x - 1) < 1e-9 and abs(middle.y) < 1e-9, middle
assert abs(spline.FirstParameter) < 1e-12 and abs(spline.LastParameter - 1) < 1e-12

# `toShape()` is the planar edge; `toShape(surface)` lifts it onto the surface,
# so every sampled point lies on the cylinder.
flat = spline.toShape()
assert flat.ShapeType == 'Edge', flat.ShapeType
assert abs(flat.Length - 2) < 1e-9, flat.Length

cylinder = Part.Cylinder()
lifted = spline.toShape(cylinder)
assert lifted.ShapeType == 'Edge'
for point in lifted.discretize(Number=5):
    assert abs(math.hypot(point.x, point.y) - 1) < 1e-6, point
    assert abs(point.z) < 1e-6, point
# The 2D curve travels with the edge as its pcurve.
assert lifted.curveOnSurface(0) is not None

# `insertKnot` rebuilds the same geometry with one more pole.
spline.insertKnot(0.5)
assert spline.NbPoles == 3, spline.NbPoles
inserted = spline.value(0.5)
assert abs(inserted.x - 1) < 1e-9 and abs(inserted.y) < 1e-9, inserted

# `interpolate` parameterises by chord length, so the domain ends at the total
# chord length and the ends are the given points.
interpolated = Part.Geom2d.BSplineCurve2d()
interpolated.interpolate([V2(0, 0), V2(1, 1), V2(2, 0)])
assert abs(interpolated.LastParameter - 2 * math.sqrt(2)) < 1e-7
start = interpolated.value(interpolated.FirstParameter)
end = interpolated.value(interpolated.LastParameter)
assert abs(start.x) < 1e-9 and abs(start.y) < 1e-9, start
assert abs(end.x - 2) < 1e-9 and abs(end.y) < 1e-9, end
peak = interpolated.value(interpolated.LastParameter / 2)
assert abs(peak.x - 1) < 1e-7 and abs(peak.y - 1) < 1e-7, peak

# `approximate` fits the points with a real curve as well.
approximated = Part.Geom2d.BSplineCurve2d()
approximated.approximate([V2(0, 0), V2(1, 1), V2(2, 0)])
assert approximated.NbPoles >= 3, approximated.NbPoles

# `segment` and the periodicity switch rebuild in place.
interpolated.segment(0, 1)
assert abs(interpolated.LastParameter - 1) < 1e-9
"#,
        );
    });
}

#[test]
fn bspline2d_on_a_freeform_surface_lies_on_it() {
    let _guard = locked();
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part
from ferrocad import Vector as V, Vector2d as V2

# A fitted surface: the pcurve is lifted through the face's own surface, so the
# edge follows the freeform patch (not the XY plane it was drawn in).
surface = Part.BSplineSurface()
surface.interpolate([[V(x, y, 0.1 * x * x + 0.065 * y * y) for y in (0, 5, 10)]
                     for x in (0, 5, 10)])
face = max(surface.toShape().Faces, key=lambda f: f.Area)
u0, u1, v0, v1 = face.ParameterRange
anchor_u = (u0 + u1) / 2
anchor_v = (v0 + v1) / 2

# A straight pcurve along u at the anchor's v.
poles = [V2(anchor_u - 0.5, anchor_v), V2(anchor_u + 0.5, anchor_v)]
curve = Part.Geom2d.BSplineCurve2d()
curve.buildFromPolesMultsKnots(poles, [2, 2], [0, 1], False, 1)
edge = curve.toShape(face.Surface)
assert edge.ShapeType == 'Edge'
for point in edge.discretize(Number=7):
    assert face.distToShape(Part.Vertex(point))[0] < 1e-6, point

# The face's own edges report their pcurve, and it round-trips through the
# surface evaluation.
for boundary in face.Edges:
    data = face.curveOnSurface(boundary)
    if data is None:
        continue
    pcurve, first, last = data
    uv = pcurve.value(first)
    assert (face.valueAt(uv.x, uv.y) - boundary.valueAt(boundary.FirstParameter)).Length < 1e-6

# A general (B-spline) surface has a normal too: it is the unit normal of the
# tangent frame, and its own bounds are the untrimmed knot range.
normal = face.normalAt(anchor_u, anchor_v)
assert abs(normal.Length - 1) < 1e-9, normal
(du, dv) = face.derivative1At(anchor_u, anchor_v)
cross = du.cross(dv)
assert abs(abs(normal.dot(cross.normalize())) - 1) < 1e-6, (normal, cross)
bounds = face.Surface.bounds()
assert bounds[0] <= anchor_u <= bounds[1], bounds
assert bounds[2] <= anchor_v <= bounds[3], bounds
"#,
        );
    });
}
