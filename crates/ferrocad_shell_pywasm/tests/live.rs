//! The bridge over the process-wide registry: the document the host installed.
//!
//! The browser's wasm-Python configuration pairs this script half with
//! `RustAppBackend::from_shared_registry`, so `FreeCAD.ActiveDocument` and the model
//! tree are one document. This proves that pairing without a browser: with nothing
//! open the facade says so, then it reads a document installed *after* boot, then it
//! follows a replacement — with no interpreter restart in between.

use ferrocad_core::{Document, Property, Quantity};
use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// The registry is process-wide; start from nothing so "no active document" means it.
fn clear_registry() {
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
}

/// A document with one object and one quantity, built without a kernel.
fn widget(length: &str) -> Document {
    let mut document = Document::new();
    let object = document.add_object("Widget", "App::FeaturePython");
    let quantity = Quantity::parse(length).expect("a quantity");
    document
        .set_property(object, "Length", Property::Quantity(quantity))
        .expect("Length on a fresh object");
    document
}

fn rows(host: &PywasmScriptHost, object: &str) -> Vec<Value> {
    let json = host.properties(object).expect("properties() runs");
    serde_json::from_str(&json).expect("properties() returns a JSON array")
}

fn row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("{name} is a row; got {rows:?}"))
}

#[test]
fn the_facade_follows_the_registry() {
    clear_registry();
    std::thread::Builder::new()
        // RustPython's compiler recurses; see `properties.rs` for the same requirement.
        .stack_size(16 << 20)
        .spawn(|| {
            let host = PywasmScriptHost::live().expect("the interpreter boots");

            // Nothing is open: an error, not a fabricated sample.
            assert!(
                host.properties("Widget").is_err(),
                "an empty registry has no active document"
            );

            ferrocad_core::application()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert_document("Bridge", widget("10 mm"));

            let first = rows(&host, "Widget");
            assert_eq!(row(&first, "Length")["value"], "10 mm");
            assert_eq!(row(&first, "Length")["kind"], "quantity");

            // The document half replaces the contents inside the same handle (that is
            // what an open does); the facade re-reads, so the console cannot show the
            // rows that existed at import.
            {
                let registry = ferrocad_core::application()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let handle = registry.get("Bridge").expect("the document is registered");
                drop(registry);
                *handle.lock().unwrap_or_else(|error| error.into_inner()) = widget("25 mm");
            }

            let second = rows(&host, "Widget");
            assert_eq!(row(&second, "Length")["value"], "25 mm");

            // The name comes from the registry, not from a constant in the bridge, and
            // the console namespace is seeded (`FreeCAD`, `App`, `doc`) even though the
            // shell script's own `bootstrap()` never ran here.
            let answer = host
                .call("evaluate", "FreeCAD.ActiveDocument.Name")
                .expect("evaluate runs");
            let value: Value = serde_json::from_str(&answer).expect("evaluate returns JSON");
            assert_eq!(value["ok"], true);
            assert_eq!(value["output"], "'Bridge'");
        })
        .expect("a worker thread")
        .join()
        .expect("the thread did not panic");
}
