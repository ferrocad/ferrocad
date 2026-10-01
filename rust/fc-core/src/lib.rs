//! # fc-core
//!
//! The pure-Rust core of the FreeCAD-on-Rust rewrite (M2): typed quantities,
//! a property container, and a document object model with a dependency-graph
//! recompute order. No Python and no UI — this is what `fc-python` will bind.

mod document;
mod property;
mod quantity;

pub use document::{Document, DocumentObject, ObjectId};
pub use property::{Property, PropertyContainer};
pub use quantity::{Quantity, Unit};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_parses_and_converts() {
        let q: Quantity = "10 mm".parse().unwrap();
        assert_eq!(q.value_mm(), 10.0);

        let q: Quantity = "2.5 in".parse().unwrap();
        assert!((q.value_mm() - 63.5).abs() < 1e-9);

        let q: Quantity = "1 m".parse().unwrap();
        assert_eq!(q.value_mm(), 1000.0);

        assert!("10 bogus".parse::<Quantity>().is_err());
    }

    #[test]
    fn property_container_get_set() {
        let mut pc = PropertyContainer::new();
        pc.set("Label", Property::String("Box".into()));
        pc.set("Width", Property::Quantity("10 mm".parse().unwrap()));
        pc.set("Visible", Property::Bool(true));

        assert_eq!(pc.len(), 3);
        assert_eq!(pc.get("Label"), Some(&Property::String("Box".into())));
        assert_eq!(pc.get("Missing"), None);
    }

    #[test]
    fn recompute_is_dependency_ordered() {
        let mut doc = Document::new();
        let a = doc.add_object("Width", "App::Feature");
        let b = doc.add_object("Height", "App::Feature");
        let c = doc.add_object("Area", "App::Feature");

        doc.add_dependency(c, a);
        doc.add_dependency(c, b);

        let order = doc.recompute_order().expect("acyclic");
        let pos = |id| order.iter().position(|&x| x == id).unwrap();
        assert!(pos(c) > pos(a));
        assert!(pos(c) > pos(b));
    }

    #[test]
    fn recompute_detects_cycles() {
        let mut doc = Document::new();
        let a = doc.add_object("A", "App::Feature");
        let b = doc.add_object("B", "App::Feature");
        doc.add_dependency(a, b);
        doc.add_dependency(b, a); // cycle

        assert!(doc.recompute_order().is_err());
    }
}
