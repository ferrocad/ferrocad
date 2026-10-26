//! Registered document-object types and their behaviour.
//!
//! FreeCAD object types are C++ classes deriving from `App::Feature` (or
//! `App::DocumentObject`), registered in `Base::Type`; the base ones (`App::*`) come
//! with the application and module ones (`Part::Feature`, …) come from a module. This
//! is the Rust equivalent: a name-keyed registry of [`ObjectType`] behaviours.
//!
//! Core's built-in types keep working through [`crate::typeregistry`] as the fallback;
//! a module registers its type here and both construction defaults and recompute
//! behaviour come from the registered [`ObjectType`]. See `docs/architecture/occt-integration.md`
//! §5 (the document-object SPI) and `docs/architecture/property-types.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::document::{Document, ObjectId};
use crate::property::Property;

/// Prefix that marks a rejected property assignment as a Python `AttributeError`.
///
/// [`Document::set_property`] reports every failure as a `String`; a few, such as
/// writing an immutable property, have to become `AttributeError` rather than
/// `ValueError` in the Python binding. [`PropertyChangeError::Attribute`] renders
/// with this prefix, and the binding strips it to choose the exception class.
pub const ATTRIBUTE_REJECTION_PREFIX: &str = "AttributeError: ";

/// Why an [`ObjectType::on_property_change`] hook refused an assignment.
///
/// The distinction is the Python exception the binding raises: upstream rejects
/// an invalid value with `ValueError`, and an immutable property with
/// `AttributeError` (the assignment never reaches `onChanged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyChangeError {
    /// The value is not acceptable in the object's current state.
    Value(String),
    /// The property cannot be written in the object's current state.
    Attribute(String),
}

impl PropertyChangeError {
    /// Reject as an invalid value (Python `ValueError`).
    pub fn value(message: impl Into<String>) -> Self {
        Self::Value(message.into())
    }

    /// Reject as an immutable property (Python `AttributeError`).
    pub fn attribute(message: impl Into<String>) -> Self {
        Self::Attribute(message.into())
    }
}

impl fmt::Display for PropertyChangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(message) => f.write_str(message),
            Self::Attribute(message) => {
                f.write_str(ATTRIBUTE_REJECTION_PREFIX)?;
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for PropertyChangeError {}

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
    /// [`Document::set_property_raw`] so execution does not re-touch the object, and mark
    /// the object with [`Document::mark_invalid`] when the rebuild cannot run; recompute
    /// reports the failure and leaves the flag set until an execute succeeds.
    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let _ = (doc, id);
    }

    /// Veto a property assignment before it is committed (FreeCAD's property
    /// `onChanged` guard).
    ///
    /// [`Document::set_property`] calls this after reading the old value and before
    /// writing the new one, so `doc` still describes the pre-assignment state.
    /// Returning `Err` leaves the object untouched. The default accepts every
    /// assignment, so this hook is additive: existing types keep working unchanged.
    ///
    /// Use [`PropertyChangeError::value`] for a value that is invalid in the
    /// current state and [`PropertyChangeError::attribute`] for a write that must
    /// be refused because the property is immutable.
    fn on_property_change(
        &self,
        doc: &Document,
        id: ObjectId,
        name: &str,
        value: &Property,
    ) -> Result<(), PropertyChangeError> {
        let _ = (doc, id, name, value);
        Ok(())
    }

    /// React to a property assignment that has already been committed (FreeCAD's
    /// `onChanged` side of the same hook).
    ///
    /// [`Document::set_property`] calls this after the value is stored, so a type
    /// can keep a **derived** property in sync without a recompute. Use
    /// [`Document::set_property_raw`] for the derived write: it neither records an
    /// undo step nor calls this hook, so the two cannot recurse. The default is a
    /// no-op.
    ///
    /// This is where a `Mesh::Feature` mirrors `Placement` into its mesh transform
    /// (and adopts a newly assigned mesh's transform into `Placement`), which is
    /// why `obj.Mesh.Topology` follows `obj.Placement` immediately.
    fn on_property_changed(&self, doc: &mut Document, id: ObjectId, name: &str) {
        let _ = (doc, id, name);
    }

    /// Whether this type's `execute` delegates the rebuild to a Python proxy.
    ///
    /// A Python-feature type (upstream `App::FeaturePython` and its Part
    /// subclasses) has no Rust-side rebuild: its `execute` only records a request
    /// for the embedding interpreter (see `Document::mark_python_pending`), and
    /// the binding runs `proxy.execute(obj)` after the document walk releases the
    /// lock. Storage code uses this flag to tell a scripted type from a plain one.
    fn is_python_feature(&self) -> bool {
        false
    }
}

/// The process-wide object-type registry: a type id maps to its behaviour.
///
/// This is kernel state, reached through [`crate::kernel()`] as well as the free
/// functions in this module. A workbench crate must not introduce its own
/// `OnceLock<…Registry>`: it would be a second registry, invisible to the kernel
/// and to every other module.
pub type ObjectTypeRegistry = Mutex<BTreeMap<String, Arc<dyn ObjectType>>>;

/// The one process-wide object-type registry.
pub(crate) fn registry() -> &'static ObjectTypeRegistry {
    static REGISTRY: OnceLock<ObjectTypeRegistry> = OnceLock::new();
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

/// Whether the process has an embedding Python interpreter available to run
/// proxy `execute` calls.
///
/// The binding sets this when its module is initialised. Without it a
/// Python-feature `execute` cannot run, so it leaves the object invalid instead
/// of pretending the rebuild succeeded.
pub fn python_executor_available() -> bool {
    PYTHON_EXECUTOR_AVAILABLE.load(Ordering::Relaxed)
}

/// Record whether an embedding interpreter is available (see
/// [`python_executor_available`]). Called by the binding once, at module init.
pub fn set_python_executor_available(available: bool) {
    PYTHON_EXECUTOR_AVAILABLE.store(available, Ordering::Relaxed);
}

static PYTHON_EXECUTOR_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether `type_id` has a registered Python-feature behaviour.
pub fn is_python_feature(type_id: &str) -> bool {
    get(type_id).is_some_and(|behavior| behavior.is_python_feature())
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

    struct Guard;
    impl ObjectType for Guard {
        fn on_property_change(
            &self,
            _doc: &Document,
            _id: ObjectId,
            name: &str,
            _value: &Property,
        ) -> Result<(), PropertyChangeError> {
            if name == "Locked" {
                Err(PropertyChangeError::attribute("Locked is immutable"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn the_property_change_hook_vetoes_before_commit() {
        register("Test::Guard", Arc::new(Guard));
        let mut doc = Document::new();
        let id = doc.add_object("guard", "Test::Guard");
        doc.set_property(id, "Count", Property::Integer(1)).unwrap();

        let error = doc
            .set_property(id, "Locked", Property::Integer(2))
            .unwrap_err();
        // The marker is how the Python binding selects `AttributeError`.
        assert!(error.starts_with(ATTRIBUTE_REJECTION_PREFIX), "{error}");
        assert!(error.contains("Locked is immutable"), "{error}");
        // The veto happened before the write.
        assert_eq!(doc.object(id).unwrap().properties.get("Locked"), None);
        assert_eq!(
            doc.object(id).unwrap().properties.get("Count"),
            Some(&Property::Integer(1))
        );
    }

    #[test]
    fn a_value_rejection_has_no_attribute_marker() {
        let error = PropertyChangeError::value("bad value").to_string();
        assert_eq!(error, "bad value");
        assert!(!error.starts_with(ATTRIBUTE_REJECTION_PREFIX));
    }

    struct PythonFeature;
    impl ObjectType for PythonFeature {
        fn execute(&self, doc: &mut Document, id: ObjectId) {
            doc.mark_python_pending(id);
        }
        fn is_python_feature(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_python_feature_requests_execution_after_the_recompute() {
        register("Test::PythonFeature", Arc::new(PythonFeature));
        assert!(is_python_feature("Test::PythonFeature"));
        assert!(!is_python_feature("Test::Counter2"));

        let mut doc = Document::new();
        let id = doc.add_object("python", "Test::PythonFeature");
        doc.touch(id, false);
        let _ = doc.recompute();
        // Core ran the type's `execute` (which only records the request) and the
        // binding drains it after the lock is released.
        assert!(doc.python_pending(id));
        assert_eq!(doc.take_python_pending(), vec![id]);
        assert!(!doc.python_pending(id));
        assert!(doc.take_python_pending().is_empty());
    }

    #[test]
    fn the_interpreter_availability_flag_is_process_wide() {
        set_python_executor_available(true);
        assert!(python_executor_available());
        set_python_executor_available(false);
        assert!(!python_executor_available());
    }
}
