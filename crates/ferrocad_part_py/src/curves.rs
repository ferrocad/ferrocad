//! The analytic half of `Part` in Python: curves, edges and wires.
//!
//! This is the first slice of the `Part` module the backlog calls the shared kernel
//! work: upstream's `Part::Geom*` classes ([`Part.LineSegment`], [`Part.Circle`],
//! [`Part.ArcOfCircle`]) plus the edge/wire containers Draft imports. The geometry is
//! [`ferrocad_curves`]; the conversion to a kernel shape is the `ferrocad_geom` seam
//! (`make_edge`/`make_wire`), implemented by OCCT.
//!
//! It also carries the small surface slice upstream's `Geom2d_tests.py` and
//! `BRep_tests.py` exercise: the `Part.Geom2d` submodule (`Line2dSegment`, `Circle2d`
//! and a `BSplineCurve2d` that records the points it is asked to fit but refuses to
//! fake knot insertion), the elementary surface descriptions `Part.Plane`,
//! `Part.Cylinder` (upstream's `Part::GeomCylinder`) and `Part.Sphere` (upstream's
//! `Part::GeomSphere`), and `Part.RectangularTrimmedSurface`. The cylinder and
//! sphere are distinct from the document primitives returned by
//! `Part.makeCylinder`/`Part.makeSphere`.
//!
//! Deliberate limits for this slice, all reported as `Part.OCCError` rather than a
//! silent `None`: an infinite `Line` has no edge, and face/solid constructors
//! (`makeFace`, `makeSolid`, …) are still to come. `Line2dSegment.toShape(surface)`
//! attaches the 2D segment as the edge's pcurve but does **not** project it onto the
//! surface: no `GeomProjLib` binding exists here, and `Part.Cylinder`/`Part.Sphere`
//! are only descriptions (radius, center, axis), not real B-Rep geometry.
//! `RectangularTrimmedSurface.toShape()` builds a real planar face for a `Part.Plane`
//! basis only; a trimmed cylinder or sphere is described but raises `Part.OCCError`
//! when asked for a face.

use std::sync::Mutex;

use ferrocad_curves::{
    ANGULAR_TOLERANCE, Arc, Circle, CurveError, EdgeCurve, Ellipse, Line, LineSegment, Vector3,
    polygon,
};
use ferrocad_geom::{ElementKind, Shape};
use ferrocad_part::backend;
use ferrocad_types::Matrix4;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyIndexError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule, PySequence, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::PartShape;

create_exception!(
    Part,
    OCCError,
    PyException,
    "A Part geometry operation failed (upstream's `Part.OCCError`)."
);

/// Map a curve-construction failure to the Python exception.
fn curve_error(error: CurveError) -> PyErr {
    OCCError::new_err(error.to_string())
}

/// Map a kernel failure to the Python exception.
fn kernel_error(error: ferrocad_geom::GeomError) -> PyErr {
    OCCError::new_err(error.to_string())
}

/// Read a point from a `Part.Vertex`, a `(x, y, z)` sequence or an object with
/// `x`/`y`/`z` (`FreeCAD.Vector`, `ferrocad.Vector`).
pub(crate) fn vector(value: &Bound<'_, PyAny>) -> PyResult<Vector3> {
    // Check the vertex first: it also satisfies the 3-sequence protocol.
    if let Ok(vertex) = value.extract::<PyRef<'_, PyVertex>>() {
        return Ok(vertex.point);
    }
    if let Ok((x, y, z)) = value.extract::<(f64, f64, f64)>() {
        return Ok(Vector3::new(x, y, z));
    }
    // Upstream coerces any 3-sequence to a vector (`BSplineCurve.buildFromPoles`
    // takes `[[0, 0, 0], …]`); the tuple extraction above only accepts a real
    // tuple, so a Python list lands here.
    if let Ok(sequence) = value.downcast::<PySequence>() {
        if sequence.len().ok() == Some(3) {
            let x: f64 = sequence.get_item(0)?.extract()?;
            let y: f64 = sequence.get_item(1)?.extract()?;
            let z: f64 = sequence.get_item(2)?.extract()?;
            return Ok(Vector3::new(x, y, z));
        }
    }
    let x: f64 = value.getattr("x")?.extract()?;
    let y: f64 = value.getattr("y")?.extract()?;
    let z: f64 = value.getattr("z")?.extract()?;
    Ok(Vector3::new(x, y, z))
}

/// A `ferrocad.Vector` from a [`Vector3`] (upstream returns `Base.Vector`).
pub(crate) fn py_vector(py: Python<'_>, value: Vector3) -> PyResult<PyObject> {
    let module = py.import("ferrocad")?;
    Ok(module
        .getattr("Vector")?
        .call1((value.x, value.y, value.z))?
        .unbind())
}

/// Read a 2D point from an `(x, y)` pair or an object with `x`/`y`
/// (`FreeCAD.Base.Vector2d`, `ferrocad.Vector2d`).
pub(crate) fn vector2d(value: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
    if let Ok((x, y)) = value.extract::<(f64, f64)>() {
        return Ok((x, y));
    }
    if let Ok(sequence) = value.downcast::<PySequence>() {
        if sequence.len().ok() == Some(2) {
            let x: f64 = sequence.get_item(0)?.extract()?;
            let y: f64 = sequence.get_item(1)?.extract()?;
            return Ok((x, y));
        }
    }
    let x: f64 = value.getattr("x")?.extract()?;
    let y: f64 = value.getattr("y")?.extract()?;
    Ok((x, y))
}

/// A `ferrocad.Vector2d` from an `(x, y)` pair (upstream returns `Base.Vector2d`).
fn py_vector2d(py: Python<'_>, value: (f64, f64)) -> PyResult<PyObject> {
    let module = py.import("ferrocad")?;
    Ok(module.getattr("Vector2d")?.call1(value)?.unbind())
}

fn shape_from_curve(curve: &EdgeCurve) -> PyResult<Shape> {
    backend().make_edge(curve).map_err(kernel_error)
}

fn curve_to_py(py: Python<'_>, curve: EdgeCurve) -> PyResult<PyObject> {
    match curve {
        EdgeCurve::Line(line) => Ok(Py::new(py, PyLine { curve: line })?.into_any()),
        EdgeCurve::LineSegment(segment) => {
            Ok(Py::new(py, PyLineSegment { curve: segment })?.into_any())
        }
        EdgeCurve::Circle(circle) => Ok(Py::new(py, PyCircle { curve: circle })?.into_any()),
        // Upstream's `TopoShapeEdgePy::getCurve` returns the *basis* curve of the
        // edge's `BRepAdaptor_Curve`, so a trimmed circular edge reports a
        // `Part.Circle` (with the edge's span available as First/LastParameter),
        // never an `ArcOfCircle`. `Part.ChFi2d` asserts exactly that.
        EdgeCurve::Arc(arc) => Ok(Py::new(py, PyCircle { curve: arc.circle() })?.into_any()),
        EdgeCurve::Ellipse(ellipse) => {
            Ok(Py::new(py, PyEllipse { curve: ellipse })?.into_any())
        }
    }
}

// ---------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------

/// `Part.LineSegment(start, end)` — a bounded straight segment.
#[pyclass(name = "LineSegment", module = "Part")]
#[derive(Clone)]
pub struct PyLineSegment {
    curve: LineSegment,
}

#[pymethods]
impl PyLineSegment {
    #[new]
    fn new(start: &Bound<'_, PyAny>, end: &Bound<'_, PyAny>) -> PyResult<Self> {
        let curve = LineSegment::new(vector(start)?, vector(end)?).map_err(curve_error)?;
        Ok(Self { curve })
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.start())
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.end())
    }

    /// The segment as an edge (upstream's `toShape`, which returns a `Part.Edge`).
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::LineSegment(self.curve))?)
    }

    fn __repr__(&self) -> String {
        format!("Part.LineSegment({:?}, {:?})", self.curve.start(), self.curve.end())
    }
}

/// `Part.Line(origin, direction)` — an infinite line.
#[pyclass(name = "Line", module = "Part")]
#[derive(Clone)]
pub struct PyLine {
    curve: Line,
}

#[pymethods]
impl PyLine {
    /// `Part.Line()` is upstream's default `Geom_Line`: the infinite line through
    /// the origin along `+Z`. `origin` and `direction` may be given together.
    #[new]
    #[pyo3(signature = (origin = None, direction = None))]
    fn new(
        origin: Option<&Bound<'_, PyAny>>,
        direction: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let origin = origin.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let direction = direction
            .map(vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        let curve = Line::new(origin, direction).map_err(curve_error)?;
        Ok(Self { curve })
    }

    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.origin())
    }

    #[getter]
    fn Direction(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.direction())
    }

    /// The point of the line closest to `point` (upstream's
    /// `GeomCurve::projectPoint`).
    fn projectPoint(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let origin = self.curve.origin();
        let direction = self.curve.direction();
        let projected = origin.add(&direction.scale(point.sub(&origin).dot(&direction)));
        py_vector(py, projected)
    }

    /// An infinite line has no bounded edge, as upstream's `GeomLine::toShape` is
    /// documented around a parameter range.
    fn toShape(&self, _py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Err(OCCError::new_err(
            "an infinite Part.Line has no edge; use Part.LineSegment",
        ))
    }
}

/// Read a `ferrocad.Matrix` through its `A11`…`A44` elements.
fn matrix_argument(value: &Bound<'_, PyAny>) -> PyResult<Matrix4> {
    let mut values = [0.0; 16];
    for row in 1..=4 {
        for column in 1..=4 {
            let name = format!("A{row}{column}");
            values[(row - 1) * 4 + (column - 1)] = value.getattr(name)?.extract()?;
        }
    }
    Ok(Matrix4::from_values(values))
}

/// The image of an analytic curve's frame under `matrix`.
///
/// Upstream's `Geometry::transform` builds a `gp_Trsf` from the matrix and calls
/// `Geom_Circle::Transform`/`Geom_Ellipse::Transform`; `gp_Trsf::SetValues`
/// accepts only a similarity (rotation, mirror, translation or uniform scale),
/// and `gp_Ax2::Transform` re-derives the normal from the transformed x and y
/// axes, so a mirror flips it. A general affine matrix is rejected rather than
/// sheared, as upstream's `Standard_ConstructionError` would.
fn transformed_frame(matrix: &Matrix4, normal: Vector3, x_axis: Vector3) -> PyResult<(Vector3, Vector3, f64)> {
    let m = &matrix.m;
    let linear = |v: Vector3| {
        Vector3::new(
            m[0] * v.x + m[1] * v.y + m[2] * v.z,
            m[4] * v.x + m[5] * v.y + m[6] * v.z,
            m[8] * v.x + m[9] * v.y + m[10] * v.z,
        )
    };
    let column = |offset: usize| Vector3::new(m[offset], m[offset + 4], m[offset + 8]);
    let (e1, e2, e3) = (column(0), column(1), column(2));
    let scale = e1.length();
    if !scale.is_finite() || scale <= 0.0 {
        return Err(OCCError::new_err(
            "a curve transform needs a non-singular matrix",
        ));
    }
    let tolerance = 1e-9 * scale * scale;
    let similar = e1.dot(&e2).abs() <= tolerance
        && e1.dot(&e3).abs() <= tolerance
        && e2.dot(&e3).abs() <= tolerance
        && (e2.length() - scale).abs() <= 1e-9 * scale
        && (e3.length() - scale).abs() <= 1e-9 * scale;
    if !similar {
        return Err(OCCError::new_err(
            "a curve transform needs a similarity matrix (rotation, mirror or uniform scale)",
        ));
    }
    let x_axis = linear(x_axis).scale(1.0 / scale);
    let y_axis = linear(normal.cross(&x_axis)).scale(1.0 / scale);
    Ok((x_axis, x_axis.cross(&y_axis), scale))
}

/// A circle mutated by `matrix` in place (`Geom_Circle::Transform`).
fn transformed_circle(circle: &Circle, matrix: &Matrix4) -> PyResult<Circle> {
    let (x_axis, normal, scale) = transformed_frame(matrix, circle.normal(), circle.x_axis())?;
    let center = matrix.transform(&circle.center());
    Circle::with_axis(center, normal, x_axis, circle.radius() * scale).map_err(curve_error)
}

/// An ellipse mutated by `matrix` in place (`Geom_Ellipse::Transform`).
fn transformed_ellipse(ellipse: &Ellipse, matrix: &Matrix4) -> PyResult<Ellipse> {
    let (x_axis, normal, scale) = transformed_frame(matrix, ellipse.normal(), ellipse.x_axis())?;
    let center = matrix.transform(&ellipse.center());
    Ellipse::with_axis(
        center,
        normal,
        x_axis,
        ellipse.major_radius() * scale,
        ellipse.minor_radius() * scale,
    )
    .map_err(curve_error)
}

/// `Part.Circle(center, normal, radius)` — a full circle.
#[pyclass(name = "Circle", module = "Part")]
#[derive(Clone)]
pub struct PyCircle {
    curve: Circle,
}

#[pymethods]
impl PyCircle {
    #[new]
    #[pyo3(signature = (center = None, normal = None, radius = 1.0))]
    fn new(
        center: Option<&Bound<'_, PyAny>>,
        normal: Option<&Bound<'_, PyAny>>,
        radius: f64,
    ) -> PyResult<Self> {
        let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let normal = normal
            .map(vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        let curve = Circle::new(center, normal, radius).map_err(curve_error)?;
        Ok(Self { curve })
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.curve.radius()
    }

    /// Upstream's `GeomCircle::setRadius`: the centre and the plane are kept, the
    /// radius is replaced (a non-positive radius raises `Part.OCCError`).
    #[setter]
    fn set_Radius(&mut self, value: f64) -> PyResult<()> {
        self.curve = Circle::with_axis(
            self.curve.center(),
            self.curve.normal(),
            self.curve.x_axis(),
            value,
        )
        .map_err(curve_error)?;
        Ok(())
    }

    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::Circle(self.curve))?)
    }

    /// `Circle.transform(matrix)` — apply a similarity transform in place, as
    /// upstream's `Geom_Circle::Transform` does.
    fn transform(&mut self, matrix: &Bound<'_, PyAny>) -> PyResult<()> {
        let matrix = matrix_argument(matrix)?;
        self.curve = transformed_circle(&self.curve, &matrix)?;
        Ok(())
    }

    /// `value(u)`: the point at angle `u` (upstream's `Geom_Curve::Value`).
    fn value(&self, py: Python<'_>, angle: f64) -> PyResult<PyObject> {
        py_vector(py, self.curve.point_at(angle))
    }

    /// `toNurbs(first=0, last=2π)`: the exact rational B-spline for the circle
    /// over the given parameter range (upstream's `GeomCircle::toNurbs`).
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        conic_spline(py, &Conic::Circle(self.curve), first, last, true)
    }

    /// `toBSpline(first=0, last=2π)`: the B-spline upstream builds with
    /// `ShapeConstruct_Curve` (an approximation, unlike [`toNurbs`](Self::toNurbs)).
    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        conic_spline(py, &Conic::Circle(self.curve), first, last, false)
    }

    /// `trim(first=0, last=2π)`: the arc over the given parameter range, as a
    /// `Part.ArcOfCircle` (upstream's `GeometryCurvePy::trim` routed through
    /// `makeTrimmedCurvePy`).
    #[pyo3(signature = (first = None, last = None))]
    fn trim(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyArcOfCircle>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        let curve = Arc::new(self.curve, first, last).map_err(curve_error)?;
        Py::new(py, PyArcOfCircle { curve })
    }
}

/// `Part.ArcOfCircle(circle, first, last)` — a circular arc (the `Part.Arc` alias).
#[pyclass(name = "ArcOfCircle", module = "Part")]
#[derive(Clone)]
pub struct PyArcOfCircle {
    curve: Arc,
}

#[pymethods]
impl PyArcOfCircle {
    #[new]
    fn new(circle: &Bound<'_, PyAny>, first: f64, last: f64) -> PyResult<Self> {
        let circle = circle.extract::<PyRef<PyCircle>>()?;
        let curve = Arc::new(circle.curve, first, last).map_err(curve_error)?;
        Ok(Self { curve })
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.circle().center())
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.curve.circle().radius()
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.start())
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.end())
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.curve.first()
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.curve.last()
    }

    /// `value(u)`: the point at angle `u` (the basis circle's parameter).
    fn value(&self, py: Python<'_>, angle: f64) -> PyResult<PyObject> {
        py_vector(py, self.curve.point_at(angle))
    }

    /// `toNurbs(first, last)`: the exact rational B-spline over the arc (the
    /// range defaults to the arc's own parameters, as upstream's
    /// `GeomArcOfCircle::toNurbs`).
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.curve.first());
        let last = last.unwrap_or(self.curve.last());
        conic_spline(py, &Conic::Circle(self.curve.circle()), first, last, true)
    }

    /// `toBSpline(first, last)`: the approximating conversion over the arc.
    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.curve.first());
        let last = last.unwrap_or(self.curve.last());
        conic_spline(py, &Conic::Circle(self.curve.circle()), first, last, false)
    }

    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::Arc(self.curve))?)
    }
}

/// `Part.Ellipse(center=None, major_radius=1.0, minor_radius=0.5)` — a full ellipse
/// (upstream's `Part::GeomEllipse`), the ellipse counterpart of [`PyCircle`].
///
/// Upstream's no-argument form comes from an empty `gp_Elips`, whose radii are
/// `RealLast`/`RealSmall`; this geometry model only holds finite radii, so the
/// default here is a unit-major, half-unit-minor ellipse. As upstream's
/// `Geom_Ellipse`, the major axis always has the larger radius: a minor radius
/// larger than the major one raises `Part.OCCError` instead of being swapped.
///
/// The ellipse lies in the plane through `center` with normal `+Z`, and
/// `value(u)` takes the angle in radians from the major axis, as upstream.
#[pyclass(name = "Ellipse", module = "Part")]
#[derive(Clone)]
pub struct PyEllipse {
    curve: Ellipse,
}

#[pymethods]
impl PyEllipse {
    #[new]
    #[pyo3(signature = (center = None, major_radius = 1.0, minor_radius = 0.5))]
    fn new(
        center: Option<&Bound<'_, PyAny>>,
        major_radius: f64,
        minor_radius: f64,
    ) -> PyResult<Self> {
        if minor_radius > major_radius {
            return Err(OCCError::new_err(
                "Part.Ellipse needs the major radius to be at least the minor radius",
            ));
        }
        let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let curve = Ellipse::new(center, Vector3::new(0.0, 0.0, 1.0), major_radius, minor_radius)
            .map_err(curve_error)?;
        Ok(Self { curve })
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    /// The unit normal of the ellipse's plane (upstream's `Axis`).
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    /// The direction of the major axis.
    #[getter]
    fn MajorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.x_axis())
    }

    /// The direction of the minor axis.
    #[getter]
    fn MinorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.y_axis())
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.curve.major_radius()
    }

    #[setter]
    fn set_MajorRadius(&mut self, value: f64) -> PyResult<()> {
        if value < self.curve.minor_radius() {
            return Err(OCCError::new_err(
                "Part.Ellipse needs the major radius to be at least the minor radius",
            ));
        }
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            self.curve.normal(),
            self.curve.x_axis(),
            value,
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn MinorRadius(&self) -> f64 {
        self.curve.minor_radius()
    }

    #[setter]
    fn set_MinorRadius(&mut self, value: f64) -> PyResult<()> {
        if value > self.curve.major_radius() {
            return Err(OCCError::new_err(
                "Part.Ellipse needs the minor radius to be at most the major radius",
            ));
        }
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            self.curve.normal(),
            self.curve.x_axis(),
            self.curve.major_radius(),
            value,
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The point at the parameter `angle` (radians, measured from the major axis).
    fn value(&self, py: Python<'_>, angle: f64) -> PyResult<PyObject> {
        py_vector(py, self.curve.point_at(angle))
    }

    /// The ellipse as an edge.
    ///
    /// A `Geom_Ellipse`-backed `BRepBuilderAPI_MakeEdge`: the ellipse's own frame
    /// (centre, plane normal, major axis and the two radii) is handed to
    /// `gp_Elips`, so the edge is the exact analytic ellipse, not a NURBS
    /// approximation.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::Ellipse(self.curve))?)
    }

    /// `Ellipse.transform(matrix)` — apply a similarity transform in place, as
    /// upstream's `Geom_Ellipse::Transform` does.
    fn transform(&mut self, matrix: &Bound<'_, PyAny>) -> PyResult<()> {
        let matrix = matrix_argument(matrix)?;
        self.curve = transformed_ellipse(&self.curve, &matrix)?;
        Ok(())
    }

    /// `toNurbs(first=0, last=2π)`: the exact rational B-spline for the ellipse
    /// over the given parameter range (upstream's `GeomEllipse::toNurbs`).
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        conic_spline(py, &Conic::Ellipse(self.curve), first, last, true)
    }

    /// `toBSpline(first=0, last=2π)`: the B-spline upstream builds with
    /// `ShapeConstruct_Curve` (an approximation, unlike [`toNurbs`](Self::toNurbs)).
    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        conic_spline(py, &Conic::Ellipse(self.curve), first, last, false)
    }

    /// `trim(first=0, last=2π)`: the elliptical arc over the given parameter
    /// range, as a `Part.ArcOfEllipse` (upstream's `GeometryCurvePy::trim`).
    #[pyo3(signature = (first = None, last = None))]
    fn trim(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyArcOfEllipse>> {
        let first = first.unwrap_or(0.0);
        let last = last.unwrap_or(std::f64::consts::TAU);
        if !first.is_finite()
            || !last.is_finite()
            || (last - first).abs() <= ANGULAR_TOLERANCE
        {
            return Err(OCCError::new_err("an ellipse arc needs a non-zero sweep"));
        }
        Py::new(
            py,
            PyArcOfEllipse {
                ellipse: self.curve,
                first,
                last,
            },
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Ellipse({:?}, {}, {})",
            self.curve.center(),
            self.curve.major_radius(),
            self.curve.minor_radius()
        )
    }
}

/// `Part.ArcOfEllipse(ellipse, first, last)` — a portion of an ellipse
/// (upstream's `Part::GeomArcOfEllipse`), what [`PyEllipse::trim`] returns.
///
/// Like `Geom_TrimmedCurve`, the parameter is the basis ellipse's angle, so
/// `value(u)` and `FirstParameter()`/`LastParameter()` live on the ellipse's own
/// parameterisation.
#[pyclass(name = "ArcOfEllipse", module = "Part")]
#[derive(Clone)]
pub struct PyArcOfEllipse {
    ellipse: Ellipse,
    first: f64,
    last: f64,
}

#[pymethods]
impl PyArcOfEllipse {
    #[new]
    fn new(ellipse: &Bound<'_, PyAny>, first: f64, last: f64) -> PyResult<Self> {
        let ellipse = ellipse.extract::<PyRef<'_, PyEllipse>>()?.curve;
        if !first.is_finite()
            || !last.is_finite()
            || (last - first).abs() <= ANGULAR_TOLERANCE
        {
            return Err(OCCError::new_err("an ellipse arc needs a non-zero sweep"));
        }
        Ok(Self {
            ellipse,
            first,
            last,
        })
    }

    /// The supporting ellipse (upstream's `Ellipse`).
    #[getter]
    fn Ellipse(&self, py: Python<'_>) -> PyResult<Py<PyEllipse>> {
        Py::new(
            py,
            PyEllipse {
                curve: self.ellipse,
            },
        )
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.center())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.normal())
    }

    #[getter]
    fn MajorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.x_axis())
    }

    #[getter]
    fn MinorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.y_axis())
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.ellipse.major_radius()
    }

    #[getter]
    fn MinorRadius(&self) -> f64 {
        self.ellipse.minor_radius()
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.point_at(self.first))
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.point_at(self.last))
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.first
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.last
    }

    /// `value(u)`: the point at angle `u` (the basis ellipse's parameter).
    fn value(&self, py: Python<'_>, angle: f64) -> PyResult<PyObject> {
        py_vector(py, self.ellipse.point_at(angle))
    }

    /// `toNurbs(first, last)`: the exact rational B-spline over the arc (the
    /// range defaults to the arc's own parameters, as upstream's
    /// `GeomArcOfEllipse::toNurbs`).
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.first);
        let last = last.unwrap_or(self.last);
        conic_spline(py, &Conic::Ellipse(self.ellipse), first, last, true)
    }

    /// `toBSpline(first, last)`: the approximating conversion over the arc.
    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.first);
        let last = last.unwrap_or(self.last);
        conic_spline(py, &Conic::Ellipse(self.ellipse), first, last, false)
    }
}

/// `Part.Plane(origin=None, normal=None)` — an infinite plane, the XY plane
/// (`Part.Plane()`) by default.
#[pyclass(name = "Plane", module = "Part")]
#[derive(Clone)]
pub struct PyPlane {
    location: Vector3,
    /// The unit normal, as upstream's `Geom_Plane` keeps a `gp_Dir`.
    normal: Vector3,
}

#[pymethods]
impl PyPlane {
    #[new]
    #[pyo3(signature = (origin = None, normal = None))]
    fn new(origin: Option<&Bound<'_, PyAny>>, normal: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let location = origin.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let normal = normal
            .map(vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        if normal.is_null() {
            return Err(OCCError::new_err("Part.Plane needs a non-zero normal"));
        }
        Ok(Self {
            location,
            normal: normal.normalize(),
        })
    }

    /// The plane's origin (upstream's `Plane.Position`).
    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.location)
    }

    /// The plane's unit normal (upstream's `Plane.Axis`).
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.normal)
    }

    /// The intersection with `other` as a list of lines: empty when the planes are
    /// parallel or coincident, otherwise the single line they share (upstream's
    /// `GeomAPI_IntSS` behaviour).
    fn intersect(&self, py: Python<'_>, other: PyRef<'_, PyPlane>) -> PyResult<Vec<Py<PyLine>>> {
        let cross = self.normal.cross(&other.normal);
        if cross.is_null() {
            return Ok(Vec::new());
        }
        // With unit normals the planes are `n1·x = d1` and `n2·x = d2`, and a
        // point on the shared line is `(d1 (n2 × c) + d2 (c × n1)) / |c|²` for
        // `c = n1 × n2`; `Line::new` normalises the direction for us.
        let d1 = self.normal.dot(&self.location);
        let d2 = other.normal.dot(&other.location);
        let point = other
            .normal
            .cross(&cross)
            .scale(d1)
            .add(&cross.cross(&self.normal).scale(d2))
            .scale(1.0 / cross.sqr());
        let line = Line::new(point, cross).map_err(curve_error)?;
        Ok(vec![Py::new(py, PyLine { curve: line })?])
    }

    /// The plane as a `Part.Face` over the parameter rectangle
    /// `(u1, u2, v1, v2)` (upstream's `GeomSurface::toShape`, which builds a
    /// `BRepBuilderAPI_MakeFace` over the given bounds).
    ///
    /// `u` and `v` run along the plane's in-plane axes ([`plane_frame`]).
    /// Upstream's no-argument form falls back to the surface's natural bounds,
    /// which an infinite plane does not have, so this slice requires the finite
    /// rectangle.
    #[pyo3(signature = (u1 = None, u2 = None, v1 = None, v2 = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        u1: Option<f64>,
        u2: Option<f64>,
        v1: Option<f64>,
        v2: Option<f64>,
    ) -> PyResult<Py<PyFace>> {
        let (Some(u1), Some(u2), Some(v1), Some(v2)) = (u1, u2, v1, v2) else {
            return Err(OCCError::new_err(
                "Part.Plane.toShape() needs the finite parameter rectangle \
                 (u1, u2, v1, v2); an infinite plane has no natural bounds",
            ));
        };
        plane_face(py, self, u1, u2, v1, v2)
    }

    fn __repr__(&self) -> String {
        format!("Part.Plane({:?}, {:?})", self.location, self.normal)
    }
}

/// `Part.Cylinder(radius=1.0, center=None, axis=None)` — an infinite cylindrical
/// **surface** (upstream's `Part::GeomCylinder`), not the document primitive that
/// `Part.makeCylinder` returns.
///
/// This slice carries only the elementary description upstream's Python surface
/// exposes: `Radius`, `Center` and `Axis`. Upstream builds the default from an
/// empty `gp_Cylinder` (center at the origin, axis `+Z`, radius 1); the `radius` /
/// `center` / `axis` keywords are our extension — upstream only accepts another
/// `Cylinder`, a `Circle`, or three points.
///
/// The object is accepted by [`PyLine2dSegment::toShape`] as the `surface`
/// argument. It answers no shape queries (`Faces`, `Edges`, …): no B-Rep geometry
/// is faked here.
#[pyclass(name = "Cylinder", module = "Part")]
#[derive(Clone)]
pub struct PyCylinder {
    radius: f64,
    center: Vector3,
    /// The unit axis direction (upstream's `Axis` on the surface is a direction).
    axis: Vector3,
}

#[pymethods]
impl PyCylinder {
    #[new]
    #[pyo3(signature = (radius = 1.0, center = None, axis = None))]
    fn new(
        radius: f64,
        center: Option<&Bound<'_, PyAny>>,
        axis: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        if !(radius > 0.0) {
            return Err(OCCError::new_err("Part.Cylinder needs a positive radius"));
        }
        let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let axis = axis
            .map(vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Cylinder needs a non-zero axis"));
        }
        Ok(Self {
            radius,
            center,
            axis: axis.normalize(),
        })
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.radius
    }

    /// The axis point (upstream's `Cylinder.Center`).
    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.center)
    }

    /// The unit axis direction (upstream's `Cylinder.Axis`).
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.axis)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Cylinder({}, {:?}, {:?})",
            self.radius, self.center, self.axis
        )
    }
}

/// `Part.Sphere(radius=1.0, center=None, axis=None)` — a spherical **surface**
/// (upstream's `Part::GeomSphere`), the sphere counterpart of [`PyCylinder`].
///
/// Like the cylinder this is an elementary description only (`Radius`, `Center`,
/// `Axis`); upstream builds the default from an empty `gp_Sphere` (center at the
/// origin, axis `+Z`, radius 1) and the `radius`/`center`/`axis` keywords are our
/// extension. It is accepted by [`PyLine2dSegment::toShape`] as the `surface`
/// argument, and answers no shape queries: no B-Rep geometry is faked here.
#[pyclass(name = "Sphere", module = "Part")]
#[derive(Clone)]
pub struct PySphere {
    radius: f64,
    center: Vector3,
    /// The unit axis direction (upstream's `Sphere.Axis` is a direction).
    axis: Vector3,
}

#[pymethods]
impl PySphere {
    #[new]
    #[pyo3(signature = (radius = 1.0, center = None, axis = None))]
    fn new(
        radius: f64,
        center: Option<&Bound<'_, PyAny>>,
        axis: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        if !(radius > 0.0) {
            return Err(OCCError::new_err("Part.Sphere needs a positive radius"));
        }
        let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
        let axis = axis
            .map(vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Sphere needs a non-zero axis"));
        }
        Ok(Self {
            radius,
            center,
            axis: axis.normalize(),
        })
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.radius
    }

    /// The sphere's center (upstream's `Sphere.Center`).
    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.center)
    }

    /// The unit axis direction (upstream's `Sphere.Axis`).
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.axis)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Sphere({}, {:?}, {:?})",
            self.radius, self.center, self.axis
        )
    }
}

/// The basis surface of a [`PyRectangularTrimmedSurface`].
#[derive(Clone)]
enum TrimmedBasis {
    Plane(PyPlane),
    Cylinder(PyCylinder),
    Sphere(PySphere),
}

/// `Part.RectangularTrimmedSurface(surface, u1, u2, v1, v2)` — a patch of a surface
/// bounded by two parameter values in each parametric direction (upstream's
/// `Part::GeomTrimmedSurface`).
///
/// The constructor accepts a [`PyPlane`], [`PyCylinder`] or [`PySphere`] as the
/// basis surface and requires `u1 < u2` and `v1 < v2` (the `usense`/`vsense`
/// arguments upstream also takes are not modelled, so a reversed trim is rejected
/// instead of silently flipping orientation).
///
/// `toShape()` builds a real planar face only for a **plane** basis: the four
/// corners are `location + u·u_dir + v·v_dir` in the plane's local frame, joined by
/// `LineSegment` edges through the kernel's `make_wire`/`make_face`. A trimmed
/// cylinder or sphere can be described and queried (`BasisSurface`, `U1`…`V2`) but
/// has no B-Rep builder in this slice, so its `toShape()` raises `Part.OCCError`.
#[pyclass(name = "RectangularTrimmedSurface", module = "Part")]
#[derive(Clone)]
pub struct PyRectangularTrimmedSurface {
    basis: TrimmedBasis,
    u1: f64,
    u2: f64,
    v1: f64,
    v2: f64,
}

#[pymethods]
impl PyRectangularTrimmedSurface {
    #[new]
    fn new(
        surface: &Bound<'_, PyAny>,
        u1: f64,
        u2: f64,
        v1: f64,
        v2: f64,
    ) -> PyResult<Self> {
        let basis = if let Ok(plane) = surface.downcast::<PyPlane>() {
            TrimmedBasis::Plane(plane.borrow().clone())
        } else if let Ok(cylinder) = surface.downcast::<PyCylinder>() {
            TrimmedBasis::Cylinder(cylinder.borrow().clone())
        } else if let Ok(sphere) = surface.downcast::<PySphere>() {
            TrimmedBasis::Sphere(sphere.borrow().clone())
        } else {
            return Err(PyTypeError::new_err(
                "Part.RectangularTrimmedSurface expects a Part surface \
                 (Part.Plane, Part.Cylinder or Part.Sphere)",
            ));
        };
        if !(u1 < u2) || !(v1 < v2) {
            return Err(OCCError::new_err(
                "Part.RectangularTrimmedSurface needs u1 < u2 and v1 < v2",
            ));
        }
        Ok(Self {
            basis,
            u1,
            u2,
            v1,
            v2,
        })
    }

    /// The surface being trimmed (upstream's `BasisSurface`). A copy of the basis
    /// is returned, matching upstream's "built from a copy" semantics.
    #[getter]
    fn BasisSurface(&self, py: Python<'_>) -> PyResult<PyObject> {
        Ok(match &self.basis {
            TrimmedBasis::Plane(plane) => Py::new(py, plane.clone())?.into_any(),
            TrimmedBasis::Cylinder(cylinder) => Py::new(py, cylinder.clone())?.into_any(),
            TrimmedBasis::Sphere(sphere) => Py::new(py, sphere.clone())?.into_any(),
        })
    }

    #[getter]
    fn U1(&self) -> f64 {
        self.u1
    }

    #[getter]
    fn U2(&self) -> f64 {
        self.u2
    }

    #[getter]
    fn V1(&self) -> f64 {
        self.v1
    }

    #[getter]
    fn V2(&self) -> f64 {
        self.v2
    }

    /// The trimmed patch as a `Part.Face`.
    ///
    /// Only a plane basis produces geometry in this slice; the four rectangle
    /// corners are joined into a wire and a planar face. A `Part.Cylinder` or
    /// `Part.Sphere` basis raises `Part.OCCError`, as does an empty rectangle.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyFace>> {
        let TrimmedBasis::Plane(plane) = &self.basis else {
            return Err(OCCError::new_err(
                "Part.RectangularTrimmedSurface.toShape() only builds a face for a \
                 Part.Plane basis in this slice (a trimmed cylinder or sphere has no \
                 B-Rep builder yet)",
            ));
        };
        plane_face(py, plane, self.u1, self.u2, self.v1, self.v2)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.RectangularTrimmedSurface({}, {}, {}, {}, {})",
            self.basis_name(),
            self.u1,
            self.u2,
            self.v1,
            self.v2
        )
    }
}

impl PyRectangularTrimmedSurface {
    fn basis_name(&self) -> &'static str {
        match &self.basis {
            TrimmedBasis::Plane(_) => "Part.Plane()",
            TrimmedBasis::Cylinder(_) => "Part.Cylinder()",
            TrimmedBasis::Sphere(_) => "Part.Sphere()",
        }
    }
}

/// In-plane `(u, v)` directions for a plane with unit `normal`.
///
/// `u` is the world X axis projected onto the plane (world Y when the normal is
/// nearly along X), so the default XY plane keeps `u = +X` and `v = +Y`; `v` is
/// then `normal × u`, which makes `(u, v, normal)` right-handed.
fn plane_frame(normal: &Vector3) -> (Vector3, Vector3) {
    let reference = if normal.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let u = reference
        .sub(&normal.scale(normal.dot(&reference)))
        .normalize();
    (u, normal.cross(&u))
}

/// The planar face over the parameter rectangle `(u1, u2, v1, v2)` of `plane`.
///
/// Shared by `Part.Plane.toShape` and `Part.RectangularTrimmedSurface.toShape`:
/// the four corners are `location + u·u_dir + v·v_dir` in the plane's frame
/// ([`plane_frame`]), joined into a wire and a planar face through the kernel's
/// `make_wire`/`make_face`.
fn plane_face(
    py: Python<'_>,
    plane: &PyPlane,
    u1: f64,
    u2: f64,
    v1: f64,
    v2: f64,
) -> PyResult<Py<PyFace>> {
    if !(u1 < u2) || !(v1 < v2) || ![u1, u2, v1, v2].iter().all(|value| value.is_finite()) {
        return Err(OCCError::new_err(
            "a planar face needs a finite parameter rectangle with u1 < u2 and v1 < v2",
        ));
    }
    let (u_dir, v_dir) = plane_frame(&plane.normal);
    let corner = |u: f64, v: f64| {
        plane
            .location
            .add(&u_dir.scale(u))
            .add(&v_dir.scale(v))
    };
    let corners = [
        corner(u1, v1),
        corner(u2, v1),
        corner(u2, v2),
        corner(u1, v2),
    ];
    let mut edges = Vec::with_capacity(4);
    for index in 0..4 {
        let segment = LineSegment::new(corners[index], corners[(index + 1) % 4])
            .map_err(curve_error)?;
        edges.push(shape_from_curve(&EdgeCurve::LineSegment(segment))?);
    }
    let wire = backend().make_wire(&edges).map_err(kernel_error)?;
    let face = backend().make_face(&[wire]).map_err(kernel_error)?;
    Py::new(py, PyFace::initializer(face))
}

/// The backing surface of a face (`Face.Surface`). Only `TypeId` is carried in
/// this slice; geometry-specific properties come later.
#[pyclass(name = "GeomSurface", module = "Part")]
pub struct PySurface {
    type_id: String,
}

impl PySurface {
    pub(crate) fn new(type_id: String) -> Self {
        Self { type_id }
    }
}

#[pymethods]
impl PySurface {
    #[getter]
    fn TypeId(&self) -> String {
        self.type_id.clone()
    }

    fn __repr__(&self) -> String {
        format!("Part.{}()", self.type_id.trim_start_matches("Part::"))
    }
}

/// `Part.BSplineCurve()` — an interpolating B-spline, a curve object like
/// `Part.LineSegment`; `toShape()` turns it into a `Part.Edge`.
///
/// The curve itself is OCCT's `Geom_BSplineCurve`. Rather than reimplementing
/// B-spline evaluation in Rust, the binding keeps the state OCCT reports back
/// (poles, weights, knots, multiplicities, degree, periodic/rational flag), so
/// `Degree`, `KnotSequence`, `getPole` … and `toShape` can never disagree, and a
/// mutation (`setKnot`, `setPole`, `setPeriodic`) is followed by a fresh read.
#[pyclass(name = "BSplineCurve", module = "Part")]
pub struct PyBSplineCurve {
    /// `None` for `Part.BSplineCurve()`, which has no geometry yet.
    state: Mutex<Option<SplineState>>,
}

/// The flattened state a B-spline reports, in the layout `fc_curve.hxx`'s
/// `fc_bspline_push_state` writes.
#[derive(Clone)]
struct SplineState {
    degree: i32,
    max_degree: i32,
    periodic: bool,
    rational: bool,
    start: Vector3,
    end: Vector3,
    first_parameter: f64,
    last_parameter: f64,
    first_u_knot: i32,
    last_u_knot: i32,
    continuity: i32,
    knot_sequence: Vec<f64>,
    knots: Vec<f64>,
    mults: Vec<i32>,
    poles: Vec<Vector3>,
    weights: Vec<f64>,
}

/// A cursor over the flat `f64` state `fc_curve.hxx` returns.
struct StateReader<'a> {
    data: &'a [f64],
    at: usize,
}

impl<'a> StateReader<'a> {
    fn new(data: &'a [f64]) -> Self {
        Self { data, at: 0 }
    }

    fn number(&mut self) -> Option<f64> {
        let value = *self.data.get(self.at)?;
        self.at += 1;
        Some(value)
    }

    fn integer(&mut self) -> Option<i32> {
        Some(self.number()? as i32)
    }

    fn count(&mut self) -> Option<usize> {
        Some(self.integer()?.max(0) as usize)
    }

    fn point(&mut self) -> Option<Vector3> {
        Some(Vector3::new(self.number()?, self.number()?, self.number()?))
    }

    fn numbers(&mut self, count: usize) -> Option<Vec<f64>> {
        let end = self.at.checked_add(count)?;
        let values = self.data.get(self.at..end)?;
        self.at = end;
        Some(values.to_vec())
    }
}

impl SplineState {
    fn from_blob(blob: &[f64]) -> Option<Self> {
        let mut reader = StateReader::new(blob);
        let degree = reader.integer()?;
        let max_degree = reader.integer()?;
        let _nb_poles = reader.integer()?;
        let _nb_knots = reader.integer()?;
        let periodic = reader.number()? != 0.0;
        let rational = reader.number()? != 0.0;
        let start = reader.point()?;
        let end = reader.point()?;
        let first_parameter = reader.number()?;
        let last_parameter = reader.number()?;
        let first_u_knot = reader.integer()?;
        let last_u_knot = reader.integer()?;
        let continuity = reader.integer()?;
        let sequence_length = reader.count()?;
        let knot_sequence = reader.numbers(sequence_length)?;
        let knot_count = reader.count()?;
        let knots = reader.numbers(knot_count)?;
        let mult_count = reader.count()?;
        let mults = reader
            .numbers(mult_count)?
            .into_iter()
            .map(|value| value as i32)
            .collect();
        let pole_count = reader.count()?;
        let mut poles = Vec::with_capacity(pole_count);
        for _ in 0..pole_count {
            poles.push(reader.point()?);
        }
        let weight_count = reader.count()?;
        let weights = reader.numbers(weight_count)?;
        Some(Self {
            degree,
            max_degree,
            periodic,
            rational,
            start,
            end,
            first_parameter,
            last_parameter,
            first_u_knot,
            last_u_knot,
            continuity,
            knot_sequence,
            knots,
            mults,
            poles,
            weights,
        })
    }

    /// Rebuild `checkRational` for the stored curve: a rational curve must keep
    /// its weights, a non-rational one is reconstructed from unit weights.
    fn check_rational(&self) -> bool {
        !self.rational
    }

    /// The name upstream's `Continuity` reports for a `GeomAbs_Shape` value.
    fn continuity_name(&self) -> &'static str {
        match self.continuity {
            0 => "C0",
            1 => "G1",
            2 => "C1",
            3 => "G2",
            4 => "C2",
            5 => "C3",
            _ => "CN",
        }
    }
}

/// The B-spline entry points the `ferrocad_geom` seam does not carry (explicit
/// knots, read-back state, periodicity conversion, resolution). `Part` is the
/// OCCT workbench, so its curve binding talks to the OCCT backend directly.
fn occt() -> ferrocad_occt::OcctBackend {
    ferrocad_occt::OcctBackend::new()
}

/// A conic the Part binding converts to B-splines: a full circle or ellipse.
///
/// `toNurbs`/`toBSpline`/`trim` on both classes share the conversion, and only
/// the frame (two equal radii for a circle) distinguishes them.
enum Conic {
    Circle(Circle),
    Ellipse(Ellipse),
}

impl Conic {
    /// `(kind, centre, normal, x axis, major radius, minor radius)`; `kind` is
    /// the `fc_conic_to_spline` tag (2 = circle, 3 = ellipse).
    fn frame(&self) -> (i32, Vector3, Vector3, Vector3, f64, f64) {
        match self {
            Conic::Circle(circle) => (
                2,
                circle.center(),
                circle.normal(),
                circle.x_axis(),
                circle.radius(),
                circle.radius(),
            ),
            Conic::Ellipse(ellipse) => (
                3,
                ellipse.center(),
                ellipse.normal(),
                ellipse.x_axis(),
                ellipse.major_radius(),
                ellipse.minor_radius(),
            ),
        }
    }
}

/// Whether the range is a whole conic, the case upstream's `GeomCircle::toNurbs`
/// and `GeomEllipse::toNurbs` answer with their seven-pole rational cubic rather
/// than the generic trimmed-curve conversion (they compare `first`/`last` with
/// `0`/`2π` exactly, too).
fn is_full_conic(first: f64, last: f64) -> bool {
    first == 0.0 && last == std::f64::consts::TAU
}

/// Upstream's whole-conic `toNurbs`: a degree-3 rational B-spline with seven
/// poles, the end weights raised to 3, and knots `0, π, 2π` for a circle (`0, 1,
/// 2` for an ellipse — upstream's own asymmetry).
///
/// The poles are given in the conic's local frame and mapped through its
/// `gp_Ax3`, so parameter 0 is the point on the x axis and the curve starts on
/// `value(0)` exactly.
fn full_conic_nurbs(conic: &Conic) -> PyResult<SplineState> {
    let (_, center, normal, x_axis, major, minor) = conic.frame();
    let y_axis = normal.cross(&x_axis);
    let local = [
        (major, 0.0),
        (major, 2.0 * minor),
        (-major, 2.0 * minor),
        (-major, 0.0),
        (-major, -2.0 * minor),
        (major, -2.0 * minor),
        (major, 0.0),
    ];
    let poles: Vec<Vector3> = local
        .iter()
        .map(|(x, y)| center.add(&x_axis.scale(*x)).add(&y_axis.scale(*y)))
        .collect();
    let weights = vec![3.0, 1.0, 1.0, 3.0, 1.0, 1.0, 3.0];
    let (knots, mults) = match conic {
        Conic::Circle(_) => (
            vec![0.0, std::f64::consts::PI, std::f64::consts::TAU],
            vec![4, 3, 4],
        ),
        Conic::Ellipse(_) => (vec![0.0, 1.0, 2.0], vec![4, 3, 4]),
    };
    let blob = occt()
        .bspline_state(&poles, &weights, &knots, &mults, 3, false, true)
        .ok_or_else(|| OCCError::new_err("conversion to a NURBS failed"))?;
    SplineState::from_blob(&blob)
        .ok_or_else(|| OCCError::new_err("conversion to a NURBS failed"))
}

/// `conic.toNurbs(first, last)` / `conic.toBSpline(first, last)` as a
/// `Part.BSplineCurve`.
///
/// `nurbs` selects the exact rational conversion (upstream's
/// `GeomConic::toNurbs`) over the approximating `GeomCurve::toBSpline`.
fn conic_spline(
    py: Python<'_>,
    conic: &Conic,
    first: f64,
    last: f64,
    nurbs: bool,
) -> PyResult<Py<PyBSplineCurve>> {
    if nurbs && is_full_conic(first, last) {
        return Py::new(py, PyBSplineCurve::from_state(full_conic_nurbs(conic)?));
    }
    let (kind, center, normal, x_axis, major, minor) = conic.frame();
    let blob = occt()
        .conic_spline_state(kind, center, normal, x_axis, major, minor, first, last, nurbs)
        .ok_or_else(|| OCCError::new_err("conversion to a B-spline failed"))?;
    let state = SplineState::from_blob(&blob)
        .ok_or_else(|| OCCError::new_err("conversion to a B-spline failed"))?;
    Py::new(py, PyBSplineCurve::from_state(state))
}

fn parse_points(points: &Bound<'_, PyAny>) -> PyResult<Vec<Vector3>> {
    let mut parsed = Vec::new();
    for point in points.try_iter()? {
        parsed.push(vector(&point?)?);
    }
    Ok(parsed)
}

fn parse_floats(values: &Bound<'_, PyAny>) -> PyResult<Vec<f64>> {
    let mut parsed = Vec::new();
    for value in values.try_iter()? {
        parsed.push(value?.extract::<f64>()?);
    }
    Ok(parsed)
}

fn parse_integers(values: &Bound<'_, PyAny>) -> PyResult<Vec<i32>> {
    let mut parsed = Vec::new();
    for value in values.try_iter()? {
        parsed.push(value?.extract::<i32>()?);
    }
    Ok(parsed)
}

/// Build the state from poles (and optional multiplicities, knots and weights),
/// mirroring `BSplineCurvePy::buildFromPolesMultsKnots` step by step: the same
/// guesses for an omitted knot vector, the same multiplicity checks and the same
/// "number of poles and sum of mults" rule.
fn build_spline(
    poles: &Bound<'_, PyAny>,
    mults: Option<&Bound<'_, PyAny>>,
    knots: Option<&Bound<'_, PyAny>>,
    periodic: bool,
    mut degree: i32,
    weights: Option<&Bound<'_, PyAny>>,
    check_rational: bool,
) -> PyResult<SplineState> {
    let poles = parse_points(poles)?;
    if poles.len() < 2 {
        return Err(OCCError::new_err("need two or more poles"));
    }
    let given_mults = mults.map(parse_integers).transpose()?;
    let given_knots = knots.map(parse_floats).transpose()?;

    let knot_count = match (&given_mults, &given_knots) {
        (Some(mults), Some(knots)) => {
            if mults.len() != knots.len() {
                return Err(OCCError::new_err("number of knots and mults mismatch"));
            }
            mults.len()
        }
        (Some(mults), None) => mults.len(),
        (None, Some(knots)) => knots.len(),
        (None, None) => {
            let count = poles.len() as i32;
            if periodic {
                if count < degree {
                    degree = count;
                }
                (count + 1) as usize
            }
            else {
                if count <= degree {
                    degree = count - 1;
                }
                (count - degree + 1) as usize
            }
        }
    };
    if knot_count == 0 || degree < 1 {
        return Err(OCCError::new_err("failed to create spline"));
    }

    let mults = match given_mults {
        Some(mults) => mults,
        None => {
            let mut mults = vec![1; knot_count];
            if !periodic {
                mults[0] = degree + 1;
                mults[knot_count - 1] = degree + 1;
            }
            mults
        }
    };
    let sum_of_mults: i32 = if periodic {
        mults[..mults.len().saturating_sub(1)].iter().sum()
    }
    else {
        mults.iter().sum()
    };
    if mults.len() >= 3 {
        for multiplicity in &mults[1..mults.len() - 1] {
            if *multiplicity > degree {
                return Err(OCCError::new_err(
                    "multiplicity of inner knot higher than degree",
                ));
            }
        }
    }

    let knots = match given_knots {
        Some(knots) => knots,
        None => {
            if knot_count == 1 {
                vec![0.0]
            }
            else {
                (0..knot_count)
                    .map(|index| index as f64 / (knot_count - 1) as f64)
                    .collect()
            }
        }
    };

    let weights = match weights.map(parse_floats).transpose()? {
        Some(weights) => {
            if weights.len() != poles.len() {
                return Err(OCCError::new_err("number of poles and weights mismatch"));
            }
            weights
        }
        None => vec![1.0; poles.len()],
    };

    let pole_count = poles.len() as i32;
    let consistent = if periodic {
        sum_of_mults == pole_count
    }
    else {
        sum_of_mults - degree - 1 == pole_count
    };
    if !consistent {
        return Err(OCCError::new_err("number of poles and sum of mults mismatch"));
    }

    let blob = occt()
        .bspline_state(&poles, &weights, &knots, &mults, degree, periodic, check_rational)
        .ok_or_else(|| OCCError::new_err("failed to create spline"))?;
    SplineState::from_blob(&blob).ok_or_else(|| OCCError::new_err("failed to create spline"))
}

#[pymethods]
impl PyBSplineCurve {
    /// `Part.BSplineCurve()` (empty) or
    /// `Part.BSplineCurve(poles, mults=None, knots=None, periodic=False,
    /// degree=3, weights=None, CheckRational=True)`.
    ///
    /// Upstream's `PyInit` delegates to `buildFromPolesMultsKnots`, so the
    /// constructor is the same parser with positional arguments only.
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        if args.len() == 0 {
            return Ok(Self {
                state: Mutex::new(None),
            });
        }
        if args.len() > 7 {
            return Err(PyTypeError::new_err(
                "B-spline constructor accepts:\n\
                 -- poles, [mults, knots, periodic, degree, weights, CheckRational]\n\
                 -- empty parameter list",
            ));
        }
        let optional = |index: usize| -> PyResult<Option<Bound<'_, PyAny>>> {
            match args.get_item(index) {
                Ok(value) if !value.is_none() => Ok(Some(value)),
                _ => Ok(None),
            }
        };
        let poles = args.get_item(0)?;
        let mults = optional(1)?;
        let knots = optional(2)?;
        let periodic = args
            .get_item(3)
            .ok()
            .and_then(|value| value.extract::<bool>().ok())
            .unwrap_or(false);
        let degree = args
            .get_item(4)
            .ok()
            .and_then(|value| value.extract::<i32>().ok())
            .unwrap_or(3);
        let weights = optional(5)?;
        let check_rational = args
            .get_item(6)
            .ok()
            .and_then(|value| value.extract::<bool>().ok())
            .unwrap_or(true);
        let state = build_spline(
            &poles,
            mults.as_ref(),
            knots.as_ref(),
            periodic,
            degree,
            weights.as_ref(),
            check_rational,
        )?;
        Ok(Self {
            state: Mutex::new(Some(state)),
        })
    }

    /// Build the curve from poles, multiplicities and knots:
    /// `buildFromPolesMultsKnots(poles, mults=None, knots=None, periodic=False,
    /// degree=3, weights=None, CheckRational=True)`.
    ///
    /// With no knots the vector is uniform on `[0, 1]`; with no multiplicities
    /// every inner knot is simple and the end knots carry `degree + 1`.
    #[pyo3(signature = (poles, mults = None, knots = None, periodic = false, degree = 3, weights = None, CheckRational = true))]
    fn buildFromPolesMultsKnots(
        &self,
        poles: &Bound<'_, PyAny>,
        mults: Option<&Bound<'_, PyAny>>,
        knots: Option<&Bound<'_, PyAny>>,
        periodic: bool,
        degree: i32,
        weights: Option<&Bound<'_, PyAny>>,
        CheckRational: bool,
    ) -> PyResult<()> {
        let state = build_spline(poles, mults, knots, periodic, degree, weights, CheckRational)?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }

    /// Build the curve from control points:
    /// `buildFromPoles(poles, periodic=False, degree=3, interpolate=False)`.
    ///
    /// As upstream does, the degree is capped at `len(poles) - 1`. `interpolate`
    /// only changes the periodic knot structure (upstream never interpolates
    /// here); use [`interpolate`](Self::interpolate) for that.
    #[pyo3(signature = (poles, periodic = false, degree = 3, interpolate = false))]
    fn buildFromPoles(
        &self,
        poles: &Bound<'_, PyAny>,
        periodic: bool,
        degree: i32,
        interpolate: bool,
    ) -> PyResult<()> {
        let parsed = parse_points(poles)?;
        if parsed.len() < 2 {
            return Err(OCCError::new_err("a B-spline needs at least two poles"));
        }
        let degree = degree.min(parsed.len() as i32 - 1).max(1);
        let mult = if periodic {
            if interpolate {
                degree
            }
            else {
                1
            }
        }
        else {
            degree + 1
        };
        let knot_count = if periodic {
            if interpolate {
                parsed.len() as i32 - degree + 2
            }
            else {
                parsed.len() as i32 + 1
            }
        }
        else {
            parsed.len() as i32 - degree + 1
        };
        let knot_count = knot_count.max(2) as usize;
        let knots: Vec<f64> = (0..knot_count)
            .map(|index| index as f64 / (knot_count - 1) as f64)
            .collect();
        let mut mults = vec![1; knot_count];
        mults[0] = mult;
        mults[knot_count - 1] = mult;
        let weights = vec![1.0; parsed.len()];
        let blob = occt()
            .bspline_state(
                &parsed,
                &weights,
                &knots,
                &mults,
                degree,
                periodic,
                true,
            )
            .ok_or_else(|| OCCError::new_err("failed to create spline"))?;
        let state =
            SplineState::from_blob(&blob).ok_or_else(|| OCCError::new_err("failed to create spline"))?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }

    /// Interpolate a B-spline through `points` (`GeomAPI_Interpolate`).
    #[pyo3(signature = (points, PeriodicFlag = false, Tolerance = 1e-7))]
    fn interpolate(
        &self,
        points: &Bound<'_, PyAny>,
        PeriodicFlag: bool,
        Tolerance: f64,
    ) -> PyResult<()> {
        let _ = Tolerance;
        let parsed = parse_points(points)?;
        if parsed.len() < 2 {
            return Err(OCCError::new_err(
                "a B-spline needs at least two interpolation points",
            ));
        }
        let blob = occt()
            .bspline_interpolated_state(&parsed, PeriodicFlag)
            .ok_or_else(|| OCCError::new_err("failed to interpolate the B-spline"))?;
        let state = SplineState::from_blob(&blob)
            .ok_or_else(|| OCCError::new_err("failed to interpolate the B-spline"))?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }

    /// The curve as an edge.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        let state = self.state()?;
        let shape = occt()
            .make_bspline_poles_mults_knots(
                &state.poles,
                &state.weights,
                &state.knots,
                &state.mults,
                state.degree,
                state.periodic,
                state.check_rational(),
            )
            .map_err(kernel_error)?;
        Py::new(
            py,
            PyEdge::from_entry(EdgeEntry {
                shape,
                curve: None,
            }),
        )
    }

    /// `value(u)`: the point at parameter `u` (`Geom_BSplineCurve::Value`).
    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        let state = self.state()?;
        let point = occt()
            .bspline_point(
                &state.poles,
                &state.weights,
                &state.knots,
                &state.mults,
                state.degree,
                state.periodic,
                state.check_rational(),
                u,
            )
            .ok_or_else(|| OCCError::new_err("failed to evaluate the B-spline"))?;
        py_vector(py, point)
    }

    #[getter]
    fn Degree(&self) -> PyResult<i32> {
        Ok(self.state()?.degree)
    }

    #[getter]
    fn MaxDegree(&self) -> PyResult<i32> {
        Ok(self.state()?.max_degree)
    }

    #[getter]
    fn NbPoles(&self) -> PyResult<usize> {
        Ok(self.state()?.poles.len())
    }

    #[getter]
    fn NbKnots(&self) -> PyResult<usize> {
        Ok(self.state()?.knots.len())
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.state()?.start)
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.state()?.end)
    }

    #[getter]
    fn FirstParameter(&self) -> PyResult<f64> {
        Ok(self.state()?.first_parameter)
    }

    #[getter]
    fn LastParameter(&self) -> PyResult<f64> {
        Ok(self.state()?.last_parameter)
    }

    #[getter]
    fn FirstUKnotIndex(&self) -> PyResult<i32> {
        Ok(self.state()?.first_u_knot)
    }

    #[getter]
    fn LastUKnotIndex(&self) -> PyResult<i32> {
        Ok(self.state()?.last_u_knot)
    }

    #[getter]
    fn KnotSequence(&self) -> PyResult<Vec<f64>> {
        Ok(self.state()?.knot_sequence)
    }

    /// The continuity (`GeomAbs_C0` … `GeomAbs_CN`), as upstream reports it.
    #[getter]
    fn Continuity(&self) -> PyResult<&'static str> {
        Ok(self.state()?.continuity_name())
    }

    fn isPeriodic(&self) -> PyResult<bool> {
        Ok(self.state()?.periodic)
    }

    fn isRational(&self) -> PyResult<bool> {
        Ok(self.state()?.rational)
    }

    fn getKnot(&self, index: i32) -> PyResult<f64> {
        let state = self.state()?;
        knot_at(&state, index)
    }

    fn getKnots(&self) -> PyResult<Vec<f64>> {
        Ok(self.state()?.knots)
    }

    fn getMultiplicity(&self, index: i32) -> PyResult<i32> {
        let state = self.state()?;
        let position = index_at(state.mults.len(), index)? as usize;
        Ok(state.mults[position - 1])
    }

    fn getMultiplicities(&self) -> PyResult<Vec<i32>> {
        Ok(self.state()?.mults)
    }

    fn getPole(&self, py: Python<'_>, index: i32) -> PyResult<PyObject> {
        let state = self.state()?;
        let position = index_at(state.poles.len(), index)? as usize - 1;
        py_vector(py, state.poles[position])
    }

    fn getPoles(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let state = self.state()?;
        state
            .poles
            .iter()
            .map(|pole| py_vector(py, *pole))
            .collect()
    }

    /// Each pole as `(x, y, z, weight)`.
    fn getPolesAndWeights(&self) -> PyResult<Vec<(f64, f64, f64, f64)>> {
        let state = self.state()?;
        Ok(state
            .poles
            .iter()
            .zip(state.weights.iter())
            .map(|(pole, weight)| (pole.x, pole.y, pole.z, *weight))
            .collect())
    }

    fn getWeight(&self, index: i32) -> PyResult<f64> {
        let state = self.state()?;
        let position = index_at(state.weights.len(), index)? as usize - 1;
        Ok(state.weights[position])
    }

    fn getWeights(&self) -> PyResult<Vec<f64>> {
        Ok(self.state()?.weights)
    }

    /// The parameter tolerance equivalent to a 3D `tolerance`
    /// (`Geom_BSplineCurve::Resolution`).
    fn getResolution(&self, tolerance: f64) -> PyResult<f64> {
        let state = self.state()?;
        occt()
            .bspline_resolution(
                &state.poles,
                &state.weights,
                &state.knots,
                &state.mults,
                state.degree,
                state.periodic,
                state.check_rational(),
                tolerance,
            )
            .ok_or_else(|| OCCError::new_err("failed to compute the spline resolution"))
    }

    /// `setKnot(Index, Value, M=-1)`: assign unique knot `Index` (and optionally
    /// its multiplicity).
    #[pyo3(signature = (index, value, multiplicity = None))]
    fn setKnot(&self, index: i32, value: f64, multiplicity: Option<i32>) -> PyResult<()> {
        let mut state = self.state()?;
        let position = index_at(state.knots.len(), index)? as usize - 1;
        // OCCT keeps the unique knots ordered: the new value must stay strictly
        // between its neighbours (`Geom_BSplineCurve::SetKnot`).
        if position > 0 && value <= state.knots[position - 1] {
            return Err(OCCError::new_err("knot value is not increasing"));
        }
        if position + 1 < state.knots.len() && value >= state.knots[position + 1] {
            return Err(OCCError::new_err("knot value is not increasing"));
        }
        state.knots[position] = value;
        if let Some(multiplicity) = multiplicity {
            // OCCT allows increasing a multiplicity, never decreasing it.
            if multiplicity < state.mults[position] || multiplicity > state.degree {
                return Err(OCCError::new_err("invalid knot multiplicity"));
            }
            state.mults[position] = multiplicity;
        }
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }

    /// `setPole(Index, Point, Weight=-1)`: move pole `Index` (and optionally set
    /// its weight).
    #[pyo3(signature = (index, point, weight = None))]
    fn setPole(
        &self,
        index: i32,
        point: &Bound<'_, PyAny>,
        weight: Option<f64>,
    ) -> PyResult<()> {
        let mut state = self.state()?;
        let position = index_at(state.poles.len(), index)? as usize - 1;
        state.poles[position] = vector(point)?;
        if let Some(weight) = weight {
            if !(weight > 0.0) {
                return Err(OCCError::new_err("weight must be positive"));
            }
            state.weights[position] = weight;
            state.rational = true;
        }
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }

    /// Convert to the periodic form (`Geom_BSplineCurve::SetPeriodic`).
    fn setPeriodic(&self) -> PyResult<()> {
        self.set_periodicity(true)
    }

    /// Convert out of the periodic form (`Geom_BSplineCurve::SetNotPeriodic`).
    fn setNotPeriodic(&self) -> PyResult<()> {
        self.set_periodicity(false)
    }

    /// `copy.copy` on the curve; upstream implements `__reduce__` for the same.
    fn __copy__(&self) -> Self {
        Self {
            state: Mutex::new(self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()),
        }
    }

    fn __repr__(&self) -> String {
        match &*self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(state) => format!(
                "Part.BSplineCurve({} poles, degree {})",
                state.poles.len(),
                state.degree
            ),
            None => "Part.BSplineCurve()".to_string(),
        }
    }
}

impl PyBSplineCurve {
    /// A spline that already carries OCCT's state (what `toNurbs`/`toBSpline`
    /// on the analytic curves return).
    fn from_state(state: SplineState) -> Self {
        Self {
            state: Mutex::new(Some(state)),
        }
    }

    fn state(&self) -> PyResult<SplineState> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| OCCError::new_err("the B-spline is empty"))
    }

    fn set_periodicity(&self, target: bool) -> PyResult<()> {
        let state = self.state()?;
        let blob = occt()
            .bspline_periodic_state(
                &state.poles,
                &state.weights,
                &state.knots,
                &state.mults,
                state.degree,
                state.periodic,
                state.check_rational(),
                target,
            )
            .ok_or_else(|| OCCError::new_err("failed to change the spline's periodicity"))?;
        let state = SplineState::from_blob(&blob)
            .ok_or_else(|| OCCError::new_err("failed to change the spline's periodicity"))?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        Ok(())
    }
}

/// Translate a 1-based OCCT index, reporting a bad one as `Part.OCCError` (which
/// is what upstream's caught `Standard_OutOfRange` becomes).
fn index_at(length: usize, index: i32) -> PyResult<i32> {
    if index < 1 || index as usize > length {
        return Err(OCCError::new_err("index out of range"));
    }
    Ok(index)
}

fn knot_at(state: &SplineState, index: i32) -> PyResult<f64> {
    let position = index_at(state.knots.len(), index)? as usize;
    Ok(state.knots[position - 1])
}

// ---------------------------------------------------------------------------
// 2D geometry (Part.Geom2d)
// ---------------------------------------------------------------------------

/// `Part.Geom2d.Line2dSegment(p1, p2)` — a bounded line segment in 2D parameter
/// space (upstream's `Part::Geom2dLineSegment`).
///
/// Points are `(u, v)` pairs or objects with `x`/`y` (`FreeCAD.Base.Vector2d`).
#[pyclass(name = "Line2dSegment", module = "Part.Geom2d")]
#[derive(Clone, Copy)]
pub struct PyLine2dSegment {
    start: (f64, f64),
    end: (f64, f64),
}

#[pymethods]
impl PyLine2dSegment {
    #[new]
    fn new(start: &Bound<'_, PyAny>, end: &Bound<'_, PyAny>) -> PyResult<Self> {
        let start = vector2d(start)?;
        let end = vector2d(end)?;
        if start == end {
            return Err(OCCError::new_err(
                "Part.Geom2d.Line2dSegment needs two distinct points",
            ));
        }
        Ok(Self { start, end })
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector2d(py, self.start)
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector2d(py, self.end)
    }

    /// The segment as a `Part.Edge`.
    ///
    /// Without `surface` this is a real 3D edge in the XY plane: the 2D segment is
    /// lifted to `(u, v, 0)` and built through the kernel's `make_edge`, and the
    /// 2D segment is attached as the edge's pcurve (upstream builds a 2D-only edge
    /// here instead).
    ///
    /// With `surface` the same edge is returned, with the same pcurve attached.
    /// **The curve is not projected onto the surface in this slice** — there is no
    /// `GeomProjLib` binding, so on a `Part.Cylinder` the coordinates are not
    /// wrapped around the axis; passing the surface only validates its type and
    /// records the association the caller asked for. `curveOnSurface(0)` reports
    /// the 2D segment in both forms.
    #[pyo3(signature = (surface = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        surface: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyEdge>> {
        if let Some(surface) = surface {
            require_surface(surface)?;
        }
        let segment = LineSegment::new(
            Vector3::new(self.start.0, self.start.1, 0.0),
            Vector3::new(self.end.0, self.end.1, 0.0),
        )
        .map_err(curve_error)?;
        let curve = EdgeCurve::LineSegment(segment);
        let entry = EdgeEntry {
            shape: shape_from_curve(&curve)?,
            curve: Some(curve),
        };
        Ok(Py::new(
            py,
            PyEdge::from_entry_with_curve2d(entry, Curve2d::LineSegment(*self)),
        )?)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Geom2d.Line2dSegment({:?}, {:?})",
            self.start, self.end
        )
    }
}

/// A 2D curve (pcurve) attached to an edge. Only the segment exists in this
/// slice.
#[derive(Clone, Copy)]
pub(crate) enum Curve2d {
    LineSegment(PyLine2dSegment),
}

/// `Part.Geom2d.BSplineCurve2d()` — upstream's 2D B-spline curve.
///
/// No 2D B-spline kernel is bound in this slice, so `interpolate` and
/// `approximate` only **record** the points they are given; no curve is computed
/// behind that record. Every operation that would need the actual curve
/// (`insertKnot`) raises `Part.OCCError` rather than answering from a curve that
/// was never built.
///
/// The four-argument call upstream's `Geom2d_tests` uses
/// (`insertKnot(0.5, 1, 0.01, 2)`) is rejected by the signature as a `TypeError`,
/// just as upstream's `PyArg_ParseTuple("d|id", …)` rejects the extra argument.
#[pyclass(name = "BSplineCurve2d", module = "Part.Geom2d")]
pub struct PyBSplineCurve2d {
    /// The interpolation/approximation points last supplied.
    points: Mutex<Vec<(f64, f64)>>,
}

impl PyBSplineCurve2d {
    fn point_count(&self) -> usize {
        self.points.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Parse and remember the `Points` argument of `interpolate`/`approximate`.
    fn record(&self, points: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut parsed = Vec::new();
        for point in points.try_iter()? {
            parsed.push(vector2d(&point?)?);
        }
        if parsed.len() < 2 {
            return Err(OCCError::new_err(
                "a 2D B-spline needs at least two points",
            ));
        }
        *self.points.lock().unwrap_or_else(|e| e.into_inner()) = parsed;
        Ok(())
    }
}

#[pymethods]
impl PyBSplineCurve2d {
    #[new]
    fn new() -> Self {
        Self {
            points: Mutex::new(Vec::new()),
        }
    }

    /// `interpolate(Points, PeriodicFlag=False, Tolerance=1e-7)` — record the
    /// points to interpolate (upstream's `Geom2dAPI_Interpolate`).
    #[pyo3(signature = (Points, PeriodicFlag = false, Tolerance = 1e-7))]
    fn interpolate(
        &self,
        Points: &Bound<'_, PyAny>,
        PeriodicFlag: bool,
        Tolerance: f64,
    ) -> PyResult<()> {
        let _ = (PeriodicFlag, Tolerance);
        self.record(Points)
    }

    /// `approximate(Points, DegMax=8, Continuity="C2", Tolerance=1e-3,
    /// DegMin=3, ParamType="ChordLength")` — record the points to approximate
    /// (upstream's `Geom2dAPI_PointsToBSpline`).
    #[pyo3(signature = (Points, DegMax = 8, Continuity = "C2", Tolerance = 1e-3, DegMin = 3, ParamType = "ChordLength"))]
    fn approximate(
        &self,
        Points: &Bound<'_, PyAny>,
        DegMax: i32,
        Continuity: &str,
        Tolerance: f64,
        DegMin: i32,
        ParamType: &str,
    ) -> PyResult<()> {
        let _ = (DegMax, Continuity, Tolerance, DegMin, ParamType);
        self.record(Points)
    }

    #[pyo3(signature = (u, mult = 1, tol = 0.0))]
    fn insertKnot(&self, u: f64, mult: i32, tol: f64) -> PyResult<()> {
        let _ = (u, mult, tol);
        Err(OCCError::new_err(
            "Part.Geom2d.BSplineCurve2d.insertKnot is not implemented \
             (this slice binds no 2D B-spline kernel)",
        ))
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Geom2d.BSplineCurve2d({} points)",
            self.point_count()
        )
    }
}

/// `Part.Geom2d.Circle2d(center=(0, 0), radius=1)` — a circle in 2D parameter
/// space (upstream's `Part::Geom2dCircle`).
///
/// The three-point constructor and the offset constructors are not modelled;
/// the static [`getCircleCenter`](Self::getCircleCenter) is, because upstream's
/// tests use it (and its `ValueError` for collinear points).
#[pyclass(name = "Circle2d", module = "Part.Geom2d")]
#[derive(Clone, Copy)]
pub struct PyCircle2d {
    center: (f64, f64),
    radius: f64,
}

/// The centre of the circle through three 2D points, using upstream's
/// `Geom2dCircle::getCircleCenter` arithmetic and tolerances.
fn circle_center(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64)) -> PyResult<(f64, f64)> {
    let sub = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0, a.1 - b.1);
    let dot = |a: (f64, f64), b: (f64, f64)| a.0 * b.0 + a.1 * b.1;
    let u = sub(p2, p1);
    let v = sub(p3, p2);
    let w = sub(p1, p3);
    let (uu, vv, ww) = (dot(u, u), dot(v, v), dot(w, w));
    // `Precision::SquareConfusion()` in OCCT.
    let eps2 = 1e-14;
    if uu < eps2 || vv < eps2 || ww < eps2 {
        return Err(PyValueError::new_err("Two points are coincident"));
    }
    let uv = -dot(u, v);
    let vw = -dot(v, w);
    let uw = -dot(u, w);
    let w0 = 2.0 * (uu * ww - uw * uw).abs().sqrt() * uw / (uu * ww);
    let w1 = 2.0 * (uu * vv - uv * uv).abs().sqrt() * uv / (uu * vv);
    let w2 = 2.0 * (vv * ww - vw * vw).abs().sqrt() * vw / (vv * ww);
    let wx = w0 + w1 + w2;
    // `Precision::Confusion()` in OCCT.
    if wx.abs() < 1e-7 {
        return Err(PyValueError::new_err("Points are collinear"));
    }
    Ok((
        (w0 * p1.0 + w1 * p2.0 + w2 * p3.0) / wx,
        (w0 * p1.1 + w1 * p2.1 + w2 * p3.1) / wx,
    ))
}

#[pymethods]
impl PyCircle2d {
    #[new]
    #[pyo3(signature = (center = None, radius = 1.0))]
    fn new(center: Option<&Bound<'_, PyAny>>, radius: f64) -> PyResult<Self> {
        if !radius.is_finite() || radius <= 0.0 {
            return Err(OCCError::new_err(
                "Part.Geom2d.Circle2d needs a positive radius",
            ));
        }
        let center = center.map(vector2d).transpose()?.unwrap_or((0.0, 0.0));
        Ok(Self { center, radius })
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector2d(py, self.center)
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.radius
    }

    /// The centre of the circle through three points, as upstream's
    /// `Geom2dCircle::getCircleCenter`: coincident points and collinear points
    /// are `ValueError`.
    #[staticmethod]
    fn getCircleCenter(
        py: Python<'_>,
        p1: &Bound<'_, PyAny>,
        p2: &Bound<'_, PyAny>,
        p3: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let center = circle_center(vector2d(p1)?, vector2d(p2)?, vector2d(p3)?)?;
        py_vector2d(py, center)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Geom2d.Circle2d({:?}, {})",
            self.center, self.radius
        )
    }
}

/// Reject anything that is not a surface this slice models.
///
/// The surface is not used yet (no projection exists), so this is a type check:
/// it keeps a bogus `toShape(42)` a `TypeError` instead of a silent success.
fn require_surface(value: &Bound<'_, PyAny>) -> PyResult<()> {
    if value.downcast::<PyCylinder>().is_ok()
        || value.downcast::<PySphere>().is_ok()
        || value.downcast::<PyPlane>().is_ok()
        || value.downcast::<PyRectangularTrimmedSurface>().is_ok()
        || value.downcast::<PySurface>().is_ok()
    {
        return Ok(());
    }
    Err(PyTypeError::new_err(
        "toShape(surface) expects a Part surface \
         (Part.Plane, Part.Cylinder, Part.Sphere or Part.RectangularTrimmedSurface)",
    ))
}

// ---------------------------------------------------------------------------
// Edges and wires
// ---------------------------------------------------------------------------

/// Upstream's error for a geometric query on an empty (null) edge or face: every
/// query on the null shape is a `ValueError` (`EmptyEdge`/`EmptyFace` upstream).
fn empty_error(what: &str) -> PyErr {
    PyValueError::new_err(format!("{what} of an empty shape"))
}

/// `require_edge` accepts a real edge or face; the empty one raises upstream's
/// `ValueError`.
fn require_edge(shape: &PartShape, what: &str) -> PyResult<()> {
    if shape.shape().is_none() {
        return Err(empty_error(what));
    }
    Ok(())
}

/// A non-empty edge or face cannot answer this query through the current seam.
fn edge_query_error(what: &str) -> PyErr {
    OCCError::new_err(format!(
        "Part.{what} is not available in this slice (the seam exposes no such query)"
    ))
}

/// One edge: a kernel shape plus, when it was built from an analytic curve, that
/// curve (so `Curve` can report the exact parameterisation).
#[derive(Clone)]
pub(crate) struct EdgeEntry {
    pub(crate) shape: Shape,
    pub(crate) curve: Option<EdgeCurve>,
}

/// `Part.Edge(shape_or_curve)` — an edge, a `Part.Shape` subclass as upstream.
#[pyclass(name = "Edge", module = "Part", extends = PartShape)]
pub struct PyEdge {
    /// `None` for the empty edge `Part.Edge()` (upstream's null `TopoDS_Edge`).
    entry: Mutex<Option<EdgeEntry>>,
    /// The 2D curve this edge was built with, reported by `curveOnSurface`.
    /// Kept beside the entry because `EdgeEntry` is also built by the shape layer
    /// (`Edges`/`getSubShape`), which never has a pcurve to supply.
    curve2d: Mutex<Option<Curve2d>>,
}

impl PyEdge {
    fn initializer(entry: EdgeEntry) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(entry.shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(entry)),
            curve2d: Mutex::new(None),
        })
    }

    /// The empty edge `Part.Edge()`: a null shape with no geometry behind it.
    fn null_initializer() -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::empty()).add_subclass(Self {
            entry: Mutex::new(None),
            curve2d: Mutex::new(None),
        })
    }

    pub(crate) fn from_entry(entry: EdgeEntry) -> PyClassInitializer<Self> {
        Self::initializer(entry)
    }

    /// An edge built from a 2D curve, with that curve attached as its pcurve.
    pub(crate) fn from_entry_with_curve2d(
        entry: EdgeEntry,
        curve2d: Curve2d,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(entry.shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(entry)),
            curve2d: Mutex::new(Some(curve2d)),
        })
    }

    /// A copy of the entry (the shape handle is cheap to clone); `None` for the
    /// empty edge.
    pub(crate) fn entry(&self) -> Option<EdgeEntry> {
        self.entry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The geometry behind a Python edge/curve object.
    pub(crate) fn entry_from(value: &Bound<'_, PyAny>) -> PyResult<EdgeEntry> {
        if let Ok(edge) = value.downcast::<PyEdge>() {
            return edge.borrow().entry().ok_or_else(|| {
                PyValueError::new_err("an empty Part.Edge has no geometry")
            });
        }
        if let Ok(line) = value.downcast::<PyLineSegment>() {
            let curve = EdgeCurve::LineSegment(line.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(circle) = value.downcast::<PyCircle>() {
            let curve = EdgeCurve::Circle(circle.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(arc) = value.downcast::<PyArcOfCircle>() {
            let curve = EdgeCurve::Arc(arc.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(shape) = value.downcast::<PartShape>() {
            let inner = shape
                .borrow()
                .shape()
                .ok_or_else(|| OCCError::new_err("a null shape cannot be an edge"))?;
            return Ok(EdgeEntry {
                shape: inner,
                curve: None,
            });
        }
        Err(PyTypeError::new_err(
            "Part.Edge expects a Part.Edge, a curve, or a Part.Shape",
        ))
    }

    /// An edge built from an analytic curve.
    pub(crate) fn from_curve(curve: EdgeCurve) -> PyResult<PyClassInitializer<Self>> {
        Ok(Self::initializer(EdgeEntry {
            shape: shape_from_curve(&curve)?,
            curve: Some(curve),
        }))
    }
}

#[pymethods]
impl PyEdge {
    /// `Part.Edge()` is upstream's empty (null) edge; `Part.Edge(value)` accepts
    /// an edge, a curve or a `Part.Shape`.
    #[new]
    #[pyo3(signature = (value = None))]
    fn new(value: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        match value {
            Some(value) => Ok(Self::initializer(Self::entry_from(value)?)),
            None => Ok(Self::null_initializer()),
        }
    }

    /// The analytic curve (`edge.Curve` upstream). `None` when the kernel cannot
    /// classify it.
    ///
    /// The edge's **basis** curve, as upstream's `TopoShapeEdgePy::getCurve`
    /// reports it: a segment's edge is a `Line`, and a trimmed circular edge is a
    /// `Circle` (not an `ArcOfCircle`) — the span is available through
    /// `FirstParameter`/`LastParameter`. The stored analytic curve is the fallback
    /// for a shape the kernel cannot classify. An empty edge raises `ValueError`,
    /// as upstream.
    #[getter]
    fn Curve(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(entry) = self.entry() else {
            return Err(empty_error("Curve"));
        };
        match backend().curve_of(&entry.shape) {
            Ok(curve) => Ok(Some(curve_to_py(py, curve)?)),
            Err(_) => match entry.curve {
                Some(curve) => Ok(Some(curve_to_py(py, curve)?)),
                None => Ok(None),
            },
        }
    }

    /// The edge as a kernel shape (the empty edge is the null shape).
    fn toShape(slf: PyRef<'_, Self>) -> PartShape {
        slf.as_super().copy_handle()
    }

    /// The 2D curve (pcurve) attached to this edge, or `None`.
    ///
    /// This slice builds at most one pcurve (`index == 0`, and only for edges made
    /// by `Part.Geom2d` curves); every other index, and every edge lifted from a
    /// B-Rep shape, returns `None`. `orientation` is accepted for signature
    /// compatibility and ignored.
    ///
    /// Upstream returns a `(curve, surface, placement, first, last)` tuple here;
    /// only the 2D curve is carried, so this slice returns the curve itself.
    #[pyo3(signature = (index, orientation = 0))]
    fn curveOnSurface(
        &self,
        py: Python<'_>,
        index: i32,
        orientation: i32,
    ) -> PyResult<Option<PyObject>> {
        let _ = orientation;
        if index != 0 {
            return Ok(None);
        }
        let curve2d = *self.curve2d.lock().unwrap_or_else(|e| e.into_inner());
        Ok(match curve2d {
            Some(Curve2d::LineSegment(segment)) => Some(Py::new(py, segment)?.into_any()),
            None => None,
        })
    }

    fn isNull(slf: PyRef<'_, Self>) -> bool {
        slf.as_super().shape().is_none()
    }

    // The query surface below is driven by upstream's `EmptyEdge` tests: on an
    // empty edge each of these is a `ValueError`. This slice has no seam call
    // for them on a real edge either, so a non-empty edge reports `OCCError`
    // instead of inventing an answer.

    /// `edge.Length` — upstream raises `ValueError` for the empty edge; a real
    /// edge reports the kernel's linear properties, as `Part.Shape.Length` does.
    #[getter]
    fn Length(slf: PyRef<'_, Self>) -> PyResult<f64> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("Length"))?;
        Ok(backend()
            .mass_properties(&shape)
            .map(|mass| mass.length)
            .unwrap_or(0.0))
    }

    #[getter]
    fn Tolerance(slf: PyRef<'_, Self>) -> PyResult<f64> {
        require_edge(slf.as_super(), "Tolerance")?;
        Err(OCCError::new_err(
            "Part.Edge.Tolerance is not available in this slice (the seam exposes \
             no edge tolerance)",
        ))
    }

    #[setter]
    fn set_Tolerance(slf: PyRef<'_, Self>, _value: f64) -> PyResult<()> {
        require_edge(slf.as_super(), "Tolerance")?;
        Err(OCCError::new_err(
            "Part.Edge.Tolerance is not available in this slice (the seam exposes \
             no edge tolerance)",
        ))
    }

    #[getter]
    fn ParameterRange(slf: PyRef<'_, Self>) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "ParameterRange")?;
        Err(OCCError::new_err(
            "Part.Edge.ParameterRange is not available in this slice (the seam \
             exposes no parameter range)",
        ))
    }

    #[getter]
    fn FirstParameter(slf: PyRef<'_, Self>) -> PyResult<f64> {
        require_edge(slf.as_super(), "FirstParameter")?;
        Err(OCCError::new_err(
            "Part.Edge.FirstParameter is not available in this slice (the seam \
             exposes no parameter range)",
        ))
    }

    #[getter]
    fn LastParameter(slf: PyRef<'_, Self>) -> PyResult<f64> {
        require_edge(slf.as_super(), "LastParameter")?;
        Err(OCCError::new_err(
            "Part.Edge.LastParameter is not available in this slice (the seam \
             exposes no parameter range)",
        ))
    }

    fn valueAt(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "valueAt")?;
        Err(edge_query_error("valueAt"))
    }

    fn tangentAt(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "tangentAt")?;
        Err(edge_query_error("tangentAt"))
    }

    fn normalAt(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "normalAt")?;
        Err(edge_query_error("normalAt"))
    }

    fn curvatureAt(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<f64> {
        require_edge(slf.as_super(), "curvatureAt")?;
        Err(edge_query_error("curvatureAt"))
    }

    fn centerOfCurvatureAt(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "centerOfCurvatureAt")?;
        Err(edge_query_error("centerOfCurvatureAt"))
    }

    fn derivative1At(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative1At")?;
        Err(edge_query_error("derivative1At"))
    }

    fn derivative2At(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative2At")?;
        Err(edge_query_error("derivative2At"))
    }

    fn derivative3At(slf: PyRef<'_, Self>, _parameter: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative3At")?;
        Err(edge_query_error("derivative3At"))
    }

    fn firstVertex(slf: PyRef<'_, Self>) -> PyResult<Py<PyVertex>> {
        require_edge(slf.as_super(), "firstVertex")?;
        Err(edge_query_error("firstVertex"))
    }

    fn lastVertex(slf: PyRef<'_, Self>) -> PyResult<Py<PyVertex>> {
        require_edge(slf.as_super(), "lastVertex")?;
        Err(edge_query_error("lastVertex"))
    }

    fn getParameterByLength(slf: PyRef<'_, Self>, _length: f64) -> PyResult<f64> {
        require_edge(slf.as_super(), "getParameterByLength")?;
        Err(edge_query_error("getParameterByLength"))
    }

    fn parameterAt(
        slf: PyRef<'_, Self>,
        _vertex: &Bound<'_, PyAny>,
    ) -> PyResult<f64> {
        require_edge(slf.as_super(), "parameterAt")?;
        Err(edge_query_error("parameterAt"))
    }

    fn parameters(
        slf: PyRef<'_, Self>,
        _face: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<f64>> {
        require_edge(slf.as_super(), "parameters")?;
        Err(edge_query_error("parameters"))
    }

    fn __repr__(&self) -> String {
        "Part.Edge(...)".to_string()
    }
}

/// `Part.Wire(edges)` — an ordered edge chain, a `Part.Shape` subclass as upstream.
#[pyclass(name = "Wire", module = "Part", extends = PartShape)]
pub struct PyWire {
    entries: Mutex<Vec<EdgeEntry>>,
}

impl PyWire {
    fn entries(&self) -> std::sync::MutexGuard<'_, Vec<EdgeEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wrap an existing wire shape (`Face.OuterWire`), with no analytic entries.
    pub(crate) fn from_shape(shape: Shape) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self {
            entries: Mutex::new(Vec::new()),
        })
    }

    /// A wire whose base shape is built eagerly, so `Part.Wire` really is a
    /// `Part.Shape` (upstream builds the B-Rep wire on construction too).
    fn initializer(entries: Vec<EdgeEntry>) -> PyResult<PyClassInitializer<Self>> {
        let base = if entries.is_empty() {
            PartShape::empty()
        } else {
            let shapes: Vec<Shape> = entries.iter().map(|entry| entry.shape.clone()).collect();
            PartShape::from_shape(backend().make_wire(&shapes).map_err(kernel_error)?)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            entries: Mutex::new(entries),
        }))
    }
}

#[pymethods]
impl PyWire {
    #[new]
    fn new(edges: &Bound<'_, PyAny>) -> PyResult<PyClassInitializer<Self>> {
        // Upstream accepts a single edge/curve (`Part.Wire(Part.makeCircle(...))`),
        // another wire, or an iterable of edges.
        if let Ok(wire) = edges.downcast::<PyWire>() {
            return Self::initializer(wire.borrow().entries().clone());
        }
        // A single edge is its own chain; an empty edge has nothing to chain, so
        // report that clearly instead of falling through to the iteration error.
        if let Ok(edge) = edges.downcast::<PyEdge>() {
            return Self::initializer(vec![PyEdge::entry_from(edge.as_any())?]);
        }
        if let Ok(entry) = PyEdge::entry_from(edges) {
            return Self::initializer(vec![entry]);
        }
        let mut entries = Vec::new();
        for edge in edges.try_iter()? {
            entries.push(PyEdge::entry_from(&edge?)?);
        }
        Self::initializer(entries)
    }

    /// Append one edge/curve; the base shape is rebuilt so the object stays a shape.
    fn add(mut slf: PyRefMut<'_, Self>, edge: &Bound<'_, PyAny>) -> PyResult<()> {
        let entry = PyEdge::entry_from(edge)?;
        let shapes = {
            let mut entries = slf.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries.push(entry);
            entries
                .iter()
                .map(|entry| entry.shape.clone())
                .collect::<Vec<_>>()
        };
        let shape = backend().make_wire(&shapes).map_err(kernel_error)?;
        *slf
            .as_super()
            .shape
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(shape);
        Ok(())
    }

    #[getter]
    fn Edges(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Vec<Py<PyEdge>>> {
        let entries = slf.entries();
        if !entries.is_empty() {
            return entries
                .iter()
                .map(|entry| Py::new(py, PyEdge::from_entry(entry.clone())))
                .collect();
        }
        // A wire wrapped from a kernel shape (for example `Face.OuterWire`) has
        // no analytic entries; read its edges from the B-Rep instead.
        slf.as_super()
            .sub_shapes(ElementKind::Edge)?
            .into_iter()
            .map(|shape| {
                Py::new(
                    py,
                    PyEdge::from_entry(EdgeEntry {
                        shape,
                        curve: None,
                    }),
                )
            })
            .collect()
    }

    /// Whether the chain closes, judged from the analytic endpoints this slice
    /// carries. A wire built only from shapes cannot answer (reported `False`).
    fn isClosed(&self) -> bool {
        let entries = self.entries();
        let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
            return false;
        };
        let points = |curve: EdgeCurve| curve.endpoints();
        match (first.curve.and_then(points), last.curve.and_then(points)) {
            (Some((start, _)), Some((_, end))) => {
                start.distance(&end) <= ferrocad_curves::TOLERANCE
            }
            _ => false,
        }
    }

    /// The wire as a kernel shape (already built by the constructor/`add`).
    fn toShape(slf: PyRef<'_, Self>) -> PartShape {
        slf.as_super().copy_handle()
    }

    /// Upstream wires iterate over their edges; the sequence protocol gives both
    /// `list(wire)` and `for edge in wire:`.
    fn __len__(&self) -> usize {
        self.entries().len()
    }

    fn __getitem__(&self, py: Python<'_>, index: isize) -> PyResult<Py<PyEdge>> {
        let entries = self.entries();
        let len = entries.len() as isize;
        let index = if index < 0 { index + len } else { index };
        if index < 0 || index >= len {
            return Err(pyo3::exceptions::PyIndexError::new_err(
                "wire index out of range",
            ));
        }
        Py::new(py, PyEdge::from_entry(entries[index as usize].clone()))
    }

    fn __repr__(&self) -> String {
        format!("Part.Wire({} edges)", self.entries().len())
    }
}

// ---------------------------------------------------------------------------
// Vertices and faces
// ---------------------------------------------------------------------------

/// Whether two handles name the same kernel shape.
///
/// [`Shape::is_same`] is handle identity at the seam, which is all a copied handle
/// needs; but the kernel re-wraps every `Shape.Vertexes` enumeration in fresh
/// handles, so two wrappers of one vertex are not `is_same`. When both handles are
/// OCCT shapes, ask OCCT's own identity instead (`IsEqual`: same `TShape`, location
/// and orientation — upstream's `IsSame` for a vertex, which we never reverse).
fn shapes_are_same(a: &Shape, b: &Shape) -> bool {
    if a.is_same(b) {
        return true;
    }
    match (
        a.downcast_ref::<ferrocad_occt::OcctShape>(),
        b.downcast_ref::<ferrocad_occt::OcctShape>(),
    ) {
        (Some(a), Some(b)) => match (a.borrow().as_ref(), b.borrow().as_ref()) {
            (Some(a), Some(b)) => a.IsEqual(b),
            _ => false,
        },
        _ => false,
    }
}

/// `Part.Vertex(x, y, z)` / `Part.Vertex(point)` / `Part.Vertex(vertex)` — a
/// vertex, a `Part.Shape`. `Part.Vertex()` is the null vertex.
#[pyclass(name = "Vertex", module = "Part", extends = PartShape)]
pub struct PyVertex {
    point: Vector3,
    /// The kernel shape; `None` for the null vertex. Cloning a `Shape` shares its
    /// identity, which is what `Part.Vertex(vertex)` relies on.
    shape: Mutex<Option<Shape>>,
}

impl PyVertex {
    fn from_parts(shape: Option<Shape>, point: Vector3) -> PyClassInitializer<Self> {
        let base = match &shape {
            Some(shape) => PartShape::from_shape(shape.clone()),
            None => PartShape::empty(),
        };
        PyClassInitializer::from(base).add_subclass(Self {
            point,
            shape: Mutex::new(shape),
        })
    }

    fn initializer(point: Vector3) -> PyResult<PyClassInitializer<Self>> {
        let shape = backend().make_vertex(point).map_err(kernel_error)?;
        Ok(Self::from_parts(Some(shape), point))
    }

    /// Wrap an existing vertex shape, reading its point from the kernel.
    pub(crate) fn from_shape(shape: Shape) -> PyResult<PyClassInitializer<Self>> {
        let point = backend()
            .vertex_point(&shape)
            .unwrap_or_else(Vector3::zero);
        Ok(Self::from_parts(Some(shape), point))
    }

    fn shape(&self) -> Option<Shape> {
        self.shape.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn is_null(&self) -> bool {
        self.shape().is_none()
    }

    /// Kernel-shape identity (upstream's `TopoDS_Shape::IsSame`).
    fn is_same(&self, other: &Self) -> bool {
        match (self.shape(), other.shape()) {
            (Some(a), Some(b)) => shapes_are_same(&a, &b),
            (None, None) => true,
            _ => false,
        }
    }
}

#[pymethods]
impl PyVertex {
    #[new]
    #[pyo3(signature = (x = None, y = None, z = None))]
    fn new(
        x: Option<&Bound<'_, PyAny>>,
        y: Option<f64>,
        z: Option<f64>,
    ) -> PyResult<PyClassInitializer<Self>> {
        match (x, y, z) {
            (Some(x), Some(y), Some(z)) => Self::initializer(Vector3::new(x.extract()?, y, z)),
            (Some(x), None, None) => {
                // `Part.Vertex(vertex)` shares the source shape, so upstream's
                // identity-based equality and hash hold for the copy.
                if let Ok(vertex) = x.extract::<PyRef<'_, Self>>() {
                    return Ok(Self::from_parts(vertex.shape(), vertex.point));
                }
                Self::initializer(vector(x)?)
            }
            (None, None, None) => Ok(Self::from_parts(None, Vector3::zero())),
            _ => Err(PyTypeError::new_err(
                "Part.Vertex takes a point or x, y, z",
            )),
        }
    }

    #[getter]
    fn Point(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.point)
    }

    #[getter]
    fn X(&self) -> f64 {
        self.point.x
    }

    #[getter]
    fn Y(&self) -> f64 {
        self.point.y
    }

    #[getter]
    fn Z(&self) -> f64 {
        self.point.z
    }

    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyVertex>> {
        Py::new(py, Self::from_parts(self.shape(), self.point))
    }

    /// Upstream's repr: `<Part.Vertex (1, 2, 3)>`, `<Part.Vertex()>` for the null
    /// vertex (an explicit `Part.Vertex(0, 0, 0)` is a real vertex).
    fn __repr__(&self) -> String {
        if self.is_null() {
            "<Part.Vertex()>".to_string()
        } else {
            format!(
                "<Part.Vertex ({}, {}, {})>",
                self.point.x, self.point.y, self.point.z
            )
        }
    }

    /// `len(vertex)` is always 3, as upstream (the null vertex included).
    fn __len__(&self) -> usize {
        3
    }

    /// Index a vertex like its point; the null vertex has no coordinates.
    fn __getitem__(&self, index: isize) -> PyResult<f64> {
        let index = if index < 0 { index + 3 } else { index };
        let value = match index {
            0 => self.point.x,
            1 => self.point.y,
            2 => self.point.z,
            _ => return Err(PyIndexError::new_err("index out of range")),
        };
        if self.is_null() {
            return Err(PyRuntimeError::new_err("a null vertex has no coordinates"));
        }
        Ok(value)
    }

    /// Equality is kernel-shape identity; non-vertices are not comparable.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        match other.extract::<PyRef<'_, Self>>() {
            Ok(other) => self.is_same(&other).into_py_any(py),
            Err(_) => Ok(py.NotImplemented()),
        }
    }

    /// Inequality mirrors [`Self::__eq__`]; ordering is undefined, as upstream.
    fn __ne__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        match other.extract::<PyRef<'_, Self>>() {
            Ok(other) => (!self.is_same(&other)).into_py_any(py),
            Err(_) => Ok(py.NotImplemented()),
        }
    }

    /// Two handles to the same vertex hash alike; a coordinate hash is enough
    /// (a different vertex at the same point is merely a collision).
    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.point.x.to_bits().hash(&mut hasher);
        self.point.y.to_bits().hash(&mut hasher);
        self.point.z.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}

/// `Part.Face(wire, face_maker="Part::FaceMakerBullseye")` — a planar face from
/// a wire (or a list of wires, the rest being holes), a `Part.Shape`.
///
/// `Part.Face()` is upstream's empty (null) face; every geometric query on it is
/// a `ValueError`, as upstream's `EmptyFace` tests require.
#[pyclass(name = "Face", module = "Part", extends = PartShape)]
pub struct PyFace;

impl PyFace {
    fn initializer(shape: Shape) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self)
    }

    /// The empty face `Part.Face()`: a null shape with no boundary.
    fn empty_initializer() -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::empty()).add_subclass(Self)
    }
}

/// The kernel shapes of a wire, a single shape, or a list of either.
fn wire_shapes(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<crate::containers::PyCompound>() {
        return Ok(compound.borrow().children());
    }
    if let Ok(wire) = value.downcast::<PyWire>() {
        let base = wire.borrow();
        let shape = base
            .as_super()
            .shape()
            .ok_or_else(|| OCCError::new_err("an empty wire has no shape"))?;
        return Ok(vec![shape]);
    }
    if let Ok(edge) = value.downcast::<PyEdge>() {
        let shape = edge
            .borrow()
            .entry()
            .ok_or_else(|| PyValueError::new_err("an empty Part.Edge has no geometry"))?
            .shape;
        let wire = backend().make_wire(&[shape]).map_err(kernel_error)?;
        return Ok(vec![wire]);
    }
    if let Ok(shape) = value.downcast::<PartShape>() {
        let inner = shape
            .borrow()
            .shape()
            .ok_or_else(|| OCCError::new_err("a null shape cannot bound a face"))?;
        // A face is not itself a boundary: `Part.makeFace(box.Faces[0])` means
        // "reuse that face's boundary", so hand the kernel its outer wire.
        // `outer_wire` only answers for faces, so wires keep their own shape.
        if let Some(outer) = backend().outer_wire(&inner) {
            return Ok(vec![outer]);
        }
        return Ok(vec![inner]);
    }
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.extend(wire_shapes(&item?)?);
    }
    Ok(shapes)
}

#[pymethods]
impl PyFace {
    #[new]
    #[pyo3(signature = (value = None, face_maker = "Part::FaceMakerBullseye"))]
    fn new(
        value: Option<&Bound<'_, PyAny>>,
        face_maker: &str,
    ) -> PyResult<PyClassInitializer<Self>> {
        let Some(value) = value else {
            return Ok(Self::empty_initializer());
        };
        let shapes = wire_shapes(value)?;
        let shape = if face_maker == "Part::FaceMakerUnified" {
            backend().make_faces(&shapes).map_err(face_error)?
        } else {
            backend().make_face(&shapes).map_err(kernel_error)?
        };
        Ok(Self::initializer(shape))
    }

    /// `Face.makeOffset(distance)` — the face grown (or shrunk) by `distance` in
    /// its plane, as `Part.Shape.makeOffset` (upstream's deprecated alias for
    /// `makeOffset2D`). An empty face is a `ValueError`, as upstream.
    fn makeOffset(slf: PyRef<'_, Self>, distance: f64) -> PyResult<PartShape> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("makeOffset"))?;
        let offset = backend()
            .make_offset(&shape, distance, 0)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(offset))
    }

    // As for `Part.Edge`, the queries below are what upstream's `EmptyFace` tests
    // drive: each is a `ValueError` on the empty face. This slice has no seam call
    // for them on a real face either, so those report `OCCError` instead of
    // inventing an answer.

    fn valueAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "valueAt")?;
        Err(edge_query_error("valueAt"))
    }

    fn normalAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "normalAt")?;
        Err(edge_query_error("normalAt"))
    }

    fn tangentAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "tangentAt")?;
        Err(edge_query_error("tangentAt"))
    }

    fn curvatureAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "curvatureAt")?;
        Err(edge_query_error("curvatureAt"))
    }

    fn derivative1At(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative1At")?;
        Err(edge_query_error("derivative1At"))
    }

    fn derivative2At(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative2At")?;
        Err(edge_query_error("derivative2At"))
    }

    fn cutHoles(slf: PyRef<'_, Self>, _holes: &Bound<'_, PyAny>) -> PyResult<()> {
        require_edge(slf.as_super(), "cutHoles")?;
        Err(edge_query_error("cutHoles"))
    }

    fn getUVNodes(slf: PyRef<'_, Self>) -> PyResult<Vec<(f64, f64)>> {
        require_edge(slf.as_super(), "getUVNodes")?;
        Err(edge_query_error("getUVNodes"))
    }

    #[getter]
    fn Tolerance(slf: PyRef<'_, Self>) -> PyResult<f64> {
        require_edge(slf.as_super(), "Tolerance")?;
        Err(edge_query_error("Tolerance"))
    }

    #[setter]
    fn set_Tolerance(slf: PyRef<'_, Self>, _value: f64) -> PyResult<()> {
        require_edge(slf.as_super(), "Tolerance")?;
        Err(edge_query_error("Tolerance"))
    }

    #[getter]
    fn ParameterRange(slf: PyRef<'_, Self>) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "ParameterRange")?;
        Err(edge_query_error("ParameterRange"))
    }
}

// ---------------------------------------------------------------------------
// Module-level constructors
// ---------------------------------------------------------------------------

/// `Part.makeLine(p1, p2)` → an edge (Draft's `Part.makeLine`).
#[pyfunction]
fn makeLine(py: Python<'_>, p1: &Bound<'_, PyAny>, p2: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    let segment = LineSegment::new(vector(p1)?, vector(p2)?).map_err(curve_error)?;
    Ok(Py::new(py, PyEdge::from_curve(EdgeCurve::LineSegment(segment))?)?.into_any())
}

/// `Part.makeCircle(radius, center=None, normal=None)` → an edge.
#[pyfunction]
#[pyo3(signature = (radius, center = None, normal = None))]
fn makeCircle(
    py: Python<'_>,
    radius: f64,
    center: Option<&Bound<'_, PyAny>>,
    normal: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
    let normal = normal
        .map(vector)
        .transpose()?
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    let circle = Circle::new(center, normal, radius).map_err(curve_error)?;
    Ok(Py::new(py, PyEdge::from_curve(EdgeCurve::Circle(circle))?)?.into_any())
}

/// `Part.makePolygon(points, closed=False)` → a `Part.Wire`.
#[pyfunction]
#[pyo3(signature = (points, closed = false))]
fn makePolygon(py: Python<'_>, points: &Bound<'_, PyAny>, closed: bool) -> PyResult<Py<PyWire>> {
    let mut parsed = Vec::new();
    for point in points.try_iter()? {
        parsed.push(vector(&point?)?);
    }
    let edges = polygon(&parsed, closed).map_err(curve_error)?;
    let mut entries = Vec::with_capacity(edges.len());
    for curve in edges {
        entries.push(EdgeEntry {
            shape: shape_from_curve(&curve)?,
            curve: Some(curve),
        });
    }
    Py::new(py, PyWire::initializer(entries)?)
}

/// `Part.makeFace(wires, face_maker="Part::FaceMakerBullseye", noElementMap=False)`.
///
/// The first wire is the outer boundary, the rest are holes. The `face_maker`
/// flavour and `noElementMap` are accepted for signature compatibility; only the
/// planar builder exists in this slice.
#[pyfunction]
#[pyo3(signature = (wires, face_maker = "Part::FaceMakerBullseye", noElementMap = false))]
fn makeFace(
    py: Python<'_>,
    wires: &Bound<'_, PyAny>,
    face_maker: &str,
    noElementMap: bool,
) -> PyResult<Py<PyFace>> {
    let _ = noElementMap;
    let shapes = wire_shapes(wires)?;
    let shape = if face_maker == "Part::FaceMakerUnified" {
        backend().make_faces(&shapes).map_err(face_error)?
    } else {
        backend().make_face(&shapes).map_err(kernel_error)?
    };
    Py::new(py, PyFace::initializer(shape))
}

/// Degenerate face inputs are `ValueError` upstream (an empty compound or an
/// open wire); kernel failures stay `Part.OCCError`.
fn face_error(error: ferrocad_geom::GeomError) -> PyErr {
    let invalid = matches!(
        error.downcast_ref::<ferrocad_occt::OcctError>(),
        Some(ferrocad_occt::OcctError::InvalidInput { .. })
    );
    if invalid {
        PyValueError::new_err(error.to_string())
    } else {
        kernel_error(error)
    }
}

/// Register the analytic half on the `Part` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLineSegment>()?;
    m.add_class::<PyLine>()?;
    m.add_class::<PyCircle>()?;
    m.add_class::<PyArcOfCircle>()?;
    m.add_class::<PyEllipse>()?;
    m.add_class::<PyArcOfEllipse>()?;
    m.add_class::<PyPlane>()?;
    m.add_class::<PyCylinder>()?;
    m.add_class::<PySphere>()?;
    m.add_class::<PyRectangularTrimmedSurface>()?;
    m.add_class::<PyEdge>()?;
    m.add_class::<PyWire>()?;
    m.add_class::<PyVertex>()?;
    m.add_class::<PyFace>()?;
    m.add_class::<PyBSplineCurve>()?;
    m.add_function(wrap_pyfunction!(makeLine, m)?)?;
    m.add_function(wrap_pyfunction!(makeCircle, m)?)?;
    m.add_function(wrap_pyfunction!(makePolygon, m)?)?;
    m.add_function(wrap_pyfunction!(makeFace, m)?)?;
    m.add("OCCError", m.py().get_type::<OCCError>())?;
    // `Part.Geom2d` is a submodule, as upstream (the classes live there rather
    // than directly on `Part`).
    let geom2d = PyModule::new(m.py(), "Part.Geom2d")?;
    geom2d.add_class::<PyLine2dSegment>()?;
    geom2d.add_class::<PyCircle2d>()?;
    geom2d.add_class::<PyBSplineCurve2d>()?;
    m.add("Geom2d", &geom2d)?;
    // Upstream offers both names for the same class.
    let arc = m.getattr("ArcOfCircle")?;
    m.add("Arc", arc)?;
    Ok(())
}
