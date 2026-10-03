//! Property containers, status flags, and persistence filtering.

use crate::{prop_status, status_from_name, status_names, Document, Property, PropertyContainer};

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
