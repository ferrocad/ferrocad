//! # fc-python (M3b)
//!
//! PyO3 bindings exposing the `fc-core` document/object model (properties,
//! quantities, expressions, transactions, recompute) to Python. This is the
//! bridge that replaces the hand-written `FreeCAD` facade from M0/M1.

#![allow(non_snake_case)]

use std::sync::{Arc, Mutex};

use fc_core::{Document as CoreDocument, ObjectId, Property, Quantity, StringHasher, StringId, Unit};
use pyo3::exceptions::{PyAttributeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods};
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

fn py_to_property(value: &Bound<'_, PyAny>) -> PyResult<Property> {
    if let Ok(q) = value.downcast::<PyQuantity>() {
        return Ok(Property::Quantity(q.borrow().inner));
    }
    if let Ok(b) = value.extract::<bool>() {
        return Ok(Property::Bool(b));
    }
    // `int` is distinct from `float` in PyO3; map it to Float.
    if let Ok(i) = value.extract::<i64>() {
        return Ok(Property::Float(i as f64));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(Property::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(Property::String(s));
    }
    Err(PyTypeError::new_err(
        "unsupported property value (expected str, int, float, bool, or Quantity)",
    ))
}

/// Map a FreeCAD property type id to its default value.
fn default_property(type_id: &str) -> Property {
    let t = type_id.to_ascii_lowercase();
    if t.ends_with("float") {
        Property::Float(0.0)
    } else if t.ends_with("bool") {
        Property::Bool(false)
    } else if t.ends_with("length") || t.ends_with("distance") || t.ends_with("quantity") {
        Property::Quantity(Quantity::new(0.0, Unit::Millimeter))
    } else {
        Property::String(String::new())
    }
}

// ---------------------------------------------------------------------------
// Document / DocumentObject
// ---------------------------------------------------------------------------

#[pyclass(name = "Document", module = "fc")]
struct PyDocument {
    name: String,
    label: String,
    inner: Arc<Mutex<CoreDocument>>,
}

#[pyclass(name = "DocumentObject", module = "fc")]
struct PyDocumentObject {
    doc: Py<PyDocument>,
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
            label: name.to_string(),
            inner: Arc::new(Mutex::new(CoreDocument::new())),
        }
    }

    #[getter]
    fn Name(&self) -> String {
        self.name.clone()
    }

    #[getter]
    fn Label(&self) -> String {
        self.label.clone()
    }

    #[setter]
    fn set_Label(&mut self, label: String) {
        self.label = label;
    }

    #[pyo3(signature = (type_id, name=None))]
    fn addObject(slf: Bound<'_, Self>, type_id: &str, name: Option<String>) -> PyDocumentObject {
        let inner = Arc::clone(&slf.borrow().inner);
        let name = name.unwrap_or_default();
        let id = inner.lock().unwrap().add_object(&name, type_id);
        PyDocumentObject {
            doc: slf.unbind(),
            inner,
            id,
        }
    }

    fn getObject(slf: Bound<'_, Self>, name: &str) -> Option<PyDocumentObject> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc: Py<PyDocument> = slf.unbind();
        let id = inner.lock().unwrap().get_by_name(name)?;
        Some(PyDocumentObject {
            doc: doc.clone_ref(py),
            inner,
            id,
        })
    }

    #[getter]
    fn Objects(slf: Bound<'_, Self>) -> Vec<PyDocumentObject> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc: Py<PyDocument> = slf.unbind();
        let ids = inner.lock().unwrap().object_ids();
        ids.into_iter()
            .map(|id| PyDocumentObject {
                doc: doc.clone_ref(py),
                inner: Arc::clone(&inner),
                id,
            })
            .collect()
    }

    #[getter]
    fn CountObjects(&self) -> usize {
        self.inner.lock().unwrap().object_ids().len()
    }

    fn removeObject(&self, name: &str) -> PyResult<()> {
        let mut doc = self.inner.lock().unwrap();
        match doc.get_by_name(name) {
            Some(id) => {
                doc.remove_object(id);
                Ok(())
            }
            None => Err(PyValueError::new_err(format!(
                "no object named '{name}' in document '{}'",
                self.name
            ))),
        }
    }

    fn recompute(&self) -> PyResult<usize> {
        self.inner
            .lock()
            .unwrap()
            .recompute()
            .map_err(PyValueError::new_err)
    }

    // -- transactions -------------------------------------------------------
    #[pyo3(signature = (name = ""))]
    fn openTransaction(&self, name: &str) {
        // The name is a label for undo/redo; the POC does not track it yet.
        let _ = name;
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
    fn Label(&self) -> String {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.label.clone())
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
    fn Document(&self, py: Python<'_>) -> Py<PyDocument> {
        self.doc.clone_ref(py)
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
        let value = {
            let doc = self.inner.lock().unwrap();
            doc.object(self.id)
                .and_then(|o| o.properties.get(name))
                .cloned()
        };
        value.map(|p| pyo3::Python::with_gil(|py| property_to_py(py, &p)))
    }

    fn setPropertyByName(&self, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let property = py_to_property(value)?;
        self.inner
            .lock()
            .unwrap()
            .set_property(self.id, name, property)
            .map_err(PyValueError::new_err)
    }

    fn getTypeIdOfProperty(&self, name: &str) -> Option<String> {
        let doc = self.inner.lock().unwrap();
        doc.object(self.id)
            .and_then(|o| o.properties.get(name))
            .map(|p| p.type_name().to_string())
    }

    #[pyo3(signature = (type_id, name, group="", doc=""))]
    fn addProperty(&self, type_id: &str, name: &str, group: &str, doc: &str) -> PyResult<()> {
        let _ = (group, doc);
        if name.is_empty() {
            return Err(PyValueError::new_err("property name must not be empty"));
        }
        let default = default_property(type_id);
        self.inner
            .lock()
            .unwrap()
            .set_property(self.id, name, default)
            .map_err(PyValueError::new_err)
    }

    fn setExpression(&self, prop: &str, source: &str) -> PyResult<()> {
        self.inner
            .lock()
            .unwrap()
            .set_expression(self.id, prop, source)
            .map_err(PyValueError::new_err)
    }

    fn __getattr__(&self, name: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let name: String = name.extract()?;
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(name));
        }
        let value = {
            let doc = self.inner.lock().unwrap();
            doc.object(self.id)
                .and_then(|o| o.properties.get(&name))
                .cloned()
        };
        match value {
            Some(p) => Ok(pyo3::Python::with_gil(|py| property_to_py(py, &p))),
            None => Err(PyAttributeError::new_err(format!(
                "'{}' object has no attribute '{name}'",
                self.TypeId()
            ))),
        }
    }

    fn __setattr__(&self, name: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let name: String = name.extract()?;
        if name == "Label" {
            let label: String = value
                .extract()
                .map_err(|_| PyTypeError::new_err("Label must be a string"))?;
            self.inner.lock().unwrap().set_label(self.id, &label);
            return Ok(());
        }
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(name));
        }
        let property = py_to_property(value)?;
        let exists = {
            let doc = self.inner.lock().unwrap();
            doc.object(self.id)
                .map(|o| o.properties.get(&name).is_some())
                .unwrap_or(false)
        };
        if !exists {
            return Err(PyAttributeError::new_err(format!(
                "'{}' object has no attribute '{name}'",
                self.TypeId()
            )));
        }
        self.inner
            .lock()
            .unwrap()
            .set_property(self.id, &name, property)
            .map_err(PyValueError::new_err)
    }
}

// ---------------------------------------------------------------------------
// StringHasher / StringID
// ---------------------------------------------------------------------------

#[pyclass(name = "StringHasher", module = "fc")]
#[derive(Clone)]
struct PyStringHasher {
    inner: StringHasher,
}

#[pymethods]
impl PyStringHasher {
    #[new]
    fn new() -> Self {
        Self {
            inner: StringHasher::new(),
        }
    }

    fn getID(&self, arg: &Bound<'_, PyAny>) -> PyResult<PyStringID> {
        if let Ok(s) = arg.extract::<String>() {
            return Ok(PyStringID {
                inner: self.inner.get_id(&s),
            });
        }
        if let Ok(i) = arg.extract::<usize>() {
            return self
                .inner
                .find_id(i)
                .map(|id| PyStringID { inner: id })
                .ok_or_else(|| PyValueError::new_err(format!("no StringID with value {i}")));
        }
        Err(PyValueError::new_err("getID expects a string or an integer"))
    }

    fn isSame(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        match other.extract::<PyRef<'_, PyStringHasher>>() {
            Ok(o) => Ok(self.inner.is_same(&o.inner)),
            Err(_) => Err(PyTypeError::new_err("isSame expects a StringHasher")),
        }
    }
}

#[pyclass(name = "StringID", module = "fc")]
#[derive(Clone)]
struct PyStringID {
    inner: StringId,
}

#[pymethods]
impl PyStringID {
    #[getter]
    fn Value(&self) -> usize {
        self.inner.value()
    }

    #[getter]
    fn Data(&self) -> String {
        self.inner.data()
    }

    fn isSame(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        match other.extract::<PyRef<'_, PyStringID>>() {
            Ok(o) => Ok(self.inner.is_same(&o.inner)),
            Err(_) => Err(PyTypeError::new_err("isSame expects a StringID")),
        }
    }

    fn __repr__(&self) -> String {
        format!("<StringID {}>", self.inner.value())
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (name=None))]
fn newDocument(name: Option<String>) -> PyDocument {
    let name = name.unwrap_or_else(|| "Unnamed".to_string());
    PyDocument {
        label: name.clone(),
        name,
        inner: Arc::new(Mutex::new(CoreDocument::new())),
    }
}

#[pymodule]
fn fc(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PyQuantity>()?;
    m.add_class::<PyDocument>()?;
    m.add_class::<PyDocumentObject>()?;
    m.add_class::<PyStringHasher>()?;
    m.add_class::<PyStringID>()?;
    m.add_function(wrap_pyfunction!(newDocument, m)?)?;
    Ok(())
}
