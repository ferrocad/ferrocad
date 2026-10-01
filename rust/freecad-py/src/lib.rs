//! # freecad-py (M1)
//!
//! PyO3 bindings that expose the Rust FreeCAD object model as a native Python
//! extension module, importable as `FreeCAD._core`.
//!
//! This replaces the `ctypes` bridge from M0. The Python-visible API is
//! unchanged (`__init__.py` re-exports these), but the objects are now real
//! `#[pyclass]` values rather than integer handles crossed through a C ABI.
//!
//! ## Shared-state model
//!
//! A `Document` and its `DocumentObject`s share one `Arc<Mutex<Model>>`. An
//! object refers back to its document by id; `document_by_id` resolves that to
//! the *same* cached `Py<Document>`, so `obj.Document is doc` holds.

// The Python-facing API deliberately uses FreeCAD's camelCase names.
#![allow(non_snake_case)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pyo3::exceptions::{PyAttributeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAny;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Property {
    type_id: String,
    value: String,
}

struct ObjData {
    type_id: String,
    name: String,
    label: String,
    props: BTreeMap<String, Property>,
}

struct Model {
    name: String,
    label: String,
    objects: Vec<ObjData>,
}

struct Registry {
    /// (document id, document name, the Python `Document` object)
    docs: Vec<(usize, String, Py<Document>)>,
    active: Option<usize>,
    next_id: usize,
}

static REG: Mutex<Registry> = Mutex::new(Registry {
    docs: Vec::new(),
    active: None,
    next_id: 1,
});

fn document_by_id(py: Python<'_>, id: usize) -> Option<Py<Document>> {
    let reg = REG.lock().unwrap();
    reg.docs
        .iter()
        .find(|(doc_id, _, _)| *doc_id == id)
        .map(|(_, _, doc)| doc.clone_ref(py))
}

fn as_string(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(s) = value.extract::<String>() {
        return Ok(s);
    }
    Ok(value.str()?.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// DocumentObject
// ---------------------------------------------------------------------------

#[pyclass(module = "FreeCAD")]
pub struct DocumentObject {
    model: Arc<Mutex<Model>>,
    name: String,
    doc_id: usize,
}

impl DocumentObject {
    fn with_obj<R>(&self, f: impl FnOnce(&ObjData) -> R) -> Option<R> {
        let model = self.model.lock().unwrap();
        model.objects.iter().find(|o| o.name == self.name).map(f)
    }

    fn with_obj_mut<R>(&self, f: impl FnOnce(&mut ObjData) -> R) -> Option<R> {
        let mut model = self.model.lock().unwrap();
        model.objects.iter_mut().find(|o| o.name == self.name).map(f)
    }

    fn type_id_str(&self) -> String {
        self.with_obj(|o| o.type_id.clone()).unwrap_or_default()
    }
}

#[pymethods]
impl DocumentObject {
    #[getter]
    fn Name(&self) -> String {
        self.name.clone()
    }

    #[getter]
    fn Label(&self) -> Option<String> {
        self.with_obj(|o| o.label.clone())
    }

    #[setter]
    fn set_Label(&self, value: String) {
        self.with_obj_mut(|o| o.label = value);
    }

    #[getter]
    fn TypeId(&self) -> Option<String> {
        self.with_obj(|o| o.type_id.clone())
    }

    #[getter]
    fn Document(&self, py: Python<'_>) -> Option<Py<Document>> {
        document_by_id(py, self.doc_id)
    }

    #[pyo3(signature = (type_id, name, group="", doc="", attr=0))]
    fn addProperty(
        &self,
        type_id: String,
        name: String,
        group: &str,
        doc: &str,
        attr: u8,
    ) -> PyResult<()> {
        let _ = (group, doc, attr);
        if name.is_empty() {
            return Err(PyValueError::new_err("property name must not be empty"));
        }
        let moved = name.clone();
        let type_id_moved = type_id;
        if self
            .with_obj_mut(move |o| {
                o.props.entry(moved).or_insert(Property {
                    type_id: type_id_moved,
                    value: String::new(),
                });
            })
            .is_none()
        {
            return Err(PyValueError::new_err("object no longer exists"));
        }
        Ok(())
    }

    fn getPropertyByName(&self, name: String) -> PyResult<String> {
        match self.with_obj(|o| o.props.get(&name).map(|p| p.value.clone())) {
            Some(Some(value)) => Ok(value),
            _ => Err(PyAttributeError::new_err(format!(
                "'{}' has no property '{}'",
                self.type_id_str(),
                name
            ))),
        }
    }

    fn setPropertyByName(&self, name: String, value: String) -> PyResult<()> {
        match self.with_obj_mut(|o| {
            o.props.get_mut(&name).map(|p| {
                p.value = value;
            })
        }) {
            Some(Some(())) => Ok(()),
            _ => Err(PyAttributeError::new_err(format!(
                "'{}' has no property '{}'",
                self.type_id_str(),
                name
            ))),
        }
    }

    fn getTypeIdOfProperty(&self, name: String) -> Option<String> {
        self.with_obj(|o| o.props.get(&name).map(|p| p.type_id.clone()))
            .flatten()
    }

    #[getter]
    fn PropertiesList(&self) -> Vec<String> {
        self.with_obj(|o| o.props.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn recompute(&self) -> bool {
        true
    }

    fn __getattr__(&self, name: &Bound<'_, PyAny>) -> PyResult<String> {
        let name: String = name.extract()?;
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(name));
        }
        match self.with_obj(|o| o.props.get(&name).map(|p| p.value.clone())) {
            Some(Some(value)) => Ok(value),
            _ => Err(PyAttributeError::new_err(format!(
                "'{}' object has no attribute '{}'",
                self.type_id_str(),
                name
            ))),
        }
    }

    fn __setattr__(&self, name: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let name: String = name.extract()?;
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(format!(
                "cannot set '{}' on a DocumentObject",
                name
            )));
        }
        let value = as_string(value)?;

        if name == "Label" {
            return match self.with_obj_mut(|o| o.label = value) {
                Some(()) => Ok(()),
                None => Err(PyValueError::new_err("object no longer exists")),
            };
        }

        match self.with_obj_mut(|o| {
            o.props.get_mut(&name).map(|p| {
                p.value = value;
            })
        }) {
            Some(Some(())) => Ok(()),
            _ => Err(PyAttributeError::new_err(format!(
                "'{}' object has no attribute '{}'",
                self.type_id_str(),
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("<{} object '{}'>", self.type_id_str(), self.name)
    }
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[pyclass(module = "FreeCAD")]
pub struct Document {
    model: Arc<Mutex<Model>>,
    id: usize,
}

#[pymethods]
impl Document {
    #[getter]
    fn Name(&self) -> String {
        self.model.lock().unwrap().name.clone()
    }

    #[getter]
    fn Label(&self) -> String {
        self.model.lock().unwrap().label.clone()
    }

    #[setter]
    fn set_Label(&self, value: String) {
        self.model.lock().unwrap().label = value;
    }

    #[getter]
    fn CountObjects(&self) -> usize {
        self.model.lock().unwrap().objects.len()
    }

    #[getter]
    fn Objects(&self, py: Python<'_>) -> PyResult<Vec<Py<DocumentObject>>> {
        let names: Vec<String> = {
            let model = self.model.lock().unwrap();
            model.objects.iter().map(|o| o.name.clone()).collect()
        };
        names
            .into_iter()
            .map(|name| {
                Py::new(
                    py,
                    DocumentObject {
                        model: Arc::clone(&self.model),
                        name,
                        doc_id: self.id,
                    },
                )
            })
            .collect()
    }

    #[pyo3(signature = (type_id, name=None))]
    fn addObject(
        &self,
        py: Python<'_>,
        type_id: String,
        name: Option<String>,
    ) -> PyResult<Py<DocumentObject>> {
        let mut obj_name = name.unwrap_or_default();
        {
            let mut model = self.model.lock().unwrap();
            if obj_name.is_empty() {
                let base = type_id
                    .rsplit("::")
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Object")
                    .to_string();
                obj_name = base.clone();
                let mut i = 1;
                while model.objects.iter().any(|o| o.name == obj_name) {
                    obj_name = format!("{}{:03}", base, i);
                    i += 1;
                }
            }
            model.objects.push(ObjData {
                type_id,
                label: obj_name.clone(),
                name: obj_name.clone(),
                props: BTreeMap::new(),
            });
        }

        Py::new(
            py,
            DocumentObject {
                model: Arc::clone(&self.model),
                name: obj_name,
                doc_id: self.id,
            },
        )
    }

    fn getObject(&self, py: Python<'_>, name: String) -> PyResult<Option<Py<DocumentObject>>> {
        let exists = self.model.lock().unwrap().objects.iter().any(|o| o.name == name);
        if !exists {
            return Ok(None);
        }
        Ok(Some(Py::new(
            py,
            DocumentObject {
                model: Arc::clone(&self.model),
                name,
                doc_id: self.id,
            },
        )?))
    }

    fn removeObject(&self, name: String) -> PyResult<()> {
        let mut model = self.model.lock().unwrap();
        let before = model.objects.len();
        model.objects.retain(|o| o.name != name);
        if model.objects.len() == before {
            Err(PyValueError::new_err(format!(
                "no object named '{}' in document '{}'",
                name, model.name
            )))
        } else {
            Ok(())
        }
    }

    fn recompute(&self) -> bool {
        // POC: the real engine would run the dependency graph.
        true
    }

    fn __repr__(&self) -> String {
        format!("<Document object '{}'>", self.model.lock().unwrap().name)
    }
}

// ---------------------------------------------------------------------------
// Module-level API
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (name=None))]
fn newDocument(py: Python<'_>, name: Option<String>) -> PyResult<Py<Document>> {
    let mut reg = REG.lock().unwrap();

    let mut doc_name = name.unwrap_or_default();
    if doc_name.is_empty() {
        doc_name = "Unnamed".to_string();
    }
    if reg.docs.iter().any(|(_, n, _)| *n == doc_name) {
        let base = doc_name.clone();
        let mut i = 1;
        loop {
            let candidate = format!("{}{:03}", base, i);
            if !reg.docs.iter().any(|(_, n, _)| *n == candidate) {
                doc_name = candidate;
                break;
            }
            i += 1;
        }
    }

    let id = reg.next_id;
    reg.next_id += 1;

    let doc = Py::new(
        py,
        Document {
            model: Arc::new(Mutex::new(Model {
                label: doc_name.clone(),
                name: doc_name.clone(),
                objects: Vec::new(),
            })),
            id,
        },
    )?;
    reg.docs.push((id, doc_name, doc.clone_ref(py)));
    reg.active = Some(id);
    Ok(doc)
}

#[pyfunction]
fn closeDocument(name: String) -> PyResult<()> {
    let mut reg = REG.lock().unwrap();
    match reg.docs.iter().position(|(_, n, _)| *n == name) {
        Some(pos) => {
            let (id, _, _) = reg.docs.remove(pos);
            if reg.active == Some(id) {
                reg.active = None;
            }
            Ok(())
        }
        None => Err(PyValueError::new_err(format!(
            "no document named '{}'",
            name
        ))),
    }
}

#[pyfunction]
fn getDocument(py: Python<'_>, name: String) -> Option<Py<Document>> {
    let reg = REG.lock().unwrap();
    reg.docs
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(_, _, doc)| doc.clone_ref(py))
}

#[pyfunction]
fn listDocuments() -> Vec<String> {
    REG.lock()
        .unwrap()
        .docs
        .iter()
        .map(|(_, n, _)| n.clone())
        .collect()
}

#[pyfunction]
fn activeDocument(py: Python<'_>) -> Option<Py<Document>> {
    let reg = REG.lock().unwrap();
    match reg.active {
        Some(id) => reg
            .docs
            .iter()
            .find(|(doc_id, _, _)| *doc_id == id)
            .map(|(_, _, doc)| doc.clone_ref(py)),
        None => None,
    }
}

#[pyfunction]
fn Version() -> Vec<String> {
    env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|s| s.to_string())
        .chain(["rust-pyo3".to_string(), String::new()])
        .collect()
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<Document>()?;
    m.add_class::<DocumentObject>()?;
    m.add_function(wrap_pyfunction!(newDocument, m)?)?;
    m.add_function(wrap_pyfunction!(closeDocument, m)?)?;
    m.add_function(wrap_pyfunction!(getDocument, m)?)?;
    m.add_function(wrap_pyfunction!(listDocuments, m)?)?;
    m.add_function(wrap_pyfunction!(activeDocument, m)?)?;
    m.add_function(wrap_pyfunction!(Version, m)?)?;
    Ok(())
}
