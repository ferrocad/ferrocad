//! The document, as JSON calls: the one document API a script runtime shares.
//!
//! `ferrocad_core` owns the document and `ferrocad_part` gives its shapes geometry.
//! Neither can be linked into a script runtime: a second copy is a second document
//! (and, for Part, a second kernel). So this crate is the **wrapper**: it implements
//! [`DocumentApi`] over the process-wide registry — the same document
//! `RustAppBackend` answers the model tree from — and every runtime that is not Rust
//! reaches the document through it:
//!
//! ```text
//! JS addon ──▶ window.ferrocad.call(request) ──▶ DocApi ──▶ registry ──▶ core
//! wasm addon ─▶ import "ferrocad" "call"       ──▶  (same)             └▶ part (registered by the host)
//! ```
//!
//! **Part arrives by registration, not by linkage.** [`DocApi`] depends on
//! `ferrocad_core` only; a `Part::Box` created through it recomputes because the host
//! called `ferrocad_part::register` (the browser backend and every edition do). A
//! build with no Part registered still creates and edits properties — there is just no
//! shape behind them, which is what a kernel-free host wants.
//!
//! # The protocol
//!
//! Every request is one JSON object with a `call` name and its arguments. Every
//! response is `{"ok": true, "version": 1, "result": …}` or
//! `{"ok": false, "version": 1, "error": "…"}` — a failure is a *response*, so a
//! script reports it and keeps running.
//!
//! | `call` | arguments | `result` |
//! | --- | --- | --- |
//! | `document` | — | the payload in [`payload`], or `null` when nothing is open |
//! | `properties` | `object` | that object's property rows |
//! | `create` | `type`, optional `name`, optional `properties` | `{name, properties, recomputed}` |
//! | `set` | `object`, `property`, `value` (editor text) | `{object, property, value, [recompute]}` |
//! | `recompute` | — | `{recomputed: [names]}` |
//!
//! A `create` property value is **either** editor text (set a property the type already
//! has) **or** a declaration, so a workbench can define its own object the way
//! `obj.addProperty(...)` does in Python:
//!
//! ```json
//! { "call": "create", "name": "Toy", "type": "App::FeaturePython",
//!   "properties": {
//!     "Height": { "type": "App::PropertyLength", "value": "20 mm", "group": "Base" },
//!     "Label": "Toy"
//!   } }
//! ```
//!
//! Writes are one named transaction each and recompute before returning, so a
//! command's outcome is visible to the next call (and the shell's `refresh` +
//! `sync_stage` draws it). Values are **editor text**, coerced by
//! `ferrocad_core::present` — the same rules the property pane uses, so `"25"` on a
//! length keeps its unit and `"fast"` is refused if `Fast` is the enumeration entry.
//!
//! # What this is not
//!
//! No selection (the shell owns it), no transactions the script can hold open, no
//! undo control, and no per-frame calls: this is human-speed work, like a workbench
//! command. A static-link addon (`ferrocad_toy` compiled into the host) does not need
//! this crate at all — it calls `ferrocad_core` directly and is the same process.

use ferrocad_core::{Document, ObjectId, present};
use ferrocad_shell_api::{DocumentApi, DocumentEvent, DocumentEventSink};
use serde_json::{Value, json};

pub mod payload;

/// The protocol version a loader may check.
pub const VERSION: u32 = 1;

/// The JSON API over the process-wide document registry.
pub struct DocApi {
    /// Where committed edits are announced, when the host installed a sink.
    events: Option<std::sync::Arc<dyn DocumentEventSink>>,
}

impl DocApi {
    /// A document API that announces nothing: reads work, writes are not observed.
    pub fn new() -> Self {
        Self { events: None }
    }

    /// A document API that announces every committed edit and successful recompute.
    ///
    /// The browser host gives it the page's sink, so an addon's own `create`/`set`
    /// reaches `onDocumentEvent` exactly like an edit the shell performed.
    pub fn with_events(events: std::sync::Arc<dyn DocumentEventSink>) -> Self {
        Self {
            events: Some(events),
        }
    }
}

impl Default for DocApi {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentApi for DocApi {
    fn call(&self, request: &str) -> String {
        let (response, events) = match dispatch(request) {
            Ok((result, events)) => (
                json!({ "ok": true, "version": VERSION, "result": result }),
                events,
            ),
            Err(error) => (
                json!({ "ok": false, "version": VERSION, "error": error }),
                Vec::new(),
            ),
        };
        // Announced after `dispatch` released the document lock: a sink is the host's
        // callback and may reach back into the document.
        if let Some(sink) = &self.events {
            for event in &events {
                sink.document_event(event);
            }
        }
        response.to_string()
    }
}

fn dispatch(request: &str) -> Result<(Value, Vec<DocumentEvent>), String> {
    let request: Value =
        serde_json::from_str(request).map_err(|error| format!("request is not JSON: {error}"))?;
    let call = request
        .get("call")
        .and_then(Value::as_str)
        .ok_or_else(|| "request has no `call` name".to_string())?;
    match call {
        "document" => Ok((payload::active_value().unwrap_or(Value::Null), Vec::new())),
        "properties" => Ok((properties(&text(&request, "object")?)?, Vec::new())),
        "create" => create(&request),
        "set" => set(&request),
        "recompute" => recompute(),
        other => Err(format!(
            "unknown call {other:?}; expected document, properties, create, set or recompute"
        )),
    }
}

/// One object's property rows, in [`payload`]'s shape.
fn properties(name: &str) -> Result<Value, String> {
    let handle = active_handle()?;
    let document = payload::lock(&handle);
    let id = document
        .get_by_name(name)
        .ok_or_else(|| format!("no object {name:?}"))?;
    let object = document
        .object(id)
        .ok_or_else(|| format!("no object {name:?}"))?;
    Ok(payload::object_value(object)["properties"].clone())
}

fn create(request: &Value) -> Result<(Value, Vec<DocumentEvent>), String> {
    let type_id = text(request, "type")?;
    // An empty name asks `add_object` for FreeCAD's default (`Box`, `Box001`, …).
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let wanted = wanted_properties(request)?;

    let handle = active_handle()?;
    let mut document = payload::lock(&handle);
    let label = if name.is_empty() {
        type_id.clone()
    } else {
        name.clone()
    };
    let transaction = format!("Create {label}");
    document.open_transaction_named(&transaction);
    match build(&mut document, &name, &type_id, &wanted) {
        Ok((name, applied)) => {
            document.commit_transaction();
            let executed = document.recompute().map_err(|error| {
                format!("{name} was created, but the recompute failed: {error}")
            })?;
            let executed = names(&document, &executed);
            let events = vec![
                DocumentEvent::edited(&transaction, std::slice::from_ref(&name)),
                DocumentEvent::recomputed(&executed),
            ];
            Ok((
                json!({
                    "name": name,
                    "properties": applied,
                    "recomputed": executed.len(),
                }),
                events,
            ))
        }
        Err(error) => {
            document.abort_transaction();
            Err(error)
        }
    }
}

/// Add the object and apply its properties, inside the caller's transaction.
fn build(
    document: &mut Document,
    name: &str,
    type_id: &str,
    wanted: &[(String, Wanted)],
) -> Result<(String, Vec<String>), String> {
    let id = document.add_object(name, type_id);
    // A fresh object is not scheduled by itself (`add_object` leaves `must_execute`
    // false), and a create with no properties would otherwise draw nothing.
    document.touch(id, false);
    let mut applied = Vec::new();
    for (property, wanted) in wanted {
        match wanted {
            Wanted::Set(text) => {
                let current = document
                    .object(id)
                    .and_then(|object| object.properties.get(property))
                    .cloned()
                    .ok_or_else(|| format!("{type_id} has no property {property:?}"))?;
                document.set_property(id, property, present::coerce(&current, text)?)?;
            }
            Wanted::Declare {
                type_id: property_type,
                value,
                group,
                doc,
            } => {
                let default = ferrocad_core::property_types::default_for(property_type)
                    .ok_or_else(|| format!("unknown property type {property_type:?}"))?;
                document.add_property(
                    id,
                    property,
                    default.clone(),
                    ferrocad_core::prop_status::NONE,
                    group,
                    doc,
                )?;
                if let Some(text) = value {
                    document.set_property(id, property, present::coerce(&default, text)?)?;
                }
            }
        }
        applied.push(property.clone());
    }
    let name = document
        .object(id)
        .map(|object| object.name.clone())
        .unwrap_or_default();
    Ok((name, applied))
}

fn set(request: &Value) -> Result<(Value, Vec<DocumentEvent>), String> {
    let object_name = text(request, "object")?;
    let property = text(request, "property")?;
    let typed = text(request, "value")?;

    let handle = active_handle()?;
    let mut document = payload::lock(&handle);
    let id = document
        .get_by_name(&object_name)
        .ok_or_else(|| format!("no object {object_name:?}"))?;
    let current = document
        .object(id)
        .and_then(|object| object.properties.get(&property))
        .cloned()
        .ok_or_else(|| format!("{object_name} has no property {property:?}"))?;
    let value = present::coerce(&current, &typed)?;
    let shown = present::format(&value);

    let transaction = format!("Set {object_name}.{property}");
    document.open_transaction_named(&transaction);
    if let Err(error) = document.set_property(id, &property, value) {
        document.abort_transaction();
        return Err(error);
    }
    document.commit_transaction();
    let mut result = json!({ "object": object_name, "property": property, "value": shown });
    let mut events = vec![DocumentEvent::edited(
        &transaction,
        std::slice::from_ref(&object_name),
    )];
    // The edit is committed; a failing recompute is reported beside it rather than
    // rolled back, because the value is what the user asked for. Only a successful
    // recompute is announced.
    match document.recompute() {
        Ok(executed) => events.push(DocumentEvent::recomputed(&names(&document, &executed))),
        Err(error) => result["recompute"] = json!(format!("failed: {error}")),
    }
    Ok((result, events))
}

fn recompute() -> Result<(Value, Vec<DocumentEvent>), String> {
    let handle = active_handle()?;
    let mut document = payload::lock(&handle);
    let executed = document.recompute()?;
    let names = names(&document, &executed);
    Ok((
        json!({ "recomputed": names }),
        vec![DocumentEvent::recomputed(&names)],
    ))
}

/// The names of `ids`, in order, skipping any object the recompute removed.
fn names(document: &Document, ids: &[ObjectId]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| document.object(*id))
        .map(|object| object.name.clone())
        .collect()
}

/// The active document's handle, or the reason a script has nothing to talk to.
fn active_handle() -> Result<ferrocad_core::DocumentHandle, String> {
    payload::active()
        .map(|(_, handle)| handle)
        .ok_or_else(|| "no active document".to_string())
}

fn text(request: &Value, key: &str) -> Result<String, String> {
    request
        .get(key)
        .map(text_of)
        .ok_or_else(|| format!("request has no `{key}`"))
}

/// A JSON scalar as the text an editor would have typed.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// One requested property on `create`.
enum Wanted {
    /// Editor text for a property the object's type already has.
    Set(String),
    /// A property the workbench declares, with its type and optional initial value.
    Declare {
        type_id: String,
        value: Option<String>,
        group: String,
        doc: String,
    },
}

/// The `properties` argument, as `(name, request)` pairs.
fn wanted_properties(request: &Value) -> Result<Vec<(String, Wanted)>, String> {
    let Some(value) = request.get("properties") else {
        return Ok(Vec::new());
    };
    let Some(map) = value.as_object() else {
        if value.is_null() {
            return Ok(Vec::new());
        }
        return Err("`properties` must be an object of name → value".to_string());
    };
    map.iter()
        .map(|(name, value)| wanted(name, value))
        .collect()
}

fn wanted(name: &str, value: &Value) -> Result<(String, Wanted), String> {
    let Value::Object(fields) = value else {
        return Ok((name.to_string(), Wanted::Set(text_of(value))));
    };
    // An object is a declaration; anything else is the text an editor would type.
    let type_id = fields
        .get("type")
        .map(text_of)
        .ok_or_else(|| format!("property {name:?}: a declaration needs a `type`"))?;
    Ok((
        name.to_string(),
        Wanted::Declare {
            type_id,
            value: fields.get("value").map(text_of),
            group: fields
                .get("group")
                .map(text_of)
                .unwrap_or_else(|| "Base".to_string()),
            doc: fields.get("doc").map(text_of).unwrap_or_default(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use ferrocad_shell_api::document_api::{is_read, is_write};

    /// Keep the seam's capability list in step with the dispatch above.
    ///
    /// A new *mutating* arm that is not in `is_write` would make a read-only document API
    /// writable, which is exactly what the sandbox must not do.
    #[test]
    fn the_document_api_verbs_are_classified() {
        for call in ["create", "set", "recompute"] {
            assert!(is_write(call), "`{call}` mutates the document");
        }
        for call in ["document", "properties"] {
            assert!(is_read(call), "`{call}` only reads");
            assert!(!is_write(call), "`{call}` must not count as a write");
        }
    }
}
