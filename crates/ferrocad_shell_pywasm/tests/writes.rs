//! B18b item 5: a RustPython script creates an object and edits it.
//!
//! The host is [`ferrocad_shell_pywasm::live`] — the browser's and the desktop preview's
//! composition — so the native module writes through `ferrocad_doc_api` to the
//! process-wide registry. That is what makes the write real: the model tree and the stage
//! read the same document afterwards, and the JS half's writes and these are one protocol.
//!
//! Both facades are exercised: a console-style `addObject` + attribute assignment, and
//! `ferrocad_shell.set_property`, which is the property pane's own path (transactions,
//! `coercePropertyText`, `recompute`).

use ferrocad_core::{Document, Property};
use ferrocad_shell_api::{DocumentGrant, ScriptHost as _};
use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// The registry is process-wide; start from nothing.
fn clear_registry() {
    let mut registry = ferrocad_core::application()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for name in registry.names() {
        registry.close_document(&name);
    }
}

/// One `App::FeatureTest`: a core type with a `Float` and a `Label`, so the write path is
/// exercised without a kernel.
fn sample_document() -> Document {
    let mut document = Document::new();
    document.add_object("Widget", "App::FeatureTest");
    document
}

fn evaluate(host: &PywasmScriptHost, code: &str) -> Value {
    let answer = host.call("evaluate", &[code]).expect("evaluate runs");
    serde_json::from_str(&answer).expect("evaluate returns JSON")
}

fn rows(host: &PywasmScriptHost, object: &str) -> Vec<Value> {
    let json = host
        .call("properties", &[object])
        .expect("properties() runs");
    serde_json::from_str(&json).expect("properties() returns a JSON array")
}

fn row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("{name} is a row; got {rows:?}"))
}

/// Run `f` with RustPython's 16 MiB stack (its compiler recurses; see `properties.rs`).
fn with_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

#[test]
fn a_script_creates_and_edits_the_documents_object() {
    clear_registry();
    with_big_stack(|| {
        let host = ferrocad_shell_pywasm::live().expect("the interpreter boots");
        assert_eq!(
            host.document_grant(),
            DocumentGrant::ReadWrite,
            "the host owns the registry, so the doorway is writable"
        );

        // Installed after boot: the facade re-reads the registry on every call.
        ferrocad_core::application()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert_document("Bridge", sample_document());

        // The console path: create, assign a float, rename.
        let answer = evaluate(
            &host,
            "doc = FreeCAD.ActiveDocument\n\
             obj = doc.addObject('App::FeatureTest', 'Measured')\n\
             obj.Float = 3.5\n\
             obj.Label = 'Measured body'\n\
             doc.recompute()\n",
        );
        assert_eq!(answer["ok"], true, "{answer}");

        // The registry is the document the shell draws: the write has to be *there*.
        {
            let registry = ferrocad_core::application()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let handle = registry.get("Bridge").expect("the document is registered");
            drop(registry);
            let document = handle.lock().unwrap_or_else(|error| error.into_inner());
            let object = document
                .get_by_name("Measured")
                .and_then(|id| document.object(id))
                .expect("the created object");
            assert_eq!(object.label, "Measured body");
            assert!(
                matches!(object.properties.get("Float"), Some(Property::Float(value)) if (*value - 3.5).abs() < 1e-9),
                "{:?}",
                object.properties.get("Float")
            );
        }

        // A fresh read through the facade agrees, including the label row it synthesizes.
        let read = rows(&host, "Measured");
        assert_eq!(row(&read, "Float")["value"], "3.5");
        assert_eq!(row(&read, "Label")["value"], "Measured body");

        // The headline the slice is for: a bare number on a quantity keeps its unit, the
        // same rule `Document::set_property` applies to a `Float` assignment.
        let answer = evaluate(&host, "obj.Distance = 5");
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(row(&rows(&host, "Measured"), "Distance")["value"], "5 mm");

        // The instance the script already holds was re-seeded from the fresh payload.
        assert_eq!(evaluate(&host, "obj.Float")["output"], "'3.5'");
        assert_eq!(evaluate(&host, "obj.Label")["output"], "'Measured body'");

        // The property pane's path: the bridge opens a transaction, coerces the editor
        // text, assigns, commits and recomputes.
        let set = host
            .call("set_property", &["Measured", "Float", "4.25"])
            .expect("set_property runs");
        let set: Value = serde_json::from_str(&set).expect("set_property returns JSON");
        assert_eq!(set["ok"], true, "{set}");
        assert_eq!(row(&rows(&host, "Measured"), "Float")["value"], "4.25");

        let rename = host
            .call("set_property", &["Measured", "Label", "Renamed"])
            .expect("set_property runs");
        let rename: Value = serde_json::from_str(&rename).expect("set_property returns JSON");
        assert_eq!(rename["ok"], true, "{rename}");
        assert_eq!(row(&rows(&host, "Measured"), "Label")["value"], "Renamed");

        // A refused write is a Python exception, not a silent no-op.
        let refused = evaluate(&host, "obj.Nope = 1");
        assert_eq!(refused["ok"], false, "{refused}");
        assert!(
            refused["output"]
                .as_str()
                .unwrap_or_default()
                .contains("Nope"),
            "the refusal names the property: {refused}"
        );
    });

    // A fixed-payload host is a projection, not the document: it keeps `Read` and refuses
    // the write rather than writing to a registry nobody asked it about. Own thread: the
    // document source is thread-local (one interpreter per thread).
    with_big_stack(|| {
        let host = ferrocad_shell_pywasm::new().expect("the sample host boots");
        assert_eq!(host.document_grant(), DocumentGrant::Read);
        let refused = evaluate(&host, "doc = FreeCAD.ActiveDocument\nobj = doc.Objects[0]");
        assert_eq!(refused["ok"], true, "{refused}");
        let refused = evaluate(&host, "obj.Label = 'nope'");
        assert_eq!(refused["ok"], false, "{refused}");
        assert!(
            refused["output"]
                .as_str()
                .unwrap_or_default()
                .contains("read-only"),
            "{refused}"
        );
    });
}
