//! The bridge payload: the sample document's property rows, as JSON.
//!
//! The spike's **native layer is one function** ([`render`]). Everything a property
//! row *means* — the editor kind and the display text — is decided by
//! `ferrocad_core::present`, exactly as the browser host decides it, and the Python
//! facade in `bootstrap.py` only hands it to the script. Nothing is re-derived in
//! Python, so the two hosts cannot drift.
//!
//! The document is the sample's *content*, not `ferrocad_shell_rust`'s instance: the
//! browser builds its box through the OCCT kernel, and this crate deliberately keeps
//! no kernel (the wasm-Python runtime is the subject, not geometry). The property
//! rows are the same ones the browser shows for `Box` and `Fillet`.

use ferrocad_core::present;
use ferrocad_core::{Document, Placement as DocPlacement, Property, Quantity, Rotation, Vector3};
use serde_json::{Value, json};

/// The edge the sample's fillet is applied to (the browser's `EDGE`).
pub const EDGE: &str = "Edge1";

/// The sample's fillet radius, in millimetres (the browser's `RADIUS`).
pub const RADIUS: f64 = 3.0;

/// The sample document's name.
pub const DOCUMENT: &str = "Sample";

/// Build the sample's objects and properties with no kernel call.
///
/// `recompute` is deliberately **not** called: it needs a registered Part backend
/// (OCCT), and none of the property rows comes from it. That is the spike's one
/// documented shortcut, and it is why the document can be built here at all.
pub fn sample() -> Document {
    let mut document = Document::new();

    let base = document.add_object("Box", "Part::Box");
    document
        .set_property(base, "Length", Property::Quantity(mm(10.0)))
        .expect("Length on a fresh box");
    document
        .set_property(base, "Width", Property::Quantity(mm(10.0)))
        .expect("Width on a fresh box");
    document
        .set_property(base, "Height", Property::Quantity(mm(10.0)))
        .expect("Height on a fresh box");
    document
        .set_property(
            base,
            "Placement",
            Property::Placement(DocPlacement::new(
                Vector3::new(-5.0, -5.0, -5.0),
                Rotation::identity(),
            )),
        )
        .expect("Placement on a fresh box");

    let fillet = document.add_object("Fillet", "Part::Fillet");
    document
        .set_property(
            fillet,
            "Base",
            Property::LinkSub("Box".to_string(), vec![EDGE.to_string()]),
        )
        .expect("Base on a fresh fillet");
    document
        .set_property(fillet, "Radius", Property::Float(RADIUS))
        .expect("Radius on a fresh fillet");

    document
}

/// A millimetre-valued quantity.
fn mm(value: f64) -> Quantity {
    Quantity::parse(&format!("{value} mm")).expect("a millimetre quantity")
}

/// The document as the Python facade's `FreeCAD` module reads it.
///
/// Property order is the container's (a `BTreeMap`, so by name), which is what the
/// property pane shows for a Rust-backed document too.
pub fn render(document: &Document) -> String {
    let objects: Vec<Value> = document
        .topological_sorted_objects()
        .into_iter()
        .filter_map(|id| document.object(id))
        .map(object)
        .collect();
    json!({ "name": DOCUMENT, "objects": objects }).to_string()
}

/// One object's rows.
fn object(object: &ferrocad_core::DocumentObject) -> Value {
    let properties: Vec<Value> = object
        .properties
        .iter()
        .map(|(name, value)| row(name, value))
        .collect();
    json!({
        "name": object.name,
        "label": object.label,
        "type_id": object.type_id,
        "additional_type_ids": object.type_id.split("::").map(str::to_string).collect::<Vec<_>>(),
        "properties": properties,
        "driven": object.expressions.keys().cloned().collect::<Vec<_>>(),
    })
}

/// One property row: the trait asks for a value, a type and a kind; `present` owns
/// the last two.
fn row(name: &str, value: &Property) -> Value {
    // The shell's `PropRow.kind` is `""` for a property with no editor, and the
    // script's `properties()` reads a non-empty kind as "editable".
    let kind = present::kind(value).map(|kind| kind.as_str()).unwrap_or("");
    // A `PropertyType` row is the property's own status; the spike reports the
    // display status FreeCAD shows for a plain property.
    json!({
        "name": name,
        "type_id": value.type_name(),
        "kind": kind,
        "value": present::format(value),
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
