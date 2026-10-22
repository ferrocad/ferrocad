//! The analytic half of `Part` in Python: curves, edges and wires.
//!
//! This is the first slice of the `Part` module the backlog calls the shared kernel
//! work: upstream's `Part::Geom*` classes ([`Part.LineSegment`], [`Part.Circle`],
//! [`Part.ArcOfCircle`]) plus the edge/wire containers Draft imports. The geometry is
//! [`ferrocad_curves`]; the conversion to a kernel shape is the `ferrocad_geom` seam
//! (`make_edge`/`make_wire`), implemented by OCCT.
//!
//! Deliberate limits for this slice, all reported as `Part.OCCError` rather than a
//! silent `None`: an infinite `Line` has no edge, an `Ellipse` edge needs a
//! `gp_Elips` binding the kernel crate does not expose, and face/solid constructors
//! (`makeFace`, `makeSolid`, …) are still to come.

use std::sync::Mutex;

use ferrocad_curves::{
    Arc, Circle, CurveError, EdgeCurve, Line, LineSegment, Vector3, polygon,
};
use ferrocad_geom::{ElementKind, Shape};
use ferrocad_part::backend;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyIndexError, PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule, PyTuple};
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
        EdgeCurve::Arc(arc) => Ok(Py::new(py, PyArcOfCircle { curve: arc })?.into_any()),
        EdgeCurve::Ellipse(_) => Err(OCCError::new_err(
            "Part.Ellipse is not implemented yet (the OCCT bindings expose no gp_Elips)",
        )),
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
    #[new]
    fn new(origin: &Bound<'_, PyAny>, direction: &Bound<'_, PyAny>) -> PyResult<Self> {
        let curve = Line::new(vector(origin)?, vector(direction)?).map_err(curve_error)?;
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

    /// An infinite line has no bounded edge, as upstream's `GeomLine::toShape` is
    /// documented around a parameter range.
    fn toShape(&self, _py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Err(OCCError::new_err(
            "an infinite Part.Line has no edge; use Part.LineSegment",
        ))
    }
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

    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::Circle(self.curve))?)
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

    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        Py::new(py, PyEdge::from_curve(EdgeCurve::Arc(self.curve))?)
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

    fn __repr__(&self) -> String {
        format!("Part.Plane({:?}, {:?})", self.location, self.normal)
    }
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
#[pyclass(name = "BSplineCurve", module = "Part")]
pub struct PyBSplineCurve {
    points: Mutex<Vec<Vector3>>,
    periodic: Mutex<bool>,
    /// `Some(degree)` when the curve came from control points; `None` for the
    /// interpolating form.
    degree: Mutex<Option<i32>>,
}

fn parse_points(points: &Bound<'_, PyAny>) -> PyResult<Vec<Vector3>> {
    let mut parsed = Vec::new();
    for point in points.try_iter()? {
        parsed.push(vector(&point?)?);
    }
    Ok(parsed)
}

#[pymethods]
impl PyBSplineCurve {
    /// `Part.BSplineCurve()` (empty) or `Part.BSplineCurve(poles, knots, weights,
    /// periodic, degree, knotSequence, checkRational)` (from control points).
    /// Only the poles, periodic flag and degree are used in this slice.
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        if args.len() == 0 {
            return Ok(Self {
                points: Mutex::new(Vec::new()),
                periodic: Mutex::new(false),
                degree: Mutex::new(None),
            });
        }
        let points = parse_points(&args.get_item(0)?)?;
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
        Ok(Self {
            points: Mutex::new(points),
            periodic: Mutex::new(periodic),
            degree: Mutex::new(Some(degree)),
        })
    }

    /// Interpolate a B-spline through `points`.
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
        *self.points.lock().unwrap_or_else(|e| e.into_inner()) = parsed;
        *self.periodic.lock().unwrap_or_else(|e| e.into_inner()) = PeriodicFlag;
        *self.degree.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    /// The curve as an edge.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<PyEdge>> {
        let points = self
            .points
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let periodic = *self.periodic.lock().unwrap_or_else(|e| e.into_inner());
        let degree = *self.degree.lock().unwrap_or_else(|e| e.into_inner());
        let shape = match degree {
            Some(degree) => backend()
                .make_bspline_poles(&points, degree, periodic)
                .map_err(kernel_error)?,
            None => backend()
                .make_bspline_edge(&points, periodic)
                .map_err(kernel_error)?,
        };
        Py::new(
            py,
            PyEdge::from_entry(EdgeEntry {
                shape,
                curve: None,
            }),
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "Part.BSplineCurve({} points)",
            self.points.lock().unwrap_or_else(|e| e.into_inner()).len()
        )
    }
}

// ---------------------------------------------------------------------------
// Edges and wires
// ---------------------------------------------------------------------------

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
    entry: Mutex<EdgeEntry>,
}

impl PyEdge {
    fn initializer(entry: EdgeEntry) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(entry.shape.clone()))
            .add_subclass(Self {
                entry: Mutex::new(entry),
            })
    }

    pub(crate) fn from_entry(entry: EdgeEntry) -> PyClassInitializer<Self> {
        Self::initializer(entry)
    }

    /// A copy of the entry (the shape handle is cheap to clone).
    pub(crate) fn entry(&self) -> EdgeEntry {
        self.entry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The geometry behind a Python edge/curve object.
    pub(crate) fn entry_from(value: &Bound<'_, PyAny>) -> PyResult<EdgeEntry> {
        if let Ok(edge) = value.downcast::<PyEdge>() {
            return Ok(edge.borrow().entry());
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
    #[new]
    fn new(value: &Bound<'_, PyAny>) -> PyResult<PyClassInitializer<Self>> {
        Ok(Self::initializer(Self::entry_from(value)?))
    }

    /// The analytic curve (`edge.Curve` upstream). `None` when the kernel cannot
    /// classify it.
    ///
    /// The kernel's **basis** curve wins: a segment's edge reports a `Line`, as
    /// upstream does. The stored analytic curve is the fallback for a shape the
    /// kernel cannot classify.
    #[getter]
    fn Curve(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let entry = self.entry();
        match backend().curve_of(&entry.shape) {
            Ok(curve) => Ok(Some(curve_to_py(py, curve)?)),
            Err(_) => match entry.curve {
                Some(curve) => Ok(Some(curve_to_py(py, curve)?)),
                None => Ok(None),
            },
        }
    }

    /// The edge as a kernel shape.
    fn toShape(&self) -> PartShape {
        PartShape::from_shape(self.entry().shape)
    }

    fn isNull(&self) -> bool {
        false
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

/// `Part.Face(wire)` — a planar face from a wire, a `Part.Shape`.
#[pyclass(name = "Face", module = "Part", extends = PartShape)]
pub struct PyFace;

impl PyFace {
    fn initializer(shape: Shape) -> PyClassInitializer<Self> {
        PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self)
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
        let shape = edge.borrow().entry().shape;
        let wire = backend().make_wire(&[shape]).map_err(kernel_error)?;
        return Ok(vec![wire]);
    }
    if let Ok(shape) = value.downcast::<PartShape>() {
        let inner = shape
            .borrow()
            .shape()
            .ok_or_else(|| OCCError::new_err("a null shape cannot bound a face"))?;
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
    #[pyo3(signature = (value, face_maker = "Part::FaceMakerBullseye"))]
    fn new(value: &Bound<'_, PyAny>, face_maker: &str) -> PyResult<PyClassInitializer<Self>> {
        let shapes = wire_shapes(value)?;
        let shape = if face_maker == "Part::FaceMakerUnified" {
            backend().make_faces(&shapes).map_err(kernel_error)?
        } else {
            backend().make_face(&shapes).map_err(kernel_error)?
        };
        Ok(Self::initializer(shape))
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
        backend().make_faces(&shapes).map_err(kernel_error)?
    } else {
        backend().make_face(&shapes).map_err(kernel_error)?
    };
    Py::new(py, PyFace::initializer(shape))
}

/// Register the analytic half on the `Part` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLineSegment>()?;
    m.add_class::<PyLine>()?;
    m.add_class::<PyCircle>()?;
    m.add_class::<PyArcOfCircle>()?;
    m.add_class::<PyPlane>()?;
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
    // Upstream offers both names for the same class.
    let arc = m.getattr("ArcOfCircle")?;
    m.add("Arc", arc)?;
    Ok(())
}
