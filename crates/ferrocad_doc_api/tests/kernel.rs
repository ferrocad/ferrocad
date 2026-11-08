//! The addon verbs against a document that really recomputes.
//!
//! `ferrocad_part` is a **dev**-dependency: the crate is core-only so the wasm-Python
//! half can share it, and Part arrives at run time through the object registry. Here
//! the test is the host — it registers Part, so a `Part::Box` created over the JSON
//! doorway gets a real shape.

use std::sync::{Arc, Mutex, MutexGuard};

use ferrocad_doc_api::DocApi;
use ferrocad_shell_api::DocumentApi;
use serde_json::{Value, json};

/// The document registry is process-wide; these tests take turns with it.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

fn call(api: &DocApi, request: Value) -> Value {
    let answer = api.call(&request.to_string());
    serde_json::from_str(&answer).expect("a JSON response")
}

/// Register Part, start from an empty registry and open one document.
fn boot_registry() {
    ferrocad_part::register(Arc::new(ferrocad_occt::OcctBackend::new()));
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
    registry.new_document(Some("AddonSpike"));
}

fn boot() -> DocApi {
    boot_registry();
    DocApi::new()
}

fn rows(api: &DocApi, object: &str) -> Vec<Value> {
    let answer = call(api, json!({ "call": "properties", "object": object }));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"]
        .as_array()
        .expect("rows are an array")
        .clone()
}

fn row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("{name} is a row; got {rows:?}"))
}

/// The events a doorway announced, in order.
#[derive(Default)]
struct RecordedEvents(Mutex<Vec<ferrocad_shell_api::DocumentEvent>>);

impl RecordedEvents {
    fn take(&self) -> Vec<ferrocad_shell_api::DocumentEvent> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }
}

impl ferrocad_shell_api::DocumentEventSink for RecordedEvents {
    fn document_event(&self, event: &ferrocad_shell_api::DocumentEvent) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event.clone());
    }
}

#[test]
fn an_addon_creates_and_edits_a_box() {
    let _serial = serial();
    let api = boot();

    let created = call(
        &api,
        json!({
            "call": "create",
            "name": "Slab",
            "type": "Part::Box",
            "properties": { "Length": "40 mm", "Width": "20 mm", "Height": "5 mm" },
        }),
    );
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(created["result"]["name"], "Slab");
    // `BTreeMap` order, which is the pane's order too.
    assert_eq!(
        created["result"]["properties"],
        json!(["Height", "Length", "Width"])
    );
    // The create touched the object, so the recompute executed it.
    assert_eq!(created["result"]["recomputed"], 1);

    let slab_rows = rows(&api, "Slab");
    // Part wrote the shape through the registry the host populated: this is the
    // `ferrocad_core` + `ferrocad_part` wiring, not a stub.
    let shape = row(&slab_rows, "Shape");
    assert_eq!(shape["type_id"], "Part::PropertyPartShape");
    let length = row(&slab_rows, "Length");
    assert_eq!(length["value"], "40 mm");
    assert_eq!(length["kind"], "quantity");

    // Editor text, coerced by `present` — the same rules the property pane uses.
    let set = call(
        &api,
        json!({ "call": "set", "object": "Slab", "property": "Length", "value": "25 mm" }),
    );
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["value"], "25 mm");
    assert_eq!(row(&rows(&api, "Slab"), "Length")["value"], "25 mm");
}

#[test]
fn a_doorway_edit_announces_one_pair_and_a_refusal_none() {
    let _serial = serial();
    boot_registry();
    let seen = Arc::new(RecordedEvents::default());
    let sink: Arc<dyn ferrocad_shell_api::DocumentEventSink> = seen.clone();
    let api = DocApi::with_events(sink);

    let created = call(
        &api,
        json!({ "call": "create", "name": "Slab", "type": "Part::Box" }),
    );
    assert_eq!(created["ok"], true, "{created}");
    let events = seen.take();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].id, ferrocad_shell_api::DocumentEvent::EDITED);
    assert_eq!(events[0].payload["transaction"], "Create Slab");
    assert_eq!(events[0].payload["objects"], json!(["Slab"]));
    assert_eq!(events[1].id, ferrocad_shell_api::DocumentEvent::RECOMPUTED);
    assert_eq!(events[1].payload["objects"], json!(["Slab"]));

    // A refused value never commits, so nothing is announced for it.
    let refused = call(
        &api,
        json!({ "call": "set", "object": "Slab", "property": "Length", "value": "banana" }),
    );
    assert_eq!(refused["ok"], false);
    assert!(seen.take().is_empty());

    // A committed set is one pair, and the recompute names the object that ran.
    let set = call(
        &api,
        json!({ "call": "set", "object": "Slab", "property": "Length", "value": "25 mm" }),
    );
    assert_eq!(set["ok"], true, "{set}");
    let events = seen.take();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].payload["transaction"], "Set Slab.Length");
    assert_eq!(events[1].id, ferrocad_shell_api::DocumentEvent::RECOMPUTED);
    assert_eq!(events[1].payload["objects"], json!(["Slab"]));
}

#[test]
fn the_document_lists_what_was_created() {
    let _serial = serial();
    let api = boot();

    let empty = call(&api, json!({ "call": "document" }));
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(empty["result"]["name"], "AddonSpike");
    assert_eq!(empty["result"]["objects"], json!([]));

    call(
        &api,
        json!({ "call": "create", "type": "Part::Box", "name": "Slab" }),
    );

    let document = call(&api, json!({ "call": "document" }));
    let objects = document["result"]["objects"]
        .as_array()
        .expect("objects are an array");
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["name"], "Slab");
    assert_eq!(objects[0]["type_id"], "Part::Box");
    // The payload carries the rows, so one call is enough for a display hook.
    assert!(
        objects[0]["properties"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|row| row["name"] == "Shape"),
        "{objects:?}"
    );
}

#[test]
fn a_typographical_error_is_a_response_not_a_panic() {
    let _serial = serial();
    let api = boot();

    let unknown = call(&api, json!({ "call": "nope" }));
    assert_eq!(unknown["ok"], false);
    let error = unknown["error"].as_str().expect("an error string");
    assert!(error.contains("unknown call"), "{error}");
    assert!(error.contains("recompute"), "{error}");

    let extra = call(&api, json!({ "call": "document", "x": 1 }));
    assert_eq!(extra["ok"], true, "extra keys are ignored");

    let no_object = call(
        &api,
        json!({ "call": "set", "object": "Nope", "property": "Length", "value": "1 mm" }),
    );
    assert_eq!(no_object["ok"], false);
    assert!(no_object["error"].as_str().unwrap().contains("Nope"));

    let bad_value = call(
        &api,
        json!({ "call": "create", "type": "Part::Box", "name": "Slab",
                "properties": { "Length": "banana" } }),
    );
    assert_eq!(bad_value["ok"], false, "{bad_value}");
    // The failed create rolled back: no half-built object is left behind.
    let document = call(&api, json!({ "call": "document" }));
    assert_eq!(document["result"]["objects"], json!([]));
}

#[test]
fn with_nothing_open_it_says_so() {
    let _serial = serial();
    let api = boot();
    {
        let mut registry = ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for name in registry.names() {
            registry.close_document(&name);
        }
    }

    let document = call(&api, json!({ "call": "document" }));
    assert_eq!(document["ok"], true);
    assert_eq!(document["result"], Value::Null);

    let rows = call(&api, json!({ "call": "properties", "object": "Slab" }));
    assert_eq!(rows["ok"], false);
    assert_eq!(rows["error"], "no active document");
}
