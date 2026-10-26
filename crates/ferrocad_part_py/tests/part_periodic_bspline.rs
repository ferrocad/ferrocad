//! Periodic B-splines: the lateral face of a cylinder through `Shape.toNurbs`
//! (`BRepBuilderAPI_NurbsConvert`) exposes each circle as a periodic
//! `Geom_BSplineCurve`. `Edge.Curve` must report it as a periodic
//! `Part.BSplineCurve`, and `toShape` must rebuild the same geometry.
//!
//! The knot/multiplicity relation of a periodic OCCT B-spline is not the clamped
//! one: the knot table carries the period-end knot, so the multiplicities sum to
//! `NbPoles + Multiplicity(1)`. The converted circle here is degree 2 with six
//! poles, knots `0, 2π/3, 4π/3, 2π`, every multiplicity 2.

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
        panic!("the periodic B-spline snippet failed:\n{source}");
    }
}

#[test]
fn to_nurbs_turns_a_circle_into_a_periodic_bspline_edge() {
    boot();
    Python::with_gil(|py| {
        run(
            py,
            r#"
import math
import Part

cylinder = Part.makeCylinder(2, 5)
lateral = [f for f in cylinder.Faces if f.Surface.TypeId == "Part::GeomCylinder"][0]
nurbs = lateral.toNurbs()

periodic = [e.Curve for e in nurbs.Faces[0].Edges if e.Curve.isPeriodic()]
assert periodic, "no periodic B-spline edge on the converted lateral face"
curve = periodic[0]

assert curve.TypeId == "Part::GeomBSplineCurve", curve.TypeId
assert curve.isPeriodic()
assert curve.isRational()
assert curve.Degree == 2, curve.Degree
assert curve.NbPoles == 6, curve.NbPoles
assert len(curve.getPoles()) == 6
assert len(curve.getWeights()) == 6
assert abs(min(curve.getWeights()) - 0.5) < 1e-12

# The period is the whole circle, and its end knot is stored in the table.
assert abs(curve.FirstParameter - 0.0) < 1e-12, curve.FirstParameter
assert abs(curve.LastParameter - 2 * math.pi) < 1e-12, curve.LastParameter
knots = curve.getKnots()
assert len(knots) == 4, knots
for got, want in zip(knots, [0.0, 2 * math.pi / 3, 4 * math.pi / 3, 2 * math.pi]):
    assert abs(got - want) < 1e-12, knots
assert curve.getMultiplicities() == [2, 2, 2, 2], curve.getMultiplicities()

# A point at a parameter: parameter 0 is on +X, pi/3 is at 60 degrees.
start = curve.value(0.0)
assert abs(start.x - 2.0) < 1e-9 and abs(start.y) < 1e-9, (start.x, start.y)
sixty = curve.value(math.pi / 3)
assert abs(sixty.x - 1.0) < 1e-9, sixty.x
assert abs(sixty.y - math.sqrt(3)) < 1e-9, sixty.y
assert abs(abs(start.z) - 5.0) < 1e-9, start.z

# `toShape` rebuilds a periodic edge with the same geometry.
edge = curve.toShape()
rebuilt = edge.Curve
assert rebuilt.isPeriodic()
assert rebuilt.NbPoles == 6
assert abs(rebuilt.LastParameter - 2 * math.pi) < 1e-9
for parameter in [0.0, 0.7, 2.4, 4.9]:
    a = curve.value(parameter)
    b = rebuilt.value(parameter)
    assert abs(a.x - b.x) < 1e-9 and abs(a.y - b.y) < 1e-9 and abs(a.z - b.z) < 1e-9, parameter

# The clamped form is unchanged: `buildFromPoles` without `periodic` stays clamped.
clamped = Part.BSplineCurve()
clamped.buildFromPoles([(0, 0, 0), (1, 1, 0), (2, 0, 0), (1, -1, 0)], False, 2)
assert not clamped.isPeriodic()
assert clamped.toShape().Curve.isPeriodic() is False
"#,
        );
    });
}
