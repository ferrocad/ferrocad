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
//! the repository `docs/record/milestones.md` for the per-slice record.

#![allow(non_snake_case)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_core::{application, canonical_name, format as doc_format, parse_unit, prop_status, status_from_name, status_names, Document as CoreDocument, DocumentFormat, ExtensionData, ExtensionValue, Matrix4, ObjectId, Placement, Property, Quantity, Rotation, StringHasher, StringId, TypeId, Unit, Vector3};
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

/// Canonical `Py<PyDocument>` per open-document name, so `getDocument`/`listDocuments`
/// return the *same* Python object (needed for `is`). Document *existence*, the
/// active document and name allocation live in `ferrocad_core::application`; this
/// cache only preserves Python object identity for the wrappers.
static DOCUMENTS: OnceLock<Mutex<BTreeMap<String, Py<PyDocument>>>> = OnceLock::new();

fn documents() -> &'static Mutex<BTreeMap<String, Py<PyDocument>>> {
    DOCUMENTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

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
            view: Mutex::new(None),
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

/// Map a `Document::set_property` rejection to the exception the object-type
/// hook asked for. A [`PropertyChangeError::Attribute`] renders with a marker
/// prefix (see `object_registry::ATTRIBUTE_REJECTION_PREFIX`); every other
/// rejection stays a `ValueError`, as before.
fn property_change_error(error: String) -> PyErr {
    match error.strip_prefix(ferrocad_core::object_registry::ATTRIBUTE_REJECTION_PREFIX) {
        Some(message) => PyAttributeError::new_err(message.to_string()),
        None => PyValueError::new_err(error),
    }
}

/// True if `id` still names an object in the document.
fn object_is_attached(inner: &Arc<Mutex<CoreDocument>>, id: ObjectId) -> bool {
    inner.lock().unwrap().object(id).is_some()
}

/// The immediate C++ base of a document-object type id, mirroring the class
/// hierarchy upstream scripts branch on (`obj.isDerivedFrom("Part::Feature")`).
///
/// Only the types this model can construct are listed; an unknown type equals
/// only itself. The `Part::` fallback covers the workbench's many feature
/// subclasses (booleans, dress-ups, arrays, datums) without listing each one.
fn type_parent(type_id: &str) -> Option<&'static str> {
    Some(match type_id {
        "App::DocumentObject" => return None,
        "App::GeoFeature" => "App::DocumentObject",
        "App::Feature" => "App::GeoFeature",
        "App::FeaturePython" => "App::Feature",
        "App::DocumentObjectGroup" => "App::DocumentObject",
        "App::DocumentObjectGroupPython" => "App::DocumentObjectGroup",
        "App::Part" => "App::GeoFeature",
        "App::Link" | "App::LinkGroup" | "App::LinkList" | "App::LinkElement" => {
            "App::DocumentObject"
        }
        "App::Origin" | "App::Plane" | "App::Line" | "App::Point" => "App::DocumentObject",
        "Part::Feature" => "App::GeoFeature",
        "Part::FeaturePython" => "Part::Feature",
        "Part::Part2DObject" => "Part::Feature",
        "Part::Part2DObjectPython" => "Part::Part2DObject",
        "PartDesign::Body" => "Part::Feature",
        "Sketcher::SketchObject" => "Part::Part2DObject",
        // Conservatively true for every known namespace: an unknown object still
        // derives from `App::DocumentObject`.
        other if other.starts_with("Part::") => "Part::Feature",
        other if other.starts_with("App::FeatureTest") => "App::Feature",
        other if other.starts_with("App::") => "App::DocumentObject",
        _ => return None,
    })
}

/// Whether `type_id` is `target` or derives from it (FreeCAD `Type::isDerivedFrom`).
fn type_is_derived_from(type_id: &str, target: &str) -> bool {
    let mut current = type_id;
    // The hierarchy is shallow; the bound only guards against a bad table entry.
    for _ in 0..16 {
        if current == target {
            return true;
        }
        match type_parent(current) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
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
// Module-owned property conversions (`Property::Extension`)
// ---------------------------------------------------------------------------

/// Convert a module-owned value to Python; `None` if the value is not this
/// module's (the caller then falls back to an opaque `None`).
pub type ExtensionToPython = fn(Python<'_>, &dyn ExtensionData) -> Option<PyObject>;

/// Recognise a Python value as a module-owned value; `None` if it is not.
pub type PythonToExtension = fn(&Bound<'_, PyAny>) -> Option<ExtensionValue>;

type ExtensionConverters = Vec<(String, ExtensionToPython, PythonToExtension)>;

fn extension_converters() -> &'static Mutex<ExtensionConverters> {
    static CONVERTERS: OnceLock<Mutex<ExtensionConverters>> = OnceLock::new();
    CONVERTERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register the Python conversions for a module-owned property type, keyed by its
/// registered type name (e.g. `Part::PropertyPartShape`). A workbench module
/// calls this at import, so `obj.Shape` round-trips between core and its own
/// `Part.Shape` class. Core stays free of any knowledge of the module's value.
pub fn register_extension_converter(
    type_name: &str,
    to_python: ExtensionToPython,
    to_extension: PythonToExtension,
) {
    let mut converters = extension_converters().lock().unwrap();
    converters.retain(|(name, _, _)| name != type_name);
    converters.push((type_name.to_string(), to_python, to_extension));
}

/// `Property::Extension` -> Python, using the converter the owning module
/// registered (or `None` when the module is not loaded).
fn extension_to_py(py: Python<'_>, value: &ExtensionValue) -> PyObject {
    let converter = extension_converters()
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _, _)| name == value.type_name())
        .map(|(_, to_python, _)| *to_python);
    converter
        .and_then(|to_python| to_python(py, value.data()))
        .unwrap_or_else(|| py.None())
}

/// Python -> `ExtensionValue`, trying each module's recogniser (there is no type
/// name to key on: the object's own class decides).
fn extension_from_py(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let converters: Vec<PythonToExtension> = extension_converters()
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, to_extension)| *to_extension)
        .collect();
    converters.iter().find_map(|to_extension| to_extension(value))
}

/// Python -> `ExtensionValue`, using only the converter registered for `type_name`.
///
/// An existing module-owned property keeps its type when reassigned, so trying its
/// own recogniser first avoids a generic guess (an empty list, for example, would
/// otherwise be taken for an `App::PropertyBoolList`).
fn extension_from_py_for(type_name: &str, value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let converter = extension_converters()
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _, _)| name == type_name)
        .map(|(_, _, to_extension)| *to_extension)?;
    converter(value)
}

/// Convert a sub-element name (`Edge1`, `Face3`) of a module-owned value to
/// Python; `None` if the name is not a sub-element of this value.
pub type SubElementToPython = fn(Python<'_>, &dyn ExtensionData, &str) -> Option<PyObject>;

type SubElementResolvers = Vec<(String, SubElementToPython)>;

fn sub_element_resolvers() -> &'static Mutex<SubElementResolvers> {
    static RESOLVERS: OnceLock<Mutex<SubElementResolvers>> = OnceLock::new();
    RESOLVERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register how a module's own property value resolves a sub-element name to a
/// Python object (`Part::PropertyPartShape` + `Edge1` -> `Part.Edge`).
///
/// `DocumentObject.getSubObject` calls this for the trailing segment of a
/// subname, so the core stays free of any knowledge of the module's geometry:
/// `ferrocad_py` only knows that *some* module registered a resolver, and the
/// module knows how to turn its own value into its own Python class.
pub fn register_sub_element_resolver(type_name: &str, resolver: SubElementToPython) {
    let mut resolvers = sub_element_resolvers().lock().unwrap();
    resolvers.retain(|(name, _)| name != type_name);
    resolvers.push((type_name.to_string(), resolver));
}

/// A module-owned value's sub-element as Python, using the resolver registered
/// for its type name (or `None` when the owning module is not loaded).
fn sub_element_to_py(py: Python<'_>, value: &ExtensionValue, name: &str) -> Option<PyObject> {
    let resolver = sub_element_resolvers()
        .lock()
        .unwrap()
        .iter()
        .find(|(type_name, _)| type_name == value.type_name())
        .map(|(_, resolver)| *resolver)?;
    resolver(py, value.data(), name)
}

// ---------------------------------------------------------------------------
// Dynamic extension registry
// ---------------------------------------------------------------------------

/// A module's `addExtension` hook: install an extension's properties on object
/// `id`. Returns whether the extension was provided (a missing object is
/// `false`). Registered by name (`"Part::AttachExtensionPython"`, …) so core and
/// this binding need no knowledge of the module.
pub type ExtensionProvider =
    fn(&mut ferrocad_core::Document, ferrocad_core::ObjectId) -> bool;

/// A module's extension method (`changeAttacherType`, `configLinkProperty`, …).
///
/// The fourth argument is the call's positional-argument tuple; when the Python
/// method also received keyword arguments, they are appended to that tuple as a
/// final `dict` element (so `configLinkProperty(LinkedObject="Base")` arrives as
/// `({"LinkedObject": "Base"},)`).
pub type ExtensionMethod = fn(
    &mut ferrocad_core::Document,
    ferrocad_core::ObjectId,
    &str,
    &pyo3::Bound<'_, pyo3::PyAny>,
) -> pyo3::PyResult<pyo3::PyObject>;

type ProviderEntry = (String, ExtensionProvider);
type MethodEntry = (String, String, ExtensionMethod);

fn extension_providers() -> &'static Mutex<Vec<ProviderEntry>> {
    static PROVIDERS: OnceLock<Mutex<Vec<ProviderEntry>>> = OnceLock::new();
    PROVIDERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn extension_methods() -> &'static Mutex<Vec<MethodEntry>> {
    static METHODS: OnceLock<Mutex<Vec<MethodEntry>>> = OnceLock::new();
    METHODS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register the properties an extension installs via `addExtension(name)`.
pub fn register_extension_provider(extension: &'static str, provider: ExtensionProvider) {
    let mut providers = extension_providers().lock().unwrap();
    providers.retain(|(name, _)| name != extension);
    providers.push((extension.to_string(), provider));
}

/// Register a method that an extension makes available on the objects carrying
/// it. The method is looked up by walking the object's own extension set, so a
/// module never edits this crate to expose it.
pub fn register_extension_method(
    extension: &'static str,
    method: &'static str,
    f: ExtensionMethod,
) {
    let mut methods = extension_methods().lock().unwrap();
    methods.retain(|(ext, name, _)| !(ext == extension && name == method));
    methods.push((extension.to_string(), method.to_string(), f));
}

/// The provider registered for `extension`, if any.
fn extension_provider(extension: &str) -> Option<ExtensionProvider> {
    extension_providers()
        .lock()
        .unwrap()
        .iter()
        .find(|(name, _)| name == extension)
        .map(|(_, provider)| *provider)
}

/// The method `name` registered by any extension the object actually has.
fn extension_method_for(
    doc: &CoreDocument,
    id: ObjectId,
    name: &str,
) -> Option<(String, ExtensionMethod)> {
    let extensions: Vec<String> = doc
        .object(id)
        .map(|object| object.extensions.iter().cloned().collect())
        .unwrap_or_default();
    let methods = extension_methods().lock().unwrap();
    for extension in &extensions {
        if let Some((ext, _, method)) = methods
            .iter()
            .find(|(ext, method, _)| ext == extension && method == name)
        {
            return Some((ext.clone(), *method));
        }
    }
    None
}

/// Dispatch an extension method on `slf`, raising `AttributeError` when none of
/// the object's extensions provides it.
fn call_extension_method(
    slf: &Bound<'_, PyDocumentObject>,
    method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let (inner, id) = {
        let this = slf.borrow();
        (Arc::clone(&this.inner), this.id)
    };
    let found = {
        let doc = inner.lock().unwrap();
        extension_method_for(&doc, id, method)
    };
    let Some((_extension, f)) = found else {
        let type_id = inner
            .lock()
            .unwrap()
            .object(id)
            .map(|object| object.type_id.clone())
            .unwrap_or_default();
        return Err(PyAttributeError::new_err(format!(
            "'{type_id}' object has no attribute '{method}'"
        )));
    };
    let mut doc = inner.lock().unwrap();
    f(&mut doc, id, method, args)
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

/// Outcome of restoring an object's `python_state` blob.
#[derive(Debug, PartialEq, Eq)]
enum ProxyRestore {
    /// The blob held no proxy (only plain instance attributes).
    Absent,
    /// The proxy class was importable and has been installed on the object.
    Installed,
    /// The blob described a proxy that could not be reconstructed.
    Failed(String),
}

/// Restore an object's Python instance state from a `capture_object_state` blob.
fn apply_object_state(
    py: Python<'_>,
    obj: &Bound<'_, PyDocumentObject>,
    state: &str,
) -> PyResult<ProxyRestore> {
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

    let Some(proxy_info) = payload.get_item("proxy")? else {
        return Ok(ProxyRestore::Absent);
    };
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
    if module.is_empty() || class.is_empty() {
        return Ok(ProxyRestore::Failed(
            "the persisted proxy names no module/class".to_string(),
        ));
    }
    let sys = py.import("sys")?;
    let modules = sys.getattr("modules")?.downcast_into::<PyDict>()?;
    let module_obj = match modules.get_item(&module)? {
        Some(m) => m,
        None => py
            .import("importlib")?
            .call_method1("import_module", (&module,))?,
    };
    let cls = match module_obj.getattr(class.as_str()) {
        Ok(cls) => cls,
        Err(_) => {
            return Ok(ProxyRestore::Failed(format!(
                "the persisted proxy class '{module}.{class}' is not available"
            )))
        }
    };
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
    Ok(ProxyRestore::Installed)
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
        let outcome = apply_object_state(py, obj.bind(py), &state);
        let problem = match outcome {
            Ok(ProxyRestore::Failed(reason)) => Some(reason),
            Ok(_) => None,
            Err(error) => Some(error.value(py).to_string()),
        };
        if let Some(reason) = problem {
            // A scripted object whose proxy could not be rebuilt is honest about
            // it: readable (its properties are loaded), but invalid until a proxy
            // is installed and the object recomputes.
            inner.lock().unwrap().mark_invalid_with(
                id,
                format!("the Python proxy could not be restored: {reason}"),
            );
        }
    }
}

/// Call `proxy.onDocumentRestored(obj)` on every object that has a proxy.
///
/// Upstream runs this after a document is opened so a proxy can rebuild
/// transient state (and add properties introduced by a newer version). It is
/// deliberately not called for an undo snapshot restore, which is not a
/// document open.
fn notify_document_restored(py: Python<'_>, doc_py: &Py<PyDocument>, inner: &Arc<Mutex<CoreDocument>>) {
    let ids = inner.lock().unwrap().object_ids();
    for id in ids {
        let obj = get_or_create_object(py, doc_py, inner, id);
        let obj = obj.bind(py);
        if object_proxy(obj).is_some() {
            call_proxy_method(obj, "onDocumentRestored", None);
        }
    }
}

/// The object's live Python proxy (`obj.Proxy`), if it has one.
fn object_proxy(obj: &Bound<'_, PyDocumentObject>) -> Option<Py<PyAny>> {
    let dict = obj.getattr("__dict__").ok()?.downcast_into::<PyDict>().ok()?;
    match dict.get_item("Proxy") {
        Ok(Some(proxy)) if !proxy.is_none() => Some(proxy.unbind()),
        _ => None,
    }
}

/// Call a proxy hook (`onChanged`, `onDocumentRestored`) if the proxy defines it.
///
/// A proxy is user code: an exception in a notification must not make a property
/// assignment or a document load fail, so it is swallowed, as upstream does (it
/// logs instead).
fn call_proxy_method(obj: &Bound<'_, PyDocumentObject>, method: &str, prop: Option<&str>) {
    let py = obj.py();
    let Some(proxy) = object_proxy(obj) else {
        return;
    };
    let proxy = proxy.bind(py);
    let Ok(method) = proxy.getattr(method) else {
        return;
    };
    let _ = match prop {
        Some(prop) => method.call1((obj, prop)),
        None => method.call1((obj,)),
    };
}

/// Run one Python feature's `proxy.execute(obj)` with the document lock released.
///
/// The proxy reads and writes its object through the normal Python surface, so it
/// must not run inside `Document::recompute` (which holds the lock). Its output
/// writes touch the object, which would schedule another execute; the request has
/// been answered, so those flags are cleared and the object's dependents are
/// enforced for the next pass.
fn run_python_execute(
    py: Python<'_>,
    doc_py: &Py<PyDocument>,
    inner: &Arc<Mutex<CoreDocument>>,
    id: ObjectId,
) {
    let obj = get_or_create_object(py, doc_py, inner, id);
    let obj = obj.bind(py);
    let Some(proxy) = object_proxy(obj) else {
        let mut doc = inner.lock().unwrap();
        doc.purge_touched(id);
        doc.mark_invalid_with(id, "the feature has no Python proxy to execute");
        return;
    };
    let result = proxy.bind(py).call_method1("execute", (obj,));
    {
        let mut doc = inner.lock().unwrap();
        doc.purge_touched(id);
        for dependent in doc.dependents(id) {
            doc.enforce_recompute(dependent);
        }
    }
    if let Err(error) = result {
        let message = error.value(py).to_string();
        inner
            .lock()
            .unwrap()
            .mark_invalid_with(id, format!("proxy.execute failed: {message}"));
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
                let second = args.get_item(1)?;
                // `Quantity(100, App.Units.Length)` passes a `Unit`; the string form
                // `Quantity(100, "mm")` must keep working.
                let u = if let Ok(unit) = second.extract::<PyRef<'_, PyUnit>>() {
                    unit.inner
                } else if let Ok(symbol) = second.extract::<String>() {
                    Unit::parse(&symbol).ok_or_else(|| {
                        PyValueError::new_err(format!("unknown unit '{symbol}'"))
                    })?
                } else {
                    return Err(PyTypeError::new_err(
                        "Quantity() second argument must be a unit string or a Unit",
                    ));
                };
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
        Property::LinkSubList(entries) => entries
            .iter()
            .map(|(name, subs)| {
                let subs = PyTuple::new(py, subs.iter().map(String::as_str)).unwrap();
                (name.clone(), subs).into_py_any(py).unwrap()
            })
            .collect::<Vec<_>>()
            .into_py_any(py)
            .unwrap(),
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
        // Core cannot interpret a module-owned value; the owning module registers
        // the Python conversion (Part, for `obj.Shape`). Without one, it is opaque.
        Property::Extension(value) => extension_to_py(py, value),
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
    // Only `Placement`/`Rotation` values build views (`property_to_py_at` is called for
    // those two), so a sub-field write only ever needs the placement it came from. This
    // keeps the view free of a whole `Property`, which could hold a !`Sync` extension.
    let expect_placement = match p {
        Property::Placement(pl) => Some(*pl),
        _ => None,
    };
    let view = |kind: ViewKind| GeometryView {
        target: ViewTarget::Document {
            inner: Arc::clone(inner),
            id,
            prop: prop.to_string(),
            version,
        },
        expect_placement,
        kind,
    };
    match p {
        Property::Placement(pl) => PyPlacement {
            inner: Arc::new(Mutex::new(*pl)),
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
        return Ok(Property::Placement(p.borrow().get()));
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
        return Ok(Property::PlacementList(v.iter().map(|x| x.get()).collect()));
    }
    if let Ok(v) = value.extract::<Vec<PyRef<'_, PyRotation>>>() {
        return Ok(Property::RotationList(v.iter().map(|x| x.inner).collect()));
    }
    // A module-owned value the owning module recognises (e.g. a Part.Shape).
    if let Some(extension) = extension_from_py(value) {
        return Ok(Property::Extension(extension));
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
    // `Immutable` is upstream's runtime `Property::Status` bit 1, which this
    // model folds into the public `ReadOnly` flag (see the FCStd status
    // translation). It is what Draft's link-array code sets on `PlacementList`.
    let bit = if bare.eq_ignore_ascii_case("immutable") {
        Some(prop_status::READONLY)
    } else {
        status_from_name(bare)
    };
    match bit {
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
    // Upstream's `PropertyLinkList` accepts a single object as a one-element
    // list (`obj.Support = p1`); a sequence is the usual form.
    if let Ok(object) = value.downcast::<PyDocumentObject>() {
        return Ok(vec![object.borrow().name_string()]);
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
    // A bare object (or name) sets the link with no sub-elements, as upstream's
    // `PropertyLinkSub` accepts. `array.Path = sketch` is the common form.
    if let Ok(object) = value.downcast::<PyDocumentObject>() {
        return Ok((object.borrow().name_string(), vec![]));
    }
    if let Ok(name) = value.extract::<String>() {
        return Ok((name, vec![]));
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

/// Coerce a value into a link-sub-list: a list of `(object, subnames)` groups.
///
/// Accepted spellings mirror upstream `PropertyLinkSubList::setPyObject`:
/// a single `DocumentObject`, a single `(object, subname-or-subnames)` pair, and
/// a sequence of either. `None` or an empty sequence clears the property. A
/// whole-object entry stores the empty subname `""`, which is how upstream
/// marks it (and what Draft checks with `sel[1] == ("",)`).
fn py_to_link_sub_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<(String, Vec<String>)>> {
    if value.is_none() {
        return Ok(vec![]);
    }
    // A single object (or name) is a one-entry list with no sub-elements.
    if let Ok(object) = value.downcast::<PyDocumentObject>() {
        return Ok(vec![(object.borrow().name_string(), vec![String::new()])]);
    }
    if let Ok(name) = value.extract::<String>() {
        return Ok(vec![(name, vec![String::new()])]);
    }
    // A single `(object, subnames)` pair.
    if let Ok(entry) = py_to_link_sub_list_entry(value) {
        return Ok(vec![entry]);
    }
    let mut entries = Vec::new();
    for item in value.try_iter().map_err(|_| {
        PyTypeError::new_err(
            "LinkSubList expects a list of DocumentObjects or (object, subnames) pairs",
        )
    })? {
        entries.push(py_to_link_sub_list_entry(&item?)?);
    }
    Ok(entries)
}

/// One entry of a link-sub-list: an object or an `(object, subnames)` pair.
fn py_to_link_sub_list_entry(value: &Bound<'_, PyAny>) -> PyResult<(String, Vec<String>)> {
    if let Ok(object) = value.downcast::<PyDocumentObject>() {
        return Ok((object.borrow().name_string(), vec![String::new()]));
    }
    if let Ok(name) = value.extract::<String>() {
        return Ok((name, vec![String::new()]));
    }
    let t = if let Ok(t) = value.downcast::<PyTuple>() {
        t.iter().collect::<Vec<_>>()
    } else if let Ok(l) = value.downcast::<pyo3::types::PyList>() {
        // Upstream's `PropertyLinkSub::setPyObject` also accepts a 2-list, which
        // makes `[object, "sub"]` a single pair; a 2-list of objects still falls
        // through to the list form below.
        l.iter().collect::<Vec<_>>()
    } else {
        return Err(PyTypeError::new_err(
            "LinkSubList entries must be DocumentObjects or (object, subnames)",
        ));
    };
    if t.len() != 2 {
        return Err(PyTypeError::new_err(
            "LinkSubList expects an (object, subnames) 2-tuple",
        ));
    }
    let obj = &t[0];
    let subs = &t[1];
    if !obj.is_none() && obj.downcast::<PyDocumentObject>().is_err() {
        return Err(PyTypeError::new_err(
            "LinkSubList object must be a DocumentObject or None",
        ));
    }
    let name = if obj.is_none() {
        String::new()
    } else {
        obj.downcast::<PyDocumentObject>()
            .expect("checked above")
            .borrow()
            .name_string()
    };
    let names: Vec<String> = if let Ok(s) = subs.extract::<String>() {
        vec![s]
    } else if let Ok(v) = subs.extract::<Vec<String>>() {
        // An empty sub-name sequence is a whole-object reference.
        if v.is_empty() {
            vec![String::new()]
        } else {
            v
        }
    } else {
        return Err(PyTypeError::new_err(
            "LinkSubList subnames must be a string or a list of strings",
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

/// Which kind of group a `getParent*()` query is looking for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParentScope {
    /// `getParentGroup()`: plain groups only (upstream returns no geo group).
    Plain,
    /// `getParentGeoFeatureGroup()`: placeable groups (`App::Part`, `PartDesign::Body`).
    Geo,
    /// `getParent()`: the first group parent of any kind.
    Any,
}

/// Find the group of `scope` that lists `slf` in its `Group` link list.
fn parent_of(slf: &Bound<'_, PyDocumentObject>, scope: ParentScope) -> Option<Py<PyDocumentObject>> {
    let py = slf.py();
    let inner = Arc::clone(&slf.borrow().inner);
    let doc_py = slf.borrow().doc.clone_ref(py);
    let name = slf.borrow().name_string();
    let found = {
        let doc = inner.lock().unwrap();
        doc.object_ids().into_iter().find(|id| {
            let matches_scope = match scope {
                ParentScope::Plain => doc.is_group_like(*id) && !doc.is_geo_group_like(*id),
                ParentScope::Geo => doc.is_geo_group_like(*id),
                ParentScope::Any => doc.is_group_like(*id),
            };
            matches_scope
                && matches!(doc.object(*id).and_then(|o| o.properties.get("Group")),
                    Some(Property::LinkList(links)) if links.contains(&name))
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
    /// The serializer that wrote `file_name` (or read it). Recorded so `save()`
    /// reuses it even if the file is moved or renamed in between.
    format: Option<DocumentFormat>,
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
    /// The canonical `ViewObject`, created on first access (one per object).
    view: Mutex<Option<Py<PyViewObject>>>,
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
            format: None,
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
        let _ = viewType;
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
            // A freshly added object with a builder is dirty, as in FreeCAD: the
            // next recompute builds its output (`Part::Box` → `Shape`) even before
            // a property is edited. `App::DocumentObjectGroupPython` has no
            // registered `ObjectType`, but its proxy must still run once.
            if ferrocad_core::object_registry::get(r#type).is_some()
                || r#type == "App::DocumentObjectGroupPython"
            {
                doc.touch(id, false);
            }
            (id, tx)
        };
        if let Some(tx) = tx {
            fire_doc_str("slotOpenTransaction", &slf, &tx);
        }
        let obj = get_or_create_object(py, &doc_py, &inner, id);
        // Store the proxies exactly where upstream does: the object proxy is the
        // object's `Proxy` instance attribute (persisted through `python_state`),
        // the view proxy belongs to the view object.
        if let Some(proxy) = objProxy {
            if !proxy.is_none() {
                obj.bind(py).setattr("Proxy", proxy)?;
            }
        }
        if let Some(proxy) = viewProxy {
            if !proxy.is_none() {
                let bound = obj.bind(py);
                let this = bound.borrow();
                let view = view_object_for(py, &this)?;
                drop(this);
                view.bind(py).setattr("Proxy", proxy)?;
            }
        }
        // `attach=True` is how a scripted object states that its proxy provides
        // the type's property set (`Draft.make_array` creates a link array this
        // way). Upstream calls the proxy's `attach` after construction.
        if attach {
            if let Some(proxy) = objProxy {
                if !proxy.is_none() && proxy.hasattr("attach").unwrap_or(false) {
                    proxy.call_method1("attach", (obj.bind(py),))?;
                }
            }
        }
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

    /// Recompute the document (FreeCAD `Document.recompute(objs, force, mode)`).
    ///
    /// Python-feature objects do not rebuild inside core's dependency walk: their
    /// registered type only records a request (`Document::take_python_pending`),
    /// because their `execute` is Python and must run with the document lock
    /// released. Each pass therefore runs the proxies for the objects the walk
    /// requested, then recomputes again so their dependents see the new output.
    /// `objs`, `force` and `mode` are accepted for upstream compatibility; this
    /// model always walks the whole dependency graph.
    #[pyo3(signature = (objs=None, force=false, mode=false))]
    fn recompute(
        slf: &Bound<'_, Self>,
        objs: Option<&Bound<'_, PyAny>>,
        force: bool,
        mode: bool,
    ) -> PyResult<usize> {
        let _ = (objs, force, mode);
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let mut executed: Vec<ObjectId> = Vec::new();
        let mut rounds = 0usize;
        loop {
            let pass = inner
                .lock()
                .unwrap()
                .recompute()
                .map_err(PyValueError::new_err)?;
            for id in pass {
                if !executed.contains(&id) {
                    executed.push(id);
                }
            }
            let pending = inner.lock().unwrap().take_python_pending();
            if pending.is_empty() {
                break;
            }
            rounds += 1;
            if rounds > 64 {
                return Err(PyRuntimeError::new_err(
                    "recompute did not converge: Python proxies keep touching their objects",
                ));
            }
            for id in pending {
                run_python_execute(py, &doc_py, &inner, id);
            }
        }
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
    /// Save under `path`, selecting the serializer from the suffix.
    ///
    /// `.ferrocad` writes native JSON; `.FCStd` writes the upstream ZIP container
    /// (`Document.xml`, `GuiDocument.xml`, `.brp` members). The suffix, not the
    /// content, decides — and an unknown suffix is an error. Both the path and the
    /// format are recorded for later `save()` calls.
    fn saveAs(slf: &Bound<'_, Self>, path: &str) -> PyResult<()> {
        let format = DocumentFormat::from_path(path).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let py = slf.py();
        let name = slf.borrow().name.clone();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        fire_doc_str("slotStartSaveDocument", slf, path);
        capture_all_python_states(py, &doc_py, &inner);
        let saved = inner.lock().unwrap().to_saved(&name);
        doc_format::write(&saved, path, format).map_err(|e| PyValueError::new_err(e.to_string()))?;
        {
            let mut this = slf.borrow_mut();
            this.file_name = Some(path.to_string());
            this.format = Some(format);
        }
        fire_doc_str("slotFinishSaveDocument", slf, path);
        Ok(())
    }

    fn save(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let (name, path, recorded) = {
            let this = slf.borrow();
            (this.name.clone(), this.file_name.clone(), this.format)
        };
        let path = path
            .ok_or_else(|| PyValueError::new_err("document has no file name; use saveAs first"))?;
        // Reuse the format the file was written (or read) with: a later rename
        // cannot flip the meaning of the same document.
        let format = match recorded {
            Some(format) => format,
            None => DocumentFormat::from_path(&path)
                .map_err(|e| PyValueError::new_err(e.to_string()))?,
        };
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        fire_doc_str("slotStartSaveDocument", slf, &path);
        capture_all_python_states(py, &doc_py, &inner);
        let saved = inner.lock().unwrap().to_saved(&name);
        doc_format::write(&saved, &path, format).map_err(|e| PyValueError::new_err(e.to_string()))?;
        slf.borrow_mut().format = Some(format);
        fire_doc_str("slotFinishSaveDocument", slf, &path);
        Ok(())
    }

    fn load(slf: &Bound<'_, Self>, path: &str) -> PyResult<()> {
        let py = slf.py();
        let (saved, format) =
            doc_format::read(path).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        forget_document(&doc_py);
        let mut this = slf.borrow_mut();
        this.name = saved.name.clone();
        this.label = saved.name.clone();
        this.file_name = Some(path.to_string());
        this.format = Some(format);
        this.inner = Arc::new(Mutex::new(CoreDocument::from_saved(&saved)));
        this.meta = Arc::new(Mutex::new(BTreeMap::new()));
        drop(this);
        let inner = Arc::clone(&slf.borrow().inner);
        apply_all_python_states(py, &doc_py, &inner);
        notify_document_restored(py, &doc_py, &inner);
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
        notify_document_restored(py, &doc_py, &inner);
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

    /// `RootObjects`: objects that no other object depends on.
    #[getter]
    fn RootObjects(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let ids = inner.lock().unwrap().root_objects();
        ids.into_iter()
            .map(|id| get_or_create_object(py, &doc_py, &inner, id))
            .collect()
    }

    /// `TopologicalSortedObjects`: dependents-first topological order.
    #[getter]
    fn TopologicalSortedObjects(slf: &Bound<'_, Self>) -> Vec<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py: Py<PyDocument> = slf.clone().unbind();
        let ids = inner.lock().unwrap().topological_sorted_objects();
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

// ---------------------------------------------------------------------------
// Link resolution helpers (used by `isLink`, `getLinkedObject`, `getSubObject`)
// ---------------------------------------------------------------------------

/// The link base-type table: `App::Link` and every `App::Link*` subclass
/// (`LinkGroup`, `LinkList`, `LinkElement`, …), plus the `Part::LinkArray*` peers.
///
/// Upstream the arrays derive from `App::Link`; FerroCAD registers them as
/// separate `ObjectType`s because there is no shared base object yet, so they are
/// named explicitly here. This replaces the `starts_with("App::Link")` test that
/// `setLink` used to carry.
fn is_link_type(type_id: &str) -> bool {
    type_id.starts_with("App::Link") || type_id.starts_with("Part::LinkArray")
}

/// The object named by `LinkedObject`, if it resolves.
///
/// `LinkedObject` is a `PropertyLink` or a `PropertyLinkSub`; both carry the
/// target by name, so the query API does not need a kernel to follow them.
fn linked_object_id(doc: &CoreDocument, id: ObjectId) -> Option<ObjectId> {
    let name = match doc.object(id)?.properties.get("LinkedObject")? {
        Property::Link(name) | Property::LinkSub(name, _) => name,
        _ => return None,
    };
    if name.is_empty() {
        return None;
    }
    doc.get_by_name(name)
}

/// The element index carried by an element reference, or `-1`.
///
/// Accepted spellings: `Element3`, `Array.Element3`, `Array_Element3`, and the
/// bare index form upstream's element paths use (`"3."`). The last
/// dot-separated segment decides, matching upstream `getElementIndex`.
fn element_index(name: &str) -> i64 {
    let trimmed = name.trim().trim_end_matches('.');
    let segment = trimmed.rsplit('.').next().unwrap_or(trimmed);
    let digits = segment
        .strip_prefix("Element")
        .or_else(|| {
            segment
                .rsplit("_Element")
                .next()
                .filter(|suffix| *suffix != segment)
        })
        .unwrap_or(segment);
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        digits.parse().unwrap_or(-1)
    } else {
        -1
    }
}

/// Link containers (upstream `isLinkGroup()`): their `ElementList` is their
/// `Group` membership rather than generated `_ElementN` children.
fn is_link_group_type(type_id: &str) -> bool {
    matches!(type_id, "App::LinkGroup" | "App::LinkList")
}

/// The element child of `id` addressed by `segment`.
///
/// `ElementList` holds ordinary document-object names in element order; a full
/// name is matched directly, while an index spelling maps through that order.
///
/// The ordinal convention differs by type, matching the two upstream shapes of
/// `ElementList`:
///
/// - a **link group** stores its members directly, so `Element1` is the first
///   member (1-based, as `getElementIndex` reports it);
/// - a generated element set (an `App::Link` or `Part::LinkArray*`) names its
///   children `_Element0`, `_Element1`, …, and `ElementN` addresses the child
///   whose ordinal is `N` (so `Element0` is also addressable).
fn element_child(doc: &CoreDocument, id: ObjectId, segment: &str) -> Option<ObjectId> {
    let object = doc.object(id)?;
    let names = match object.properties.get("ElementList") {
        Some(Property::LinkList(names)) => names,
        _ => return None,
    };
    if names.iter().any(|name| name == segment) {
        return doc.get_by_name(segment);
    }
    let index = element_index(segment);
    if index < 0 {
        return None;
    }
    let position = if is_link_group_type(&object.type_id) {
        usize::try_from(index.checked_sub(1)?).ok()?
    } else {
        index as usize
    };
    doc.get_by_name(names.get(position)?)
}

/// Whether `id` owns a non-empty `ElementList`.
fn has_elements(doc: &CoreDocument, id: ObjectId) -> bool {
    matches!(
        doc.object(id).and_then(|object| object.properties.get("ElementList")),
        Some(Property::LinkList(names)) if !names.is_empty()
    )
}

/// A child listed in `Group`, addressed by document-object name.
fn named_child(doc: &CoreDocument, id: ObjectId, segment: &str) -> Option<ObjectId> {
    let members = match doc.object(id)?.properties.get("Group") {
        Some(Property::LinkList(members)) => members,
        _ => return None,
    };
    if !members.iter().any(|name| name == segment) {
        return None;
    }
    doc.get_by_name(segment)
}

fn placement_of(doc: &CoreDocument, id: ObjectId) -> Placement {
    doc.object(id)
        .and_then(|object| object.properties.get("Placement"))
        .and_then(|property| match property {
            Property::Placement(placement) => Some(*placement),
            _ => None,
        })
        .unwrap_or_else(Placement::identity)
}

/// One resolved subname: the owning object, plus the sub-element name when the
/// subname addressed an element of that object (`Face1`) rather than the object
/// itself (`Array.Element3`).
#[derive(Clone)]
struct ResolvedSub {
    id: ObjectId,
    element: Option<String>,
}

/// Resolve one subname relative to `start`.
///
/// Accepts a whole document-object name first (the original behaviour, which is
/// how `App::Origin`'s `X_Axis` and friends resolve), then a dotted path.
fn resolve_sub_name(doc: &CoreDocument, start: ObjectId, subname: &str) -> Option<ResolvedSub> {
    let normalised = subname.trim().trim_matches('.');
    if normalised.is_empty() {
        return Some(ResolvedSub {
            id: start,
            element: None,
        });
    }
    if let Some(id) = doc.get_by_name(normalised) {
        return Some(ResolvedSub {
            id,
            element: None,
        });
    }

    let mut segments: Vec<&str> = normalised.split('.').filter(|s| !s.is_empty()).collect();
    // A persisted reference may carry its owner as a prefix (`Array.Element3`).
    if let Some(first) = segments.first().copied() {
        let own = doc.object(start).map(|object| object.name.as_str()).unwrap_or("");
        if first == own {
            segments.remove(0);
        }
    }
    resolve_segments(doc, start, &segments, 0)
}

fn resolve_segments(
    doc: &CoreDocument,
    current: ObjectId,
    segments: &[&str],
    depth: usize,
) -> Option<ResolvedSub> {
    if segments.is_empty() {
        return Some(ResolvedSub {
            id: current,
            element: None,
        });
    }
    if depth > 32 {
        return None;
    }
    let segment = segments[0];
    let rest = &segments[1..];

    if let Some(child) = element_child(doc, current, segment) {
        if let Some(found) = resolve_segments(doc, child, rest, depth + 1) {
            return Some(found);
        }
    }
    if let Some(child) = named_child(doc, current, segment) {
        if let Some(found) = resolve_segments(doc, child, rest, depth + 1) {
            return Some(found);
        }
    }
    // An element spelling against an object that owns elements is an element
    // reference, not a sub-element of the linked target: an out-of-range
    // `Element5` is missing, and must not silently resolve to the target.
    if element_index(segment) >= 0 && has_elements(doc, current) {
        return None;
    }
    // A link forwards an unresolved path to its target, so `Link.Face1` reaches
    // the linked feature. Sub-elements themselves are still resolved by the
    // kernel positionally (L5 turns this into a lineage lookup).
    if let Some(target) = linked_object_id(doc, current) {
        if let Some(found) = resolve_segments(doc, target, segments, depth + 1) {
            return Some(found);
        }
    }
    // Nothing matched by name: if this object owns geometry, the remaining path
    // names one of its sub-elements and the owner is the answer (upstream
    // returns the owning feature for a sub-element reference). Resolution is
    // delegated to the shape value's lineage: a name the lineage retired fails
    // here instead of silently selecting whatever element now has that ordinal.
    // The element path is reported so `getSubObject` can hand it to the owning
    // module, which turns it into a `Part.Edge`/`Part.Face`; the owner alone is
    // what link and element resolution needs.
    if accepts_sub_element(doc, current, segment) {
        return Some(ResolvedSub {
            id: current,
            element: Some(segments.join(".")),
        });
    }
    None
}

/// Whether `segment` is a usable sub-element reference on `current`.
///
/// The shape value owns the rule (through the geometry-free
/// [`ExtensionData::resolve_element_name`] hook): a current or positionally
/// resolvable name is accepted, a retired or missing one is rejected. A value
/// that has no opinion — including a shape without a lineage — keeps the
/// caller's existing behaviour. Objects without a `Shape` property never accept
/// a sub-element here.
fn accepts_sub_element(doc: &CoreDocument, current: ObjectId, segment: &str) -> bool {
    match doc
        .object(current)
        .and_then(|object| object.properties.get("Shape"))
    {
        Some(Property::Extension(value)) => value.data().resolve_element_name(segment).is_ok(),
        // A non-extension `Shape` value cannot interpret names; keep the old
        // permissive fallback rather than changing an unrelated shape type.
        Some(_) => true,
        None => false,
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

    /// FreeCAD `DocumentObject.isDerivedFrom(typeId)`.
    ///
    /// Draft and other proxies branch on the C++ base class (`obj.isDerivedFrom
    /// ("Part::Part2DObject")`), so the type hierarchy has to be walkable even
    /// though our object types are registered behaviours rather than C++ classes.
    fn isDerivedFrom(&self, type_id: &str) -> bool {
        match self.inner.lock().unwrap().object(self.id) {
            Some(o) => type_is_derived_from(&o.type_id, type_id),
            None => false,
        }
    }

    /// The headless view provider (`Transparency`, `ShapeAppearance`, …).
    ///
    /// Upstream only has a view object in a GUI session; FerroCAD keeps the model
    /// that App-level code reads in every session (see `ferrocad_core::ViewState`).
    #[getter]
    fn ViewObject(slf: PyRef<'_, Self>) -> PyResult<Py<PyViewObject>> {
        if !object_is_attached(&slf.inner, slf.id) {
            return Err(deleted_object_error("ViewObject"));
        }
        view_object_for(slf.py(), &slf)
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

    /// Whether the object is valid (`DocumentObject.isValid`).
    ///
    /// A geometry object reports its `Shape`'s validity; an object without a
    /// `Shape` property is valid.
    fn isValid(slf: &Bound<'_, Self>) -> bool {
        let Ok(shape) = slf.getattr("Shape") else {
            return true;
        };
        shape
            .call_method0("isValid")
            .and_then(|value| value.extract::<bool>())
            .unwrap_or(true)
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

    fn setPropertyByName(slf: &Bound<'_, Self>, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let property = py_to_property(value)?;
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        inner
            .lock()
            .unwrap()
            .set_property(id, name, property)
            .map_err(property_change_error)?;
        call_proxy_method(slf, "onChanged", Some(name));
        Ok(())
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
    ) -> PyResult<Py<Self>> {
        let _ = (group, doc, locked);
        if name.is_empty() {
            return Err(PyValueError::new_err("property name must not be empty"));
        }
        // Resolve the type through the registry (FreeCAD's `Base::Type` by name):
        // an unknown type is an error, never a silent default.
        let base = ferrocad_core::property_types::default_for(type_id).ok_or_else(|| {
            PyTypeError::new_err(format!("'{type_id}' is not a property type"))
        })?;
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
        let default = match enum_vals {
            Some(vals) => Property::Enumeration(vals, 0),
            None => base,
        };
        inner
            .lock()
            .unwrap()
            .add_property(id, name, default, status, group, doc)
            .map_err(PyValueError::new_err)?;
        fire_obj_str("slotAppendDynamicProperty", slf, name);
        // Upstream returns the document object, so `addProperty(...).KeepLeftMargin = False`
        // chains work (Draft's ShapeString relies on it).
        Ok(slf.clone().unbind())
    }

    #[pyo3(signature = (prop, source=None))]
    fn setExpression(&self, prop: &str, source: Option<String>) -> PyResult<()> {
        let mut doc = self.inner.lock().unwrap();
        match source {
            Some(src) => doc.set_expression(self.id, prop, &src).map_err(|e| {
                // The core reports upstream's wording now
                // ("{prop} reference creates a cyclic dependency."); the "cyclic"
                // spelling is kept as a fallback for other core errors.
                if e.contains("creates a cyclic dependency") || e.starts_with("cyclic") {
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

    /// Recompute this object (FreeCAD `DocumentObject.recompute`).
    ///
    /// Runs its type behaviour now and clears its touched/execute flags; unlike
    /// `Document.recompute` this does not walk the whole dependency graph. A
    /// Python-feature object runs its proxy here, outside the document lock.
    fn recompute(slf: &Bound<'_, Self>) -> bool {
        let py = slf.py();
        let (inner, id, doc_py) = {
            let this = slf.borrow();
            (
                Arc::clone(&this.inner),
                this.id,
                this.doc.clone_ref(py),
            )
        };
        let executed = {
            let mut doc = inner.lock().unwrap();
            doc.take_must_execute_one(id);
            let executed = doc.execute_object(id);
            doc.purge_touched(id);
            executed
        };
        if inner.lock().unwrap().take_python_pending_one(id) {
            run_python_execute(py, &doc_py, &inner, id);
        }
        if executed {
            fire_obj("slotRecomputedObject", slf, None);
        }
        !inner
            .lock()
            .unwrap()
            .object(id)
            .is_some_and(|object| object.invalid)
    }

    fn enforceRecompute(&self) {
        self.inner.lock().unwrap().enforce_recompute(self.id);
    }

    /// Mark the object as unchanged (FreeCAD `purgeTouched`).
    fn purgeTouched(&self) {
        self.inner.lock().unwrap().purge_touched(self.id);
    }

    /// A short status string: `Invalid[: message]`, `Touched` or `Valid`.
    ///
    /// FreeCAD reports the execute failure after the state, so a rejected
    /// pattern shows its reason (`Invalid: Circular pattern created more than
    /// 10000 circles`) instead of a bare `Invalid`.
    fn getStatusString(&self) -> String {
        let doc = self.inner.lock().unwrap();
        match doc.object(self.id) {
            Some(o) if o.invalid => match o.error.as_deref() {
                Some(message) if !message.is_empty() => format!("Invalid: {message}"),
                _ => "Invalid".to_string(),
            },
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
        ferrocad_core::property_types::names()
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
            // A module registers the properties its extension adds; core and
            // this binding stay free of module names.
            if let Some(provider) = extension_provider(name) {
                let _ = provider(&mut doc, id);
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

    /// `changeAttacherType(name)`: provided by an attached extension (Part's
    /// `AttachExtensionPython`), not by this crate.
    #[pyo3(signature = (mode))]
    fn changeAttacherType(slf: &Bound<'_, Self>, mode: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let args = PyTuple::new(slf.py(), [mode.clone()])?;
        call_extension_method(slf, "changeAttacherType", args.as_any())
    }

    /// `configLinkProperty(property, **links)`: provided by an attached
    /// extension (App's `LinkExtensionPython`), not by this crate. Keyword
    /// arguments reach the provider as a final `dict` element of the tuple.
    #[pyo3(signature = (*args, **kwargs))]
    fn configLinkProperty(
        slf: &Bound<'_, Self>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PyObject> {
        let mut items: Vec<Bound<'_, PyAny>> = args.iter().collect();
        if let Some(kwargs) = kwargs {
            if !kwargs.is_empty() {
                items.push(kwargs.clone().into_any());
            }
        }
        let call_args = PyTuple::new(slf.py(), items)?;
        call_extension_method(slf, "configLinkProperty", call_args.as_any())
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
        let type_id = doc
            .object(self.id)
            .map(|o| o.type_id.clone())
            .unwrap_or_default();
        let link_group = is_link_group_type(&type_id);
        if !doc.is_group_like(self.id) && !link_group {
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
            .map_err(PyValueError::new_err)?;
        if link_group {
            // See `set_group`: a container's output depends on its members' shapes.
            doc.add_dependency(self.id, other_id);
        }
        Ok(())
    }

    /// `GroupExtension.removeObject(obj)`: drop `obj` from this group's `Group`
    /// link list (which also clears its derived parent) and return the objects
    /// that were actually removed.
    ///
    /// A non-member is not an error: upstream's `removeObjects` filters the
    /// list and returns an empty vector, so the object and group are unchanged.
    /// Like `addObject`, the method only exists on group extensions upstream, so
    /// a non-group object refuses with `AttributeError`.
    fn removeObject(
        &self,
        py: Python<'_>,
        other: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<Py<PyDocumentObject>>> {
        let other_ref: PyRef<'_, PyDocumentObject> = other
            .extract()
            .map_err(|_| PyTypeError::new_err("removeObject expects a DocumentObject"))?;
        let other_id = other_ref.id;
        let other_name = other_ref.name_string();
        drop(other_ref);

        let mut doc = self.inner.lock().unwrap();
        let type_id = doc
            .object(self.id)
            .map(|o| o.type_id.clone())
            .unwrap_or_default();
        if !doc.is_group_like(self.id) && !is_link_group_type(&type_id) {
            return Err(PyAttributeError::new_err(format!(
                "'{type_id}' object has no attribute 'removeObject'"
            )));
        }

        let mut links = match doc.object(self.id).and_then(|o| o.properties.get("Group")) {
            Some(Property::LinkList(links)) => links.clone(),
            _ => vec![],
        };
        let before = links.len();
        links.retain(|name| name != &other_name);
        if links.len() == before {
            // Upstream returns the list of removed objects: a non-member is a no-op.
            return Ok(Vec::new());
        }
        doc.set_property(self.id, "Group", Property::LinkList(links))
            .map_err(PyValueError::new_err)?;
        drop(doc);

        let doc_py = self.doc.clone_ref(py);
        Ok(vec![get_or_create_object(
            py,
            &doc_py,
            &self.inner,
            other_id,
        )])
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
        parent_of(slf, ParentScope::Plain)
    }

    /// The geo-feature group (`App::Part`, `PartDesign::Body`) that lists this object.
    fn getParentGeoFeatureGroup(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        parent_of(slf, ParentScope::Geo)
    }

    /// `getParent()`: the first group (plain, part or body) this object belongs to.
    fn getParent(slf: &Bound<'_, Self>) -> Option<Py<PyDocumentObject>> {
        parent_of(slf, ParentScope::Any)
    }

    /// `App::Link.setLink(obj)`: point `LinkedObject` at `obj` (or clear it with
    /// `None`). `subname` narrows the link to one sub-element of the target.
    #[pyo3(signature = (value, subname=None))]
    fn setLink(
        slf: &Bound<'_, Self>,
        value: &Bound<'_, PyAny>,
        subname: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let (inner, id, type_id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id, {
                let doc = this.inner.lock().unwrap();
                doc.object(this.id)
                    .map(|object| object.type_id.clone())
                    .unwrap_or_default()
            })
        };
        if !is_link_type(&type_id) {
            return Err(PyAttributeError::new_err(format!(
                "'{type_id}' object has no attribute 'setLink'"
            )));
        }
        let name = if value.is_none() {
            String::new()
        } else if let Ok(object) = value.extract::<PyRef<'_, PyDocumentObject>>() {
            object.name_string()
        } else if let Ok(name) = value.extract::<String>() {
            name
        } else {
            return Err(PyTypeError::new_err(
                "setLink expects a DocumentObject, a name, or None",
            ));
        };
        let property = match subname {
            Some(sub) if !sub.is_none() => {
                let subs: Vec<String> = if let Ok(list) = sub.extract::<Vec<String>>() {
                    list
                } else if let Ok(one) = sub.extract::<String>() {
                    vec![one]
                } else {
                    return Err(PyTypeError::new_err(
                        "setLink's subname must be a string or a list of strings",
                    ));
                };
                Property::LinkSub(name, subs)
            }
            _ => Property::Link(name),
        };
        inner
            .lock()
            .unwrap()
            .set_property(id, "LinkedObject", property)
            .map_err(PyValueError::new_err)
    }

    /// `isLink()`: true for `App::Link` and its registered containers, and for
    /// the `Part::LinkArray*` types.
    ///
    /// The arrays are FerroCAD **peers**, not `App::Link` subclasses yet (they
    /// are separate `ObjectType`s); the local [`is_link_type`] table treats them
    /// as links, so a saved `Part::LinkArray*` answers like its upstream base
    /// class.
    fn isLink(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .object(self.id)
            .map(|object| is_link_type(&object.type_id))
            .unwrap_or(false)
    }

    /// `getLinkedObject(resolve=True)`: the object a link points at.
    ///
    /// - A non-link object returns itself.
    /// - `resolve=False` returns the *immediate* target, which may be another
    ///   link; an empty or dangling `LinkedObject` returns `None`.
    /// - `resolve=True` (the default) follows `LinkedObject` chains to the final
    ///   non-link object, returning `None` for an empty/dangling link or a cycle.
    ///
    /// Only the object is resolved: a `LinkSub`'s sub-element names do not enter
    /// here (sub-element resolution is L5).
    #[pyo3(signature = (resolve=true))]
    fn getLinkedObject(slf: &Bound<'_, Self>, resolve: bool) -> Option<Py<PyDocumentObject>> {
        let py = slf.py();
        let inner = Arc::clone(&slf.borrow().inner);
        let doc_py = slf.borrow().doc.clone_ref(py);
        let start = slf.borrow().id;
        let doc = inner.lock().unwrap();

        let is_link = |id: ObjectId| {
            doc.object(id)
                .map(|object| is_link_type(&object.type_id))
                .unwrap_or(false)
        };

        if !is_link(start) {
            return Some(get_or_create_object(py, &doc_py, &inner, start));
        }

        let mut current = start;
        // The bound is a guard against a malformed cycle; upstream resolution is
        // bounded by the dependency graph, which rejects cycles separately.
        for _ in 0..64 {
            let next = linked_object_id(&doc, current)?;
            if !resolve {
                return Some(get_or_create_object(py, &doc_py, &inner, next));
            }
            if !is_link(next) {
                return Some(get_or_create_object(py, &doc_py, &inner, next));
            }
            current = next;
        }
        None
    }

    /// `getElementIndex(name)`: the index in an element reference.
    ///
    /// `"Element3"`, `"Array.Element3"`, `"Array_Element3"` and the bare
    /// upstream form `"3."` all report `3`; anything else reports `-1`
    /// (upstream's failure value).
    #[staticmethod]
    fn getElementIndex(name: &str) -> i64 {
        element_index(name)
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
                                Property::LinkSubList(entries) => {
                                    entries.iter().any(|(obj, _)| obj == &name)
                                }
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
    /// 0 → the sub-shape (`Part.Edge`/`Part.Face`, …) when `subname` names an
    /// element, else the DocumentObject; 1 → DocumentObject; 2 →
    /// `(obj, Matrix, None)`; 3 → Placement; 4 → Matrix.
    ///
    /// `subname` may be:
    ///
    /// - empty → this object;
    /// - a whole document-object name (the original behaviour, e.g. an
    ///   `App::Origin` child `X_Axis`, with or without a trailing `.`);
    /// - a dotted path relative to this object (`Element3`, `Element3.Face1`,
    ///   `Array.Element3`, `Group.Child`), where each segment is an
    ///   `ElementList` child (by index spelling or stored name), a `Group`
    ///   member, or forwarded through `LinkedObject`;
    /// - a path into a link's target (`Link.Face1`), which resolves to the object
    ///   that owns the sub-element.
    ///
    /// The sub-shape is produced by the module that owns the object's `Shape`
    /// value (see [`register_sub_element_resolver`]); a module that is not loaded,
    /// or a name its value does not recognise, falls back to the object.
    ///
    /// The placement reported with `retType` 2/3/4 is the resolved object's own
    /// `Placement` (identity if it has none); intermediate link placements are not
    /// accumulated yet. `matrix`, `transform` and `depth` are accepted and ignored.
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
        let self_id = slf.borrow().id;

        // A resolved sub-element also carries its object's `Shape` value out of
        // the lock, so the module's resolver (which builds a `Part.Edge`, …) runs
        // without holding the document.
        struct Entry {
            sub: Option<ResolvedSub>,
            placement: Placement,
            element_value: Option<ExtensionValue>,
        }

        let resolved: Vec<Entry> = {
            let doc = inner.lock().unwrap();
            subs.iter()
                .map(|sub| match resolve_sub_name(&doc, self_id, sub) {
                    Some(sub) => {
                        let placement = placement_of(&doc, sub.id);
                        let element_value = sub.element.as_ref().and_then(|_| {
                            match doc
                                .object(sub.id)
                                .and_then(|object| object.properties.get("Shape"))
                            {
                                Some(Property::Extension(value)) => Some(value.clone()),
                                _ => None,
                            }
                        });
                        Entry {
                            sub: Some(sub),
                            placement,
                            element_value,
                        }
                    }
                    None => Entry {
                        sub: None,
                        placement: Placement::identity(),
                        element_value: None,
                    },
                })
                .collect()
        };

        let build = |entry: &Entry| -> PyObject {
            let obj_py = entry
                .sub
                .as_ref()
                .map(|sub| {
                    get_or_create_object(py, &doc_py, &inner, sub.id)
                        .into_py_any(py)
                        .unwrap()
                })
                .unwrap_or_else(|| py.None());
            match retType {
                0 => {
                    // A sub-element reference returns the element itself, through
                    // the module that owns the `Shape` value; a whole-object
                    // reference (empty subname, `Element3`, `Group.Child`) returns
                    // the object.
                    if let (Some(sub), Some(value)) = (&entry.sub, &entry.element_value) {
                        if let Some(element) = sub.element.as_deref() {
                            if let Some(element) = sub_element_to_py(py, value, element) {
                                return element;
                            }
                        }
                    }
                    obj_py
                }
                1 => obj_py,
                2 => {
                    let mat_py = PyMatrix {
                        inner: entry.placement.to_matrix(),
                    }
                    .into_py_any(py)
                    .unwrap();
                    (obj_py, mat_py, py.None()).into_py_any(py).unwrap()
                }
                3 => PyPlacement::fresh(entry.placement).into_py_any(py).unwrap(),
                4 => PyMatrix {
                    inner: entry.placement.to_matrix(),
                }
                .into_py_any(py)
                .unwrap(),
                _ => py.None(),
            }
        };

        if resolved.len() == 1 {
            Ok(build(&resolved[0]))
        } else {
            let items: Vec<PyObject> = resolved.iter().map(build).collect();
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
        // `Proxy` is an instance attribute (upstream `App::FeaturePython` stores
        // it there, not as a document property). A scripted feature without a
        // proxy reports `None`, as upstream does; other object types keep the
        // normal `AttributeError` for an unknown attribute.
        if name == "Proxy" {
            let type_id = {
                let this = slf.borrow();
                let doc = this.inner.lock().unwrap();
                doc.object(this.id)
                    .map(|object| object.type_id.clone())
                    .unwrap_or_default()
            };
            if type_id.ends_with("Python") {
                let dict = slf.getattr("__dict__")?;
                let dict = dict.downcast_into::<PyDict>()?;
                return Ok(match dict.get_item("Proxy")? {
                    Some(proxy) => proxy.unbind(),
                    None => py.None(),
                });
            }
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
            // The subnames are returned exactly as stored: they are opaque
            // references resolved at use time (see `getSubObject`), never
            // rewritten on read, so a native/`.FCStd` round trip is lossless
            // even when the target's lineage retires a name after a rebuild.
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
            // LinkSubList exposes `[(object_or_None, (subnames,)), …]`, grouped
            // per object like upstream `PropertyLinkSubList::getPyObject`. A
            // whole-object entry carries the empty subname `("",)`.
            Some(Property::LinkSubList(entries)) => {
                let items: Vec<PyObject> = entries
                    .iter()
                    .map(|(name, subs)| {
                        let obj_py = if name.is_empty() {
                            py.None()
                        } else {
                            match inner.lock().unwrap().get_by_name(name) {
                                Some(id) => get_or_create_object(py, &doc_py, &inner, id)
                                    .into_py_any(py)
                                    .unwrap(),
                                None => py.None(),
                            }
                        };
                        let subs = PyTuple::new(py, subs.iter().map(String::as_str)).unwrap();
                        (obj_py, subs).into_py_any(py).unwrap()
                    })
                    .collect();
                Ok(items.into_py_any(py).unwrap())
            }
            Some(p) => Ok(property_to_py_at(py, &p, &inner, this_id, &name)),
            None => {
                // A method registered by one of the object's extensions is exposed
                // as a callable, so modules install their whole Python surface via
                // the registry (`positionBySupport`, …) without editing this crate.
                if extension_method_for(&inner.lock().unwrap(), this_id, &name).is_some() {
                    let method_inner = Arc::clone(&inner);
                    let method_name = name.clone();
                    let function = pyo3::types::PyCFunction::new_closure(
                        py,
                        None,
                        None,
                        move |args: &Bound<'_, PyTuple>,
                              kwargs: Option<&Bound<'_, PyDict>>| {
                            let mut items: Vec<Bound<'_, PyAny>> = args.iter().collect();
                            if let Some(kwargs) = kwargs {
                                if !kwargs.is_empty() {
                                    items.push(kwargs.clone().into_any());
                                }
                            }
                            let call = PyTuple::new(args.py(), items)?;
                            let mut doc = method_inner.lock().unwrap();
                            let (_, f) = extension_method_for(&doc, this_id, &method_name)
                                .ok_or_else(|| {
                                    PyAttributeError::new_err(format!(
                                        "object has no attribute '{method_name}'"
                                    ))
                                })?;
                            f(&mut doc, this_id, &method_name, call.as_any())
                        },
                    )?;
                    return Ok(function.into_any().unbind());
                }
                // View-provider properties (`obj.Visibility`, `obj.Transparency`, …)
                // are forwarded in a GUI session upstream; here the view model is
                // always present.
                if VIEW_PROPERTIES.contains(&name.as_str()) {
                    let view = {
                        let this = slf.borrow();
                        view_object_for(py, &this)
                    };
                    if let Ok(view) = view {
                        return Ok(view.bind(py).getattr(name.as_str())?.unbind());
                    }
                }
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
            call_proxy_method(slf, "onChanged", Some("Label"));
            return Ok(());
        }
        if name == "Group" {
            slf.borrow().set_group(value)?;
            call_proxy_method(slf, "onChanged", Some("Group"));
            return Ok(());
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
            Some(Property::LinkSubList(_)) => {
                Property::LinkSubList(py_to_link_sub_list(value)?)
            }
            // A bare string is a one-element string list (`obj.Text = "Label"`).
            Some(Property::StringList(_)) => match value.extract::<String>() {
                Ok(s) => Property::StringList(vec![s]),
                Err(_) => py_to_property(value)?,
            },
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
            // A module-owned value keeps its type: try the converter registered for
            // it before the generic guesses (`obj.Shapes = []` must stay a shape
            // list, not become an `App::PropertyBoolList`).
            Some(Property::Extension(ext)) => {
                match extension_from_py_for(ext.type_name(), value) {
                    Some(v) => Property::Extension(v),
                    None => py_to_property(value)?,
                }
            }
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
            // View-provider properties are forwarded to `ViewObject` (upstream's
            // GUI-only `DocumentObjectPy` behaves the same way).
            None if VIEW_PROPERTIES.contains(&name.as_str()) => {
                let view = {
                    let this = slf.borrow();
                    view_object_for(slf.py(), &this)
                }?;
                view.bind(slf.py()).setattr(name.as_str(), value)?;
                return Ok(());
            }
            // Not a known property: store it as a Python attribute (`Proxy`, …).
            None => return set_instance_attr(slf, &name, value),
        };
        let (inner, id) = {
            let this = slf.borrow();
            (Arc::clone(&this.inner), this.id)
        };
        fire_obj_str("slotBeforeChangeObject", slf, &name);
        let result = inner.lock().unwrap().set_property(id, &name, property);
        result.map_err(property_change_error)?;
        fire_obj_str("slotChangedObject", slf, &name);
        // FreeCAD's `DocumentObject::onChanged` hook: a Python feature observes
        // property writes here (`proxy.onChanged(obj, prop)`).
        call_proxy_method(slf, "onChanged", Some(&name));
        Ok(())
    }

    /// Assign the `Group` link list from a sequence of objects (or names).
    fn set_group(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let names: Vec<String> = if let Ok(object) = value.downcast::<PyDocumentObject>() {
            vec![object.borrow().name_string()]
        } else if let Ok(objs) = value.extract::<Vec<PyRef<'_, PyDocumentObject>>>() {
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
        let type_id = doc
            .object(self.id)
            .map(|object| object.type_id.clone())
            .unwrap_or_default();
        let members: Vec<ObjectId> = names
            .iter()
            .filter_map(|name| doc.get_by_name(name))
            .collect();
        doc.set_property(self.id, "Group", Property::LinkList(names))
            .map_err(PyValueError::new_err)?;
        // A group's membership does not order recompute (core deliberately skips
        // `Group`), but a link container derives its *shape* from its members, so
        // it must run after them. Record the edges only for the containers, which
        // is the same distinction as upstream's `isLinkGroup()`.
        if is_link_group_type(&type_id) {
            for member in members {
                if member != self.id {
                    doc.add_dependency(self.id, member);
                }
            }
        }
        Ok(())
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
    target: ViewTarget,
    /// The `Placement` the view was derived from, so a `Base`/`Rotation` sub-field
    /// write can be merged back into the whole value. `None` for a whole-value view.
    expect_placement: Option<Placement>,
    kind: ViewKind,
}

/// Where a live geometry view writes its new value.
#[derive(Clone)]
enum ViewTarget {
    /// A geometry property on a document. A write is dropped once the property
    /// has been reassigned (its version changed), even if the new value compares
    /// equal.
    Document {
        inner: Arc<Mutex<CoreDocument>>,
        id: ObjectId,
        prop: String,
        version: u64,
    },
    /// A module-owned value (currently `Part.Shape`'s accumulated placement)
    /// that installs its own write-back hook.
    Hook(Arc<dyn Fn(Property) + Send + Sync>),
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
        let new = match self.kind {
            ViewKind::Placement | ViewKind::Rotation => value,
            ViewKind::PlacementBase | ViewKind::PlacementRotation => {
                let Some(mut placement) = self.expect_placement else {
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
        match &self.target {
            ViewTarget::Document { inner, id, prop, version } => {
                let mut doc = inner.lock().unwrap();
                if doc.property_version(*id, prop) != *version {
                    return;
                }
                let _ = doc.set_property(*id, prop, new);
            }
            ViewTarget::Hook(hook) => hook(new),
        }
    }
}

#[pyclass(name = "Vector", module = "ferrocad")]
#[derive(Clone)]
pub struct PyVector {
    inner: Vector3,
    /// Set when this vector is a live view of a geometry property.
    view: Option<GeometryView>,
}

impl PyVector {
    /// A detached vector value.
    ///
    /// Public so sibling Python modules (`Part`, `Mesh`, …) can return
    /// `Base.Vector`-compatible results without duplicating the class.
    pub fn fresh(inner: Vector3) -> Self {
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

    /// `multiply(factor)`: scale in place (upstream mutates and returns self).
    fn multiply(&mut self, factor: f64) -> PyVector {
        self.inner = self.inner.scale(factor);
        self.write_back();
        PyVector::fresh(self.inner)
    }

    /// `scale(factor)` or `scale(x, y, z)`: scale in place (upstream mutates and
    /// returns self). Draft's fillets use the three-argument form.
    #[pyo3(signature = (x, y=None, z=None))]
    fn scale(&mut self, x: f64, y: Option<f64>, z: Option<f64>) -> PyVector {
        self.inner = match (y, z) {
            (Some(y), Some(z)) => Vector3::new(
                self.inner.x * x,
                self.inner.y * y,
                self.inner.z * z,
            ),
            _ => self.inner.scale(x),
        };
        self.write_back();
        PyVector::fresh(self.inner)
    }

    /// `normalize()`: normalise in place and return self (upstream's semantics).
    fn normalize(&mut self) -> PyVector {
        self.inner = self.inner.normalize();
        self.write_back();
        PyVector::fresh(self.inner)
    }

    /// `distanceToLine(base, direction)`: distance from this point to the line.
    fn distanceToLine(&self, base: PyRef<'_, PyVector>, direction: PyRef<'_, PyVector>) -> f64 {
        let offset = self.inner.sub(&base.inner);
        let length = direction.inner.length();
        if length == 0.0 {
            offset.length()
        } else {
            offset.cross(&direction.inner).length() / length
        }
    }

    /// `projectToPlane(base, normal)`: project in place onto the plane through
    /// `base` with the given normal.
    fn projectToPlane(
        &mut self,
        base: PyRef<'_, PyVector>,
        normal: PyRef<'_, PyVector>,
    ) -> PyVector {
        let denominator = normal.inner.dot(&normal.inner);
        if denominator != 0.0 {
            let distance = self.inner.sub(&base.inner).dot(&normal.inner) / denominator;
            self.inner = self.inner.sub(&normal.inner.scale(distance));
            self.write_back();
        }
        PyVector::fresh(self.inner)
    }

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

/// FreeCAD's Matrix prints with `ostream`'s default 6 significant digits
/// (1.66667, 0.0, 1.0). Rust has no `%g`, so pick the decimal count from the
/// magnitude and trim trailing zeros.
fn format_matrix_value(value: f64) -> String {
    if value == 0.0 {
        return "0.0".to_string();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).clamp(0, 17) as usize;
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.push('0');
        }
    }
    text
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
            return Ok(PyMatrix { inner: self.inner.mul(&p.get().to_matrix()) }
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
        let values: Vec<String> = self
            .inner
            .m
            .iter()
            .map(|value| format_matrix_value(*value))
            .collect();
        format!("Matrix ([{}])", values.join(", "))
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

/// `Rotation::makeRotationByAxes` (FreeCAD `Rotation.cpp`): build a rotation from
/// three direction vectors, honouring a three-letter priority order such as
/// `"ZXY"`. The first non-degenerate priority direction is the main axis, the
/// next supplies a perpendicular hint, and the third completes a right-handed
/// basis.
fn rotation_by_axes(
    xdir: Vector3,
    ydir: Vector3,
    zdir: Vector3,
    priority: &str,
) -> Result<Rotation, String> {
    const TOL: f64 = 1e-7;
    if priority.len() != 3 {
        return Err("makeRotationByAxes: priorityOrder must have length 3".to_string());
    }
    let bytes = priority.as_bytes();
    let mut order = [0usize; 3];
    for (i, &c) in bytes.iter().enumerate() {
        if !(b'X'..=b'Z').contains(&c) {
            return Err(format!(
                "makeRotationByAxes: {priority:?} must contain only uppercase X, Y, Z"
            ));
        }
        order[i] = (c - b'X') as usize;
    }
    if order[0] == order[1] || order[1] == order[2] || order[2] == order[0] {
        return Err("makeRotationByAxes: not all axes are listed in priorityOrder".to_string());
    }
    let dirs = [xdir, ydir, zdir];

    fn drop_priority(order: &mut [usize; 3], index: usize) {
        match index {
            0 => order.rotate_left(1),
            1 => order.swap(1, 2),
            _ => {}
        }
    }

    let mut main = Vector3::zero();
    for i in 0..3 {
        main = dirs[order[0]];
        if main.length() > TOL {
            break;
        }
        drop_priority(&mut order, 0);
        if i == 2 {
            return Err("makeRotationByAxes: all directions are zero".to_string());
        }
    }
    let main = main.normalize();

    let mut hint = Vector3::zero();
    for i in 0..2 {
        hint = dirs[order[1]];
        if hint.cross(&main).length() > TOL {
            break;
        }
        drop_priority(&mut order, 1);
        if i == 1 {
            hint = Vector3::zero();
        }
    }
    if hint.length() == 0.0 {
        // The supplied hint is parallel to the main direction: choose one.
        match order[0] {
            0 => {
                order[1] = 2;
                order[2] = 1;
                hint = Vector3::new(0.0, 0.0, 1.0);
                if hint.cross(&main).length() <= TOL {
                    hint = Vector3::new(0.0, 1.0, 0.0);
                    order[1] = 1;
                    order[2] = 2;
                }
            }
            1 => {
                order[1] = 2;
                order[2] = 0;
                hint = if main.z > -TOL {
                    Vector3::new(0.0, 0.0, 1.0)
                } else {
                    Vector3::new(0.0, 0.0, -1.0)
                };
                if hint.cross(&main).length() <= TOL {
                    hint = Vector3::new(1.0, 0.0, 0.0);
                    order[1] = 0;
                    order[2] = 2;
                }
            }
            _ => {
                order[1] = 1;
                order[2] = 0;
                hint = Vector3::new(0.0, 0.0, 1.0);
                if hint.cross(&main).length() <= TOL {
                    hint = Vector3::new(1.0, 0.0, 0.0);
                    order[1] = 0;
                    order[2] = 1;
                }
            }
        }
    }

    let hint = hint.normalize();
    let last = main.cross(&hint).normalize();
    let hint = last.cross(&main).normalize();

    let mut finaldirs = [Vector3::zero(); 3];
    finaldirs[order[0]] = main;
    finaldirs[order[1]] = hint;
    finaldirs[order[2]] = last;
    if finaldirs[0].cross(&finaldirs[1]).dot(&finaldirs[2]) < 0.0 {
        finaldirs[order[2]] = finaldirs[order[2]].neg();
    }

    let mut matrix = Matrix4::identity();
    for i in 0..3 {
        matrix.m[i] = finaldirs[i].x;
        matrix.m[4 + i] = finaldirs[i].y;
        matrix.m[8 + i] = finaldirs[i].z;
    }
    Ok(Rotation::from_matrix(&matrix))
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
                // `Rotation(axis, degree)` (degrees) or `Rotation(from, to)`.
                let first = args.get_item(0)?;
                let second = args.get_item(1)?;
                if let (Ok(from), Ok(to)) = (
                    first.extract::<PyRef<'_, PyVector>>(),
                    second.extract::<PyRef<'_, PyVector>>(),
                ) {
                    Rotation::from_vectors(from.inner, to.inner)
                } else {
                    let axis: PyRef<'_, PyVector> = first.extract()?;
                    let degree: f64 = second.extract()?;
                    Rotation::from_axis_angle(&axis.inner, degree.to_radians())
                }
            }
            3 => {
                // `Rotation(yaw, pitch, roll)` or three direction vectors.
                let a = args.get_item(0)?;
                let b = args.get_item(1)?;
                let c = args.get_item(2)?;
                if let (Ok(va), Ok(vb), Ok(vc)) = (
                    a.extract::<PyRef<'_, PyVector>>(),
                    b.extract::<PyRef<'_, PyVector>>(),
                    c.extract::<PyRef<'_, PyVector>>(),
                ) {
                    rotation_by_axes(va.inner, vb.inner, vc.inner, "ZXY")
                        .map_err(PyValueError::new_err)?
                } else {
                    let yaw: f64 = a.extract()?;
                    let pitch: f64 = b.extract()?;
                    let roll: f64 = c.extract()?;
                    Rotation::from_euler_deg(yaw, pitch, roll)
                }
            }
            4 => {
                // `Rotation(x, y, z, w)` (quaternion) or
                // `Rotation(xdir, ydir, zdir, priority)`.
                let a = args.get_item(0)?;
                let b = args.get_item(1)?;
                let c = args.get_item(2)?;
                let d = args.get_item(3)?;
                if let (Ok(va), Ok(vb), Ok(vc), Ok(priority)) = (
                    a.extract::<PyRef<'_, PyVector>>(),
                    b.extract::<PyRef<'_, PyVector>>(),
                    c.extract::<PyRef<'_, PyVector>>(),
                    d.extract::<String>(),
                ) {
                    rotation_by_axes(va.inner, vb.inner, vc.inner, &priority)
                        .map_err(PyValueError::new_err)?
                } else {
                    let x: f64 = a.extract()?;
                    let y: f64 = b.extract()?;
                    let z: f64 = c.extract()?;
                    let w: f64 = d.extract()?;
                    Rotation { q: [w, x, y, z], raw_axis: None }
                }
            }
            9 => {
                // `Rotation(a11 … a33)`: the upper-left 3x3 of a 4x4 matrix.
                let mut m = Matrix4::identity();
                for row in 0..3 {
                    for column in 0..3 {
                        m.m[row * 4 + column] =
                            args.get_item(row * 3 + column)?.extract()?;
                    }
                }
                Rotation::from_matrix(&m)
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
                    "Rotation() takes 0, 1, 2, 3, 4, 9 or 16 arguments",
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

    /// Rotate a vector (FreeCAD `Rotation.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector::fresh(self.inner.to_matrix().transform(&v.inner))
    }

    /// `Rotation * Rotation` composes; `Rotation * Vector` rotates the vector.
    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        if let Ok(rotation) = other.extract::<PyRef<'_, PyRotation>>() {
            return Ok(Py::new(other.py(), self.multiply(rotation))?.into_any());
        }
        if let Ok(vector) = other.extract::<PyRef<'_, PyVector>>() {
            let rotated = self.inner.to_matrix().transform(&vector.inner);
            return Ok(Py::new(other.py(), PyVector::fresh(rotated))?.into_any());
        }
        Err(PyTypeError::new_err(
            "Rotation can only multiply a Rotation or a Vector",
        ))
    }

    fn invert(&mut self) {
        self.inner = self.inner.inverse();
        self.axis_cache = None;
    }

    fn inverse(&self) -> PyRotation {
        PyRotation::fresh(self.inner.inverse(), None)
    }

    /// `inverted()`: a new rotation with the inverse value (the quaternion
    /// conjugate). Upstream's `Rotation::inverse()` also negates the retained
    /// axis, so an axis set for an angle-0 rotation survives the inversion.
    fn inverted(&self) -> PyRotation {
        PyRotation::fresh(
            self.inner.inverse(),
            self.axis_cache.map(|axis| axis.neg()),
        )
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
pub struct PyPlacement {
    /// Shared so a detached placement's `Rotation`/`Base` sub-object can write
    /// back through the parent (`p.Rotation.Angle = …`).
    inner: Arc<Mutex<Placement>>,
    /// Set when this placement is a live view of a geometry property.
    view: Option<GeometryView>,
}

impl PyPlacement {
    /// A detached placement value.
    ///
    /// Public so sibling Python modules (`Part`, `Mesh`, …) can return
    /// `Base.Placement`-compatible results without duplicating the class.
    pub fn fresh(inner: Placement) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            view: None,
        }
    }

    /// The current placement value.
    ///
    /// Public so sibling Python modules can read a detached placement back into
    /// their own payload.
    pub fn get(&self) -> Placement {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the current placement value.
    fn set(&self, value: Placement) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }

    /// Propagate a mutation to the backing geometry property, if any.
    fn write_back(&self) {
        if let Some(view) = &self.view {
            view.write(Property::Placement(self.get()));
        }
    }

    /// The view a `Base`/`Rotation` sub-object writes through.
    ///
    /// A document/`Shape` view reuses its own target. A **detached** placement
    /// gets a local hook into this placement's shared cell, so
    /// `p.Rotation.Angle = …` and `p.Base.x = …` stick to `p` itself instead of
    /// mutating a throw-away copy.
    fn child_view(&self, kind: ViewKind) -> GeometryView {
        match &self.view {
            Some(view) => view.child(kind),
            None => {
                let cell = Arc::clone(&self.inner);
                GeometryView {
                    target: ViewTarget::Hook(Arc::new(move |property| {
                        if let Property::Placement(placement) = property {
                            *cell.lock().unwrap_or_else(|e| e.into_inner()) = placement;
                        }
                    })),
                    expect_placement: Some(self.get()),
                    kind,
                }
            }
        }
    }
}

/// Compose two placements the way FreeCAD's `Placement::operator*` does:
/// `a * b` applies `b` first, then `a`. In particular the rotation part is
/// `a.rotation * b.rotation` and the base is `a.rotation * b.base + a.base`.
///
/// This is deliberately not `Placement::mul` from `ferrocad_types` (which is a
/// placeholder that drops the rotation composition).
fn compose_placements(a: &Placement, b: &Placement) -> Placement {
    Placement {
        base: a.base.add(&a.rotation.to_matrix().transform(&b.base)),
        rotation: a.rotation.multiply(&b.rotation),
    }
}

/// Build a live `Placement` value whose mutations are reported to `on_change`.
///
/// This is the write-through path for module-owned geometry that is not a
/// document property. `Part.Shape` uses it so
/// `shape.Placement.Base = …` and `shape.Placement.Rotation.Angle = …` mutate the
/// shape's accumulated placement, exactly as the document geometry views do for
/// `obj.Placement`. Sub-field writes (`Base.x`, `Rotation.Angle`) merge into the
/// whole placement before the hook sees it.
///
/// The returned value is detached from the document property machinery: the
/// caller owns the callback and decides what an update means (Part re-bakes its
/// geometry by the placement delta).
pub fn placement_with_hook(
    py: Python<'_>,
    value: Placement,
    on_change: Arc<dyn Fn(Placement) + Send + Sync>,
) -> PyResult<PyObject> {
    let view = GeometryView {
        target: ViewTarget::Hook(Arc::new(move |property| {
            if let Property::Placement(placement) = property {
                on_change(placement);
            }
        })),
        expect_placement: Some(value),
        kind: ViewKind::Placement,
    };
    PyPlacement {
        inner: Arc::new(Mutex::new(value)),
        view: Some(view),
    }
    .into_py_any(py)
}

#[pymethods]
impl PyPlacement {
    #[new]
    #[pyo3(signature = (base=None, rotation=None, center=None))]
    fn new(
        base: Option<&Bound<'_, PyAny>>,
        rotation: Option<&Bound<'_, PyAny>>,
        center: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        // `Placement(otherPlacement)` copies the base and rotation (upstream's
        // copy constructor), and `Placement(matrix)` builds from a 4x4 matrix.
        if rotation.is_none() && center.is_none() {
            if let Some(b) = base {
                if let Ok(other) = b.extract::<PyRef<'_, PyPlacement>>() {
                    return Ok(Self::fresh(other.get()));
                }
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
        // `Placement(base, rotation, center)` rotates about `center`: the base
        // becomes `center + base - rotation·center`, as upstream does.
        let b = match center {
            Some(c) => {
                let c = extract_vector(c)?;
                c.add(&b).sub(&r.to_matrix().transform(&c))
            }
            None => b,
        };
        Ok(Self::fresh(Placement::new(b, r)))
    }

    fn inverse(&self) -> PyPlacement {
        PyPlacement::fresh(self.get().inverse())
    }

    /// A detached copy (`Placement.copy`).
    fn copy(&self) -> PyPlacement {
        PyPlacement::fresh(self.get())
    }

    /// Rotate this placement in place (`Placement.rotate`).
    ///
    /// With `comp=False` (the default, and upstream's compatibility behaviour)
    /// the rotation is applied in the placement's own frame, `p = p * r`; with
    /// `comp=True` it is applied in the global frame, `p = r * p`, which is what
    /// `Shape.rotate` does.
    #[pyo3(signature = (center, axis, angle, comp = false))]
    fn rotate(
        &mut self,
        center: &Bound<'_, PyAny>,
        axis: &Bound<'_, PyAny>,
        angle: f64,
        comp: bool,
    ) -> PyResult<()> {
        let center = extract_vector(center)?;
        let axis = extract_vector(axis)?;
        if axis.length() == 0.0 {
            return Err(PyValueError::new_err("rotation axis must not be zero"));
        }
        let rotation = Rotation::from_axis_angle(&axis, angle.to_radians());
        // A placement rotating about `center`: its base is `center - R * center`.
        let about = Placement::new(center.sub(&rotation.to_matrix().transform(&center)), rotation);
        let current = self.get();
        let new = if comp {
            compose_placements(&about, &current)
        } else {
            compose_placements(&current, &about)
        };
        self.set(new);
        self.write_back();
        Ok(())
    }

    fn toMatrix(&self) -> PyMatrix {
        PyMatrix { inner: self.get().to_matrix() }
    }

    /// The same matrix as an attribute (`Placement.Matrix`), which upstream code
    /// (`Draft.draftobjects.hatch`) reads as a property.
    #[getter]
    fn Matrix(&self) -> PyMatrix {
        PyMatrix { inner: self.get().to_matrix() }
    }

    /// Move this placement in place (`Placement.translate`).
    ///
    /// Accepts a `Vector` or three floats. The rotation is unchanged and the base
    /// moves in the global frame, matching upstream's `Placement.translate`.
    #[pyo3(signature = (*args))]
    fn translate(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        let offset = match args.len() {
            1 => extract_vector(&args.get_item(0)?)?,
            3 => Vector3::new(
                args.get_item(0)?.extract()?,
                args.get_item(1)?.extract()?,
                args.get_item(2)?.extract()?,
            ),
            _ => {
                return Err(PyTypeError::new_err(
                    "translate expects a vector or three floats",
                ));
            }
        };
        let mut placement = self.get();
        placement.base = placement.base.add(&offset);
        self.set(placement);
        self.write_back();
        Ok(())
    }

    #[pyo3(signature = (o, tol=0.0))]
    fn isSame(&self, o: PyRef<'_, PyPlacement>, tol: f64) -> bool {
        self.get().is_same(&o.get(), tol)
    }

    #[getter]
    fn Base(&self) -> PyVector {
        let mut v = PyVector::fresh(self.get().base);
        v.view = Some(self.child_view(ViewKind::PlacementBase));
        v
    }
    #[setter]
    fn set_Base(&self, v: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut placement = self.get();
        placement.base = extract_vector(v)?;
        self.set(placement);
        self.write_back();
        Ok(())
    }

    #[getter]
    fn Rotation(&self) -> PyRotation {
        let mut r = PyRotation::fresh(self.get().rotation, None);
        // Always install a view: a detached placement's `Rotation` writes back
        // into the placement itself, not into a throw-away copy.
        r.view = Some(self.child_view(ViewKind::PlacementRotation));
        r
    }
    #[setter]
    fn set_Rotation(&self, q: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut placement = self.get();
        placement.rotation = extract_rotation(q)?;
        self.set(placement);
        self.write_back();
        Ok(())
    }

    fn __mul__(&self, o: PyRef<'_, PyPlacement>) -> PyPlacement {
        PyPlacement::fresh(compose_placements(&self.get(), &o.get()))
    }

    /// Compose this placement with `other` (`Placement.multiply`).
    ///
    /// Upstream's `mod` selects what is composed:
    ///
    /// - `0` (default): the full placement, `self * other`;
    /// - `1`: the rotations only;
    /// - `2`: the translation only, `self.Base + self.Rotation * other.Base`.
    ///
    /// The result is a new placement; neither operand changes.
    #[pyo3(signature = (other, r#mod = 0))]
    fn multiply(&self, other: PyRef<'_, PyPlacement>, r#mod: i32) -> PyResult<PyPlacement> {
        let left = self.get();
        let right = other.get();
        let composed = match r#mod {
            0 => compose_placements(&left, &right),
            1 => Placement::new(left.base, left.rotation.multiply(&right.rotation)),
            2 => Placement::new(
                left.base.add(&left.rotation.to_matrix().transform(&right.base)),
                left.rotation,
            ),
            _ => {
                return Err(PyValueError::new_err(
                    "Placement.multiply mod must be 0 (all), 1 (rotation) or 2 (translation)",
                ));
            }
        };
        Ok(PyPlacement::fresh(composed))
    }

    /// Transform a vector by this placement (FreeCAD `Placement.multVec`).
    fn multVec(&self, v: PyRef<'_, PyVector>) -> PyVector {
        PyVector::fresh(self.get().to_matrix().transform(&v.inner))
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        match other.extract::<PyRef<'_, PyPlacement>>() {
            Ok(o) => self.get() == o.get(),
            Err(_) => false,
        }
    }

    fn __repr__(&self) -> String {
        let placement = self.get();
        format!("Placement ({:?}, {:?})", placement.base, placement.rotation.q)
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
#[derive(Clone)]
struct PyMaterial {
    diffuse: [f64; 4],
    /// When this handle came from a `ViewObject`, changes are written back to it
    /// (`ShapeAppearance[i].DiffuseColor = ...`). A plain `App.Material` has none.
    owner: Option<MaterialOwner>,
}

/// Which view-state slot a [`PyMaterial`] handle writes back to.
#[derive(Clone, Copy, Debug)]
enum MaterialSlot {
    /// `ShapeAppearance[index]`.
    Appearance(usize),
    /// `ShapeMaterial`.
    Material,
}

/// The document object a [`PyMaterial`] handle belongs to.
#[derive(Clone)]
struct MaterialOwner {
    inner: Arc<Mutex<CoreDocument>>,
    id: ObjectId,
    slot: MaterialSlot,
}

impl PyMaterial {
    /// A handle on `color` that writes back into `object`'s view state.
    fn owned(color: [f64; 4], inner: Arc<Mutex<CoreDocument>>, id: ObjectId, slot: MaterialSlot) -> Self {
        Self {
            diffuse: color,
            owner: Some(MaterialOwner { inner, id, slot }),
        }
    }
}

#[pymethods]
impl PyMaterial {
    #[new]
    fn new() -> Self {
        Self {
            diffuse: [0.8, 0.8, 0.8, 1.0],
            owner: None,
        }
    }

    #[getter]
    fn DiffuseColor(&self) -> (f64, f64, f64, f64) {
        (self.diffuse[0], self.diffuse[1], self.diffuse[2], self.diffuse[3])
    }

    #[setter]
    fn set_DiffuseColor(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let color = py_to_color(value)?;
        self.diffuse = color;
        // A colour change is not a transparency change (upstream PR 11866): only
        // the colour slot moves.
        if let Some(owner) = &self.owner {
            update_view_state(&owner.inner, owner.id, |state| match owner.slot {
                MaterialSlot::Appearance(index) => state.set_appearance_slot(index, color),
                MaterialSlot::Material => state.material = Some(color),
            });
        }
        Ok(())
    }

    /// The material's transparency in `[0, 1]` (`Material.Transparency`).
    ///
    /// Upstream keeps transparency separate from the diffuse colour and reports
    /// it as the complement of the alpha channel.
    #[getter]
    fn Transparency(&self) -> f64 {
        1.0 - self.diffuse[3]
    }

    #[setter]
    fn set_Transparency(&mut self, value: f64) -> PyResult<()> {
        if !(0.0..=1.0).contains(&value) {
            return Err(PyValueError::new_err(
                "Transparency must be between 0.0 and 1.0",
            ));
        }
        let mut color = self.diffuse;
        color[3] = 1.0 - value;
        self.diffuse = color;
        if let Some(owner) = &self.owner {
            update_view_state(&owner.inner, owner.id, |state| match owner.slot {
                MaterialSlot::Appearance(index) => state.set_appearance_slot(index, color),
                MaterialSlot::Material => state.material = Some(color),
            });
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

// ---------------------------------------------------------------------------
// ViewObject (the headless view provider)
// ---------------------------------------------------------------------------

/// Display modes a Part view provider offers (FreeCAD `ViewProviderPartExt`).
const DISPLAY_MODES: &[&str] = &["Flat Lines", "Shaded", "Wireframe", "Points"];

/// Properties a document object forwards to its view object when it has no
/// property of that name (`box.Visibility = False`, as upstream allows in a GUI
/// session).
const VIEW_PROPERTIES: &[&str] = &[
    "Visibility",
    "Transparency",
    "ShapeColor",
    "ShapeAppearance",
    "ShapeMaterial",
    "DiffuseColor",
    "DisplayMode",
];

/// The view object of one document object (`obj.ViewObject`).
///
/// Upstream creates a `Gui::ViewProvider` next to every object only in a GUI
/// session. FerroCAD keeps the same small model in every session: the state lives
/// in [`ViewState`](ferrocad_core::ViewState) on the object, and this class is the
/// Python face of it. `RootNode` is a real Coin scene graph when `pivy` is
/// importable (it mirrors the state); otherwise it is absent, exactly as a
/// Coin-less build has no scene graph.
#[pyclass(name = "ViewObject", module = "ferrocad")]
struct PyViewObject {
    doc: Py<PyDocument>,
    inner: Arc<Mutex<CoreDocument>>,
    id: ObjectId,
    /// The view provider proxy (`doc.addObject(..., viewProxy=...)`, upstream
    /// `ViewObject.Proxy`). Process state, never persisted by core.
    proxy: Mutex<Option<Py<PyAny>>>,
}

/// The canonical `ViewObject` for a document object, created once per object.
fn view_object_for(py: Python<'_>, obj: &PyDocumentObject) -> PyResult<Py<PyViewObject>> {
    if let Some(view) = obj.view.lock().unwrap().as_ref() {
        return Ok(view.clone_ref(py));
    }
    let view = Py::new(
        py,
        PyViewObject {
            doc: obj.doc.clone_ref(py),
            inner: Arc::clone(&obj.inner),
            id: obj.id,
            proxy: Mutex::new(None),
        },
    )?;
    *obj.view.lock().unwrap() = Some(view.clone_ref(py));
    Ok(view)
}

/// Apply a change to an object's view state, if it has one.
fn update_view_state(
    inner: &Arc<Mutex<CoreDocument>>,
    id: ObjectId,
    apply: impl FnOnce(&mut ferrocad_core::ViewState),
) -> bool {
    let mut doc = inner.lock().unwrap();
    match doc.object_mut(id).and_then(|object| object.view.as_mut()) {
        Some(state) => {
            apply(state);
            true
        }
        None => false,
    }
}

/// Datum objects (`App::Origin` and its axes/planes) are drawn opaque whatever the
/// shape-transparency preference says; upstream's origin view provider overrides it.
fn is_origin_family(type_id: &str) -> bool {
    matches!(type_id, "App::Origin" | "App::Plane" | "App::Line" | "App::Point")
}

/// The `DefaultShapeTransparency` / `DefaultShapeColor` preferences.
///
/// FreeCAD reads them once, when a view provider is constructed, so the values are
/// captured into the object's [`ViewState`](ferrocad_core::ViewState) and persisted
/// with it. Without the `FreeCAD` facade (a bare extension-module session) the
/// built-in defaults apply.
fn view_preferences(py: Python<'_>) -> (i32, [f64; 4]) {
    // FreeCAD's classic shape colour (`0xCCCCE6FF`), as Draft records it.
    const DEFAULT_COLOR: u32 = 0xCCCC_E6FF;
    let mut transparency = 0i32;
    let mut color = ferrocad_core::color_from_packed(DEFAULT_COLOR, true);
    if let Ok(freecad) = py.import("FreeCAD") {
        if let Ok(group) =
            freecad.call_method1("ParamGet", ("User parameter:BaseApp/Preferences/View",))
        {
            if let Ok(value) = group.call_method1("GetInt", ("DefaultShapeTransparency", 0)) {
                transparency = value.extract::<i64>().unwrap_or(0).clamp(0, 100) as i32;
            }
            if let Ok(value) =
                group.call_method1("GetUnsigned", ("DefaultShapeColor", DEFAULT_COLOR))
            {
                color = ferrocad_core::color_from_packed(
                    value.extract::<u32>().unwrap_or(DEFAULT_COLOR),
                    true,
                );
            }
        }
    }
    (transparency, color)
}

/// Coerce an RGBA tuple (3 or 4 components) or a packed `0xRRGGBBAA` integer.
fn py_to_color(value: &Bound<'_, PyAny>) -> PyResult<[f64; 4]> {
    if let Ok((r, g, b, a)) = value.extract::<(f64, f64, f64, f64)>() {
        return Ok([r, g, b, a]);
    }
    if let Ok((r, g, b)) = value.extract::<(f64, f64, f64)>() {
        return Ok([r, g, b, 1.0]);
    }
    if let Ok(packed) = value.extract::<u32>() {
        return Ok(ferrocad_core::color_from_packed(packed, false));
    }
    Err(PyTypeError::new_err(
        "expected an (r, g, b[, a]) colour or a packed 0xRRGGBBAA integer",
    ))
}

/// Coerce a `Material` or a colour.
fn py_to_material_color(value: &Bound<'_, PyAny>) -> PyResult<[f64; 4]> {
    if let Ok(material) = value.extract::<PyRef<'_, PyMaterial>>() {
        return Ok(material.diffuse);
    }
    py_to_color(value)
}

/// Coerce a list of materials (or colours); a single material becomes a 1-list.
fn py_to_view_colors(value: &Bound<'_, PyAny>) -> PyResult<Vec<[f64; 4]>> {
    if let Ok(material) = value.extract::<PyRef<'_, PyMaterial>>() {
        return Ok(vec![material.diffuse]);
    }
    let iterator = value
        .try_iter()
        .map_err(|_| PyTypeError::new_err("expected a list of materials or colours"))?;
    let mut colors = Vec::new();
    for item in iterator {
        let item = item?;
        if let Ok(material) = item.extract::<PyRef<'_, PyMaterial>>() {
            colors.push(material.diffuse);
        } else {
            colors.push(py_to_color(&item)?);
        }
    }
    Ok(colors)
}

/// The colour a view object paints its shape with.
///
/// An explicit `ShapeColor` wins. Otherwise, when every object this one links to
/// (`Shapes` for a `Part::MultiFuse`, `Base`/`Tool` for a boolean, `LinkedObject`
/// for an `App::Link`) carries the *same explicit* colour, that colour is used:
/// this is the headless analogue of upstream propagating the operands' face colours
/// through a boolean, and it is what keeps a fuse of two red objects red. Finally
/// the preference-derived default applies.
fn view_effective_color(doc: &CoreDocument, id: ObjectId) -> Option<[f64; 4]> {
    let object = doc.object(id)?;
    let state = object.view.as_ref()?;
    if let Some(color) = state.shape_color {
        return Some(color);
    }
    if let Some(color) = operand_color(doc, object) {
        return Some(color);
    }
    Some(state.default_color)
}

/// The common explicit `ShapeColor` of the objects `object` links to, if any.
fn operand_color(
    doc: &CoreDocument,
    object: &ferrocad_core::DocumentObject,
) -> Option<[f64; 4]> {
    let mut common: Option<[f64; 4]> = None;
    let mut any = false;
    for (name, property) in object.properties.iter() {
        // Group membership is not an appearance input.
        if name == "Group" {
            continue;
        }
        let targets: Vec<&String> = match property {
            Property::Link(target) => vec![target],
            Property::LinkList(targets) => targets.iter().collect(),
            Property::LinkSub(target, _) => vec![target],
            Property::LinkSubList(entries) => {
                entries.iter().map(|(target, _)| target).collect()
            }
            _ => continue,
        };
        for target in targets {
            if target.is_empty() {
                continue;
            }
            let Some(other) = doc.get_by_name(target).and_then(|id| doc.object(id)) else {
                continue;
            };
            // Only a real choice propagates; the preference default must not be
            // mistaken for one (that is `DefaultShapeColor`'s job).
            let Some(explicit) = other.view.as_ref().and_then(|state| state.shape_color) else {
                return None;
            };
            if common.is_some_and(|color| color != explicit) {
                return None;
            }
            common = Some(explicit);
            any = true;
        }
    }
    if any {
        common
    } else {
        None
    }
}

#[pymethods]
impl PyViewObject {
    /// The view provider proxy, or `None` when none was supplied.
    #[getter]
    fn Proxy(&self, py: Python<'_>) -> PyObject {
        match self.proxy.lock().unwrap().as_ref() {
            Some(proxy) => proxy.clone_ref(py).into_any(),
            None => py.None(),
        }
    }

    #[setter]
    fn set_Proxy(&self, value: &Bound<'_, PyAny>) {
        *self.proxy.lock().unwrap() = if value.is_none() {
            None
        } else {
            Some(value.clone().unbind())
        };
    }

    /// The document object this view object belongs to.
    #[getter]
    fn Object(&self, py: Python<'_>) -> Py<PyDocumentObject> {
        get_or_create_object(py, &self.doc, &self.inner, self.id)
    }

    #[getter]
    fn Visibility(&self, py: Python<'_>) -> PyResult<bool> {
        self.ensure_state(py)?;
        self.with_state(|state| state.visibility)
    }

    #[setter]
    fn set_Visibility(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.ensure_state(py)?;
        self.update(|state| state.visibility = value);
        Ok(())
    }

    #[getter]
    fn Transparency(&self, py: Python<'_>) -> PyResult<i32> {
        self.ensure_state(py)?;
        self.with_state(|state| state.transparency)
    }

    #[setter]
    fn set_Transparency(&self, py: Python<'_>, value: i64) -> PyResult<()> {
        self.ensure_state(py)?;
        self.update(|state| state.transparency = value.clamp(0, 100) as i32);
        Ok(())
    }

    #[getter]
    fn ShapeColor(&self, py: Python<'_>) -> PyResult<(f64, f64, f64, f64)> {
        self.ensure_state(py)?;
        let color = self.effective_color()?;
        Ok((color[0], color[1], color[2], color[3]))
    }

    #[setter]
    fn set_ShapeColor(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let color = py_to_material_color(value)?;
        self.ensure_state(py)?;
        self.update(|state| state.set_shape_color(color));
        Ok(())
    }

    /// `ShapeAppearance`: the shape's material list. Each handle writes back, so
    /// `ShapeAppearance[0].DiffuseColor = ...` changes the shape colour without
    /// touching the transparency.
    #[getter]
    fn ShapeAppearance(&self, py: Python<'_>) -> PyResult<Vec<Py<PyMaterial>>> {
        self.ensure_state(py)?;
        let colors = self.with_state(|state| state.appearance())?;
        colors
            .into_iter()
            .enumerate()
            .map(|(index, color)| {
                Py::new(
                    py,
                    PyMaterial::owned(
                        color,
                        Arc::clone(&self.inner),
                        self.id,
                        MaterialSlot::Appearance(index),
                    ),
                )
            })
            .collect()
    }

    #[setter]
    fn set_ShapeAppearance(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let colors = py_to_view_colors(value)?;
        self.ensure_state(py)?;
        self.update(|state| state.set_appearance(colors));
        Ok(())
    }

    #[getter]
    fn ShapeMaterial(&self, py: Python<'_>) -> PyResult<Py<PyMaterial>> {
        self.ensure_state(py)?;
        let color = self.with_state(|state| state.material())?;
        Py::new(
            py,
            PyMaterial::owned(
                color,
                Arc::clone(&self.inner),
                self.id,
                MaterialSlot::Material,
            ),
        )
    }

    #[setter]
    fn set_ShapeMaterial(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let color = py_to_material_color(value)?;
        self.ensure_state(py)?;
        self.update(|state| state.material = Some(color));
        Ok(())
    }

    /// `DiffuseColor`: one RGBA colour per face, or the shape's whole colour when no
    /// per-face colour was set.
    #[getter]
    fn DiffuseColor(&self, py: Python<'_>) -> PyResult<Vec<(f64, f64, f64, f64)>> {
        Ok(self
            .diffuse_colors(py)?
            .into_iter()
            .map(|color| (color[0], color[1], color[2], color[3]))
            .collect())
    }

    #[setter]
    fn set_DiffuseColor(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let colors = py_to_view_colors(value)?;
        self.ensure_state(py)?;
        self.update(|state| state.diffuse_color = Some(colors));
        Ok(())
    }

    #[getter]
    fn DisplayMode(&self, py: Python<'_>) -> PyResult<String> {
        self.ensure_state(py)?;
        self.with_state(|state| state.display_mode.clone())
    }

    #[setter]
    fn set_DisplayMode(&self, py: Python<'_>, value: String) -> PyResult<()> {
        if !DISPLAY_MODES.contains(&value.as_str()) {
            return Err(PyValueError::new_err(format!(
                "'{value}' is not part of the enumeration"
            )));
        }
        self.ensure_state(py)?;
        self.update(|state| state.display_mode = value);
        Ok(())
    }

    fn getEnumerationsOfProperty(&self, name: &str) -> PyResult<Vec<String>> {
        if name == "DisplayMode" {
            Ok(DISPLAY_MODES.iter().map(|mode| mode.to_string()).collect())
        } else {
            Err(PyAttributeError::new_err(format!("no property '{name}'")))
        }
    }

    /// The Coin root of this view provider.
    ///
    /// This is a real `pivy.coin` scene graph when the bindings are importable, and
    /// it mirrors the view state: one material (per-face colours, bound `PER_PART`)
    /// for each of the two display representations. There is deliberately no
    /// geometry — that belongs to the viewport, not to `App`. Without `pivy` there
    /// is no attribute at all, as in an upstream App-only session.
    #[getter]
    fn RootNode(&self, py: Python<'_>) -> PyResult<PyObject> {
        let colors = self.diffuse_colors(py)?;
        let coin = py.import("pivy.coin").map_err(|_| {
            PyAttributeError::new_err(
                "RootNode needs the pivy Coin bindings; the view state is still \
                 readable as ViewObject properties",
            )
        })?;
        let root = coin.getattr("SoSeparator")?.call0()?;
        // The two display representations a Part view provider has ("Flat Lines"
        // and "Shaded"). Both carry the same material model.
        for _ in DISPLAY_MODES.iter().take(2) {
            let separator = coin.getattr("SoSeparator")?.call0()?;
            let material = coin.getattr("SoMaterial")?.call0()?;
            let diffuse = material.getattr("diffuseColor")?;
            diffuse.call_method1("setNum", (colors.len(),))?;
            for (index, color) in colors.iter().enumerate() {
                diffuse.call_method1("set1Value", (index, (color[0], color[1], color[2])))?;
            }
            let binding = coin.getattr("SoMaterialBinding")?.call0()?;
            let per_part = binding.getattr("PER_PART")?;
            binding.setattr("value", per_part)?;
            separator.call_method1("addChild", (&material,))?;
            separator.call_method1("addChild", (&binding,))?;
            root.call_method1("addChild", (&separator,))?;
        }
        Ok(root.unbind())
    }

    fn __repr__(&self) -> PyResult<String> {
        let doc = self.inner.lock().unwrap();
        let name = doc
            .object(self.id)
            .map(|object| object.name.as_str())
            .unwrap_or("<deleted>");
        Ok(format!("<ViewObject of {name}>"))
    }
}

impl PyViewObject {
    /// Create the view state on first access, from the user preferences.
    fn ensure_state(&self, py: Python<'_>) -> PyResult<()> {
        let type_id = {
            let doc = self.inner.lock().unwrap();
            match doc.object(self.id) {
                Some(object) if object.view.is_some() => return Ok(()),
                Some(object) => object.type_id.clone(),
                None => return Err(deleted_object_error("ViewObject")),
            }
        };
        let (mut transparency, color) = view_preferences(py);
        if is_origin_family(&type_id) {
            transparency = 0;
        }
        let mut doc = self.inner.lock().unwrap();
        if let Some(object) = doc.object_mut(self.id) {
            if object.view.is_none() {
                object.view = Some(ferrocad_core::ViewState::new(transparency, color));
            }
        }
        Ok(())
    }

    fn with_state<R>(
        &self,
        read: impl FnOnce(&ferrocad_core::ViewState) -> R,
    ) -> PyResult<R> {
        let doc = self.inner.lock().unwrap();
        let state = doc
            .object(self.id)
            .and_then(|object| object.view.as_ref())
            .ok_or_else(|| deleted_object_error("ViewObject"))?;
        Ok(read(state))
    }

    fn update(&self, change: impl FnOnce(&mut ferrocad_core::ViewState)) {
        update_view_state(&self.inner, self.id, change);
    }

    fn effective_color(&self) -> PyResult<[f64; 4]> {
        let doc = self.inner.lock().unwrap();
        view_effective_color(&doc, self.id).ok_or_else(|| deleted_object_error("ViewObject"))
    }

    /// The number of faces of the object's shape, asked of the `Part` layer itself
    /// (the core does not know what a shape is). `None` when the object has none.
    fn face_count(&self, py: Python<'_>) -> Option<usize> {
        if !object_is_attached(&self.inner, self.id) {
            return None;
        }
        let doc = self.doc.clone_ref(py);
        let object = get_or_create_object(py, &doc, &self.inner, self.id);
        let object = object.bind(py);
        let shape = object.getattr("Shape").ok()?;
        let faces = shape.getattr("Faces").ok()?;
        faces.len().ok()
    }

    /// Per-face colours: an explicit `DiffuseColor`, else the effective colour
    /// repeated once per face.
    fn diffuse_colors(&self, py: Python<'_>) -> PyResult<Vec<[f64; 4]>> {
        self.ensure_state(py)?;
        let (explicit, color) = {
            let doc = self.inner.lock().unwrap();
            let state = doc
                .object(self.id)
                .and_then(|object| object.view.as_ref())
                .ok_or_else(|| deleted_object_error("ViewObject"))?;
            (
                state.diffuse_color.clone(),
                view_effective_color(&doc, self.id).unwrap_or(state.default_color),
            )
        };
        if let Some(colors) = explicit {
            return Ok(colors);
        }
        match self.face_count(py) {
            Some(count) => Ok(vec![color; count]),
            None => Ok(Vec::new()),
        }
    }
}

#[pyclass(name = "BoundBox", module = "ferrocad")]
#[derive(Clone)]
pub struct PyBoundBox {
    min: [f64; 3],
    max: [f64; 3],
    valid: bool,
}

impl PyBoundBox {
    /// A valid box spanning `min`..`max`.
    ///
    /// Public so sibling Python modules (`Mesh`, …) can return an
    /// `App.BoundBox` without duplicating the class; the caller owns the
    /// emptiness/validity decision.
    pub fn fresh(min: Vector3, max: Vector3) -> Self {
        Self {
            min: [min.x, min.y, min.z],
            max: [max.x, max.y, max.z],
            valid: true,
        }
    }

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
        xmin: Option<&Bound<'_, PyAny>>,
        ymin: Option<f64>,
        zmin: Option<f64>,
        xmax: Option<f64>,
        ymax: Option<f64>,
        zmax: Option<f64>,
    ) -> PyResult<Self> {
        // Upstream also accepts another BoundBox: `App.BoundBox(other)`.
        if let Some(value) = xmin {
            if let Ok(other) = value.downcast::<PyBoundBox>() {
                let other = other.borrow();
                return Ok(Self {
                    min: other.min,
                    max: other.max,
                    valid: other.valid,
                });
            }
        }
        let xmin = xmin.map(|value| value.extract::<f64>()).transpose()?;
        Ok(match (xmin, ymin, zmin, xmax, ymax, zmax) {
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
        })
    }

    /// Grow the box by `amount` in every direction (`BoundBox.enlarge`).
    fn enlarge(&mut self, amount: f64) {
        if self.valid {
            for i in 0..3 {
                self.min[i] -= amount;
                self.max[i] += amount;
            }
        }
    }

    #[getter]
    fn XMin(&self) -> f64 {
        self.min[0]
    }
    #[getter]
    fn YMin(&self) -> f64 {
        self.min[1]
    }
    #[getter]
    fn ZMin(&self) -> f64 {
        self.min[2]
    }
    #[getter]
    fn XMax(&self) -> f64 {
        self.max[0]
    }
    #[getter]
    fn YMax(&self) -> f64 {
        self.max[1]
    }
    #[getter]
    fn ZMax(&self) -> f64 {
        self.max[2]
    }

    fn setVoid(&mut self) {
        self.valid = false;
    }

    fn isValid(&self) -> bool {
        self.valid
    }

    #[pyo3(signature = (x, y = None, z = None))]
    fn add(&mut self, x: &Bound<'_, PyAny>, y: Option<f64>, z: Option<f64>) -> PyResult<()> {
        // `BoundBox.add(otherBox)` grows this box by another box's extent.
        if let Ok(other) = x.extract::<PyRef<'_, PyBoundBox>>() {
            if other.valid {
                self.add_point(other.min);
                self.add_point(other.max);
            }
            return Ok(());
        }
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

    /// The length of the space diagonal (`sqrt(dx² + dy² + dz²)`); `0` for an
    /// empty/invalid box, matching upstream's usable extent.
    #[getter]
    fn DiagonalLength(&self) -> f64 {
        if !self.valid {
            return 0.0;
        }
        let dx = self.max[0] - self.min[0];
        let dy = self.max[1] - self.min[1];
        let dz = self.max[2] - self.min[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    #[getter]
    fn Center(&self) -> PyVector {
        PyVector::fresh(Vector3::new(
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.max[2]) / 2.0,
        ))
    }

    fn isInside(&self, value: &Bound<'_, PyAny>) -> bool {
        if let Ok(point) = value.extract::<PyRef<'_, PyVector>>() {
            return self.contains([point.inner.x, point.inner.y, point.inner.z]);
        }
        if let Ok(other) = value.downcast::<PyBoundBox>() {
            let other = other.borrow();
            return self.valid
                && other.valid
                && (0..3).all(|i| other.min[i] >= self.min[i] && other.max[i] <= self.max[i]);
        }
        false
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
    // Existence, unique naming and the active pointer live in `ferrocad_core`;
    // this layer keeps the Python wrapper (identity + label/meta/comment).
    let (doc_name, handle) = application()
        .lock()
        .unwrap()
        .new_document(name.as_deref());
    let doc = Py::new(
        py,
        PyDocument {
            label: doc_name.clone(),
            name: doc_name.clone(),
            file_name: None,
            format: None,
            auto_created: false,
            inner: handle,
            meta: Arc::new(Mutex::new(BTreeMap::new())),
            comment: Mutex::new(String::new()),
        },
    )?;
    documents().lock().unwrap().insert(doc_name, doc.clone_ref(py));
    let bound = doc.bind(py);
    fire_doc("slotCreatedDocument", bound, None);
    fire_doc_str("slotBeforeChangeDocument", bound, "Label");
    fire_doc_str("slotChangedDocument", bound, "Label");
    fire_doc("slotRelabelDocument", bound, None);
    Ok(doc)
}

/// Open a document from disk.
///
/// The suffix selects the serializer, exactly as for [`PyDocument::saveAs`]:
/// `.ferrocad` is native JSON, `.FCStd` is the ZIP container (with the
/// transitional marker-gated JSON fallback while no FCStd writer exists). An
/// unknown or missing suffix is a `ValueError`; the content is never sniffed to
/// override the suffix.
///
/// `hidden` and `readonly` are accepted for compatibility with upstream's
/// `FreeCAD.openDocument(path, hidden=False, readonly=False)`. They are ignored:
/// this backend always loads the document fully into memory, and the opened
/// document becomes active even when `hidden` is true (matching upstream).
#[pyfunction]
#[pyo3(signature = (path, hidden=false, readonly=false))]
fn openDocument(
    py: Python<'_>,
    path: &str,
    hidden: bool,
    readonly: bool,
) -> PyResult<Py<PyDocument>> {
    let _ = (hidden, readonly);
    let (saved, format) =
        doc_format::read(path).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let core_doc = CoreDocument::from_saved(&saved);
    let (doc_name, handle) = application()
        .lock()
        .unwrap()
        .insert_document(&saved.name, core_doc);
    let doc = Py::new(
        py,
        PyDocument {
            name: doc_name.clone(),
            label: doc_name.clone(),
            file_name: Some(path.to_string()),
            format: Some(format),
            auto_created: false,
            inner: Arc::clone(&handle),
            meta: Arc::new(Mutex::new(BTreeMap::new())),
            comment: Mutex::new(String::new()),
        },
    )?;
    documents().lock().unwrap().insert(doc_name, doc.clone_ref(py));
    apply_all_python_states(py, &doc, &handle);
    notify_document_restored(py, &doc, &handle);
    Ok(doc)
}

/// The registered Python wrapper for an open document, if any.
#[pyfunction]
fn getDocument(py: Python<'_>, name: &str) -> Option<Py<PyDocument>> {
    documents().lock().unwrap().get(name).map(|doc| doc.clone_ref(py))
}

/// `name -> wrapper` for every open document.
#[pyfunction]
fn listDocuments(py: Python<'_>) -> BTreeMap<String, Py<PyDocument>> {
    documents()
        .lock()
        .unwrap()
        .iter()
        .map(|(name, doc)| (name.clone(), doc.clone_ref(py)))
        .collect()
}

/// Close a document; emits the deletion signal. Returns whether it was open.
#[pyfunction]
fn closeDocument(py: Python<'_>, name: &str) -> bool {
    let doc = documents().lock().unwrap().remove(name);
    application().lock().unwrap().close_document(name);
    match doc {
        Some(doc) => {
            fire_doc("slotDeletedDocument", doc.bind(py), None);
            forget_document(&doc);
            true
        }
        None => false,
    }
}

/// Make a document active; emits the activation signal when it changes.
#[pyfunction]
fn setActiveDocument(py: Python<'_>, name: &str) -> bool {
    let changed = {
        let mut app = application().lock().unwrap();
        if !app.contains(name) {
            return false;
        }
        let changed = app.active() != Some(name);
        app.set_active(name);
        changed
    };
    if changed {
        let doc = documents().lock().unwrap().get(name).map(|d| d.clone_ref(py));
        if let Some(doc) = doc {
            fire_doc("slotActivateDocument", doc.bind(py), None);
        }
    }
    true
}

/// The active document's wrapper, if any.
#[pyfunction]
fn activeDocument(py: Python<'_>) -> Option<Py<PyDocument>> {
    let name = application().lock().unwrap().active().map(str::to_string)?;
    documents().lock().unwrap().get(&name).map(|doc| doc.clone_ref(py))
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

#[pymodule]
pub fn ferrocad(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Python-feature `execute` requests (Part::FeaturePython and friends) are
    // drained by this binding, so core may now let their types rebuild.
    ferrocad_core::object_registry::set_python_executor_available(true);
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
    m.add_class::<PyViewObject>()?;
    m.add_class::<PyDocumentSettings>()?;
    m.add_class::<PyStringHasher>()?;
    m.add_class::<PyStringID>()?;
    m.add_function(wrap_pyfunction!(newDocument, m)?)?;
    m.add_function(wrap_pyfunction!(openDocument, m)?)?;
    m.add_function(wrap_pyfunction!(getDocument, m)?)?;
    m.add_function(wrap_pyfunction!(listDocuments, m)?)?;
    m.add_function(wrap_pyfunction!(closeDocument, m)?)?;
    m.add_function(wrap_pyfunction!(setActiveDocument, m)?)?;
    m.add_function(wrap_pyfunction!(activeDocument, m)?)?;
    m.add_function(wrap_pyfunction!(addDocumentObserver, m)?)?;
    m.add_function(wrap_pyfunction!(removeDocumentObserver, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyTuple;
    use std::sync::Once;

    /// The extension is built without `pyo3/auto-initialize` (it is a library
    /// module), so tests must start the interpreter themselves.
    fn init_python() {
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn quantity_args(py: Python<'_>, second: Py<PyAny>) -> PyResult<Bound<'_, PyTuple>> {
        let value: Py<PyAny> = 100.0f64.into_py_any(py)?;
        PyTuple::new(py, [value, second])
    }

    #[test]
    fn quantity_accepts_a_unit_in_second_position() {
        init_python();
        Python::with_gil(|py| {
            let unit: Py<PyAny> = PyUnit { inner: Unit::parse("mm").unwrap() }
                .into_py_any(py)
                .unwrap();
            let args = quantity_args(py, unit).unwrap();
            let q = PyQuantity::new(&args).unwrap();
            assert_eq!(q.inner, Quantity::new(100.0, Unit::parse("mm").unwrap()));
        });
    }

    #[test]
    fn quantity_still_accepts_a_unit_string_in_second_position() {
        init_python();
        Python::with_gil(|py| {
            let symbol: Py<PyAny> = "mm".into_py_any(py).unwrap();
            let args = quantity_args(py, symbol).unwrap();
            let q = PyQuantity::new(&args).unwrap();
            assert_eq!(q.inner, Quantity::new(100.0, Unit::parse("mm").unwrap()));
        });
    }

    #[test]
    fn quantity_rejects_a_quantity_as_the_unit() {
        init_python();
        Python::with_gil(|py| {
            let other: Py<PyAny> = PyQuantity {
                inner: Quantity::new(1.0, Unit::parse("mm").unwrap()),
            }
            .into_py_any(py)
            .unwrap();
            let args = quantity_args(py, other).unwrap();
            match PyQuantity::new(&args) {
                Ok(_) => panic!("a Quantity must not be accepted as the unit"),
                Err(err) => assert!(
                    err.is_instance_of::<PyTypeError>(py),
                    "expected TypeError, got {err:?}"
                ),
            }
        });
    }
}
