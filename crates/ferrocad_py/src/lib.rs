//! # ferrocad_py
//!
//! The PyO3 extension that exposes [`ferrocad_core`](https://crates.io/crates/ferrocad_core)
//! to Python as the **`ferrocad`** module. It contains no model logic of its own —
//! it wraps the core types as `#[pyclass]`es and adapts FreeCAD's Python surface
//! (document/object/property APIs, `Base` value types, observers, transactions).
//!
//! This is an implementation detail of the `ferrocad` distribution. **Users do not
//! import it directly**: the pure-Python `python/FreeCAD` facade imports `ferrocad`
//! and re-exports the App-level API under the `FreeCAD` namespace, so workbench
//! scripts keep writing `import FreeCAD`.
//!
//! The documentation is organised following [Diátaxis](https://diataxis.fr).
//!
//! # How-to guide: use it from Python
//!
//! ```python
//! import FreeCAD                       # the facade
//! doc = FreeCAD.newDocument("Demo")
//! obj = doc.addObject("App::FeaturePython", "Box")
//! obj.addProperty("App::PropertyLength", "Height")
//! obj.Height = "25 mm"
//! doc.recompute()
//! ```
//!
//! The same code runs against upstream FreeCAD and against FerroCAD; only the
//! backend behind `import FreeCAD` differs.
//!
//! # Explanation
//!
//! Each core document or object is shared with Python behind an
//! `Arc<Mutex<..>>`. Observer arguments are backed by a global identity cache so
//! the same object compares equal with `is` across calls. Because the types are
//! built with the `abi3` stable ABI, one binary works across CPython versions.
//!
//! The crate is compiled as a `cdylib` extension module (not published docs); the
//! Rust-level API reference here is thin by design — the interesting surface is the
//! Python API, documented on the `FreeCAD` facade and by the upstream
//! [FreeCAD documentation](https://wiki.freecad.org).
//!
//! # Implemented capabilities
//!
//! Follows `ferrocad_core`: the `Base`/`App` surface through M4 slices 1–16, plus the
//! MVP slices A1 (object state / property status), B1 (name/label semantics), and A2
//! ([`PropertyEnumeration`](https://docs.rs/ferrocad_core) and type validation). See
//! the repository `docs/milestones.md` for the per-slice record.

#![allow(non_snake_case)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_core::{canonical_name, parse_unit, prop_status, status_from_name, status_names, Document as CoreDocument, Matrix4, ObjectId, Placement, Property, Quantity, Rotation, StringHasher, StringId, TypeId, Unit, Vector3};
use pyo3::exceptions::{
    PyAttributeError, PyIndexError, PyNotImplementedError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PyBool, PyBytes, PyDict, PyDictMethods, PyTuple, PyType};
use pyo3::IntoPyObjectExt;

// ---------------------------------------------------------------------------
// Observer registry + object identity cache
// ---------------------------------------------------------------------------

/// Python objects registered via `FreeCAD.addDocumentObserver`.
static OBSERVERS: OnceLock<Mutex<Vec<Py<PyAny>>>> = OnceLock::new();

/// Canonical `Py<PyDocumentObject>` per `(document pointer, object id)`, so the
/// same Rust object always maps to the *same* Python object (needed for `is`).
static OBJECT_CACHE: OnceLock<Mutex<HashMap<(usize, ObjectId), Py<PyDocumentObject>>>> = OnceLock::new();

fn observers() -> &'static Mutex<Vec<Py<PyAny>>> {
    OBSERVERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn object_cache() -> &'static Mutex<HashMap<(usize, ObjectId), Py<PyDocumentObject>>> {
    OBJECT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn doc_key(doc: &Py<PyDocument>) -> usize {
    doc.as_ptr() as usize
}

/// Return the canonical Python object for `(doc, id)`, creating it once.
fn get_or_create_object(
    py: Python<'_>,
    doc: &Py<PyDocument>,
    inner: &Arc<Mutex<CoreDocument>>,
    id: ObjectId,
) -> Py<PyDocumentObject> {
    let key = (doc_key(doc), id);
    if let Some(existing) = object_cache().lock().unwrap().get(&key) {
        return existing.clone_ref(py);
    }
    let obj = Py::new(
        py,
        PyDocumentObject {
            doc: doc.clone_ref(py),
            inner: Arc::clone(inner),
            id,
        },
    )
    .unwrap();
    object_cache().lock().unwrap().insert(key, obj.clone_ref(py));
    obj
}

fn forget_object(doc: &Py<PyDocument>, id: ObjectId) {
    object_cache().lock().unwrap().remove(&(doc_key(doc), id));
}

/// Error for any access on a document object that is no longer in its document
/// (FreeCAD raises `ReferenceError` for deleted objects).
fn deleted_object_error(attr: &str) -> PyErr {
    pyo3::exceptions::PyReferenceError::new_err(format!(
        "Cannot access attribute '{attr}' of deleted object"
    ))
}

/// True if `id` still names an object in the document.
fn object_is_attached(inner: &Arc<Mutex<CoreDocument>>, id: ObjectId) -> bool {
    inner.lock().unwrap().object(id).is_some()
}

fn forget_document(doc: &Py<PyDocument>) {
    let key = doc_key(doc);
    object_cache().lock().unwrap().retain(|(d, _), _| *d != key);
}

fn observer_snapshot(py: Python<'_>) -> Vec<Py<PyAny>> {
    observers()
        .lock()
        .unwrap()
        .iter()
        .map(|o| o.clone_ref(py))
        .collect()
}

/// Call `slot` on every registered observer, swallowing per-observer errors.
fn fire_doc(slot: &str, doc: &Bound<'_, PyDocument>, extra: Option<&Bound<'_, PyAny>>) {
    let py = doc.py();
    for ob in observer_snapshot(py) {
        let ob = ob.bind(py);
        if let Ok(m) = ob.getattr(slot) {
            let _ = match extra {
                Some(e) => m.call1((doc, e)),
                None => m.call1((doc,)),
            };
        }
    }
}

fn fire_obj(slot: &str, obj: &Bound<'_, PyDocumentObject>, extra: Option<&Bound<'_, PyAny>>) {
    let py = obj.py();
    for ob in observer_snapshot(py) {
        let ob = ob.bind(py);
        if let Ok(m) = ob.getattr(slot) {
            let _ = match extra {
                Some(e) => m.call1((obj, e)),
                None => m.call1((obj,)),
            };
        }
    }
}

fn fire_doc_str(slot: &str, doc: &Bound<'_, PyDocument>, extra: &str) {
    let py = doc.py();
    let e = extra.into_py_any(py).unwrap().into_bound(py);
    fire_doc(slot, doc, Some(&e));
}

fn fire_obj_str(slot: &str, obj: &Bound<'_, PyDocumentObject>, extra: &str) {
    let py = obj.py();
    let e = extra.into_py_any(py).unwrap().into_bound(py);
    fire_obj(slot, obj, Some(&e));
}

// ---------------------------------------------------------------------------
// Pickling helpers (for `PropertyPythonObject` and Python instance state)
// ---------------------------------------------------------------------------

/// Pickle a Python value to a base64 string.
fn pickle_b64(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let pickle = py.import("pickle")?;
    let base64 = py.import("base64")?;
    let data = pickle.call_method1("dumps", (value,))?;
    let encoded = base64.call_method1("b64encode", (data,))?;
    encoded.call_method0("decode")?.extract()
}

/// Unpickle a base64 string back into a Python value.
fn unpickle_b64<'py>(py: Python<'py>, data: &str) -> PyResult<Bound<'py, PyAny>> {
    let pickle = py.import("pickle")?;
    let base64 = py.import("base64")?;
    let raw = base64.call_method1("b64decode", (data,))?;
    pickle.call_method1("loads", (raw,))
}

/// Capture an object's Python instance state (`__dict__` + `Proxy`) as a base64
/// pickle, or `None` when there is nothing to persist.
fn capture_object_state(py: Python<'_>, obj: &Bound<'_, PyDocumentObject>) -> Option<String> {
    let dict = obj.getattr("__dict__").ok()?.downcast_into::<PyDict>().ok()?;
    if dict.is_empty() {
        return None;
    }

    let payload = PyDict::new(py);
    let attrs = PyDict::new(py);
    for (k, v) in dict.iter() {
        if k.extract::<String>().map(|s| s == "Proxy").unwrap_or(false) {
            continue;
        }
        let _ = attrs.set_item(k, v);
    }
    let _ = payload.set_item("attrs", &attrs);

    if let Ok(Some(proxy)) = dict.get_item("Proxy") {
        if !proxy.is_none() {
            let info = PyDict::new(py);
            let module: String = proxy
                .getattr("__module__")
                .and_then(|m| m.extract())
                .unwrap_or_default();
            let class: String = proxy
                .get_type()
                .name()
                .and_then(|n| n.extract())
                .unwrap_or_default();
            // FreeCAD protocol: `Proxy.dumps()` if present, else the proxy dict.
            let data = if proxy.hasattr("dumps").unwrap_or(false) {
                proxy.call_method0("dumps")
            } else {
                proxy.getattr("__dict__")
            }
            .unwrap_or_else(|_| py.None().into_bound(py));
            let _ = info.set_item("module", module);
            let _ = info.set_item("class", class);
            let _ = info.set_item("data", data);
            let _ = payload.set_item("proxy", info);
        }
    }

    pickle_b64(py, payload.as_any()).ok()
}

/// Restore an object's Python instance state from a `capture_object_state` blob.
fn apply_object_state(
    py: Python<'_>,
    obj: &Bound<'_, PyDocumentObject>,
    state: &str,
) -> PyResult<()> {
    let payload = unpickle_b64(py, state)?;
    let payload = payload.downcast::<PyDict>()?;

    if let Ok(Some(attrs)) = payload.get_item("attrs") {
        if let Ok(attrs) = attrs.downcast_into::<PyDict>() {
            let dict = obj.getattr("__dict__")?.downcast_into::<PyDict>()?;
            for (k, v) in attrs.iter() {
                dict.set_item(k, v)?;
            }
        }
    }

    if let Ok(Some(proxy_info)) = payload.get_item("proxy") {
        let info = proxy_info.downcast_into::<PyDict>()?;
        let module: String = info
            .get_item("module")?
            .and_then(|m| m.extract().ok())
            .unwrap_or_default();
        let class: String = info
            .get_item("class")?
            .and_then(|m| m.extract().ok())
            .unwrap_or_default();
        let data = info
            .get_item("data")?
            .unwrap_or_else(|| py.None().into_bound(py));
        if !module.is_empty() && !class.is_empty() {
            let sys = py.import("sys")?;
            let modules = sys.getattr("modules")?.downcast_into::<PyDict>()?;
            let module_obj = match modules.get_item(&module)? {
                Some(m) => m,
                None => py
                    .import("importlib")?
                    .call_method1("import_module", (&module,))?,
            };
            if let Ok(cls) = module_obj.getattr(class.as_str()) {
                // Bypass `__init__` (which may require the object argument).
                let proxy = cls.getattr("__new__")?.call1((&cls,))?;
                if proxy.hasattr("loads")? {
                    proxy.call_method1("loads", (data,))?;
                } else if let Ok(d) = data.downcast::<PyDict>() {
                    let pd = proxy.getattr("__dict__")?.downcast_into::<PyDict>()?;
                    for (k, v) in d.iter() {
                        pd.set_item(k, v)?;
                    }
                }
                obj.setattr("Proxy", proxy)?;
            }
        }
    }
    Ok(())
}

/// Capture Python state for every object into its core `python_state` field.
fn capture_all_python_states(
    py: Python<'_>,
    doc_py: &Py<PyDocument>,
    inner: &Arc<Mutex<CoreDocument>>,
) {
    let ids = inner.lock().unwrap().object_ids();
    let mut states: Vec<(ObjectId, Option<String>)> = Vec::new();
    for id in ids {
        let obj = get_or_create_object(py, doc_py, inner, id);
        states.push((id, capture_object_state(py, obj.bind(py))));
    }
    let mut doc = inner.lock().unwrap();
    for (id, state) in states {
        if let Some(o) = doc.object_mut(id) {
            o.python_state = state;
        }
    }
}

/// Apply every object's core `python_state` back onto its Python object.
fn apply_all_python_states(py: Python<'_>, doc_py: &Py<PyDocument>, inner: &Arc<Mutex<CoreDocument>>) {
    let states: Vec<(ObjectId, String)> = {
        let doc = inner.lock().unwrap();
        doc.object_ids()
            .into_iter()
            .filter_map(|id| {
                doc.object(id)
                    .and_then(|o| o.python_state.clone())
                    .map(|s| (id, s))
            })
            .collect()
    };
    for (id, state) in states {
        let obj = get_or_create_object(py, doc_py, inner, id);
        let _ = apply_object_state(py, obj.bind(py), &state);
    }
}

// ---------------------------------------------------------------------------
// Unit
// ---------------------------------------------------------------------------

#[pyclass(name = "Unit", module = "ferrocad")]
#[derive(Clone, Copy)]
struct PyUnit {
    inner: Unit,
}

#[pymethods]
impl PyUnit {
    #[new]
    fn new(symbol: &str) -> PyResult<Self> {
        match Unit::parse(symbol) {
            Some(u) => Ok(Self { inner: u }),
            None => Err(PyValueError::new_err(format!("unknown unit '{symbol}'"))),
        }
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyUnit>>() {
            Ok(o) => {
                self.inner.sig == o.inner.sig
                    && (self.inner.scale - o.inner.scale).abs() < 1e-12
            }
            Err(_) => false,
        }
    }

    fn __repr__(&self) -> String {
        let name = canonical_name(self.inner.sig);
        if name.is_empty() {
            "1".to_string()
        } else {
            name
        }
    }
}

// ---------------------------------------------------------------------------
// Quantity
// ---------------------------------------------------------------------------

#[pyclass(name = "Quantity", module = "ferrocad")]
#[derive(Clone)]
struct PyQuantity {
    inner: Quantity,
}

#[pymethods]
impl PyQuantity {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let q = match args.len() {
            0 => Quantity::dimensionless(0.0),
            1 => {
                let a = args.get_item(0)?;
                if let Ok(q) = a.extract::<PyRef<'_, PyQuantity>>() {
                    q.inner
                } else if let Ok(s) = a.extract::<String>() {
                    s.parse().map_err(PyValueError::new_err)?
                } else if let Ok(v) = a.extract::<f64>() {
                    Quantity::dimensionless(v)
                } else {
                    return Err(PyTypeError::new_err(
                        "Quantity() expects a string, number, or Quantity",
                    ));
                }
            }
            2 => {
                let value: f64 = args.get_item(0)?.extract()?;
                let unit: String = args.get_item(1)?.extract()?;
                let u = Unit::parse(&unit)
                    .ok_or_else(|| PyValueError::new_err(format!("unknown unit '{unit}'")))?;
                Quantity::new(value, u)
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Quantity() takes 0, 1, or 2 arguments",
                ))
            }
        };
        Ok(Self { inner: q })
    }

    #[getter]
    fn Value(&self) -> f64 {
        self.inner.value()
    }

    #[getter]
    fn UserString(&self) -> String {
        self.inner.user_string()
    }

    #[getter]
    fn Unit(&self) -> PyUnit {
        PyUnit { inner: self.inner.unit() }
    }

    #[getter]
    fn Format(&self, py: Python<'_>) -> PyObject {
        pyo3::types::PyDict::new(py).into_py_any(py).unwrap()
    }

    #[setter]
    fn set_Format(&self, _value: &Bound<'_, PyAny>) {}

    fn getValueAs(&self, unit: &str) -> PyResult<PyQuantity> {
        let u = parse_unit(unit)
            .ok_or_else(|| PyValueError::new_err(format!("unknown unit '{unit}'")))?;
        if u.sig != self.inner.sig() {
            return Err(PyValueError::new_err("incompatible unit for getValueAs"));
        }
        Ok(PyQuantity { inner: self.inner.in_unit(u) })
    }

    fn toStr(&self) -> String {
        self.inner.user_string()
    }

    // backward-compat (M2/M3b surface)
    fn value_mm(&self) -> f64 {
        self.inner.canonical_value()
    }

    fn value_in(&self, unit: &str) -> PyResult<f64> {
        let u = Unit::parse(unit)
            .ok_or_else(|| PyValueError::new_err(format!("unknown unit '{unit}'")))?;
        Ok(self.inner.value_in(u))
    }

    // arithmetic
    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyQuantity> {
        let o = other
            .extract::<PyRef<'_, PyQuantity>>()
            .map_err(|_| PyTypeError::new_err("can only add Quantity to Quantity"))?;
        Ok(PyQuantity { inner: self.inner.add(&o.inner).map_err(PyValueError::new_err)? })
    }

    fn __sub__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyQuantity> {
        let o = other
            .extract::<PyRef<'_, PyQuantity>>()
            .map_err(|_| PyTypeError::new_err("can only subtract Quantity from Quantity"))?;
        Ok(PyQuantity { inner: self.inner.sub(&o.inner).map_err(PyValueError::new_err)? })
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyQuantity> {
        let o = other
            .extract::<PyRef<'_, PyQuantity>>()
            .map_err(|_| PyTypeError::new_err("can only multiply Quantity by Quantity"))?;
        Ok(PyQuantity { inner: self.inner.mul(&o.inner) })
    }

    fn __truediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyQuantity> {
        let o = other
            .extract::<PyRef<'_, PyQuantity>>()
            .map_err(|_| PyTypeError::new_err("can only divide Quantity by Quantity"))?;
        Ok(PyQuantity { inner: self.inner.div(&o.inner) })
    }

    fn __pow__(&self, e: i32, _modulo: Option<&Bound<'_, PyAny>>) -> PyQuantity {
        PyQuantity { inner: self.inner.powi(e) }
    }

    fn __neg__(&self) -> PyQuantity {
        PyQuantity { inner: self.inner.neg() }
    }

    fn __float__(&self) -> f64 {
        self.inner.value()
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyQuantity>>() {
            Ok(o) => self.inner == o.inner,
            Err(_) => false,
        }
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
        Property::Integer(i) => (*i).into_py_any(py).unwrap(),
        Property::Quantity(q) => PyQuantity { inner: *q }.into_py_any(py).unwrap(),
        Property::FloatList(v) => v.clone().into_py_any(py).unwrap(),
        Property::IntegerList(v) => v.clone().into_py_any(py).unwrap(),
        Property::StringList(v) => v.clone().into_py_any(py).unwrap(),
        Property::BoolList(v) => v.clone().into_py_any(py).unwrap(),
        Property::Vector(v) => PyVector::fresh(*v).into_py_any(py).unwrap(),
        Property::VectorList(v) => v
            .iter()
            .map(|x| PyVector::fresh(*x).into_py_any(py).unwrap())
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::Placement(p) => PyPlacement::fresh(*p).into_py_any(py).unwrap(),
        Property::PlacementList(v) => v
            .iter()
            .map(|x| PyPlacement::fresh(*x).into_py_any(py).unwrap())
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::Rotation(r) => PyRotation::fresh(*r, None).into_py_any(py).unwrap(),
        Property::RotationList(v) => v
            .iter()
            .map(|x| PyRotation::fresh(*x, None).into_py_any(py).unwrap())
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::Matrix(m) => PyMatrix { inner: *m }.into_py_any(py).unwrap(),
        Property::Link(s) => s.clone().into_py_any(py).unwrap(),
        Property::LinkList(v) => v.clone().into_py_any(py).unwrap(),
        Property::LinkSub(s, subs) => {
            let obj = if s.is_empty() { py.None() } else { s.clone().into_py_any(py).unwrap() };
            (obj, subs.clone()).into_py_any(py).unwrap()
        }
        Property::ColorList(v) => v
            .iter()
            .map(|c| (c[0], c[1], c[2], c[3]))
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::PythonObject(s) => {
            if s.is_empty() {
                py.None()
            } else {
                unpickle_b64(py, s)
                    .unwrap_or_else(|_| py.None().into_bound(py))
                    .unbind()
            }
        }
        Property::FileIncluded(s) => s.clone().into_py_any(py).unwrap(),
        Property::IntPairList(v) => v
            .iter()
            .map(|p| *p)
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        // The current selection (empty string when no choices are set).
        Property::Enumeration(choices, idx) => choices
            .get(*idx)
            .cloned()
            .unwrap_or_default()
            .into_py_any(py)
            .unwrap(),
        // Constraints expose their value like the underlying scalar type.
        Property::IntegerConstraint { value, .. } => (*value).into_py_any(py).unwrap(),
        Property::FloatConstraint { value, .. } => (*value).into_py_any(py).unwrap(),
    }
}

/// Like [`property_to_py`], but for a geometry value read from a document
/// object: the result carries a write-through view back to `prop`, so
/// `obj.Placement.Base.x = 5` (and similar) propagate to the property.
fn property_to_py_at(
    py: Python<'_>,
    p: &Property,
    inner: &Arc<Mutex<CoreDocument>>,
    id: ObjectId,
    prop: &str,
) -> PyObject {
    let version = inner.lock().unwrap().property_version(id, prop);
    let view = |kind: ViewKind| GeometryView {
        inner: Arc::clone(inner),
        id,
        prop: prop.to_string(),
        expect: p.clone(),
        version,
        kind,
    };
    match p {
        Property::Placement(pl) => PyPlacement {
            inner: *pl,
            view: Some(view(ViewKind::Placement)),
        }
        .into_py_any(py)
        .unwrap(),
        Property::Rotation(r) => PyRotation {
            inner: *r,
            axis_cache: None,
            view: Some(view(ViewKind::Rotation)),
        }
        .into_py_any(py)
        .unwrap(),
        _ => property_to_py(py, p),
    }
}

fn py_to_property(value: &Bound<'_, PyAny>) -> PyResult<Property> {
    if let Ok(q) = value.downcast::<PyQuantity>() {
        return Ok(Property::Quantity(q.borrow().inner));
    }
    if let Ok(v) = value.downcast::<PyVector>() {
        return Ok(Property::Vector(v.borrow().inner));
    }
    if let Ok(p) = value.downcast::<PyPlacement>() {
        return Ok(Property::Placement(p.borrow().inner));
    }
    if let Ok(r) = value.downcast::<PyRotation>() {
        return Ok(Property::Rotation(r.borrow().inner));
    }
    if let Ok(m) = value.downcast::<PyMatrix>() {
        return Ok(Property::Matrix(m.borrow().inner));
    }
    if let Ok(b) = value.extract::<bool>() {
        return Ok(Property::Bool(b));
    }
    if let Ok(i) = value.extract::<i64>() {
        return Ok(Property::Integer(i));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(Property::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(Property::String(s));
    }
    // sequences (list/tuple) of homogeneous values
    if let Ok(v) = value.extract::<Vec<bool>>() {
        return Ok(Property::BoolList(v));
    }
    if let Ok(v) = value.extract::<Vec<i64>>() {
        return Ok(Property::IntegerList(v));
    }
    if let Ok(v) = value.extract::<Vec<f64>>() {
        return Ok(Property::FloatList(v));
    }
    if let Ok(v) = value.extract::<Vec<String>>() {
        return Ok(Property::StringList(v));
    }
    if let Ok(v) = value.extract::<Vec<PyRef<'_, PyVector>>>() {
        return Ok(Property::VectorList(v.iter().map(|x| x.inner).collect()));
    }
    if let Ok(v) = value.extract::<Vec<(f64, f64, f64)>>() {
        return Ok(Property::VectorList(
            v.iter().map(|(x, y, z)| Vector3::new(*x, *y, *z)).collect(),
        ));
    }
    if let Ok(v) = value.extract::<Vec<PyRef<'_, PyPlacement>>>() {
        return Ok(Property::PlacementList(v.iter().map(|x| x.inner).collect()));
    }
    if let Ok(v) = value.extract::<Vec<PyRef<'_, PyRotation>>>() {
        return Ok(Property::RotationList(v.iter().map(|x| x.inner).collect()));
    }
    Err(PyTypeError::new_err(
        "unsupported property value (expected str, int, float, bool, list, Quantity, Vector, Placement, Rotation, or Matrix)",
    ))
}

/// Apply a `setPropertyStatus` value to a status mask. Accepts an int (negative
/// clears the bits), a status name, or a sequence of either; a leading `-` on a
/// name clears that flag.
fn apply_status(status: &mut u32, val: &Bound<'_, PyAny>) -> PyResult<()> {
    if let Ok(i) = val.extract::<i64>() {
        if i >= 0 {
            *status |= i as u32;
        } else {
            *status &= !((-i) as u32);
        }
        return Ok(());
    }
    if let Ok(s) = val.extract::<String>() {
        return apply_status_name(status, &s);
    }
    if let Ok(iter) = val.try_iter() {
        for item in iter {
            apply_status(status, &item?)?;
        }
        return Ok(());
    }
    Err(PyTypeError::new_err(
        "property status must be an int, a str, or a sequence of either",
    ))
}

fn apply_status_name(status: &mut u32, name: &str) -> PyResult<()> {
    let (clear, bare) = match name.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    match status_from_name(bare) {
        Some(bit) if clear => {
            *status &= !bit;
            Ok(())
        }
        Some(bit) => {
            *status |= bit;
            Ok(())
        }
        None => Err(PyValueError::new_err(format!(
            "unknown property status '{name}'"
        ))),
    }
}

/// Map a FreeCAD property type id to its default value.
fn default_property(type_id: &str) -> Property {
    let t = type_id.to_ascii_lowercase();
    if t.ends_with("integerconstraint") {
        Property::IntegerConstraint { value: 0, min: 0, max: 0, step: 1 }
    } else if t.ends_with("floatconstraint") {
        Property::FloatConstraint { value: 0.0, min: 0.0, max: 0.0, step: 1.0 }
    } else if t.ends_with("enumeration") {
        Property::Enumeration(vec![], 0)
    } else if t.ends_with("placementlist") {
        Property::PlacementList(vec![])
    } else if t.ends_with("rotationlist") {
        Property::RotationList(vec![])
    } else if t.ends_with("integerlist") {
        Property::IntegerList(vec![])
    } else if t.ends_with("floatlist") {
        Property::FloatList(vec![])
    } else if t.ends_with("stringlist") {
        Property::StringList(vec![])
    } else if t.ends_with("boollist") {
        Property::BoolList(vec![])
    } else if t.ends_with("vectorlist") {
        Property::VectorList(vec![])
    } else if t.ends_with("placement") {
        Property::Placement(Placement::identity())
    } else if t.ends_with("rotation") {
        Property::Rotation(Rotation::identity())
    } else if t.ends_with("vector") {
        Property::Vector(Vector3::zero())
    } else if t.ends_with("matrix") {
        Property::Matrix(Matrix4::identity())
    } else if t.ends_with("integer") {
        Property::Integer(0)
    } else if t.ends_with("float") {
        Property::Float(0.0)
    } else if t.ends_with("bool") {
        Property::Bool(false)
    } else if t.ends_with("linksublist") {
        Property::LinkList(vec![])
    } else if t.ends_with("linksub") {
        Property::LinkSub(String::new(), vec![])
    } else if t.ends_with("colorlist") || t.ends_with("colourlist") {
        Property::ColorList(vec![])
    } else if t.ends_with("pythonobject") {
        Property::PythonObject(String::new())
    } else if t.ends_with("fileincluded") {
        Property::FileIncluded(String::new())
    } else if t.ends_with("intpairlist") {
        Property::IntPairList(vec![])
    } else if t.ends_with("linklist") {
        Property::LinkList(vec![])
    } else if t.ends_with("link") || t.ends_with("linksub") || t.ends_with("linksublist") {
        Property::Link(String::new())
    } else if t.ends_with("length") || t.ends_with("distance") || t.ends_with("quantity") || t.ends_with("angle") {
        Property::Quantity(Quantity::new(0.0, Unit::Millimeter))
    } else {
        Property::String(String::new())
    }
}

// ---------------------------------------------------------------------------
// Document / DocumentObject
// ---------------------------------------------------------------------------

fn copy_source_data(
    obj: &PyRef<'_, PyDocumentObject>,
) -> PyResult<(String, String, String, Vec<(String, Property)>, Vec<(String, String)>)> {
    let doc = obj.inner.lock().unwrap();
    let o = doc
        .object(obj.id)
        .ok_or_else(|| PyValueError::new_err("source object no longer exists"))?;
    Ok((
        o.name.clone(),
        o.label.clone(),
        o.type_id.clone(),
        o.properties.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        o.expressions.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ))
}

/// Whether duplicate labels are allowed: the `DuplicateLabels` document preference
/// (`User parameter:BaseApp/Preferences/Document`). Defaults to `false`.
fn duplicate_labels(py: Python<'_>) -> bool {
    py.import("FreeCAD")
        .and_then(|m| m.call_method1("ParamGet", ("User parameter:BaseApp/Preferences/Document",)))
        .and_then(|g| g.call_method1("GetBool", ("DuplicateLabels", false)))
        .and_then(|v| v.extract::<bool>())
        .unwrap_or(false)
}

/// Extension type ids (`App::DocumentObjectExtension`, `App::LinkExtensionPython`, …)
/// cannot be instantiated as document objects nor used as property types.
fn is_extension_type(type_id: &str) -> bool {
    type_id.ends_with("Extension") || type_id.ends_with("ExtensionPython")
}

/// Property type ids are namespaced `…::Property…` (e.g. `App::PropertyLength`).
fn is_property_type(type_id: &str) -> bool {
    type_id.contains("Property") && !is_extension_type(type_id)
}

/// Create the 6 datum sub-elements of an `App::Origin` and link them into its
/// `Group`. Each carries a `Placement` encoding the standard axis/plane frame.
fn create_origin_children(doc: &mut CoreDocument, origin_id: ObjectId) {
    use std::f64::consts::FRAC_PI_2;
    let children: [(&str, &str, Rotation); 6] = [
        ("X_Axis", "App::Line", Rotation::identity()),
        ("Y_Axis", "App::Line", Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), FRAC_PI_2)),
        ("Z_Axis", "App::Line", Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), -FRAC_PI_2)),
        ("XY_Plane", "App::Plane", Rotation::identity()),
        ("XZ_Plane", "App::Plane", Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), FRAC_PI_2)),
        ("YZ_Plane", "App::Plane", Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), FRAC_PI_2)),
    ];
    let mut names = Vec::with_capacity(6);
    for (base, ty, rot) in children {
        let child_id = doc.add_object(base, ty);
        let name = doc
            .object(child_id)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| base.to_string());
        let _ = doc.set_property(
            child_id,
            "Placement",
            Property::Placement(Placement::new(Vector3::zero(), rot)),
        );
        names.push(name);
    }
    let _ = doc.set_property(origin_id, "Group", Property::LinkList(names));
}

/// Extract a `Vector3` from a `Vector` or a 3-sequence.
fn extract_vector(v: &Bound<'_, PyAny>) -> PyResult<Vector3> {
    if let Ok(vec) = v.extract::<PyRef<'_, PyVector>>() {
        return Ok(vec.inner);
    }
    if let Ok(seq) = v.extract::<Vec<f64>>() {
        if seq.len() == 3 {
            return Ok(Vector3::new(seq[0], seq[1], seq[2]));
        }
    }
    Err(PyTypeError::new_err("expected a Vector or a 3-sequence"))
}

/// Extract a `Rotation` from a `Rotation` or a 4-sequence (quaternion).
fn extract_rotation(v: &Bound<'_, PyAny>) -> PyResult<Rotation> {
    if let Ok(r) = v.extract::<PyRef<'_, PyRotation>>() {
        return Ok(r.inner);
    }
    if let Ok(seq) = v.extract::<Vec<f64>>() {
        if seq.len() == 4 {
            return Ok(Rotation { q: [seq[0], seq[1], seq[2], seq[3]], raw_axis: None });
        }
    }
    Err(PyTypeError::new_err("expected a Rotation or a 4-sequence"))
}

/// Coerce a value into a link target (an object name, or `""` for null).
fn py_to_link(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if value.is_none() {
        return Ok(String::new());
    }
    if let Ok(o) = value.downcast::<PyDocumentObject>() {
        return Ok(o.borrow().name_string());
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(s);
    }
    Err(PyTypeError::new_err("Link expects a DocumentObject or a name"))
}

/// Coerce a value into a link list (names).
fn py_to_link_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    if value.is_none() {
        return Ok(vec![]);
    }
    if let Ok(v) = value.extract::<Vec<PyRef<'_, PyDocumentObject>>>() {
        return Ok(v.iter().map(|o| o.name_string()).collect());
    }
    if let Ok(v) = value.extract::<Vec<String>>() {
        return Ok(v);
    }
    Err(PyTypeError::new_err("LinkList expects a list of DocumentObjects"))
}

/// Coerce a value into a color list (`(r, g, b[, a])` tuples, alpha defaults to 1).
fn py_to_color_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<[f64; 4]>> {
    if value.is_none() {
        return Ok(vec![]);
    }
    if let Ok(v) = value.extract::<Vec<(f64, f64, f64, f64)>>() {
        return Ok(v.iter().map(|(r, g, b, a)| [*r, *g, *b, *a]).collect());
    }
    if let Ok(v) = value.extract::<Vec<(f64, f64, f64)>>() {
        return Ok(v.iter().map(|(r, g, b)| [*r, *g, *b, 1.0]).collect());
    }
    Err(PyTypeError::new_err(
        "ColorList expects a list of (r, g, b[, a]) tuples",
    ))
}

/// Coerce a value into a link-sub `(object_name, subnames)` pair.
fn py_to_link_sub(value: &Bound<'_, PyAny>) -> PyResult<(String, Vec<String>)> {
    if value.is_none() {
        return Ok((String::new(), vec![]));
    }
    let t = value
        .downcast::<PyTuple>()
        .map_err(|_| PyTypeError::new_err("LinkSub expects an (object, subnames) tuple"))?;
    if t.len() != 2 {
        return Err(PyTypeError::new_err("LinkSub expects a 2-tuple"));
    }
    let obj = t.get_item(0)?;
    let subs = t.get_item(1)?;
    let name = if obj.is_none() {
        String::new()
    } else if let Ok(o) = obj.downcast::<PyDocumentObject>() {
        o.borrow().name_string()
    } else {
        return Err(PyTypeError::new_err(
            "LinkSub object must be a DocumentObject or None",
        ));
    };
    let names: Vec<String> = if let Ok(s) = subs.extract::<String>() {
        if s.is_empty() {
            vec![]
        } else {
            vec![s]
        }
    } else if let Ok(v) = subs.extract::<Vec<String>>() {
        v
    } else {
        return Err(PyTypeError::new_err(
            "LinkSub subnames must be a string or a list of strings",
        ));
    };
    Ok((name, names))
}

/// Parse one `(int, int)` pair, rejecting wrong arity/types (OverflowError for
/// out-of-range ints is propagated from the `i64` extraction).
fn parse_int_pair(item: &Bound<'_, PyAny>) -> PyResult<(i64, i64)> {
    if item.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err("expected an (int, int) pair"));
    }
    let items: Vec<Bound<'_, PyAny>> = item
        .try_iter()
        .map_err(|_| PyTypeError::new_err("expected an (int, int) pair"))?
        .collect::<PyResult<Vec<_>>>()
        .map_err(|_| PyTypeError::new_err("expected an (int, int) pair"))?;
    if items.len() != 2 {
        return Err(PyTypeError::new_err("expected an (int, int) pair"));
    }
    Ok((items[0].extract::<i64>()?, items[1].extract::<i64>()?))
}

/// Parse a sequence of `(int, int)` pairs (`App::PropertyIntPairList`).
fn py_to_int_pair_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<(i64, i64)>> {
    let mut out = Vec::new();
    for item in value
        .try_iter()
        .map_err(|_| PyTypeError::new_err("expected a list of (int, int) pairs"))?
    {
        out.push(parse_int_pair(&item?)?);
    }
    Ok(out)
}

/// Coerce a value into an included-file path: a `(source, name)` tuple copies
/// `source` into the document's transient dir under `name`; a string is used as-is.
fn py_to_file_included(transient: &Path, value: &Bound<'_, PyAny>) -> PyResult<String> {
    if value.is_none() {
        return Ok(String::new());
    }
    if let Ok((source, name)) = value.extract::<(String, String)>() {
        std::fs::create_dir_all(transient).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let dest = transient.join(&name);
        std::fs::copy(&source, &dest).map_err(|e| PyValueError::new_err(e.to_string()))?;
        return Ok(dest.to_string_lossy().to_string());
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(s);
    }
    Err(PyTypeError::new_err(
        "File expects a path or a (source, name) tuple",
    ))
}

/// Store an arbitrary Python attribute on a document object (its `__dict__`),
/// mirroring FreeCAD's support for `obj.Proxy` and similar.
fn set_instance_attr(
    slf: &Bound<'_, PyDocumentObject>,
    name: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let dict = slf.getattr("__dict__")?;
    dict.downcast_into::<PyDict>()?.set_item(name, value)
}

/// Find the plain group (`geo == false`) or geo-feature group (`geo == true`)
/// that lists `slf` in its `Group` link list.
fn parent_of(slf: &Bound<'_, PyDocumentObject>, geo: bool) -> Option<Py<PyDocumentObject>> {
    let py = slf.py();
    let inner = Arc::clone(&slf.borrow().inner);
    let doc_py = slf.borrow().doc.clone_ref(py);
    let name = slf.borrow().name_string();
    let found = {
        let doc = inner.lock().unwrap();
        doc.object_ids().into_iter().find(|id| {
            match doc.object(*id) {
                Some(other) => {
                    (other.type_id == "App::Part") == geo
                        && doc.is_group_like(*id)
                        && matches!(other.properties.get("Group"),
                            Some(Property::LinkList(links)) if links.contains(&name))
                }
                None => false,
            }
        })
    };
    found.map(|id| get_or_create_object(py, &doc_py, &inner, id))
}

// ---------------------------------------------------------------------------
// Document settings (a namespaced view over `Document.Meta`)
// ---------------------------------------------------------------------------

/// Whether an expression source references the object `name` (e.g. `Name.Prop`),
/// guarding against matching inside a longer identifier.
fn expression_references(source: &str, name: &str) -> bool {
    let needle = format!("{name}.");
    source.match_indices(&needle).any(|(i, _)| {
        let boundary_before = source[..i]
            .chars()
            .next_back()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true);
        let after = &source[i + needle.len()..];
        let has_prop = after
            .chars()
            .next()
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false);
        boundary_before && has_prop
    })
}

fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_namespace(ns: &str) -> PyResult<()> {
    if ns.is_empty() || !ns.split('.').all(valid_ident) {
        return Err(PyValueError::new_err(format!(
            "invalid settings namespace '{ns}'"
        )));
    }
    Ok(())
}

fn validate_key(key: &str) -> PyResult<()> {
    if !valid_ident(key) {
        return Err(PyValueError::new_err(format!("invalid setting key '{key}'")));
    }
    Ok(())
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

#[pyclass(name = "DocumentSettings", module = "ferrocad")]
struct PyDocumentSettings {
    meta: Arc<Mutex<BTreeMap<String, String>>>,
    namespace: String,
}

impl PyDocumentSettings {
    fn full_key(&self, key: &str) -> String {
        format!("{}.{}", self.namespace, key)
    }

    fn get_raw(&self, key: &str) -> Option<String> {
        self.meta.lock().unwrap().get(&self.full_key(key)).cloned()
    }

    fn set_raw(&self, key: &str, value: String) {
        self.meta.lock().unwrap().insert(self.full_key(key), value);
    }
}

#[pymethods]
impl PyDocumentSettings {
    #[pyo3(signature = (key, default=""))]
    fn getString(&self, key: &str, default: &str) -> String {
        self.get_raw(key).unwrap_or_else(|| default.to_string())
    }

    #[pyo3(signature = (key, default=0))]
    fn getInt(&self, key: &str, default: i64) -> i64 {
        self.get_raw(key)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(default)
    }

    #[pyo3(signature = (key, default=0.0))]
    fn getFloat(&self, key: &str, default: f64) -> f64 {
        self.get_raw(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(default)
    }

    fn getBool(&self, key: &str, default: &Bound<'_, PyAny>) -> PyResult<bool> {
        if !default.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("default must be a bool"));
        }
        let default: bool = default.extract()?;
        Ok(self.get_raw(key).and_then(|v| parse_bool(&v)).unwrap_or(default))
    }

    fn setString(&self, key: &str, value: &str) -> PyResult<()> {
        validate_key(key)?;
        self.set_raw(key, value.to_string());
        Ok(())
    }

    fn setInt(&self, key: &str, value: i64) -> PyResult<()> {
        validate_key(key)?;
        self.set_raw(key, value.to_string());
        Ok(())
    }

    fn setFloat(&self, key: &str, value: f64) -> PyResult<()> {
        validate_key(key)?;
        self.set_raw(key, format!("{value}"));
        Ok(())
    }

    fn setBool(&self, key: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        validate_key(key)?;
        if !value.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("value must be a bool"));
        }
        let value: bool = value.extract()?;
        self.set_raw(key, if value { "true".into() } else { "false".into() });
        Ok(())
    }

    fn remove(&self, key: &str) -> PyResult<()> {
        validate_key(key)?;
        self.meta.lock().unwrap().remove(&self.full_key(key));
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        let prefix = format!("{}.", self.namespace);
        let mut out: Vec<String> = self
            .meta
            .lock()
            .unwrap()
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('.'))
            .map(|s| s.to_string())
            .collect();
        out.sort();
        out
    }
}

#[pyclass(name = "Document", module = "ferrocad")]
struct PyDocument {
    name: String,
    label: String,
    file_name: Option<String>,
    auto_created: bool,
    inner: Arc<Mutex<CoreDocument>>,
    /// Namespaced document meta-settings (`Doc.Meta` / `Doc.settings`).
    meta: Arc<Mutex<BTreeMap<String, String>>>,
    comment: Mutex<String>,
}

#[pyclass(name = "DocumentObject", module = "ferrocad", dict)]
struct PyDocumentObject {
    doc: Py<PyDocument>,
    inner: Arc<Mutex<CoreDocument>>,
    id: ObjectId,
}

impl PyDocumentObject {
    /// Internal, non-fallible name accessor (the fallible Python `Name` getter
    /// raises for deleted objects; internal coercions only see attached ones).
    fn name_string(&self) -> String {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.name.clone())
            .unwrap_or_default()
    }
}

impl PyDocument {
    fn transient_dir(&self) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("FreeCAD_transient_{}", self.name));
        dir
    }
}

#[pymethods]
impl PyDocument {
    #[new]
    #[pyo3(signature = (name="Unnamed"))]
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            label: name.to_string(),
            file_name: None,
            auto_created: false,
            inner: Arc::new(Mutex::new(CoreDocument::new())),
            meta: Arc::new(Mutex::new(BTreeMap::new())),
            comment: Mutex::new(String::new()),
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

    #[getter]
    fn FileName(&self) -> String {
        self.file_name.clone().unwrap_or_default()
    }

    #[getter]
    fn Comment(&self) -> String {
        self.comment.lock().unwrap().clone()
    }

    #[setter]
    fn set_Comment(slf: &Bound<'_, Self>, value: String) {
        fire_doc_str("slotBeforeChangeDocument", slf, "Comment");
        *slf.borrow().comment.lock().unwrap() = value;
        fire_doc_str("slotChangedDocument", slf, "Comment");
    }

    #[pyo3(signature = (r#type, name=None, objProxy=None, viewProxy=None, attach=false, viewType=None))]
    fn addObject(
        slf: Bound<'_, Self>,
        r#type: &str,
        name: Option<String>,
        objProxy: Option<&Bound<'_, PyAny>>,
        viewProxy: Option<&Bound<'_, PyAny>>,
        attach: bool,
        viewType: Option<String>,
    ) -> PyResult<Py<PyDocumentObject>> {
        let _ = (objProxy, viewProxy, attach, viewType);
        if is_extension_type(r#type) {
            return Err(PyTypeError::new_err(format!(
                "'{0}' is not a document object type",
                r#type
            )));
        }
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let name = name.unwrap_or_default();
        let dup = duplicate_labels(py);
        let (id, tx) = {
            let mut doc = inner.lock().unwrap();
            let tx = doc.begin_transaction_if_pending();
            let id = doc.add_object_with(&name, r#type, dup);
            if r#type == "App::Origin" {
                create_origin_children(&mut doc, id);
            }
            (id, tx)
        };
        if let Some(tx) = tx {
            fire_doc_str("slotOpenTransaction", &slf, &tx);
        }
        let obj = get_or_create_object(py, &doc_py, &inner, id);
        fire_obj("slotCreatedObject", obj.bind(py), None);
        Ok(obj)
    }

    fn getObject(
        slf: &Bound<'_, Self>,
        key: &Bound<'_, PyAny>,
    ) -> PyResult<Option<Py<PyDocumentObject>>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let id = if let Ok(name) = key.extract::<String>() {
            inner.lock().unwrap().get_by_name(&name)
        } else if let Ok(id) = key.extract::<usize>() {
            if inner.lock().unwrap().object(id).is_some() {
                Some(id)
            } else {
                None
            }
        } else {
            return Err(PyTypeError::new_err(
                "getObject expects a name (str) or an object id (int)",
            ));
        };
        Ok(id.map(|id| get_or_create_object(py, &doc_py, &inner, id)))
    }

    #[getter]
    fn Objects(slf: Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc: Py<PyDocument> = slf.unbind();
        let ids = inner.lock().unwrap().object_ids();
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc, &inner, id))
            .collect()
    }

    #[getter]
    fn CountObjects(&self) -> usize {
        self.inner.lock().unwrap().object_ids().len()
    }

    fn removeObject(slf: &Bound<'_, Self>, name: &str) -> PyResult<()> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let (found, tx) = {
            let mut doc = inner.lock().unwrap();
            let tx = doc.begin_transaction_if_pending();
            match doc.get_by_name(name) {
                Some(id) => {
                    doc.remove_object(id);
                    (Some(id), tx)
                }
                None => (None, tx),
            }
        };
        let id = found.ok_or_else(|| {
            PyValueError::new_err(format!(
                "no object named '{name}' in document '{}'",
                slf.borrow().name
            ))
        })?;
        if let Some(tx) = tx {
            fire_doc_str("slotOpenTransaction", slf, &tx);
        }
        let obj = get_or_create_object(py, &doc_py, &inner, id);
        forget_object(&doc_py, id);
        fire_obj("slotDeletedObject", obj.bind(py), None);
        Ok(())
    }

    fn recompute(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let executed = inner
            .lock()
            .unwrap()
            .recompute()
            .map_err(PyValueError::new_err)?;
        for id in &executed {
            let obj = get_or_create_object(py, &doc_py, &inner, *id);
            fire_obj("slotRecomputedObject", obj.bind(py), None);
        }
        fire_doc("slotRecomputedDocument", slf, None);
        Ok(executed.len())
    }

    // -- transactions -------------------------------------------------------
    #[pyo3(signature = (name = ""))]
    fn openTransaction(&self, name: &str) {
        self.inner.lock().unwrap().open_transaction_named(name);
    }

    fn commitTransaction(slf: &Bound<'_, Self>) {
        if slf.borrow().inner.lock().unwrap().commit_transaction() {
            fire_doc("slotCommitTransaction", slf, None);
        }
    }

    fn abortTransaction(slf: &Bound<'_, Self>) {
        if slf.borrow().inner.lock().unwrap().abort_transaction() {
            fire_doc("slotAbortTransaction", slf, None);
        }
    }

    fn undo(slf: &Bound<'_, Self>) -> bool {
        let did = slf.borrow().inner.lock().unwrap().undo();
        if did {
            fire_doc("slotUndoDocument", slf, None);
        }
        did
    }

    fn redo(slf: &Bound<'_, Self>) -> bool {
        let did = slf.borrow().inner.lock().unwrap().redo();
        if did {
            fire_doc("slotRedoDocument", slf, None);
        }
        did
    }

    // -- persistence ---------------------------------------------------------
    fn saveAs(slf: &Bound<'_, Self>, path: &str) -> PyResult<()> {
        let py = slf.py();
        let name = slf.borrow().name.clone();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        fire_doc_str("slotStartSaveDocument", slf, path);
        capture_all_python_states(py, &doc_py, &inner);
        inner
            .lock()
            .unwrap()
            .save_to_file(&name, path)
            .map_err(PyValueError::new_err)?;
        slf.borrow_mut().file_name = Some(path.to_string());
        fire_doc_str("slotFinishSaveDocument", slf, path);
        Ok(())
    }

    fn save(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let (name, path) = {
            let this = slf.borrow();
            (this.name.clone(), this.file_name.clone())
        };
        let path = path
            .ok_or_else(|| PyValueError::new_err("document has no file name; use saveAs first"))?;
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        fire_doc_str("slotStartSaveDocument", slf, &path);
        capture_all_python_states(py, &doc_py, &inner);
        inner
            .lock()
            .unwrap()
            .save_to_file(&name, &path)
            .map_err(PyValueError::new_err)?;
        fire_doc_str("slotFinishSaveDocument", slf, &path);
        Ok(())
    }

    fn load(slf: &Bound<'_, Self>, path: &str) -> PyResult<()> {
        let py = slf.py();
        let saved = CoreDocument::load_from_file(path).map_err(PyValueError::new_err)?;
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        forget_document(&doc_py);
        let mut this = slf.borrow_mut();
        this.name = saved.name.clone();
        this.label = saved.name.clone();
        this.file_name = Some(path.to_string());
        this.inner = Arc::new(Mutex::new(CoreDocument::from_saved(&saved)));
        this.meta = Arc::new(Mutex::new(BTreeMap::new()));
        drop(this);
        let inner = Arc::clone(&slf.borrow().inner);
        apply_all_python_states(py, &doc_py, &inner);
        Ok(())
    }

    /// Copy one object (or a sequence of objects) from another document.
    #[pyo3(signature = (object, recursive=false, return_all=false))]
    fn copyObject(
        slf: &Bound<'_, Self>,
        object: &Bound<'_, PyAny>,
        recursive: bool,
        return_all: bool,
    ) -> PyResult<PyObject> {
        let _ = (recursive, return_all);
        let py = slf.py();

        // Collect source objects' data (name, label, type, properties, expressions).
        let sources: Vec<(String, String, String, Vec<(String, Property)>, Vec<(String, String)>)> =
            if let Ok(obj) = object.extract::<PyRef<'_, PyDocumentObject>>() {
                vec![copy_source_data(&obj)?]
            } else if let Ok(objs) = object.extract::<Vec<PyRef<'_, PyDocumentObject>>>() {
                objs.iter().map(copy_source_data).collect::<PyResult<_>>()?
            } else {
                return Err(PyTypeError::new_err(
                    "copyObject expects a DocumentObject or a sequence of them",
                ));
            };

        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let dup = duplicate_labels(py);
        let mut copied: Vec<Py<PyDocumentObject>> = Vec::new();
        {
            let mut doc = inner.lock().unwrap();
            for (name, label, type_id, props, exprs) in &sources {
                // The copy keeps the source label only when duplicates are allowed;
                // otherwise the label is made unique (computed before inserting).
                let new_label = if dup { label.clone() } else { doc.unique_label(label) };
                let id = doc.add_object_with(name, type_id, dup);
                doc.set_label(id, &new_label);
                for (k, v) in props {
                    let _ = doc.set_property(id, k, v.clone());
                }
                for (k, v) in exprs {
                    let _ = doc.set_expression(id, k, v);
                }
                copied.push(get_or_create_object(py, &doc_py, &inner, id));
            }
        }

        if copied.len() == 1 {
            Ok(copied.pop().unwrap().into_py_any(py).unwrap())
        } else {
            Ok(copied.into_py_any(py).unwrap())
        }
    }

    // -- undo/redo metadata --------------------------------------------------
    #[getter]
    fn UndoNames(&self) -> Vec<String> {
        self.inner.lock().unwrap().undo_names()
    }

    #[getter]
    fn RedoNames(&self) -> Vec<String> {
        self.inner.lock().unwrap().redo_names()
    }

    #[getter]
    fn UndoCount(&self) -> usize {
        self.inner.lock().unwrap().undo_names().len()
    }

    #[getter]
    fn RedoCount(&self) -> usize {
        self.inner.lock().unwrap().redo_names().len()
    }

    /// Drop the whole undo/redo history (FreeCAD `clearUndos`).
    fn clearUndos(&self) {
        self.inner.lock().unwrap().clear_undos();
    }

    /// The undo mode. FreeCAD reports `1`; assignment is accepted and ignored
    /// (upstream's setter is a no-op), so `doc.UndoMode = 1` never fails.
    #[getter]
    fn UndoMode(&self) -> i64 {
        1
    }

    #[setter]
    fn set_UndoMode(&self, _value: i64) {}

    /// The number of undo steps, or the depth of transaction `id`
    /// (FreeCAD `getAvailableUndos`).
    #[pyo3(signature = (id=0))]
    fn getAvailableUndos(&self, id: usize) -> usize {
        self.inner.lock().unwrap().available_undos(id)
    }

    /// The number of redo steps, or the depth of transaction `id`
    /// (FreeCAD `getAvailableRedos`).
    #[pyo3(signature = (id=0))]
    fn getAvailableRedos(&self, id: usize) -> usize {
        self.inner.lock().unwrap().available_redos(id)
    }

    #[getter]
    fn MemSize(&self) -> usize {
        0
    }

    #[getter]
    fn UndoRedoMemSize(&self) -> usize {
        0
    }

    #[pyo3(signature = (Type="", Name="", Label=""))]
    fn findObjects(
        slf: &Bound<'_, Self>,
        Type: &str,
        Name: &str,
        Label: &str,
    ) -> PyResult<Vec<Py<PyDocumentObject>>> {
        if !Type.is_empty() && is_extension_type(Type) {
            return Err(PyTypeError::new_err(format!(
                "'{Type}' is not a document object type"
            )));
        }
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let ids: Vec<ObjectId> = {
            let doc = inner.lock().unwrap();
            doc.object_ids()
                .into_iter()
                .filter(|id| {
                    doc.object(*id)
                        .map(|o| {
                            (Type.is_empty() || o.type_id == Type)
                                && (Name.is_empty() || o.name == Name)
                                && (Label.is_empty() || o.label == Label)
                        })
                        .unwrap_or(false)
                })
                .collect()
        };
        Ok(ids
            .into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect())
    }

    #[getter]
    fn ActiveObject(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let id = inner.lock().unwrap().active_object()?;
        let doc: Py<PyDocument> = slf.clone().unbind();
        Some(get_or_create_object(py, &doc, &inner, id))
    }

    fn setAutoCreated(&mut self, v: bool) {
        self.auto_created = v;
    }

    fn isAutoCreated(&self) -> bool {
        self.auto_created
    }

    fn getBookedTransactionID(&self) -> usize {
        self.inner.lock().unwrap().booked_transaction_id()
    }

    // -- persistence / recovery ---------------------------------------------

    /// A per-document transient directory (used for recovery snapshots).
    #[getter]
    fn TransientDir(&self) -> String {
        self.transient_dir().to_string_lossy().to_string()
    }

    /// A file name inside the transient directory (`getTempFileName`).
    fn getTempFileName(&self, basename: &str) -> String {
        let dir = self.transient_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(basename).to_string_lossy().to_string()
    }

    /// Recovery snapshots are only writable outside a transaction.
    fn canWriteRecoverySnapshot(&self) -> bool {
        !self.inner.lock().unwrap().is_in_transaction()
    }

    #[pyo3(signature = (Compression=0))]
    fn dumpContent(slf: &Bound<'_, Self>, Compression: i64) -> PyResult<Py<PyBytes>> {
        let _ = Compression;
        let py = slf.py();
        let name = slf.borrow().name.clone();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        capture_all_python_states(py, &doc_py, &inner);
        let data = inner
            .lock()
            .unwrap()
            .dump(&name)
            .map_err(PyValueError::new_err)?;
        Ok(PyBytes::new(py, &data).unbind())
    }

    /// Replace this document's content from a `dumpContent` payload.
    fn restoreContent(slf: &Bound<'_, Self>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let bytes: Vec<u8> = data.extract()?;
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        forget_document(&doc_py);
        let inner = Arc::clone(&slf.borrow().inner);
        inner
            .lock()
            .unwrap()
            .restore_from_bytes(&bytes)
            .map_err(PyValueError::new_err)?;
        apply_all_python_states(py, &doc_py, &inner);
        Ok(())
    }

    /// Re-load the document from its saved file (clearing current content).
    fn restore(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let path = slf
            .borrow()
            .file_name
            .clone()
            .ok_or_else(|| PyValueError::new_err("document has no file name"))?;
        let saved = CoreDocument::load_from_file(&path).map_err(PyValueError::new_err)?;
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        forget_document(&doc_py);
        slf.borrow_mut().inner = Arc::new(Mutex::new(CoreDocument::from_saved(&saved)));
        let inner = Arc::clone(&slf.borrow().inner);
        apply_all_python_states(py, &doc_py, &inner);
        Ok(())
    }

    // -- meta settings -------------------------------------------------------

    #[getter]
    fn Meta(&self, py: Python<'_>) -> Py<PyDict> {
        let dict = PyDict::new(py);
        for (k, v) in self.meta.lock().unwrap().iter() {
            let _ = dict.set_item(k, v);
        }
        dict.unbind()
    }

    #[setter]
    fn set_Meta(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let dict = value
            .downcast::<PyDict>()
            .map_err(|_| PyTypeError::new_err("Meta must be a dict"))?;
        let mut meta = self.meta.lock().unwrap();
        meta.clear();
        for (k, v) in dict.iter() {
            meta.insert(k.extract()?, v.extract()?);
        }
        Ok(())
    }

    /// Return a namespaced view over `Meta` (keys `"<namespace>.<key>"`).
    fn settings(&self, namespace: &str) -> PyResult<PyDocumentSettings> {
        validate_namespace(namespace)?;
        Ok(PyDocumentSettings {
            meta: Arc::clone(&self.meta),
            namespace: namespace.to_string(),
        })
    }

    // -- topology ------------------------------------------------------------

    #[getter]
    fn RootObjects(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let ids: Vec<ObjectId> = {
            let doc = inner.lock().unwrap();
            let mut referenced = std::collections::BTreeSet::new();
            for id in doc.object_ids() {
                if let Some(o) = doc.object(id) {
                    for (_, p) in o.properties.iter() {
                        match p {
                            Property::Link(n) if !n.is_empty() => {
                                referenced.insert(n.clone());
                            }
                            Property::LinkList(ns) => referenced.extend(ns.iter().cloned()),
                            _ => {}
                        }
                    }
                }
            }
            doc.object_ids()
                .into_iter()
                .filter(|id| {
                    doc.object(*id)
                        .map(|o| !referenced.contains(&o.name))
                        .unwrap_or(false)
                })
                .collect()
        };
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect()
    }

    /// All objects in dependency-first order.
    #[getter]
    fn TopologicalSortedObjects(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let ids = {
            let doc = inner.lock().unwrap();
            doc.recompute_order().unwrap_or_else(|_| doc.object_ids())
        };
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect()
    }

    /// Objects are also exposed by name as attributes (`doc.Label_1`).
    fn __getattr__(slf: &Bound<'_, Self>, name: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = slf.py();
        let name: String = name.extract()?;
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(name));
        }
        let inner = Arc::clone(&slf.borrow().inner);
        let id = inner.lock().unwrap().get_by_name(&name).ok_or_else(|| {
            PyAttributeError::new_err(format!("'Document' object has no attribute '{name}'"))
        })?;
        let doc: Py<PyDocument> = slf.clone().unbind();
        Ok(get_or_create_object(py, &doc, &inner, id)
            .into_py_any(py)
            .unwrap())
    }
}

#[pymethods]
impl PyDocumentObject {
    /// Two objects are equal when they are the same object in the same document.
    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyDocumentObject>>() {
            Ok(o) => self.doc.is(&o.doc) && self.id == o.id,
            Err(_) => false,
        }
    }

    fn __hash__(&self) -> u64 {
        self.id as u64
    }

    /// The object's document-unique id (also accepted by `getObject`).
    #[getter]
    fn ID(&self) -> u64 {
        self.id as u64
    }

    #[getter]
    fn Name(&self) -> PyResult<String> {
        match self.inner.lock().unwrap().object(self.id) {
            Some(o) => Ok(o.name.clone()),
            None => Err(deleted_object_error("Name")),
        }
    }

    #[getter]
    fn Label(&self) -> PyResult<String> {
        match self.inner.lock().unwrap().object(self.id) {
            Some(o) => Ok(o.label.clone()),
            None => Err(deleted_object_error("Label")),
        }
    }

    #[getter]
    fn TypeId(&self) -> PyResult<String> {
        match self.inner.lock().unwrap().object(self.id) {
            Some(o) => Ok(o.type_id.clone()),
            None => Err(deleted_object_error("TypeId")),
        }
    }

    /// `ViewObject` is only present when a GUI is up; headless → `None`.
    #[getter]
    fn ViewObject(&self) -> Option<PyObject> {
        None
    }

    /// Whether the object must be recomputed (set by `enforceRecompute`/`touch`).
    #[getter]
    fn MustExecute(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|o| o.must_execute)
            .unwrap_or(false)
    }

    /// The object's state flags (`Invalid`/`Touched` vs `Up-to-date`).
    #[getter]
    fn State(&self) -> Vec<String> {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id) {
            Some(o) if o.invalid => vec!["Invalid".to_string(), "Touched".to_string()],
            Some(o) if o.must_execute => vec!["Touched".to_string()],
            _ => vec!["Up-to-date".to_string()],
        }
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

    /// `getTypeOfProperty`: the property's status flags as text names.
    fn getTypeOfProperty(&self, name: &str) -> PyResult<Vec<String>> {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id).and_then(|o| o.properties.status(name)) {
            Some(status) => Ok(status_names(status).into_iter().map(String::from).collect()),
            None => Err(PyAttributeError::new_err(format!("no property '{name}'"))),
        }
    }

    /// `getGroupOfProperty`: the UI group the property belongs to.
    fn getGroupOfProperty(&self, name: &str) -> PyResult<String> {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id).and_then(|o| o.properties.group(name)) {
            Some(group) => Ok(group.to_string()),
            None => Err(PyAttributeError::new_err(format!("no property '{name}'"))),
        }
    }

    /// `getDocumentationOfProperty`: the property's documentation string.
    fn getDocumentationOfProperty(&self, name: &str) -> PyResult<String> {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id).and_then(|o| o.properties.doc(name)) {
            Some(doc) => Ok(doc.to_string()),
            None => Err(PyAttributeError::new_err(format!("no property '{name}'"))),
        }
    }

    /// `getEnumerationsOfProperty`: an enumeration's allowed values, else `None`.
    fn getEnumerationsOfProperty(&self, name: &str) -> PyResult<Option<Vec<String>>> {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id).and_then(|o| o.properties.get(name)) {
            Some(Property::Enumeration(choices, _)) => Ok(Some(choices.clone())),
            Some(_) => Ok(None),
            None => Err(PyAttributeError::new_err(format!("no property '{name}'"))),
        }
    }

    /// `getPropertyStatus(name="")`: with no name, the supported status names.
    #[pyo3(signature = (name=""))]
    fn getPropertyStatus(&self, name: &str) -> PyResult<Vec<String>> {
        if name.is_empty() {
            return Ok([
                "ReadOnly",
                "Hidden",
                "Transient",
                "Output",
                "NoRecompute",
                "NoPersist",
                "Input",
            ]
            .into_iter()
            .map(String::from)
            .collect());
        }
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id).and_then(|o| o.properties.status(name)) {
            Some(status) => Ok(status_names(status).into_iter().map(String::from).collect()),
            None => Err(PyAttributeError::new_err(format!("no property '{name}'"))),
        }
    }

    /// `setPropertyStatus(name, val)`: add (or, with a leading `-`, clear) flags.
    fn setPropertyStatus(&self, name: &str, val: &Bound<'_, PyAny>) -> PyResult<()> {
        let current = {
            let doc = self.inner.lock().unwrap();
            doc.property_status(self.id, name)
                .ok_or_else(|| PyAttributeError::new_err(format!("no property '{name}'")))?
        };
        let mut status = current;
        apply_status(&mut status, val)?;
        let mut doc = self.inner.lock().unwrap();
        doc.set_property_status(self.id, name, status);
        Ok(())
    }

    #[pyo3(signature = (type_id, name, group="", doc="", attr=0, read_only=false, hidden=false, locked=false, enum_vals=None))]
    fn addProperty(
        slf: &Bound<'_, Self>,
        type_id: &str,
        name: &str,
        group: &str,
        doc: &str,
        attr: i64,
        read_only: bool,
        hidden: bool,
        locked: bool,
        enum_vals: Option<Vec<String>>,
    ) -> PyResult<()> {
        let _ = (group, doc, locked);
        if name.is_empty() {
            return Err(PyValueError::new_err("property name must not be empty"));
        }
        if !is_property_type(type_id) {
            return Err(PyTypeError::new_err(format!(
                "'{type_id}' is not a property type"
            )));
        }
        let mut status = attr.max(0) as u32;
        if read_only {
            status |= prop_status::READONLY;
        }
        if hidden {
            status |= prop_status::HIDDEN;
        }
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        // `enum_vals` seeds an enumeration's allowed values.
        let default = match (default_property(type_id), enum_vals) {
            (_, Some(vals)) => Property::Enumeration(vals, 0),
            (d, None) => d,
        };
        inner
            .lock()
            .unwrap()
            .add_property(id, name, default, status, group, doc)
            .map_err(PyValueError::new_err)?;
        fire_obj_str("slotAppendDynamicProperty", slf, name);
        Ok(())
    }

    #[pyo3(signature = (prop, source=None))]
    fn setExpression(&self, prop: &str, source: Option<String>) -> PyResult<()> {
        let mut doc = self.inner.lock().unwrap();
        match source {
            Some(src) => doc.set_expression(self.id, prop, &src).map_err(|e| {
                if e.starts_with("cyclic") {
                    PyRuntimeError::new_err(e)
                } else {
                    PyValueError::new_err(e)
                }
            }),
            None => {
                doc.remove_expression(self.id, prop);
                Ok(())
            }
        }
    }

    /// `ExpressionEngine`: the object's expressions as `(property, source)` pairs.
    #[getter]
    fn ExpressionEngine(&self) -> Vec<(String, Option<String>)> {
        self.inner
            .lock()
            .unwrap()
            .expressions(self.id)
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect()
    }

    /// Evaluate an expression source in this object's context.
    fn evalExpression(&self, source: &str) -> PyResult<f64> {
        self.inner
            .lock()
            .unwrap()
            .eval_expression(self.id, source)
            .map_err(PyValueError::new_err)
    }

    /// Mark the object for recompute (FreeCAD `DocumentObject.touch`).
    ///
    /// `touch()` forces execution; `touch("")` marks the object touched without
    /// forcing its own execution (dependents still recompute).
    #[pyo3(signature = (prop=None))]
    fn touch(&self, prop: Option<&str>) {
        let mut doc = self.inner.lock().unwrap();
        match prop {
            Some("") => doc.touch(self.id, true),
            _ => doc.touch(self.id, false),
        }
    }

    fn recompute(slf: &Bound<'_, Self>) -> bool {
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        if inner.lock().unwrap().take_must_execute_one(id) {
            fire_obj("slotRecomputedObject", slf, None);
        }
        true
    }

    fn enforceRecompute(&self) {
        self.inner.lock().unwrap().enforce_recompute(self.id);
    }

    /// Mark the object as unchanged (FreeCAD `purgeTouched`).
    fn purgeTouched(&self) {
        self.inner.lock().unwrap().purge_touched(self.id);
    }

    /// A short status string: `Invalid`, `Touched` or `Valid`.
    fn getStatusString(&self) -> String {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id) {
            Some(o) if o.invalid => "Invalid".to_string(),
            Some(o) if o.must_execute => "Touched".to_string(),
            _ => "Valid".to_string(),
        }
    }

    /// Record a property editor mode (`ReadOnly`, …); POC: fires the event only.
    fn setEditorMode(slf: &Bound<'_, Self>, prop: &str, _modes: &Bound<'_, PyAny>) {
        fire_obj_str("slotChangePropertyEditor", slf, prop);
    }

    fn removeProperty(slf: &Bound<'_, Self>, name: &str) -> PyResult<()> {
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        if inner.lock().unwrap().remove_property(id, name) {
            fire_obj_str("slotRemoveDynamicProperty", slf, name);
            Ok(())
        } else {
            Err(PyValueError::new_err(format!(
                "no property '{name}' on object"
            )))
        }
    }

    // -- persistence ---------------------------------------------------------

    /// Serialize this object's content to bytes (`dumpContent`).
    fn dumpContent(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        let data = self
            .inner
            .lock()
            .unwrap()
            .dump_object(self.id)
            .map_err(PyValueError::new_err)?;
        Ok(PyBytes::new(py, &data).unbind())
    }

    /// Restore this object's content from bytes (`restoreContent`).
    fn restoreContent(&self, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let bytes: Vec<u8> = data.extract()?;
        self.inner
            .lock()
            .unwrap()
            .restore_object(self.id, &bytes)
            .map_err(PyValueError::new_err)
    }

    /// Serialize one property to bytes (`dumpPropertyContent`).
    #[pyo3(signature = (name, Compression=0))]
    fn dumpPropertyContent(
        &self,
        py: Python<'_>,
        name: &str,
        Compression: i64,
    ) -> PyResult<Py<PyBytes>> {
        let _ = Compression;
        let data = self
            .inner
            .lock()
            .unwrap()
            .dump_property(self.id, name)
            .map_err(PyValueError::new_err)?;
        Ok(PyBytes::new(py, &data).unbind())
    }

    /// Restore one property from bytes (`restorePropertyContent`).
    fn restorePropertyContent(&self, name: &str, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let bytes: Vec<u8> = data.extract()?;
        self.inner
            .lock()
            .unwrap()
            .restore_property(self.id, name, &bytes)
            .map_err(PyValueError::new_err)
    }

    fn supportedProperties(&self) -> Vec<String> {
        vec![
            "App::PropertyString".to_string(),
            "App::PropertyFloat".to_string(),
            "App::PropertyBool".to_string(),
            "App::PropertyInteger".to_string(),
            "App::PropertyLength".to_string(),
            "App::PropertyVector".to_string(),
            "App::PropertyPlacement".to_string(),
            "App::PropertyMatrix".to_string(),
            "App::PropertyLink".to_string(),
        ]
    }

    // -- extensions ---------------------------------------------------------

    fn addExtension(slf: &Bound<'_, Self>, name: &str) -> PyResult<()> {
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        fire_obj_str("slotBeforeAddingDynamicExtension", slf, name);
        {
            let mut doc = inner.lock().unwrap();
            if !doc.add_extension(id, name) {
                return Err(PyValueError::new_err(format!("no object {id}")));
            }
            // Group extensions carry a `Group` link list property.
            if doc.is_group_like(id)
                && doc
                    .object(id)
                    .map(|o| o.properties.get("Group").is_none())
                    .unwrap_or(false)
            {
                let _ = doc.set_property(id, "Group", Property::LinkList(vec![]));
            }
        }
        fire_obj_str("slotAddedDynamicExtension", slf, name);
        Ok(())
    }

    fn hasExtension(&self, name: &str) -> bool {
        self.inner.lock().unwrap().has_extension(self.id, name)
    }

    fn removeExtension(&self, name: &str) -> PyResult<()> {
        if self.inner.lock().unwrap().remove_extension(self.id, name) {
            Ok(())
        } else {
            Err(PyValueError::new_err(format!(
                "object has no extension '{name}'"
            )))
        }
    }

    // -- groups -------------------------------------------------------------

    /// Resolve this object's `Group` link list into `DocumentObject`s.
    fn group_members(&self, py: Python<'_>) -> Vec<Py<PyDocumentObject>> {
        let inner = Arc::clone(&self.inner);
        let names: Vec<String> = {
            let doc = inner.lock().unwrap();
            match doc.object(self.id).and_then(|o| o.properties.get("Group")) {
                Some(Property::LinkList(links)) => links.clone(),
                _ => return vec![],
            }
        };
        names
            .into_iter()
            .filter_map(|name| {
                let id = inner.lock().unwrap().get_by_name(&name)?;
                Some(get_or_create_object(py, &self.doc, &inner, id))
            })
            .collect()
    }

    fn addObject(&self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let other_ref: PyRef<'_, PyDocumentObject> = other
            .extract()
            .map_err(|_| PyTypeError::new_err("addObject expects a DocumentObject"))?;
        let other_id = other_ref.id;
        let other_name = other_ref.name_string();
        drop(other_ref);

        let mut doc = self.inner.lock().unwrap();
        if !doc.is_group_like(self.id) {
            let type_id = doc.object(self.id).map(|o| o.type_id.clone()).unwrap_or_default();
            return Err(PyAttributeError::new_err(format!(
                "'{type_id}' is not a group; add a group extension first"
            )));
        }
        if other_id == self.id {
            return Err(PyValueError::new_err("a group cannot contain itself"));
        }
        doc.unlink_from_groups(other_id);
        let mut links = match doc.object(self.id).and_then(|o| o.properties.get("Group")) {
            Some(Property::LinkList(l)) => l.clone(),
            _ => vec![],
        };
        if !links.contains(&other_name) {
            links.push(other_name);
        }
        doc.set_property(self.id, "Group", Property::LinkList(links))
            .map_err(PyValueError::new_err)
    }

    fn hasObject(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let other_ref: PyRef<'_, PyDocumentObject> = other
            .extract()
            .map_err(|_| PyTypeError::new_err("hasObject expects a DocumentObject"))?;
        let name = other_ref.name_string();
        drop(other_ref);
        let doc = self.inner.lock().unwrap();
        Ok(match doc.object(self.id).and_then(|o| o.properties.get("Group")) {
            Some(Property::LinkList(links)) => links.contains(&name),
            _ => false,
        })
    }

    fn getObject(slf: &Bound<'_, Self>, name: &str) -> Option<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py = slf.borrow().doc.clone_ref(py);
        let in_group = {
            let doc = inner.lock().unwrap();
            match doc.object(slf.borrow().id).and_then(|o| o.properties.get("Group")) {
                Some(Property::LinkList(links)) => links.iter().any(|n| n == name),
                _ => false,
            }
        };
        if !in_group {
            return None;
        }
        let id = inner.lock().unwrap().get_by_name(name)?;
        Some(get_or_create_object(py, &doc_py, &inner, id))
    }

    /// The plain group (or group-extension object) that lists this object.
    fn getParentGroup(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        parent_of(slf, false)
    }

    /// The geo-feature group (`App::Part`) that lists this object.
    fn getParentGeoFeatureGroup(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        parent_of(slf, true)
    }

    /// `OutList`: the objects this object links to (group members).
    #[getter]
    fn OutList(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        slf.borrow().group_members(py)
    }

    /// `InList`: the objects that link to this object (any `Link`/`LinkList`/
    /// `LinkSub` property, or an expression referencing it).
    #[getter]
    fn InList(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py = slf.borrow().doc.clone_ref(py);
        let self_id = slf.borrow().id;
        let name = slf.borrow().name_string();
        let ids: Vec<ObjectId> = {
            let doc = inner.lock().unwrap();
            doc.object_ids()
                .into_iter()
                .filter(|id| {
                    if *id == self_id {
                        return false;
                    }
                    doc.object(*id)
                        .map(|o| {
                            let linked = o.properties.iter().any(|(_, p)| match p {
                                Property::Link(n) => n == &name,
                                Property::LinkList(ns) => ns.contains(&name),
                                Property::LinkSub(obj, _) => obj == &name,
                                _ => false,
                            });
                            linked
                                || o.expressions
                                    .values()
                                    .any(|src| expression_references(src, &name))
                        })
                        .unwrap_or(false)
                })
                .collect()
        };
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect()
    }

    /// Resolve sub-object(s) and return them per FreeCAD's `retType` convention:
    /// 1 → DocumentObject, 2 → (obj, Matrix, proxy), 3 → Placement, 4 → Matrix.
    #[pyo3(signature = (subname, retType=0, matrix=None, transform=true, depth=0))]
    fn getSubObject(
        slf: &Bound<'_, Self>,
        subname: &Bound<'_, PyAny>,
        retType: i64,
        matrix: Option<&Bound<'_, PyAny>>,
        transform: bool,
        depth: i64,
    ) -> PyResult<PyObject> {
        let _ = (matrix, transform, depth);
        let py = slf.py();

        let subs: Vec<String> = if let Ok(s) = subname.extract::<String>() {
            vec![s]
        } else if let Ok(seq) = subname.extract::<Vec<String>>() {
            seq
        } else {
            return Err(PyTypeError::new_err(
                "subname must be a string or a sequence of strings",
            ));
        };

        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py = slf.borrow().doc.clone_ref(py);

        let resolved: Vec<(Option<Py<PyDocumentObject>>, Placement)> = {
            let doc = inner.lock().unwrap();
            subs.iter()
                .map(|sub| {
                    let stripped = sub.trim_end_matches('.');
                    let child_id = doc.get_by_name(stripped);
                    let placement = child_id
                        .and_then(|id| doc.object(id))
                        .and_then(|o| o.properties.get("Placement"))
                        .and_then(|p| match p {
                            Property::Placement(pl) => Some(*pl),
                            _ => None,
                        })
                        .unwrap_or_else(Placement::identity);
                    let obj = child_id.map(|id| get_or_create_object(py, &doc_py, &inner, id));
                    (obj, placement)
                })
                .collect()
        };

        let build = |obj: &Option<Py<PyDocumentObject>>, pl: &Placement| -> PyObject {
            let obj_py = obj
                .as_ref()
                .map(|o| o.clone_ref(py).into_py_any(py).unwrap())
                .unwrap_or_else(|| py.None());
            match retType {
                1 => obj_py,
                2 => {
                    let mat_py = PyMatrix { inner: pl.to_matrix() }.into_py_any(py).unwrap();
                    (obj_py, mat_py, py.None()).into_py_any(py).unwrap()
                }
                3 => PyPlacement::fresh(*pl).into_py_any(py).unwrap(),
                4 => PyMatrix { inner: pl.to_matrix() }.into_py_any(py).unwrap(),
                _ => py.None(),
            }
        };

        if resolved.len() == 1 {
            let (obj, pl) = &resolved[0];
            Ok(build(obj, pl))
        } else {
            let items: Vec<PyObject> = resolved.iter().map(|(o, pl)| build(o, pl)).collect();
            Ok(items.into_py_any(py).unwrap())
        }
    }

    fn __getattr__(slf: &Bound<'_, Self>, name: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = slf.py();
        let name: String = name.extract()?;
        if name.starts_with('_') {
            return Err(PyAttributeError::new_err(name));
        }
        if !object_is_attached(&slf.borrow().inner, slf.borrow().id) {
            return Err(deleted_object_error(&name));
        }
        if name == "Group" {
            return Ok(slf.borrow().group_members(py).into_py_any(py).unwrap());
        }
        let (value, inner, doc_py, this_id) = {
            let this = slf.borrow();
            let doc = this.inner.lock().unwrap();
            let value = doc.object(this.id).and_then(|o| o.properties.get(&name)).cloned();
            (value, Arc::clone(&this.inner), this.doc.clone_ref(py), this.id)
        };
        match value {
            // Link properties resolve to the referenced object (or None).
            Some(Property::Link(link)) => {
                if link.is_empty() {
                    return Ok(py.None());
                }
                match inner.lock().unwrap().get_by_name(&link) {
                    Some(id) => Ok(get_or_create_object(py, &doc_py, &inner, id)
                        .into_py_any(py)
                        .unwrap()),
                    None => Ok(py.None()),
                }
            }
            Some(Property::LinkList(links)) => {
                let objs: Vec<Py<PyDocumentObject>> = links
                    .iter()
                    .filter_map(|n| {
                        let id = inner.lock().unwrap().get_by_name(n)?;
                        Some(get_or_create_object(py, &doc_py, &inner, id))
                    })
                    .collect();
                Ok(objs.into_py_any(py).unwrap())
            }
            // LinkSub exposes `(object_or_None, [subnames])`, or None when empty.
            Some(Property::LinkSub(obj, subs)) => {
                if obj.is_empty() && subs.is_empty() {
                    return Ok(py.None());
                }
                let obj_py = if obj.is_empty() {
                    py.None()
                } else {
                    match inner.lock().unwrap().get_by_name(&obj) {
                        Some(id) => get_or_create_object(py, &doc_py, &inner, id)
                            .into_py_any(py)
                            .unwrap(),
                        None => py.None(),
                    }
                };
                Ok((obj_py, subs.clone()).into_py_any(py).unwrap())
            }
            Some(p) => Ok(property_to_py_at(py, &p, &inner, this_id, &name)),
            None => {
                let type_id = {
                    let this = slf.borrow();
                    let doc = this.inner.lock().unwrap();
                    doc.object(this.id).map(|o| o.type_id.clone()).unwrap_or_default()
                };
                Err(PyAttributeError::new_err(format!(
                    "'{type_id}' object has no attribute '{name}'"
                )))
            }
        }
    }

    fn __setattr__(slf: &Bound<'_, Self>, name: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let name: String = name.extract()?;
        if name == "Label" {
            let label: String = value
                .extract()
                .map_err(|_| PyTypeError::new_err("Label must be a string"))?;
            let (inner, id) = {
                let this = slf.borrow();
                (Arc::clone(&this.inner), this.id)
            };
            fire_obj_str("slotBeforeChangeObject", slf, "Label");
            inner.lock().unwrap().set_label(id, &label);
            fire_obj_str("slotChangedObject", slf, "Label");
            return Ok(());
        }
        if name == "Group" {
            return slf.borrow().set_group(value);
        }
        if name.starts_with('_') {
            return set_instance_attr(slf, &name, value);
        }
        // Coerce by the existing property's type where it matters (links).
        let existing = {
            let this = slf.borrow();
            let doc = this.inner.lock().unwrap();
            doc.object(this.id).and_then(|o| o.properties.get(&name)).cloned()
        };
        let property = match existing {
            Some(Property::Link(_)) => Property::Link(py_to_link(value)?),
            Some(Property::LinkList(_)) => Property::LinkList(py_to_link_list(value)?),
            Some(Property::LinkSub(_, _)) => {
                let (obj, subs) = py_to_link_sub(value)?;
                Property::LinkSub(obj, subs)
            }
            Some(Property::ColorList(_)) => Property::ColorList(py_to_color_list(value)?),
            Some(Property::PythonObject(_)) => Property::PythonObject(pickle_b64(slf.py(), value)?),
            Some(Property::FileIncluded(_)) => {
                let doc = slf.borrow().doc.clone_ref(slf.py());
                let transient = doc.bind(slf.py()).borrow().transient_dir();
                Property::FileIncluded(py_to_file_included(&transient, value)?)
            }
            Some(Property::IntPairList(existing)) => {
                if let Ok(dict) = value.downcast::<PyDict>() {
                    let mut list = existing.clone();
                    for (k, v) in dict.iter() {
                        let idx: usize = k
                            .extract()
                            .map_err(|_| PyTypeError::new_err("index must be an int"))?;
                        if idx >= list.len() {
                            return Err(pyo3::exceptions::PyIndexError::new_err(
                                "index out of range",
                            ));
                        }
                        list[idx] = parse_int_pair(&v)?;
                    }
                    Property::IntPairList(list)
                } else {
                    Property::IntPairList(py_to_int_pair_list(value)?)
                }
            }
            Some(Property::Enumeration(choices, _)) => {
                // A string selects a value (error if not offered); an int selects by
                // index; a sequence of strings sets the allowed values.
                if let Ok(s) = value.extract::<String>() {
                    match choices.iter().position(|c| c == &s) {
                        Some(i) => Property::Enumeration(choices, i),
                        None => {
                            return Err(PyValueError::new_err(format!(
                                "{s:?} is not part of the enumeration"
                            )))
                        }
                    }
                } else if let Ok(i) = value.extract::<i64>() {
                    if i < 0 || i as usize >= choices.len() {
                        return Err(PyValueError::new_err("enumeration index out of range"));
                    }
                    Property::Enumeration(choices, i as usize)
                } else if let Ok(list) = value.extract::<Vec<String>>() {
                    Property::Enumeration(list, 0)
                } else {
                    return Err(PyTypeError::new_err(
                        "expected a list of strings, an index, or a value",
                    ));
                }
            }
            Some(Property::Float(_)) => match value.extract::<f64>() {
                Ok(f) => Property::Float(f),
                Err(_) => py_to_property(value)?,
            },
            Some(Property::Integer(_)) => match value.extract::<i64>() {
                Ok(i) => Property::Integer(i),
                Err(_) => py_to_property(value)?,
            },
            // Assigning `(value, min, max, step)` sets the range too; a bare
            // number is clamped into the existing range.
            Some(Property::IntegerConstraint { min, max, step, .. }) => {
                if let Ok((v, lo, hi, st)) = value.extract::<(i64, i64, i64, i64)>() {
                    Property::IntegerConstraint {
                        value: v.clamp(lo, hi),
                        min: lo,
                        max: hi,
                        step: st,
                    }
                } else {
                    let v: i64 = value.extract().map_err(|_| {
                        PyTypeError::new_err("expected an int or an (int, min, max, step) tuple")
                    })?;
                    Property::IntegerConstraint { value: v.clamp(min, max), min, max, step }
                }
            }
            Some(Property::FloatConstraint { min, max, step, .. }) => {
                if let Ok((v, lo, hi, st)) =
                    value.extract::<(f64, f64, f64, f64)>()
                {
                    Property::FloatConstraint {
                        value: v.clamp(lo, hi),
                        min: lo,
                        max: hi,
                        step: st,
                    }
                } else {
                    let v: f64 = value.extract().map_err(|_| {
                        PyTypeError::new_err("expected a float or a (float, min, max, step) tuple")
                    })?;
                    Property::FloatConstraint { value: v.clamp(min, max), min, max, step }
                }
            }
            Some(Property::Bool(_)) => match value.extract::<bool>() {
                Ok(b) => Property::Bool(b),
                Err(_) => py_to_property(value)?,
            },
            // A length/quantity property keeps its kind: "10 mm" parses, and a
            // bare number is read in the property's own unit.
            Some(Property::Quantity(existing)) => {
                if let Ok(q) = value.extract::<PyRef<'_, PyQuantity>>() {
                    Property::Quantity(q.inner)
                } else if let Ok(s) = value.extract::<String>() {
                    Property::Quantity(s.parse::<Quantity>().map_err(PyValueError::new_err)?)
                } else if let Ok(f) = value.extract::<f64>() {
                    Property::Quantity(Quantity::new(f, existing.unit()))
                } else {
                    py_to_property(value)?
                }
            }
            Some(_) => py_to_property(value)?,
            // Not a known property: store it as a Python attribute (`Proxy`, …).
            None => return set_instance_attr(slf, &name, value),
        };
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        fire_obj_str("slotBeforeChangeObject", slf, &name);
        let result = inner.lock().unwrap().set_property(id, &name, property);
        result.map_err(PyValueError::new_err)?;
        fire_obj_str("slotChangedObject", slf, &name);
        Ok(())
    }

    /// Assign the `Group` link list from a sequence of objects (or names).
    fn set_group(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let names: Vec<String> = if let Ok(objs) = value.extract::<Vec<PyRef<'_, PyDocumentObject>>>() {
            objs.iter().map(|o| o.name_string()).collect()
        } else if let Ok(names) = value.extract::<Vec<String>>() {
            names
        } else {
            return Err(PyTypeError::new_err(
                "Group expects a list of DocumentObject (or names)",
            ));
        };
        let mut doc = self.inner.lock().unwrap();
        // Enforce single-group membership.
        for n in &names {
            if let Some(member) = doc.get_by_name(n) {
                doc.unlink_from_groups(member);
            }
        }
        doc.set_property(self.id, "Group", Property::LinkList(names))
            .map_err(PyValueError::new_err)
    }
}

// ---------------------------------------------------------------------------
// StringHasher / StringID
// ---------------------------------------------------------------------------

#[pyclass(name = "StringHasher", module = "ferrocad")]
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

#[pyclass(name = "StringID", module = "ferrocad")]
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
// Geometry types
// ---------------------------------------------------------------------------

/// A write-through view into a document geometry property.
///
/// FreeCAD exposes `obj.Placement` as a value copy, so mutating a sub-object
/// (`obj.Placement.Base.x = 5`) would be lost. To support that ergonomic
/// pattern we return *live* handles instead, recording the property version
/// the handle was derived from: a write only applies while the property's
/// version is unchanged. Reassigning the property therefore detaches handles
/// captured earlier, even when the new value compares equal (see
/// `Document.testNotification_Issue2902Part2`).
#[derive(Clone)]
struct GeometryView {
    inner: Arc<Mutex<CoreDocument>>,
    id: ObjectId,
    prop: String,
    /// The property value when the view was created (used to merge a
    /// sub-field like `Base`/`Rotation` back into the whole value).
    expect: Property,
    /// The property version when the view was created. A write is dropped once
    /// the property has been reassigned (its version changed), even if the new
    /// value compares equal.
    version: u64,
    kind: ViewKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewKind {
    /// The whole `PropertyPlacement`.
    Placement,
    /// The whole `PropertyRotation`.
    Rotation,
    /// The base vector inside a `PropertyPlacement`.
    PlacementBase,
    /// The rotation inside a `PropertyPlacement`.
    PlacementRotation,
}

impl GeometryView {
    /// A sub-view of the same property with a different field `kind`.
    fn child(&self, kind: ViewKind) -> GeometryView {
        GeometryView { kind, ..self.clone() }
    }

    /// Write this view's new `value` back to the property, unless the property
    /// has since been reassigned (in which case the view is detached).
    fn write(&self, value: Property) {
        let mut doc = self.inner.lock().unwrap();
        if doc.property_version(self.id, &self.prop) != self.version {
            return;
        }
        let new = match self.kind {
            ViewKind::Placement | ViewKind::Rotation => value,
            ViewKind::PlacementBase | ViewKind::PlacementRotation => {
                let Property::Placement(mut placement) = self.expect.clone() else {
                    return;
                };
                match (self.kind, value) {
                    (ViewKind::PlacementBase, Property::Vector(v)) => placement.base = v,
                    (ViewKind::PlacementRotation, Property::Rotation(r)) => placement.rotation = r,
                    _ => return,
                }
                Property::Placement(placement)
            }
        };
        let _ = doc.set_property(self.id, &self.prop, new);
    }
}

#[pyclass(name = "Vector", module = "ferrocad")]
#[derive(Clone)]
struct PyVector {
    inner: Vector3,
    /// Set when this vector is a live view of a geometry property.
    view: Option<GeometryView>,
}

impl PyVector {
    /// A detached vector value.
    fn fresh(inner: Vector3) -> Self {
        Self { inner, view: None }
    }

    /// Propagate a mutation to the backing geometry property, if any.
    fn write_back(&self) {
        if let Some(view) = &self.view {
            view.write(Property::Vector(self.inner));
        }
    }
}

#[pymethods]
impl PyVector {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let v = match args.len() {
            0 => Vector3::zero(),
            1 => {
                let a = args.get_item(0)?;
                if let Ok(v) = a.extract::<PyRef<'_, PyVector>>() {
                    v.inner
                } else if let Ok(seq) = a.extract::<Vec<f64>>() {
                    if seq.len() != 3 {
                        return Err(PyValueError::new_err("Vector sequence must have 3 items"));
                    }
                    Vector3::new(seq[0], seq[1], seq[2])
                } else {
                    return Err(PyTypeError::new_err(
                        "Vector() expects x,y,z, a Vector, or a 3-sequence",
                    ));
                }
            }
            _ => {
                let x: f64 = args.get_item(0)?.extract()?;
                let y: f64 = args.get_item(1)?.extract()?;
                let z: f64 = if args.len() > 2 { args.get_item(2)?.extract()? } else { 0.0 };
                Vector3::new(x, y, z)
            }
        };
        Ok(Self::fresh(v))
    }

    #[getter]
    fn x(&self) -> f64 { self.inner.x }
    #[setter]
    fn set_x(&mut self, v: f64) { self.inner.x = v; self.write_back(); }

    #[getter]
    fn y(&self) -> f64 { self.inner.y }
    #[setter]
    fn set_y(&mut self, v: f64) { self.inner.y = v; self.write_back(); }

    #[getter]
    fn z(&self) -> f64 { self.inner.z }
    #[setter]
    fn set_z(&mut self, v: f64) { self.inner.z = v; self.write_back(); }

    #[getter]
    fn Length(&self) -> f64 { self.inner.length() }
    #[setter]
    fn set_Length(&mut self, v: f64) {
        self.inner = self.inner.normalize().scale(v);
        self.write_back();
    }

    fn add(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector::fresh(self.inner.add(&o.inner)) }
    fn sub(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector::fresh(self.inner.sub(&o.inner)) }
    fn negative(&self) -> PyVector { PyVector::fresh(self.inner.neg()) }
    fn dot(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.dot(&o.inner) }
    fn cross(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector::fresh(self.inner.cross(&o.inner)) }
    fn normalize(&self) -> PyVector { PyVector::fresh(self.inner.normalize()) }
    fn distanceToPoint(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.distance(&o.inner) }
    fn getAngle(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.angle(&o.inner) }
    fn isEqual(&self, o: PyRef<'_, PyVector>, tol: f64) -> bool { self.inner.is_equal(&o.inner, tol) }

    fn __add__(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector::fresh(self.inner.add(&o.inner)) }
    fn __sub__(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector::fresh(self.inner.sub(&o.inner)) }
    fn __neg__(&self) -> PyVector { PyVector::fresh(self.inner.neg()) }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = other.py();
        if let Ok(f) = other.extract::<f64>() {
            return Ok(PyVector::fresh(self.inner.scale(f)).into_py_any(py).unwrap());
        }
        if let Ok(o) = other.extract::<PyRef<'_, PyVector>>() {
            return Ok(self.inner.dot(&o.inner).into_py_any(py).unwrap());
        }
        Err(PyTypeError::new_err("Vector can only multiply by a number or Vector"))
    }

    fn __rmul__(&self, f: f64) -> PyVector { PyVector::fresh(self.inner.scale(f)) }
    fn __truediv__(&self, f: f64) -> PyVector { PyVector::fresh(self.inner.scale(1.0 / f)) }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyVector>>() {
            Ok(o) => self.inner.is_equal(&o.inner, 1e-12),
            Err(_) => false,
        }
    }

    fn __len__(&self) -> usize { 3 }

    fn __getitem__(&self, i: isize) -> PyResult<f64> {
        let idx = if i < 0 { 3 + i } else { i };
        match idx {
            0 => Ok(self.inner.x),
            1 => Ok(self.inner.y),
            2 => Ok(self.inner.z),
            _ => Err(pyo3::exceptions::PyIndexError::new_err("vector index out of range")),
        }
    }

    fn __setitem__(&mut self, i: isize, v: f64) -> PyResult<()> {
        let idx = if i < 0 { 3 + i } else { i };
        match idx {
            0 => self.inner.x = v,
            1 => self.inner.y = v,
            2 => self.inner.z = v,
            _ => return Err(pyo3::exceptions::PyIndexError::new_err("vector index out of range")),
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!("Vector ({}, {}, {})", self.inner.x, self.inner.y, self.inner.z)
    }
}

#[pyclass(name = "Matrix", module = "ferrocad")]
#[derive(Clone, Copy)]
struct PyMatrix {
    inner: Matrix4,
}

/// Parse a FreeCAD `A<row><col>` element name (1-based) into a 0-based index.
fn parse_a_name(name: &str) -> Option<usize> {
    let b = name.as_bytes();
    if b.len() == 3 && b[0] == b'A' && (b'1'..=b'4').contains(&b[1]) && (b'1'..=b'4').contains(&b[2])
    {
        Some((b[1] - b'1') as usize * 4 + (b[2] - b'1') as usize)
    } else {
        None
    }
}

/// Extract a 3-vector from `(x, y, z)` or a `Vector` argument tuple.
fn extract_vec_args(args: &Bound<'_, PyTuple>) -> PyResult<Vector3> {
    if args.len() == 1 {
        if let Ok(v) = args.get_item(0)?.extract::<PyRef<'_, PyVector>>() {
            return Ok(v.inner);
        }
    }
    if args.len() == 3 {
        return Ok(Vector3::new(
            args.get_item(0)?.extract()?,
            args.get_item(1)?.extract()?,
            args.get_item(2)?.extract()?,
        ));
    }
    Err(PyTypeError::new_err("expected (x, y, z) or a Vector"))
}

#[pymethods]
impl PyMatrix {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let mut m = Matrix4::identity();
        match args.len() {
            0 => {}
            4 => {
                for i in 0..4 {
                    m.m[i] = args.get_item(i)?.extract()?;
                }
            }
            12 => {
                for i in 0..12 {
                    m.m[i] = args.get_item(i)?.extract()?;
                }
                m.m[12] = 0.0;
                m.m[13] = 0.0;
                m.m[14] = 0.0;
                m.m[15] = 1.0;
            }
            16 => {
                for i in 0..16 {
                    m.m[i] = args.get_item(i)?.extract()?;
                }
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Matrix() takes 0, 4, 12 or 16 arguments",
                ))
            }
        }
        Ok(Self { inner: m })
    }

    // -- elements -----------------------------------------------------------
    #[getter]
    fn A(&self) -> Vec<f64> {
        self.inner.m.to_vec()
    }

    fn __getattr__(&self, name: &Bound<'_, PyAny>) -> PyResult<f64> {
        let n: String = name.extract()?;
        parse_a_name(&n)
            .map(|i| self.inner.m[i])
            .ok_or_else(|| PyAttributeError::new_err(n))
    }

    fn __setattr__(&mut self, name: &Bound<'_, PyAny>, value: f64) -> PyResult<()> {
        let n: String = name.extract()?;
        match parse_a_name(&n) {
            Some(i) => {
                self.inner.m[i] = value;
                Ok(())
            }
            None => Err(PyAttributeError::new_err(n)),
        }
    }

    // `__setattr__` above cannot mutate `&self`, so expose explicit setters.
    fn setRow(&mut self, row: usize, v: PyRef<'_, PyVector>) -> PyResult<()> {
        if row > 3 {
            return Err(PyIndexError::new_err("row out of range"));
        }
        self.inner.set_row(row, v.inner);
        Ok(())
    }

    fn setCol(&mut self, col: usize, v: PyRef<'_, PyVector>) -> PyResult<()> {
        if col > 3 {
            return Err(PyIndexError::new_err("col out of range"));
        }
        self.inner.set_col(col, v.inner);
        Ok(())
    }

    fn row(&self, row: usize) -> PyResult<PyVector> {
        if row > 3 {
            return Err(PyIndexError::new_err("row out of range"));
        }
        Ok(PyVector::fresh(self.inner.row(row)))
    }

    fn col(&self, col: usize) -> PyResult<PyVector> {
        if col > 3 {
            return Err(PyIndexError::new_err("col out of range"));
        }
        Ok(PyVector::fresh(self.inner.col(col)))
    }

    fn diagonal(&self) -> PyVector {
        PyVector::fresh(self.inner.diagonal())
    }

    // -- predicates ---------------------------------------------------------
    #[pyo3(signature = (tol=0.0))]
    fn isUnity(&self, tol: f64) -> bool {
        self.inner.is_unity(tol)
    }

    fn isNull(&self) -> bool {
        self.inner.is_null()
    }

    fn unity(&mut self) {
        self.inner.unity();
    }

    fn nullify(&mut self) {
        self.inner.nullify();
    }

    fn determinant(&self) -> f64 {
        self.inner.determinant()
    }

    fn inverse(&self) -> PyResult<PyMatrix> {
        self.inner
            .inverse()
            .map(|inner| PyMatrix { inner })
            .ok_or_else(|| PyRuntimeError::new_err("matrix is singular"))
    }

    fn transpose(&self) -> PyMatrix {
        PyMatrix { inner: self.inner.transpose() }
    }

    /// Classify scaling; returns a `FreeCAD.ScaleType` member like upstream.
    #[pyo3(signature = (tol=0.0))]
    fn hasScale(&self, py: Python<'_>, tol: f64) -> PyResult<PyObject> {
        let value = self.inner.has_scale(tol) as i32;
        // Upstream builds the enum via `FreeCAD.ScaleType(int)`; fall back to a
        // plain int when the facade isn't importable (e.g. bare `fc` usage).
        match py.import("FreeCAD") {
            Ok(module) => Ok(module.getattr("ScaleType")?.call1((value,))?.into_py_any(py)?),
            Err(_) => Ok(value.into_py_any(py)?),
        }
    }

    /// `(shear, scale, rotation, move)` such that `self == move * rotation * scale * shear`.
    fn decompose(&self) -> (PyMatrix, PyMatrix, PyMatrix, PyMatrix) {
        let [shear, scale, rotation, mv] = self.inner.decompose();
        (
            PyMatrix { inner: shear },
            PyMatrix { inner: scale },
            PyMatrix { inner: rotation },
            PyMatrix { inner: mv },
        )
    }

    fn multiply(&self, o: PyRef<'_, PyMatrix>) -> PyMatrix {
        PyMatrix { inner: self.inner.mul(&o.inner) }
    }

    /// Transform a vector by this matrix (FreeCAD `Matrix.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector::fresh(self.inner.transform(&v.inner))
    }

    // -- in-place transforms (pre-multiply, matching FreeCAD) ---------------
    #[pyo3(name = "move", signature = (*args))]
    fn move_(&mut self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        self.inner.pre_move(extract_vec_args(args)?);
        Ok(())
    }

    #[pyo3(signature = (*args))]
    fn scale(&mut self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        let s = if args.len() == 1 {
            if let Ok(v) = args.get_item(0)?.extract::<PyRef<'_, PyVector>>() {
                v.inner
            } else {
                let f: f64 = args.get_item(0)?.extract()?;
                Vector3::new(f, f, f)
            }
        } else {
            extract_vec_args(args)?
        };
        self.inner.pre_scale(s);
        Ok(())
    }

    fn rotateX(&mut self, angle: f64) {
        self.inner.pre_rotate(0, angle);
    }
    fn rotateY(&mut self, angle: f64) {
        self.inner.pre_rotate(1, angle);
    }
    fn rotateZ(&mut self, angle: f64) {
        self.inner.pre_rotate(2, angle);
    }

    // -- operators ----------------------------------------------------------
    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        // Upstream `Matrix4D::operator==` compares element-wise within
        // `std::numeric_limits<double>::epsilon()` (not exact).
        match other.extract::<PyRef<'_, PyMatrix>>() {
            Ok(o) => self
                .inner
                .m
                .iter()
                .zip(o.inner.m.iter())
                .all(|(a, b)| (a - b).abs() <= f64::EPSILON),
            Err(_) => false,
        }
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = other.py();
        if let Ok(f) = other.extract::<f64>() {
            let mut m = self.inner.m;
            for v in m.iter_mut() {
                *v *= f;
            }
            return Ok(PyMatrix { inner: Matrix4 { m } }.into_py_any(py).unwrap());
        }
        if let Ok(o) = other.extract::<PyRef<'_, PyMatrix>>() {
            return Ok(PyMatrix { inner: self.inner.mul(&o.inner) }.into_py_any(py).unwrap());
        }
        if let Ok(v) = other.extract::<PyRef<'_, PyVector>>() {
            return Ok(PyVector::fresh(self.inner.transform(&v.inner)).into_py_any(py).unwrap());
        }
        if let Ok(r) = other.extract::<PyRef<'_, PyRotation>>() {
            return Ok(PyMatrix { inner: self.inner.mul(&r.inner.to_matrix()) }
                .into_py_any(py)
                .unwrap());
        }
        if let Ok(p) = other.extract::<PyRef<'_, PyPlacement>>() {
            return Ok(PyMatrix { inner: self.inner.mul(&p.inner.to_matrix()) }
                .into_py_any(py)
                .unwrap());
        }
        Err(PyNotImplementedError::new_err(
            "unsupported operand type(s) for *",
        ))
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyMatrix> {
        let o = other
            .extract::<PyRef<'_, PyMatrix>>()
            .map_err(|_| PyNotImplementedError::new_err("unsupported operand type(s) for +"))?;
        let mut m = [0.0; 16];
        for i in 0..16 {
            m[i] = self.inner.m[i] + o.inner.m[i];
        }
        Ok(PyMatrix { inner: Matrix4 { m } })
    }

    fn __sub__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyMatrix> {
        let o = other
            .extract::<PyRef<'_, PyMatrix>>()
            .map_err(|_| PyNotImplementedError::new_err("unsupported operand type(s) for -"))?;
        let mut m = [0.0; 16];
        for i in 0..16 {
            m[i] = self.inner.m[i] - o.inner.m[i];
        }
        Ok(PyMatrix { inner: Matrix4 { m } })
    }

    fn __neg__(&self) -> PyMatrix {
        let mut m = self.inner.m;
        for v in m.iter_mut() {
            *v = -*v;
        }
        PyMatrix { inner: Matrix4 { m } }
    }

    fn __pos__(&self) -> PyMatrix {
        *self
    }

    fn __bool__(&self) -> bool {
        true
    }

    fn __pow__(&self, e: &Bound<'_, PyAny>, _modulo: Option<&Bound<'_, PyAny>>) -> PyResult<PyMatrix> {
        let e: i32 = e
            .extract()
            .map_err(|_| PyNotImplementedError::new_err("unsupported operand type(s) for **"))?;
        if e == 0 {
            return Ok(PyMatrix { inner: Matrix4::identity() });
        }
        let base = if e < 0 {
            self.inner
                .inverse()
                .ok_or_else(|| PyRuntimeError::new_err("matrix is singular"))?
        } else {
            self.inner
        };
        let mut m = Matrix4::identity();
        for _ in 0..e.abs() {
            m = m.mul(&base);
        }
        Ok(PyMatrix { inner: m })
    }

    // Unsupported numeric protocol (mirrors FreeCAD's NotImplementedError).
    fn __truediv__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __mod__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __divmod__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __float__(&self) -> PyResult<f64> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __int__(&self) -> PyResult<i64> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __or__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __and__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __xor__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __lshift__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __rshift__(&self, _o: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __invert__(&self) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }
    fn __abs__(&self) -> PyResult<PyObject> {
        Err(PyNotImplementedError::new_err("unsupported operand"))
    }

    fn __repr__(&self) -> String {
        format!("Matrix ({:?})", &self.inner.m)
    }
}

#[pyclass(name = "Rotation", module = "ferrocad")]
#[derive(Clone)]
struct PyRotation {
    inner: Rotation,
    /// FreeCAD keeps the axis passed to `Axis`/`Angle` even at angle 0 (its
    /// `Rotation` stores `_axis`/`_angle` alongside the quaternion). The quaternion
    /// alone can't represent "axis X, angle 0", so cache it here.
    axis_cache: Option<Vector3>,
    /// Set when this rotation is a live view of a geometry property.
    view: Option<GeometryView>,
}

impl PyRotation {
    /// A detached rotation value.
    fn fresh(inner: Rotation, axis_cache: Option<Vector3>) -> Self {
        Self { inner, axis_cache, view: None }
    }

    /// Propagate a mutation to the backing geometry property, if any.
    fn write_back(&self) {
        if let Some(view) = &self.view {
            view.write(Property::Rotation(self.inner));
        }
    }
}

#[pymethods]
impl PyRotation {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let inner = match args.len() {
            0 => Rotation::identity(),
            1 => {
                let a = args.get_item(0)?;
                if let Ok(r) = a.extract::<PyRef<'_, PyRotation>>() {
                    r.inner
                } else if let Ok(m) = a.extract::<PyRef<'_, PyMatrix>>() {
                    Rotation::from_matrix(&m.inner)
                } else {
                    return Err(PyTypeError::new_err(
                        "Rotation() expects a Rotation or a Matrix",
                    ));
                }
            }
            2 => {
                // `Rotation(axis, degree)` — the angle is in degrees (FreeCAD).
                let axis: PyRef<'_, PyVector> = args.get_item(0)?.extract()?;
                let degree: f64 = args.get_item(1)?.extract()?;
                Rotation::from_axis_angle(&axis.inner, degree.to_radians())
            }
            3 => {
                let yaw: f64 = args.get_item(0)?.extract()?;
                let pitch: f64 = args.get_item(1)?.extract()?;
                let roll: f64 = args.get_item(2)?.extract()?;
                Rotation::from_euler_deg(yaw, pitch, roll)
            }
            4 => {
                // `Rotation(x, y, z, w)`.
                let x: f64 = args.get_item(0)?.extract()?;
                let y: f64 = args.get_item(1)?.extract()?;
                let z: f64 = args.get_item(2)?.extract()?;
                let w: f64 = args.get_item(3)?.extract()?;
                Rotation { q: [w, x, y, z], raw_axis: None }
            }
            16 => {
                let mut m = Matrix4::identity();
                for i in 0..16 {
                    m.m[i] = args.get_item(i)?.extract()?;
                }
                Rotation::from_matrix(&m)
            }
            _ => {
                return Err(PyTypeError::new_err(
                    "Rotation() takes 0, 1, 2, 3, 4 or 16 arguments",
                ))
            }
        };
        Ok(Self::fresh(inner, None))
    }

    #[getter]
    fn Angle(&self) -> f64 {
        self.inner.angle()
    }

    #[getter]
    fn Axis(&self) -> PyVector {
        // `Axis` is normalized; `RawAxis` (below) is not.
        PyVector::fresh(self.axis_cache.unwrap_or_else(|| self.inner.axis()).normalize())
    }

    #[setter]
    fn set_Angle(&mut self, angle: f64) {
        let axis = self.axis_cache.unwrap_or_else(|| self.inner.axis());
        self.inner = Rotation::from_axis_angle(&axis, angle);
        self.inner.raw_axis = Some(axis);
        self.write_back();
    }

    #[setter]
    fn set_Axis(&mut self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = extract_vector(v)?;
        let angle = self.inner.angle();
        self.axis_cache = Some(axis);
        self.inner = Rotation::from_axis_angle(&axis, angle);
        // FreeCAD keeps the raw (unnormalized) axis for `RawAxis`.
        self.inner.raw_axis = Some(axis);
        self.write_back();
        Ok(())
    }

    #[getter]
    fn RawAxis(&self) -> PyVector {
        PyVector::fresh(self.axis_cache.unwrap_or_else(|| self.inner.axis()))
    }

    #[getter]
    fn Q(&self) -> (f64, f64, f64, f64) {
        (self.inner.q[1], self.inner.q[2], self.inner.q[3], self.inner.q[0])
    }

    #[setter]
    fn set_Q(&mut self, v: (f64, f64, f64, f64)) {
        self.inner.q = [v.3, v.0, v.1, v.2];
        self.inner.raw_axis = None;
        self.axis_cache = None;
        self.write_back();
    }

    #[getter]
    fn Matrix(&self) -> PyMatrix {
        PyMatrix { inner: self.inner.to_matrix() }
    }

    #[setter]
    fn set_Matrix(&mut self, m: PyRef<'_, PyMatrix>) {
        self.inner = Rotation::from_matrix(&m.inner);
        self.axis_cache = None;
    }

    /// FreeCAD custom attribute: `Rotation.Axes = (fromVec, toVec)`.
    #[setter]
    fn set_Axes(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let a = value.get_item(0)?;
        let b = value.get_item(1)?;
        self.inner = Rotation::from_vectors(extract_vector(&a)?, extract_vector(&b)?);
        self.axis_cache = None;
        Ok(())
    }

    // FreeCAD custom attributes: `Rotation.Yaw`/`Pitch`/`Roll` (degrees).
    #[setter]
    fn set_Yaw(&mut self, v: f64) {
        let (_, p, r) = self.getYawPitchRoll();
        self.inner = Rotation::from_euler_deg(v, p, r);
    }

    #[setter]
    fn set_Pitch(&mut self, v: f64) {
        let (y, _, r) = self.getYawPitchRoll();
        self.inner = Rotation::from_euler_deg(y, v, r);
    }

    #[setter]
    fn set_Roll(&mut self, v: f64) {
        let (y, p, _) = self.getYawPitchRoll();
        self.inner = Rotation::from_euler_deg(y, p, v);
    }

    fn toMatrix(&self) -> PyMatrix {
        PyMatrix { inner: self.inner.to_matrix() }
    }

    fn multiply(&self, o: PyRef<'_, PyRotation>) -> PyRotation {
        PyRotation::fresh(self.inner.multiply(&o.inner), None)
    }

    fn invert(&mut self) {
        self.inner = self.inner.inverse();
        self.axis_cache = None;
    }

    fn inverse(&self) -> PyRotation {
        PyRotation::fresh(self.inner.inverse(), None)
    }

    #[pyo3(signature = (o, tol=0.0))]
    fn isSame(&self, o: PyRef<'_, PyRotation>, tol: f64) -> bool {
        self.inner.is_same(&o.inner, tol)
    }

    fn setYawPitchRoll(&mut self, yaw: f64, pitch: f64, roll: f64) {
        self.inner = Rotation::from_euler_deg(yaw, pitch, roll);
        self.axis_cache = None;
    }

    fn getYawPitchRoll(&self) -> (f64, f64, f64) {
        let (y, p, r) = self.inner.yaw_pitch_roll();
        (y.to_degrees(), p.to_degrees(), r.to_degrees())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyRotation>>() {
            Ok(o) => self.inner == o.inner,
            Err(_) => false,
        }
    }

    fn __repr__(&self) -> String {
        format!("Rotation ({:?})", self.inner.q)
    }
}

#[pyclass(name = "Placement", module = "ferrocad")]
#[derive(Clone)]
struct PyPlacement {
    inner: Placement,
    /// Set when this placement is a live view of a geometry property.
    view: Option<GeometryView>,
}

impl PyPlacement {
    /// A detached placement value.
    fn fresh(inner: Placement) -> Self {
        Self { inner, view: None }
    }

    /// Propagate a mutation to the backing geometry property, if any.
    fn write_back(&self) {
        if let Some(view) = &self.view {
            view.write(Property::Placement(self.inner));
        }
    }
}

#[pymethods]
impl PyPlacement {
    #[new]
    #[pyo3(signature = (base=None, rotation=None))]
    fn new(base: Option<&Bound<'_, PyAny>>, rotation: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        // `Placement(matrix)` builds from a 4x4 transformation matrix.
        if rotation.is_none() {
            if let Some(b) = base {
                if let Ok(m) = b.extract::<PyRef<'_, PyMatrix>>() {
                    return Ok(Self::fresh(Placement::new(
                        Vector3::new(m.inner.m[3], m.inner.m[7], m.inner.m[11]),
                        Rotation::from_matrix(&m.inner),
                    )));
                }
            }
        }
        let b = match base {
            Some(v) => extract_vector(v)?,
            None => Vector3::zero(),
        };
        let r = match rotation {
            Some(q) => extract_rotation(q)?,
            None => Rotation::identity(),
        };
        Ok(Self::fresh(Placement::new(b, r)))
    }

    fn inverse(&self) -> PyPlacement {
        PyPlacement::fresh(self.inner.inverse())
    }

    fn toMatrix(&self) -> PyMatrix {
        PyMatrix { inner: self.inner.to_matrix() }
    }

    #[pyo3(signature = (o, tol=0.0))]
    fn isSame(&self, o: PyRef<'_, PyPlacement>, tol: f64) -> bool {
        self.inner.is_same(&o.inner, tol)
    }

    #[getter]
    fn Base(&self) -> PyVector {
        let mut v = PyVector::fresh(self.inner.base);
        v.view = self.view.as_ref().map(|view| view.child(ViewKind::PlacementBase));
        v
    }
    #[setter]
    fn set_Base(&mut self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        self.inner.base = extract_vector(v)?;
        self.write_back();
        Ok(())
    }

    #[getter]
    fn Rotation(&self) -> PyRotation {
        let mut r = PyRotation::fresh(self.inner.rotation, None);
        r.view = self.view.as_ref().map(|view| view.child(ViewKind::PlacementRotation));
        r
    }
    #[setter]
    fn set_Rotation(&mut self, q: &Bound<'_, PyAny>) -> PyResult<()> {
        self.inner.rotation = extract_rotation(q)?;
        self.write_back();
        Ok(())
    }

    fn __mul__(&self, o: PyRef<'_, PyPlacement>) -> PyPlacement { PyPlacement::fresh(self.inner.mul(&o.inner)) }

    /// Transform a vector by this placement (FreeCAD `Placement.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector::fresh(self.inner.to_matrix().transform(&v.inner))
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyPlacement>>() {
            Ok(o) => self.inner == o.inner,
            Err(_) => false,
        }
    }

    fn __repr__(&self) -> String {
        format!("Placement ({:?}, {:?})", self.inner.base, self.inner.rotation.q)
    }
}

#[pyclass(name = "TypeId", module = "ferrocad")]
#[derive(Clone)]
struct PyTypeId {
    inner: TypeId,
}

#[pymethods]
impl PyTypeId {
    #[new]
    fn new(name: &str) -> Self {
        Self { inner: TypeId::from_name(name) }
    }

    #[classmethod]
    fn fromName(_cls: &Bound<'_, PyType>, name: &str) -> PyTypeId {
        PyTypeId { inner: TypeId::from_name(name) }
    }

    #[classmethod]
    fn getAllDerivedFrom(_cls: &Bound<'_, PyType>, _parent: Option<&Bound<'_, PyAny>>) -> Vec<String> {
        vec![]
    }

    #[getter]
    fn Name(&self) -> String {
        self.inner.name().to_string()
    }

    fn createInstance(&self) -> Option<PyObject> {
        None
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyTypeId>>() {
            Ok(o) => self.inner == o.inner,
            Err(_) => false,
        }
    }

    fn __repr__(&self) -> String {
        format!("TypeId({})", self.inner.name())
    }
}

// ---------------------------------------------------------------------------
// Vector2d / Material / BoundBox
// ---------------------------------------------------------------------------

#[pyclass(name = "Vector2d", module = "ferrocad")]
#[derive(Clone, Copy)]
struct PyVector2d {
    x: f64,
    y: f64,
}

#[pymethods]
impl PyVector2d {
    #[new]
    #[pyo3(signature = (x=0.0, y=0.0))]
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[getter]
    fn x(&self) -> f64 {
        self.x
    }
    #[setter]
    fn set_x(&mut self, v: f64) {
        self.x = v;
    }
    #[getter]
    fn y(&self) -> f64 {
        self.y
    }
    #[setter]
    fn set_y(&mut self, v: f64) {
        self.y = v;
    }

    fn rotate(&mut self, angle: f64) {
        let (s, c) = angle.sin_cos();
        let (x, y) = (self.x, self.y);
        self.x = x * c - y * s;
        self.y = x * s + y * c;
    }

    fn __repr__(&self) -> String {
        format!("Vector2d ({}, {})", self.x, self.y)
    }
}

#[pyclass(name = "Material", module = "ferrocad")]
#[derive(Clone, PartialEq)]
struct PyMaterial {
    diffuse: [f64; 4],
}

#[pymethods]
impl PyMaterial {
    #[new]
    fn new() -> Self {
        Self { diffuse: [0.8, 0.8, 0.8, 1.0] }
    }

    #[getter]
    fn DiffuseColor(&self) -> (f64, f64, f64, f64) {
        (self.diffuse[0], self.diffuse[1], self.diffuse[2], self.diffuse[3])
    }

    #[setter]
    fn set_DiffuseColor(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok((r, g, b, a)) = value.extract::<(f64, f64, f64, f64)>() {
            self.diffuse = [r, g, b, a];
        } else if let Ok((r, g, b)) = value.extract::<(f64, f64, f64)>() {
            self.diffuse = [r, g, b, 1.0];
        } else {
            return Err(PyTypeError::new_err("DiffuseColor expects (r, g, b[, a])"));
        }
        Ok(())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyMaterial>>() {
            Ok(o) => self.diffuse == o.diffuse,
            Err(_) => false,
        }
    }

    fn __ne__(&self, other: &Bound<'_, PyAny>) -> bool {
        !self.__eq__(other)
    }
}

#[pyclass(name = "BoundBox", module = "ferrocad")]
#[derive(Clone)]
struct PyBoundBox {
    min: [f64; 3],
    max: [f64; 3],
    valid: bool,
}

impl PyBoundBox {
    fn add_point(&mut self, p: [f64; 3]) {
        if self.valid {
            for i in 0..3 {
                self.min[i] = self.min[i].min(p[i]);
                self.max[i] = self.max[i].max(p[i]);
            }
        } else {
            self.min = p;
            self.max = p;
            self.valid = true;
        }
    }

    fn contains(&self, p: [f64; 3]) -> bool {
        self.valid && (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
}

#[pymethods]
impl PyBoundBox {
    #[new]
    #[pyo3(signature = (xmin=None, ymin=None, zmin=None, xmax=None, ymax=None, zmax=None))]
    fn new(
        xmin: Option<f64>,
        ymin: Option<f64>,
        zmin: Option<f64>,
        xmax: Option<f64>,
        ymax: Option<f64>,
        zmax: Option<f64>,
    ) -> Self {
        match (xmin, ymin, zmin, xmax, ymax, zmax) {
            (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) => Self {
                min: [a, b, c],
                max: [d, e, f],
                valid: true,
            },
            _ => Self {
                min: [0.0; 3],
                max: [0.0; 3],
                valid: false,
            },
        }
    }

    fn setVoid(&mut self) {
        self.valid = false;
    }

    fn isValid(&self) -> bool {
        self.valid
    }

    #[pyo3(signature = (x, y, z))]
    fn add(&mut self, x: &Bound<'_, PyAny>, y: Option<f64>, z: Option<f64>) -> PyResult<()> {
        if let Ok(v) = x.extract::<PyRef<'_, PyVector>>() {
            self.add_point([v.inner.x, v.inner.y, v.inner.z]);
            return Ok(());
        }
        let x = x.extract::<f64>()?;
        self.add_point([x, y.unwrap_or(0.0), z.unwrap_or(0.0)]);
        Ok(())
    }

    #[getter]
    fn XLength(&self) -> f64 {
        self.max[0] - self.min[0]
    }
    #[getter]
    fn YLength(&self) -> f64 {
        self.max[1] - self.min[1]
    }
    #[getter]
    fn ZLength(&self) -> f64 {
        self.max[2] - self.min[2]
    }

    #[getter]
    fn Center(&self) -> PyVector {
        PyVector::fresh(Vector3::new(
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.max[2]) / 2.0,
        ))
    }

    fn isInside(&self, point: PyRef<'_, PyVector>) -> bool {
        self.contains([point.inner.x, point.inner.y, point.inner.z])
    }

    /// The first box-surface point hit by a ray from `point` along `direction`.
    fn getIntersectionPoint(&self, point: PyRef<'_, PyVector>, direction: PyRef<'_, PyVector>) -> PyVector {
        let p = point.inner;
        let d = direction.inner;
        let mut t = f64::INFINITY;
        for i in 0..3 {
            let (pi, di) = ([p.x, p.y, p.z][i], [d.x, d.y, d.z][i]);
            if di.abs() > 1e-15 {
                for bound in [self.min[i], self.max[i]] {
                    let ti = (bound - pi) / di;
                    if ti >= 0.0 {
                        t = t.min(ti);
                    }
                }
            }
        }
        if t.is_finite() {
            PyVector::fresh(Vector3::new(p.x + t * d.x, p.y + t * d.y, p.z + t * d.z))
        } else {
            PyVector::fresh(p)
        }
    }

    fn intersect(&self, other: PyRef<'_, PyBoundBox>) -> bool {
        self.valid
            && other.valid
            && (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i])
    }

    fn intersected(&self, other: PyRef<'_, PyBoundBox>) -> PyBoundBox {
        let overlaps = self.valid
            && other.valid
            && (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i]);
        if !overlaps {
            return PyBoundBox { min: [0.0; 3], max: [0.0; 3], valid: false };
        }
        let mut out = PyBoundBox { min: [0.0; 3], max: [0.0; 3], valid: true };
        for i in 0..3 {
            out.min[i] = self.min[i].max(other.min[i]);
            out.max[i] = self.max[i].min(other.max[i]);
        }
        out
    }

    fn __repr__(&self) -> String {
        format!("BoundBox ({:?}, {:?})", self.min, self.max)
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (name=None))]
fn newDocument(py: Python<'_>, name: Option<String>) -> PyResult<Py<PyDocument>> {
    let name = name.unwrap_or_else(|| "Unnamed".to_string());
    let doc = Py::new(
        py,
        PyDocument {
            label: name.clone(),
            name,
            file_name: None,
            auto_created: false,
            inner: Arc::new(Mutex::new(CoreDocument::new())),
            meta: Arc::new(Mutex::new(BTreeMap::new())),
            comment: Mutex::new(String::new()),
        },
    )?;
    let bound = doc.bind(py);
    fire_doc("slotCreatedDocument", bound, None);
    fire_doc_str("slotBeforeChangeDocument", bound, "Label");
    fire_doc_str("slotChangedDocument", bound, "Label");
    fire_doc("slotRelabelDocument", bound, None);
    Ok(doc)
}

#[pyfunction]
fn openDocument(py: Python<'_>, path: &str) -> PyResult<Py<PyDocument>> {
    let saved = CoreDocument::load_from_file(path).map_err(PyValueError::new_err)?;
    let doc = Py::new(
        py,
        PyDocument {
            name: saved.name.clone(),
            label: saved.name.clone(),
            file_name: Some(path.to_string()),
            auto_created: false,
            inner: Arc::new(Mutex::new(CoreDocument::from_saved(&saved))),
            meta: Arc::new(Mutex::new(BTreeMap::new())),
            comment: Mutex::new(String::new()),
        },
    )?;
    let inner = Arc::clone(&doc.bind(py).borrow().inner);
    apply_all_python_states(py, &doc, &inner);
    Ok(doc)
}

#[pyfunction]
fn addDocumentObserver(observer: Py<PyAny>) {
    let mut obs = observers().lock().unwrap();
    if !obs.iter().any(|o| o.as_ptr() == observer.as_ptr()) {
        obs.push(observer);
    }
}

#[pyfunction]
fn removeDocumentObserver(observer: &Bound<'_, PyAny>) {
    observers()
        .lock()
        .unwrap()
        .retain(|o| o.as_ptr() != observer.as_ptr());
}

/// Emit a document-level signal (used by the `FreeCAD` facade for the
/// operations it owns: `closeDocument`, `setActiveDocument`).
#[pyfunction]
#[pyo3(signature = (slot, doc, extra=None))]
fn _emitDocument(slot: &str, doc: &Bound<'_, PyDocument>, extra: Option<&Bound<'_, PyAny>>) {
    fire_doc(slot, doc, extra);
}

/// Drop all cached Python object handles for a closed document.
#[pyfunction]
fn _forgetDocument(doc: &Bound<'_, PyDocument>) {
    forget_document(&doc.clone().unbind());
}

#[pymodule]
pub fn ferrocad(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PyQuantity>()?;
    m.add_class::<PyUnit>()?;
    m.add_class::<PyVector>()?;
    m.add_class::<PyMatrix>()?;
    m.add_class::<PyRotation>()?;
    m.add_class::<PyPlacement>()?;
    m.add_class::<PyTypeId>()?;
    m.add_class::<PyVector2d>()?;
    m.add_class::<PyMaterial>()?;
    m.add_class::<PyBoundBox>()?;
    m.add_class::<PyDocument>()?;
    m.add_class::<PyDocumentObject>()?;
    m.add_class::<PyDocumentSettings>()?;
    m.add_class::<PyStringHasher>()?;
    m.add_class::<PyStringID>()?;
    m.add_function(wrap_pyfunction!(newDocument, m)?)?;
    m.add_function(wrap_pyfunction!(openDocument, m)?)?;
    m.add_function(wrap_pyfunction!(addDocumentObserver, m)?)?;
    m.add_function(wrap_pyfunction!(removeDocumentObserver, m)?)?;
    m.add_function(wrap_pyfunction!(_emitDocument, m)?)?;
    m.add_function(wrap_pyfunction!(_forgetDocument, m)?)?;
    Ok(())
}
