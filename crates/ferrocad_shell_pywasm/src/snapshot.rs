//! The bridge payload: the document, as `python/ferrocad_shell` reads it.
//!
//! The presenter is [`ferrocad_doc_api::payload`] — the same one a JS addon gets over
//! the JSON doorway — so the console, the property pane and an addon cannot drift.
//! This module adds only what is specific to the wasm-Python half: the geometry-free
//! sample a host with no document registry boots with.
//!
//! The sample is built here, not through the kernel: `recompute` would need a
//! registered Part backend, and none of its rows comes from one. That is the half's
//! one documented shortcut, and it is why the crate has no `ferrocad_part`.

use ferrocad_core::{Document, Placement as DocPlacement, Property, Quantity, Rotation, Vector3};

/// The active document in `ferrocad_core`'s registry, as JSON (`null` when none).
///
/// The registry lock is released before the document is locked — the same order
/// `RustAppBackend` keeps, so a callback walking the other way cannot deadlock.
pub use ferrocad_doc_api::payload::active_json as active;

/// The edge the sample's fillet is applied to (the browser's `EDGE`).
pub const EDGE: &str = "Edge1";

/// The sample's fillet radius, in millimetres (the browser's `RADIUS`).
pub const RADIUS: f64 = 3.0;

/// The sample document's name.
pub const DOCUMENT: &str = "Sample";

/// Build the sample's objects and properties with no kernel call.
///
/// `recompute` is deliberately **not** called: it needs a registered Part backend
/// (OCCT), and none of the property rows comes from it. That is the one documented
/// shortcut, and it is why the document can be built here at all.
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

/// [`sample`] under its own name, as the facade's `FreeCAD.ActiveDocument`.
pub fn render(document: &Document) -> String {
    ferrocad_doc_api::payload::render(DOCUMENT, document)
}
