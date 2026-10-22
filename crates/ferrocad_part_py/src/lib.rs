//! # ferrocad_part_py
//!
//! The **`Part`** workbench's Python module, built on
//! [`ferrocad_part`](https://crates.io/crates/ferrocad_part) (the object/property
//! types) and [`ferrocad_occt`](https://crates.io/crates/ferrocad_occt) (the kernel).
//!
//! This crate is the **composition root** for Part: it is the one place that links a
//! concrete kernel and calls [`ferrocad_part::register`], so Part itself stays
//! kernel-agnostic and `ferrocad_core` stays geometry-free (see
//! `docs/architecture/occt-integration.md` §4).
//!
//! It exposes two things to Python:
//!
//! - `Part.makeBox(l, w, h)` → a `Part.Shape`;
//! - `Part.Shape`, the opaque wrapper the document returns for `obj.Shape`.
//!
//! The second works through the converter hook in `ferrocad_py`: a `Property::Extension`
//! is opaque to core, so the owning module registers how to move it to and from Python.
//! Here that is `Part::PropertyPartShape` ↔ `Part.Shape`.
//!
//! # How it is meant to be loaded
//!
//! An application links this as an `rlib` and registers the `Part` module as a
//! built-in, so `import Part` shares one process-wide core with `import FreeCAD`:
//!
//! ```ignore
//! use ferrocad_part_py::Part as part_module;
//! pyo3::append_to_inittab!(part_module);
//! ```
//!
//! A separate `.so` cannot share that state (each extension module would statically
//! link its own `ferrocad_core`), which is why Part must live in the same image as
//! the core bindings.

#![allow(non_snake_case)]

mod containers;
mod curves;

use std::sync::{Arc, Mutex};

use ferrocad_core::{ExtensionData, ExtensionValue};
use ferrocad_geom::{ElementKind, Shape, TessellationOptions};
use ferrocad_occt::OcctBackend;
use ferrocad_part::{ShapeProperty, TYPE_NAME};
use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyModule};

/// Register the Part kernel (OCCT) and its core types. Idempotent; every entry
/// point that can build a shape calls it first.
fn register_kernel() {
    ferrocad_part::register(Arc::new(OcctBackend::new()));
}

/// `Part.Shape`: an opaque handle to a kernel shape.
///
/// The `Mutex` is here at the Python boundary only: `Shape` is `Send` but not `Sync`
/// (see `ferrocad_geom`), and a `#[pyclass]` must be `Send + Sync`. It is not a lock
/// in the seam.
#[pyclass(name = "Shape", module = "Part", subclass)]
pub struct PartShape {
    shape: Mutex<Option<Shape>>,
}

impl PartShape {
    /// The null shape.
    pub(crate) fn empty() -> Self {
        PartShape {
            shape: Mutex::new(None),
        }
    }

    /// A second handle to the same shape (for `Shape.copy` and subclasses).
    pub(crate) fn copy_handle(&self) -> Self {
        PartShape {
            shape: Mutex::new(self.shape()),
        }
    }

    /// A copy of the wrapped shape (shape handles are cheap to clone).
    pub(crate) fn shape(&self) -> Option<Shape> {
        self.shape.lock().unwrap().clone()
    }

    /// Wrap an existing kernel shape.
    pub(crate) fn from_shape(shape: Shape) -> Self {
        PartShape {
            shape: Mutex::new(Some(shape)),
        }
    }

    /// Replace the wrapped shape (a subclass rebuilds its base shape in place).
    pub(crate) fn replace_shape(&self, shape: Option<Shape>) {
        *self.shape.lock().unwrap_or_else(|e| e.into_inner()) = shape;
    }

    /// The sub-shapes of one kind, in the kernel's order.
    fn sub_shapes(&self, kind: ElementKind) -> PyResult<Vec<Shape>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape has no sub-shapes"))?;
        Ok(ferrocad_part::backend()
            .elements(&shape, kind)
            .into_iter()
            .map(|element| element.shape)
            .collect())
    }
}

#[pymethods]
impl PartShape {
    /// `Part.Shape()` is the null shape; `Part.Shape(other)` wraps a copy of
    /// `other` (upstream's copy constructor).
    #[new]
    #[pyo3(signature = (value = None))]
    fn new(value: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        match value {
            None => Ok(Self::empty()),
            Some(value) => Ok(Self::from_shape(shape_argument(value)?)),
        }
    }

    /// A copy of the shape (`Shape.copy`); the kernel handle is reference-counted
    /// so no geometry is duplicated.
    #[pyo3(signature = (noElementMap = false))]
    fn copy(&self, noElementMap: bool) -> Self {
        let _ = noElementMap;
        self.copy_handle()
    }

    /// The shape's placement. Kernel shapes are stored with baked geometry, so
    /// this is always the identity (`Shape.transformed` bakes the matrix in).
    #[getter]
    fn Placement(&self, py: Python<'_>) -> PyResult<PyObject> {
        let module = py.import("ferrocad")?;
        Ok(module.getattr("Placement")?.call0()?.unbind())
    }

    /// Assigning a placement bakes it into the geometry (there is no separate
    /// shape location in this kernel).
    #[setter]
    fn set_Placement(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let matrix = matrix_from_py(&value.call_method0("toMatrix")?)?;
        let shape = self.named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        self.replace_shape(Some(moved));
        Ok(())
    }

    /// A copy translated by `vector` (`Shape.translated`).
    fn translated(&self, vector: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let offset = curves::vector(vector)?;
        self.transformed_by(&translation_matrix(offset))
    }

    /// A copy scaled uniformly by `factor` about `center` (`Shape.scaled`).
    #[pyo3(signature = (factor, center = None))]
    fn scaled(
        &self,
        factor: f64,
        center: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PartShape> {
        let center = center
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        self.transformed_by(&scale_matrix(factor, center))
    }

    /// Mirror this shape across the plane through `point` with `normal`, in place
    /// (`Shape.mirror`).
    fn mirror(
        slf: &Bound<'_, Self>,
        point: &Bound<'_, PyAny>,
        normal: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let point = curves::vector(point)?;
        let normal = curves::vector(normal)?;
        let matrix = mirror_matrix(point, normal)?;
        let shape = slf.borrow().named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        slf.borrow_mut().replace_shape(Some(moved));
        Ok(slf.clone().into_any().unbind())
    }

    /// The direct children (`Shape.childShapes`): the highest-level sub-shapes.
    #[pyo3(signature = (shape_type = None))]
    fn childShapes(
        &self,
        py: Python<'_>,
        shape_type: Option<&str>,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let _ = shape_type;
        for kind in [
            ElementKind::Solid,
            ElementKind::Shell,
            ElementKind::Face,
            ElementKind::Wire,
            ElementKind::Edge,
            ElementKind::Vertex,
        ] {
            let shapes = self.sub_shapes(kind)?;
            if !shapes.is_empty() {
                return shapes
                    .into_iter()
                    .map(|shape| Py::new(py, PartShape::from_shape(shape)))
                    .collect();
            }
        }
        Ok(Vec::new())
    }

    /// A transformed copy (`Shape.transformed(matrix, copy=True)`).
    #[pyo3(signature = (matrix, copy = true))]
    fn transformed(
        &self,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PartShape> {
        let _ = copy;
        let shape = self.named_shape()?;
        let matrix = matrix_from_py(matrix)?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(moved))
    }

    /// Transform this shape in place and return it (`Shape.transformShape`).
    #[pyo3(signature = (matrix, copy = true))]
    fn transformShape<'py>(
        slf: &Bound<'py, Self>,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PyObject> {
        let _ = copy;
        let shape = slf.borrow().named_shape()?;
        let matrix = matrix_from_py(matrix)?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        slf.borrow_mut().replace_shape(Some(moved));
        Ok(slf.clone().into_any().unbind())
    }

    /// A copy with redundant edges/faces unified (`Shape.cleaned`).
    fn cleaned(&self) -> Self {
        let Some(shape) = self.shape() else {
            return Self::empty();
        };
        match ferrocad_part::backend().clean(&shape) {
            Some(cleaned) => Self::from_shape(cleaned),
            None => self.copy_handle(),
        }
    }

    /// Whether this is the empty/null shape (FreeCAD's `Shape.isNull()`).
    fn isNull(&self) -> bool {
        self.shape.lock().unwrap().is_none()
    }

    /// Whether two handles are the same kernel object.
    fn isSame(&self, other: &Bound<'_, PyAny>) -> bool {
        let Ok(other) = other.downcast::<PartShape>() else {
            return false;
        };
        // Clone each shape out (releasing each lock at statement end) before
        // comparing, so `shape.isSame(shape)` cannot deadlock on one `Mutex`.
        let a = self.shape();
        let b = other.borrow().shape();
        match (a, b) {
            (Some(x), Some(y)) => x.is_same(&y),
            _ => false,
        }
    }

    /// Write the shape as BREP to `filename` (FreeCAD's `Shape.exportBrep`).
    fn exportBrep(&self, filename: &str) -> PyResult<()> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        std::fs::write(filename, bytes).map_err(|e| PyIOError::new_err(e.to_string()))
    }

    /// The BREP encoding of the shape, as `bytes`.
    fn exportBrepToString(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        Ok(PyBytes::new(py, &bytes).unbind())
    }

    /// A display mesh with per-primitive sub-element ids and names.
    ///
    /// A FerroCAD extension (upstream `Shape.tessellate` returns bare vertices and
    /// facets): the dict carries the arrays of a `ferrocad_geom::Tessellation`, so a
    /// host can draw the shape **and** map a pick back to `Face6`/`Edge3`.
    ///
    /// `deflection` is the linear deflection in model units; `angular` is the angular
    /// deflection in radians.
    #[pyo3(signature = (deflection = 0.5, angular = 0.5))]
    fn mesh(&self, py: Python<'_>, deflection: f64, angular: f64) -> PyResult<Py<PyAny>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot tessellate a null shape"))?;
        let options = TessellationOptions {
            linear_deflection: Some(deflection),
            angular_deflection: angular,
            relative: false,
        };
        let mesh = ferrocad_part::backend()
            .tessellate(&shape, &options)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

        let dict = PyDict::new(py);
        dict.set_item(
            "positions",
            PyList::new(py, mesh.positions.iter().map(|p| (p[0], p[1], p[2])))?,
        )?;
        dict.set_item("triangles", mesh.triangles)?;
        dict.set_item("triangle_parts", mesh.triangle_parts)?;
        dict.set_item("edges", mesh.edges)?;
        dict.set_item("segment_parts", mesh.segment_parts)?;
        dict.set_item("vertices", mesh.vertices)?;
        dict.set_item("vertex_parts", mesh.vertex_parts)?;
        dict.set_item("parts", mesh.parts)?;
        Ok(dict.into_any().unbind())
    }

    fn __repr__(&self) -> String {
        match self.shape.lock().unwrap().as_ref() {
            Some(shape) => format!("Part.Shape({shape:?})"),
            None => "Part.Shape()".to_string(),
        }
    }

    /// The edges (`Shape.Edges`) as `Part.Edge` wrappers.
    #[getter]
    fn Edges(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyEdge>>> {
        self.sub_shapes(ElementKind::Edge)?
            .into_iter()
            .map(|shape| {
                Py::new(
                    py,
                    curves::PyEdge::from_entry(curves::EdgeEntry {
                        shape,
                        curve: None,
                    }),
                )
            })
            .collect()
    }

    /// The wires (`Shape.Wires`) as `Part.Shape` handles.
    #[getter]
    fn Wires(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Wire)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The faces (`Shape.Faces`) as `Part.Shape` handles.
    #[getter]
    fn Faces(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Face)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The vertices (`Shape.Vertexes`) as `Part.Vertex` wrappers with `Point`.
    #[getter]
    fn Vertexes(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyVertex>>> {
        self.sub_shapes(ElementKind::Vertex)?
            .into_iter()
            .map(|shape| Py::new(py, curves::PyVertex::from_shape(shape)?))
            .collect()
    }

    /// The axis-aligned bounds as a `ferrocad.BoundBox` (`Shape.BoundBox`).
    #[getter]
    fn BoundBox(&self, py: Python<'_>) -> PyResult<PyObject> {
        let module = py.import("ferrocad")?;
        let bound_box = module.getattr("BoundBox")?;
        let Some(shape) = self.shape() else {
            return Ok(bound_box.call0()?.unbind());
        };
        match ferrocad_part::backend().bounds(&shape) {
            Some((min, max)) => Ok(bound_box
                .call1((min.x, min.y, min.z, max.x, max.y, max.z))?
                .unbind()),
            None => Ok(bound_box.call0()?.unbind()),
        }
    }

    /// The shape's length (`Shape.Length`): edge/wire length, 0 otherwise.
    #[getter]
    fn Length(&self) -> f64 {
        self.mass_properties().map(|mass| mass.length).unwrap_or(0.0)
    }

    /// The surface area (`Shape.Area`).
    #[getter]
    fn Area(&self) -> f64 {
        self.mass_properties().map(|mass| mass.area).unwrap_or(0.0)
    }

    /// The enclosed volume (`Shape.Volume`).
    #[getter]
    fn Volume(&self) -> f64 {
        self.mass_properties().map(|mass| mass.volume).unwrap_or(0.0)
    }

    /// The centre of mass (`Shape.CenterOfMass`).
    #[getter]
    fn CenterOfMass(&self, py: Python<'_>) -> PyResult<PyObject> {
        let point = self
            .mass_properties()
            .map(|mass| mass.center_of_mass)
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        curves::py_vector(py, point)
    }

    /// Upstream's alias for `CenterOfMass`.
    #[getter]
    fn CenterOfGravity(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.CenterOfMass(py)
    }

    /// Upstream's element-map version. This kernel emits no element maps, so it
    /// is empty and `ElementMapSize` is zero (upstream's own tests treat that as
    /// "skip the map assertions").
    #[getter]
    fn ElementMapVersion(&self) -> &'static str {
        ""
    }

    #[getter]
    fn ElementMapSize(&self) -> usize {
        0
    }

    #[getter]
    fn ElementReverseMap(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        Ok(PyDict::new(py).unbind())
    }

    /// `Shape.Vertex1`, `Shape.Edge2`, `Shape.Face3`, … — the kernel's 1-based
    /// sub-shape lists, as upstream's `TopoShape` shortcuts.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let Some((kind, index)) = sub_shape_name(name) else {
            return Err(pyo3::exceptions::PyAttributeError::new_err(format!(
                "'Part.Shape' object has no attribute '{name}'"
            )));
        };
        let shapes = self.sub_shapes(kind)?;
        let shape = shapes.get(index).ok_or_else(|| {
            pyo3::exceptions::PyAttributeError::new_err(format!(
                "'Part.Shape' object has no attribute '{name}'"
            ))
        })?
        .clone();
        match kind {
            ElementKind::Vertex => Ok(Py::new(py, curves::PyVertex::from_shape(shape)?)?.into_any()),
            ElementKind::Edge => Ok(Py::new(
                py,
                curves::PyEdge::from_entry(curves::EdgeEntry {
                    shape,
                    curve: None,
                }),
            )?
            .into_any()),
            _ => Ok(Py::new(py, PartShape::from_shape(shape))?.into_any()),
        }
    }

    /// `Shape.fuse(other)` — the union of the two shapes.
    ///
    /// `glue`, `tolerance` and `refine` are accepted for upstream signature
    /// compatibility; this slice runs the plain boolean and produces no element
    /// map, so `noElementMap` changes nothing.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn fuse(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Fuse)
    }

    /// `Shape.cut(other)` — `self` with `other` removed.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn cut(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Cut)
    }

    /// `Shape.common(other)` — the intersection of the two shapes.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn common(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Common)
    }

    /// `Shape.multiFuse(shapes)` — fuse this shape with each shape in `shapes`.
    #[pyo3(signature = (shapes, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn multiFuse(
        &self,
        shapes: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        let mut shape = self.named_shape()?;
        for item in shapes.try_iter()? {
            let other = shape_argument(&item?)?;
            shape = ferrocad_part::backend()
                .fuse(&shape, &other)
                .map_err(kernel_error)?
                .shape;
        }
        Ok(PartShape::from_shape(shape))
    }
}

/// Split `Face3` into `(ElementKind::Face, 2)`.
fn sub_shape_name(name: &str) -> Option<(ElementKind, usize)> {
    let (prefix, digits) = ["CompSolid", "Solid", "Shell", "Face", "Wire", "Edge", "Vertex"]
        .into_iter()
        .find_map(|prefix| name.strip_prefix(prefix).map(|digits| (prefix, digits)))?;
    let index: usize = digits.parse().ok()?;
    if index == 0 {
        return None;
    }
    let kind = match prefix {
        "Solid" => ElementKind::Solid,
        "Shell" => ElementKind::Shell,
        "Face" => ElementKind::Face,
        "Wire" => ElementKind::Wire,
        "Edge" => ElementKind::Edge,
        "Vertex" => ElementKind::Vertex,
        // The seam has no comp-solid kind yet.
        _ => return None,
    };
    Some((kind, index - 1))
}

/// Map a kernel failure to `Part.OCCError`.
fn kernel_error(error: ferrocad_geom::GeomError) -> PyErr {
    curves::OCCError::new_err(error.to_string())
}

fn translation_matrix(offset: ferrocad_curves::Vector3) -> ferrocad_types::Matrix4 {
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[3] = offset.x;
    values[7] = offset.y;
    values[11] = offset.z;
    ferrocad_types::Matrix4::from_values(values)
}

fn scale_matrix(factor: f64, center: ferrocad_curves::Vector3) -> ferrocad_types::Matrix4 {
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[0] = factor;
    values[5] = factor;
    values[10] = factor;
    values[3] = center.x * (1.0 - factor);
    values[7] = center.y * (1.0 - factor);
    values[11] = center.z * (1.0 - factor);
    ferrocad_types::Matrix4::from_values(values)
}

fn mirror_matrix(
    point: ferrocad_curves::Vector3,
    normal: ferrocad_curves::Vector3,
) -> PyResult<ferrocad_types::Matrix4> {
    if normal.length() == 0.0 {
        return Err(curves::OCCError::new_err(
            "a mirror plane needs a non-zero normal",
        ));
    }
    let n = normal.normalize();
    let d = n.x * point.x + n.y * point.y + n.z * point.z;
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[0] = 1.0 - 2.0 * n.x * n.x;
    values[1] = -2.0 * n.x * n.y;
    values[2] = -2.0 * n.x * n.z;
    values[3] = 2.0 * d * n.x;
    values[4] = values[1];
    values[5] = 1.0 - 2.0 * n.y * n.y;
    values[6] = -2.0 * n.y * n.z;
    values[7] = 2.0 * d * n.y;
    values[8] = values[2];
    values[9] = values[6];
    values[10] = 1.0 - 2.0 * n.z * n.z;
    values[11] = 2.0 * d * n.z;
    Ok(ferrocad_types::Matrix4::from_values(values))
}

/// Read a `ferrocad.Matrix` through its `A11`…`A44` elements.
fn matrix_from_py(value: &Bound<'_, PyAny>) -> PyResult<ferrocad_types::Matrix4> {
    let mut values = [0.0; 16];
    for row in 1..=4 {
        for column in 1..=4 {
            let name = format!("A{row}{column}");
            values[(row - 1) * 4 + (column - 1)] = value.getattr(name)?.extract()?;
        }
    }
    Ok(ferrocad_types::Matrix4::from_values(values))
}

/// The shape wrapped by a Python `Part.Shape` (or one of its subclasses).
fn shape_argument(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let shape = value.downcast::<PartShape>().map_err(|_| {
        pyo3::exceptions::PyTypeError::new_err("expected a Part.Shape")
    })?;
    shape
        .borrow()
        .shape()
        .ok_or_else(|| curves::OCCError::new_err("a null shape cannot be combined"))
}

enum BooleanOp {
    Fuse,
    Cut,
    Common,
}

impl PartShape {
    fn transformed_by(&self, matrix: &ferrocad_types::Matrix4) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, matrix)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(moved))
    }

    fn mass_properties(&self) -> Option<ferrocad_geom::MassProperties> {
        let shape = self.shape()?;
        ferrocad_part::backend().mass_properties(&shape)
    }

    fn named_shape(&self) -> PyResult<Shape> {
        self.shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape cannot be combined"))
    }

    fn boolean(&self, other: &Bound<'_, PyAny>, operation: BooleanOp) -> PyResult<PartShape> {
        let a = self.named_shape()?;
        let b = shape_argument(other)?;
        let backend = ferrocad_part::backend();
        let result = match operation {
            BooleanOp::Fuse => backend.fuse(&a, &b),
            BooleanOp::Cut => backend.cut(&a, &b),
            BooleanOp::Common => backend.common(&a, &b),
        };
        Ok(PartShape::from_shape(result.map_err(kernel_error)?.shape))
    }
}

/// `Part.makeCylinder(radius, height, pnt=None, dir=None, angle=360)`.
#[pyfunction]
#[pyo3(signature = (radius, height, pnt = None, dir = None, angle = 360.0))]
fn makeCylinder(
    radius: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle: f64,
) -> PyResult<PartShape> {
    require_full_revolution("makeCylinder", angle)?;
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
    let shape = ferrocad_part::backend()
        .make_cylinder(radius, height, base, direction)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeSphere(radius, center=None, dir=None, angle1=-90, angle2=90, angle3=360)`.
#[pyfunction]
#[pyo3(signature = (radius, center = None, dir = None, angle1 = -90.0, angle2 = 90.0, angle3 = 360.0))]
fn makeSphere(
    radius: f64,
    center: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle1: f64,
    angle2: f64,
    angle3: f64,
) -> PyResult<PartShape> {
    let _ = dir;
    if (angle1 + 90.0).abs() > 1e-9 || (angle2 - 90.0).abs() > 1e-9 {
        return Err(curves::OCCError::new_err(
            "partial spheres are not implemented yet",
        ));
    }
    require_full_revolution("makeSphere", angle3)?;
    let center = center
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let shape = ferrocad_part::backend()
        .make_sphere(radius, center)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeCone(radius1, radius2, height, pnt=None, dir=None, angle=360)`.
#[pyfunction]
#[pyo3(signature = (radius1, radius2, height, pnt = None, dir = None, angle = 360.0))]
fn makeCone(
    radius1: f64,
    radius2: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle: f64,
) -> PyResult<PartShape> {
    require_full_revolution("makeCone", angle)?;
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
    let shape = ferrocad_part::backend()
        .make_cone(radius1, radius2, height, base, direction)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

fn require_full_revolution(operation: &str, angle: f64) -> PyResult<()> {
    if (angle - 360.0).abs() > 1e-9 {
        return Err(curves::OCCError::new_err(format!(
            "{operation}: partial revolutions are not implemented yet"
        )));
    }
    Ok(())
}

/// `Part.makeBox(length, width, height, pnt=None, dir=None)` → a `Part.Shape`.
#[pyfunction]
#[pyo3(signature = (length, width, height, pnt = None, dir = None))]
fn makeBox(
    length: f64,
    width: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
) -> PyResult<PartShape> {
    let mut shape = ferrocad_part::make_box(length, width, height)
        .ok_or_else(|| PyRuntimeError::new_err("Part.makeBox did not produce a shape"))?;
    if pnt.is_some() || dir.is_some() {
        let base = pnt
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        let rotation = match dir.map(curves::vector).transpose()? {
            Some(direction) if direction.length() > 0.0 => {
                ferrocad_types::Rotation::from_vectors(
                    ferrocad_curves::Vector3::new(0.0, 0.0, 1.0),
                    direction,
                )
            }
            _ => ferrocad_types::Rotation::identity(),
        };
        shape = ferrocad_part::backend()
            .place(&shape, &ferrocad_types::Placement { base, rotation })
            .map_err(kernel_error)?;
    }
    Ok(PartShape::from_shape(shape))
}

/// `Part::PropertyPartShape` → `Part.Shape` (what `obj.Shape` returns).
fn shape_to_py(py: Python<'_>, data: &dyn ExtensionData) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<ShapeProperty>()?;
    let shape = property.shape();
    Py::new(py, PartShape { shape: Mutex::new(shape) })
        .ok()
        .map(Py::into_any)
}

/// `Part.Shape` → `Part::PropertyPartShape` (what `obj.Shape = shape` stores).
fn shape_to_extension(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let shape = value.downcast::<PartShape>().ok()?;
    let inner = shape.borrow().shape();
    let property = match inner {
        Some(shape) => ShapeProperty::new(shape),
        None => ShapeProperty::empty(),
    };
    Some(ExtensionValue::new(Box::new(property)))
}

#[pymodule]
pub fn Part(m: &Bound<'_, PyModule>) -> PyResult<()> {
    register_kernel();
    ferrocad_py::register_extension_converter(TYPE_NAME, shape_to_py, shape_to_extension);
    m.add_class::<PartShape>()?;
    m.add_function(wrap_pyfunction!(makeBox, m)?)?;
    m.add_function(wrap_pyfunction!(makeCylinder, m)?)?;
    m.add_function(wrap_pyfunction!(makeSphere, m)?)?;
    m.add_function(wrap_pyfunction!(makeCone, m)?)?;
    curves::register(m)?;
    containers::register(m)?;
    Ok(())
}
