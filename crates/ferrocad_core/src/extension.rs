//! Module-owned property values (`Property::Extension`).
//!
//! Core's `Property` enum covers the `App::Property*` value types. A module (Part)
//! needs to hold its own data — an OCCT shape — which core can neither name nor
//! serialise. `Property::Extension` boxes such a value behind [`ExtensionData`], with
//! cloning and serialisation delegated to the module.
//!
//! The newtype implements `Clone`/`Debug`/`PartialEq`/serde **by hand** so that
//! `Property`'s own derives stay intact (serde only needs the field type to implement
//! the traits). See `docs/property-value-extension.md`.

use std::any::Any;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::property_types;

/// A module-owned property value.
///
/// Must be `Send` because documents are shared behind `Arc<Mutex<…>>` (and `Arc<Mutex<T>>`
/// is `Sync` only when `T: Send`). It need not be `Sync`: a kernel handle that is only
/// `Send` (OCCT's `TopoDS_Shape`) is stored as-is; exclusive access is provided by the
/// document's own `Mutex`, not by a second lock inside the value.
///
/// Only [`type_name`](ExtensionData::type_name), [`save`](ExtensionData::save) and
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
