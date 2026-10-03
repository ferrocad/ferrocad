//! The document object model: names/labels, the dependency graph, extensions,
//! groups, expressions and observers.

use crate::{sanitize_name, Document, ObjectId, Observer, Property};
use std::sync::{Arc, Mutex};

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
