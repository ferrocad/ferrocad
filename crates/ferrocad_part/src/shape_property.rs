//! `Part::PropertyPartShape`: a kernel shape held as a [`Property::Extension`].
//!
//! The value wraps a [`ferrocad_geom::Shape`] in a `Mutex` so it is `Sync` (OCCT's
//! `TopoDS_Shape` is `Send` but not `Sync`) and gives kernel calls exclusive access.
//! Its `save`/`restore` use the OCCT backend's in-memory BREP read/write, so a shape
//! persists through the ordinary document JSON without core knowing anything about it.

use std::any::Any;
use std::fmt;
use std::sync::Mutex;

use ferrocad_core::property_types;
use ferrocad_core::{ExtensionData, ExtensionValue, Property};
use ferrocad_geom::Shape;

/// The registered property type name.
pub const TYPE_NAME: &str = "Part::PropertyPartShape";

/// A shape-valued property.
pub struct ShapeProperty {
    shape: Mutex<Option<Shape>>,
}

impl ShapeProperty {
    /// An empty (unset) shape — the default value.
    pub fn empty() -> Self {
        ShapeProperty {
            shape: Mutex::new(None),
        }
    }

    /// A property holding `shape`.
    pub fn new(shape: Shape) -> Self {
        ShapeProperty {
            shape: Mutex::new(Some(shape)),
        }
    }

    /// A copy of the held shape (shape handles are cheap to clone), if any.
    pub fn shape(&self) -> Option<Shape> {
        self.shape.lock().unwrap().clone()
    }

    /// Replace the held shape.
    pub fn set_shape(&self, shape: Shape) {
        *self.shape.lock().unwrap() = Some(shape);
    }

    /// Clear the held shape.
    pub fn clear(&self) {
        *self.shape.lock().unwrap() = None;
    }
}

impl ExtensionData for ShapeProperty {
    fn type_name(&self) -> &str {
        TYPE_NAME
    }

    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(ShapeProperty {
            shape: Mutex::new(self.shape.lock().unwrap().clone()),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq(&self, other: &dyn ExtensionData) -> bool {
        let Some(other) = other.as_any().downcast_ref::<ShapeProperty>() else {
            return false;
        };
        // Clone the handles out (releasing each lock immediately) before comparing, so
        // comparing a value with itself cannot deadlock on the non-reentrant `Mutex`.
        let a = self.shape.lock().unwrap().clone();
        let b = other.shape.lock().unwrap().clone();
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => x.is_same(&y),
            _ => false,
        }
    }

    fn save(&self) -> Vec<u8> {
        match self.shape.lock().unwrap().as_ref() {
            Some(shape) => ferrocad_occt::write_brep(shape).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.shape.lock().unwrap().as_ref() {
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
    let shape = ferrocad_occt::read_brep(bytes)?;
    Some(Box::new(ShapeProperty::new(shape)))
}
