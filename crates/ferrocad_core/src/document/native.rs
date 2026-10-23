//! FerroCAD's native document serializer: JSON plus a format/version envelope.
//!
//! A native document is UTF-8 JSON shaped like
//!
//! ```json
//! {"format": "ferrocad", "version": 1, "name": "…", "objects": [...]}
//! ```
//!
//! The envelope identifies the format (a `.FCStd` placeholder written before the
//! FCStd writer exists is detected by the `format` marker) and lets a future
//! FerroCAD refuse a document it is too old to read. Both fields are optional on
//! input: files written before the envelope existed are version 1, and unknown
//! top-level keys are ignored rather than denied.
//!
//! [`encode`] is the single writer, so `saveAs`, `save` and `dump` all produce the
//! same bytes. Reading a file (with envelope checks) is [`read`]; [`decode`] is the
//! in-memory half used by the transitional `.FCStd` JSON fallback.

use serde::Serialize;

use super::format::FormatError;
use super::{SavedDocument, SavedObject};

/// The value of the envelope's `format` field.
pub const NATIVE_FORMAT: &str = "ferrocad";

/// The envelope version this build writes (and the newest one it reads).
pub const NATIVE_VERSION: u32 = 1;

/// Serde default: a document without a `format` field is native.
pub fn default_format() -> String {
    NATIVE_FORMAT.to_owned()
}

/// Serde default: a document without a `version` field is version 1.
pub fn default_version() -> u32 {
    NATIVE_VERSION
}

/// Encode `doc` as native JSON bytes, always writing the current envelope.
pub fn encode(doc: &SavedDocument) -> Result<Vec<u8>, FormatError> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        format: &'static str,
        version: u32,
        name: &'a str,
        objects: &'a [SavedObject],
    }

    serde_json::to_vec(&Envelope {
        format: NATIVE_FORMAT,
        version: NATIVE_VERSION,
        name: &doc.name,
        objects: &doc.objects,
    })
    .map_err(FormatError::Encode)
}

/// Write `doc` to `path` as native JSON.
pub fn write(doc: &SavedDocument, path: &str) -> Result<(), FormatError> {
    let bytes = encode(doc)?;
    std::fs::write(path, bytes).map_err(|e| FormatError::io(path, e))
}

/// Read and validate a native document from `path`.
pub fn read(path: &str) -> Result<SavedDocument, FormatError> {
    let data = std::fs::read(path).map_err(|e| FormatError::io(path, e))?;
    decode(path, &data)
}

/// Decode and validate native bytes already in memory.
///
/// `path` only names the document in error messages.
pub fn decode(path: &str, data: &[u8]) -> Result<SavedDocument, FormatError> {
    let doc: SavedDocument =
        serde_json::from_slice(data).map_err(|e| FormatError::json(path, e))?;
    check_envelope(path, &doc)?;
    Ok(doc)
}

/// Reject an envelope this build cannot honour.
///
/// A missing envelope has already been defaulted by serde (`ferrocad` / 1).
pub fn check_envelope(path: &str, doc: &SavedDocument) -> Result<(), FormatError> {
    if doc.format != NATIVE_FORMAT {
        return Err(FormatError::NotNative {
            path: path.to_owned(),
            format: doc.format.clone(),
        });
    }
    if doc.version > NATIVE_VERSION {
        return Err(FormatError::NewerVersion {
            path: path.to_owned(),
            version: doc.version,
        });
    }
    Ok(())
}
