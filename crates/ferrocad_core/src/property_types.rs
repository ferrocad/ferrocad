//! Registered property types: the `App::Property` half of FreeCAD's `Base::Type`.
//!
//! FreeCAD property types are named C++ classes registered in a reflective runtime
//! type system; Python addresses them **by name**
//! (`obj.addProperty("App::PropertyLength", "Length", …)`) and enumerates them
//! (`supportedProperties()`). This is the Rust equivalent: a name-keyed registry of
//! factories. Core seeds the `App::*` types; modules register their own
//! (e.g. `Part::PropertyPartShape`). See `docs/property-types.md`.
//!
//! An unknown name is *not* a default value: `default_for` returns `None` so callers
//! can raise, matching upstream (which raises for an unregistered type) rather than
//! silently creating the wrong type.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::geometry::{Matrix4, Placement, Rotation, Vector3};
use crate::property::Property;
use crate::quantity::Quantity;
use crate::unit::Unit;

type Factory = Box<dyn Fn() -> Property + Send + Sync>;

fn registry() -> &'static Mutex<BTreeMap<String, Factory>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, Factory>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map: BTreeMap<String, Factory> = BTreeMap::new();
        seed(&mut map);
        Mutex::new(map)
    })
}

/// Register a property type under its FreeCAD name (`"App::PropertyLength"`, …).
/// Overwrites any existing registration of that name.
pub fn register(name: &str, factory: impl Fn() -> Property + Send + Sync + 'static) {
    registry()
        .lock()
        .unwrap()
        .insert(name.to_string(), Box::new(factory));
}

/// Whether `name` is a registered property type.
pub fn contains(name: &str) -> bool {
    registry().lock().unwrap().contains_key(name)
}

/// A fresh default value for the registered type `name`, or `None` if unregistered.
pub fn default_for(name: &str) -> Option<Property> {
    registry().lock().unwrap().get(name).map(|factory| factory())
}

/// Every registered property type name, sorted (Feeds `supportedProperties()`).
pub fn names() -> Vec<String> {
    registry().lock().unwrap().keys().cloned().collect()
}

fn seed(map: &mut BTreeMap<String, Factory>) {
    fn length() -> Property {
        Property::Quantity(Quantity::new(0.0, Unit::Millimeter))
    }
    fn insert(
        map: &mut BTreeMap<String, Factory>,
        name: &str,
        factory: impl Fn() -> Property + Send + Sync + 'static,
    ) {
        map.insert(name.to_string(), Box::new(factory));
    }

    // Scalars.
    insert(map, "App::PropertyString", || Property::String(String::new()));
    insert(map, "App::PropertyFloat", || Property::Float(0.0));
    insert(map, "App::PropertyBool", || Property::Bool(false));
    insert(map, "App::PropertyInteger", || Property::Integer(0));
    insert(map, "App::PropertyInt", || Property::Integer(0));
    insert(map, "App::PropertyPath", || Property::String(String::new()));
    insert(map, "App::PropertyFont", || Property::String(String::new()));
    insert(map, "App::PropertyMap", || Property::String(String::new()));

    // Values with units (a canonical millimetre quantity, as FreeCAD normalises).
    for name in [
        "App::PropertyLength",
        "App::PropertyDistance",
        "App::PropertyQuantity",
        "App::PropertyAngle",
        "App::PropertyArea",
        "App::PropertyPressure",
        "App::PropertySpeed",
        "App::PropertyPercent",
        "App::PropertyVectorDistance",
    ] {
        insert(map, name, length);
    }

    // Geometry.
    insert(map, "App::PropertyVector", || Property::Vector(Vector3::zero()));
    insert(map, "App::PropertyPlacement", || {
        Property::Placement(Placement::identity())
    });
    insert(map, "App::PropertyMatrix", || Property::Matrix(Matrix4::identity()));
    insert(map, "App::PropertyRotation", || {
        Property::Rotation(Rotation::identity())
    });

    // Links.
    insert(map, "App::PropertyLink", || Property::Link(String::new()));
    insert(map, "App::PropertyLinkGlobal", || Property::Link(String::new()));
    insert(map, "App::PropertyLinkList", || Property::LinkList(Vec::new()));
    insert(map, "App::PropertyLinkListGlobal", || Property::LinkList(Vec::new()));
    insert(map, "App::PropertyLinkListHidden", || Property::LinkList(Vec::new()));
    insert(map, "App::PropertyLinkSub", || {
        Property::LinkSub(String::new(), Vec::new())
    });
    insert(map, "App::PropertyLinkSubHidden", || {
        Property::LinkSub(String::new(), Vec::new())
    });
    insert(map, "App::PropertyLinkSubList", || Property::LinkList(Vec::new()));

    // Lists.
    insert(map, "App::PropertyFloatList", || Property::FloatList(Vec::new()));
    insert(map, "App::PropertyIntegerList", || {
        Property::IntegerList(Vec::new())
    });
    insert(map, "App::PropertyStringList", || Property::StringList(Vec::new()));
    insert(map, "App::PropertyBoolList", || Property::BoolList(Vec::new()));
    insert(map, "App::PropertyVectorList", || Property::VectorList(Vec::new()));
    insert(map, "App::PropertyPlacementList", || {
        Property::PlacementList(Vec::new())
    });
    insert(map, "App::PropertyRotationList", || {
        Property::RotationList(Vec::new())
    });
    insert(map, "App::PropertyIntPairList", || {
        Property::IntPairList(Vec::new())
    });

    // Enumeration, colour, Python, file, constraints.
    insert(map, "App::PropertyEnumeration", || {
        Property::Enumeration(Vec::new(), 0)
    });
    insert(map, "App::PropertyColor", || Property::ColorList(Vec::new()));
    insert(map, "App::PropertyColorList", || Property::ColorList(Vec::new()));
    insert(map, "App::PropertyColourList", || Property::ColorList(Vec::new()));
    insert(map, "App::PropertyPythonObject", || {
        Property::PythonObject(String::new())
    });
    insert(map, "App::PropertyFile", || {
        Property::FileIncluded(String::new())
    });
    insert(map, "App::PropertyFileIncluded", || {
        Property::FileIncluded(String::new())
    });
    insert(map, "App::PropertyIntegerConstraint", || Property::IntegerConstraint {
        value: 0,
        min: 0,
        max: 0,
        step: 1,
    });
    insert(map, "App::PropertyFloatConstraint", || Property::FloatConstraint {
        value: 0.0,
        min: 0.0,
        max: 0.0,
        step: 1.0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types_resolve() {
        assert_eq!(
            default_for("App::PropertyLength"),
            Some(Property::Quantity(Quantity::new(0.0, Unit::Millimeter)))
        );
        assert_eq!(default_for("App::PropertyString"), Some(Property::String(String::new())));
        assert!(contains("App::PropertyPlacement"));
    }

    #[test]
    fn unknown_types_are_none_not_a_default() {
        // The hazard this registry exists to remove: an unknown property type must
        // not silently become a `String`.
        assert_eq!(default_for("Part::PropertyPartShape"), None);
        assert_eq!(default_for("App::DocumentObjectExtension"), None);
    }

    #[test]
    fn modules_can_register_and_names_include_them() {
        register("Test::PropertyWidget", || Property::Integer(7));
        assert_eq!(default_for("Test::PropertyWidget"), Some(Property::Integer(7)));
        assert!(names().contains(&"Test::PropertyWidget".to_string()));
    }
}
