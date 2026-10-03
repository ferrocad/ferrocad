//! # FerroCAD core (`ferrocad_core`)
//!
//! The pure-Rust document model behind the `FreeCAD` Python package: typed
//! properties, a document object model with a dependency graph, transactions with
//! undo/redo, observers, expressions, and FreeCAD's geometry and unit types. No
//! Python and no UI.
//!
//! This crate is the *engine*. [`ferrocad_py`](https://crates.io/crates/ferrocad_py)
//! binds it to Python, and the pure-Python `FreeCAD` facade (the `ferrocad`
//! distribution) presents it as the drop-in `import FreeCAD` package.
//!
//! The documentation is organised following [Diátaxis](https://diataxis.fr):
//! a tutorial to get started, how-to guides for common tasks, an explanation of
//! the design, and a reference (the item pages).
//!
//! # Tutorial
//!
//! Build a document, wire a dependency with an expression, and recompute it.
//!
//! ```
//! use ferrocad_core::{Document, Property};
//!
//! let mut doc = Document::new();
//! let width = doc.add_object("Width", "App::Feature");
//! let area = doc.add_object("Area", "App::Feature");
//!
//! // A value on one object…
//! doc.set_property(width, "Value", Property::Float(10.0)).unwrap();
//! // …an expression on another (dependencies are tracked automatically)…
//! doc.set_expression(area, "Result", "Width.Value * 2").unwrap();
//!
//! // …and a recompute in dependency order.
//! assert_eq!(doc.recompute().unwrap(), 1);
//! assert_eq!(
//!     doc.object(area).unwrap().properties.get("Result"),
//!     Some(&Property::Float(20.0)),
//! );
//! ```
//!
//! # How-to guides
//!
//! - **Add a typed property with flags**: [`Document::add_property`] takes a value
//!   and a `PropertyType` bitmask (see [`prop_status`]); unlike [`Document::set_property`]
//!   it does not touch the object. Setting a property touches it unless it is
//!   `Output`/`NoRecompute`.
//! - **Undo a change**: wrap edits in [`Document::open_transaction`] …
//!   [`Document::commit_transaction`], then [`Document::undo`] / [`Document::redo`].
//! - **Persist a document**: [`Document::save_to_file`] / [`Document::load_from_file`]
//!   (JSON). Properties flagged `NoPersist` are dropped from the saved form.
//! - **React to changes**: register an [`Observer`] with [`Document::add_observer`].
//! - **Name objects**: [`Document::add_object_with`] applies FreeCAD's rules — names
//!   are sanitized ([`sanitize_name`]) and made unique, labels are unique unless
//!   duplicates are allowed.
//!
//! # Explanation
//!
//! A [`Document`] owns a set of [`DocumentObject`]s and a dependency graph over
//! them. Each object is a [`PropertyContainer`] — a name → value map where every
//! property also carries a status bitmask ([`prop_status`]). Recompute walks the
//! graph in dependency order and evaluates each object's expressions.
//!
//! Editing goes through transactions: [`Document::set_property`] records a
//! reversible change so a whole edit can be undone or redone. Assigning a property
//! *touches* the object (sets its `must_execute` flag) so a later recompute knows
//! what is dirty; `Output`/`NoRecompute` properties opt out.
//!
//! The value types mirror FreeCAD's `Base` module: [`Quantity`]/[`Unit`] for values
//! with units, and [`Vector3`]/[`Matrix4`]/[`Rotation`]/[`Placement`] for geometry.
//! Names are FreeCAD-compatible: internal names are sanitized and made unique,
//! display labels are a separate, user-facing string.
//!
//! # Implemented capabilities
//!
//! The crate grew milestone by milestone (see the repository `docs/milestones.md`):
//!
//! - **M2** — the core model: [`Property`]/[`PropertyContainer`], [`Quantity`]/[`Unit`],
//!   the `petgraph` recompute DAG, transaction-backed undo/redo, [`Observer`]s,
//!   and the expression parser/evaluator.
//! - **M4 (slices 1–16)** — the `Base`/`App` surface bound to Python: geometry helpers,
//!   persistence (`SavedDocument`), document metadata, extensions/groups, links,
//!   observers that fire, Python-object/`Proxy` persistence, expressions, and
//!   matrix decomposition / rotation numerics.
//! - **MVP slice A1** — object state and per-property status flags; touch-on-assign.
//! - **MVP slice B1** — name/label semantics ([`sanitize_name`], [`Document::unique_name`]).
//! - **MVP slice A2** — [`Property::Enumeration`] and type-registry validation.
//! - **MVP slice B2** — a general reversible undo/redo change set (property edits,
//!   object add/remove, expressions) with named transactions and an active-object
//!   pointer ([`Document::undo_names`], [`Document::active_object`]).
//!
//! # Reference
//!
//! The item pages (`Document`, `DocumentObject`, `Property`, the geometry and unit
//! types, and the supporting modules) are the reference. The `mod` items below are
//! the module-level documentation.

mod document;
mod expr;
mod geometry;
mod observer;
mod property;
mod quantity;
mod stringhasher;
mod transaction;
mod typeregistry;
mod unit;

pub use document::{sanitize_name, Document, DocumentObject, ObjectId, SavedDocument};
pub use geometry::{Matrix4, Placement, Rotation, ScaleType, TypeId, Vector3};
pub use observer::Observer;
pub use property::{prop_status, status_from_name, status_names, Property, PropertyContainer};
pub use quantity::{canonical_name, parse_unit, Quantity};
pub use stringhasher::{StringHasher, StringId};
pub use unit::Unit;

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
    fn property_status_and_touch() {
        let mut doc = Document::new();
        let o = doc.add_object("Obj", "App::FeaturePython");
        doc.add_property(o, "Plain", Property::String(String::new()), prop_status::NONE)
            .unwrap();
        doc.add_property(o, "Out", Property::String(String::new()), prop_status::OUTPUT)
            .unwrap();

        // `add_property` itself does not touch the object.
        assert!(!doc.object(o).unwrap().must_execute);

        // Setting a plain property touches it; `purge_touched` clears the flag.
        doc.set_property(o, "Plain", Property::String("x".into())).unwrap();
        assert!(doc.object(o).unwrap().must_execute);
        doc.purge_touched(o);
        assert!(!doc.object(o).unwrap().must_execute);

        // Setting an output property does not touch it.
        doc.set_property(o, "Out", Property::String("y".into())).unwrap();
        assert!(!doc.object(o).unwrap().must_execute);

        assert_eq!(doc.property_status(o, "Out"), Some(prop_status::OUTPUT));
        assert!(doc.set_property_status(o, "Out", prop_status::HIDDEN));
        assert_eq!(doc.property_status(o, "Out"), Some(prop_status::HIDDEN));
        assert!(!doc.set_property_status(o, "Missing", prop_status::NONE));
    }

    #[test]
    fn no_persist_properties_are_dropped_but_dynamic_transients_saved() {
        let mut doc = Document::new();
        let o = doc.add_object("Obj", "App::FeaturePython");
        doc.add_property(o, "Kept", Property::String("v".into()), prop_status::NONE)
            .unwrap();
        doc.add_property(o, "Transient", Property::String("t".into()), prop_status::TRANSIENT)
            .unwrap();
        doc.add_property(o, "NoPersist", Property::String("n".into()), prop_status::NOPERSIST)
            .unwrap();

        let saved = doc.to_saved("Doc");
        let obj = &saved.objects[0];
        assert!(obj.properties.contains_key("Kept"));
        // A dynamically added transient property is still persisted (FreeCAD
        // only drops transient *static* properties).
        assert!(obj.properties.contains_key("Transient"));
        assert!(!obj.properties.contains_key("NoPersist"));

        let restored = Document::from_saved(&saved);
        let id = restored.get_by_name("Obj").unwrap();
        let obj = restored.object(id).unwrap();
        assert!(obj.properties.get("NoPersist").is_none());
        assert_eq!(obj.properties.status("Kept"), Some(prop_status::NONE));
    }

    #[test]
    fn name_sanitization_and_uniqueness() {
        assert_eq!(sanitize_name("My Label"), "My_Label");
        assert_eq!(sanitize_name("abc\u{1}ef"), "abc_ef");
        assert_eq!(sanitize_name("5x"), "_5x");
        assert_eq!(sanitize_name(""), "_");

        let mut doc = Document::new();
        let a = doc.add_object("Label", "App::FeatureTest");
        let b = doc.add_object("Label", "App::FeatureTest");
        assert_eq!(doc.object(a).unwrap().name, "Label");
        assert_eq!(doc.object(a).unwrap().label, "Label");
        assert_eq!(doc.object(b).unwrap().name, "Label001");
        assert_eq!(doc.object(b).unwrap().label, "Label001");

        // Duplicate labels allowed: the requested label is kept verbatim.
        let c = doc.add_object_with("Label", "App::FeatureTest", true);
        assert_eq!(doc.object(c).unwrap().name, "Label002");
        assert_eq!(doc.object(c).unwrap().label, "Label");

        // Unique-label helper skips taken labels.
        assert_eq!(doc.unique_label("Label"), "Label002");
    }

    #[test]
    fn enumeration_property_roundtrips() {
        let p = Property::Enumeration(vec!["a".to_string(), "b".to_string()], 1);
        assert_eq!(p.type_name(), "App::PropertyEnumeration");
        let json = serde_json::to_string(&p).unwrap();
        let back: Property = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn status_name_mapping() {
        assert_eq!(status_names(prop_status::NONE), Vec::<&str>::new());
        assert_eq!(status_names(prop_status::OUTPUT), vec!["Output"]);
        assert_eq!(
            status_names(prop_status::TRANSIENT | prop_status::NOPERSIST),
            vec!["NoPersist", "Transient"]
        );
        assert_eq!(status_from_name("readonly"), Some(prop_status::READONLY));
        assert_eq!(status_from_name("bogus"), None);
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
    fn extensions_and_groups() {
        let mut doc = Document::new();
        let grp = doc.add_object("Group", "App::DocumentObjectGroup");
        let obj = doc.add_object("Obj", "App::DocumentObject");
        assert!(doc.is_group_like(grp));
        assert!(!doc.has_extension(obj, "App::GroupExtension"));

        doc.add_extension(obj, "App::GroupExtensionPython");
        assert!(doc.has_extension(obj, "App::GroupExtension"));
        assert!(doc.has_extension(obj, "App::GroupExtensionPython"));
        assert!(doc.is_group_like(obj));
    }

    #[test]
    fn removing_object_drops_it_from_groups() {
        let mut doc = Document::new();
        let grp = doc.add_object("Group", "App::DocumentObjectGroup");
        let obj = doc.add_object("Obj", "App::DocumentObject");
        doc.set_property(grp, "Group", Property::LinkList(vec!["Obj".into(), "Obj".into()]))
            .unwrap();

        doc.remove_object(obj);
        match doc.object(grp).unwrap().properties.get("Group") {
            Some(Property::LinkList(links)) => assert!(links.is_empty()),
            other => panic!("expected empty LinkList, got {other:?}"),
        }
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

    #[test]
    fn undo_names_track_active_and_committed_transactions() {
        let mut doc = Document::new();
        let a = doc.add_object("A", "App::Feature");
        doc.set_property(a, "Width", Property::Float(1.0)).unwrap();

        doc.open_transaction_named("T1");
        doc.set_property(a, "Width", Property::Float(2.0)).unwrap();
        // The active transaction is visible immediately as the newest entry.
        assert_eq!(doc.undo_names(), vec!["T1"]);

        // Opening a second transaction creates no entry yet…
        doc.open_transaction_named("T2");
        assert_eq!(doc.undo_names(), vec!["T1"]);
        // …its first change commits T1 and makes T2 the active transaction.
        doc.set_property(a, "Width", Property::Float(3.0)).unwrap();
        assert_eq!(doc.undo_names(), vec!["T2", "T1"]);

        // Undo commits the active transaction, then reverts it.
        assert!(doc.undo());
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(2.0))
        );
        assert_eq!(doc.undo_names(), vec!["T1"]);
        assert_eq!(doc.redo_names(), vec!["T2"]);

        assert!(doc.redo());
        assert_eq!(
            doc.object(a).unwrap().properties.get("Width"),
            Some(&Property::Float(3.0))
        );
    }

    #[test]
    fn undo_add_object_clears_active_and_redo_restores_the_same_id() {
        let mut doc = Document::new();
        doc.open_transaction_named("Add");
        let id = doc.add_object("Obj", "App::Feature");
        assert_eq!(doc.active_object(), Some(id));
        doc.commit_transaction();
        assert_eq!(doc.undo_names(), vec!["Add"]);

        assert!(doc.undo());
        assert!(doc.object(id).is_none());
        assert_eq!(doc.get_by_name("Obj"), None);
        assert_eq!(doc.active_object(), None);

        assert!(doc.redo());
        assert!(doc.object(id).is_some());
        assert_eq!(doc.get_by_name("Obj"), Some(id));
    }

    #[test]
    fn undo_remove_restores_group_membership() {
        let mut doc = Document::new();
        let grp = doc.add_object("Group", "App::DocumentObjectGroup");
        let obj = doc.add_object("Obj", "App::DocumentObject");
        doc.set_property(grp, "Group", Property::LinkList(vec!["Obj".into()]))
            .unwrap();

        doc.open_transaction_named("Remove");
        assert!(doc.remove_object(obj));
        match doc.object(grp).unwrap().properties.get("Group") {
            Some(Property::LinkList(links)) => assert!(links.is_empty()),
            other => panic!("expected empty LinkList, got {other:?}"),
        }
        doc.commit_transaction();

        assert!(doc.undo());
        assert_eq!(doc.get_by_name("Obj"), Some(obj));
        match doc.object(grp).unwrap().properties.get("Group") {
            Some(Property::LinkList(links)) => assert_eq!(links, &vec!["Obj".to_string()]),
            other => panic!("expected restored LinkList, got {other:?}"),
        }
    }

    #[test]
    fn undo_expression_set_and_remove() {
        let mut doc = Document::new();
        let a = doc.add_object("A", "App::Feature");
        doc.set_property(a, "Width", Property::Float(2.0)).unwrap();

        doc.open_transaction_named("Expr");
        doc.set_expression(a, "Result", "Width * 3").unwrap();
        doc.commit_transaction();
        assert_eq!(doc.recompute().unwrap(), 1);
        assert_eq!(
            doc.object(a).unwrap().properties.get("Result"),
            Some(&Property::Float(6.0))
        );

        assert!(doc.undo());
        assert!(doc.object(a).unwrap().expressions.is_empty());

        assert!(doc.redo());
        assert_eq!(
            doc.object(a).unwrap().expressions.get("Result").map(String::as_str),
            Some("Width * 3")
        );
        assert_eq!(doc.recompute().unwrap(), 1);
        assert_eq!(
            doc.object(a).unwrap().properties.get("Result"),
            Some(&Property::Float(6.0))
        );
    }
}
