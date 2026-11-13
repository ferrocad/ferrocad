//! The bridge payload: the document, as `python/ferrocad_shell` reads it.
//!
//! The presenter is [`ferrocad_doc_api::payload`] — the same one a JS addon gets over
//! the JSON API — so the console, the property pane and an addon cannot drift.
//! This module adds only what is specific to the wasm-Python half: the sample a host
//! with no document registry boots with.
//!
//! That sample is the one checked-in `python/ferrocad_shell/Sample.FCStd`, embedded
//! here and in `ferrocad_shell_rust`: the desktop's Python `bootstrap()` opens the
//! same bytes from disk, so both hosts boot one document name, object tree and shape
//! rather than two recipes that can drift.

use std::sync::Arc;

use ferrocad_core::Document;
use ferrocad_occt::OcctBackend;

/// The active document in `ferrocad_core`'s registry, as JSON (`null` when none).
///
/// The registry lock is released before the document is locked — the same order
/// `RustAppBackend` keeps, so a callback walking the other way cannot deadlock.
pub use ferrocad_doc_api::payload::active_json as active;

/// The sample document's name.
pub const DOCUMENT: &str = "Sample";

/// The one startup document both hosts boot.
const SAMPLE_FCSTD: &[u8] = include_bytes!("../../../python/ferrocad_shell/Sample.FCStd");

/// Load the canonical sample with the linked kernel.
///
/// Part is registered before the archive is decoded: `decode_fcstd` restores the
/// `Part::PropertyPartShape` payload through the property-type registry, and
/// `recompute` rebuilds the fillet from its `Base`/`Edge1` link. Registration is
/// idempotent, so a host that already registered its backend keeps it.
pub fn sample() -> Result<Document, String> {
    ferrocad_part::register(Arc::new(OcctBackend::new()));
    let saved =
        ferrocad_core::fcstd::decode_fcstd(SAMPLE_FCSTD).map_err(|error| error.to_string())?;
    let mut document = Document::from_saved(&saved);
    document.recompute()?;
    Ok(document)
}

/// [`sample`] under its own name, as the facade's `FreeCAD.ActiveDocument`.
pub fn render(document: &Document) -> String {
    ferrocad_doc_api::payload::render(DOCUMENT, document)
}
