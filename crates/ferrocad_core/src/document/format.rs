//! Document format selection and the read/write dispatch.
//!
//! FerroCAD keeps **two explicit serializers** and never sniffs a file's content
//! to choose between them:
//!
//! | Suffix | [`DocumentFormat`] | Backend |
//! | --- | --- | --- |
//! | `.ferrocad` (any case) | [`DocumentFormat::Native`] | [`native`] — JSON envelope |
//! | `.FCStd` (any case) | [`DocumentFormat::Fcstd`] | [`fcstd`] — ZIP + `Document.xml` |
//!
//! A path with no suffix, or with any other suffix, is an error naming both valid
//! suffixes: there is no silent default.
//!
//! # The transitional `.FCStd` rule
//!
//! No `.FCStd` **writer** exists yet (checklist items 5–8). Until it lands:
//!
//! 1. [`write`] with [`DocumentFormat::Fcstd`] is `FcstdWriter::JsonPlaceholder`:
//!    the suffix chose the backend, and the backend currently emits native bytes.
//!    This is the one deliberate dishonesty in the format policy, removed at the
//!    writer switch (item 8).
//! 2. [`read`] with [`DocumentFormat::Fcstd`] checks the first four bytes. A ZIP
//!    local-file header goes to [`fcstd::read_fcstd`]; anything else is accepted
//!    only when it carries the native envelope marker (`"format": "ferrocad"`)
//!    and is otherwise reported as "not a ZIP archive", never as a JSON parse
//!    error. The fallback is read-only and can be deleted one release after the
//!    writer lands.
//!
//! `.ferrocad` never falls back to the FCStd reader, and an unknown suffix is
//! never guessed.

use std::fmt;
use std::io;

use serde_json::Value;

use super::fcstd::{self, FcstdError};
use super::native::{self, NATIVE_FORMAT};
use super::SavedDocument;

/// The serializers a document path can select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentFormat {
    /// FerroCAD's own JSON format (`.ferrocad`).
    Native,
    /// Upstream FreeCAD's ZIP container (`.FCStd`).
    Fcstd,
}

impl DocumentFormat {
    /// Select the format from `path`'s suffix, case-insensitively.
    ///
    /// A missing or unrecognised suffix is [`FormatError::UnknownExtension`];
    /// nothing is defaulted.
    pub fn from_path(path: &str) -> Result<Self, FormatError> {
        let suffix = std::path::Path::new(path)
            .extension()
            .and_then(|s| s.to_str());
        match suffix {
            Some(s) if s.eq_ignore_ascii_case("ferrocad") => Ok(DocumentFormat::Native),
            Some(s) if s.eq_ignore_ascii_case("fcstd") => Ok(DocumentFormat::Fcstd),
            _ => Err(FormatError::UnknownExtension {
                path: path.to_owned(),
            }),
        }
    }

    /// The canonical suffix, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            DocumentFormat::Native => "ferrocad",
            DocumentFormat::Fcstd => "FCStd",
        }
    }
}

impl fmt::Display for DocumentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// Read `path` with the serializer its suffix selects.
///
/// The returned format is the one that was used, so a caller can record it and
/// reuse it on a later save.
pub fn read(path: &str) -> Result<(SavedDocument, DocumentFormat), FormatError> {
    let format = DocumentFormat::from_path(path)?;
    let saved = match format {
        DocumentFormat::Native => native::read(path)?,
        DocumentFormat::Fcstd => read_fcstd_backend(path)?,
    };
    Ok((saved, format))
}

/// Write `doc` to `path` with the serializer `format` selects.
///
/// The FCStd arm is the transitional JSON placeholder described in the module
/// docs; it is the only case where the suffix and the bytes disagree.
pub fn write(
    doc: &SavedDocument,
    path: &str,
    format: DocumentFormat,
) -> Result<(), FormatError> {
    match format {
        DocumentFormat::Native => native::write(doc, path),
        // `FcstdWriter::JsonPlaceholder`: removed at the writer switch (item 8).
        DocumentFormat::Fcstd => native::write(doc, path),
    }
}

/// The `.FCStd` read arm: ZIP first, then the marker-gated native fallback.
fn read_fcstd_backend(path: &str) -> Result<SavedDocument, FormatError> {
    let data = std::fs::read(path).map_err(|e| FormatError::io(path, e))?;
    if data.starts_with(b"PK\x03\x04") {
        return fcstd::read_fcstd(path).map_err(|e| FormatError::Fcstd {
            path: path.to_owned(),
            source: e,
        });
    }
    // Transitional, read-only: FerroCAD used to write native JSON under an
    // `.FCStd` name. Accept it only when the envelope carries the marker, so a
    // truncated or renamed ZIP is reported below instead of as a JSON error.
    let probe: Value = serde_json::from_slice(&data)
        .map_err(|_| FormatError::NeitherZipNorNative { path: path.to_owned() })?;
    if probe.get("format").and_then(Value::as_str) != Some(NATIVE_FORMAT) {
        return Err(FormatError::NeitherZipNorNative {
            path: path.to_owned(),
        });
    }
    native::decode(path, &data)
}

/// Errors from format selection, encoding, and both read/write backends.
#[derive(Debug)]
pub enum FormatError {
    /// The path has no suffix, or a suffix neither serializer recognises.
    UnknownExtension { path: String },
    /// A native envelope names a format other than `ferrocad`.
    NotNative { path: String, format: String },
    /// A native envelope was written by a newer FerroCAD.
    NewerVersion { path: String, version: u32 },
    /// An `.FCStd` path holds neither a ZIP archive nor a native envelope.
    NeitherZipNorNative { path: String },
    /// The file could not be read or written.
    Io { path: String, source: io::Error },
    /// Native JSON could not be parsed.
    Json {
        path: String,
        source: serde_json::Error,
    },
    /// Native JSON could not be produced.
    Encode(serde_json::Error),
    /// The `.FCStd` backend failed.
    Fcstd { path: String, source: FcstdError },
}

impl FormatError {
    pub(crate) fn io(path: &str, source: io::Error) -> Self {
        FormatError::Io {
            path: path.to_owned(),
            source,
        }
    }

    pub(crate) fn json(path: &str, source: serde_json::Error) -> Self {
        FormatError::Json {
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::UnknownExtension { path } => write!(
                f,
                "unknown document extension for '{path}': expected '.ferrocad' or '.FCStd'"
            ),
            FormatError::NotNative { path, format } => write!(
                f,
                "'{path}' is not a FerroCAD document: envelope format '{format}' \
                 (expected '{NATIVE_FORMAT}')"
            ),
            FormatError::NewerVersion { path, version } => write!(
                f,
                "'{path}' was written by a newer FerroCAD (format version {version}; \
                 this build reads up to version {})",
                native::NATIVE_VERSION
            ),
            FormatError::NeitherZipNorNative { path } => write!(
                f,
                "'{path}' is not a ZIP archive and carries no FerroCAD native envelope"
            ),
            FormatError::Io { path, source } => write!(f, "cannot access '{path}': {source}"),
            FormatError::Json { path, source } => {
                write!(f, "malformed FerroCAD document '{path}': {source}")
            }
            FormatError::Encode(source) => {
                write!(f, "cannot encode the document as FerroCAD JSON: {source}")
            }
            FormatError::Fcstd { path, source } => write!(f, "cannot read '{path}': {source}"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormatError::Io { source, .. } => Some(source),
            FormatError::Json { source, .. } | FormatError::Encode(source) => Some(source),
            FormatError::Fcstd { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn temp_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("ferrocad-format-{}-{name}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    fn sample_document() -> SavedDocument {
        let mut doc = Document::new();
        let id = doc.add_object("Box", "App::FeaturePython");
        assert!(doc.set_label(id, "a box"));
        doc.to_saved("Sample")
    }

    #[test]
    fn from_path_selects_by_suffix_case_insensitively() {
        let cases = [
            ("model.ferrocad", Ok(DocumentFormat::Native)),
            ("model.FERROCAD", Ok(DocumentFormat::Native)),
            ("model.Ferrocad", Ok(DocumentFormat::Native)),
            ("model.FCStd", Ok(DocumentFormat::Fcstd)),
            ("model.fcstd", Ok(DocumentFormat::Fcstd)),
            ("model.FCStd.bak", Err(())),
            ("model.FcStD", Ok(DocumentFormat::Fcstd)),
            ("/tmp/dir.with.dots/model.ferrocad", Ok(DocumentFormat::Native)),
            ("model", Err(())),
            ("model.json", Err(())),
            ("model.ferrocad.json", Err(())),
            (".ferrocad", Err(())),
            ("model.", Err(())),
        ];
        for (path, expected) in cases {
            match (DocumentFormat::from_path(path), expected) {
                (Ok(actual), Ok(want)) => assert_eq!(actual, want, "{path}"),
                (Err(err), Err(())) => {
                    let message = err.to_string();
                    assert!(
                        message.contains(".ferrocad") && message.contains(".FCStd"),
                        "{path}: {message}"
                    );
                }
                (actual, want) => panic!("{path}: got {actual:?}, wanted {want:?}"),
            }
        }
    }

    #[test]
    fn extension_is_canonical() {
        assert_eq!(DocumentFormat::Native.extension(), "ferrocad");
        assert_eq!(DocumentFormat::Fcstd.extension(), "FCStd");
    }

    #[test]
    fn native_round_trip_carries_the_envelope() {
        let path = temp_path("native.ferrocad");
        let doc = sample_document();
        write(&doc, &path, DocumentFormat::Native).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes[0], b'{');
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["format"], "ferrocad");
        assert_eq!(json["version"], 1);
        assert_eq!(json["name"], "Sample");

        let (read_back, format) = read(&path).unwrap();
        assert_eq!(format, DocumentFormat::Native);
        assert_eq!(read_back.format, "ferrocad");
        assert_eq!(read_back.version, 1);
        assert_eq!(read_back.name, "Sample");
        assert_eq!(read_back.objects.len(), 1);
        assert_eq!(read_back.objects[0].name, "Box");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bare_native_json_is_version_1() {
        let path = temp_path("bare.ferrocad");
        std::fs::write(&path, br#"{"name":"Old","objects":[]}"#).unwrap();
        let (doc, format) = read(&path).unwrap();
        assert_eq!(format, DocumentFormat::Native);
        assert_eq!(doc.format, "ferrocad");
        assert_eq!(doc.version, 1);
        assert_eq!(doc.name, "Old");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_native_format_and_future_version_are_rejected() {
        let path = temp_path("wrong-format.ferrocad");
        std::fs::write(
            &path,
            br#"{"format":"something-else","version":1,"name":"X","objects":[]}"#,
        )
        .unwrap();
        let err = read(&path).unwrap_err().to_string();
        assert!(err.contains("something-else"), "{err}");
        assert!(err.contains(&path), "{err}");

        std::fs::write(
            &path,
            br#"{"format":"ferrocad","version":2,"name":"X","objects":[]}"#,
        )
        .unwrap();
        let err = read(&path).unwrap_err().to_string();
        assert!(err.contains("newer FerroCAD"), "{err}");
        assert!(err.contains(&path), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fcstd_placeholder_round_trip() {
        let path = temp_path("placeholder.FCStd");
        let doc = sample_document();
        write(&doc, &path, DocumentFormat::Fcstd).unwrap();

        // The placeholder is native JSON: the one documented dishonesty.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes[0], b'{');

        let (read_back, format) = read(&path).unwrap();
        assert_eq!(format, DocumentFormat::Fcstd);
        assert_eq!(read_back.name, "Sample");
        assert_eq!(read_back.objects[0].name, "Box");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zip_named_ferrocad_is_rejected() {
        let path = temp_path("renamed.ferrocad");
        std::fs::write(&path, b"PK\x03\x04not really a document").unwrap();
        let err = read(&path).unwrap_err();
        assert!(
            matches!(err, FormatError::Json { .. }),
            "the native arm must not fall back to FCStd: {err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncated_zip_named_fcstd_reports_a_zip_error() {
        let path = temp_path("truncated.FCStd");
        std::fs::write(&path, b"PK\x03\x04 truncated archive").unwrap();
        let err = read(&path).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("not a ZIP archive"), "{message}");
        assert!(!message.contains("malformed FerroCAD"), "{message}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn json_without_marker_named_fcstd_is_rejected() {
        let path = temp_path("plain.FCStd");
        std::fs::write(&path, br#"{"name":"X","objects":[]}"#).unwrap();
        let message = read(&path).unwrap_err().to_string();
        assert!(message.contains("not a ZIP archive"), "{message}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_suffix_is_rejected() {
        let path = temp_path("model.json");
        let message = DocumentFormat::from_path(&path).unwrap_err().to_string();
        assert!(message.contains(".ferrocad"), "{message}");
        assert!(message.contains(".FCStd"), "{message}");
        assert!(read(&path).is_err());
        assert!(!std::path::Path::new(&path).exists());
    }
}
