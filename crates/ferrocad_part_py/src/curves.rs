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
use ferrocad_geom::Shape;
use ferrocad_part::backend;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyModule};

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

/// Read a point from a `(x, y, z)` sequence or an object with `x`/`y`/`z`
/// (`FreeCAD.Vector`, `ferrocad.Vector`).
fn vector(value: &Bound<'_, PyAny>) -> PyResult<Vector3> {
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

    /// A wire whose base shape is built eagerly, so `Part.Wire` really is a
    /// `Part.Shape` (upstream builds the B-Rep wire on construction too).
    fn initializer(entries: Vec<EdgeEntry>) -> PyResult<PyClassInitializer<Self>> {
        let base = if entries.is_empty() {
            PartShape::new()
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
    fn Edges(&self, py: Python<'_>) -> PyResult<Vec<Py<PyEdge>>> {
        self.entries()
            .iter()
            .map(|entry| Py::new(py, PyEdge::from_entry(entry.clone())))
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
        slf.as_super().copy()
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

/// `Part.Vertex(x, y, z)` / `Part.Vertex(point)` — a vertex, a `Part.Shape`.
#[pyclass(name = "Vertex", module = "Part", extends = PartShape)]
pub struct PyVertex {
    point: Vector3,
}

impl PyVertex {
    fn initializer(point: Vector3) -> PyResult<PyClassInitializer<Self>> {
        let shape = backend().make_vertex(point).map_err(kernel_error)?;
        Ok(PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self {
            point,
        }))
    }

    /// Wrap an existing vertex shape, reading its point from the kernel.
    pub(crate) fn from_shape(shape: Shape) -> PyResult<PyClassInitializer<Self>> {
        let point = backend()
            .vertex_point(&shape)
            .unwrap_or_else(Vector3::zero);
        Ok(PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self {
            point,
        }))
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
        let point = match (x, y, z) {
            (Some(x), Some(y), Some(z)) => Vector3::new(x.extract()?, y, z),
            (Some(x), None, None) => vector(x)?,
            (None, None, None) => Vector3::zero(),
            _ => {
                return Err(PyTypeError::new_err(
                    "Part.Vertex takes a point or x, y, z",
                ));
            }
        };
        Self::initializer(point)
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
        Py::new(py, Self::initializer(self.point)?)
    }

    /// Upstream's repr: `<Part.Vertex (1, 2, 3)>`, `<Part.Vertex()>` at the origin.
    fn __repr__(&self) -> String {
        if self.point.is_null() {
            "<Part.Vertex()>".to_string()
        } else {
            format!(
                "<Part.Vertex ({}, {}, {})>",
                self.point.x, self.point.y, self.point.z
            )
        }
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
    fn new(value: &Bound<'_, PyAny>) -> PyResult<PyClassInitializer<Self>> {
        let shapes = wire_shapes(value)?;
        let shape = backend().make_face(&shapes).map_err(kernel_error)?;
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
    let _ = (face_maker, noElementMap);
    let shapes = wire_shapes(wires)?;
    let shape = backend().make_face(&shapes).map_err(kernel_error)?;
    Py::new(py, PyFace::initializer(shape))
}

/// Register the analytic half on the `Part` module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLineSegment>()?;
    m.add_class::<PyLine>()?;
    m.add_class::<PyCircle>()?;
    m.add_class::<PyArcOfCircle>()?;
    m.add_class::<PyEdge>()?;
    m.add_class::<PyWire>()?;
    m.add_class::<PyVertex>()?;
    m.add_class::<PyFace>()?;
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
