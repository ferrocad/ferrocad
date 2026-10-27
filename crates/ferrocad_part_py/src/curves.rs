//! The analytic half of `Part` in Python: curves, edges and wires.
//!
//! This is the first slice of the `Part` module the backlog calls the shared kernel
//! work: upstream's `Part::Geom*` classes ([`Part.LineSegment`], [`Part.Circle`],
//! [`Part.ArcOfCircle`]) plus the edge/wire containers Draft imports. The geometry is
//! [`ferrocad_curves`]; the conversion to a kernel shape is the `ferrocad_geom` seam
//! (`make_edge`/`make_wire`), implemented by OCCT.
//!
//! It also carries the `Part.Geom2d` submodule: the abstract `Geometry2d` /
//! `Curve2d` / `Conic2d` / `ArcOfConic2d` bases and the concrete `Line2d`,
//! `Line2dSegment`, `Circle2d`, `ArcOfCircle2d`, `Ellipse2d`, `ArcOfEllipse2d`,
//! `Hyperbola2d`, `ArcOfHyperbola2d`, `Parabola2d`, `ArcOfParabola2d`,
//! `BezierCurve2d`, `OffsetCurve2d` and `BSplineCurve2d` classes. Their
//! geometry is the XY projection of the `ferrocad_curves` types above, so
//! `value`, the parameter range and `toShape()` are evaluated for real; the
//! `BSplineCurve2d` only records the points it is asked to fit (this slice binds
//! no 2D B-spline kernel) and says so instead of inventing a curve. It also
//! carries the elementary surface descriptions `Part.Plane`, `Part.Cylinder`
//! (upstream's `Part::GeomCylinder`) and `Part.Sphere` (upstream's
//! `Part::GeomSphere`), and `Part.RectangularTrimmedSurface`. The cylinder and
//! sphere are distinct from the document primitives returned by
//! `Part.makeCylinder`/`Part.makeSphere`.
//!
//! Deliberate limits for this slice, all reported as `Part.OCCError` or
//! `NotImplementedError` rather than a silent `None`: an infinite `Line` has no
//! edge, and face/solid constructors (`makeFace`, `makeSolid`, …) are still to
//! come. `Part.Geom2d`'s `toShape(surface)` attaches the 2D curve as the edge's
//! pcurve but does **not** project it onto the surface: no `GeomProjLib` binding
//! exists here, and `Part.Cylinder`/`Part.Sphere` are only descriptions (radius,
//! center, axis), not real B-Rep geometry. `RectangularTrimmedSurface.toShape()`
//! builds a real planar face for a `Part.Plane` basis only; a trimmed cylinder or
//! sphere is described but raises `Part.OCCError` when asked for a face.

use std::sync::Mutex;

use ferrocad_curves::{
    ANGULAR_TOLERANCE, Arc, ArcOfHyperbola, ArcOfParabola, BezierCurve, Circle, CurveError,
    EdgeCurve, Ellipse, EllipticalArc, Hyperbola, Line, LineSegment, NurbsCurve, OffsetCurve,
    Parabola, TOLERANCE, Vector3, polygon,
};
use ferrocad_geom::{ElementKind, Shape, SurfaceData, SurfaceSpec};
use ferrocad_occt::surface_ops;
use ferrocad_part::backend;
use ferrocad_types::Matrix4;
use pyo3::create_exception;
use pyo3::exceptions::{
    PyException, PyIndexError, PyNotImplementedError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyModule, PySequence, PyTuple};
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
        // The same basis rule for the other trimmed conics and the ellipse arc.
        EdgeCurve::EllipticalArc(arc) => Ok(Py::new(
            py,
            PyEllipse {
                curve: arc.ellipse(),
            },
        )?
        .into_any()),
        EdgeCurve::Hyperbola(hyperbola) => {
            Ok(Py::new(py, PyHyperbola { curve: hyperbola })?.into_any())
        }
        EdgeCurve::ArcOfHyperbola(arc) => Ok(Py::new(
            py,
            PyHyperbola {
                curve: arc.hyperbola(),
            },
        )?
        .into_any()),
        EdgeCurve::Parabola(parabola) => {
            Ok(Py::new(py, PyParabola { curve: parabola })?.into_any())
        }
        EdgeCurve::ArcOfParabola(arc) => Ok(Py::new(
            py,
            PyParabola {
                curve: arc.parabola(),
            },
        )?
        .into_any()),
        EdgeCurve::BezierCurve(bezier) => Ok(Py::new(
            py,
            PyBezierCurve {
                curve: Mutex::new(bezier),
            },
        )?
        .into_any()),
        EdgeCurve::BSplineCurve(spline) => Ok(Py::new(
            py,
            PyBSplineCurve::from_state(nurbs_state(&spline)?),
        )?
        .into_any()),
        EdgeCurve::OffsetCurve(offset) => {
            Ok(Py::new(py, PyOffsetCurve { curve: offset })?.into_any())
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
    /// The registered type name (`Part::GeomLineSegment`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomLineSegment"
    }

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

    /// The segment as an edge (upstream's `toShape`, which returns a `Part.Edge`),
    /// or the piece between two parameters along it.
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        curve_to_shape(
            py,
            EdgeCurve::LineSegment(self.curve),
            first,
            last,
            "line segment",
        )
    }

    /// The curve parameter of `point` (`LineSegment.parameter(point)`): the
    /// distance from the start point along the segment.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let start = self.curve.start();
        let offset = point.sub(&start);
        let span = self.curve.end().sub(&start);
        let length = span.length();
        let distance = if length > 0.0 {
            offset.dot(&span.scale(1.0 / length))
        } else {
            0.0
        };
        Ok(distance.into_py_any(py)?)
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
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
    /// The registered type name (`Part::GeomLine`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomLine"
    }

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

    /// The curve parameter of `point` (`Line.parameter(point)`): the signed
    /// distance from the origin along the direction.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.curve.origin());
        Ok(offset.dot(&self.curve.direction()).into_py_any(py)?)
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
    }

    /// An infinite line has no unbounded edge; with an explicit range it is the
    /// segment between the two parameters (`GeometryCurvePy::toShape`).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        if first.is_none() && last.is_none() {
            return Err(OCCError::new_err(
                "an infinite Part.Line has no edge; use Part.LineSegment or pass a parameter range",
            ));
        }
        curve_to_shape(py, EdgeCurve::Line(self.curve), first, last, "line")
    }
}

/// `GeometryCurve.toShape(first, last)`: the whole curve's edge, or the piece
/// between two curve parameters, as a `Part.Edge`.
///
/// Upstream's `GeometryCurvePy::toShape` accepts an optional range and trims the
/// curve to it; without a range the whole (bounded) curve is the edge.
fn curve_to_shape(
    py: Python<'_>,
    curve: EdgeCurve,
    first: Option<f64>,
    last: Option<f64>,
    what: &str,
) -> PyResult<Py<PyEdge>> {
    let curve = match (first, last) {
        (None, None) => curve,
        (Some(first), Some(last)) => curve.trimmed(first, last).ok_or_else(|| {
            OCCError::new_err(format!(
                "a {what} cannot be trimmed to the given parameter range"
            ))
        })?,
        _ => {
            return Err(OCCError::new_err(format!(
                "{what}.toShape needs both first and last parameters"
            )));
        }
    };
    Py::new(py, PyEdge::from_curve(curve)?)
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
    /// The registered type name (`Part::GeomCircle`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomCircle"
    }

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

    /// Move the circle (`Circle.Center = …`), keeping its plane, frame and radius.
    #[setter]
    fn set_Center(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.curve = Circle::with_axis(
            vector(value)?,
            self.curve.normal(),
            self.curve.x_axis(),
            self.curve.radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// Upstream's deprecated alias of [`Center`](Self::Center).
    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    #[setter]
    fn set_Location(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set_Center(value)
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    /// Replace the circle's plane normal (`Circle.Axis = …`), keeping the centre,
    /// radius and the direction of the X axis projected into the new plane.
    #[setter]
    fn set_Axis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.curve = Circle::with_axis(
            self.curve.center(),
            vector(value)?,
            self.curve.x_axis(),
            self.curve.radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.curve.radius()
    }

    /// The circle's in-plane X direction (`Circle.XAxis`).
    #[getter]
    fn XAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.x_axis())
    }

    /// Replace the circle's in-plane X direction, keeping its centre, plane and
    /// radius (`Circle.XAxis = …`).
    #[setter]
    fn set_XAxis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let x_axis = vector(value)?;
        self.curve = Circle::with_axis(
            self.curve.center(),
            self.curve.normal(),
            x_axis,
            self.curve.radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The circle's in-plane Y direction, `Axis × XAxis` (`Circle.YAxis`).
    #[getter]
    fn YAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.y_axis())
    }

    #[setter]
    fn set_YAxis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let y_axis = vector(value)?;
        let normal = self.curve.normal();
        self.curve = Circle::with_axis(
            self.curve.center(),
            normal,
            y_axis.cross(&normal),
            self.curve.radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The rotation of the X axis from the plane's reference direction, in radians
    /// (`Conic.AngleXU`). A circle created without an explicit X axis is zero.
    #[getter]
    fn AngleXU(&self) -> f64 {
        axis_angle(self.curve.normal(), self.curve.x_axis())
    }

    #[setter]
    fn set_AngleXU(&mut self, value: f64) -> PyResult<()> {
        let normal = self.curve.normal();
        let x_axis = ferrocad_curves::rotate_about(
            ferrocad_curves::reference_x_axis(normal),
            normal,
            value,
        );
        self.curve = Circle::with_axis(
            self.curve.center(),
            normal,
            x_axis,
            self.curve.radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The curve parameter of `point` (`Circle.parameter(point)`): its angle on
    /// the circle, measured from the X axis.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.curve.center());
        let y_axis = self.curve.y_axis();
        let angle = offset
            .dot(&y_axis)
            .atan2(offset.dot(&self.curve.x_axis()));
        Ok(angle.into_py_any(py)?)
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
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

    /// The circle as an edge, or the arc over `[first, last]`
    /// (`GeometryCurvePy::toShape`).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        curve_to_shape(py, EdgeCurve::Circle(self.curve), first, last, "circle")
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

/// The circle through three points and the arc from `p1` through `p2` to `p3`
/// (`GC_MakeCircle` + `GC_MakeArcOfCircle`). The points must be distinct and not
/// collinear; the centre is the circumcentre of the triangle.
fn arc_through_three_points(p1: Vector3, p2: Vector3, p3: Vector3) -> PyResult<Arc> {
    let a = p2.sub(&p1);
    let b = p3.sub(&p1);
    let normal = a.cross(&b);
    let normal_length = normal.length();
    if normal_length <= TOLERANCE {
        return Err(OCCError::new_err(
            "Three points are collinear or coincident",
        ));
    }
    // Circumcentre: p1 + (|a|² (b × n) + |b|² (n × a)) / (2 |n|²).
    let offset = b
        .cross(&normal)
        .scale(a.sqr())
        .add(&normal.cross(&a).scale(b.sqr()))
        .scale(1.0 / (2.0 * normal_length * normal_length));
    let center = p1.add(&offset);
    let x_axis = p1.sub(&center);
    let radius = x_axis.length();
    if radius <= TOLERANCE {
        return Err(OCCError::new_err("Three points are coincident"));
    }
    let circle = Circle::with_axis(center, normal, x_axis, radius).map_err(curve_error)?;
    let y_axis = circle.y_axis();
    let angle = |point: &Vector3| {
        let offset = point.sub(&center);
        offset.dot(&y_axis).atan2(offset.dot(&circle.x_axis()))
    };
    let first = angle(&p1);
    let through = (angle(&p2) - first).rem_euclid(std::f64::consts::TAU);
    let end = (angle(&p3) - first).rem_euclid(std::f64::consts::TAU);
    // Follow the parameter direction that passes through `p2`.
    let last = if through < end {
        first + end
    } else {
        first + end - std::f64::consts::TAU
    };
    Arc::new(circle, first, last).map_err(curve_error)
}

/// `Part.ArcOfCircle(circle, first, last)` — a circular arc (the `Part.Arc` alias).
#[pyclass(name = "ArcOfCircle", module = "Part")]
#[derive(Clone)]
pub struct PyArcOfCircle {
    curve: Arc,
}

#[pymethods]
impl PyArcOfCircle {
    /// The registered type name (`Part::GeomArcOfCircle`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomArcOfCircle"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
    }

    /// `Part.ArcOfCircle(circle, first, last)` (the aliased `Part.Arc` form) or
    /// `Part.ArcOfCircle(p1, p2, p3)` / `Part.ArcOfCircle([p1, p2, p3])`, the arc
    /// through three points (`GC_MakeArcOfCircle`).
    #[new]
    #[pyo3(signature = (circle, first = None, last = None))]
    fn new(
        circle: &Bound<'_, PyAny>,
        first: Option<&Bound<'_, PyAny>>,
        last: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        if let Ok(circle) = circle.extract::<PyRef<'_, PyCircle>>() {
            let (Some(first), Some(last)) = (first, last) else {
                return Err(PyTypeError::new_err(
                    "ArcOfCircle(circle, first, last) needs both an angle range",
                ));
            };
            let curve = Arc::new(circle.curve, first.extract()?, last.extract()?)
                .map_err(curve_error)?;
            return Ok(Self { curve });
        }
        let (p1, p2, p3) = match (first, last) {
            (Some(first), Some(last)) => (vector(circle)?, vector(first)?, vector(last)?),
            (None, None) => {
                let sequence = circle.downcast::<PySequence>().map_err(|_| {
                    PyTypeError::new_err(
                        "ArcOfCircle expects (circle, first, last) or three points",
                    )
                })?;
                if sequence.len().ok() != Some(3) {
                    return Err(PyTypeError::new_err(
                        "ArcOfCircle expects exactly three points",
                    ));
                }
                (
                    vector(&sequence.get_item(0)?)?,
                    vector(&sequence.get_item(1)?)?,
                    vector(&sequence.get_item(2)?)?,
                )
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "ArcOfCircle expects (circle, first, last) or three points",
                ));
            }
        };
        let curve = arc_through_three_points(p1, p2, p3)?;
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

    /// The arc's plane normal (`ArcOfCircle.Axis`), the underlying circle's axis.
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.circle().normal())
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

    /// The arc as an edge, or the sub-arc over `[first, last]`.
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        curve_to_shape(py, EdgeCurve::Arc(self.curve), first, last, "arc")
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
    /// The registered type name (`Part::GeomEllipse`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomEllipse"
    }

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

    /// Move the ellipse (`Ellipse.Center = …`), keeping its plane, frame and radii.
    #[setter]
    fn set_Center(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.curve = Ellipse::with_axis(
            vector(value)?,
            self.curve.normal(),
            self.curve.x_axis(),
            self.curve.major_radius(),
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// Upstream's deprecated alias of [`Center`](Self::Center).
    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    #[setter]
    fn set_Location(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set_Center(value)
    }

    /// The unit normal of the ellipse's plane (upstream's `Axis`).
    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    /// Replace the plane normal (`Ellipse.Axis = …`), keeping the centre, radii
    /// and the major axis projected into the new plane.
    #[setter]
    fn set_Axis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            vector(value)?,
            self.curve.x_axis(),
            self.curve.major_radius(),
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The direction of the major axis.
    #[getter]
    fn MajorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.x_axis())
    }

    /// Replace the major-axis direction (`Ellipse.XAxis = …`), keeping the centre,
    /// plane and radii.
    #[setter]
    fn set_XAxis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            self.curve.normal(),
            vector(value)?,
            self.curve.major_radius(),
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The direction of the minor axis.
    #[getter]
    fn MinorAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.y_axis())
    }

    #[setter]
    fn set_YAxis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let y_axis = vector(value)?;
        let normal = self.curve.normal();
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            normal,
            y_axis.cross(&normal),
            self.curve.major_radius(),
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The rotation of the major axis from the plane's reference direction
    /// (`Conic.AngleXU`), in radians.
    #[getter]
    fn AngleXU(&self) -> f64 {
        axis_angle(self.curve.normal(), self.curve.x_axis())
    }

    #[setter]
    fn set_AngleXU(&mut self, value: f64) -> PyResult<()> {
        let normal = self.curve.normal();
        let x_axis = ferrocad_curves::rotate_about(
            ferrocad_curves::reference_x_axis(normal),
            normal,
            value,
        );
        self.curve = Ellipse::with_axis(
            self.curve.center(),
            normal,
            x_axis,
            self.curve.major_radius(),
            self.curve.minor_radius(),
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The curve parameter of `point` (`Ellipse.parameter(point)`): its angle on
    /// the ellipse, measured from the major axis.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.curve.center());
        let y_axis = self.curve.y_axis();
        let angle = offset
            .dot(&y_axis)
            .atan2(offset.dot(&self.curve.x_axis()));
        Ok(angle.into_py_any(py)?)
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
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
    /// The ellipse as an edge, or the elliptical arc over `[first, last]`.
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        curve_to_shape(py, EdgeCurve::Ellipse(self.curve), first, last, "ellipse")
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
    /// The registered type name (`Part::GeomArcOfEllipse`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomArcOfEllipse"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            ellipse: self.ellipse,
            first: self.first,
            last: self.last,
        }
    }

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

    /// The arc as an edge, or the sub-arc over `[first, last]`
    /// (`GeometryCurvePy::toShape`).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let first = first.unwrap_or(self.first);
        let last = last.unwrap_or(self.last);
        let arc = EllipticalArc::new(self.ellipse, first, last).map_err(curve_error)?;
        Py::new(py, PyEdge::from_curve(EdgeCurve::EllipticalArc(arc))?)
    }
}

/// Construct upstream's `Hyperbola(S1, S2, Center)` form: the major axis runs
/// from `Center` to `S1`, and the minor radius is the distance from `S2` to it.
fn hyperbola_from_three_points(
    s1: Vector3,
    s2: Vector3,
    center: Vector3,
) -> PyResult<Hyperbola> {
    let major_axis = s1.sub(&center);
    let major = major_axis.length();
    if major <= TOLERANCE {
        return Err(OCCError::new_err("the hyperbola points are confused"));
    }
    let x_axis = major_axis.scale(1.0 / major);
    let offset = s2.sub(&center);
    let minor_axis = offset.sub(&x_axis.scale(offset.dot(&x_axis)));
    if minor_axis.length() <= TOLERANCE {
        return Err(OCCError::new_err("the hyperbola points are collinear"));
    }
    Hyperbola::with_axis(center, x_axis.cross(&offset), x_axis, major, minor_axis.length())
        .map_err(curve_error)
}

/// `Part.Hyperbola()` — one branch of a hyperbola (`Part::GeomHyperbola`).
///
/// The constructors mirror upstream: empty (a `2`/`1` hyperbola in the XY plane),
/// a copy, `(Center, MajorRadius, MinorRadius)` (normal `+Z`) and
/// `(S1, S2, Center)` (the plane through the three points).
#[pyclass(name = "Hyperbola", module = "Part")]
#[derive(Clone)]
pub struct PyHyperbola {
    curve: Hyperbola,
}

#[pymethods]
impl PyHyperbola {
    /// The registered type name (`Part::GeomHyperbola`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomHyperbola"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
    }

    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Ok(Self {
                curve: Hyperbola::with_axis(
                    Vector3::zero(),
                    Vector3::new(0.0, 0.0, 1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                    2.0,
                    1.0,
                )
                .map_err(curve_error)?,
            }),
            1 => {
                let other = args.get_item(0)?;
                let other = other.extract::<PyRef<'_, PyHyperbola>>()?;
                Ok(Self { curve: other.curve })
            }
            3 => {
                let first = args.get_item(0)?;
                let second = args.get_item(1)?;
                let third = args.get_item(2)?;
                if let (Ok(major), Ok(minor)) =
                    (second.extract::<f64>(), third.extract::<f64>())
                {
                    let center = vector(&first)?;
                    return Ok(Self {
                        curve: Hyperbola::with_axis(
                            center,
                            Vector3::new(0.0, 0.0, 1.0),
                            Vector3::new(1.0, 0.0, 0.0),
                            major,
                            minor,
                        )
                        .map_err(curve_error)?,
                    });
                }
                Ok(Self {
                    curve: hyperbola_from_three_points(
                        vector(&first)?,
                        vector(&second)?,
                        vector(&third)?,
                    )?,
                })
            }
            _ => Err(PyTypeError::new_err(
                "Hyperbola constructor accepts:\n\
                 -- empty parameter list\n\
                 -- Hyperbola\n\
                 -- Point, double, double\n\
                 -- Point, Point, Point",
            )),
        }
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    /// The centre (upstream's newer name for `Center`).
    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.center())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    #[getter]
    fn XAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.x_axis())
    }

    #[getter]
    fn YAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.y_axis())
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.curve.major_radius()
    }

    #[setter]
    fn set_MajorRadius(&mut self, value: f64) -> PyResult<()> {
        self.curve = Hyperbola::with_axis(
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
        self.curve = Hyperbola::with_axis(
            self.curve.center(),
            self.curve.normal(),
            self.curve.x_axis(),
            self.curve.major_radius(),
            value,
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// The focal distance (`sqrt(MajorRadius^2 + MinorRadius^2)`).
    #[getter]
    fn Focal(&self) -> f64 {
        self.curve.focal()
    }

    #[getter]
    fn Focus1(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.focus1())
    }

    #[getter]
    fn Focus2(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.focus2())
    }

    #[getter]
    fn Eccentricity(&self) -> f64 {
        self.curve.eccentricity()
    }

    /// OCC reports an unbounded conic's range as `±Precision::Infinite()`.
    #[getter]
    fn FirstParameter(&self) -> f64 {
        -2.0e100
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        2.0e100
    }

    /// `value(u)`: `center + a*cosh(u)*X + b*sinh(u)*Y`.
    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        if !u.is_finite() {
            return Err(OCCError::new_err("the hyperbola parameter must be finite"));
        }
        py_vector(py, self.curve.point_at(u))
    }

    /// `trim(first, last)`: the `Part.ArcOfHyperbola` over the given range.
    #[pyo3(signature = (first = None, last = None))]
    fn trim(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyArcOfHyperbola>> {
        let (first, last) = required_range(first, last, "hyperbola")?;
        let arc = self.curve.trim(first, last).map_err(curve_error)?;
        Py::new(py, PyArcOfHyperbola { arc })
    }

    /// The exact rational quadratic B-spline over `[first, last]`.
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let (first, last) = required_range(first, last, "hyperbola")?;
        let nurbs = self.curve.nurbs(first, last).map_err(curve_error)?;
        spline_curve(py, &nurbs)
    }

    /// The same exact conversion as [`toNurbs`](Self::toNurbs).
    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        self.toNurbs(py, first, last)
    }

    /// The exact edge over `[first, last]` (a rational quadratic B-spline).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let (first, last) = required_range(first, last, "hyperbola")?;
        let nurbs = self.curve.nurbs(first, last).map_err(curve_error)?;
        spline_edge(py, &nurbs)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Hyperbola({:?}, {}, {})",
            self.curve.center(),
            self.curve.major_radius(),
            self.curve.minor_radius()
        )
    }
}

/// `Part.ArcOfHyperbola(hyperbola, first, last, sense=True)` — a portion of a
/// hyperbola (`Part::GeomArcOfHyperbola`), what [`PyHyperbola::trim`] returns.
#[pyclass(name = "ArcOfHyperbola", module = "Part")]
#[derive(Clone)]
pub struct PyArcOfHyperbola {
    arc: ArcOfHyperbola,
}

#[pymethods]
impl PyArcOfHyperbola {
    /// The registered type name (`Part::GeomArcOfHyperbola`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomArcOfHyperbola"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }

    #[new]
    #[pyo3(signature = (hyperbola, first, last, sense = true))]
    fn new(
        hyperbola: &Bound<'_, PyAny>,
        first: f64,
        last: f64,
        sense: bool,
    ) -> PyResult<Self> {
        let _ = sense;
        let curve = hyperbola.extract::<PyRef<'_, PyHyperbola>>()?.curve;
        let arc = curve.trim(first, last).map_err(curve_error)?;
        Ok(Self { arc })
    }

    /// The supporting hyperbola (`GeomArcOfHyperbola::getHyperbola`).
    #[getter]
    fn Hyperbola(&self, py: Python<'_>) -> PyResult<Py<PyHyperbola>> {
        Py::new(
            py,
            PyHyperbola {
                curve: self.arc.hyperbola(),
            },
        )
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().center())
    }

    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().center())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().normal())
    }

    #[getter]
    fn XAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().x_axis())
    }

    #[getter]
    fn YAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().y_axis())
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.arc.hyperbola().major_radius()
    }

    #[setter]
    fn set_MajorRadius(&mut self, value: f64) -> PyResult<()> {
        let hyperbola = Hyperbola::with_axis(
            self.arc.hyperbola().center(),
            self.arc.hyperbola().normal(),
            self.arc.hyperbola().x_axis(),
            value,
            self.arc.hyperbola().minor_radius(),
        )
        .map_err(curve_error)?;
        self.arc = ArcOfHyperbola::new(hyperbola, self.arc.first(), self.arc.last())
            .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn MinorRadius(&self) -> f64 {
        self.arc.hyperbola().minor_radius()
    }

    #[setter]
    fn set_MinorRadius(&mut self, value: f64) -> PyResult<()> {
        let hyperbola = Hyperbola::with_axis(
            self.arc.hyperbola().center(),
            self.arc.hyperbola().normal(),
            self.arc.hyperbola().x_axis(),
            self.arc.hyperbola().major_radius(),
            value,
        )
        .map_err(curve_error)?;
        self.arc = ArcOfHyperbola::new(hyperbola, self.arc.first(), self.arc.last())
            .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn Focal(&self) -> f64 {
        self.arc.hyperbola().focal()
    }

    #[getter]
    fn Focus1(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().focus1())
    }

    #[getter]
    fn Focus2(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.hyperbola().focus2())
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.arc.first()
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.arc.last()
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.start())
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.end())
    }

    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        py_vector(py, self.arc.point_at(u))
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.arc.first());
        let last = last.unwrap_or(self.arc.last());
        let nurbs = self
            .arc
            .hyperbola()
            .nurbs(first, last)
            .map_err(curve_error)?;
        spline_curve(py, &nurbs)
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        self.toNurbs(py, first, last)
    }

    /// The exact edge over the arc's range (or an explicit sub-range).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let first = first.unwrap_or(self.arc.first());
        let last = last.unwrap_or(self.arc.last());
        let nurbs = self
            .arc
            .hyperbola()
            .nurbs(first, last)
            .map_err(curve_error)?;
        spline_edge(py, &nurbs)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.ArcOfHyperbola({}, {})",
            self.arc.first(),
            self.arc.last()
        )
    }
}

/// `Part.Parabola()` — a parabola (`Part::GeomParabola`).
///
/// The constructors mirror upstream: empty (focal `1`, vertex at the origin),
/// a copy, and `(Focus, Center, Normal)` where `Center` is the vertex.
#[pyclass(name = "Parabola", module = "Part")]
#[derive(Clone)]
pub struct PyParabola {
    curve: Parabola,
}

#[pymethods]
impl PyParabola {
    /// The registered type name (`Part::GeomParabola`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomParabola"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
    }

    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Ok(Self {
                curve: Parabola::with_axis(
                    Vector3::zero(),
                    Vector3::new(0.0, 0.0, 1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                    1.0,
                )
                .map_err(curve_error)?,
            }),
            1 => {
                let other = args.get_item(0)?;
                let other = other.extract::<PyRef<'_, PyParabola>>()?;
                Ok(Self { curve: other.curve })
            }
            3 => {
                let focus = vector(&args.get_item(0)?)?;
                let center = vector(&args.get_item(1)?)?;
                let normal = vector(&args.get_item(2)?)?;
                let x_axis = focus.sub(&center);
                let focal = x_axis.length();
                Ok(Self {
                    curve: Parabola::with_axis(center, normal, x_axis, focal)
                        .map_err(curve_error)?,
                })
            }
            _ => Err(PyTypeError::new_err(
                "Parabola constructor accepts:\n\
                 -- empty parameter list\n\
                 -- Parabola\n\
                 -- Point, Point, Point",
            )),
        }
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.vertex())
    }

    /// The vertex (upstream's newer name for `Center`).
    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.vertex())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.normal())
    }

    #[getter]
    fn XAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.x_axis())
    }

    #[getter]
    fn YAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.y_axis())
    }

    #[getter]
    fn Focal(&self) -> f64 {
        self.curve.focal()
    }

    #[setter]
    fn set_Focal(&mut self, value: f64) -> PyResult<()> {
        self.curve = Parabola::with_axis(
            self.curve.vertex(),
            self.curve.normal(),
            self.curve.x_axis(),
            value,
        )
        .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn Focus(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.focus())
    }

    #[getter]
    fn Parameter(&self) -> f64 {
        self.curve.parameter()
    }

    #[getter]
    fn Eccentricity(&self) -> f64 {
        self.curve.eccentricity()
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        -2.0e100
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        2.0e100
    }

    /// `value(u)`: `vertex + (u^2 / (4*focal))*X + u*Y`.
    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        if !u.is_finite() {
            return Err(OCCError::new_err("the parabola parameter must be finite"));
        }
        py_vector(py, self.curve.point_at(u))
    }

    /// Fit the parabola `x = a*y^2 + b*y + c` through three points that lie in a
    /// plane parallel to XY (upstream's `ParabolaPy::compute`).
    fn compute(
        &mut self,
        p1: &Bound<'_, PyAny>,
        p2: &Bound<'_, PyAny>,
        p3: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let v1 = vector(p1)?;
        let v2 = vector(p2)?;
        let v3 = vector(p3)?;
        if v1.sub(&v2).cross(&v3.sub(&v2)).length() < 0.0001 {
            return Err(OCCError::new_err("Points are collinear"));
        }
        let determinant = |m: [[f64; 3]; 3]| -> f64 {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let y = [v1.y, v2.y, v3.y];
        let x = [v1.x, v2.x, v3.x];
        let fit = determinant([
            [y[0] * y[0], y[0], 1.0],
            [y[1] * y[1], y[1], 1.0],
            [y[2] * y[2], y[2], 1.0],
        ]);
        if fit.abs() <= 1e-12 {
            return Err(OCCError::new_err("Points are collinear"));
        }
        let a = determinant([[x[0], y[0], 1.0], [x[1], y[1], 1.0], [x[2], y[2], 1.0]]) / fit;
        let b = determinant([
            [y[0] * y[0], x[0], 1.0],
            [y[1] * y[1], x[1], 1.0],
            [y[2] * y[2], x[2], 1.0],
        ]) / fit;
        let c = determinant([
            [y[0] * y[0], y[0], x[0]],
            [y[1] * y[1], y[1], x[1]],
            [y[2] * y[2], y[2], x[2]],
        ]) / fit;
        if !a.is_finite() || a.abs() <= 1e-12 {
            return Err(OCCError::new_err("Points are collinear"));
        }
        let focal = 1.0 / (4.0 * a.abs());
        let vertex_y = -b / (2.0 * a);
        let vertex_x = a * vertex_y * vertex_y + b * vertex_y + c;
        self.curve = Parabola::with_axis(
            Vector3::new(vertex_x, vertex_y, v1.z),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
            focal,
        )
        .map_err(curve_error)?;
        Ok(())
    }

    /// `trim(first, last)`: the `Part.ArcOfParabola` over the given range.
    #[pyo3(signature = (first = None, last = None))]
    fn trim(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyArcOfParabola>> {
        let (first, last) = required_range(first, last, "parabola")?;
        let arc = self.curve.trim(first, last).map_err(curve_error)?;
        Py::new(py, PyArcOfParabola { arc })
    }

    /// The exact quadratic B-spline over `[first, last]`.
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let (first, last) = required_range(first, last, "parabola")?;
        let nurbs = self.curve.nurbs(first, last).map_err(curve_error)?;
        spline_curve(py, &nurbs)
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        self.toNurbs(py, first, last)
    }

    /// The exact edge over `[first, last]` (a quadratic B-spline).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let (first, last) = required_range(first, last, "parabola")?;
        let nurbs = self.curve.nurbs(first, last).map_err(curve_error)?;
        spline_edge(py, &nurbs)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.Parabola({:?}, {})",
            self.curve.vertex(),
            self.curve.focal()
        )
    }
}

/// `Part.ArcOfParabola(parabola, first, last, sense=True)` — a portion of a
/// parabola (`Part::GeomArcOfParabola`), what [`PyParabola::trim`] returns.
#[pyclass(name = "ArcOfParabola", module = "Part")]
#[derive(Clone)]
pub struct PyArcOfParabola {
    arc: ArcOfParabola,
}

#[pymethods]
impl PyArcOfParabola {
    /// The registered type name (`Part::GeomArcOfParabola`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomArcOfParabola"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            arc: self.arc.clone(),
        }
    }

    #[new]
    #[pyo3(signature = (parabola, first, last, sense = true))]
    fn new(
        parabola: &Bound<'_, PyAny>,
        first: f64,
        last: f64,
        sense: bool,
    ) -> PyResult<Self> {
        let _ = sense;
        let curve = parabola.extract::<PyRef<'_, PyParabola>>()?.curve;
        let arc = curve.trim(first, last).map_err(curve_error)?;
        Ok(Self { arc })
    }

    /// The supporting parabola (`GeomArcOfParabola::getParabola`).
    #[getter]
    fn Parabola(&self, py: Python<'_>) -> PyResult<Py<PyParabola>> {
        Py::new(
            py,
            PyParabola {
                curve: self.arc.parabola(),
            },
        )
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().vertex())
    }

    #[getter]
    fn Location(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().vertex())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().normal())
    }

    #[getter]
    fn XAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().x_axis())
    }

    #[getter]
    fn YAxis(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().y_axis())
    }

    #[getter]
    fn Focal(&self) -> f64 {
        self.arc.parabola().focal()
    }

    #[setter]
    fn set_Focal(&mut self, value: f64) -> PyResult<()> {
        let parabola = Parabola::with_axis(
            self.arc.parabola().vertex(),
            self.arc.parabola().normal(),
            self.arc.parabola().x_axis(),
            value,
        )
        .map_err(curve_error)?;
        self.arc = ArcOfParabola::new(parabola, self.arc.first(), self.arc.last())
            .map_err(curve_error)?;
        Ok(())
    }

    #[getter]
    fn Focus(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.parabola().focus())
    }

    #[getter]
    fn Parameter(&self) -> f64 {
        self.arc.parabola().parameter()
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.arc.first()
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.arc.last()
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.start())
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.arc.end())
    }

    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        py_vector(py, self.arc.point_at(u))
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let first = first.unwrap_or(self.arc.first());
        let last = last.unwrap_or(self.arc.last());
        let nurbs = self
            .arc
            .parabola()
            .nurbs(first, last)
            .map_err(curve_error)?;
        spline_curve(py, &nurbs)
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        self.toNurbs(py, first, last)
    }

    /// The exact edge over the arc's range (or an explicit sub-range).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let first = first.unwrap_or(self.arc.first());
        let last = last.unwrap_or(self.arc.last());
        let nurbs = self
            .arc
            .parabola()
            .nurbs(first, last)
            .map_err(curve_error)?;
        spline_edge(py, &nurbs)
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.ArcOfParabola({}, {})",
            self.arc.first(),
            self.arc.last()
        )
    }
}

/// `Part.OffsetCurve(basis, offset, direction)` — a constant offset of another
/// curve (`Part::GeomOffsetCurve`). `Edge.Curve` reports this class for an edge
/// whose kernel curve is a `Geom_OffsetCurve`.
#[pyclass(name = "OffsetCurve", module = "Part")]
#[derive(Clone)]
pub struct PyOffsetCurve {
    curve: OffsetCurve,
}

#[pymethods]
impl PyOffsetCurve {
    /// The registered type name (`Part::GeomOffsetCurve`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomOffsetCurve"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: self.curve.clone(),
        }
    }

    #[new]
    fn new(
        basis: &Bound<'_, PyAny>,
        offset: f64,
        direction: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let entry = PyEdge::entry_from(basis)?;
        let basis_curve = match entry.curve {
            Some(curve) => curve,
            None => backend().curve_of(&entry.shape).map_err(kernel_error)?,
        };
        let direction = vector(direction)?;
        let (first, last) = basis_curve
            .parameter_range()
            .ok_or_else(|| OCCError::new_err("an offset needs a bounded basis curve"))?;
        let curve = OffsetCurve::new(basis_curve, offset, direction, first, last)
            .map_err(curve_error)?;
        Ok(Self { curve })
    }

    /// The curve being offset (upstream's `BasisCurve`).
    #[getter]
    fn BasisCurve(&self, py: Python<'_>) -> PyResult<PyObject> {
        curve_to_py(py, self.curve.basis().clone())
    }

    /// The signed offset distance.
    #[getter]
    fn Offset(&self) -> f64 {
        self.curve.offset()
    }

    /// The unit direction the offset is measured against.
    #[getter]
    fn Direction(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve.direction())
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.curve.first()
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.curve.last()
    }

    /// The point at the basis parameter `u`.
    fn value(&self, py: Python<'_>, parameter: f64) -> PyResult<PyObject> {
        py_vector(py, self.curve.point_at(parameter))
    }

    /// The offset as an edge.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(
            py,
            PyEdge::from_curve(EdgeCurve::OffsetCurve(self.curve.clone()))?,
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.OffsetCurve({}, {})",
            self.curve.basis().kind(),
            self.curve.offset()
        )
    }
}

/// `Part.BezierCurve()` — a rational or non-rational Bezier curve
/// (`Part::GeomBezierCurve`), evaluated with de Casteljau subdivision.
///
/// A default curve has upstream's two poles `(0, 0, 0)`/`(0, 0, 1)`; use
/// `setPoles` (or the optional poles argument) to shape it. Every degree up to
/// OCCT's 25 is supported, and `toShape`/`toNurbs` convert it exactly to a
/// clamped B-spline.
#[pyclass(name = "BezierCurve", module = "Part")]
pub struct PyBezierCurve {
    curve: Mutex<BezierCurve>,
}

impl PyBezierCurve {
    fn curve(&self) -> BezierCurve {
        self.curve.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn store(&self, curve: BezierCurve) {
        *self.curve.lock().unwrap_or_else(|e| e.into_inner()) = curve;
    }
}

#[pymethods]
impl PyBezierCurve {
    /// The registered type name (`Part::GeomBezierCurve`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomBezierCurve"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            curve: Mutex::new(self.curve()),
        }
    }

    /// `Part.BezierCurve()` (upstream's default two-pole curve) or
    /// `Part.BezierCurve(poles, weights=None)`.
    #[new]
    #[pyo3(signature = (poles = None, weights = None))]
    fn new(
        poles: Option<&Bound<'_, PyAny>>,
        weights: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let Some(poles) = poles else {
            return Ok(Self {
                curve: Mutex::new(BezierCurve::default()),
            });
        };
        if let Ok(other) = poles.extract::<PyRef<'_, PyBezierCurve>>() {
            return Ok(Self {
                curve: Mutex::new(other.curve()),
            });
        }
        let points = parse_points(poles)?;
        let weights = match weights {
            Some(weights) => parse_floats(weights)?,
            None => vec![1.0; points.len()],
        };
        let curve = BezierCurve::new(points, weights).map_err(curve_error)?;
        Ok(Self {
            curve: Mutex::new(curve),
        })
    }

    #[getter]
    fn Degree(&self) -> i32 {
        self.curve().degree()
    }

    #[getter]
    fn MaxDegree(&self) -> i32 {
        25
    }

    #[getter]
    fn NbPoles(&self) -> usize {
        self.curve().poles().len()
    }

    #[getter]
    fn StartPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve().start())
    }

    #[getter]
    fn EndPoint(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.curve().end())
    }

    #[getter]
    fn FirstParameter(&self) -> f64 {
        self.curve().first()
    }

    #[getter]
    fn LastParameter(&self) -> f64 {
        self.curve().last()
    }

    fn isRational(&self) -> bool {
        self.curve().is_rational()
    }

    fn isPeriodic(&self) -> bool {
        false
    }

    fn isClosed(&self) -> bool {
        self.curve().is_closed()
    }

    fn getPole(&self, py: Python<'_>, index: i32) -> PyResult<PyObject> {
        let curve = self.curve();
        let position = index_at(curve.poles().len(), index)? as usize - 1;
        py_vector(py, curve.poles()[position])
    }

    fn getPoles(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let curve = self.curve();
        curve
            .poles()
            .iter()
            .map(|pole| py_vector(py, *pole))
            .collect()
    }

    #[pyo3(signature = (index, pole, weight = None))]
    fn setPole(
        &self,
        index: i32,
        pole: &Bound<'_, PyAny>,
        weight: Option<f64>,
    ) -> PyResult<()> {
        let mut curve = self.curve();
        let position = index_at(curve.poles().len(), index)? as usize - 1;
        // Upstream treats a negative weight as "keep the current one".
        let weight = weight.filter(|weight| *weight >= 0.0);
        curve
            .set_pole(position, vector(pole)?, weight)
            .map_err(curve_error)?;
        self.store(curve);
        Ok(())
    }

    fn setPoles(&self, poles: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut curve = self.curve();
        curve.set_poles(parse_points(poles)?).map_err(curve_error)?;
        self.store(curve);
        Ok(())
    }

    fn getWeight(&self, index: i32) -> PyResult<f64> {
        let curve = self.curve();
        let position = index_at(curve.weights().len(), index)? as usize - 1;
        Ok(curve.weights()[position])
    }

    fn setWeight(&self, index: i32, weight: f64) -> PyResult<()> {
        let mut curve = self.curve();
        let position = index_at(curve.weights().len(), index)? as usize - 1;
        curve.set_weight(position, weight).map_err(curve_error)?;
        self.store(curve);
        Ok(())
    }

    fn getWeights(&self) -> Vec<f64> {
        self.curve().weights().to_vec()
    }

    /// `value(u)`: rational de Casteljau on the curve's own range.
    fn value(&self, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        if !u.is_finite() {
            return Err(OCCError::new_err("the Bezier parameter must be finite"));
        }
        py_vector(py, self.curve().point_at(u))
    }

    /// `trim(first, last)`: the segmented curve, still a `Part.BezierCurve`
    /// (upstream's `makeFromTrimmedCurve` returns a Bezier here).
    #[pyo3(signature = (first = None, last = None))]
    fn trim(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBezierCurve>> {
        let curve = self.curve();
        let first = first.unwrap_or(curve.first());
        let last = last.unwrap_or(curve.last());
        let part = curve.trim(first, last).map_err(curve_error)?;
        Py::new(
            py,
            PyBezierCurve {
                curve: Mutex::new(part),
            },
        )
    }

    /// `segment(u1, u2)`: trim in place (`Geom_BezierCurve::Segment`).
    fn segment(&self, first: f64, last: f64) -> PyResult<()> {
        let part = self.curve().trim(first, last).map_err(curve_error)?;
        self.store(part);
        Ok(())
    }

    /// The exact B-spline form over `[first, last]`.
    #[pyo3(signature = (first = None, last = None))]
    fn toNurbs(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        let curve = self.curve();
        let first = first.unwrap_or(curve.first());
        let last = last.unwrap_or(curve.last());
        let part = curve.trim(first, last).map_err(curve_error)?;
        spline_curve(py, &part.nurbs())
    }

    #[pyo3(signature = (first = None, last = None))]
    fn toBSpline(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyBSplineCurve>> {
        self.toNurbs(py, first, last)
    }

    /// The exact edge over `[first, last]` (a clamped B-spline).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let curve = self.curve();
        let first = first.unwrap_or(curve.first());
        let last = last.unwrap_or(curve.last());
        let part = curve.trim(first, last).map_err(curve_error)?;
        spline_edge(py, &part.nurbs())
    }

    fn __repr__(&self) -> String {
        let curve = self.curve();
        format!(
            "Part.BezierCurve({} poles, degree {})",
            curve.poles().len(),
            curve.degree()
        )
    }
}

/// Whether two elementary surface frames are the same within tolerance, the way
/// upstream's `GeomElementarySurface::isSame` compares a `gp_Ax3`: the locations
/// coincide and the directions agree (allowing the right-handed frame flips OCC
/// may choose).
fn same_surface_frame(
    first: (&Vector3, &Vector3, &Vector3, &Vector3),
    second: (&Vector3, &Vector3, &Vector3, &Vector3),
    tol: f64,
    atol: f64,
) -> bool {
    fn angle(a: &Vector3, b: &Vector3) -> f64 {
        a.dot(b).clamp(-1.0, 1.0).acos()
    }
    let (a_loc, a_dir, a_x, a_y) = first;
    let (b_loc, b_dir, b_x, b_y) = second;
    a_loc.sub(b_loc).sqr() <= tol * tol
        && angle(a_dir, b_dir) <= atol
        && ((angle(a_x, b_x) <= atol && angle(a_y, b_y) <= atol)
            || (angle(a_x, b_y) <= atol && angle(a_y, &b_x.neg()) <= atol)
            || (angle(a_y, b_x) <= atol && angle(a_x, &b_y.neg()) <= atol))
}

/// `Part.Plane(origin=None, normal=None)` — an infinite plane, the XY plane
/// (`Part.Plane()`) by default.
#[pyclass(name = "Plane", module = "Part")]
#[derive(Clone)]
pub struct PyPlane {
    location: Vector3,
    /// The unit normal, as upstream's `Geom_Plane` keeps a `gp_Dir`.
    normal: Vector3,
    /// The in-plane `u` direction (upstream's `gp_Ax3::XDirection`).
    x_dir: Vector3,
    /// The in-plane `v` direction (upstream's `gp_Ax3::YDirection`).
    y_dir: Vector3,
}

impl PyPlane {
    /// A plane through `origin` with `normal` (normalised), for the Rust side
    /// (`Shape.findPlane`). The frame is the one [`plane_frame`] derives, which
    /// `toShape` also uses. The Python constructor validates the same way.
    pub(crate) fn from_parts(origin: Vector3, normal: Vector3) -> Self {
        let normal = normal.normalize();
        let (x_dir, y_dir) = plane_frame(&normal);
        Self {
            location: origin,
            normal,
            x_dir,
            y_dir,
        }
    }

    /// A plane with the backend's own surface frame (`gp_Ax3`), so
    /// [`parameter`](Self::parameter) and `Face.valueAt` agree exactly.
    pub(crate) fn from_frame(
        origin: Vector3,
        normal: Vector3,
        x_dir: Vector3,
        y_dir: Vector3,
    ) -> Self {
        Self {
            location: origin,
            normal: normal.normalize(),
            x_dir: x_dir.normalize(),
            y_dir: y_dir.normalize(),
        }
    }
}

#[pymethods]
impl PyPlane {
    /// The registered type name (`Part::GeomPlane`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomPlane"
    }

    /// An independent copy (`Geometry.copy`) with the same surface.
    fn copy(&self) -> Self {
        Self {
            location: self.location,
            normal: self.normal,
            x_dir: self.x_dir,
            y_dir: self.y_dir,
        }
    }

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
        Ok(Self::from_parts(location, normal))
    }

    /// The plane's origin (upstream's `Plane.Position`).
    #[getter]
    fn Position(&self, py: Python<'_>) -> PyResult<PyObject> {
        py_vector(py, self.location)
    }

    /// The unit normal at `(u, v)` (upstream `GeomSurface.normal`); it is
    /// constant over a plane. `Plane.Axis` is the same direction as a property.
    fn normal(&self, py: Python<'_>, _u: f64, _v: f64) -> PyResult<PyObject> {
        py_vector(py, self.normal)
    }

    /// The parameters `(u, v)` of `point` (`Plane.parameter`): the point's
    /// coordinates in the plane frame, `u = (p - origin)·x_dir` and
    /// `v = (p - origin)·y_dir`.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.location);
        Ok(
            PyTuple::new(py, [offset.dot(&self.x_dir), offset.dot(&self.y_dir)])?
                .into_any()
                .unbind(),
        )
    }

    /// The point at surface parameters `(u, v)` (`GeomSurface.value`).
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let point = self
            .location
            .add(&self.x_dir.scale(u))
            .add(&self.y_dir.scale(v));
        py_vector(py, point)
    }

    /// A plane is periodic in neither parameter (`GeomSurface.isUPeriodic`).
    fn isUPeriodic(&self) -> bool {
        false
    }

    /// A plane is periodic in neither parameter (`GeomSurface.isVPeriodic`).
    fn isVPeriodic(&self) -> bool {
        false
    }

    /// A plane has no period (`GeomSurface.UPeriod`).
    fn UPeriod(&self) -> f64 {
        0.0
    }

    /// A plane has no period (`GeomSurface.VPeriod`).
    fn VPeriod(&self) -> f64 {
        0.0
    }

    /// The principal curvature in `kind` (`GeomSurface.curvature`); a plane is
    /// flat, so every kind is zero.
    fn curvature(&self, _u: f64, _v: f64, _kind: &str) -> f64 {
        0.0
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

    /// Whether `other` is the same plane within `tol`/`atol`
    /// (`GeomSurface.isSame`).
    #[pyo3(signature = (other, tol = 1e-7, atol = 1e-10))]
    fn isSame(&self, other: &Bound<'_, PyAny>, tol: f64, atol: f64) -> bool {
        let Ok(other) = other.extract::<PyRef<'_, PyPlane>>() else {
            return false;
        };
        same_surface_frame(
            (&self.location, &self.normal, &self.x_dir, &self.y_dir),
            (&other.location, &other.normal, &other.x_dir, &other.y_dir),
            tol,
            atol,
        )
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
/// The object is accepted by [`PyCurve2d::toShape`] as the `surface`
/// argument. It answers no shape queries (`Faces`, `Edges`, …): no B-Rep geometry
/// is faked here.
#[pyclass(name = "Cylinder", module = "Part")]
#[derive(Clone)]
pub struct PyCylinder {
    radius: f64,
    center: Vector3,
    /// The unit axis direction (upstream's `Axis` on the surface is a direction).
    axis: Vector3,
    /// The surface's `u = 0` direction (upstream's `gp_Ax3::XDirection`).
    x_dir: Vector3,
    /// Whether the frame is direct (`y_dir = axis × x_dir`) or indirect.
    direct: bool,
}

impl PyCylinder {
    fn y_dir(&self) -> Vector3 {
        let y = self.axis.cross(&self.x_dir);
        if self.direct {
            y
        } else {
            y.neg()
        }
    }

    /// A cylinder with the backend's own surface frame (`gp_Ax3`), so
    /// [`parameter`](Self::parameter) and `Face.valueAt` agree exactly.
    pub(crate) fn from_parts(
        radius: f64,
        origin: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        direct: bool,
    ) -> Self {
        Self {
            radius,
            center: origin,
            axis: axis.normalize(),
            x_dir: x_dir.normalize(),
            direct,
        }
    }
}

#[pymethods]
impl PyCylinder {
    /// The registered type name (`Part::GeomCylinder`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomCylinder"
    }

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
        let axis = axis.normalize();
        let (x_dir, _) = plane_frame(&axis);
        Ok(Self {
            radius,
            center,
            axis,
            x_dir,
            direct: true,
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

    /// Whether `other` is the same cylinder within `tol`/`atol`
    /// (`GeomSurface.isSame`), including the radius.
    #[pyo3(signature = (other, tol = 1e-7, atol = 1e-10))]
    fn isSame(&self, other: &Bound<'_, PyAny>, tol: f64, atol: f64) -> bool {
        let Ok(other) = other.extract::<PyRef<'_, PyCylinder>>() else {
            return false;
        };
        let y_dir = self.axis.cross(&self.x_dir);
        let other_y = other.axis.cross(&other.x_dir);
        (self.radius - other.radius).abs() <= tol
            && same_surface_frame(
                (&self.center, &self.axis, &self.x_dir, &y_dir),
                (&other.center, &other.axis, &other.x_dir, &other_y),
                tol,
                atol,
            )
    }

    /// The unit normal at `(u, v)` (upstream `GeomSurface.normal`): radial at
    /// `u`, independent of `v`.
    fn normal(&self, py: Python<'_>, u: f64, _v: f64) -> PyResult<PyObject> {
        let y_dir = self.y_dir();
        let radial = self.x_dir.scale(u.cos()).add(&y_dir.scale(u.sin()));
        py_vector(py, radial)
    }

    /// The parameters `(u, v)` of `point` (`Cylinder.parameter`): the angle
    /// around the axis measured from `x_dir`, normalized to OCCT's `[0, 2π)`, and
    /// the height along the axis.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.center);
        let y_dir = self.y_dir();
        let mut u = offset.dot(&y_dir).atan2(offset.dot(&self.x_dir));
        if u < 0.0 {
            u += std::f64::consts::TAU;
        }
        Ok(PyTuple::new(py, [u, offset.dot(&self.axis)])?
            .into_any()
            .unbind())
    }

    /// The point at surface parameters `(u, v)` (`GeomSurface.value`): the
    /// radius at angle `u`, at height `v` along the axis.
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let y_dir = self.y_dir();
        let radial = self.x_dir.scale(u.cos()).add(&y_dir.scale(u.sin()));
        let point = self
            .center
            .add(&radial.scale(self.radius))
            .add(&self.axis.scale(v));
        py_vector(py, point)
    }

    /// A cylinder is periodic around its axis (`GeomSurface.isUPeriodic`).
    fn isUPeriodic(&self) -> bool {
        true
    }

    /// A cylinder's height is unbounded, not periodic (`GeomSurface.isVPeriodic`).
    fn isVPeriodic(&self) -> bool {
        false
    }

    /// The angular period around the axis (`GeomSurface.UPeriod`).
    fn UPeriod(&self) -> f64 {
        std::f64::consts::TAU
    }

    /// A cylinder has no V period (`GeomSurface.VPeriod`).
    fn VPeriod(&self) -> f64 {
        0.0
    }

    /// The principal curvature in `kind` (`GeomSurface.curvature`): `1/r` around
    /// the circumference and `0` along the axis.
    fn curvature(&self, _u: f64, _v: f64, kind: &str) -> PyResult<f64> {
        let curvature = 1.0 / self.radius;
        Ok(match kind {
            "Max" | "Maximum" => curvature,
            "Min" | "Minimum" | "Gaussian" => 0.0,
            "Mean" => curvature / 2.0,
            other => {
                return Err(OCCError::new_err(format!(
                    "unknown curvature kind {other:?}; use Max, Min, Gaussian or Mean"
                )));
            }
        })
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
/// extension. It is accepted by [`PyCurve2d::toShape`] as the `surface`
/// argument, and answers no shape queries: no B-Rep geometry is faked here.
#[pyclass(name = "Sphere", module = "Part")]
#[derive(Clone)]
pub struct PySphere {
    radius: f64,
    center: Vector3,
    /// The unit axis direction (upstream's `Sphere.Axis` is a direction).
    axis: Vector3,
    /// The surface's `u = 0` direction (upstream's `gp_Ax3::XDirection`).
    x_dir: Vector3,
    /// Whether the frame is direct (`y_dir = axis × x_dir`) or indirect.
    direct: bool,
}

impl PySphere {
    fn y_dir(&self) -> Vector3 {
        let y = self.axis.cross(&self.x_dir);
        if self.direct {
            y
        } else {
            y.neg()
        }
    }
    /// A sphere with the backend's own surface frame (`gp_Ax3`), so
    /// [`parameter`](Self::parameter) and `Face.valueAt` agree exactly.
    pub(crate) fn from_parts(
        radius: f64,
        center: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        direct: bool,
    ) -> Self {
        Self {
            radius,
            center,
            axis: axis.normalize(),
            x_dir: x_dir.normalize(),
            direct,
        }
    }
}

#[pymethods]
impl PySphere {
    /// The registered type name (`Part::GeomSphere`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomSphere"
    }

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
        let axis = axis.normalize();
        let (x_dir, _) = plane_frame(&axis);
        Ok(Self {
            radius,
            center,
            axis,
            x_dir,
            direct: true,
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

    /// Whether `other` is the same sphere within `tol`/`atol`
    /// (`GeomSurface.isSame`), including the radius.
    #[pyo3(signature = (other, tol = 1e-7, atol = 1e-10))]
    fn isSame(&self, other: &Bound<'_, PyAny>, tol: f64, atol: f64) -> bool {
        let Ok(other) = other.extract::<PyRef<'_, PySphere>>() else {
            return false;
        };
        let y_dir = self.y_dir();
        let other_y = other.y_dir();
        (self.radius - other.radius).abs() <= tol
            && same_surface_frame(
                (&self.center, &self.axis, &self.x_dir, &y_dir),
                (&other.center, &other.axis, &other.x_dir, &other_y),
                tol,
                atol,
            )
    }

    /// The unit normal at `(u, v)` (upstream `GeomSurface.normal`): the radial
    /// direction at that longitude/latitude.
    fn normal(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let y_dir = self.y_dir();
        let radial = self
            .x_dir
            .scale(v.cos() * u.cos())
            .add(&y_dir.scale(v.cos() * u.sin()))
            .add(&self.axis.scale(v.sin()));
        py_vector(py, radial)
    }

    /// The parameters `(u, v)` of `point` (`Sphere.parameter`): the longitude
    /// around the axis measured from `x_dir`, normalized to OCCT's `[0, 2π)`, and
    /// the latitude in `[-π/2, π/2]` measured from the equator.
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let point = vector(point)?;
        let offset = point.sub(&self.center);
        let y_dir = self.y_dir();
        let mut u = offset.dot(&y_dir).atan2(offset.dot(&self.x_dir));
        if u < 0.0 {
            u += std::f64::consts::TAU;
        }
        let latitude = (offset.dot(&self.axis) / self.radius).clamp(-1.0, 1.0);
        Ok(PyTuple::new(py, [u, latitude.asin()])?
            .into_any()
            .unbind())
    }

    /// The point at surface parameters `(u, v)` (`GeomSurface.value`).
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let y_dir = self.y_dir();
        let radial = self
            .x_dir
            .scale(v.cos() * u.cos())
            .add(&y_dir.scale(v.cos() * u.sin()))
            .add(&self.axis.scale(v.sin()));
        py_vector(py, self.center.add(&radial.scale(self.radius)))
    }

    /// A sphere is periodic in longitude (`GeomSurface.isUPeriodic`).
    fn isUPeriodic(&self) -> bool {
        true
    }

    /// A sphere is not periodic in latitude (`GeomSurface.isVPeriodic`).
    fn isVPeriodic(&self) -> bool {
        false
    }

    /// The angular period around the axis (`GeomSurface.UPeriod`).
    fn UPeriod(&self) -> f64 {
        std::f64::consts::TAU
    }

    /// A sphere has no V period (`GeomSurface.VPeriod`).
    fn VPeriod(&self) -> f64 {
        0.0
    }

    /// The principal curvature in `kind` (`GeomSurface.curvature`); a sphere is
    /// umbilical, so every kind is `1/r` (Gaussian `1/r²`, mean `1/r`).
    fn curvature(&self, _u: f64, _v: f64, kind: &str) -> PyResult<f64> {
        let curvature = 1.0 / self.radius;
        Ok(match kind {
            "Max" | "Maximum" | "Min" | "Minimum" | "Mean" => curvature,
            "Gaussian" => curvature * curvature,
            other => {
                return Err(OCCError::new_err(format!(
                    "unknown curvature kind {other:?}; use Max, Min, Gaussian or Mean"
                )));
            }
        })
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
    /// The registered type name (`Part::GeomRectangularTrimmedSurface`).
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomRectangularTrimmedSurface"
    }

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

/// The backing surface of a face whose kind has no dedicated class
/// (`Face.Surface`). The face the surface was read from is kept so
/// `parameter`/`value` evaluate the very same surface.
#[pyclass(name = "GeomSurface", module = "Part")]
pub struct PySurface {
    type_id: String,
    /// The face the surface was read from. Behind a `Mutex` because a `#[pyclass]`
    /// must be `Sync` and a kernel shape handle is only `Send`.
    face: Mutex<Option<Shape>>,
}

impl PySurface {
    pub(crate) fn new(type_id: String, face: Option<Shape>) -> Self {
        Self {
            type_id,
            face: Mutex::new(face),
        }
    }
}

#[pymethods]
impl PySurface {
    #[getter]
    fn TypeId(&self) -> String {
        self.type_id.clone()
    }

    /// The `(u, v)` of `point` on the surface (`GeomSurface.parameter`).
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let face = self.surface_face()?;
        let (u, v) = ferrocad_occt::part_ops::face_parameter(&face, vector(point)?)
            .map_err(kernel_error)?;
        Ok(PyTuple::new(py, [u, v])?.into_any().unbind())
    }

    /// The point at surface parameters `(u, v)` (`GeomSurface.value`).
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let face = self.surface_face()?;
        let point = backend().face_value_at(&face, u, v).map_err(kernel_error)?;
        py_vector(py, point)
    }

    fn __repr__(&self) -> String {
        format!("Part.{}()", self.type_id.trim_start_matches("Part::"))
    }
}

impl PySurface {
    fn surface_face(&self) -> PyResult<Shape> {
        self.face
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| OCCError::new_err("this GeomSurface has no face to evaluate"))
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

/// Read a [`NurbsCurve`] back through OCCT so the Python curve and the edge
/// `toShape` builds agree exactly.
fn nurbs_state(curve: &NurbsCurve) -> PyResult<SplineState> {
    let blob = occt()
        .bspline_state(
            curve.poles(),
            curve.weights(),
            curve.knots(),
            curve.mults(),
            curve.degree(),
            curve.is_periodic(),
            !curve.is_rational(),
        )
        .ok_or_else(|| OCCError::new_err("conversion to a B-spline failed"))?;
    SplineState::from_blob(&blob).ok_or_else(|| OCCError::new_err("conversion to a B-spline failed"))
}

/// A `Part.BSplineCurve` carrying the exact conversion of a conic or a Bezier.
fn spline_curve(py: Python<'_>, curve: &NurbsCurve) -> PyResult<Py<PyBSplineCurve>> {
    Py::new(py, PyBSplineCurve::from_state(nurbs_state(curve)?))
}

/// An exact edge from a B-spline description. `Edge.Curve` reports that very
/// B-spline (the edge's stored geometry), as OCCT would for the built edge.
fn spline_edge(py: Python<'_>, curve: &NurbsCurve) -> PyResult<Py<PyEdge>> {
    let state = nurbs_state(curve)?;
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
    Py::new(py, PyEdge::from_bspline(shape, state))
}

/// A hyperbola and a parabola are unbounded, so converting one needs an explicit
/// finite parameter range (upstream defaults to `±Infinity`, which fails).
fn required_range(first: Option<f64>, last: Option<f64>, what: &str) -> PyResult<(f64, f64)> {
    match (first, last) {
        (Some(first), Some(last)) => Ok((first, last)),
        _ => Err(OCCError::new_err(format!(
            "a {what} is unbounded; pass a finite parameter range"
        ))),
    }
}

pub(crate) fn parse_points(points: &Bound<'_, PyAny>) -> PyResult<Vec<Vector3>> {
    let mut parsed = Vec::new();
    for point in points.try_iter()? {
        parsed.push(vector(&point?)?);
    }
    Ok(parsed)
}

pub(crate) fn parse_floats(values: &Bound<'_, PyAny>) -> PyResult<Vec<f64>> {
    let mut parsed = Vec::new();
    for value in values.try_iter()? {
        parsed.push(value?.extract::<f64>()?);
    }
    Ok(parsed)
}

pub(crate) fn parse_integers(values: &Bound<'_, PyAny>) -> PyResult<Vec<i32>> {
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
    /// The registered type name (`Part::GeomBSplineCurve`), as upstream reports it.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomBSplineCurve"
    }

    /// An independent copy (`Geometry.copy`) with the same geometry.
    fn copy(&self) -> Self {
        Self {
            state: Mutex::new(self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()),
        }
    }

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

    /// The curve as an edge, or the piece between two parameters
    /// (`GeometryCurvePy::toShape`).
    #[pyo3(signature = (first = None, last = None))]
    fn toShape(
        &self,
        py: Python<'_>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<Py<PyEdge>> {
        let state = self.state()?;
        let state = match (first, last) {
            (None, None) => state,
            (Some(first), Some(last)) => {
                let blob = occt()
                    .bspline_segment(
                        &state.poles,
                        &state.weights,
                        &state.knots,
                        &state.mults,
                        state.degree,
                        state.periodic,
                        state.check_rational(),
                        first,
                        last,
                    )
                    .ok_or_else(|| {
                        OCCError::new_err("the B-spline cannot be trimmed to that parameter range")
                    })?;
                SplineState::from_blob(&blob)
                    .ok_or_else(|| OCCError::new_err("failed to trim the B-spline"))?
            }
            _ => {
                return Err(OCCError::new_err(
                    "BSplineCurve.toShape needs both first and last parameters",
                ));
            }
        };
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

    /// `segment(u1, u2)`: restrict this curve to `[u1, u2]` in place
    /// (`Geom_BSplineCurve::Segment`).
    fn segment(&self, u1: f64, u2: f64) -> PyResult<()> {
        let state = self.state()?;
        let blob = occt()
            .bspline_segment(
                &state.poles,
                &state.weights,
                &state.knots,
                &state.mults,
                state.degree,
                state.periodic,
                state.check_rational(),
                u1,
                u2,
            )
            .ok_or_else(|| OCCError::new_err("Failed to segment BSpline curve"))?;
        let trimmed = SplineState::from_blob(&blob)
            .ok_or_else(|| OCCError::new_err("Failed to segment BSpline curve"))?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(trimmed);
        Ok(())
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

    /// The B-spline description behind this curve, for `Part.Edge(...)`.
    fn to_nurbs(&self) -> PyResult<NurbsCurve> {
        let state = self.state()?;
        let periodic = state.periodic;
        let (degree, knots, mults, poles, weights) =
            (state.degree, state.knots, state.mults, state.poles, state.weights);
        if periodic {
            NurbsCurve::new_periodic(degree, knots, mults, poles, weights).map_err(curve_error)
        } else {
            NurbsCurve::new(degree, knots, mults, poles, weights).map_err(curve_error)
        }
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
//
// Upstream's `Part::Geom2d*` classes are the 2D counterpart of the `Part::Geom*`
// curves above (`src/Mod/Part/App/Geometry2d.h`). This slice keeps one geometry
// model: every `Part.Geom2d` curve is the XY projection of the matching
// `ferrocad_curves` curve (the same types `Part.Circle`, `Part.Ellipse`, …
// wrap), with `z = 0`; `Line2dSegment` and `OffsetCurve2d` have no exact 3D
// counterpart and are stored explicitly.
//
// `value`, the parameter range, the tangents, the curvature and the length are
// evaluated from that geometry (never from a recorded sample), and `toShape()`
// builds the corresponding XY edge. `toShape(surface)` builds the same XY edge
// and attaches the 2D curve as its pcurve, but does **not** project it onto the
// surface: the seam binds no `GeomProjLib`, exactly as `Line2dSegment` already
// documented before this slice.

/// Upstream's `Precision::Infinite()` (`1e100`), the parameter limit of an
/// unbounded 2D curve.
const GEOM2D_INFINITE: f64 = 1.0e100;

fn xyz(x: f64, y: f64) -> Vector3 {
    Vector3::new(x, y, 0.0)
}

fn xy(point: Vector3) -> (f64, f64) {
    (point.x, point.y)
}

fn unit_normal() -> Vector3 {
    Vector3::new(0.0, 0.0, 1.0)
}

fn unit_x() -> Vector3 {
    Vector3::new(1.0, 0.0, 0.0)
}

/// A unit 2D vector, or `None` for a (near) zero or non-finite input.
fn unit2(vector: (f64, f64)) -> Option<(f64, f64)> {
    let length = (vector.0 * vector.0 + vector.1 * vector.1).sqrt();
    if length.is_finite() && length > TOLERANCE {
        Some((vector.0 / length, vector.1 / length))
    } else {
        None
    }
}

/// The 2D cross product (the signed area of the parallelogram).
fn cross2(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn distance2(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// A clear failure for a capability this slice's 2D seam does not have.
fn geom2d_not_implemented(what: &str) -> PyErr {
    PyNotImplementedError::new_err(format!(
        "Part.Geom2d.{what} is not implemented in this slice: the 2D seam has no \
         such capability"
    ))
}

/// The geometry behind a `Part.Geom2d` object.
#[derive(Clone)]
pub(crate) enum Geom2dData {
    /// An infinite line (`Geom2d_Line`), parameterised by the distance along
    /// its direction.
    Line(Line),
    /// A trimmed line (`Geom2d_TrimmedCurve` on a `Geom2d_Line`):
    /// `value(u) = origin + u * direction` over `[first, last]`.
    Segment {
        origin: (f64, f64),
        direction: (f64, f64),
        first: f64,
        last: f64,
    },
    Circle(Circle),
    Arc(Arc),
    Ellipse(Ellipse),
    ArcOfEllipse {
        ellipse: Ellipse,
        first: f64,
        last: f64,
    },
    Hyperbola(Hyperbola),
    ArcOfHyperbola(ArcOfHyperbola),
    Parabola(Parabola),
    ArcOfParabola(ArcOfParabola),
    Bezier(BezierCurve),
    /// The points a `BSplineCurve2d` was asked to fit. This slice binds no 2D
    /// B-spline kernel, so no curve is built behind that record.
    BSpline(Vec<(f64, f64)>),
    Offset {
        basis: Box<Geom2dData>,
        offset: f64,
    },
}

impl Geom2dData {
    fn kind(&self) -> &'static str {
        match self {
            Self::Line(_) => "Line2d",
            Self::Segment { .. } => "Line2dSegment",
            Self::Circle(_) => "Circle2d",
            Self::Arc(_) => "ArcOfCircle2d",
            Self::Ellipse(_) => "Ellipse2d",
            Self::ArcOfEllipse { .. } => "ArcOfEllipse2d",
            Self::Hyperbola(_) => "Hyperbola2d",
            Self::ArcOfHyperbola(_) => "ArcOfHyperbola2d",
            Self::Parabola(_) => "Parabola2d",
            Self::ArcOfParabola(_) => "ArcOfParabola2d",
            Self::Bezier(_) => "BezierCurve2d",
            Self::BSpline(_) => "BSplineCurve2d",
            Self::Offset { .. } => "OffsetCurve2d",
        }
    }

    /// The point at parameter `u`; `None` when no curve exists behind the data.
    fn point(&self, u: f64) -> Option<(f64, f64)> {
        Some(match self {
            Self::Line(line) => xy(line.point_at(u)),
            Self::Segment {
                origin, direction, ..
            } => (
                origin.0 + u * direction.0,
                origin.1 + u * direction.1,
            ),
            Self::Circle(circle) => xy(circle.point_at(u)),
            Self::Arc(arc) => xy(arc.point_at(u)),
            Self::Ellipse(ellipse) => xy(ellipse.point_at(u)),
            Self::ArcOfEllipse { ellipse, .. } => xy(ellipse.point_at(u)),
            Self::Hyperbola(hyperbola) => xy(hyperbola.point_at(u)),
            Self::ArcOfHyperbola(arc) => xy(arc.point_at(u)),
            Self::Parabola(parabola) => xy(parabola.point_at(u)),
            Self::ArcOfParabola(arc) => xy(arc.point_at(u)),
            Self::Bezier(curve) => xy(curve.point_at(u)),
            Self::BSpline(_) => return None,
            Self::Offset { basis, offset } => {
                let point = basis.point(u)?;
                let tangent = basis.tangent(u)?;
                // The left normal of the basis curve.
                (point.0 - offset * tangent.1, point.1 + offset * tangent.0)
            }
        })
    }

    /// The unit tangent at parameter `u`.
    fn tangent(&self, u: f64) -> Option<(f64, f64)> {
        Some(match self {
            Self::Line(line) => xy(line.tangent_at(u)),
            Self::Segment { direction, .. } => *direction,
            Self::Circle(circle) => xy(circle.tangent_at(u)),
            Self::Arc(arc) => xy(arc.tangent_at(u)),
            Self::Ellipse(ellipse) => xy(ellipse.tangent_at(u)),
            Self::ArcOfEllipse { ellipse, .. } => xy(ellipse.tangent_at(u)),
            Self::Hyperbola(hyperbola) => xy(hyperbola.tangent_at(u)),
            Self::ArcOfHyperbola(arc) => xy(arc.hyperbola().tangent_at(u)),
            Self::Parabola(parabola) => xy(parabola.tangent_at(u)),
            Self::ArcOfParabola(arc) => xy(arc.parabola().tangent_at(u)),
            Self::Bezier(curve) => xy(curve.tangent_at(u)),
            Self::BSpline(_) => return None,
            Self::Offset { .. } => {
                // The offset curve's derivative is not a named analytic
                // quantity here, so it is taken from the offset points.
                let step = 1e-6 * (1.0 + u.abs());
                let before = self.point(u - step)?;
                let after = self.point(u + step)?;
                unit2((after.0 - before.0, after.1 - before.1))?
            }
        })
    }

    /// `|dP/du|` at `u`.
    fn speed(&self, u: f64) -> Option<f64> {
        Some(match self {
            Self::Line(_) | Self::Segment { .. } => 1.0,
            Self::Circle(circle) => circle.radius(),
            Self::Arc(arc) => arc.circle().radius(),
            Self::Ellipse(ellipse) => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                (a * a * u.sin().powi(2) + b * b * u.cos().powi(2)).sqrt()
            }
            Self::ArcOfEllipse { ellipse, .. } => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                (a * a * u.sin().powi(2) + b * b * u.cos().powi(2)).sqrt()
            }
            Self::Hyperbola(hyperbola) => {
                let (a, b) = (hyperbola.major_radius(), hyperbola.minor_radius());
                (a * a * u.sinh().powi(2) + b * b * u.cosh().powi(2)).sqrt()
            }
            Self::ArcOfHyperbola(arc) => {
                let hyperbola = arc.hyperbola();
                let (a, b) = (hyperbola.major_radius(), hyperbola.minor_radius());
                (a * a * u.sinh().powi(2) + b * b * u.cosh().powi(2)).sqrt()
            }
            Self::Parabola(parabola) => {
                let f = parabola.focal();
                (u * u / (4.0 * f * f) + 1.0).sqrt()
            }
            Self::ArcOfParabola(arc) => {
                let f = arc.parabola().focal();
                (u * u / (4.0 * f * f) + 1.0).sqrt()
            }
            Self::Bezier(_) | Self::Offset { .. } => {
                let step = 1e-6 * (1.0 + u.abs());
                let before = self.point(u - step)?;
                let after = self.point(u + step)?;
                distance2(before, after) / (2.0 * step)
            }
            Self::BSpline(_) => return None,
        })
    }

    fn first(&self) -> Option<f64> {
        Some(match self {
            Self::Line(_) | Self::Hyperbola(_) | Self::Parabola(_) => -GEOM2D_INFINITE,
            Self::Segment { first, .. } => *first,
            Self::Circle(_) | Self::Ellipse(_) => 0.0,
            Self::Arc(arc) => arc.first(),
            Self::ArcOfEllipse { first, .. } => *first,
            Self::ArcOfHyperbola(arc) => arc.first(),
            Self::ArcOfParabola(arc) => arc.first(),
            Self::Bezier(curve) => curve.first(),
            Self::BSpline(_) => return None,
            Self::Offset { basis, .. } => basis.first()?,
        })
    }

    fn last(&self) -> Option<f64> {
        Some(match self {
            Self::Line(_) | Self::Hyperbola(_) | Self::Parabola(_) => GEOM2D_INFINITE,
            Self::Segment { last, .. } => *last,
            Self::Circle(_) | Self::Ellipse(_) => std::f64::consts::TAU,
            Self::Arc(arc) => arc.last(),
            Self::ArcOfEllipse { last, .. } => *last,
            Self::ArcOfHyperbola(arc) => arc.last(),
            Self::ArcOfParabola(arc) => arc.last(),
            Self::Bezier(curve) => curve.last(),
            Self::BSpline(_) => return None,
            Self::Offset { basis, .. } => basis.last()?,
        })
    }

    fn is_closed(&self) -> bool {
        match self {
            Self::Circle(_) | Self::Ellipse(_) => true,
            Self::Line(_) | Self::Hyperbola(_) | Self::Parabola(_) | Self::BSpline(_) => false,
            Self::Bezier(curve) => curve.is_closed(),
            Self::Offset { basis, .. } => basis.is_closed(),
            _ => match (self.first(), self.last()) {
                (Some(first), Some(last)) if first != last => {
                    match (self.point(first), self.point(last)) {
                        (Some(start), Some(end)) => distance2(start, end) <= TOLERANCE,
                        _ => false,
                    }
                }
                _ => false,
            },
        }
    }

    fn is_periodic(&self) -> bool {
        matches!(self, Self::Circle(_) | Self::Ellipse(_))
    }

    fn continuity(&self) -> &'static str {
        match self {
            Self::Segment { .. }
            | Self::Arc(_)
            | Self::ArcOfEllipse { .. }
            | Self::ArcOfHyperbola(_)
            | Self::ArcOfParabola(_)
            | Self::BSpline(_)
            | Self::Offset { .. } => "C0",
            _ => "CN",
        }
    }

    /// The location of a conic (its centre, or the vertex of a parabola).
    fn location(&self) -> Option<(f64, f64)> {
        Some(match self {
            Self::Line(line) => xy(line.origin()),
            Self::Segment { origin, .. } => *origin,
            Self::Circle(circle) => xy(circle.center()),
            Self::Arc(arc) => xy(arc.circle().center()),
            Self::Ellipse(ellipse) => xy(ellipse.center()),
            Self::ArcOfEllipse { ellipse, .. } => xy(ellipse.center()),
            Self::Hyperbola(hyperbola) => xy(hyperbola.center()),
            Self::ArcOfHyperbola(arc) => xy(arc.hyperbola().center()),
            Self::Parabola(parabola) => xy(parabola.vertex()),
            Self::ArcOfParabola(arc) => xy(arc.parabola().vertex()),
            _ => return None,
        })
    }

    fn normal(&self) -> Option<Vector3> {
        Some(match self {
            Self::Line(_) | Self::Segment { .. } | Self::Bezier(_) => unit_normal(),
            Self::Circle(circle) => circle.normal(),
            Self::Arc(arc) => arc.circle().normal(),
            Self::Ellipse(ellipse) => ellipse.normal(),
            Self::ArcOfEllipse { ellipse, .. } => ellipse.normal(),
            Self::Hyperbola(hyperbola) => hyperbola.normal(),
            Self::ArcOfHyperbola(arc) => arc.hyperbola().normal(),
            Self::Parabola(parabola) => parabola.normal(),
            Self::ArcOfParabola(arc) => arc.parabola().normal(),
            _ => return None,
        })
    }

    fn x_axis(&self) -> Option<(f64, f64)> {
        Some(match self {
            Self::Line(line) => xy(line.direction()),
            Self::Segment { direction, .. } => *direction,
            Self::Circle(circle) => xy(circle.x_axis()),
            Self::Arc(arc) => xy(arc.circle().x_axis()),
            Self::Ellipse(ellipse) => xy(ellipse.x_axis()),
            Self::ArcOfEllipse { ellipse, .. } => xy(ellipse.x_axis()),
            Self::Hyperbola(hyperbola) => xy(hyperbola.x_axis()),
            Self::ArcOfHyperbola(arc) => xy(arc.hyperbola().x_axis()),
            Self::Parabola(parabola) => xy(parabola.x_axis()),
            Self::ArcOfParabola(arc) => xy(arc.parabola().x_axis()),
            _ => return None,
        })
    }

    fn y_axis(&self) -> Option<(f64, f64)> {
        Some(match self {
            Self::Line(line) => xy(line.direction()),
            Self::Segment { direction, .. } => *direction,
            Self::Circle(circle) => xy(circle.y_axis()),
            Self::Arc(arc) => xy(arc.circle().y_axis()),
            Self::Ellipse(ellipse) => xy(ellipse.y_axis()),
            Self::ArcOfEllipse { ellipse, .. } => xy(ellipse.y_axis()),
            Self::Hyperbola(hyperbola) => xy(hyperbola.y_axis()),
            Self::ArcOfHyperbola(arc) => xy(arc.hyperbola().y_axis()),
            Self::Parabola(parabola) => xy(parabola.y_axis()),
            Self::ArcOfParabola(arc) => xy(arc.parabola().y_axis()),
            _ => return None,
        })
    }

    fn eccentricity(&self) -> Option<f64> {
        Some(match self {
            Self::Circle(_) | Self::Arc(_) => 0.0,
            Self::Ellipse(ellipse) => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                (1.0 - (b * b) / (a * a)).max(0.0).sqrt()
            }
            Self::ArcOfEllipse { ellipse, .. } => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                (1.0 - (b * b) / (a * a)).max(0.0).sqrt()
            }
            Self::Hyperbola(hyperbola) => hyperbola.eccentricity(),
            Self::ArcOfHyperbola(arc) => arc.hyperbola().eccentricity(),
            Self::Parabola(_) | Self::ArcOfParabola(_) => 1.0,
            _ => return None,
        })
    }

    /// A copy of a conic moved to `center`, keeping its axes and (for an arc)
    /// its parameter range.
    fn with_location(&self, center: (f64, f64)) -> Option<Self> {
        let point = xyz(center.0, center.1);
        Some(match self {
            Self::Line(line) => Self::Line(Line::new(point, line.direction()).ok()?),
            Self::Segment {
                direction,
                first,
                last,
                ..
            } => Self::Segment {
                origin: center,
                direction: *direction,
                first: *first,
                last: *last,
            },
            Self::Circle(circle) => Self::Circle(
                Circle::with_axis(point, circle.normal(), circle.x_axis(), circle.radius())
                    .ok()?,
            ),
            Self::Arc(arc) => {
                let circle = Circle::with_axis(
                    point,
                    arc.circle().normal(),
                    arc.circle().x_axis(),
                    arc.circle().radius(),
                )
                .ok()?;
                Self::Arc(Arc::new(circle, arc.first(), arc.last()).ok()?)
            }
            Self::Ellipse(ellipse) => Self::Ellipse(
                Ellipse::with_axis(
                    point,
                    ellipse.normal(),
                    ellipse.x_axis(),
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfEllipse {
                ellipse,
                first,
                last,
            } => Self::ArcOfEllipse {
                ellipse: Ellipse::with_axis(
                    point,
                    ellipse.normal(),
                    ellipse.x_axis(),
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                )
                .ok()?,
                first: *first,
                last: *last,
            },
            Self::Hyperbola(hyperbola) => Self::Hyperbola(
                Hyperbola::with_axis(
                    point,
                    hyperbola.normal(),
                    hyperbola.x_axis(),
                    hyperbola.major_radius(),
                    hyperbola.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfHyperbola(arc) => {
                let hyperbola = Hyperbola::with_axis(
                    point,
                    arc.hyperbola().normal(),
                    arc.hyperbola().x_axis(),
                    arc.hyperbola().major_radius(),
                    arc.hyperbola().minor_radius(),
                )
                .ok()?;
                Self::ArcOfHyperbola(ArcOfHyperbola::new(hyperbola, arc.first(), arc.last()).ok()?)
            }
            Self::Parabola(parabola) => Self::Parabola(
                Parabola::with_axis(point, parabola.normal(), parabola.x_axis(), parabola.focal())
                    .ok()?,
            ),
            Self::ArcOfParabola(arc) => {
                let parabola = Parabola::with_axis(
                    point,
                    arc.parabola().normal(),
                    arc.parabola().x_axis(),
                    arc.parabola().focal(),
                )
                .ok()?;
                Self::ArcOfParabola(ArcOfParabola::new(parabola, arc.first(), arc.last()).ok()?)
            }
            _ => return None,
        })
    }

    /// A copy of a conic with its `XAxis` direction replaced.
    fn with_x_axis(&self, direction: (f64, f64)) -> Option<Self> {
        let direction = unit2(direction)?;
        let axis = xyz(direction.0, direction.1);
        Some(match self {
            Self::Line(line) => Self::Line(Line::new(line.origin(), axis).ok()?),
            Self::Segment {
                origin,
                first,
                last,
                ..
            } => Self::Segment {
                origin: *origin,
                direction,
                first: *first,
                last: *last,
            },
            Self::Circle(circle) => Self::Circle(
                Circle::with_axis(circle.center(), circle.normal(), axis, circle.radius()).ok()?,
            ),
            Self::Arc(arc) => {
                let circle = Circle::with_axis(
                    arc.circle().center(),
                    arc.circle().normal(),
                    axis,
                    arc.circle().radius(),
                )
                .ok()?;
                Self::Arc(Arc::new(circle, arc.first(), arc.last()).ok()?)
            }
            Self::Ellipse(ellipse) => Self::Ellipse(
                Ellipse::with_axis(
                    ellipse.center(),
                    ellipse.normal(),
                    axis,
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfEllipse {
                ellipse,
                first,
                last,
            } => Self::ArcOfEllipse {
                ellipse: Ellipse::with_axis(
                    ellipse.center(),
                    ellipse.normal(),
                    axis,
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                )
                .ok()?,
                first: *first,
                last: *last,
            },
            Self::Hyperbola(hyperbola) => Self::Hyperbola(
                Hyperbola::with_axis(
                    hyperbola.center(),
                    hyperbola.normal(),
                    axis,
                    hyperbola.major_radius(),
                    hyperbola.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfHyperbola(arc) => {
                let hyperbola = Hyperbola::with_axis(
                    arc.hyperbola().center(),
                    arc.hyperbola().normal(),
                    axis,
                    arc.hyperbola().major_radius(),
                    arc.hyperbola().minor_radius(),
                )
                .ok()?;
                Self::ArcOfHyperbola(ArcOfHyperbola::new(hyperbola, arc.first(), arc.last()).ok()?)
            }
            Self::Parabola(parabola) => Self::Parabola(
                Parabola::with_axis(parabola.vertex(), parabola.normal(), axis, parabola.focal())
                    .ok()?,
            ),
            Self::ArcOfParabola(arc) => {
                let parabola = Parabola::with_axis(
                    arc.parabola().vertex(),
                    arc.parabola().normal(),
                    axis,
                    arc.parabola().focal(),
                )
                .ok()?;
                Self::ArcOfParabola(ArcOfParabola::new(parabola, arc.first(), arc.last()).ok()?)
            }
            _ => return None,
        })
    }

    /// A copy of a conic with its `YAxis` direction replaced.
    fn with_y_axis(&self, direction: (f64, f64)) -> Option<Self> {
        let direction = unit2(direction)?;
        let normal = self.normal()?;
        // `y = normal × x`, so `x = y × normal`.
        let y = xyz(direction.0, direction.1);
        let x = y.cross(&normal);
        self.with_x_axis((x.x, x.y))
    }

    /// The same geometry traversed in the opposite direction.
    fn reversed(&self) -> Option<Self> {
        Some(match self {
            Self::Line(line) => {
                Self::Line(Line::new(line.origin(), line.direction().scale(-1.0)).ok()?)
            }
            Self::Segment {
                origin,
                direction,
                first,
                last,
            } => {
                let start = (
                    origin.0 + last * direction.0,
                    origin.1 + last * direction.1,
                );
                Self::Segment {
                    origin: start,
                    direction: (-direction.0, -direction.1),
                    first: 0.0,
                    last: (last - first).abs(),
                }
            }
            Self::Circle(circle) => Self::Circle(
                Circle::with_axis(
                    circle.center(),
                    circle.normal().scale(-1.0),
                    circle.x_axis(),
                    circle.radius(),
                )
                .ok()?,
            ),
            Self::Arc(arc) => Self::Arc(Arc::new(arc.circle(), arc.last(), arc.first()).ok()?),
            Self::Ellipse(ellipse) => Self::Ellipse(
                Ellipse::with_axis(
                    ellipse.center(),
                    ellipse.normal().scale(-1.0),
                    ellipse.x_axis(),
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfEllipse {
                ellipse,
                first,
                last,
            } => Self::ArcOfEllipse {
                ellipse: *ellipse,
                first: *last,
                last: *first,
            },
            Self::Hyperbola(hyperbola) => Self::Hyperbola(
                Hyperbola::with_axis(
                    hyperbola.center(),
                    hyperbola.normal().scale(-1.0),
                    hyperbola.x_axis(),
                    hyperbola.major_radius(),
                    hyperbola.minor_radius(),
                )
                .ok()?,
            ),
            Self::ArcOfHyperbola(arc) => {
                let hyperbola = Hyperbola::with_axis(
                    arc.hyperbola().center(),
                    arc.hyperbola().normal().scale(-1.0),
                    arc.hyperbola().x_axis(),
                    arc.hyperbola().major_radius(),
                    arc.hyperbola().minor_radius(),
                )
                .ok()?;
                // Flipping the Y axis maps `u` to `-u`, so the forward range
                // over the same point set is `[-last, -first]`.
                Self::ArcOfHyperbola(
                    ArcOfHyperbola::new(hyperbola, -arc.last(), -arc.first()).ok()?,
                )
            }
            Self::Parabola(parabola) => Self::Parabola(
                Parabola::with_axis(
                    parabola.vertex(),
                    parabola.normal().scale(-1.0),
                    parabola.x_axis(),
                    parabola.focal(),
                )
                .ok()?,
            ),
            Self::ArcOfParabola(arc) => {
                let parabola = Parabola::with_axis(
                    arc.parabola().vertex(),
                    arc.parabola().normal().scale(-1.0),
                    arc.parabola().x_axis(),
                    arc.parabola().focal(),
                )
                .ok()?;
                Self::ArcOfParabola(
                    ArcOfParabola::new(parabola, -arc.last(), -arc.first()).ok()?,
                )
            }
            Self::Bezier(curve) => {
                let poles: Vec<Vector3> = curve.poles().iter().rev().copied().collect();
                let weights: Vec<f64> = curve.weights().iter().rev().copied().collect();
                Self::Bezier(
                    BezierCurve::with_range(poles, weights, curve.first(), curve.last()).ok()?,
                )
            }
            Self::BSpline(points) => Self::BSpline(points.iter().rev().copied().collect()),
            Self::Offset { basis, offset } => Self::Offset {
                basis: Box::new(basis.reversed()?),
                offset: *offset,
            },
        })
    }

    /// The same geometry restricted to `[first, last]`, the form a pcurve and a
    /// trimmed edge are built from.
    fn restricted(&self, first: f64, last: f64) -> PyResult<Self> {
        if !first.is_finite() || !last.is_finite() {
            return Err(OCCError::new_err("a 2D curve range must be finite"));
        }
        if (last - first).abs() <= ANGULAR_TOLERANCE {
            return Err(OCCError::new_err(
                "a 2D curve range needs a non-zero sweep",
            ));
        }
        Ok(match self {
            Self::Line(line) => Self::Segment {
                origin: xy(line.origin()),
                direction: xy(line.direction()),
                first,
                last,
            },
            Self::Segment {
                origin, direction, ..
            } => Self::Segment {
                origin: *origin,
                direction: *direction,
                first,
                last,
            },
            Self::Circle(circle) => {
                Self::Arc(Arc::new(*circle, first, last).map_err(curve_error)?)
            }
            Self::Arc(arc) => {
                Self::Arc(Arc::new(arc.circle(), first, last).map_err(curve_error)?)
            }
            Self::Ellipse(ellipse) => Self::ArcOfEllipse {
                ellipse: *ellipse,
                first,
                last,
            },
            Self::ArcOfEllipse { ellipse, .. } => Self::ArcOfEllipse {
                ellipse: *ellipse,
                first,
                last,
            },
            Self::Hyperbola(hyperbola) => Self::ArcOfHyperbola(
                ArcOfHyperbola::new(*hyperbola, first, last).map_err(curve_error)?,
            ),
            Self::ArcOfHyperbola(arc) => Self::ArcOfHyperbola(
                ArcOfHyperbola::new(arc.hyperbola(), first, last).map_err(curve_error)?,
            ),
            Self::Parabola(parabola) => Self::ArcOfParabola(
                ArcOfParabola::new(*parabola, first, last).map_err(curve_error)?,
            ),
            Self::ArcOfParabola(arc) => Self::ArcOfParabola(
                ArcOfParabola::new(arc.parabola(), first, last).map_err(curve_error)?,
            ),
            Self::Bezier(curve) => Self::Bezier(curve.trim(first, last).map_err(curve_error)?),
            Self::BSpline(_) => {
                return Err(geom2d_not_implemented(
                    "BSplineCurve2d.toShape (this slice binds no 2D B-spline kernel)",
                ))
            }
            Self::Offset { .. } => self.clone(),
        })
    }

    /// The numbered parameter of the point on the curve closest to `point`.
    fn closest_parameter(&self, point: (f64, f64)) -> Option<f64> {
        match self {
            Self::Line(line) => {
                let direction = xy(line.direction());
                let origin = xy(line.origin());
                Some((point.0 - origin.0) * direction.0 + (point.1 - origin.1) * direction.1)
            }
            Self::Segment {
                origin,
                direction,
                first,
                last,
            } => {
                let projected =
                    (point.0 - origin.0) * direction.0 + (point.1 - origin.1) * direction.1;
                Some(projected.clamp(*first, *last))
            }
            _ => {
                let (first, last) = (self.first()?, self.last()?);
                if !first.is_finite() || !last.is_finite() || last <= first || last - first > 1.0e6 {
                    return None;
                }
                let samples = 200usize;
                let step = (last - first) / samples as f64;
                let mut best = first;
                let mut best_distance = f64::INFINITY;
                for index in 0..=samples {
                    let u = first + index as f64 * step;
                    let candidate = self.point(u)?;
                    let distance = distance2(candidate, point);
                    if distance < best_distance {
                        best_distance = distance;
                        best = u;
                    }
                }
                // Golden-section refinement inside the bracketing interval.
                let mut low = (best - step).max(first);
                let mut high = (best + step).min(last);
                for _ in 0..60 {
                    let left = low + 0.382 * (high - low);
                    let right = low + 0.618 * (high - low);
                    let left_distance = distance2(self.point(left)?, point);
                    let right_distance = distance2(self.point(right)?, point);
                    if left_distance <= right_distance {
                        high = right;
                    } else {
                        low = left;
                    }
                }
                Some(0.5 * (low + high))
            }
        }
    }
}

/// The curvature at `u`: exact for a line and a circle, a central difference of
/// the analytic unit tangent for the other curves.
fn curvature_of(data: &Geom2dData, u: f64) -> PyResult<f64> {
    match data {
        Geom2dData::Line(_) | Geom2dData::Segment { .. } => Ok(0.0),
        Geom2dData::Circle(circle) => Ok(1.0 / circle.radius()),
        Geom2dData::Arc(arc) => Ok(1.0 / arc.circle().radius()),
        _ => {
            let step = 1e-6 * (1.0 + u.abs());
            let before = data
                .tangent(u - step)
                .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.curvature"))?;
            let after = data
                .tangent(u + step)
                .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.curvature"))?;
            let rate = distance2(before, after) / (2.0 * step);
            let speed = data
                .speed(u)
                .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.curvature"))?;
            Ok(rate / speed)
        }
    }
}

/// The geometry behind a Python `Part.Geom2d` object.
fn geometry_data(value: &Bound<'_, PyAny>) -> PyResult<Geom2dData> {
    let geometry = value
        .downcast::<PyGeometry2d>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Geom2d geometry"))?;
    Ok(geometry.borrow().snapshot())
}

/// Replace the geometry behind a Python `Part.Geom2d` object in place.
fn replace_geometry_data(value: &Bound<'_, PyAny>, data: Geom2dData) -> PyResult<()> {
    let geometry = value
        .downcast::<PyGeometry2d>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Geom2d geometry"))?;
    *geometry
        .borrow()
        .data
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = data;
    Ok(())
}

/// `Part.Geom2d.Geometry2d` — the abstract root of the 2D geometry classes.
///
/// The concrete geometry is held here, so every `Part.Geom2d` subclass shares
/// one copy of it (the classes below add no fields of their own).
#[pyclass(name = "Geometry2d", module = "Part.Geom2d", subclass)]
pub struct PyGeometry2d {
    data: Mutex<Geom2dData>,
}

impl PyGeometry2d {
    fn with_data(data: Geom2dData) -> Self {
        Self {
            data: Mutex::new(data),
        }
    }

    fn snapshot(&self) -> Geom2dData {
        self.data
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[pymethods]
impl PyGeometry2d {
    #[new]
    #[pyo3(signature = (*_args))]
    fn new(_args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        Err(PyRuntimeError::new_err(
            "You cannot create an instance of the abstract class 'Geometry'.",
        ))
    }

    /// A new geometry object of the same concrete class.
    fn copy(&self, py: Python<'_>) -> PyResult<PyObject> {
        geometry2d_to_py(py, self.snapshot())
    }

    /// Upstream's `Geometry2d.transform` takes a 2×3 matrix. This slice has no
    /// 2D similarity transform in its geometry model, so the operation is
    /// reported rather than approximated.
    fn transform(&self, _matrix: &Bound<'_, PyAny>) -> PyResult<()> {
        Err(geom2d_not_implemented("Geometry2d.transform"))
    }

    #[pyo3(signature = (point, axis = None))]
    fn mirror(&self, point: &Bound<'_, PyAny>, axis: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let _ = (point, axis);
        Err(geom2d_not_implemented("Geometry2d.mirror"))
    }

    fn rotate(&self, _point: &Bound<'_, PyAny>, _angle: f64) -> PyResult<()> {
        Err(geom2d_not_implemented("Geometry2d.rotate"))
    }

    fn scale(&self, _point: &Bound<'_, PyAny>, _factor: f64) -> PyResult<()> {
        Err(geom2d_not_implemented("Geometry2d.scale"))
    }

    fn translate(&self, _offset: &Bound<'_, PyAny>) -> PyResult<()> {
        Err(geom2d_not_implemented("Geometry2d.translate"))
    }

    fn __repr__(&self) -> String {
        format!("<Geometry2d object ({})>", self.snapshot().kind())
    }
}

/// `Part.Geom2d.Curve2d` — the abstract root of the 2D curves.
#[pyclass(name = "Curve2d", module = "Part.Geom2d", extends = PyGeometry2d, subclass)]
pub struct PyCurve2d;

#[pymethods]
impl PyCurve2d {
    #[new]
    #[pyo3(signature = (*_args))]
    fn new(_args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        Err(PyRuntimeError::new_err(
            "You cannot create an instance of the abstract class 'Curve2d'.",
        ))
    }

    #[getter]
    fn Continuity(slf: &Bound<'_, Self>) -> PyResult<String> {
        Ok(geometry_data(slf.as_any())?.continuity().to_string())
    }

    #[getter]
    fn Closed(slf: &Bound<'_, Self>) -> PyResult<bool> {
        Ok(geometry_data(slf.as_any())?.is_closed())
    }

    #[getter]
    fn Periodic(slf: &Bound<'_, Self>) -> PyResult<bool> {
        Ok(geometry_data(slf.as_any())?.is_periodic())
    }

    #[getter]
    fn FirstParameter(slf: &Bound<'_, Self>) -> PyResult<f64> {
        geometry_data(slf.as_any())?
            .first()
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.FirstParameter"))
    }

    #[getter]
    fn LastParameter(slf: &Bound<'_, Self>) -> PyResult<f64> {
        geometry_data(slf.as_any())?
            .last()
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.LastParameter"))
    }

    /// The point at parameter `u` (`Geom2d_Curve::Value`).
    fn value(slf: &Bound<'_, Self>, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let point = data
            .point(u)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.value"))?;
        py_vector2d(py, point)
    }

    /// The unit tangent at `u`.
    fn tangent(slf: &Bound<'_, Self>, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let tangent = data
            .tangent(u)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.tangent"))?;
        py_vector2d(py, tangent)
    }

    /// The unit normal at `u` (the tangent turned onto the principal side).
    fn normal(slf: &Bound<'_, Self>, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let tangent = data
            .tangent(u)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.normal"))?;
        py_vector2d(py, (-tangent.1, tangent.0))
    }

    /// The curvature at `u`. Exact for the lines and circles; the remaining
    /// curves use a central difference of the analytic unit tangent.
    fn curvature(slf: &Bound<'_, Self>, u: f64) -> PyResult<f64> {
        curvature_of(&geometry_data(slf.as_any())?, u)
    }

    /// The centre of curvature at `u`.
    fn centerOfCurvature(slf: &Bound<'_, Self>, py: Python<'_>, u: f64) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let point = data
            .point(u)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.centerOfCurvature"))?;
        let tangent = data
            .tangent(u)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.centerOfCurvature"))?;
        let curvature = curvature_of(&data, u)?;
        if curvature.abs() <= TOLERANCE {
            return Err(OCCError::new_err(
                "the centre of curvature is undefined where the curvature is zero",
            ));
        }
        let normal = (-tangent.1, tangent.0);
        py_vector2d(
            py,
            (
                point.0 + normal.0 / curvature,
                point.1 + normal.1 / curvature,
            ),
        )
    }

    /// The parameter of the point on the curve closest to `point`
    /// (`Geom2dAPI_ProjectPointOnCurve`). Unbounded conics are reported rather
    /// than sampled over an infinite range.
    fn parameter(slf: &Bound<'_, Self>, point: &Bound<'_, PyAny>) -> PyResult<f64> {
        let data = geometry_data(slf.as_any())?;
        data.closest_parameter(vector2d(point)?)
            .ok_or_else(|| geom2d_not_implemented("Curve2d.parameter on an unbounded curve"))
    }

    /// The arc length over `[uMin, uMax]`, by composite Simpson integration of
    /// the analytic speed.
    #[pyo3(signature = (uMin = None, uMax = None, tolerance = None))]
    fn length(
        slf: &Bound<'_, Self>,
        uMin: Option<f64>,
        uMax: Option<f64>,
        tolerance: Option<f64>,
    ) -> PyResult<f64> {
        let _ = tolerance;
        let data = geometry_data(slf.as_any())?;
        let (first, last) = match (uMin, uMax) {
            (Some(first), Some(last)) => (first, last),
            (None, None) => (
                data.first()
                    .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.length"))?,
                data.last()
                    .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.length"))?,
            ),
            _ => {
                return Err(PyTypeError::new_err(
                    "length takes either no range or both uMin and uMax",
                ))
            }
        };
        let span = last - first;
        if !span.is_finite() || span.abs() > 1.0e6 {
            return Err(OCCError::new_err(
                "the length of an unbounded 2D curve needs a finite parameter range",
            ));
        }
        let intervals = 512usize;
        let step = span / intervals as f64;
        let mut sum = data
            .speed(first)
            .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.length"))?
            + data
                .speed(last)
                .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.length"))?;
        for index in 1..intervals {
            let weight = if index % 2 == 0 { 2.0 } else { 4.0 };
            let speed = data
                .speed(first + index as f64 * step)
                .ok_or_else(|| geom2d_not_implemented("BSplineCurve2d.length"))?;
            sum += weight * speed;
        }
        Ok(sum * step / 3.0)
    }

    /// Reverse the parameterization in place (`Geom2d_Curve::Reverse`).
    fn reverse(slf: &Bound<'_, Self>) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let reversed = data
            .reversed()
            .ok_or_else(|| geom2d_not_implemented("Curve2d.reverse on this curve"))?;
        replace_geometry_data(slf.as_any(), reversed)
    }

    /// The curve as a `Part.Edge`.
    ///
    /// `toShape()` lifts the 2D curve to `z = 0` and builds a real XY edge.
    /// `toShape(first, last)` trims it. `toShape(surface)` (or a `Part.Face`)
    /// builds the same XY edge and attaches the 2D curve as its pcurve, but
    /// **does not project it onto the surface** — the seam binds no
    /// `GeomProjLib`, so the surface is validated and recorded only.
    #[pyo3(signature = (*args))]
    fn toShape(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<Py<PyEdge>> {
        let data = geometry_data(slf.as_any())?;
        let (_has_surface, first, last) = parse_geometry2d_to_shape_args(args)?;
        curve2d_edge(py, &data, first, last)
    }

    /// This slice records the points but builds no 2D B-spline.
    #[pyo3(signature = (*_args, **_kwargs))]
    fn discretize(
        &self,
        _args: &Bound<'_, PyTuple>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Vec<f64>> {
        Err(geom2d_not_implemented("Curve2d.discretize"))
    }

    #[pyo3(signature = (*_args))]
    fn toBSpline(&self, _args: &Bound<'_, PyTuple>) -> PyResult<PyObject> {
        Err(geom2d_not_implemented(
            "Curve2d.toBSpline (this slice binds no 2D B-spline kernel; use toShape())",
        ))
    }

    #[pyo3(signature = (*_args))]
    fn approximateBSpline(&self, _args: &Bound<'_, PyTuple>) -> PyResult<PyObject> {
        Err(geom2d_not_implemented("Curve2d.approximateBSpline"))
    }

    #[pyo3(signature = (*_args))]
    fn intersectCC(&self, _args: &Bound<'_, PyTuple>) -> PyResult<Vec<f64>> {
        Err(geom2d_not_implemented("Curve2d.intersectCC"))
    }

    #[pyo3(signature = (*_args))]
    fn parameterAtDistance(&self, _args: &Bound<'_, PyTuple>) -> PyResult<f64> {
        Err(geom2d_not_implemented("Curve2d.parameterAtDistance"))
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let data = geometry_data(slf.as_any())?;
        Ok(match data.location() {
            Some(location) => format!(
                "Part.Geom2d.{}({:?}, {})",
                data.kind(),
                location,
                data.first().unwrap_or(0.0)
            ),
            None => format!("Part.Geom2d.{}(...)", data.kind()),
        })
    }
}

/// `Part.Geom2d.Conic2d` — the abstract root of the 2D conics.
#[pyclass(name = "Conic2d", module = "Part.Geom2d", extends = PyCurve2d, subclass)]
pub struct PyConic2d;

#[pymethods]
impl PyConic2d {
    #[new]
    #[pyo3(signature = (*_args))]
    fn new(_args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        Err(PyRuntimeError::new_err(
            "You cannot create an instance of the abstract class 'Conic2d'.",
        ))
    }

    #[getter]
    fn Location(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let location = geometry_data(slf.as_any())?
            .location()
            .ok_or_else(|| geom2d_not_implemented("Conic2d.Location"))?;
        py_vector2d(py, location)
    }

    #[setter]
    fn set_Location(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let location = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_location(location)
            .ok_or_else(|| geom2d_not_implemented("Conic2d.Location"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn XAxis(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let axis = geometry_data(slf.as_any())?
            .x_axis()
            .ok_or_else(|| geom2d_not_implemented("Conic2d.XAxis"))?;
        py_vector2d(py, axis)
    }

    #[setter]
    fn set_XAxis(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_x_axis(axis)
            .ok_or_else(|| geom2d_not_implemented("Conic2d.XAxis"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn YAxis(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let axis = geometry_data(slf.as_any())?
            .y_axis()
            .ok_or_else(|| geom2d_not_implemented("Conic2d.YAxis"))?;
        py_vector2d(py, axis)
    }

    #[setter]
    fn set_YAxis(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_y_axis(axis)
            .ok_or_else(|| geom2d_not_implemented("Conic2d.YAxis"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn Eccentricity(slf: &Bound<'_, Self>) -> PyResult<f64> {
        geometry_data(slf.as_any())?
            .eccentricity()
            .ok_or_else(|| geom2d_not_implemented("Conic2d.Eccentricity"))
    }
}

/// `Part.Geom2d.ArcOfConic2d` — the abstract root of the 2D conic arcs.
#[pyclass(name = "ArcOfConic2d", module = "Part.Geom2d", extends = PyCurve2d, subclass)]
pub struct PyArcOfConic2d;

#[pymethods]
impl PyArcOfConic2d {
    #[new]
    #[pyo3(signature = (*_args))]
    fn new(_args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        Err(PyRuntimeError::new_err(
            "You cannot create an instance of the abstract class 'ArcOfConic2d'.",
        ))
    }

    #[getter]
    fn Location(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let location = geometry_data(slf.as_any())?
            .location()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.Location"))?;
        py_vector2d(py, location)
    }

    #[setter]
    fn set_Location(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let location = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_location(location)
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.Location"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn XAxis(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let axis = geometry_data(slf.as_any())?
            .x_axis()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.XAxis"))?;
        py_vector2d(py, axis)
    }

    #[setter]
    fn set_XAxis(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_x_axis(axis)
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.XAxis"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn YAxis(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let axis = geometry_data(slf.as_any())?
            .y_axis()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.YAxis"))?;
        py_vector2d(py, axis)
    }

    #[setter]
    fn set_YAxis(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_y_axis(axis)
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.YAxis"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn Eccentricity(slf: &Bound<'_, Self>) -> PyResult<f64> {
        geometry_data(slf.as_any())?
            .eccentricity()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.Eccentricity"))
    }

    #[getter]
    fn StartPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let first = data
            .first()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.StartPoint"))?;
        let point = data
            .point(first)
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.StartPoint"))?;
        py_vector2d(py, point)
    }

    #[getter]
    fn EndPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let last = data
            .last()
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.EndPoint"))?;
        let point = data
            .point(last)
            .ok_or_else(|| geom2d_not_implemented("ArcOfConic2d.EndPoint"))?;
        py_vector2d(py, point)
    }
}

// --- Concrete 2D curve initializers ----------------------------------------

/// The initializer chain for a curve whose base is `PyCurve2d`.
macro_rules! curve2d_initializer {
    ($class:ident) => {
        impl $class {
            fn initializer(data: Geom2dData) -> PyClassInitializer<Self> {
                PyClassInitializer::from(PyGeometry2d::with_data(data))
                    .add_subclass(PyCurve2d)
                    .add_subclass(Self)
            }
        }
    };
}

/// The initializer chain for a curve whose base is `PyConic2d`.
macro_rules! conic2d_initializer {
    ($class:ident) => {
        impl $class {
            fn initializer(data: Geom2dData) -> PyClassInitializer<Self> {
                PyClassInitializer::from(PyGeometry2d::with_data(data))
                    .add_subclass(PyCurve2d)
                    .add_subclass(PyConic2d)
                    .add_subclass(Self)
            }
        }
    };
}

/// The initializer chain for a curve whose base is `PyArcOfConic2d`.
macro_rules! arc_of_conic2d_initializer {
    ($class:ident) => {
        impl $class {
            fn initializer(data: Geom2dData) -> PyClassInitializer<Self> {
                PyClassInitializer::from(PyGeometry2d::with_data(data))
                    .add_subclass(PyCurve2d)
                    .add_subclass(PyArcOfConic2d)
                    .add_subclass(Self)
            }
        }
    };
}

// --- Builders --------------------------------------------------------------

fn line_data(origin: (f64, f64), direction: (f64, f64)) -> PyResult<Geom2dData> {
    let direction = unit2(direction)
        .ok_or_else(|| OCCError::new_err("the line direction must not be null"))?;
    Ok(Geom2dData::Line(
        Line::new(xyz(origin.0, origin.1), xyz(direction.0, direction.1))
            .map_err(curve_error)?,
    ))
}

fn segment_data(start: (f64, f64), end: (f64, f64)) -> PyResult<Geom2dData> {
    let delta = (end.0 - start.0, end.1 - start.1);
    let length = (delta.0 * delta.0 + delta.1 * delta.1).sqrt();
    if !length.is_finite() || length <= TOLERANCE {
        return Err(OCCError::new_err("Both points are equal"));
    }
    Ok(Geom2dData::Segment {
        origin: start,
        direction: (delta.0 / length, delta.1 / length),
        first: 0.0,
        last: length,
    })
}

fn circle_data(center: (f64, f64), radius: f64) -> PyResult<Geom2dData> {
    if !radius.is_finite() || radius <= TOLERANCE {
        return Err(OCCError::new_err("the circle radius must be positive"));
    }
    Ok(Geom2dData::Circle(
        Circle::with_axis(xyz(center.0, center.1), unit_normal(), unit_x(), radius)
            .map_err(curve_error)?,
    ))
}

fn ellipse_data(
    center: (f64, f64),
    x_axis: (f64, f64),
    major: f64,
    minor: f64,
) -> PyResult<Geom2dData> {
    if !major.is_finite() || !minor.is_finite() || major <= TOLERANCE || minor <= TOLERANCE {
        return Err(OCCError::new_err("the ellipse radii must be positive"));
    }
    if major < minor {
        return Err(OCCError::new_err(
            "the ellipse major radius must not be smaller than the minor radius",
        ));
    }
    Ok(Geom2dData::Ellipse(
        Ellipse::with_axis(
            xyz(center.0, center.1),
            unit_normal(),
            xyz(x_axis.0, x_axis.1),
            major,
            minor,
        )
        .map_err(curve_error)?,
    ))
}

fn hyperbola_data(
    center: (f64, f64),
    x_axis: (f64, f64),
    major: f64,
    minor: f64,
) -> PyResult<Geom2dData> {
    if !major.is_finite() || !minor.is_finite() || major <= TOLERANCE || minor <= TOLERANCE {
        return Err(OCCError::new_err("the hyperbola radii must be positive"));
    }
    Ok(Geom2dData::Hyperbola(
        Hyperbola::with_axis(
            xyz(center.0, center.1),
            unit_normal(),
            xyz(x_axis.0, x_axis.1),
            major,
            minor,
        )
        .map_err(curve_error)?,
    ))
}

fn parabola_data(vertex: (f64, f64), x_axis: (f64, f64), focal: f64) -> PyResult<Geom2dData> {
    if !focal.is_finite() || focal <= TOLERANCE {
        return Err(OCCError::new_err(
            "the parabola focal length must be positive",
        ));
    }
    Ok(Geom2dData::Parabola(
        Parabola::with_axis(
            xyz(vertex.0, vertex.1),
            unit_normal(),
            xyz(x_axis.0, x_axis.1),
            focal,
        )
        .map_err(curve_error)?,
    ))
}

fn arc_of_circle_data(circle: Circle, first: f64, last: f64) -> PyResult<Geom2dData> {
    if !first.is_finite() || !last.is_finite() || (last - first).abs() <= ANGULAR_TOLERANCE {
        return Err(OCCError::new_err(
            "an arc of a circle needs a non-zero, finite parameter range",
        ));
    }
    Ok(Geom2dData::Arc(
        Arc::new(circle, first, last).map_err(curve_error)?,
    ))
}

/// The directed sweep an arc constructor asks for. For a periodic curve
/// (`periodic`), `sense` selects the counter-clockwise (`True`) or clockwise
/// (`False`) arc from `first`.
fn arc_range(first: f64, last: f64, periodic: bool, sense: bool) -> PyResult<(f64, f64)> {
    if !first.is_finite() || !last.is_finite() {
        return Err(OCCError::new_err("an arc parameter range must be finite"));
    }
    if (last - first).abs() <= ANGULAR_TOLERANCE {
        return Err(OCCError::new_err(
            "an arc needs a non-zero parameter range",
        ));
    }
    if !periodic {
        return Ok((first, last));
    }
    let counter_clockwise = (last - first).rem_euclid(std::f64::consts::TAU);
    Ok(if sense {
        (first, first + counter_clockwise)
    } else {
        (first, first - (std::f64::consts::TAU - counter_clockwise))
    })
}

fn arc_of_ellipse_data(ellipse: Ellipse, first: f64, last: f64) -> PyResult<Geom2dData> {
    if !first.is_finite() || !last.is_finite() || (last - first).abs() <= ANGULAR_TOLERANCE {
        return Err(OCCError::new_err(
            "an arc of an ellipse needs a non-zero, finite parameter range",
        ));
    }
    Ok(Geom2dData::ArcOfEllipse {
        ellipse,
        first,
        last,
    })
}

fn arc_of_hyperbola_data(
    hyperbola: Hyperbola,
    first: f64,
    last: f64,
    sense: bool,
) -> PyResult<Geom2dData> {
    let (first, last) = if sense {
        if last <= first {
            return Err(OCCError::new_err(
                "a forward arc of a hyperbola needs last > first",
            ));
        }
        (first, last)
    } else {
        (last.min(first), last.max(first))
    };
    Ok(Geom2dData::ArcOfHyperbola(
        ArcOfHyperbola::new(hyperbola, first, last).map_err(curve_error)?,
    ))
}

fn arc_of_parabola_data(
    parabola: Parabola,
    first: f64,
    last: f64,
    sense: bool,
) -> PyResult<Geom2dData> {
    let (first, last) = if sense {
        if last <= first {
            return Err(OCCError::new_err(
                "a forward arc of a parabola needs last > first",
            ));
        }
        (first, last)
    } else {
        (last.min(first), last.max(first))
    };
    Ok(Geom2dData::ArcOfParabola(
        ArcOfParabola::new(parabola, first, last).map_err(curve_error)?,
    ))
}

/// The circle through three points and the arc from the first through the
/// second to the third (`GCE2d_MakeArcOfCircle`).
fn arc_through_points(
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) -> PyResult<Geom2dData> {
    let center = circle_center(p1, p2, p3)?;
    let radius = distance2(center, p1);
    let x_axis = unit2((p1.0 - center.0, p1.1 - center.1))
        .ok_or_else(|| OCCError::new_err("Two points are coincident"))?;
    let circle = Circle::with_axis(
        xyz(center.0, center.1),
        unit_normal(),
        xyz(x_axis.0, x_axis.1),
        radius,
    )
    .map_err(curve_error)?;
    let angle = |point: (f64, f64)| (point.1 - center.1).atan2(point.0 - center.0);
    let first = angle(p1);
    let through = (angle(p2) - first).rem_euclid(std::f64::consts::TAU);
    let end = (angle(p3) - first).rem_euclid(std::f64::consts::TAU);
    let last = if through < end {
        first + end
    } else {
        first + end - std::f64::consts::TAU
    };
    arc_of_circle_data(circle, first, last)
}

/// The `(S1, S2, Center)` ellipse form: `S1` ends the major axis and `S2` fixes
/// the minor radius (its distance to the major axis).
fn ellipse_through_points(
    s1: (f64, f64),
    s2: (f64, f64),
    center: (f64, f64),
) -> PyResult<Geom2dData> {
    let axis = (s1.0 - center.0, s1.1 - center.1);
    let major = (axis.0 * axis.0 + axis.1 * axis.1).sqrt();
    let direction = unit2(axis)
        .ok_or_else(|| OCCError::new_err("The first point must not be the centre"))?;
    let minor = cross2((s2.0 - center.0, s2.1 - center.1), direction).abs();
    ellipse_data(center, direction, major, minor)
}

fn hyperbola_through_points(
    s1: (f64, f64),
    s2: (f64, f64),
    center: (f64, f64),
) -> PyResult<Geom2dData> {
    let axis = (s1.0 - center.0, s1.1 - center.1);
    let major = (axis.0 * axis.0 + axis.1 * axis.1).sqrt();
    let direction = unit2(axis)
        .ok_or_else(|| OCCError::new_err("The first point must not be the centre"))?;
    let minor = cross2((s2.0 - center.0, s2.1 - center.1), direction).abs();
    hyperbola_data(center, direction, major, minor)
}

/// Parse `toShape(*args)` into `(surface?, first?, last?)`.
fn parse_geometry2d_to_shape_args(
    args: &Bound<'_, PyTuple>,
) -> PyResult<(bool, Option<f64>, Option<f64>)> {
    match args.len() {
        0 => Ok((false, None, None)),
        1 => {
            require_surface_or_face(&args.get_item(0)?)?;
            Ok((true, None, None))
        }
        2 => {
            let first: f64 = args
                .get_item(0)?
                .extract()
                .map_err(|_| to_shape_error())?;
            let last: f64 = args
                .get_item(1)?
                .extract()
                .map_err(|_| to_shape_error())?;
            Ok((false, Some(first), Some(last)))
        }
        3 => {
            require_surface_or_face(&args.get_item(0)?)?;
            let first: f64 = args
                .get_item(1)?
                .extract()
                .map_err(|_| to_shape_error())?;
            let last: f64 = args
                .get_item(2)?
                .extract()
                .map_err(|_| to_shape_error())?;
            Ok((true, Some(first), Some(last)))
        }
        _ => Err(to_shape_error()),
    }
}

fn to_shape_error() -> PyErr {
    PyTypeError::new_err(
        "toShape accepts (), (first, last), (surface), (surface, first, last), \
         (face) or (face, first, last)",
    )
}

/// An edge carrying a 2D curve as its pcurve.
fn curve2d_entry(shape: Shape, record: &Geom2dData) -> PyClassInitializer<PyEdge> {
    PyEdge::from_entry_with_curve2d(
        EdgeEntry {
            shape,
            curve: None,
        },
        record.clone(),
    )
}

/// The XY edge of a 2D curve, with the 2D curve attached as its pcurve.
fn curve2d_edge(
    py: Python<'_>,
    data: &Geom2dData,
    first: Option<f64>,
    last: Option<f64>,
) -> PyResult<Py<PyEdge>> {
    let record = match (first, last) {
        (Some(first), Some(last)) => data.restricted(first, last)?,
        _ => data.clone(),
    };
    match &record {
        Geom2dData::Line(_) => Err(OCCError::new_err(
            "an infinite 2D line has no edge; pass a parameter range to toShape",
        )),
        Geom2dData::Segment {
            origin,
            direction,
            first,
            last,
        } => {
            let start = xyz(
                origin.0 + first * direction.0,
                origin.1 + first * direction.1,
            );
            let end = xyz(
                origin.0 + last * direction.0,
                origin.1 + last * direction.1,
            );
            let segment = LineSegment::new(start, end).map_err(curve_error)?;
            let shape = shape_from_curve(&EdgeCurve::LineSegment(segment))?;
            Ok(Py::new(py, curve2d_entry(shape, &record))?)
        }
        Geom2dData::Circle(circle) => {
            let shape = shape_from_curve(&EdgeCurve::Circle(*circle))?;
            Ok(Py::new(py, curve2d_entry(shape, &record))?)
        }
        Geom2dData::Arc(arc) => {
            let shape = shape_from_curve(&EdgeCurve::Arc(*arc))?;
            Ok(Py::new(py, curve2d_entry(shape, &record))?)
        }
        Geom2dData::Ellipse(ellipse) => {
            let shape = shape_from_curve(&EdgeCurve::Ellipse(*ellipse))?;
            Ok(Py::new(py, curve2d_entry(shape, &record))?)
        }
        Geom2dData::ArcOfEllipse {
            ellipse,
            first,
            last,
        } => {
            let state = conic_arc_state(ellipse, *first, *last)?;
            let shape = spline_state_shape(&state)?;
            Ok(Py::new(
                py,
                PyEdge::from_bspline_with_curve2d(shape, state, record),
            )?)
        }
        Geom2dData::Hyperbola(_) => Err(OCCError::new_err(
            "a hyperbola is unbounded; pass a finite parameter range to toShape",
        )),
        Geom2dData::Parabola(_) => Err(OCCError::new_err(
            "a parabola is unbounded; pass a finite parameter range to toShape",
        )),
        Geom2dData::ArcOfHyperbola(arc) => {
            spline_curve2d_edge(py, &arc.nurbs().map_err(curve_error)?, record)
        }
        Geom2dData::ArcOfParabola(arc) => {
            spline_curve2d_edge(py, &arc.nurbs().map_err(curve_error)?, record)
        }
        Geom2dData::Bezier(curve) => spline_curve2d_edge(py, &curve.nurbs(), record),
        Geom2dData::Offset { .. } => Err(geom2d_not_implemented(
            "OffsetCurve2d.toShape (no exact offset edge is bound in this slice)",
        )),
        Geom2dData::BSpline(_) => Err(geom2d_not_implemented(
            "BSplineCurve2d.toShape (this slice binds no 2D B-spline kernel)",
        )),
    }
}

/// The exact NURBS state of an elliptical arc over `[first, last]`.
fn conic_arc_state(ellipse: &Ellipse, first: f64, last: f64) -> PyResult<SplineState> {
    let blob = occt()
        .conic_spline_state(
            3,
            ellipse.center(),
            ellipse.normal(),
            ellipse.x_axis(),
            ellipse.major_radius(),
            ellipse.minor_radius(),
            first,
            last,
            true,
        )
        .ok_or_else(|| OCCError::new_err("conversion of the elliptical arc to a NURBS failed"))?;
    SplineState::from_blob(&blob)
        .ok_or_else(|| OCCError::new_err("conversion of the elliptical arc to a NURBS failed"))
}

fn spline_state_shape(state: &SplineState) -> PyResult<Shape> {
    occt()
        .make_bspline_poles_mults_knots(
            &state.poles,
            &state.weights,
            &state.knots,
            &state.mults,
            state.degree,
            state.periodic,
            state.check_rational(),
        )
        .map_err(kernel_error)
}

fn spline_curve2d_edge(
    py: Python<'_>,
    curve: &NurbsCurve,
    record: Geom2dData,
) -> PyResult<Py<PyEdge>> {
    let state = nurbs_state(curve)?;
    let shape = spline_state_shape(&state)?;
    Py::new(py, PyEdge::from_bspline_with_curve2d(shape, state, record))
}

// --- Line2d ----------------------------------------------------------------

/// `Part.Geom2d.Line2d()` — an infinite line in 2D space (`Part::Geom2dLine`).
#[pyclass(name = "Line2d", module = "Part.Geom2d", extends = PyCurve2d)]
pub struct PyLine2d;
curve2d_initializer!(PyLine2d);

#[pymethods]
impl PyLine2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => line_data((0.0, 0.0), (1.0, 0.0))?,
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Line(_)) {
                    return Err(PyTypeError::new_err("Line2d copy expects a Line2d"));
                }
                data
            }
            2 => line_data(
                vector2d(&args.get_item(0)?)?,
                vector2d(&args.get_item(1)?)?,
            )?,
            _ => {
                return Err(PyTypeError::new_err(
                    "Line2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Line2d\n\
                     -- Point, Point",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn Location(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let location = data
            .location()
            .ok_or_else(|| OCCError::new_err("the object is not a Line2d"))?;
        py_vector2d(py, location)
    }

    #[setter]
    fn set_Location(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let location = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_location(location)
            .ok_or_else(|| OCCError::new_err("the object is not a Line2d"))?;
        replace_geometry_data(slf.as_any(), updated)
    }

    #[getter]
    fn Direction(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let axis = data
            .x_axis()
            .ok_or_else(|| OCCError::new_err("the object is not a Line2d"))?;
        py_vector2d(py, axis)
    }

    #[setter]
    fn set_Direction(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let direction = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let updated = data
            .with_x_axis(direction)
            .ok_or_else(|| OCCError::new_err("the direction must not be null"))?;
        replace_geometry_data(slf.as_any(), updated)
    }
}

// --- Line2dSegment ---------------------------------------------------------

/// `Part.Geom2d.Line2dSegment(p1, p2)` — a bounded line segment in 2D
/// parameter space (upstream's `Part::Geom2dLineSegment`).
///
/// Points are `(u, v)` pairs or objects with `x`/`y` (`ferrocad.Vector2d`).
#[pyclass(name = "Line2dSegment", module = "Part.Geom2d", extends = PyCurve2d)]
pub struct PyLine2dSegment;
curve2d_initializer!(PyLine2dSegment);

#[pymethods]
impl PyLine2dSegment {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => Geom2dData::Segment {
                origin: (0.0, 0.0),
                direction: (0.0, 1.0),
                first: 0.0,
                last: 1.0,
            },
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Segment { .. }) {
                    return Err(PyTypeError::new_err(
                        "Line2dSegment copy expects a Line2dSegment",
                    ));
                }
                data
            }
            2 => segment_data(
                vector2d(&args.get_item(0)?)?,
                vector2d(&args.get_item(1)?)?,
            )?,
            _ => {
                return Err(PyTypeError::new_err(
                    "Line2dSegment constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Line2dSegment\n\
                     -- Point1, Point2",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn StartPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let first = data
            .first()
            .ok_or_else(|| OCCError::new_err("the object is not a Line2dSegment"))?;
        let point = data
            .point(first)
            .ok_or_else(|| OCCError::new_err("the object is not a Line2dSegment"))?;
        py_vector2d(py, point)
    }

    #[setter]
    fn set_StartPoint(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let start = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Segment {
            origin,
            direction,
            last,
            ..
        } = data
        else {
            return Err(OCCError::new_err("the object is not a Line2dSegment"));
        };
        let end = (
            origin.0 + last * direction.0,
            origin.1 + last * direction.1,
        );
        replace_geometry_data(slf.as_any(), segment_data(start, end)?)
    }

    #[getter]
    fn EndPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let data = geometry_data(slf.as_any())?;
        let last = data
            .last()
            .ok_or_else(|| OCCError::new_err("the object is not a Line2dSegment"))?;
        let point = data
            .point(last)
            .ok_or_else(|| OCCError::new_err("the object is not a Line2dSegment"))?;
        py_vector2d(py, point)
    }

    #[setter]
    fn set_EndPoint(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let end = vector2d(value)?;
        let data = geometry_data(slf.as_any())?;
        let start = data
            .point(data.first().unwrap_or(0.0))
            .ok_or_else(|| OCCError::new_err("the object is not a Line2dSegment"))?;
        replace_geometry_data(slf.as_any(), segment_data(start, end)?)
    }

    /// `setParameterRange(first, last)` — retrim the underlying line, as
    /// `Geom2d_TrimmedCurve::SetTrim`.
    fn setParameterRange(slf: &Bound<'_, Self>, first: f64, last: f64) -> PyResult<()> {
        if !first.is_finite() || !last.is_finite() {
            return Err(OCCError::new_err(
                "a Line2dSegment parameter range must be finite",
            ));
        }
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Segment {
            origin, direction, ..
        } = data
        else {
            return Err(OCCError::new_err("the object is not a Line2dSegment"));
        };
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::Segment {
                origin,
                direction,
                first,
                last,
            },
        )
    }
}

// --- Circle2d --------------------------------------------------------------

/// `Part.Geom2d.Circle2d()` — a circle in 2D space (`Part::Geom2dCircle`).
///
/// The constructors follow upstream: empty (centre `(0, 0)`, radius `1`), a
/// copy, `(Center, Radius)`, `(Circle, Distance)` for a parallel circle, and
/// `(Point1, Point2, Point3)`.
#[pyclass(name = "Circle2d", module = "Part.Geom2d", extends = PyConic2d)]
pub struct PyCircle2d;
conic2d_initializer!(PyCircle2d);

#[pymethods]
impl PyCircle2d {
    #[new]
    #[pyo3(signature = (*args, **kwargs))]
    fn new(
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PyClassInitializer<Self>> {
        if let Some(kwargs) = kwargs {
            if !kwargs.is_empty() {
                if !args.is_empty() {
                    return Err(PyTypeError::new_err(
                        "Circle2d takes either positional or keyword arguments, not both",
                    ));
                }
                let center = dict_value(kwargs, &["Center", "center"])?;
                let radius = dict_value(kwargs, &["Radius", "radius"])?;
                let (Some(center), Some(radius)) = (center, radius) else {
                    return Err(PyTypeError::new_err(
                        "Circle2d keyword arguments are Center and Radius",
                    ));
                };
                return Ok(Self::initializer(circle_data(
                    vector2d(&center)?,
                    radius.extract()?,
                )?));
            }
        }
        let data = match args.len() {
            0 => circle_data((0.0, 0.0), 1.0)?,
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Circle(_)) {
                    return Err(PyTypeError::new_err("Circle2d copy expects a Circle2d"));
                }
                data
            }
            2 => {
                let first = args.get_item(0)?;
                let second = args.get_item(1)?;
                if let Ok(Geom2dData::Circle(circle)) = geometry_data(&first) {
                    let distance: f64 = second.extract().map_err(|_| {
                        PyTypeError::new_err("Circle2d(Circle, Distance) expects a float")
                    })?;
                    circle_data(xy(circle.center()), circle.radius() + distance)?
                } else {
                    circle_data(
                        vector2d(&first)?,
                        second.extract().map_err(|_| {
                            PyTypeError::new_err("Circle2d(Center, Radius) expects a float")
                        })?,
                    )?
                }
            }
            3 => {
                let p1 = vector2d(&args.get_item(0)?)?;
                let p2 = vector2d(&args.get_item(1)?)?;
                let p3 = vector2d(&args.get_item(2)?)?;
                let center = circle_center(p1, p2, p3)?;
                let radius = distance2(center, p1);
                let x_axis = unit2((p1.0 - center.0, p1.1 - center.1))
                    .ok_or_else(|| OCCError::new_err("Two points are coincident"))?;
                Geom2dData::Circle(
                    Circle::with_axis(
                        xyz(center.0, center.1),
                        unit_normal(),
                        xyz(x_axis.0, x_axis.1),
                        radius,
                    )
                    .map_err(curve_error)?,
                )
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Circle2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Circle2d\n\
                     -- Circle2d, Distance\n\
                     -- Center, Radius\n\
                     -- Point1, Point2, Point3",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn Radius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Circle(circle) => Ok(circle.radius()),
            _ => Err(OCCError::new_err("the object is not a Circle2d")),
        }
    }

    #[setter]
    fn set_Radius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Circle(circle) = data else {
            return Err(OCCError::new_err("the object is not a Circle2d"));
        };
        replace_geometry_data(slf.as_any(), circle_data(xy(circle.center()), value)?)
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
}

fn dict_value<'a>(
    kwargs: &'a Bound<'a, PyDict>,
    names: &[&str],
) -> PyResult<Option<Bound<'a, PyAny>>> {
    for name in names {
        if let Some(value) = kwargs.get_item(*name)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

// --- ArcOfCircle2d ---------------------------------------------------------

/// `Part.Geom2d.ArcOfCircle2d(circle, first, last, sense=True)` — a portion of
/// a circle (`Part::Geom2dArcOfCircle`).
///
/// The three-point form `(Point1, Point2, Point3)` builds the arc from the
/// first through the second to the third point.
#[pyclass(name = "ArcOfCircle2d", module = "Part.Geom2d", extends = PyArcOfConic2d)]
pub struct PyArcOfCircle2d;
arc_of_conic2d_initializer!(PyArcOfCircle2d);

#[pymethods]
impl PyArcOfCircle2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            3 | 4 => {
                let first_arg = args.get_item(0)?;
                if let Ok(Geom2dData::Circle(circle)) = geometry_data(&first_arg) {
                    let first: f64 = args.get_item(1)?.extract()?;
                    let last: f64 = args.get_item(2)?.extract()?;
                    let sense = if args.len() == 4 {
                        args.get_item(3)?.extract::<bool>()?
                    } else {
                        true
                    };
                    let (first, last) = arc_range(first, last, true, sense)?;
                    arc_of_circle_data(circle, first, last)?
                } else {
                    if args.len() != 3 {
                        return Err(PyTypeError::new_err(
                            "ArcOfCircle2d constructor expects a circle curve and a \
                             parameter range or three points",
                        ));
                    }
                    arc_through_points(
                        vector2d(&args.get_item(0)?)?,
                        vector2d(&args.get_item(1)?)?,
                        vector2d(&args.get_item(2)?)?,
                    )?
                }
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "ArcOfCircle2d constructor expects a circle curve and a \
                     parameter range or three points",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn Radius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Arc(arc) => Ok(arc.circle().radius()),
            _ => Err(OCCError::new_err("the object is not an ArcOfCircle2d")),
        }
    }

    #[setter]
    fn set_Radius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Arc(arc) = data else {
            return Err(OCCError::new_err("the object is not an ArcOfCircle2d"));
        };
        let circle = Circle::with_axis(
            arc.circle().center(),
            arc.circle().normal(),
            arc.circle().x_axis(),
            value,
        )
        .map_err(curve_error)?;
        replace_geometry_data(
            slf.as_any(),
            arc_of_circle_data(circle, arc.first(), arc.last())?,
        )
    }

    /// The internal circle (`GeomArcOfCircle::getCircle`).
    #[getter]
    fn Circle(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<PyCircle2d>> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Arc(arc) => {
                Py::new(py, PyCircle2d::initializer(Geom2dData::Circle(arc.circle())))
            }
            _ => Err(OCCError::new_err("the object is not an ArcOfCircle2d")),
        }
    }
}

// --- Ellipse2d -------------------------------------------------------------

/// `Part.Geom2d.Ellipse2d()` — an ellipse in 2D space (`Part::Geom2dEllipse`).
#[pyclass(name = "Ellipse2d", module = "Part.Geom2d", extends = PyConic2d)]
pub struct PyEllipse2d;
conic2d_initializer!(PyEllipse2d);

#[pymethods]
impl PyEllipse2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => ellipse_data((0.0, 0.0), (1.0, 0.0), 2.0, 1.0)?,
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Ellipse(_)) {
                    return Err(PyTypeError::new_err("Ellipse2d copy expects an Ellipse2d"));
                }
                data
            }
            3 => {
                let major: Result<f64, _> = args.get_item(1)?.extract();
                let minor: Result<f64, _> = args.get_item(2)?.extract();
                match (major, minor) {
                    (Ok(major), Ok(minor)) => {
                        let center = vector2d(&args.get_item(0)?)?;
                        // Upstream's `(Center, MajorRadius, MinorRadius)` puts
                        // the major axis along +Y.
                        ellipse_data(center, (0.0, 1.0), major, minor)?
                    }
                    _ => ellipse_through_points(
                        vector2d(&args.get_item(0)?)?,
                        vector2d(&args.get_item(1)?)?,
                        vector2d(&args.get_item(2)?)?,
                    )?,
                }
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Ellipse2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Ellipse2d\n\
                     -- Center, MajorRadius, MinorRadius\n\
                     -- Point, Point, Point",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn MajorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Ellipse(ellipse) => Ok(ellipse.major_radius()),
            _ => Err(OCCError::new_err("the object is not an Ellipse2d")),
        }
    }

    #[setter]
    fn set_MajorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Ellipse(ellipse) = data else {
            return Err(OCCError::new_err("the object is not an Ellipse2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            ellipse_data(
                xy(ellipse.center()),
                xy(ellipse.x_axis()),
                value,
                ellipse.minor_radius(),
            )?,
        )
    }

    #[getter]
    fn MinorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Ellipse(ellipse) => Ok(ellipse.minor_radius()),
            _ => Err(OCCError::new_err("the object is not an Ellipse2d")),
        }
    }

    #[setter]
    fn set_MinorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Ellipse(ellipse) = data else {
            return Err(OCCError::new_err("the object is not an Ellipse2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            ellipse_data(
                xy(ellipse.center()),
                xy(ellipse.x_axis()),
                ellipse.major_radius(),
                value,
            )?,
        )
    }

    #[getter]
    fn Focal(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Ellipse(ellipse) => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                Ok((a * a - b * b).max(0.0).sqrt())
            }
            _ => Err(OCCError::new_err("the object is not an Ellipse2d")),
        }
    }

    #[getter]
    fn Focus1(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Ellipse(ellipse) => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                let focal = (a * a - b * b).max(0.0).sqrt();
                let x = xy(ellipse.x_axis());
                let center = xy(ellipse.center());
                py_vector2d(py, (center.0 + focal * x.0, center.1 + focal * x.1))
            }
            _ => Err(OCCError::new_err("the object is not an Ellipse2d")),
        }
    }

    #[getter]
    fn Focus2(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Ellipse(ellipse) => {
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                let focal = (a * a - b * b).max(0.0).sqrt();
                let x = xy(ellipse.x_axis());
                let center = xy(ellipse.center());
                py_vector2d(py, (center.0 - focal * x.0, center.1 - focal * x.1))
            }
            _ => Err(OCCError::new_err("the object is not an Ellipse2d")),
        }
    }
}

// --- ArcOfEllipse2d --------------------------------------------------------

/// `Part.Geom2d.ArcOfEllipse2d(ellipse, first, last, sense=True)` — a portion
/// of an ellipse (`Part::Geom2dArcOfEllipse`).
#[pyclass(name = "ArcOfEllipse2d", module = "Part.Geom2d", extends = PyArcOfConic2d)]
pub struct PyArcOfEllipse2d;
arc_of_conic2d_initializer!(PyArcOfEllipse2d);

#[pymethods]
impl PyArcOfEllipse2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        if args.len() < 3 || args.len() > 4 {
            return Err(PyTypeError::new_err(
                "ArcOfEllipse2d constructor expects an ellipse curve and a \
                 parameter range",
            ));
        }
        let Geom2dData::Ellipse(ellipse) = geometry_data(&args.get_item(0)?)? else {
            return Err(PyTypeError::new_err(
                "ArcOfEllipse2d expects an Ellipse2d",
            ));
        };
        let first: f64 = args.get_item(1)?.extract()?;
        let last: f64 = args.get_item(2)?.extract()?;
        let sense = if args.len() == 4 {
            args.get_item(3)?.extract::<bool>()?
        } else {
            true
        };
        let (first, last) = arc_range(first, last, true, sense)?;
        Ok(Self::initializer(arc_of_ellipse_data(
            ellipse, first, last,
        )?))
    }

    #[getter]
    fn MajorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfEllipse { ellipse, .. } => Ok(ellipse.major_radius()),
            _ => Err(OCCError::new_err("the object is not an ArcOfEllipse2d")),
        }
    }

    #[setter]
    fn set_MajorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::ArcOfEllipse {
            ellipse,
            first,
            last,
        } = data
        else {
            return Err(OCCError::new_err("the object is not an ArcOfEllipse2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::ArcOfEllipse {
                ellipse: Ellipse::with_axis(
                    ellipse.center(),
                    ellipse.normal(),
                    ellipse.x_axis(),
                    value,
                    ellipse.minor_radius(),
                )
                .map_err(curve_error)?,
                first,
                last,
            },
        )
    }

    #[getter]
    fn MinorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfEllipse { ellipse, .. } => Ok(ellipse.minor_radius()),
            _ => Err(OCCError::new_err("the object is not an ArcOfEllipse2d")),
        }
    }

    #[setter]
    fn set_MinorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::ArcOfEllipse {
            ellipse,
            first,
            last,
        } = data
        else {
            return Err(OCCError::new_err("the object is not an ArcOfEllipse2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::ArcOfEllipse {
                ellipse: Ellipse::with_axis(
                    ellipse.center(),
                    ellipse.normal(),
                    ellipse.x_axis(),
                    ellipse.major_radius(),
                    value,
                )
                .map_err(curve_error)?,
                first,
                last,
            },
        )
    }

    /// The internal ellipse (`GeomArcOfEllipse::getEllipse`).
    #[getter]
    fn Ellipse(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<PyEllipse2d>> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfEllipse { ellipse, .. } => {
                Py::new(py, PyEllipse2d::initializer(Geom2dData::Ellipse(ellipse)))
            }
            _ => Err(OCCError::new_err("the object is not an ArcOfEllipse2d")),
        }
    }
}

// --- Hyperbola2d -----------------------------------------------------------

/// `Part.Geom2d.Hyperbola2d()` — a hyperbola in 2D space
/// (`Part::Geom2dHyperbola`).
#[pyclass(name = "Hyperbola2d", module = "Part.Geom2d", extends = PyConic2d)]
pub struct PyHyperbola2d;
conic2d_initializer!(PyHyperbola2d);

#[pymethods]
impl PyHyperbola2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => hyperbola_data((0.0, 0.0), (1.0, 0.0), 2.0, 1.0)?,
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Hyperbola(_)) {
                    return Err(PyTypeError::new_err(
                        "Hyperbola2d copy expects a Hyperbola2d",
                    ));
                }
                data
            }
            3 => {
                let major: Result<f64, _> = args.get_item(1)?.extract();
                let minor: Result<f64, _> = args.get_item(2)?.extract();
                match (major, minor) {
                    (Ok(major), Ok(minor)) => {
                        let center = vector2d(&args.get_item(0)?)?;
                        // Upstream's `(Center, MajorRadius, MinorRadius)` puts
                        // the major axis along +Y.
                        hyperbola_data(center, (0.0, 1.0), major, minor)?
                    }
                    _ => hyperbola_through_points(
                        vector2d(&args.get_item(0)?)?,
                        vector2d(&args.get_item(1)?)?,
                        vector2d(&args.get_item(2)?)?,
                    )?,
                }
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Hyperbola2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Hyperbola2d\n\
                     -- Center, MajorRadius, MinorRadius\n\
                     -- Point, Point, Point",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn MajorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Hyperbola(hyperbola) => Ok(hyperbola.major_radius()),
            _ => Err(OCCError::new_err("the object is not a Hyperbola2d")),
        }
    }

    #[setter]
    fn set_MajorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Hyperbola(hyperbola) = data else {
            return Err(OCCError::new_err("the object is not a Hyperbola2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            hyperbola_data(
                xy(hyperbola.center()),
                xy(hyperbola.x_axis()),
                value,
                hyperbola.minor_radius(),
            )?,
        )
    }

    #[getter]
    fn MinorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Hyperbola(hyperbola) => Ok(hyperbola.minor_radius()),
            _ => Err(OCCError::new_err("the object is not a Hyperbola2d")),
        }
    }

    #[setter]
    fn set_MinorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Hyperbola(hyperbola) = data else {
            return Err(OCCError::new_err("the object is not a Hyperbola2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            hyperbola_data(
                xy(hyperbola.center()),
                xy(hyperbola.x_axis()),
                hyperbola.major_radius(),
                value,
            )?,
        )
    }

    #[getter]
    fn Focal(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Hyperbola(hyperbola) => Ok(hyperbola.focal()),
            _ => Err(OCCError::new_err("the object is not a Hyperbola2d")),
        }
    }

    #[getter]
    fn Focus1(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Hyperbola(hyperbola) => py_vector2d(py, xy(hyperbola.focus1())),
            _ => Err(OCCError::new_err("the object is not a Hyperbola2d")),
        }
    }

    #[getter]
    fn Focus2(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Hyperbola(hyperbola) => py_vector2d(py, xy(hyperbola.focus2())),
            _ => Err(OCCError::new_err("the object is not a Hyperbola2d")),
        }
    }
}

// --- ArcOfHyperbola2d ------------------------------------------------------

/// `Part.Geom2d.ArcOfHyperbola2d(hyperbola, first, last, sense=True)` — a
/// portion of a hyperbola (`Part::Geom2dArcOfHyperbola`).
#[pyclass(name = "ArcOfHyperbola2d", module = "Part.Geom2d", extends = PyArcOfConic2d)]
pub struct PyArcOfHyperbola2d;
arc_of_conic2d_initializer!(PyArcOfHyperbola2d);

#[pymethods]
impl PyArcOfHyperbola2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        if args.len() < 3 || args.len() > 4 {
            return Err(PyTypeError::new_err(
                "ArcOfHyperbola2d constructor expects a hyperbola curve and a \
                 parameter range",
            ));
        }
        let Geom2dData::Hyperbola(hyperbola) = geometry_data(&args.get_item(0)?)? else {
            return Err(PyTypeError::new_err(
                "ArcOfHyperbola2d expects a Hyperbola2d",
            ));
        };
        let first: f64 = args.get_item(1)?.extract()?;
        let last: f64 = args.get_item(2)?.extract()?;
        let sense = if args.len() == 4 {
            args.get_item(3)?.extract::<bool>()?
        } else {
            true
        };
        Ok(Self::initializer(arc_of_hyperbola_data(
            hyperbola, first, last, sense,
        )?))
    }

    #[getter]
    fn MajorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfHyperbola(arc) => Ok(arc.hyperbola().major_radius()),
            _ => Err(OCCError::new_err("the object is not an ArcOfHyperbola2d")),
        }
    }

    #[setter]
    fn set_MajorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let Geom2dData::ArcOfHyperbola(arc) = geometry_data(slf.as_any())? else {
            return Err(OCCError::new_err("the object is not an ArcOfHyperbola2d"));
        };
        let hyperbola = Hyperbola::with_axis(
            arc.hyperbola().center(),
            arc.hyperbola().normal(),
            arc.hyperbola().x_axis(),
            value,
            arc.hyperbola().minor_radius(),
        )
        .map_err(curve_error)?;
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::ArcOfHyperbola(
                ArcOfHyperbola::new(hyperbola, arc.first(), arc.last()).map_err(curve_error)?,
            ),
        )
    }

    #[getter]
    fn MinorRadius(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfHyperbola(arc) => Ok(arc.hyperbola().minor_radius()),
            _ => Err(OCCError::new_err("the object is not an ArcOfHyperbola2d")),
        }
    }

    #[setter]
    fn set_MinorRadius(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let Geom2dData::ArcOfHyperbola(arc) = geometry_data(slf.as_any())? else {
            return Err(OCCError::new_err("the object is not an ArcOfHyperbola2d"));
        };
        let hyperbola = Hyperbola::with_axis(
            arc.hyperbola().center(),
            arc.hyperbola().normal(),
            arc.hyperbola().x_axis(),
            arc.hyperbola().major_radius(),
            value,
        )
        .map_err(curve_error)?;
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::ArcOfHyperbola(
                ArcOfHyperbola::new(hyperbola, arc.first(), arc.last()).map_err(curve_error)?,
            ),
        )
    }

    /// The internal hyperbola (`GeomArcOfHyperbola::getHyperbola`).
    #[getter]
    fn Hyperbola(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<PyHyperbola2d>> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfHyperbola(arc) => Py::new(
                py,
                PyHyperbola2d::initializer(Geom2dData::Hyperbola(arc.hyperbola())),
            ),
            _ => Err(OCCError::new_err("the object is not an ArcOfHyperbola2d")),
        }
    }
}

// --- Parabola2d ------------------------------------------------------------

/// `Part.Geom2d.Parabola2d()` — a parabola in 2D space
/// (`Part::Geom2dParabola`). The focal length is writable.
#[pyclass(name = "Parabola2d", module = "Part.Geom2d", extends = PyConic2d)]
pub struct PyParabola2d;
conic2d_initializer!(PyParabola2d);

#[pymethods]
impl PyParabola2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => parabola_data((0.0, 0.0), (1.0, 0.0), 1.0)?,
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Parabola(_)) {
                    return Err(PyTypeError::new_err(
                        "Parabola2d copy expects a Parabola2d",
                    ));
                }
                data
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Parabola2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- Parabola2d",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn Focal(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Parabola(parabola) => Ok(parabola.focal()),
            _ => Err(OCCError::new_err("the object is not a Parabola2d")),
        }
    }

    #[setter]
    fn set_Focal(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Parabola(parabola) = data else {
            return Err(OCCError::new_err("the object is not a Parabola2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            parabola_data(xy(parabola.vertex()), xy(parabola.x_axis()), value)?,
        )
    }

    #[getter]
    fn Focus(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Parabola(parabola) => py_vector2d(py, xy(parabola.focus())),
            _ => Err(OCCError::new_err("the object is not a Parabola2d")),
        }
    }

    /// The parameter (`2 × focal`).
    #[getter]
    fn Parameter(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Parabola(parabola) => Ok(parabola.parameter()),
            _ => Err(OCCError::new_err("the object is not a Parabola2d")),
        }
    }
}

// --- ArcOfParabola2d -------------------------------------------------------

/// `Part.Geom2d.ArcOfParabola2d(parabola, first, last, sense=True)` — a portion
/// of a parabola (`Part::Geom2dArcOfParabola`).
#[pyclass(name = "ArcOfParabola2d", module = "Part.Geom2d", extends = PyArcOfConic2d)]
pub struct PyArcOfParabola2d;
arc_of_conic2d_initializer!(PyArcOfParabola2d);

#[pymethods]
impl PyArcOfParabola2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        if args.len() < 3 || args.len() > 4 {
            return Err(PyTypeError::new_err(
                "ArcOfParabola2d constructor expects a parabola curve and a \
                 parameter range",
            ));
        }
        let Geom2dData::Parabola(parabola) = geometry_data(&args.get_item(0)?)? else {
            return Err(PyTypeError::new_err(
                "ArcOfParabola2d expects a Parabola2d",
            ));
        };
        let first: f64 = args.get_item(1)?.extract()?;
        let last: f64 = args.get_item(2)?.extract()?;
        let sense = if args.len() == 4 {
            args.get_item(3)?.extract::<bool>()?
        } else {
            true
        };
        Ok(Self::initializer(arc_of_parabola_data(
            parabola, first, last, sense,
        )?))
    }

    #[getter]
    fn Focal(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfParabola(arc) => Ok(arc.parabola().focal()),
            _ => Err(OCCError::new_err("the object is not an ArcOfParabola2d")),
        }
    }

    #[setter]
    fn set_Focal(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        let Geom2dData::ArcOfParabola(arc) = geometry_data(slf.as_any())? else {
            return Err(OCCError::new_err("the object is not an ArcOfParabola2d"));
        };
        let parabola = Parabola::with_axis(
            arc.parabola().vertex(),
            arc.parabola().normal(),
            arc.parabola().x_axis(),
            value,
        )
        .map_err(curve_error)?;
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::ArcOfParabola(
                ArcOfParabola::new(parabola, arc.first(), arc.last()).map_err(curve_error)?,
            ),
        )
    }

    /// The internal parabola (`GeomArcOfParabola::getParabola`).
    #[getter]
    fn Parabola(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<PyParabola2d>> {
        match geometry_data(slf.as_any())? {
            Geom2dData::ArcOfParabola(arc) => Py::new(
                py,
                PyParabola2d::initializer(Geom2dData::Parabola(arc.parabola())),
            ),
            _ => Err(OCCError::new_err("the object is not an ArcOfParabola2d")),
        }
    }
}

// --- OffsetCurve2d ---------------------------------------------------------

/// `Part.Geom2d.OffsetCurve2d(basis, offset)` — the offset of a 2D curve
/// (`Part::Geom2dOffsetCurve`).
///
/// `value(u)` is the offset point `basis(u) + offset × normal(u)`.
/// `toShape()` is reported as not implemented: this seam binds no exact 2D
/// offset edge.
#[pyclass(name = "OffsetCurve2d", module = "Part.Geom2d", extends = PyCurve2d)]
pub struct PyOffsetCurve2d;
curve2d_initializer!(PyOffsetCurve2d);

#[pymethods]
impl PyOffsetCurve2d {
    #[new]
    fn new(basis: &Bound<'_, PyAny>, offset: f64) -> PyResult<PyClassInitializer<Self>> {
        if !offset.is_finite() {
            return Err(OCCError::new_err("the offset value must be finite"));
        }
        let basis = geometry_data(basis)?;
        Ok(Self::initializer(Geom2dData::Offset {
            basis: Box::new(basis),
            offset,
        }))
    }

    #[getter]
    fn OffsetValue(slf: &Bound<'_, Self>) -> PyResult<f64> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Offset { offset, .. } => Ok(offset),
            _ => Err(OCCError::new_err("the object is not an OffsetCurve2d")),
        }
    }

    #[setter]
    fn set_OffsetValue(slf: &Bound<'_, Self>, value: f64) -> PyResult<()> {
        if !value.is_finite() {
            return Err(OCCError::new_err("the offset value must be finite"));
        }
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Offset { basis, .. } = data else {
            return Err(OCCError::new_err("the object is not an OffsetCurve2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::Offset {
                basis,
                offset: value,
            },
        )
    }

    #[getter]
    fn BasisCurve(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        match geometry_data(slf.as_any())? {
            Geom2dData::Offset { basis, .. } => geometry2d_to_py(py, *basis),
            _ => Err(OCCError::new_err("the object is not an OffsetCurve2d")),
        }
    }

    #[setter]
    fn set_BasisCurve(slf: &Bound<'_, Self>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let basis = geometry_data(value)?;
        let data = geometry_data(slf.as_any())?;
        let Geom2dData::Offset { offset, .. } = data else {
            return Err(OCCError::new_err("the object is not an OffsetCurve2d"));
        };
        replace_geometry_data(
            slf.as_any(),
            Geom2dData::Offset {
                basis: Box::new(basis),
                offset,
            },
        )
    }
}

// --- BezierCurve2d ---------------------------------------------------------

/// `Part.Geom2d.BezierCurve2d()` — a rational or non-rational Bezier curve in
/// 2D space (`Part::Geom2dBezierCurve`).
///
/// The default curve is the two-pole segment `(0, 0) → (0, 1)` over `[0, 1]`,
/// as upstream. `increase`, `insertPoleAfter`, `insertPoleBefore`, `removePole`
/// and `getResolution` are reported as not implemented.
#[pyclass(name = "BezierCurve2d", module = "Part.Geom2d", extends = PyCurve2d)]
pub struct PyBezierCurve2d;
curve2d_initializer!(PyBezierCurve2d);

fn bezier2d(slf: &Bound<'_, PyAny>) -> PyResult<BezierCurve> {
    match geometry_data(slf)? {
        Geom2dData::Bezier(curve) => Ok(curve),
        _ => Err(OCCError::new_err("the object is not a BezierCurve2d")),
    }
}

#[pymethods]
impl PyBezierCurve2d {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<PyClassInitializer<Self>> {
        let data = match args.len() {
            0 => Geom2dData::Bezier(
                BezierCurve::with_range(
                    vec![xyz(0.0, 0.0), xyz(0.0, 1.0)],
                    vec![1.0, 1.0],
                    0.0,
                    1.0,
                )
                .map_err(curve_error)?,
            ),
            1 => {
                let data = geometry_data(&args.get_item(0)?)?;
                if !matches!(data, Geom2dData::Bezier(_)) {
                    return Err(PyTypeError::new_err(
                        "BezierCurve2d copy expects a BezierCurve2d",
                    ));
                }
                data
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "BezierCurve2d constructor accepts:\n\
                     -- empty parameter list\n\
                     -- BezierCurve2d",
                ))
            }
        };
        Ok(Self::initializer(data))
    }

    #[getter]
    fn Degree(slf: &Bound<'_, Self>) -> PyResult<i32> {
        Ok(bezier2d(slf.as_any())?.degree())
    }

    /// OCCT's maximum Bezier degree.
    #[getter]
    fn MaxDegree(&self) -> i32 {
        25
    }

    #[getter]
    fn NbPoles(slf: &Bound<'_, Self>) -> PyResult<usize> {
        Ok(bezier2d(slf.as_any())?.poles().len())
    }

    #[getter]
    fn StartPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let curve = bezier2d(slf.as_any())?;
        py_vector2d(py, xy(curve.start()))
    }

    #[getter]
    fn EndPoint(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let curve = bezier2d(slf.as_any())?;
        py_vector2d(py, xy(curve.end()))
    }

    fn isRational(slf: &Bound<'_, Self>) -> PyResult<bool> {
        Ok(bezier2d(slf.as_any())?.is_rational())
    }

    fn isPeriodic(&self) -> bool {
        false
    }

    fn isClosed(slf: &Bound<'_, Self>) -> PyResult<bool> {
        Ok(bezier2d(slf.as_any())?.is_closed())
    }

    /// `getPole(index)` — the 1-based pole.
    fn getPole(slf: &Bound<'_, Self>, py: Python<'_>, index: i32) -> PyResult<PyObject> {
        let curve = bezier2d(slf.as_any())?;
        let position = index_at(curve.poles().len(), index)? as usize - 1;
        py_vector2d(py, xy(curve.poles()[position]))
    }

    fn getPoles(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        bezier2d(slf.as_any())?
            .poles()
            .iter()
            .map(|pole| py_vector2d(py, xy(*pole)))
            .collect()
    }

    #[pyo3(signature = (index, pole, weight = None))]
    fn setPole(
        slf: &Bound<'_, Self>,
        index: i32,
        pole: &Bound<'_, PyAny>,
        weight: Option<f64>,
    ) -> PyResult<()> {
        let point = vector2d(pole)?;
        let mut curve = bezier2d(slf.as_any())?;
        let position = index_at(curve.poles().len(), index)? as usize - 1;
        curve
            .set_pole(position, xyz(point.0, point.1), weight)
            .map_err(curve_error)?;
        replace_geometry_data(slf.as_any(), Geom2dData::Bezier(curve))
    }

    fn setPoles(slf: &Bound<'_, Self>, poles: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut parsed = Vec::new();
        for point in poles.try_iter()? {
            let point = vector2d(&point?)?;
            parsed.push(xyz(point.0, point.1));
        }
        let mut curve = bezier2d(slf.as_any())?;
        curve.set_poles(parsed).map_err(curve_error)?;
        replace_geometry_data(slf.as_any(), Geom2dData::Bezier(curve))
    }

    fn getWeight(slf: &Bound<'_, Self>, index: i32) -> PyResult<f64> {
        let curve = bezier2d(slf.as_any())?;
        let position = index_at(curve.weights().len(), index)? as usize - 1;
        Ok(curve.weights()[position])
    }

    fn getWeights(slf: &Bound<'_, Self>) -> PyResult<Vec<f64>> {
        Ok(bezier2d(slf.as_any())?.weights().to_vec())
    }

    fn setWeight(slf: &Bound<'_, Self>, index: i32, weight: f64) -> PyResult<()> {
        let mut curve = bezier2d(slf.as_any())?;
        let position = index_at(curve.weights().len(), index)? as usize - 1;
        curve.set_weight(position, weight).map_err(curve_error)?;
        replace_geometry_data(slf.as_any(), Geom2dData::Bezier(curve))
    }

    /// `segment(u1, u2)` — trim the curve in place.
    fn segment(slf: &Bound<'_, Self>, u1: f64, u2: f64) -> PyResult<()> {
        let curve = bezier2d(slf.as_any())?;
        let trimmed = curve.trim(u1, u2).map_err(curve_error)?;
        replace_geometry_data(slf.as_any(), Geom2dData::Bezier(trimmed))
    }

    #[pyo3(signature = (*_args))]
    fn increase(&self, _args: &Bound<'_, PyTuple>) -> PyResult<()> {
        Err(geom2d_not_implemented("BezierCurve2d.increase"))
    }

    #[pyo3(signature = (*_args))]
    fn insertPoleAfter(&self, _args: &Bound<'_, PyTuple>) -> PyResult<()> {
        Err(geom2d_not_implemented("BezierCurve2d.insertPoleAfter"))
    }

    #[pyo3(signature = (*_args))]
    fn insertPoleBefore(&self, _args: &Bound<'_, PyTuple>) -> PyResult<()> {
        Err(geom2d_not_implemented("BezierCurve2d.insertPoleBefore"))
    }

    #[pyo3(signature = (*_args))]
    fn removePole(&self, _args: &Bound<'_, PyTuple>) -> PyResult<()> {
        Err(geom2d_not_implemented("BezierCurve2d.removePole"))
    }

    #[pyo3(signature = (*_args))]
    fn getResolution(&self, _args: &Bound<'_, PyTuple>) -> PyResult<f64> {
        Err(geom2d_not_implemented("BezierCurve2d.getResolution"))
    }
}

// --- BSplineCurve2d --------------------------------------------------------

/// `Part.Geom2d.BSplineCurve2d()` — upstream's 2D B-spline curve.
///
/// No 2D B-spline kernel is bound in this slice, so `interpolate` and
/// `approximate` only **record** the points they are given; no curve is computed
/// behind that record. Every operation that would need the actual curve
/// (`insertKnot`, `value`, `toShape`) reports `Part.OCCError` /
/// `NotImplementedError` rather than answering from a curve that was never
/// built.
///
/// The four-argument call upstream's `Geom2d_tests` uses
/// (`insertKnot(0.5, 1, 0.01, 2)`) is rejected by the signature as a `TypeError`,
/// just as upstream's `PyArg_ParseTuple("d|id", …)` rejects the extra argument.
#[pyclass(name = "BSplineCurve2d", module = "Part.Geom2d", extends = PyCurve2d)]
pub struct PyBSplineCurve2d;
curve2d_initializer!(PyBSplineCurve2d);

impl PyBSplineCurve2d {
    fn point_count(value: &Bound<'_, PyAny>) -> PyResult<usize> {
        match geometry_data(value)? {
            Geom2dData::BSpline(points) => Ok(points.len()),
            _ => Err(OCCError::new_err("the object is not a BSplineCurve2d")),
        }
    }

    /// Parse and remember the `Points` argument of `interpolate`/`approximate`.
    fn record(slf: &Bound<'_, Self>, points: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut parsed = Vec::new();
        for point in points.try_iter()? {
            parsed.push(vector2d(&point?)?);
        }
        if parsed.len() < 2 {
            return Err(OCCError::new_err(
                "a 2D B-spline needs at least two points",
            ));
        }
        replace_geometry_data(slf.as_any(), Geom2dData::BSpline(parsed))
    }
}

#[pymethods]
impl PyBSplineCurve2d {
    #[new]
    fn new() -> PyResult<PyClassInitializer<Self>> {
        Ok(Self::initializer(Geom2dData::BSpline(Vec::new())))
    }

    /// `interpolate(Points, PeriodicFlag=False, Tolerance=1e-7)` — record the
    /// points to interpolate (upstream's `Geom2dAPI_Interpolate`).
    #[pyo3(signature = (Points, PeriodicFlag = false, Tolerance = 1e-7))]
    fn interpolate(
        slf: &Bound<'_, Self>,
        Points: &Bound<'_, PyAny>,
        PeriodicFlag: bool,
        Tolerance: f64,
    ) -> PyResult<()> {
        let _ = (PeriodicFlag, Tolerance);
        Self::record(slf, Points)
    }

    /// `approximate(Points, DegMax=8, Continuity="C2", Tolerance=1e-3,
    /// DegMin=3, ParamType="ChordLength")` — record the points to approximate
    /// (upstream's `Geom2dAPI_PointsToBSpline`).
    #[pyo3(signature = (Points, DegMax = 8, Continuity = "C2", Tolerance = 1e-3, DegMin = 3, ParamType = "ChordLength"))]
    fn approximate(
        slf: &Bound<'_, Self>,
        Points: &Bound<'_, PyAny>,
        DegMax: i32,
        Continuity: &str,
        Tolerance: f64,
        DegMin: i32,
        ParamType: &str,
    ) -> PyResult<()> {
        let _ = (DegMax, Continuity, Tolerance, DegMin, ParamType);
        Self::record(slf, Points)
    }

    /// `buildFromPolesMultsKnots(poles, mults=None, knots=None, periodic=False,
    /// degree=3, weights=None)` — build the 2D B-spline from control points.
    ///
    /// The 2D binding records the poles (its `toShape` still reports that no 2D
    /// B-spline kernel is bound), so the signature and the pole list match
    /// upstream's `BSplineCurve2d`.
    #[pyo3(signature = (poles, mults = None, knots = None, periodic = false, degree = 3, weights = None))]
    fn buildFromPolesMultsKnots(
        slf: &Bound<'_, Self>,
        poles: &Bound<'_, PyAny>,
        mults: Option<&Bound<'_, PyAny>>,
        knots: Option<&Bound<'_, PyAny>>,
        periodic: bool,
        degree: i32,
        weights: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let _ = (mults, knots, periodic, degree, weights);
        Self::record(slf, poles)
    }

    #[pyo3(signature = (u, mult = 1, tol = 0.0))]
    fn insertKnot(&self, u: f64, mult: i32, tol: f64) -> PyResult<()> {
        let _ = (u, mult, tol);
        Err(OCCError::new_err(
            "Part.Geom2d.BSplineCurve2d.insertKnot is not implemented \
             (this slice binds no 2D B-spline kernel)",
        ))
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        Ok(format!(
            "Part.Geom2d.BSplineCurve2d({} points)",
            Self::point_count(slf.as_any())?
        ))
    }
}

/// Build the Python object for a piece of 2D geometry, picking the concrete
/// `Part.Geom2d` class from the data.
fn geometry2d_to_py(py: Python<'_>, data: Geom2dData) -> PyResult<PyObject> {
    Ok(match data {
        data @ Geom2dData::Line(_) => Py::new(py, PyLine2d::initializer(data))?.into_any(),
        data @ Geom2dData::Segment { .. } => {
            Py::new(py, PyLine2dSegment::initializer(data))?.into_any()
        }
        data @ Geom2dData::Circle(_) => Py::new(py, PyCircle2d::initializer(data))?.into_any(),
        data @ Geom2dData::Arc(_) => {
            Py::new(py, PyArcOfCircle2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::Ellipse(_) => Py::new(py, PyEllipse2d::initializer(data))?.into_any(),
        data @ Geom2dData::ArcOfEllipse { .. } => {
            Py::new(py, PyArcOfEllipse2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::Hyperbola(_) => {
            Py::new(py, PyHyperbola2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::ArcOfHyperbola(_) => {
            Py::new(py, PyArcOfHyperbola2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::Parabola(_) => {
            Py::new(py, PyParabola2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::ArcOfParabola(_) => {
            Py::new(py, PyArcOfParabola2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::Bezier(_) => {
            Py::new(py, PyBezierCurve2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::BSpline(_) => {
            Py::new(py, PyBSplineCurve2d::initializer(data))?.into_any()
        }
        data @ Geom2dData::Offset { .. } => {
            Py::new(py, PyOffsetCurve2d::initializer(data))?.into_any()
        }
    })
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

/// Reject anything that is not a surface or a face this slice models.
///
/// The surface is not used for a projection (no `GeomProjLib` is bound), so this
/// is a type check: it keeps a bogus `toShape(42)` a `TypeError` instead of a
/// silent success.
fn require_surface_or_face(value: &Bound<'_, PyAny>) -> PyResult<()> {
    if value.downcast::<PyFace>().is_ok() {
        return Ok(());
    }
    if value.downcast::<PyCylinder>().is_ok()
        || value.downcast::<PySphere>().is_ok()
        || value.downcast::<PyPlane>().is_ok()
        || value.downcast::<PyRectangularTrimmedSurface>().is_ok()
        || value.downcast::<PySurface>().is_ok()
        || value.downcast::<crate::solids::PyBSplineSurface>().is_ok()
    {
        return Ok(());
    }
    Err(PyTypeError::new_err(
        "toShape(surface) expects a Part surface \
         (Part.Plane, Part.Cylinder, Part.Sphere, Part.BSplineSurface or \
         Part.RectangularTrimmedSurface) or a Part.Face",
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

/// An edge's start (`first`) or end vertex.
///
/// The kernel lists an edge's vertices in topological order but exposes no
/// "first endpoint" query, so the list's first/last entry is the endpoint. A
/// closed edge has one vertex, which answers both ends.
fn edge_vertex(edge: &PyRef<'_, PyEdge>, first: bool) -> PyResult<Py<PyVertex>> {
    let what = if first { "firstVertex" } else { "lastVertex" };
    require_edge(edge.as_super(), what)?;
    let shape = edge
        .as_super()
        .shape()
        .ok_or_else(|| empty_error(what))?;
    let vertices = backend().elements(&shape, ElementKind::Vertex);
    let vertex = if first {
        vertices.first()
    } else {
        vertices.last()
    }
    .ok_or_else(|| OCCError::new_err(format!("Part.{what}: the edge has no vertices")))?;
    Py::new(edge.py(), PyVertex::from_shape(vertex.shape.clone())?)
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
    curve2d: Mutex<Option<Geom2dData>>,
    /// The exact B-spline an edge built from a conic or a Bezier stores. There is
    /// no `EdgeCurve` variant for it, so it is carried beside the entry and
    /// reported by `Curve` when the kernel cannot classify the edge.
    bspline: Mutex<Option<SplineState>>,
}

impl PyEdge {
    fn initializer(entry: EdgeEntry) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(entry.shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(entry)),
            curve2d: Mutex::new(None),
            bspline: Mutex::new(None),
        })
    }

    /// The empty edge `Part.Edge()`: a null shape with no geometry behind it.
    fn null_initializer() -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::empty()).add_subclass(Self {
            entry: Mutex::new(None),
            curve2d: Mutex::new(None),
            bspline: Mutex::new(None),
        })
    }

    pub(crate) fn from_entry(entry: EdgeEntry) -> PyClassInitializer<Self> {
        Self::initializer(entry)
    }

    /// An edge built from a 2D curve, with that curve attached as its pcurve.
    pub(crate) fn from_entry_with_curve2d(
        entry: EdgeEntry,
        curve2d: Geom2dData,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(entry.shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(entry)),
            curve2d: Mutex::new(Some(curve2d)),
            bspline: Mutex::new(None),
        })
    }

    /// An edge whose geometry is an exact B-spline (a conic or a Bezier).
    fn from_bspline(shape: Shape, state: SplineState) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(EdgeEntry {
                shape,
                curve: None,
            })),
            curve2d: Mutex::new(None),
            bspline: Mutex::new(Some(state)),
        })
    }

    /// An edge whose geometry is an exact B-spline, with a 2D curve attached as
    /// its pcurve (a trimmed elliptical arc).
    fn from_bspline_with_curve2d(
        shape: Shape,
        state: SplineState,
        curve2d: Geom2dData,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(shape.clone())).add_subclass(Self {
            entry: Mutex::new(Some(EdgeEntry {
                shape,
                curve: None,
            })),
            curve2d: Mutex::new(Some(curve2d)),
            bspline: Mutex::new(Some(state)),
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
        if let Ok(ellipse) = value.downcast::<PyEllipse>() {
            let curve = EdgeCurve::Ellipse(ellipse.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(arc) = value.downcast::<PyArcOfEllipse>() {
            let arc = arc.borrow();
            let curve = EdgeCurve::EllipticalArc(
                EllipticalArc::new(arc.ellipse, arc.first, arc.last).map_err(curve_error)?,
            );
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(hyperbola) = value.downcast::<PyHyperbola>() {
            let curve = EdgeCurve::Hyperbola(hyperbola.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(arc) = value.downcast::<PyArcOfHyperbola>() {
            let curve = EdgeCurve::ArcOfHyperbola(arc.borrow().arc.clone());
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(parabola) = value.downcast::<PyParabola>() {
            let curve = EdgeCurve::Parabola(parabola.borrow().curve);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(arc) = value.downcast::<PyArcOfParabola>() {
            let curve = EdgeCurve::ArcOfParabola(arc.borrow().arc.clone());
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(bezier) = value.downcast::<PyBezierCurve>() {
            let curve = EdgeCurve::BezierCurve(bezier.borrow().curve());
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(spline) = value.downcast::<PyBSplineCurve>() {
            let curve = EdgeCurve::BSplineCurve(spline.borrow().to_nurbs()?);
            return Ok(EdgeEntry {
                shape: shape_from_curve(&curve)?,
                curve: Some(curve),
            });
        }
        if let Ok(offset) = value.downcast::<PyOffsetCurve>() {
            let curve = EdgeCurve::OffsetCurve(offset.borrow().curve.clone());
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
    /// an edge, a curve or a `Part.Shape`; `Part.Edge(curve, first, last)` is the
    /// piece of that curve between the two parameters.
    #[new]
    #[pyo3(signature = (value = None, first = None, last = None))]
    fn new(
        value: Option<&Bound<'_, PyAny>>,
        first: Option<f64>,
        last: Option<f64>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let Some(value) = value else {
            if first.is_some() || last.is_some() {
                return Err(PyTypeError::new_err(
                    "Part.Edge needs a curve for a parameter range",
                ));
            }
            return Ok(Self::null_initializer());
        };
        let entry = Self::entry_from(value)?;
        match (first, last) {
            (None, None) => Ok(Self::initializer(entry)),
            (Some(first), Some(last)) => {
                let curve = match entry.curve {
                    Some(curve) => curve,
                    None => backend().curve_of(&entry.shape).map_err(kernel_error)?,
                };
                let trimmed = curve.trimmed(first, last).ok_or_else(|| {
                    OCCError::new_err(
                        "Part.Edge(curve, first, last) cannot be trimmed to that range",
                    )
                })?;
                Self::from_curve(trimmed)
            }
            _ => Err(PyTypeError::new_err(
                "Part.Edge(curve, first, last) needs both parameters",
            )),
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
            Err(_) => {
                if let Some(curve) = entry.curve {
                    return Ok(Some(curve_to_py(py, curve)?));
                }
                let state = self
                    .bspline
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                match state {
                    Some(state) => Ok(Some(Py::new(py, PyBSplineCurve::from_state(state))?.into_any())),
                    None => Ok(None),
                }
            }
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
        let curve2d = self
            .curve2d
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match curve2d {
            Some(data) => Ok(Some(geometry2d_to_py(py, data)?)),
            None => Ok(None),
        }
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
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("Tolerance"))?;
        backend()
            .shape_tolerance(&shape)
            .map_err(kernel_error)
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
    fn ParameterRange(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("ParameterRange"))?;
        let (first, last) = backend()
            .edge_parameter_range(&shape)
            .map_err(kernel_error)?;
        Ok(PyTuple::new(py, [first, last])?.into_any().unbind())
    }

    #[getter]
    fn FirstParameter(slf: PyRef<'_, Self>) -> PyResult<f64> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("FirstParameter"))?;
        Ok(backend()
            .edge_parameter_range(&shape)
            .map_err(kernel_error)?
            .0)
    }

    #[getter]
    fn LastParameter(slf: PyRef<'_, Self>) -> PyResult<f64> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("LastParameter"))?;
        Ok(backend()
            .edge_parameter_range(&shape)
            .map_err(kernel_error)?
            .1)
    }

    /// Whether the edge's 3D curve collapses to a point (`Edge.Degenerated`).
    /// A cone apex or a sphere pole carries such an edge.
    #[getter]
    fn Degenerated(slf: PyRef<'_, Self>) -> PyResult<bool> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("Degenerated"))?;
        backend().is_degenerate(&shape).map_err(kernel_error)
    }

    /// Whether the edge is a seam of `face` (`Edge.isSeam`): it has two pcurves
    /// on the face because the face's surface is closed.
    fn isSeam(slf: PyRef<'_, Self>, face: &Bound<'_, PyAny>) -> PyResult<bool> {
        let edge = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("isSeam"))?;
        let face = face
            .downcast::<PartShape>()
            .map_err(|_| PyTypeError::new_err("isSeam expects a Part.Face"))?;
        let face_shape = face
            .borrow()
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape cannot bound a seam edge"))?;
        backend()
            .is_seam(&edge, &face_shape)
            .map_err(kernel_error)
    }

    /// The 3D point at curve parameter `t` (`Edge.valueAt`, upstream's
    /// `BRepAdaptor_Curve::Value`). An empty edge raises `ValueError`.
    fn valueAt(slf: PyRef<'_, Self>, py: Python<'_>, parameter: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("valueAt"))?;
        let point = backend()
            .edge_value_at(&shape, parameter)
            .map_err(kernel_error)?;
        py_vector(py, point)
    }

    /// The unit tangent at curve parameter `t` (`Edge.tangentAt`), the
    /// normalised first derivative of the edge's curve.
    fn tangentAt(slf: PyRef<'_, Self>, py: Python<'_>, parameter: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("tangentAt"))?;
        py_vector(py, edge_derivative(&shape, parameter)?.normalize())
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

    /// The first derivative at curve parameter `t` (`Edge.derivative1At`).
    fn derivative1At(slf: PyRef<'_, Self>, py: Python<'_>, parameter: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("derivative1At"))?;
        py_vector(py, edge_derivative(&shape, parameter)?)
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
        edge_vertex(&slf, true)
    }

    fn lastVertex(slf: PyRef<'_, Self>) -> PyResult<Py<PyVertex>> {
        edge_vertex(&slf, false)
    }

    /// Sample the edge into points (`Edge.discretize`), upstream's
    /// `GCPnts_*` samplers.
    ///
    /// - a positional int, or `Number=n`, gives `n` points at uniform arc length;
    /// - a positional float, or `Distance=d`, gives points every `d` along the
    ///   edge;
    /// - `Deflection=d` bounds the sagitta;
    /// - `Angular=a, Curvature=c, Minimum=n` bounds the turn and the chord error.
    ///
    /// `First`/`Last` trim the sampled span. Only `Number`, `Distance`,
    /// `Deflection` and `Angular`/`Curvature` are supported; the quasi-uniform
    /// forms report an honest `OCCError` rather than silently approximating.
    #[pyo3(signature = (*args, **kwargs))]
    fn discretize(
        slf: PyRef<'_, Self>,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Vec<PyObject>> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("discretize"))?;

        let mut mode = -1;
        let mut value = 0.0;
        let mut curvature = 0.0;
        let mut minimum = 2;

        if args.len() > 1 {
            return Err(PyTypeError::new_err(
                "discretize expects one number or keyword arguments",
            ));
        }
        if args.len() == 1 {
            let argument = args.get_item(0)?;
            if argument.is_instance_of::<pyo3::types::PyInt>() {
                mode = 0;
                value = argument.extract::<i64>()? as f64;
            } else if argument.is_instance_of::<pyo3::types::PyFloat>() {
                mode = 1;
                value = argument.extract::<f64>()?;
            } else {
                return Err(PyTypeError::new_err("Either int or float expected"));
            }
        }

        let mut first = f64::NAN;
        let mut last = f64::NAN;
        if let Some(kwargs) = kwargs {
            let number = |name: &str| -> PyResult<Option<f64>> {
                match kwargs.get_item(name)? {
                    Some(value) => Ok(Some(value.extract::<f64>()?)),
                    None => Ok(None),
                }
            };
            if let Some(value) = number("First")? {
                first = value;
            }
            if let Some(value) = number("Last")? {
                last = value;
            }
            if let Some(number) = number("Number")? {
                mode = 0;
                value = number;
            } else if let Some(distance) = number("Distance")? {
                mode = 1;
                value = distance;
            } else if let Some(deflection) = number("Deflection")? {
                mode = 2;
                value = deflection;
            } else if let Some(angular) = number("Angular")? {
                mode = 3;
                value = angular;
                curvature = number("Curvature")?.unwrap_or(0.1);
                if let Some(count) = number("Minimum")? {
                    minimum = count as i32;
                }
            } else if let Some(curvature_value) = number("Curvature")? {
                mode = 3;
                value = 0.1;
                curvature = curvature_value;
                if let Some(count) = number("Minimum")? {
                    minimum = count as i32;
                }
            }
        }
        if mode < 0 {
            return Err(PyTypeError::new_err(
                "discretize expects Number, Distance, Deflection or Angular/Curvature",
            ));
        }

        let points = backend()
            .edge_discretize(&shape, mode, value, curvature, minimum, first, last)
            .map_err(|_| OCCError::new_err("Discretization of edge failed"))?;
        points.iter().map(|point| py_vector(py, *point)).collect()
    }

    /// The curve parameter at arc length `length` from the first parameter
    /// (`Edge.getParameterByLength`, `GCPnts_AbscissaPoint`).
    ///
    /// A negative length counts back from the end, and a length outside the
    /// edge raises `ValueError`, as upstream.
    #[pyo3(signature = (length, tolerance = 1e-7))]
    fn getParameterByLength(
        slf: PyRef<'_, Self>,
        length: f64,
        tolerance: f64,
    ) -> PyResult<f64> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("getParameterByLength"))?;
        backend()
            .edge_parameter_by_length(&shape, length, tolerance)
            .map_err(|error| {
                let message = error.to_string();
                if message.contains("out of range") {
                    PyValueError::new_err("value out of range")
                } else {
                    OCCError::new_err(message)
                }
            })
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

    /// The wire's vertices in edge-chain order (`Wire.OrderedVertexes`): the
    /// start vertex of each edge, following the vertices adjacent edges share,
    /// plus the final vertex of an open wire. A closed wire reports each corner
    /// once, as upstream's `BRepTools_WireExplorer` walk does.
    ///
    /// A wire that is not one chain (or an edge without a clean pair of
    /// endpoints) falls back to the kernel's vertex list, so the result is never
    /// worse than `Vertexes`.
    #[getter]
    fn OrderedVertexes(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Vec<Py<PyVertex>>> {
        if slf.as_super().shape().is_none() {
            return Ok(Vec::new());
        }
        let edges = slf.as_super().sub_shapes(ElementKind::Edge)?;
        let shapes = match ordered_vertex_shapes(&edges) {
            Some(shapes) => shapes,
            None => slf.as_super().sub_shapes(ElementKind::Vertex)?,
        };
        shapes
            .into_iter()
            .map(|shape| Py::new(py, PyVertex::from_shape(shape)?))
            .collect()
    }

    /// Whether the wire is closed (`Wire.isClosed`).
    ///
    /// A wire with kernel geometry reports OCCT's own `BRep_Tool::IsClosed` flag,
    /// exactly as `Shape.isClosed` and upstream's `TopoShape::isClosed` do, so the
    /// wire and its `Wires[0]` never disagree. Only a wire with no kernel shape yet
    /// falls back to comparing the analytic endpoints.
    fn isClosed(slf: PyRef<'_, Self>) -> PyResult<bool> {
        if let Some(shape) = slf.as_super().shape() {
            return backend().is_closed(&shape).map_err(kernel_error);
        }
        let entries = slf.entries();
        let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
            return Ok(false);
        };
        let points = |curve: EdgeCurve| curve.endpoints();
        Ok(match (
            first.curve.clone().and_then(points),
            last.curve.clone().and_then(points),
        ) {
            (Some((start, _)), Some((_, end))) => {
                start.distance(&end) <= ferrocad_curves::TOLERANCE
            }
            _ => false,
        })
    }

    /// The wire's edges in connection order (`Wire.OrderedEdges`), the
    /// `BRepTools_WireExplorer` walk upstream exposes as a property.
    #[getter]
    fn OrderedEdges(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Vec<Py<PyEdge>>> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("OrderedEdges"))?;
        ferrocad_occt::part_ops::wire_ordered_edges(&shape)
            .map_err(kernel_error)?
            .into_iter()
            .map(|edge| Py::new(py, PyEdge::from_entry(EdgeEntry { shape: edge, curve: None })))
            .collect()
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

/// The two endpoint vertices of an edge, in either order.
///
/// A closed edge (a full circle) has one vertex used at both ends; a shape with
/// anything other than one or two vertices is not a chainable edge.
fn edge_endpoint_shapes(edge: &Shape) -> Option<(Shape, Shape)> {
    let vertices = backend().elements(edge, ElementKind::Vertex);
    match vertices.as_slice() {
        [vertex] => Some((vertex.shape.clone(), vertex.shape.clone())),
        [first, second] => Some((first.shape.clone(), second.shape.clone())),
        _ => None,
    }
}

/// Order a wire's vertices by following the vertices adjacent edges share.
///
/// `None` when the edges do not form one chain. The walk starts at the first
/// edge and grows both ways, so an open wire whose first edge is in the middle
/// still comes out end-to-end. Edge directions are irrelevant: a wire is a
/// cyclic chain, and an open wire's direction only decides which end is first.
fn ordered_vertex_shapes(edges: &[Shape]) -> Option<Vec<Shape>> {
    if edges.is_empty() {
        return Some(Vec::new());
    }
    let endpoints = edges
        .iter()
        .map(edge_endpoint_shapes)
        .collect::<Option<Vec<_>>>()?;
    let same = |a: &Shape, b: &Shape| backend().same_shape(a, b);

    let mut used = vec![false; edges.len()];
    used[0] = true;
    let (first, second) = endpoints[0].clone();

    // Grow from the first edge's second endpoint.
    let mut back = vec![second.clone()];
    let mut tail = second;
    loop {
        let Some((index, next)) = endpoints.iter().enumerate().find_map(|(index, (a, b))| {
            if used[index] {
                None
            } else if same(&tail, a) {
                Some((index, b.clone()))
            } else if same(&tail, b) {
                Some((index, a.clone()))
            } else {
                None
            }
        }) else {
            break;
        };
        used[index] = true;
        tail = next.clone();
        back.push(next);
    }

    // Grow the other way from the first edge's first endpoint.
    let mut front = vec![first.clone()];
    let mut head = first;
    loop {
        let Some((index, next)) = endpoints.iter().enumerate().find_map(|(index, (a, b))| {
            if used[index] {
                None
            } else if same(&head, a) {
                Some((index, b.clone()))
            } else if same(&head, b) {
                Some((index, a.clone()))
            } else {
                None
            }
        }) else {
            break;
        };
        used[index] = true;
        head = next.clone();
        front.push(next);
    }

    if used.iter().any(|used| !used) {
        return None;
    }

    // `front` runs [first, …, far end]; reverse it so it ends at the junction
    // `first`, then append the chain that starts at the second endpoint.
    front.reverse();
    let mut ordered = front;
    ordered.extend(back);

    // A chain that closes on itself reports its starting corner once.
    if ordered.len() > 1 && same(&ordered[0], &ordered[ordered.len() - 1]) {
        ordered.pop();
    }
    Some(ordered)
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

    /// Wrap an existing face shape (`ancestorsOfType`, a sub-shape query).
    pub(crate) fn from_shape(shape: Shape) -> PyClassInitializer<Self> {
        Self::initializer(shape)
    }

    /// The empty face `Part.Face()`: a null shape with no boundary.
    fn empty_initializer() -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::empty()).add_subclass(Self)
    }
}

/// The unit normal of a planar face's surface, from two non-parallel tangent
/// directions of its outer wire. Used only as the fallback for a surface this
/// slice does not classify (upstream derives the normal the same way for a
/// plane, then flips it for a reversed face).
fn planar_wire_normal(shape: &Shape) -> Option<Vector3> {
    let backend = backend();
    let wire = backend.outer_wire(shape)?;
    let mut directions = Vec::new();
    for edge in backend.elements(&wire, ElementKind::Edge) {
        if let Ok(curve) = backend.curve_of(&edge.shape) {
            directions.push(curve.tangent_at(0.0));
        }
    }
    for (index, first) in directions.iter().enumerate() {
        for second in directions.iter().skip(index + 1) {
            let normal = first.cross(second);
            if normal.length() > 1e-9 {
                return Some(normal.normalize());
            }
        }
    }
    None
}

/// The first partial derivatives `(∂P/∂u, ∂P/∂v)` of a face's surface at
/// `(u, v)`, by central differences over `Face.valueAt`.
///
/// The seam exposes the surface point but not a `D1` query, so this differentiates
/// numerically; the step is scaled to the parameter magnitude so large
/// parameters do not lose precision.
fn surface_derivatives(shape: &Shape, u: f64, v: f64) -> PyResult<(Vector3, Vector3)> {
    let backend = backend();
    let h = 1e-6 * (1.0 + u.abs().max(v.abs()));
    let plus_u = backend
        .face_value_at(shape, u + h, v)
        .map_err(kernel_error)?;
    let minus_u = backend
        .face_value_at(shape, u - h, v)
        .map_err(kernel_error)?;
    let plus_v = backend
        .face_value_at(shape, u, v + h)
        .map_err(kernel_error)?;
    let minus_v = backend
        .face_value_at(shape, u, v - h)
        .map_err(kernel_error)?;
    Ok((
        plus_u.sub(&minus_u).scale(1.0 / (2.0 * h)),
        plus_v.sub(&minus_v).scale(1.0 / (2.0 * h)),
    ))
}

/// The derivative of an edge's curve at `parameter`, by central differences over
/// `Edge.valueAt` (the seam exposes no `Geom_Curve::DN`).
fn edge_derivative(shape: &Shape, parameter: f64) -> PyResult<Vector3> {
    let backend = backend();
    let h = 1e-6 * (1.0 + parameter.abs());
    let plus = backend
        .edge_value_at(shape, parameter + h)
        .map_err(kernel_error)?;
    let minus = backend
        .edge_value_at(shape, parameter - h)
        .map_err(kernel_error)?;
    Ok(plus.sub(&minus).scale(1.0 / (2.0 * h)))
}

/// The `v` direction of a surface frame: `axis × x_dir` for a direct frame and
/// the opposite for an indirect one.
fn surface_y_dir(axis: Vector3, x_dir: Vector3, direct: bool) -> Vector3 {
    let y_dir = axis.cross(&x_dir);
    if direct {
        y_dir
    } else {
        y_dir.neg()
    }
}

/// The signed angle from the plane's reference direction to `x_axis`, about
/// `normal` (`Conic.AngleXU`). A conic framed by `Circle::new`/`Ellipse::new`
/// reports zero.
fn axis_angle(normal: Vector3, x_axis: Vector3) -> f64 {
    let reference = ferrocad_curves::reference_x_axis(normal);
    let y_axis = normal.cross(&reference);
    x_axis.dot(&y_axis).atan2(x_axis.dot(&reference))
}

/// The outward unit normal of a face at surface parameters `(u, v)`.
///
/// A classified surface gives the analytic normal (constant on a plane, radial
/// on a cylinder or sphere); any other surface falls back to the planar edge
/// normal, and reports an honest error when that is undefined. The face
/// orientation flips the result, as upstream's `Face.normalAt` (and
/// `Tools::getNormal`) does.
fn surface_normal(shape: &Shape, u: f64, v: f64) -> PyResult<Vector3> {
    let backend = backend();
    let normal = match backend.surface_data(shape) {
        Some(SurfaceData::Plane { normal, .. }) => Some(normal),
        Some(SurfaceData::Cylinder {
            axis,
            x_dir,
            direct,
            ..
        }) => {
            let y_dir = surface_y_dir(axis, x_dir, direct);
            Some(x_dir.scale(u.cos()).add(&y_dir.scale(u.sin())))
        }
        Some(SurfaceData::Sphere {
            axis,
            x_dir,
            direct,
            ..
        }) => {
            let y_dir = surface_y_dir(axis, x_dir, direct);
            let radial = x_dir.scale(u.cos()).add(&y_dir.scale(u.sin()));
            Some(radial.scale(v.cos()).add(&axis.scale(v.sin())))
        }
        Some(SurfaceData::Cone {
            axis,
            x_dir,
            semi_angle,
            direct,
            ..
        }) => {
            let y_dir = surface_y_dir(axis, x_dir, direct);
            let radial = x_dir.scale(u.cos()).add(&y_dir.scale(u.sin()));
            Some(radial.scale(semi_angle.cos()).sub(&axis.scale(semi_angle.sin())))
        }
        Some(SurfaceData::Torus {
            axis,
            x_dir,
            direct,
            ..
        }) => {
            let y_dir = surface_y_dir(axis, x_dir, direct);
            let radial = x_dir.scale(u.cos()).add(&y_dir.scale(u.sin()));
            Some(radial.scale(v.cos()).add(&axis.scale(v.sin())))
        }
        Some(SurfaceData::Other { .. }) | None => planar_wire_normal(shape),
    };
    let Some(mut normal) = normal else {
        return Err(OCCError::new_err(
            "normal not defined: only planar, cylindrical and spherical faces are supported",
        ));
    };
    if backend.orientation(shape) == Some("Reversed") {
        normal = normal.neg();
    }
    Ok(normal.normalize())
}

/// The kernel shapes of a wire, a single shape, or a list of either.
fn wire_shapes(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<crate::containers::PyCompound>() {
        let compound = compound.borrow();
        return Ok(compound.children(compound.as_super()));
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

/// The elementary surface a `Part.Face(surface, wires)` call should build on,
/// when `value` is one of the analytic surface objects.
///
/// A trimmed surface or a generic `Part.GeomSurface` is deliberately rejected:
/// neither carries the exact analytic description the kernel constructor needs.
fn face_surface_spec(value: &Bound<'_, PyAny>) -> Option<SurfaceSpec> {
    if let Ok(plane) = value.downcast::<PyPlane>() {
        let plane = plane.borrow();
        let direct = plane.normal.cross(&plane.x_dir).dot(&plane.y_dir) > 0.0;
        return Some(SurfaceSpec::Plane {
            origin: plane.location,
            normal: plane.normal,
            x_dir: plane.x_dir,
            direct,
        });
    }
    if let Ok(cylinder) = value.downcast::<PyCylinder>() {
        let cylinder = cylinder.borrow();
        return Some(SurfaceSpec::Cylinder {
            origin: cylinder.center,
            axis: cylinder.axis,
            x_dir: cylinder.x_dir,
            radius: cylinder.radius,
            direct: cylinder.direct,
        });
    }
    if let Ok(sphere) = value.downcast::<PySphere>() {
        let sphere = sphere.borrow();
        return Some(SurfaceSpec::Sphere {
            center: sphere.center,
            axis: sphere.axis,
            x_dir: sphere.x_dir,
            radius: sphere.radius,
            direct: sphere.direct,
        });
    }
    if let Ok(cone) = value.downcast::<crate::solids::PyCone>() {
        return Some(cone.borrow().spec());
    }
    if let Ok(torus) = value.downcast::<crate::solids::PyToroid>() {
        return Some(torus.borrow().spec());
    }
    None
}

#[pymethods]
impl PyFace {
    /// `Part.Face(wire)`, `Part.Face([outer, hole…])` or
    /// `Part.Face(surface, wires)`.
    ///
    /// The first form builds a planar face from one or more boundary wires (the
    /// rest are holes). The second is upstream's
    /// `BRepBuilderAPI_MakeFace(Geom_Surface, wire)`: the boundary lies on the
    /// given elementary surface (`Part.Plane`, `Cylinder`, `Cone`, `Sphere` or
    /// `Toroid`), so a face of a cylinder or a cone can be rebuilt without
    /// passing through a solid. `face_maker` keeps upstream's
    /// `"Part::FaceMakerUnified"` selector for the planar form.
    #[new]
    #[pyo3(signature = (value = None, face_maker = None))]
    fn new(
        value: Option<&Bound<'_, PyAny>>,
        face_maker: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyClassInitializer<Self>> {
        let Some(value) = value else {
            return Ok(Self::empty_initializer());
        };
        // A B-spline surface is trimmed through its own control-point geometry.
        if let Ok(surface) = value.downcast::<crate::solids::PyBSplineSurface>() {
            let geometry = surface.borrow().geometry().cloned().ok_or_else(|| {
                OCCError::new_err("Part.Face needs a built BSplineSurface (call buildFromPolesMultsKnots)")
            })?;
            let Some(wires) = face_maker else {
                return Err(PyTypeError::new_err(
                    "Part.Face(surface, wires) needs boundary wires",
                ));
            };
            let shapes = wire_shapes(wires)?;
            let shape = ferrocad_occt::part_ops::face_on_bspline_surface(&geometry, &shapes)
                .map_err(kernel_error)?;
            return Ok(Self::initializer(shape));
        }
        if let Some(surface) = face_surface_spec(value) {
            let Some(wires) = face_maker else {
                return Err(PyTypeError::new_err(
                    "Part.Face(surface, wires) needs boundary wires",
                ));
            };
            if wires.downcast::<pyo3::types::PyString>().is_ok() {
                return Err(PyTypeError::new_err(
                    "Part.Face(surface, wires) needs boundary wires, not a face-maker name",
                ));
            }
            let shapes = wire_shapes(wires)?;
            let shape = backend()
                .make_face_on_surface(&surface, &shapes)
                .map_err(face_error)?;
            return Ok(Self::initializer(shape));
        }
        let maker = match face_maker {
            Some(maker) => maker.extract::<String>().map_err(|_| {
                PyTypeError::new_err(
                    "Part.Face expects a face-maker name or, for a surface, boundary wires",
                )
            })?,
            None => "Part::FaceMakerBullseye".to_string(),
        };
        let shapes = wire_shapes(value)?;
        let shape = if maker == "Part::FaceMakerUnified" {
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

    /// The 3D point at surface parameters `(u, v)` (`Face.valueAt`, upstream's
    /// `BRepAdaptor_Surface::Value`). An empty face raises `ValueError`.
    fn valueAt(slf: PyRef<'_, Self>, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("valueAt"))?;
        let point = backend()
            .face_value_at(&shape, u, v)
            .map_err(kernel_error)?;
        py_vector(py, point)
    }

    fn normalAt(slf: PyRef<'_, Self>, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("normalAt"))?;
        let normal = surface_normal(&shape, u, v)?;
        py_vector(py, normal)
    }

    fn tangentAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "tangentAt")?;
        Err(edge_query_error("tangentAt"))
    }

    fn curvatureAt(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "curvatureAt")?;
        Err(edge_query_error("curvatureAt"))
    }

    /// The first partial derivatives `(∂P/∂u, ∂P/∂v)` at surface parameters
    /// `(u, v)` (`Face.derivative1At`, upstream's `GeomSurface.derivative1At`).
    fn derivative1At(slf: PyRef<'_, Self>, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("derivative1At"))?;
        let (du, dv) = surface_derivatives(&shape, u, v)?;
        Ok(PyTuple::new(py, [py_vector(py, du)?, py_vector(py, dv)?])?
            .into_any()
            .unbind())
    }

    fn derivative2At(slf: PyRef<'_, Self>, _u: f64, _v: f64) -> PyResult<PyObject> {
        require_edge(slf.as_super(), "derivative2At")?;
        Err(edge_query_error("derivative2At"))
    }

    fn cutHoles(slf: PyRef<'_, Self>, _holes: &Bound<'_, PyAny>) -> PyResult<()> {
        require_edge(slf.as_super(), "cutHoles")?;
        Err(edge_query_error("cutHoles"))
    }

    /// Whether surface parameters `(u, v)` lie inside the face (`Face.isPartOfDomain`,
    /// upstream's `BRepTopAdaptor_FClass2d`).
    fn isPartOfDomain(slf: PyRef<'_, Self>, u: f64, v: f64) -> PyResult<bool> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("isPartOfDomain"))?;
        ferrocad_occt::part_ops::face_is_part_of_domain(&shape, u, v).map_err(kernel_error)
    }

    /// Repair an invalid face's wires in place (`Face.validate`), as upstream's
    /// `TopoShapeFacePy::validate`. A valid face is left untouched.
    fn validate(slf: PyRef<'_, Self>) -> PyResult<()> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("validate"))?;
        let fixed = ferrocad_occt::part_ops::validate_face(&shape).map_err(kernel_error)?;
        slf.as_super().replace_geometry(Some(fixed));
        Ok(())
    }

    fn getUVNodes(slf: PyRef<'_, Self>) -> PyResult<Vec<(f64, f64)>> {
        require_edge(slf.as_super(), "getUVNodes")?;
        Err(edge_query_error("getUVNodes"))
    }

    #[getter]
    fn Tolerance(slf: PyRef<'_, Self>) -> PyResult<f64> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("Tolerance"))?;
        backend()
            .shape_tolerance(&shape)
            .map_err(kernel_error)
    }

    #[setter]
    fn set_Tolerance(slf: PyRef<'_, Self>, _value: f64) -> PyResult<()> {
        require_edge(slf.as_super(), "Tolerance")?;
        Err(edge_query_error("Tolerance"))
    }

    #[getter]
    fn ParameterRange(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| empty_error("ParameterRange"))?;
        let (u0, u1, v0, v1) = backend()
            .face_parameter_range(&shape)
            .map_err(kernel_error)?;
        Ok(PyTuple::new(py, [u0, u1, v0, v1])?.into_any().unbind())
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

/// `Part.makeCircle(radius, center=None, normal=None, angle1=None, angle2=None)` →
/// an edge.
///
/// Upstream's own signature is `makeCircle(radius, [pnt, dir, angle1, angle2])`:
/// the two angles are in degrees and turn the result into a trimmed arc edge
/// (`Draft`'s `Circle` proxy calls exactly that form). With equal or omitted
/// angles the edge is the full circle.
#[pyfunction]
#[pyo3(signature = (radius, center = None, normal = None, angle1 = None, angle2 = None))]
fn makeCircle(
    py: Python<'_>,
    radius: f64,
    center: Option<&Bound<'_, PyAny>>,
    normal: Option<&Bound<'_, PyAny>>,
    angle1: Option<f64>,
    angle2: Option<f64>,
) -> PyResult<PyObject> {
    let center = center.map(vector).transpose()?.unwrap_or_else(Vector3::zero);
    let normal = normal
        .map(vector)
        .transpose()?
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    let circle = Circle::new(center, normal, radius).map_err(curve_error)?;
    let first = angle1.unwrap_or(0.0);
    let last = angle2.unwrap_or(360.0);
    // Upstream keeps a full circle when the endpoints coincide (`angle1 ==
    // angle2`); an omitted pair defaults to 0°/360°, which is also full.
    if (last - first).abs() <= ANGULAR_TOLERANCE
        || (last - first).abs() >= 360.0 - ANGULAR_TOLERANCE
    {
        return Ok(Py::new(py, PyEdge::from_curve(EdgeCurve::Circle(circle))?)?.into_any());
    }
    let arc = Arc::new(circle, first.to_radians(), last.to_radians()).map_err(curve_error)?;
    Ok(Py::new(py, PyEdge::from_curve(EdgeCurve::Arc(arc))?)?.into_any())
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

// ---------------------------------------------------------------------------
// Helices (`Part.makeHelix`, `Part.makeLongHelix`)
// ---------------------------------------------------------------------------

/// The kernel's confusion tolerance (upstream's `Precision::Confusion`).
const HELIX_CONFUSION: f64 = 1e-7;

/// `Part.makeHelix(pitch, height, radius, angle=-1.0, pleft=False, pvertHeight=False)`
/// → a `Part.Edge`.
///
/// Upstream builds the helix as a straight segment in the `(u, v)` parameter
/// space of an analytic cylinder or cone and returns that single edge
/// (`TopoShape::makeHelix`). The kernel's [`surface_ops::make_helix_edge`]
/// builds exactly the same object — an edge whose pcurve is the exact segment —
/// so `makeHelix` now returns the exact edge rather than an interpolating
/// B-spline.
///
/// `angle` is the cone's half angle in degrees; a value below
/// `Precision::Confusion` selects the cylinder, exactly as upstream. For a cone,
/// `pvertHeight` selects upstream's *new style*: the segment ends at
/// `v = height / cos(angle)`, whose axial rise is exactly `height` (mantis
/// 0001247). The default (old style) ends at `v = height`, i.e. an axial rise of
/// `height * cos(angle)`. Both styles are exact segments in the cone's `(u, v)`
/// space: the old style is the kernel's new-style segment with **both** `pitch`
/// and `height` scaled by `cos(angle)`.
#[pyfunction]
#[pyo3(signature = (pitch, height, radius, angle = -1.0, pleft = false, pvertHeight = false))]
fn makeHelix(
    py: Python<'_>,
    pitch: f64,
    height: f64,
    radius: f64,
    angle: f64,
    pleft: bool,
    pvertHeight: bool,
) -> PyResult<Py<PyEdge>> {
    // Upstream's checks and messages, in its order (`TopoShape::makeHelix`).
    if !pitch.is_finite() || pitch.abs() < HELIX_CONFUSION {
        return Err(OCCError::new_err("Pitch of helix too small"));
    }
    if !height.is_finite() || height.abs() < HELIX_CONFUSION {
        return Err(OCCError::new_err("Height of helix too small"));
    }
    if (height > 0.0 && pitch < 0.0) || (height < 0.0 && pitch > 0.0) {
        return Err(OCCError::new_err(
            "Pitch and height of helix not compatible",
        ));
    }
    let conical = angle >= HELIX_CONFUSION;
    if !conical {
        if radius < HELIX_CONFUSION {
            return Err(OCCError::new_err("Radius of helix too small"));
        }
    } else if angle.to_radians() < HELIX_CONFUSION {
        return Err(OCCError::new_err("Angle of helix too small"));
    }

    // Upstream's old-style conical segment: `line->Value` ends at `(u, v)` with
    // `v = height`, while the analytic cone parameterisation the kernel builds
    // ends at `v = height / cos(angle)`. Scaling the requested `pitch` and
    // `height` by `cos(angle)` leaves `u` unchanged and puts `v` at `height`.
    let (pitch_argument, height_argument) = if conical && !pvertHeight {
        let cosine = angle.to_radians().cos();
        (pitch * cosine, height * cosine)
    } else {
        (pitch, height)
    };

    let shape = surface_ops::make_helix_edge(
        pitch_argument,
        height_argument,
        radius,
        if conical { angle } else { 0.0 },
        pleft,
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

/// The axial height one whole turn of the helix rises.
///
/// OCCT's cone parameter `v` is the distance along the generator
/// (`z = v cos(angle)`), and upstream's `makeLongHelix` advances `v` by one
/// pitch per turn, so a conical turn rises `pitch cos(angle)`.
fn helix_turn_height(pitch: f64, conical: bool, angle: f64) -> f64 {
    if conical {
        pitch * angle.to_radians().cos()
    } else {
        pitch
    }
}

/// The height (`z`) of the split point at `parameter` on an exact helix edge.
///
/// The seam exposes no point evaluation, but `split_edge` returns the pieces and
/// their shared vertex is the point at the parameter. An exact helix is strictly
/// monotone in `z`, so the interior vertex is the split point.
fn helix_split_height(edge: &Shape, parameter: f64, z_end: f64) -> Result<f64, String> {
    let wire = backend()
        .split_edge(edge, &[parameter])
        .map_err(|error| error.to_string())?;
    let margin = 1e-6 * z_end.abs().max(1.0);
    let mut heights = Vec::new();
    for element in backend().elements(&wire, ElementKind::Edge) {
        for vertex in backend().elements(&element.shape, ElementKind::Vertex) {
            if let Some(point) = backend().vertex_point(&vertex.shape) {
                if point.z > margin && point.z < z_end - margin {
                    heights.push(point.z);
                }
            }
        }
    }
    heights.sort_by(|a, b| a.total_cmp(b));
    heights.dedup_by(|a, b| (*a - *b).abs() <= margin * 0.1);
    heights
        .first()
        .copied()
        .ok_or_else(|| "the helix turn boundary could not be measured".to_string())
}

/// The largest parameter `split_edge` accepts on `edge`, i.e. the last parameter
/// of its 3D representative.
///
/// The representative's parameter is **not** the surface `u` (OCCT's
/// `BRepLib::BuildCurves3d` rebuilds it as a B-spline with its own
/// parameterisation), so this is found by bracketing rather than assumed.
fn helix_split_limit(edge: &Shape) -> Result<f64, String> {
    let mut inside = 0.0;
    let mut outside = 1.0;
    while backend().split_edge(edge, &[outside]).is_ok() {
        inside = outside;
        outside *= 2.0;
        if outside > 1.0e30 {
            return Err("the helix edge has no finite parameter limit".to_string());
        }
    }
    for _ in 0..60 {
        let middle = 0.5 * (inside + outside);
        if backend().split_edge(edge, &[middle]).is_ok() {
            inside = middle;
        } else {
            outside = middle;
        }
    }
    Ok(inside)
}

/// The parameter at which the exact helix edge reaches axial height `target`.
///
/// `samples` holds `(parameter, height)` pairs sorted by height and is extended
/// as the search proceeds, so the next turn starts from a tight bracket. The
/// search is a safeguarded secant iteration on the monotone height.
fn helix_parameter_at_height(
    edge: &Shape,
    target: f64,
    z_end: f64,
    samples: &mut Vec<(f64, f64)>,
) -> Result<f64, String> {
    let lower = samples
        .iter()
        .rev()
        .find(|(_, height)| *height <= target)
        .copied()
        .ok_or_else(|| "no lower helix sample".to_string())?;
    let upper = samples
        .iter()
        .find(|(_, height)| *height >= target)
        .copied()
        .ok_or_else(|| "no upper helix sample".to_string())?;
    let mut low = lower;
    let mut high = upper;
    let tolerance = 1e-9 * z_end.abs().max(1.0);
    let mut best = low.0;
    for _ in 0..60 {
        let span = high.1 - low.1;
        if span.abs() <= tolerance {
            break;
        }
        let mut parameter = low.0 + (target - low.1) * (high.0 - low.0) / span;
        if high.0 - low.0 <= 1e-12 {
            break;
        }
        parameter = parameter.clamp(low.0 + 1e-12, high.0 - 1e-12);
        let height = helix_split_height(edge, parameter, z_end)?;
        samples.push((parameter, height));
        samples.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (height - target).abs() <= tolerance {
            return Ok(parameter);
        }
        if height < target {
            low = (parameter, height);
        } else {
            high = (parameter, height);
        }
        best = parameter;
    }
    Ok(best)
}

/// `Part.makeLongHelix(pitch, height, radius, angle=-1.0, pleft=False)` → a
/// `Part.Wire`.
///
/// Upstream splits the helix into one edge per whole turn (a workaround for an
/// OCC limit on long helices) and returns the resulting wire. This builds the
/// **exact** helix edge once and splits it with the seam's `split_edge`, so the
/// pieces are exact sub-arcs of the analytic helix and their boundaries sit at
/// the same whole turns as upstream's.
///
/// The exact edge's 3D representative does not carry the surface's `u`
/// parameterisation (OCCT rebuilds it as a B-spline), so the turn boundaries are
/// located by bracketing the monotone height of the split vertex rather than by
/// assuming `u = 2πk`. If a boundary cannot be located, the wire falls back to
/// the single exact edge (`len(wire.Edges) == 1` instead of one per turn): the
/// geometry is still exact, only the split is missing.
#[pyfunction]
#[pyo3(signature = (pitch, height, radius, angle = -1.0, pleft = false))]
fn makeLongHelix(
    py: Python<'_>,
    pitch: f64,
    height: f64,
    radius: f64,
    angle: f64,
    pleft: bool,
) -> PyResult<Py<PyWire>> {
    // Upstream's `makeLongHelix` requires a positive pitch and height.
    if !pitch.is_finite() || pitch < HELIX_CONFUSION {
        return Err(OCCError::new_err("Pitch of helix too small"));
    }
    if !height.is_finite() || height < HELIX_CONFUSION {
        return Err(OCCError::new_err("Height of helix too small"));
    }
    // Upstream selects with `fabs(angle)`, but a negative angle then asks for a
    // cone with a negative semi-angle; the Part module has always treated a
    // non-positive angle as the cylinder (that is `makeHelix`'s and the previous
    // `makeLongHelix`'s convention, and what callers rely on), so it does here.
    let conical = angle >= HELIX_CONFUSION;
    if !conical && radius < HELIX_CONFUSION {
        return Err(OCCError::new_err("Radius of helix too small"));
    }

    let shape = surface_ops::make_helix_edge(
        // Upstream's `makeLongHelix` advances the cone's generator parameter by
        // one pitch per turn (`v = pitch`), i.e. the old-style segment. The
        // kernel builds the new-style cone (`v = height / cos(angle)`), so both
        // arguments are scaled by `cos(angle)` to recover `v = pitch` per turn
        // and `v = height` at the end.
        if conical {
            pitch * angle.to_radians().cos()
        } else {
            pitch
        },
        if conical {
            height * angle.to_radians().cos()
        } else {
            height
        },
        radius,
        if conical { angle } else { 0.0 },
        pleft,
    )
    .map_err(kernel_error)?;

    let turns = height / pitch;
    let whole_turns = turns.floor();
    let part_turn = turns - whole_turns;
    let z_end = if conical {
        height * angle.to_radians().cos()
    } else {
        height
    };
    let turn_height = helix_turn_height(pitch, conical, angle);

    // The interior whole-turn boundaries: turn 1 .. turn `whole_turns`, and the
    // beginning of the part turn. A boundary at the curve's end is not a split.
    let mut targets = Vec::new();
    let mut boundary = 1.0;
    while boundary < whole_turns - HELIX_CONFUSION {
        targets.push(boundary * turn_height);
        boundary += 1.0;
    }
    if part_turn > HELIX_CONFUSION && whole_turns >= 1.0 {
        // The part turn starts where the last whole turn ended.
        targets.push(whole_turns * turn_height);
    }

    let wire = if targets.is_empty() || whole_turns > 1.0e4 {
        // Nothing to split, or more turns than `Part::Helix` allows (the split
        // search would dominate the construction time): the wire then holds the
        // single exact edge.
        backend().make_wire(&[shape]).map_err(kernel_error)?
    } else {
        match split_helix_turns(&shape, &targets, z_end) {
            Ok(parameters) => backend()
                .split_edge(&shape, &parameters)
                .map_err(kernel_error)?,
            Err(_) => backend().make_wire(&[shape]).map_err(kernel_error)?,
        }
    };
    Py::new(py, PyWire::from_shape(wire))
}

/// The parameters at which the exact helix edge reaches each `target` height.
fn split_helix_turns(
    shape: &Shape,
    targets: &[f64],
    z_end: f64,
) -> Result<Vec<f64>, String> {
    let limit = helix_split_limit(shape)?;
    if !(limit > 0.0) {
        return Err("the helix edge has no split range".to_string());
    }
    let mut samples = vec![(0.0, 0.0), (limit, z_end)];
    let mut parameters = Vec::with_capacity(targets.len());
    for target in targets {
        parameters.push(helix_parameter_at_height(shape, *target, z_end, &mut samples)?);
    }
    Ok(parameters)
}

// ---------------------------------------------------------------------------
// `Part.__sortEdges__` / `Part.sortEdges`
// ---------------------------------------------------------------------------

/// The two endpoints of an edge, in the edge's own direction.
///
/// A closed edge has a single vertex in the kernel's map; both ends report it,
/// which is what upstream's `sort_Edges` would see for such an edge.
fn edge_endpoints(shape: &Shape) -> Option<(Vector3, Vector3)> {
    let mut points = Vec::new();
    for element in backend().elements(shape, ElementKind::Vertex) {
        if let Some(point) = backend().vertex_point(&element.shape) {
            points.push(point);
        }
    }
    match points.as_slice() {
        [first, second, ..] => Some((*first, *second)),
        [only] => Some((*only, *only)),
        _ => None,
    }
}

/// One edge with its endpoints, for the chain search.
struct PointedEdge {
    first: Vector3,
    second: Vector3,
    shape: Shape,
}

/// Upstream's `sort_Edges` (`AppPartPy.cpp`): chain the edges by shared endpoints.
///
/// The first edge seeds the chain; each step looks for an edge touching either end
/// (reversing it when it arrives backwards) and stops when no edge is adjacent or
/// the chain closes. Returns the chain and the edges that were not used, so the
/// cluster form can start the next chain from them.
fn sort_edge_chain(tolerance: f64, edges: Vec<Shape>) -> Result<(Vec<Shape>, Vec<Shape>), String> {
    let squared_tolerance = (tolerance * tolerance).max(0.0);
    let mut remaining: Vec<PointedEdge> = Vec::with_capacity(edges.len());
    for shape in edges {
        let (first, second) = edge_endpoints(&shape)
            .ok_or_else(|| "an edge without vertices cannot be sorted".to_string())?;
        remaining.push(PointedEdge {
            first,
            second,
            shape,
        });
    }
    if remaining.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let seed = remaining.remove(0);
    let mut first = seed.first;
    let mut last = seed.second;
    let mut sorted = std::collections::VecDeque::new();
    sorted.push_back(seed.shape);

    while !remaining.is_empty() {
        let mut found = None;
        for (index, candidate) in remaining.iter().enumerate() {
            let square_distance = |a: Vector3, b: Vector3| {
                let delta = a.sub(&b);
                delta.dot(&delta)
            };
            if square_distance(candidate.first, last) <= squared_tolerance {
                last = candidate.second;
                sorted.push_back(candidate.shape.clone());
                found = Some(index);
                break;
            } else if square_distance(candidate.second, first) <= squared_tolerance {
                first = candidate.first;
                sorted.push_front(candidate.shape.clone());
                found = Some(index);
                break;
            } else if square_distance(candidate.second, last) <= squared_tolerance {
                last = candidate.first;
                match backend().reversed(&candidate.shape) {
                    Ok(reversed) => sorted.push_back(reversed),
                    Err(_) => sorted.push_back(candidate.shape.clone()),
                }
                found = Some(index);
                break;
            } else if square_distance(candidate.first, first) <= squared_tolerance {
                first = candidate.second;
                match backend().reversed(&candidate.shape) {
                    Ok(reversed) => sorted.push_front(reversed),
                    Err(_) => sorted.push_front(candidate.shape.clone()),
                }
                found = Some(index);
                break;
            }
        }
        match found {
            Some(index) => {
                remaining.remove(index);
            }
            None => break,
        }
        if first.distance(&last) <= tolerance {
            // The chain closed: upstream stops here even if edges remain.
            break;
        }
    }

    Ok((
        sorted.into_iter().collect(),
        remaining.into_iter().map(|pointed| pointed.shape).collect(),
    ))
}

/// The shapes in an edge list, rejecting anything that is not an edge.
fn edge_argument_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    let mut edges = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        let shape = item
            .downcast::<PartShape>()
            .map_err(|_| PyTypeError::new_err("item is not a shape"))?;
        let shape = shape.borrow();
        if shape.shape_type() != "Edge" {
            return Err(PyTypeError::new_err("shape is not an edge"));
        }
        edges.push(
            shape
                .shape()
                .ok_or_else(|| OCCError::new_err("a null shape is not an edge"))?,
        );
    }
    Ok(edges)
}

fn edges_to_python(py: Python<'_>, edges: Vec<Shape>) -> PyResult<Vec<Py<PyEdge>>> {
    edges
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

/// `Part.__sortEdges__(edges)` → one chain of edges (upstream's `Module::sortEdges`).
#[pyfunction(name = "__sortEdges__")]
fn sortEdgesSingle(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<Py<PyEdge>>> {
    let edges = edge_argument_list(obj)?;
    let (sorted, _unused) = sort_edge_chain(1e-7, edges).map_err(OCCError::new_err)?;
    edges_to_python(py, sorted)
}

/// `Part.sortEdges(edges, tol3d=None)` → the connected chains (upstream's
/// `Module::sortEdges2`).
#[pyfunction]
#[pyo3(signature = (obj, tol3d = None))]
fn sortEdges(py: Python<'_>, obj: &Bound<'_, PyAny>, tol3d: Option<f64>) -> PyResult<Vec<Vec<Py<PyEdge>>>> {
    let tolerance = tol3d.unwrap_or(1e-7);
    let mut remaining = edge_argument_list(obj)?;
    let mut clusters = Vec::new();
    while !remaining.is_empty() {
        let (sorted, unused) = sort_edge_chain(tolerance, remaining).map_err(OCCError::new_err)?;
        remaining = unused;
        clusters.push(edges_to_python(py, sorted)?);
    }
    Ok(clusters)
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
    m.add_class::<PyHyperbola>()?;
    m.add_class::<PyArcOfHyperbola>()?;
    m.add_class::<PyParabola>()?;
    m.add_class::<PyArcOfParabola>()?;
    m.add_class::<PyBezierCurve>()?;
    m.add_class::<PyOffsetCurve>()?;
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
    m.add_function(wrap_pyfunction!(makeHelix, m)?)?;
    m.add_function(wrap_pyfunction!(makeLongHelix, m)?)?;
    m.add_function(wrap_pyfunction!(sortEdgesSingle, m)?)?;
    m.add_function(wrap_pyfunction!(sortEdges, m)?)?;
    m.add("OCCError", m.py().get_type::<OCCError>())?;
    // `Part.Geom2d` is a submodule, as upstream (the classes live there rather
    // than directly on `Part`).
    let geom2d = PyModule::new(m.py(), "Part.Geom2d")?;
    geom2d.add_class::<PyGeometry2d>()?;
    geom2d.add_class::<PyCurve2d>()?;
    geom2d.add_class::<PyConic2d>()?;
    geom2d.add_class::<PyArcOfConic2d>()?;
    geom2d.add_class::<PyLine2d>()?;
    geom2d.add_class::<PyLine2dSegment>()?;
    geom2d.add_class::<PyCircle2d>()?;
    geom2d.add_class::<PyArcOfCircle2d>()?;
    geom2d.add_class::<PyEllipse2d>()?;
    geom2d.add_class::<PyArcOfEllipse2d>()?;
    geom2d.add_class::<PyHyperbola2d>()?;
    geom2d.add_class::<PyArcOfHyperbola2d>()?;
    geom2d.add_class::<PyParabola2d>()?;
    geom2d.add_class::<PyArcOfParabola2d>()?;
    geom2d.add_class::<PyBezierCurve2d>()?;
    geom2d.add_class::<PyBSplineCurve2d>()?;
    geom2d.add_class::<PyOffsetCurve2d>()?;
    m.add("Geom2d", &geom2d)?;
    // Upstream offers both names for the same class.
    let arc = m.getattr("ArcOfCircle")?;
    m.add("Arc", arc)?;
    Ok(())
}
