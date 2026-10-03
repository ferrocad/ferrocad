//! # ferrocad_ctypes (legacy M0 C-ABI fallback)
//!
//! A deliberately small re-implementation of the *headless* FreeCAD document
//! object model in Rust, exposed to Python through a flat C ABI.
//!
//! This is the "replace the C++ bindings" half of the milestone: the Python
//! facade in `python/FreeCAD/` speaks the familiar FreeCAD API
//! (`newDocument`, `addObject`, `recompute`, properties, ...), but every call
//! is served by the Rust code below over a `cdylib` boundary.
//!
//! ## Why a C ABI and not PyO3?
//!
//! PyO3 is the production path (see `../docs/`), but it needs the CPython
//! headers to compile. The build sandbox has no `python3-dev`, so this POC
//! uses a dependency-free C ABI loaded with `ctypes` — one of the mechanisms
//! from the feasibility assessment. The seam is intentionally thin so it can
//! later be swapped for `#[pyclass]` bindings without touching the model.
//!
//! ## Ownership model (POC-simple)
//!
//! * Documents live in a global registry as `Box<FcDocument>`, so their heap
//!   addresses are stable for the lifetime of the document.
//! * Objects live inside their document (`Vec<Box<FcObject>>`); same stability
//!   argument.
//! * Every function returning a string returns an owned C string that the
//!   caller must release with `fc_string_free`.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Property {
    type_id: String,
    value: String,
}

pub struct FcObject {
    type_id: String,
    name: String,
    label: String,
    props: BTreeMap<String, Property>,
}

pub struct FcDocument {
    name: String,
    label: String,
    objects: Vec<Box<FcObject>>,
}

struct Registry {
    docs: Vec<Box<FcDocument>>,
    active: Option<usize>,
}

static REG: Mutex<Registry> = Mutex::new(Registry {
    docs: Vec::new(),
    active: None,
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Borrow a NUL-terminated C string as an owned Rust `String`.
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the C ABI guarantees a non-null, NUL-terminated string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Move a Rust `String` out to an owned C string (freed by `fc_string_free`).
fn to_c(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

fn doc_ptr(d: &mut FcDocument) -> *mut FcDocument {
    d as *mut FcDocument
}

fn obj_ptr(o: &mut FcObject) -> *mut FcObject {
    o as *mut FcObject
}

// ---------------------------------------------------------------------------
// Library lifecycle
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn fc_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Frees a string previously returned by this library.
#[unsafe(no_mangle)]
pub extern "C" fn fc_string_free(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) };
    }
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn fc_new_document(name: *const c_char) -> *mut FcDocument {
    let mut doc_name = unsafe { cstr(name) };
    if doc_name.is_empty() {
        doc_name = "Unnamed".to_string();
    }

    let mut reg = REG.lock().unwrap();

    // Make the name unique, mirroring FreeCAD's "Unnamed001" behaviour.
    if reg.docs.iter().any(|d| d.name == doc_name) {
        let base = doc_name.clone();
        let mut i = 1;
        loop {
            let candidate = format!("{}{:03}", base, i);
            if !reg.docs.iter().any(|d| d.name == candidate) {
                doc_name = candidate;
                break;
            }
            i += 1;
        }
    }

    let doc = Box::new(FcDocument {
        label: doc_name.clone(),
        name: doc_name,
        objects: Vec::new(),
    });
    reg.docs.push(doc);
    let index = reg.docs.len() - 1;
    reg.active = Some(index);
    doc_ptr(&mut reg.docs[index])
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_close_document(doc: *mut FcDocument) -> i32 {
    if doc.is_null() {
        return -1;
    }
    let addr = doc as *const FcDocument;
    let mut reg = REG.lock().unwrap();
    let pos = reg
        .docs
        .iter()
        .position(|d| (&**d as *const FcDocument) == addr);
    match pos {
        None => -1,
        Some(i) => {
            reg.docs.remove(i);
            reg.active = match reg.active {
                Some(a) if a == i => None,
                Some(a) if a > i => Some(a - 1),
                other => other,
            };
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_active_document() -> *mut FcDocument {
    let mut reg = REG.lock().unwrap();
    match reg.active {
        Some(i) => doc_ptr(&mut reg.docs[i]),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_get_document(name: *const c_char) -> *mut FcDocument {
    let target = unsafe { cstr(name) };
    let mut reg = REG.lock().unwrap();
    match reg.docs.iter_mut().find(|d| d.name == target) {
        Some(d) => doc_ptr(d),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_count() -> i32 {
    REG.lock().unwrap().docs.len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_at(index: i32) -> *mut FcDocument {
    if index < 0 {
        return ptr::null_mut();
    }
    let mut reg = REG.lock().unwrap();
    match reg.docs.get_mut(index as usize) {
        Some(d) => doc_ptr(d),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_name(doc: *mut FcDocument) -> *mut c_char {
    if doc.is_null() {
        return ptr::null_mut();
    }
    to_c(unsafe { &*doc }.name.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_label(doc: *mut FcDocument) -> *mut c_char {
    if doc.is_null() {
        return ptr::null_mut();
    }
    to_c(unsafe { &*doc }.label.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_set_label(doc: *mut FcDocument, label: *const c_char) -> i32 {
    if doc.is_null() {
        return -1;
    }
    unsafe { &mut *doc }.label = unsafe { cstr(label) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_object_count(doc: *mut FcDocument) -> i32 {
    if doc.is_null() {
        return 0;
    }
    unsafe { &*doc }.objects.len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_object_at(doc: *mut FcDocument, index: i32) -> *mut FcObject {
    if doc.is_null() || index < 0 {
        return ptr::null_mut();
    }
    let doc = unsafe { &mut *doc };
    match doc.objects.get_mut(index as usize) {
        Some(o) => obj_ptr(o),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_get_object(
    doc: *mut FcDocument,
    name: *const c_char,
) -> *mut FcObject {
    if doc.is_null() {
        return ptr::null_mut();
    }
    let target = unsafe { cstr(name) };
    let doc = unsafe { &mut *doc };
    match doc.objects.iter_mut().find(|o| o.name == target) {
        Some(o) => obj_ptr(o),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_add_object(
    doc: *mut FcDocument,
    type_id: *const c_char,
    name: *const c_char,
) -> *mut FcObject {
    if doc.is_null() {
        return ptr::null_mut();
    }
    let doc = unsafe { &mut *doc };
    let type_id = unsafe { cstr(type_id) };
    let mut name = unsafe { cstr(name) };

    if name.is_empty() {
        // FreeCAD-style default: last segment of the type id + a counter.
        let base = match type_id.rsplit("::").next() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "Object".to_string(),
        };
        name = base.clone();
        let mut i = 1;
        while doc.objects.iter().any(|o| o.name == name) {
            name = format!("{}{:03}", base, i);
            i += 1;
        }
    }

    let obj = Box::new(FcObject {
        type_id,
        label: name.clone(),
        name,
        props: BTreeMap::new(),
    });
    doc.objects.push(obj);
    obj_ptr(doc.objects.last_mut().unwrap())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_remove_object(doc: *mut FcDocument, name: *const c_char) -> i32 {
    if doc.is_null() {
        return -1;
    }
    let target = unsafe { cstr(name) };
    let doc = unsafe { &mut *doc };
    let before = doc.objects.len();
    doc.objects.retain(|o| o.name != target);
    if doc.objects.len() == before {
        -1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_document_recompute(doc: *mut FcDocument) -> i32 {
    if doc.is_null() {
        return 0;
    }
    // POC: the real engine would run the dependency graph per object.
    unsafe { &*doc }.objects.len() as i32
}

// ---------------------------------------------------------------------------
// Document objects
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_name(obj: *mut FcObject) -> *mut c_char {
    if obj.is_null() {
        return ptr::null_mut();
    }
    to_c(unsafe { &*obj }.name.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_label(obj: *mut FcObject) -> *mut c_char {
    if obj.is_null() {
        return ptr::null_mut();
    }
    to_c(unsafe { &*obj }.label.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_set_label(obj: *mut FcObject, label: *const c_char) -> i32 {
    if obj.is_null() {
        return -1;
    }
    unsafe { &mut *obj }.label = unsafe { cstr(label) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_type_id(obj: *mut FcObject) -> *mut c_char {
    if obj.is_null() {
        return ptr::null_mut();
    }
    to_c(unsafe { &*obj }.type_id.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_add_property(
    obj: *mut FcObject,
    type_id: *const c_char,
    name: *const c_char,
    default_value: *const c_char,
) -> i32 {
    if obj.is_null() {
        return -1;
    }
    let name = unsafe { cstr(name) };
    if name.is_empty() {
        return -1;
    }
    let obj = unsafe { &mut *obj };
    obj.props.entry(name).or_insert_with(|| Property {
        type_id: unsafe { cstr(type_id) },
        value: unsafe { cstr(default_value) },
    });
    0
}

/// Returns the value of `name`, or NULL if the property does not exist.
#[unsafe(no_mangle)]
pub extern "C" fn fc_object_get_property(obj: *mut FcObject, name: *const c_char) -> *mut c_char {
    if obj.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { cstr(name) };
    match unsafe { &*obj }.props.get(&name) {
        Some(p) => to_c(p.value.clone()),
        None => ptr::null_mut(),
    }
}

/// Sets an existing property. Returns 0 on success, -1 if unknown.
#[unsafe(no_mangle)]
pub extern "C" fn fc_object_set_property(
    obj: *mut FcObject,
    name: *const c_char,
    value: *const c_char,
) -> i32 {
    if obj.is_null() {
        return -1;
    }
    let name = unsafe { cstr(name) };
    let value = unsafe { cstr(value) };
    match unsafe { &mut *obj }.props.get_mut(&name) {
        Some(p) => {
            p.value = value;
            0
        }
        None => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_property_count(obj: *mut FcObject) -> i32 {
    if obj.is_null() {
        return 0;
    }
    unsafe { &*obj }.props.len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn fc_object_property_name_at(obj: *mut FcObject, index: i32) -> *mut c_char {
    if obj.is_null() || index < 0 {
        return ptr::null_mut();
    }
    match unsafe { &*obj }.props.keys().nth(index as usize) {
        Some(k) => to_c(k.clone()),
        None => to_c(String::new()),
    }
}

/// Returns the declared type (e.g. "App::PropertyString") of a property.
#[unsafe(no_mangle)]
pub extern "C" fn fc_object_property_type(obj: *mut FcObject, name: *const c_char) -> *mut c_char {
    if obj.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { cstr(name) };
    match unsafe { &*obj }.props.get(&name) {
        Some(p) => to_c(p.type_id.clone()),
        None => ptr::null_mut(),
    }
}
