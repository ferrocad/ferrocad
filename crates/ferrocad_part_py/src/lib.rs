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

    /// A second handle to the same shape (for a subclass's `toShape`).
    pub(crate) fn copy(&self) -> Self {
        PartShape {
            shape: Mutex::new(self.shape()),
        }
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
    #[new]
    fn new() -> Self {
        PartShape { shape: Mutex::new(None) }
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

/// `Part.makeBox(length, width, height)` → a `Part.Shape`.
#[pyfunction]
fn makeBox(length: f64, width: f64, height: f64) -> PyResult<PartShape> {
    match ferrocad_part::make_box(length, width, height) {
        Some(shape) => Ok(PartShape { shape: Mutex::new(Some(shape)) }),
        None => Err(PyRuntimeError::new_err("Part.makeBox did not produce a shape")),
    }
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
    curves::register(m)?;
    containers::register(m)?;
    Ok(())
}
