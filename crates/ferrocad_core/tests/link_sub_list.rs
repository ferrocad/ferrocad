//! `App::PropertyLinkSubList`: the model, its type id, and native JSON persistence.
//!
//! The value is a list of `(object name, sub-element names)` groups. Unlike the
//! older flattening to `LinkList`, the per-link subnames now survive a save/load
//! round trip.

use ferrocad_core::{native, Document, Property};

#[test]
fn the_type_is_a_real_link_sub_list() {
    assert_eq!(
        ferrocad_core::property_types::default_for("App::PropertyLinkSubList"),
        Some(Property::LinkSubList(vec![]))
    );
    assert_eq!(
        Property::LinkSubList(vec![]).type_name(),
        "App::PropertyLinkSubList"
    );
    assert_eq!(
        Property::LinkSubList(vec![("A".into(), vec!["Face1".into()])]),
        Property::LinkSubList(vec![("A".into(), vec!["Face1".into()])])
    );
    assert_ne!(
        Property::LinkSubList(vec![("A".into(), vec![])]),
        Property::LinkSubList(vec![("A".into(), vec!["Face1".into()])])
    );
}

#[test]
fn native_json_round_trips_groups_and_subnames() {
    let mut doc = Document::new();
    let facebinder = doc.add_object("Faces", "App::FeaturePython");
    let _target = doc.add_object("Target", "App::FeaturePython");
    let value = Property::LinkSubList(vec![
        ("Target".to_string(), vec!["Face1".to_string(), "Face2".to_string()]),
        ("Faces".to_string(), vec![String::new()]),
    ]);

    doc.set_property(facebinder, "Faces", value.clone())
        .expect("LinkSubList accepts grouped subnames");

    let saved = doc.to_saved("LinkSubList");
    let bytes = native::encode(&saved).expect("encode");
    let restored = native::decode("link-sub-list.ferrocad", &bytes).expect("decode");
    let object = restored
        .objects
        .iter()
        .find(|o| o.name == "Faces")
        .expect("Faces persists");
    assert_eq!(object.properties.get("Faces"), Some(&value));

    // The target must still be present for the names to be meaningful.
    assert!(restored.objects.iter().any(|o| o.name == "Target"));
}
