//! The [`crate::kernel()`] facade and the process-wide registries behind it.
//!
//! These assert that the facade is *the same state* the free functions use, not a
//! second registry: a document created through `application()` and an object type
//! registered through `object_registry` are both visible through `kernel()`.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use crate::extension::{ExtensionData, ExtensionValue};
use crate::object_registry::{self, ObjectType};
use crate::property_types;
use crate::{application, kernel, Property};

struct Probe;

impl ObjectType for Probe {}

struct Counter(i64);

impl ExtensionData for Counter {
    fn type_name(&self) -> &str {
        "KernelFacade::PropertyCounter"
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
            .is_some_and(|counter| counter.0 == self.0)
    }
    fn save(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Counter({})", self.0)
    }
}

#[test]
fn documents_created_through_application_are_visible_through_the_kernel() {
    let (name, handle) = application()
        .lock()
        .unwrap()
        .new_document(Some("KernelFacadeDocuments"));

    let registry = kernel().documents().lock().unwrap();
    assert!(registry.contains(&name));
    let visible = registry.get(&name).expect("kernel sees the document");
    assert!(Arc::ptr_eq(&handle, &visible));
    drop(registry);

    assert!(kernel().documents().lock().unwrap().close_document(&name));
}

#[test]
fn object_types_registered_through_the_registry_are_visible_through_the_kernel() {
    object_registry::register("KernelFacade::Probe", Arc::new(Probe));

    let registry = kernel().object_types().lock().unwrap();
    assert!(registry.contains_key("KernelFacade::Probe"));
}

#[test]
fn property_registries_are_visible_through_the_kernel() {
    property_types::register("KernelFacade::PropertyInteger", || Property::Integer(7));
    assert_eq!(
        kernel()
            .properties()
            .lock()
            .unwrap()
            .get("KernelFacade::PropertyInteger")
            .map(|factory| factory()),
        Some(Property::Integer(7))
    );

    property_types::register_extension(
        "KernelFacade::PropertyCounter",
        || Property::Extension(ExtensionValue::new(Box::new(Counter(0)))),
        |_bytes| None,
    );
    assert!(kernel()
        .property_extensions()
        .lock()
        .unwrap()
        .contains_key("KernelFacade::PropertyCounter"));
}
