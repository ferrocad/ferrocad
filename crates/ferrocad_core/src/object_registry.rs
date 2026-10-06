//! Registered document-object types and their behaviour.
//!
//! FreeCAD object types are C++ classes deriving from `App::Feature` (or
//! `App::DocumentObject`), registered in `Base::Type`; the base ones (`App::*`) come
//! with the application and module ones (`Part::Feature`, …) come from a module. This
//! is the Rust equivalent: a name-keyed registry of [`ObjectType`] behaviours.
//!
//! Core's built-in types keep working through [`crate::typeregistry`] as the fallback;
//! a module registers its type here and both construction defaults and recompute
//! behaviour come from the registered [`ObjectType`]. See `docs/occt-integration.md`
//! §5 (the document-object SPI) and `docs/property-types.md`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::document::{Document, ObjectId};
use crate::property::Property;

/// A module-defined document-object type.
///
/// Every method has a default, so a module implements only what it needs.
pub trait ObjectType: Send + Sync {
    /// Default properties (name, initial value, status flags) added at construction.
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        Vec::new()
    }
    /// UI group and documentation for a default property.
    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        let _ = name;
        None
    }
    /// Type-specific recompute behaviour. Set properties with
    /// [`Document::set_property_raw`] so execution does not re-touch the object.
    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let _ = (doc, id);
    }
}

fn registry() -> &'static Mutex<BTreeMap<String, Arc<dyn ObjectType>>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, Arc<dyn ObjectType>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Register (or replace) the behaviour for a document-object type id.
pub fn register(type_id: &str, behavior: Arc<dyn ObjectType>) {
    registry()
        .lock()
        .unwrap()
        .insert(type_id.to_string(), behavior);
}

/// The behaviour registered for `type_id`, if any.
pub fn get(type_id: &str) -> Option<Arc<dyn ObjectType>> {
    registry().lock().unwrap().get(type_id).cloned()
}

/// Whether `type_id` has a registered behaviour.
pub fn is_registered(type_id: &str) -> bool {
    registry().lock().unwrap().contains_key(type_id)
}

/// The registered object type ids, sorted.
pub fn names() -> Vec<String> {
    registry().lock().unwrap().keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter;
    impl ObjectType for Counter {
        fn default_properties(&self) -> Vec<(String, Property, u32)> {
            vec![("Count".to_string(), Property::Integer(0), 0)]
        }
        fn execute(&self, doc: &mut Document, id: ObjectId) {
            let next = doc
                .object(id)
                .and_then(|o| o.properties.get("Count"))
                .and_then(|p| match p {
                    Property::Integer(n) => Some(*n + 1),
                    _ => None,
                })
                .unwrap_or(1);
            doc.set_property_raw(id, "Count", Property::Integer(next));
        }
    }

    #[test]
    fn registering_a_type_gives_defaults() {
        register("Test::Counter", Arc::new(Counter));
        assert!(is_registered("Test::Counter"));
        let behavior = get("Test::Counter").unwrap();
        assert_eq!(behavior.default_properties().len(), 1);
    }

    #[test]
    fn a_registered_type_constructs_and_executes() {
        register("Test::Counter2", Arc::new(Counter));
        let mut doc = Document::new();
        let id = doc.add_object("counter", "Test::Counter2");
        // Default property came from the registered `ObjectType`.
        assert_eq!(
            doc.object(id).unwrap().properties.get("Count"),
            Some(&Property::Integer(0))
        );
        // Recompute runs the registered behaviour.
        doc.touch(id, false);
        let _ = doc.recompute();
        assert_eq!(
            doc.object(id).unwrap().properties.get("Count"),
            Some(&Property::Integer(1))
        );
    }

    #[test]
    fn unknown_type_has_no_behaviour() {
        assert!(get("Nope::Nope").is_none());
    }
}
