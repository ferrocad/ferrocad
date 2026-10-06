//! # ferrocad_part_py
//!
//! The **`Part`** workbench's Python module, built on
//! [`ferrocad_part`](https://crates.io/crates/ferrocad_part) (the object/property
//! types) and [`ferrocad_occt`](https://crates.io/crates/ferrocad_occt) (the kernel).
//!
//! This crate is the **composition root** for Part: it is the one place that links a
//! concrete kernel and calls [`ferrocad_part::register`], so Part itself stays
//! kernel-agnostic and `ferrocad_core` stays geometry-free (see
//! `docs/occt-integration.md` §4).
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

use std::sync::{Arc, Mutex};

use ferrocad_core::{ExtensionData, ExtensionValue};
use ferrocad_geom::Shape;
use ferrocad_occt::OcctBackend;
use ferrocad_part::{ShapeProperty, TYPE_NAME};
use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyModule};

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
#[pyclass(name = "Shape", module = "Part")]
struct PartShape {
    shape: Mutex<Option<Shape>>,
}

impl PartShape {
    /// A copy of the wrapped shape (shape handles are cheap to clone).
    fn shape(&self) -> Option<Shape> {
        self.shape.lock().unwrap().clone()
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

    fn __repr__(&self) -> String {
        match self.shape.lock().unwrap().as_ref() {
            Some(shape) => format!("Part.Shape({shape:?})"),
            None => "Part.Shape()".to_string(),
        }
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
    Ok(())
}
