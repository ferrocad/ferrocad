//! `Part::PropertyPartShape`: a kernel shape held as a [`Property::Extension`].
//!
//! The value wraps a [`ferrocad_geom::Shape`] directly — no lock. A document is shared
//! behind an `Arc<Mutex<…>>`, so its contents need only be `Send`; every access to a
//! property already runs under that document lock. Its `save`/`restore` go through the
//! **injected** backend's in-memory encoding, so a shape persists through the ordinary
//! document JSON without Part naming a kernel or core knowing anything about it.

use std::any::Any;
use std::fmt;

use ferrocad_core::property_types;
use ferrocad_core::{ExtensionData, ExtensionValue, Property};
use ferrocad_geom::Shape;

/// The registered property type name.
pub const TYPE_NAME: &str = "Part::PropertyPartShape";

/// A shape-valued property.
pub struct ShapeProperty {
    shape: Option<Shape>,
}

impl ShapeProperty {
    /// An empty (unset) shape — the default value.
    pub fn empty() -> Self {
        ShapeProperty { shape: None }
    }

    /// A property holding `shape`.
    pub fn new(shape: Shape) -> Self {
        ShapeProperty { shape: Some(shape) }
    }

    /// A copy of the held shape (shape handles are cheap to clone), if any.
    pub fn shape(&self) -> Option<Shape> {
        self.shape.clone()
    }

    /// Replace the held shape.
    pub fn set_shape(&mut self, shape: Shape) {
        self.shape = Some(shape);
    }

    /// Clear the held shape.
    pub fn clear(&mut self) {
        self.shape = None;
    }
}

impl ExtensionData for ShapeProperty {
    fn type_name(&self) -> &str {
        TYPE_NAME
    }

    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(ShapeProperty {
            shape: self.shape.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq(&self, other: &dyn ExtensionData) -> bool {
        let Some(other) = other.as_any().downcast_ref::<ShapeProperty>() else {
            return false;
        };
        match (&self.shape, &other.shape) {
            (None, None) => true,
            (Some(x), Some(y)) => x.is_same(y),
            _ => false,
        }
    }

    fn save(&self) -> Vec<u8> {
        match self.shape.as_ref() {
            Some(shape) => crate::backend().save_shape(shape),
            None => Vec::new(),
        }
    }

    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.shape.as_ref() {
            Some(shape) => write!(f, "ShapeProperty({shape:?})"),
            None => f.write_str("ShapeProperty(empty)"),
        }
    }
}

/// Wrap a [`ShapeProperty`] as a `Property::Extension` value.
pub fn make_shape_property(property: ShapeProperty) -> Property {
    Property::Extension(ExtensionValue::new(Box::new(property)))
}

/// Borrow the [`ShapeProperty`] inside `property`, if it is one.
pub fn shape_of(property: &Property) -> Option<&ShapeProperty> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<ShapeProperty>(),
        _ => None,
    }
}

/// Register `Part::PropertyPartShape`. Idempotent.
pub fn register() {
    property_types::register_extension(
        TYPE_NAME,
        || make_shape_property(ShapeProperty::empty()),
        restore,
    );
}

fn restore(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    if bytes.is_empty() {
        return Some(Box::new(ShapeProperty::empty()));
    }
    let shape = crate::backend().load_shape(bytes)?;
    Some(Box::new(ShapeProperty::new(shape)))
}
