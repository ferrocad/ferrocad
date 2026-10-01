//! # fc-python (M3b)
//!
//! PyO3 bindings exposing the `fc-core` document/object model (properties,
//! quantities, expressions, transactions, recompute) to Python. This is the
//! bridge that replaces the hand-written `FreeCAD` facade from M0/M1.

#![allow(non_snake_case)]

use std::sync::{Arc, Mutex};

use fc_core::{Document as CoreDocument, ObjectId, Property, Quantity, Unit};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAnyMethods;
use pyo3::IntoPyObjectExt;

// ---------------------------------------------------------------------------
// Quantity
// ---------------------------------------------------------------------------

#[pyclass(name = "Quantity", module = "fc")]
#[derive(Clone)]
struct PyQuantity {
    inner: Quantity,
}

#[pymethods]
impl PyQuantity {
    #[new]
    fn new(s: &str) -> PyResult<Self> {
        let q: Quantity = s.parse().map_err(PyValueError::new_err)?;
        Ok(Self { inner: q })
    }

    fn value_mm(&self) -> f64 {
        self.inner.value_mm()
    }

    fn value_in(&self, unit: &str) -> PyResult<f64> {
        let u = Unit::parse(unit)
            .ok_or_else(|| PyValueError::new_err(format!("unknown unit '{unit}'")))?;
        Ok(self.inner.value_in(u))
    }

    fn __repr__(&self) -> String {
        self.inner.to_string()
    }
}

// ---------------------------------------------------------------------------
// Property <-> Python conversion
// ---------------------------------------------------------------------------

fn property_to_py(py: Python<'_>, value: &Property) -> PyObject {
    match value {
        Property::String(s) => s.clone().into_py_any(py).unwrap(),
        Property::Float(f) => (*f).into_py_any(py).unwrap(),
        Property::Bool(b) => (*b).into_py_any(py).unwrap(),
        Property::Quantity(q) => PyQuantity { inner: *q }.into_py_any(py).unwrap(),
    }
}

fn py_to_property(value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<Property> {
    if let Ok(q) = value.extract::<PyRef<'_, PyQuantity>>() {
        return Ok(Property::Quantity(q.inner));
    }
    if let Ok(b) = value.extract::<bool>() {
        return Ok(Property::Bool(b));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(Property::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(Property::String(s));
    }
    Err(PyTypeError::new_err(
        "unsupported property value (expected str, float, bool, or Quantity)",
    ))
}

// ---------------------------------------------------------------------------
// Document / DocumentObject handles
// ---------------------------------------------------------------------------

#[pyclass(name = "Document", module = "fc")]
struct PyDocument {
    name: String,
    inner: Arc<Mutex<CoreDocument>>,
}

#[pyclass(name = "DocumentObject", module = "fc")]
#[derive(Clone)]
struct PyDocumentObject {
    inner: Arc<Mutex<CoreDocument>>,
    id: ObjectId,
}

#[pymethods]
impl PyDocument {
    #[new]
    #[pyo3(signature = (name="Unnamed"))]
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inner: Arc::new(Mutex::new(CoreDocument::new())),
        }
    }

    #[getter]
    fn Name(&self) -> String {
        self.name.clone()
    }

    #[pyo3(signature = (type_id, name=None))]
    fn addObject(&self, type_id: &str, name: Option<String>) -> PyDocumentObject {
        let mut doc = self.inner.lock().unwrap();
        let name = name.unwrap_or_default();
        let id = doc.add_object(&name, type_id);
        PyDocumentObject {
            inner: Arc::clone(&self.inner),
            id,
        }
    }

    fn getObject(&self, name: &str) -> Option<PyDocumentObject> {
        let doc = self.inner.lock().unwrap();
        doc.get_by_name(name).map(|id| PyDocumentObject {
            inner: Arc::clone(&self.inner),
            id,
        })
    }

    #[getter]
    fn Objects(&self) -> Vec<PyDocumentObject> {
        let doc = self.inner.lock().unwrap();
        doc.object_ids()
            .into_iter()
            .map(|id| PyDocumentObject {
                inner: Arc::clone(&self.inner),
                id,
            })
            .collect()
    }

    fn recompute(&self) -> PyResult<usize> {
        self.inner
            .lock()
            .unwrap()
            .recompute()
            .map_err(PyValueError::new_err)
    }

    // -- transactions -------------------------------------------------------
    fn openTransaction(&self) {
        self.inner.lock().unwrap().open_transaction();
    }

    fn commitTransaction(&self) {
        self.inner.lock().unwrap().commit_transaction();
    }

    fn abortTransaction(&self) {
        self.inner.lock().unwrap().abort_transaction();
    }

    fn undo(&self) -> bool {
        self.inner.lock().unwrap().undo()
    }

    fn redo(&self) -> bool {
        self.inner.lock().unwrap().redo()
    }
}

#[pymethods]
impl PyDocumentObject {
    #[getter]
    fn Name(&self) -> String {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.name.clone())
            .unwrap_or_default()
    }

    #[getter]
    fn TypeId(&self) -> String {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.type_id.clone())
            .unwrap_or_default()
    }

    #[getter]
    fn PropertiesList(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.properties.names().cloned().collect())
            .unwrap_or_default()
    }

    fn getPropertyByName(&self, name: &str) -> Option<PyObject> {
        let doc = self.inner.lock().unwrap();
        let obj = doc.object(self.id)?;
        let value = obj.properties.get(name)?;
        Some(pyo3::Python::with_gil(|py| property_to_py(py, value)))
    }

    fn setPropertyByName(&self, name: &str, value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<()> {
        let property = py_to_property(value)?;
        self.inner
            .lock()
            .unwrap()
            .set_property(self.id, name, property)
            .map_err(PyValueError::new_err)
    }

    fn setExpression(&self, prop: &str, source: &str) -> PyResult<()> {
        self.inner
            .lock()
            .unwrap()
            .set_expression(self.id, prop, source)
            .map_err(PyValueError::new_err)
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (name=None))]
fn newDocument(name: Option<String>) -> PyDocument {
    PyDocument {
        name: name.unwrap_or_else(|| "Unnamed".to_string()),
        inner: Arc::new(Mutex::new(CoreDocument::new())),
    }
}

#[pymodule]
fn fc(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyQuantity>()?;
    m.add_class::<PyDocument>()?;
    m.add_class::<PyDocumentObject>()?;
    m.add_function(wrap_pyfunction!(newDocument, m)?)?;
    Ok(())
}
