//! Module-owned property values (`Property::Extension`).
//!
//! Core's `Property` enum covers the `App::Property*` value types. A module (Part)
//! needs to hold its own data — an OCCT shape — which core can neither name nor
//! serialise. `Property::Extension` boxes such a value behind [`ExtensionData`], with
//! cloning and serialisation delegated to the module.
//!
//! The newtype implements `Clone`/`Debug`/`PartialEq`/serde **by hand** so that
//! `Property`'s own derives stay intact (serde only needs the field type to implement
//! the traits). See `docs/developer/architecture/property-value-extension.md`.

use std::any::Any;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::property_types;

/// The `.FCStd` shape of a module-owned payload: the archive member stem, the
/// `Document.xml` element tag and the member's file extension.
///
/// A single payload is written as `{stem}.{extension}` and referenced by
/// `<{tag} file="…"/>`; a repeated stem is numbered (`MeshKernel1.bms`). Core
/// never interprets the payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FcstdKind {
    /// Base member name without the extension, e.g. `"PartShape"`.
    pub stem: &'static str,
    /// `Document.xml` element for a single payload, e.g. `"Part"`.
    pub tag: &'static str,
    /// Member extension without the dot, e.g. `"brp"`.
    pub extension: &'static str,
}

impl FcstdKind {
    /// A kind from its three parts.
    pub const fn new(stem: &'static str, tag: &'static str, extension: &'static str) -> Self {
        FcstdKind {
            stem,
            tag,
            extension,
        }
    }

    /// The default kind: `PartShapeN.brp` referenced by `<Part file="…"/>`.
    pub const PART_SHAPE: FcstdKind = FcstdKind::new("PartShape", "Part", "brp");
}

/// A module-owned property value.
///
/// Must be `Send` because documents are shared behind `Arc<Mutex<…>>` (and `Arc<Mutex<T>>`
/// is `Sync` only when `T: Send`). It need not be `Sync`: a kernel handle that is only
/// `Send` (OCCT's `TopoDS_Shape`) is stored as-is; exclusive access is provided by the
/// document's own `Mutex`, not by a second lock inside the value.
///
/// Only [`type_name`](ExtensionData::type_name), [`save`](ExtensionData::save),
/// [`fcstd_members`](ExtensionData::fcstd_members),
/// [`fcstd_kind`](ExtensionData::fcstd_kind),
/// [`fcstd_sidecar`](ExtensionData::fcstd_sidecar),
/// [`resolve_element_name`](ExtensionData::resolve_element_name) and
/// [`clone_box`](ExtensionData::clone_box) cross the core boundary; core never
/// interprets the bytes.
pub trait ExtensionData: Send {
    /// The registered type name, e.g. `"Part::PropertyPartShape"`.
    fn type_name(&self) -> &str;
    /// A deep clone (the enum's `Clone` cannot be derived through a trait object).
    fn clone_box(&self) -> Box<dyn ExtensionData>;
    /// For equality: downcast via [`Any`].
    fn as_any(&self) -> &dyn Any;
    /// Equality within the same `type_name`.
    fn eq(&self, other: &dyn ExtensionData) -> bool;
    /// Serialise the value; core stores the bytes verbatim.
    fn save(&self) -> Vec<u8>;
    /// The value's `Document.xml` payload for a binary `.FCStd` save.
    ///
    /// A module value that upstream persists in separate ZIP members (shapes,
    /// shape lists) returns them here; core writes the bytes verbatim under the
    /// names [`fcstd_kind`](ExtensionData::fcstd_kind) supplies. The tuple is
    /// `(suffix, bytes)`:
    ///
    /// - one entry with an **empty** suffix is a single payload, written as
    ///   `{stem}.{extension}` (the default kind: `PartShapeN.brp`);
    /// - entries with a non-empty suffix (`"0"`, `"1"`, …) are a list, written
    ///   as `PartShapeN.M.brp` with `<ShapeList count="…"><TopoShape file="…"/>`.
    ///
    /// `None` (the default) means the value has no upstream `.FCStd`
    /// representation yet; the writer then refuses the save and names the type
    /// instead of silently dropping the value. Core never interprets the bytes.
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        None
    }
    /// The archive member stem, XML tag and file extension for
    /// [`fcstd_members`](ExtensionData::fcstd_members).
    ///
    /// The default is the Part shape kind (`PartShapeN.brp` under `<Part>`), so a
    /// module that only supplies the bytes keeps working unchanged. A value
    /// persisted as `MeshKernelN.bms` under `<Mesh>` returns that kind instead.
    /// Core uses it for both the member name and the element it writes.
    fn fcstd_kind(&self) -> FcstdKind {
        FcstdKind::PART_SHAPE
    }
    /// An optional secondary `.FCStd` member carrying module metadata that must
    /// not go into the upstream-compatible payload.
    ///
    /// A module whose [`fcstd_members`](ExtensionData::fcstd_members) is a
    /// **single** payload may return `(suffix, bytes)` here. Core writes the
    /// bytes to `{stem}.{suffix}` and references that member from the `<Part>`
    /// element with a `FerroCADSidecar` attribute, which upstream FreeCAD
    /// ignores. The payload member itself (and therefore `PartShapeN.brp`) stays
    /// exactly what `fcstd_members` returned — raw BREP for a shape — so upstream
    /// still opens the file.
    ///
    /// The suffix must be a safe member-name fragment (`"lineage"`, not a path).
    /// The default is `None`. Sidecars are not written for shape-list payloads.
    fn fcstd_sidecar(&self) -> Option<(String, Vec<u8>)> {
        None
    }
    /// Resolve a stored sub-element name against this value's lineage.
    ///
    /// The return type is `Result<Option<String>, &'static str>`:
    ///
    /// - `Ok(Some(current))` — the name denotes a current sub-element; core
    ///   callers should use `current` (the module may return the stored name).
    /// - `Ok(None)` — the value has no lineage for the name, so a caller may fall
    ///   back to positional naming. This is also the default, so a module value
    ///   that knows nothing about sub-element names keeps the old behaviour.
    /// - `Err("retired")` — the value carries a non-empty lineage that does not
    ///   justify the name. A caller must fail the reference rather than retarget
    ///   it positionally.
    /// - `Err("missing")` — the name does not denote an element of this value.
    ///
    /// The string payload deliberately avoids naming a core error type; the
    /// vocabulary (`retired`/`missing`) is part of this contract.
    fn resolve_element_name(&self, _name: &str) -> Result<Option<String>, &'static str> {
        Ok(None)
    }
    /// Human-readable form for `Debug`.
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

/// A boxed module value, with the impls `Property`'s derives require.
pub struct ExtensionValue(Box<dyn ExtensionData>);

impl ExtensionValue {
    pub fn new(data: Box<dyn ExtensionData>) -> Self {
        ExtensionValue(data)
    }

    /// The boxed value.
    pub fn data(&self) -> &dyn ExtensionData {
        self.0.as_ref()
    }

    /// The registered type name of the boxed value.
    pub fn type_name(&self) -> &str {
        self.0.type_name()
    }
}

impl Clone for ExtensionValue {
    fn clone(&self) -> Self {
        ExtensionValue(self.0.clone_box())
    }
}

impl fmt::Debug for ExtensionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.debug(f)
    }
}

impl PartialEq for ExtensionValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.type_name() == other.0.type_name() && self.0.eq(other.0.as_ref())
    }
}

/// The wire form: `{ "type": <name>, "bytes": [...] }`.
#[derive(Serialize, Deserialize)]
struct RawExtension {
    #[serde(rename = "type")]
    type_name: String,
    bytes: Vec<u8>,
}

impl Serialize for ExtensionValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RawExtension {
            type_name: self.0.type_name().to_string(),
            bytes: self.0.save(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExtensionValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawExtension::deserialize(deserializer)?;
        property_types::restore_extension(&raw.type_name, &raw.bytes)
            .map(ExtensionValue)
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "property type '{}' is not registered (module not loaded?)",
                    raw.type_name
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::Property;

    /// A toy module value: a counter with its own byte encoding.
    struct Counter(i64);

    impl ExtensionData for Counter {
        fn type_name(&self) -> &str {
            "Test::PropertyCounter"
        }
        fn clone_box(&self) -> Box<dyn ExtensionData> {
            Box::new(Counter(self.0))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn eq(&self, other: &dyn ExtensionData) -> bool {
            other
                .as_any()
                .downcast_ref::<Counter>()
                .is_some_and(|c| c.0 == self.0)
        }
        fn save(&self) -> Vec<u8> {
            self.0.to_le_bytes().to_vec()
        }
        fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Counter({})", self.0)
        }
    }

    fn restore(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
        let arr: [u8; 8] = bytes.try_into().ok()?;
        Some(Box::new(Counter(i64::from_le_bytes(arr))))
    }

    fn register() {
        property_types::register_extension(
            "Test::PropertyCounter",
            || Property::Extension(ExtensionValue::new(Box::new(Counter(0)))),
            restore,
        );
    }

    #[test]
    fn clone_equality_debug_and_type_name() {
        register();
        let a = Property::Extension(ExtensionValue::new(Box::new(Counter(3))));
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(
            a,
            Property::Extension(ExtensionValue::new(Box::new(Counter(4))))
        );
        assert_eq!(format!("{a:?}"), "Extension(Counter(3))");
        assert_eq!(a.type_name(), "Test::PropertyCounter");
    }

    #[test]
    fn round_trips_through_json() {
        register();
        let p = Property::Extension(ExtensionValue::new(Box::new(Counter(42))));
        let json = serde_json::to_string(&p).unwrap();
        let back: Property = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn unregistered_type_fails_to_load() {
        let json = r#"{"Extension":{"type":"Nope::PropertyX","bytes":[1,2]}}"#;
        let err = serde_json::from_str::<Property>(json).unwrap_err().to_string();
        assert!(err.contains("not registered"), "{err}");
    }
}
