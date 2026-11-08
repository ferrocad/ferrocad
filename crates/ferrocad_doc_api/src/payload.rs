//! The document as a JSON payload: what a script runtime sees.
//!
//! One presenter, two consumers. `ferrocad_shell_pywasm` hands these rows to the
//! Python facade, and [`crate::DocApi`] answers the addon verbs with them. The kind
//! and the display text come from `ferrocad_core::present`, exactly as the property
//! pane decides them, so a JS addon, a Python script and the pane cannot drift.
//!
//! The shape is the one `python/ferrocad_shell` already reads:
//!
//! ```json
//! { "name": "Sample",
//!   "objects": [ { "name": "Box", "label": "Box", "type_id": "Part::Box",
//!                  "entity": 1,
//!                  "additional_type_ids": ["Part", "Box"],
//!                  "properties": [ { "name": "Length", "type_id": "App::PropertyLength",
//!                                    "kind": "quantity", "value": "10 mm",
//!                                    "status": [], "choices": [] } ],
//!                  "driven": [] } ] }
//! ```
//!
//! `entity` is the pick identity the stage uses for the object (`id + 1`, the same
//! convention `RustAppBackend` assigns), so a display hook that answers for an object
//! can echo it and a pick still resolves to the right name.
//!
//! `status` is the names of the property's flag mask, the way FreeCAD's
//! `getTypeOfProperty` reports it. A facade has to answer that call, and without the
//! names in the payload it would have to invent something (the wasm Python half used
//! to answer with the type's last segment).

use std::sync::MutexGuard;

use ferrocad_core::{Document, DocumentHandle, DocumentObject, Property, present, status_names};
use serde_json::{Value, json};

/// The active document and its shared handle, if one is open.
///
/// The registry lock is released before the handle is locked, the same order
/// `RustAppBackend` keeps: a callback that walks from the document back to the
/// registry must not deadlock against the other order.
pub fn active() -> Option<(String, DocumentHandle)> {
    let registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let name = registry.active()?.to_string();
    let handle = registry.get(&name)?;
    drop(registry);
    Some((name, handle))
}

/// Lock a document handle, ignoring a previous owner's panic.
pub fn lock(handle: &DocumentHandle) -> MutexGuard<'_, Document> {
    handle.lock().unwrap_or_else(|error| error.into_inner())
}

/// The active document as a payload value, or `None` when nothing is open.
pub fn active_value() -> Option<Value> {
    let (name, handle) = active()?;
    let document = lock(&handle);
    Some(value(&name, &document))
}

/// [`active_value`] as text (`null` when nothing is open).
pub fn active_json() -> String {
    match active_value() {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

/// A document under the name it is known by.
pub fn value(name: &str, document: &Document) -> Value {
    let objects: Vec<Value> = document
        .topological_sorted_objects()
        .into_iter()
        .filter_map(|id| document.object(id))
        .map(object_value)
        .collect();
    json!({ "name": name, "objects": objects })
}

/// [`value`] as text.
pub fn render(name: &str, document: &Document) -> String {
    value(name, document).to_string()
}

/// One object's rows.
pub fn object_value(object: &DocumentObject) -> Value {
    let properties: Vec<Value> = object
        .properties
        .iter()
        .map(|(name, value)| row(name, value, object.properties.status(name)))
        .collect();
    json!({
        "name": object.name,
        "label": object.label,
        "type_id": object.type_id,
        // The stage's pick identity (`RustAppBackend` assigns `id + 1`): a display
        // hook echoes it so a pick of its mesh resolves to this object's name.
        "entity": object.id as u32 + 1,
        "additional_type_ids": object.type_id.split("::").map(str::to_string).collect::<Vec<_>>(),
        "properties": properties,
        "driven": object.expressions.keys().cloned().collect::<Vec<_>>(),
    })
}

/// One property row: the trait asks for a value, a type and a kind; `present` owns
/// the last two. `status` is the flag mask's names (`getTypeOfProperty`).
fn row(name: &str, value: &Property, status: Option<u32>) -> Value {
    // The shell's `PropRow.kind` is `""` for a property with no editor, and the
    // script's `properties()` reads a non-empty kind as "editable".
    let kind = present::kind(value).map(|kind| kind.as_str()).unwrap_or("");
    json!({
        "name": name,
        "type_id": value.type_name(),
        "kind": kind,
        "value": present::format(value),
        "status": status.map(status_names).unwrap_or_default(),
        "choices": choices(value),
    })
}

/// An enumeration's options; `[]` for anything else.
fn choices(value: &Property) -> Vec<String> {
    match value {
        Property::Enumeration(values, _) => values.clone(),
        _ => Vec::new(),
    }
}
