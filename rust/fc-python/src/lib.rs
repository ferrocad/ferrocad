//! # fc-python (M3b)
//!
//! PyO3 bindings exposing the `fc-core` document/object model (properties,
//! quantities, expressions, transactions, recompute) to Python. This is the
//! bridge that replaces the hand-written `FreeCAD` facade from M0/M1.

#![allow(non_snake_case)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use fc_core::{canonical_name, parse_unit, Document as CoreDocument, Matrix4, ObjectId, Placement, Property, Quantity, Rotation, StringHasher, StringId, TypeId, Unit, Vector3};
use pyo3::exceptions::{PyAttributeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PyBool, PyDict, PyDictMethods, PyTuple, PyType};
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
// Unit
// ---------------------------------------------------------------------------

#[pyclass(name = "Unit", module = "fc")]
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

#[pyclass(name = "Quantity", module = "fc")]
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
        Property::Vector(v) => PyVector { inner: *v }.into_py_any(py).unwrap(),
        Property::VectorList(v) => v
            .iter()
            .map(|x| PyVector { inner: *x }.into_py_any(py).unwrap())
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::Placement(p) => PyPlacement { inner: *p }.into_py_any(py).unwrap(),
        Property::PlacementList(v) => v
            .iter()
            .map(|x| PyPlacement { inner: *x }.into_py_any(py).unwrap())
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
        Property::Rotation(r) => PyRotation { inner: *r }.into_py_any(py).unwrap(),
        Property::RotationList(v) => v
            .iter()
            .map(|x| PyRotation { inner: *x }.into_py_any(py).unwrap())
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

/// Map a FreeCAD property type id to its default value.
fn default_property(type_id: &str) -> Property {
    let t = type_id.to_ascii_lowercase();
    if t.ends_with("placementlist") {
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

fn unique_name(doc: &CoreDocument, name: &str) -> String {
    if doc.get_by_name(name).is_none() {
        return name.to_string();
    }
    let mut i = 1;
    loop {
        let candidate = format!("{}{:03}", name, i);
        if doc.get_by_name(&candidate).is_none() {
            return candidate;
        }
        i += 1;
    }
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
        let name = unique_name(doc, base);
        let child_id = doc.add_object(&name, ty);
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
            return Ok(Rotation { q: [seq[0], seq[1], seq[2], seq[3]] });
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
        return Ok(o.borrow().Name());
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
        return Ok(v.iter().map(|o| o.Name()).collect());
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
        o.borrow().Name()
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
    let name = slf.borrow().Name();
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

#[pyclass(name = "DocumentSettings", module = "fc")]
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

#[pyclass(name = "Document", module = "fc")]
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

#[pyclass(name = "DocumentObject", module = "fc", dict)]
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

    #[pyo3(signature = (type_id, name=None))]
    fn addObject(
        slf: Bound<'_, Self>,
        type_id: &str,
        name: Option<String>,
    ) -> PyResult<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let name = name.unwrap_or_default();
        let (id, tx) = {
            let mut doc = inner.lock().unwrap();
            let tx = doc.begin_transaction_if_pending();
            let id = doc.add_object(&name, type_id);
            if type_id == "App::Origin" {
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
        let (flagged, count) = {
            let mut doc = inner.lock().unwrap();
            let flagged = doc.take_must_execute();
            let count = doc.recompute().map_err(PyValueError::new_err)?;
            (flagged, count)
        };
        for id in flagged {
            let obj = get_or_create_object(py, &doc_py, &inner, id);
            fire_obj("slotRecomputedObject", obj.bind(py), None);
        }
        fire_doc("slotRecomputedDocument", slf, None);
        Ok(count)
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
        let name = slf.borrow().name.clone();
        fire_doc_str("slotStartSaveDocument", slf, path);
        slf.borrow()
            .inner
            .lock()
            .unwrap()
            .save_to_file(&name, path)
            .map_err(PyValueError::new_err)?;
        slf.borrow_mut().file_name = Some(path.to_string());
        fire_doc_str("slotFinishSaveDocument", slf, path);
        Ok(())
    }

    fn save(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (name, path) = {
            let this = slf.borrow();
            (this.name.clone(), this.file_name.clone())
        };
        let path = path
            .ok_or_else(|| PyValueError::new_err("document has no file name; use saveAs first"))?;
        fire_doc_str("slotStartSaveDocument", slf, &path);
        slf.borrow()
            .inner
            .lock()
            .unwrap()
            .save_to_file(&name, &path)
            .map_err(PyValueError::new_err)?;
        fire_doc_str("slotFinishSaveDocument", slf, &path);
        Ok(())
    }

    fn load(slf: &Bound<'_, Self>, path: &str) -> PyResult<()> {
        let saved = CoreDocument::load_from_file(path).map_err(PyValueError::new_err)?;
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        forget_document(&doc_py);
        let mut this = slf.borrow_mut();
        this.name = saved.name.clone();
        this.label = saved.name.clone();
        this.file_name = Some(path.to_string());
        this.inner = Arc::new(Mutex::new(CoreDocument::from_saved(&saved)));
        this.meta = Arc::new(Mutex::new(BTreeMap::new()));
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
        let mut copied: Vec<Py<PyDocumentObject>> = Vec::new();
        {
            let mut doc = inner.lock().unwrap();
            for (name, label, type_id, props, exprs) in &sources {
                let unique = unique_name(&doc, name);
                let id = doc.add_object(&unique, type_id);
                doc.set_label(id, label);
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

    // -- undo/redo metadata (POC: not tracked yet) ---------------------------
    #[getter]
    fn UndoNames(&self) -> Vec<String> {
        vec![]
    }

    #[getter]
    fn RedoNames(&self) -> Vec<String> {
        vec![]
    }

    #[getter]
    fn UndoCount(&self) -> usize {
        0
    }

    #[getter]
    fn RedoCount(&self) -> usize {
        0
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
    ) -> Vec<Py<PyDocumentObject>> {
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
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect()
    }

    #[getter]
    fn ActiveObject(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let id = inner.lock().unwrap().object_ids().into_iter().last()?;
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
        0
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

    /// `ViewObject` is only present when a GUI is up; headless → `None`.
    #[getter]
    fn ViewObject(&self) -> Option<PyObject> {
        None
    }

    /// Whether the object must be recomputed. POC: never (recompute is a no-op).
    #[getter]
    fn MustExecute(&self) -> bool {
        false
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

    /// FreeCAD alias: `getTypeOfProperty` returns the property's type flags
    /// (the POC returns the type id, which round-trips for the tests).
    fn getTypeOfProperty(&self, name: &str) -> Option<String> {
        self.getTypeIdOfProperty(name)
    }

    #[pyo3(signature = (type_id, name, group="", doc="", attr=0, read_only=false, hidden=false, locked=false))]
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
    ) -> PyResult<()> {
        let _ = (group, doc, attr, read_only, hidden, locked);
        if name.is_empty() {
            return Err(PyValueError::new_err("property name must not be empty"));
        }
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        let default = default_property(type_id);
        inner
            .lock()
            .unwrap()
            .set_property(id, name, default)
            .map_err(PyValueError::new_err)?;
        fire_obj_str("slotAppendDynamicProperty", slf, name);
        Ok(())
    }

    fn setExpression(&self, prop: &str, source: &str) -> PyResult<()> {
        self.inner
            .lock()
            .unwrap()
            .set_expression(self.id, prop, source)
            .map_err(PyValueError::new_err)
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
        let other_name = other_ref.Name();
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
        let name = other_ref.Name();
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

    /// `InList`: the objects that link to this object.
    #[getter]
    fn InList(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py = slf.borrow().doc.clone_ref(py);
        let name = slf.borrow().Name();
        let ids: Vec<ObjectId> = {
            let doc = inner.lock().unwrap();
            doc.object_ids()
                .into_iter()
                .filter(|id| {
                    if doc.object(*id).map(|o| o.name == name).unwrap_or(false) {
                        return false;
                    }
                    matches!(doc.object(*id).and_then(|o| o.properties.get("Group")),
                        Some(Property::LinkList(links)) if links.contains(&name))
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
                3 => PyPlacement { inner: *pl }.into_py_any(py).unwrap(),
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
        if name == "Group" {
            return Ok(slf.borrow().group_members(py).into_py_any(py).unwrap());
        }
        let (value, inner, doc_py) = {
            let this = slf.borrow();
            let doc = this.inner.lock().unwrap();
            let value = doc.object(this.id).and_then(|o| o.properties.get(&name)).cloned();
            (value, Arc::clone(&this.inner), this.doc.clone_ref(py))
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
            Some(p) => Ok(property_to_py(py, &p)),
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
            objs.iter().map(|o| o.Name()).collect()
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
// Geometry types
// ---------------------------------------------------------------------------

#[pyclass(name = "Vector", module = "fc")]
#[derive(Clone, Copy)]
struct PyVector {
    inner: Vector3,
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
        Ok(Self { inner: v })
    }

    #[getter]
    fn x(&self) -> f64 { self.inner.x }
    #[setter]
    fn set_x(&mut self, v: f64) { self.inner.x = v; }

    #[getter]
    fn y(&self) -> f64 { self.inner.y }
    #[setter]
    fn set_y(&mut self, v: f64) { self.inner.y = v; }

    #[getter]
    fn z(&self) -> f64 { self.inner.z }
    #[setter]
    fn set_z(&mut self, v: f64) { self.inner.z = v; }

    #[getter]
    fn Length(&self) -> f64 { self.inner.length() }
    #[setter]
    fn set_Length(&mut self, v: f64) {
        self.inner = self.inner.normalize().scale(v);
    }

    fn add(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector { inner: self.inner.add(&o.inner) } }
    fn sub(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector { inner: self.inner.sub(&o.inner) } }
    fn negative(&self) -> PyVector { PyVector { inner: self.inner.neg() } }
    fn dot(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.dot(&o.inner) }
    fn cross(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector { inner: self.inner.cross(&o.inner) } }
    fn normalize(&self) -> PyVector { PyVector { inner: self.inner.normalize() } }
    fn distanceToPoint(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.distance(&o.inner) }
    fn getAngle(&self, o: PyRef<'_, PyVector>) -> f64 { self.inner.angle(&o.inner) }
    fn isEqual(&self, o: PyRef<'_, PyVector>, tol: f64) -> bool { self.inner.is_equal(&o.inner, tol) }

    fn __add__(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector { inner: self.inner.add(&o.inner) } }
    fn __sub__(&self, o: PyRef<'_, PyVector>) -> PyVector { PyVector { inner: self.inner.sub(&o.inner) } }
    fn __neg__(&self) -> PyVector { PyVector { inner: self.inner.neg() } }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = other.py();
        if let Ok(f) = other.extract::<f64>() {
            return Ok(PyVector { inner: self.inner.scale(f) }.into_py_any(py).unwrap());
        }
        if let Ok(o) = other.extract::<PyRef<'_, PyVector>>() {
            return Ok(self.inner.dot(&o.inner).into_py_any(py).unwrap());
        }
        Err(PyTypeError::new_err("Vector can only multiply by a number or Vector"))
    }

    fn __rmul__(&self, f: f64) -> PyVector { PyVector { inner: self.inner.scale(f) } }
    fn __truediv__(&self, f: f64) -> PyVector { PyVector { inner: self.inner.scale(1.0 / f) } }

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

#[pyclass(name = "Matrix", module = "fc")]
#[derive(Clone, Copy)]
struct PyMatrix {
    inner: Matrix4,
}

#[pymethods]
impl PyMatrix {
    #[new]
    fn new() -> Self {
        Self { inner: Matrix4::identity() }
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyMatrix>>() {
            Ok(o) => self.inner == o.inner,
            Err(_) => false,
        }
    }

    fn multiply(&self, o: PyRef<'_, PyMatrix>) -> PyMatrix { PyMatrix { inner: self.inner.mul(&o.inner) } }

    /// Transform a vector by this matrix (FreeCAD `Matrix.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector { inner: self.inner.transform(&v.inner) }
    }

    fn __mul__(&self, o: PyRef<'_, PyMatrix>) -> PyMatrix { PyMatrix { inner: self.inner.mul(&o.inner) } }

    fn __repr__(&self) -> String {
        let m = &self.inner.m;
        format!("Matrix ({:?})", m)
    }
}

#[pyclass(name = "Rotation", module = "fc")]
#[derive(Clone, Copy)]
struct PyRotation {
    inner: Rotation,
}

#[pymethods]
impl PyRotation {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Ok(Self { inner: Rotation::identity() }),
            2 => {
                let axis: PyRef<'_, PyVector> = args.get_item(0)?.extract()?;
                let angle: f64 = args.get_item(1)?.extract()?;
                Ok(Self { inner: Rotation::from_axis_angle(&axis.inner, angle) })
            }
            _ => Err(PyTypeError::new_err(
                "Rotation() takes 0 or 2 arguments (axis, angle)",
            )),
        }
    }

    #[getter]
    fn Angle(&self) -> f64 { self.inner.angle() }

    #[getter]
    fn Axis(&self) -> PyVector {
        let q = self.inner.q;
        let s = (1.0 - q[0] * q[0]).sqrt();
        let axis = if s < 1e-12 {
            Vector3::new(0.0, 0.0, 1.0)
        } else {
            Vector3::new(q[1] / s, q[2] / s, q[3] / s)
        };
        PyVector { inner: axis }
    }

    #[setter]
    fn set_Axis(&mut self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = extract_vector(v)?;
        let angle = self.inner.angle();
        self.inner = Rotation::from_axis_angle(&axis, angle);
        Ok(())
    }

    #[getter]
    fn RawAxis(&self) -> PyVector {
        self.Axis()
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

#[pyclass(name = "Placement", module = "fc")]
#[derive(Clone, Copy)]
struct PyPlacement {
    inner: Placement,
}

#[pymethods]
impl PyPlacement {
    #[new]
    #[pyo3(signature = (base=None, rotation=None))]
    fn new(base: Option<&Bound<'_, PyAny>>, rotation: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let b = match base {
            Some(v) => extract_vector(v)?,
            None => Vector3::zero(),
        };
        let r = match rotation {
            Some(q) => extract_rotation(q)?,
            None => Rotation::identity(),
        };
        Ok(Self { inner: Placement::new(b, r) })
    }

    #[getter]
    fn Base(&self) -> PyVector { PyVector { inner: self.inner.base } }
    #[setter]
    fn set_Base(&mut self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        self.inner.base = extract_vector(v)?;
        Ok(())
    }

    #[getter]
    fn Rotation(&self) -> PyRotation { PyRotation { inner: self.inner.rotation } }
    #[setter]
    fn set_Rotation(&mut self, q: &Bound<'_, PyAny>) -> PyResult<()> {
        self.inner.rotation = extract_rotation(q)?;
        Ok(())
    }

    fn __mul__(&self, o: PyRef<'_, PyPlacement>) -> PyPlacement { PyPlacement { inner: self.inner.mul(&o.inner) } }

    /// Transform a vector by this placement (FreeCAD `Placement.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector { inner: self.inner.to_matrix().transform(&v.inner) }
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

#[pyclass(name = "TypeId", module = "fc")]
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
    Ok(Py::new(
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
    )?)
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
fn fc(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PyQuantity>()?;
    m.add_class::<PyUnit>()?;
    m.add_class::<PyVector>()?;
    m.add_class::<PyMatrix>()?;
    m.add_class::<PyRotation>()?;
    m.add_class::<PyPlacement>()?;
    m.add_class::<PyTypeId>()?;
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
