//! # fc-core
//!
//! The pure-Rust core of the FreeCAD-on-Rust rewrite (M2): typed quantities,
//! a property container, and a document object model with a dependency-graph
//! recompute order, transactions (open/commit/abort + undo/redo), observers,
//! and expression-driven recompute. No Python and no UI — this is what
//! `fc-python` will bind.

mod document;
mod expr;
mod observer;
mod property;
mod quantity;
mod stringhasher;
mod transaction;

pub use document::{Document, DocumentObject, ObjectId};
pub use observer::Observer;
pub use property::{Property, PropertyContainer};
pub use quantity::{Quantity, Unit};
pub use stringhasher::{StringHasher, StringId};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

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

    #[test]
    fn transactions_abort_undo_redo() {
        let mut doc = Document::new();
        let a = doc.add_object("A", "App::Feature");
        doc.set_property(a, "Width", Property::Float(1.0)).unwrap();

        // abort reverts
        doc.open_transaction();
        doc.set_property(a, "Width", Property::Float(2.0)).unwrap();
        doc.set_property(a, "Height", Property::Float(3.0)).unwrap();
        doc.abort_transaction();
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(1.0))
        );
        assert_eq!(doc.object(a).unwrap().properties.get("Height"), None);

        // commit keeps, undo/redo round-trips
        doc.open_transaction();
        doc.set_property(a, "Width", Property::Float(5.0)).unwrap();
        doc.commit_transaction();
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(5.0))
        );

        assert!(doc.undo());
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(1.0))
        );

        assert!(doc.redo());
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(5.0))
        );
    }

    struct RecordingObserver {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Observer for RecordingObserver {
        fn on_object_added(&mut self, object: ObjectId) {
            self.log.lock().unwrap().push(format!("added:{object}"));
        }

        fn on_property_changed(
            &mut self,
            object: ObjectId,
            name: &str,
            _old: Option<&Property>,
            new: &Property,
        ) {
            self.log
                .lock()
                .unwrap()
                .push(format!("changed:{object}:{name}:{new:?}"));
        }
    }

    #[test]
    fn observers_fire_on_changes() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut doc = Document::new();
        doc.add_observer(Box::new(RecordingObserver { log: log.clone() }));

        let a = doc.add_object("A", "App::Feature");
        doc.set_property(a, "Width", Property::Float(10.0)).unwrap();

        let log = log.lock().unwrap();
        assert!(log.iter().any(|e| e == "added:0"));
        assert!(log.iter().any(|e| e.starts_with("changed:0:Width:")));
    }

    #[test]
    fn expressions_recompute() {
        let mut doc = Document::new();
        let a = doc.add_object("A", "App::Feature");
        let b = doc.add_object("B", "App::Feature");
        let c = doc.add_object("C", "App::Feature");

        doc.set_property(a, "Width", Property::Float(10.0)).unwrap();
        doc.set_property(b, "Height", Property::Float(5.0)).unwrap();
        doc.set_expression(c, "Area", "A.Width * B.Height").unwrap();

        assert_eq!(doc.recompute().unwrap(), 1);
        assert_eq!(
            doc.object(c).unwrap().properties.get("Area"),
            Some(&Property::Float(50.0))
        );
    }
}
