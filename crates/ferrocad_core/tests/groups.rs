//! Group-container types: membership predicates and the `PartDesign::Body`
//! property surface.
//!
//! `App::Part` and `PartDesign::Body` are geo-feature groups upstream (both carry
//! `App::OriginGroupExtension`), so they are `getParent()`/`getParentGeoFeatureGroup()`
//! parents and own a `Group` link list. A plain `App::DocumentObjectGroup` is a
//! group but not a geo one.

use ferrocad_core::prop_status;
use ferrocad_core::Property;
use ferrocad_core::{Document, Placement};

#[test]
fn geo_group_types_are_groups_but_plain_groups_are_not_geo() {
    let mut doc = Document::new();
    let part = doc.add_object("Part", "App::Part");
    let body = doc.add_object("Body", "PartDesign::Body");
    let plain = doc.add_object("Group", "App::DocumentObjectGroup");
    let feature = doc.add_object("Feature", "App::FeatureTest");

    assert!(doc.is_group_like(part));
    assert!(doc.is_group_like(body));
    assert!(doc.is_group_like(plain));
    assert!(!doc.is_group_like(feature));

    assert!(doc.is_geo_group_like(part));
    assert!(doc.is_geo_group_like(body));
    assert!(!doc.is_geo_group_like(plain));
    assert!(!doc.is_geo_group_like(feature));

    assert!(Document::is_geo_group_type("App::Part"));
    assert!(Document::is_geo_group_type("PartDesign::Body"));
    assert!(!Document::is_geo_group_type("App::DocumentObjectGroup"));
}

#[test]
fn part_design_body_has_the_container_properties() {
    let mut doc = Document::new();
    let body = doc.add_object("Body", "PartDesign::Body");
    let properties = doc.object(body).unwrap();

    // The upstream `Group` membership list starts empty.
    assert_eq!(
        properties.properties.get("Group"),
        Some(&Property::LinkList(vec![]))
    );
    // `Tip`/`BaseFeature`/`Origin` are empty links; `Origin` is hidden.
    assert_eq!(
        properties.properties.get("Tip"),
        Some(&Property::Link("".to_string()))
    );
    assert_eq!(
        properties.properties.get("BaseFeature"),
        Some(&Property::Link("".to_string()))
    );
    assert_eq!(
        properties.properties.get("Origin"),
        Some(&Property::Link("".to_string()))
    );
    assert_eq!(
        properties.properties.status("Origin"),
        Some(prop_status::HIDDEN)
    );
    // `Body::Body()` adds `AllowCompound` (default true) on top of `Part::Feature`'s
    // placement.
    assert_eq!(
        properties.properties.get("AllowCompound"),
        Some(&Property::Bool(true))
    );
    assert_eq!(
        properties.properties.get("Placement"),
        Some(&Property::Placement(Placement::identity()))
    );
}

#[test]
fn membership_is_the_group_link_list() {
    let mut doc = Document::new();
    let body = doc.add_object("Body", "PartDesign::Body");
    let feature = doc.add_object("Feature", "App::FeatureTest");
    doc.set_property(body, "Group", Property::LinkList(vec!["Feature".into()]))
        .expect("Group accepts a link list");

    match doc.object(body).unwrap().properties.get("Group") {
        Some(Property::LinkList(links)) => assert_eq!(links, &["Feature".to_string()]),
        other => panic!("expected LinkList, got {other:?}"),
    }
    // Deleting the member drops it from the group's membership (and therefore
    // from any derived parent relationship).
    doc.remove_object(feature);
    match doc.object(body).unwrap().properties.get("Group") {
        Some(Property::LinkList(links)) => assert!(links.is_empty()),
        other => panic!("expected empty LinkList, got {other:?}"),
    }
}
