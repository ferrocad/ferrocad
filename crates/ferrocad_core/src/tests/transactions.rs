//! The transaction engine: abort/commit, undo/redo, names, counts, booking ids,
//! and reversible object/expression changes.

use crate::{prop_status, Document, Property};

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
fn transaction_booking_and_available_steps() {
    let mut doc = Document::new();
    let a = doc.add_object("A", "App::Feature");
    doc.set_property(a, "Width", Property::Float(0.0)).unwrap();

    // No transaction booked initially.
    assert_eq!(doc.booked_transaction_id(), 0);
    assert_eq!(doc.available_undos(0), 0);

    // Opening books an id immediately, before any change.
    doc.open_transaction_named("T1");
    let t1 = doc.booked_transaction_id();
    assert_ne!(t1, 0);
    assert_eq!(doc.available_undos(0), 0); // pending only, not yet undoable

    doc.set_property(a, "Width", Property::Float(1.0)).unwrap();
    assert_eq!(doc.booked_transaction_id(), t1);
    assert_eq!(doc.available_undos(0), 1);
    assert_eq!(doc.available_undos(t1), 1);

    // Commit clears the booking but keeps the undo entry.
    doc.commit_transaction();
    assert_eq!(doc.booked_transaction_id(), 0);
    assert_eq!(doc.available_undos(0), 1);
    assert_eq!(doc.available_undos(t1), 1);

    // A second transaction is a distinct id, at depth 1 from the top.
    doc.open_transaction_named("T2");
    let t2 = doc.booked_transaction_id();
    assert_ne!(t2, t1);
    doc.set_property(a, "Width", Property::Float(2.0)).unwrap();
    assert_eq!(doc.available_undos(0), 2);
    assert_eq!(doc.available_undos(t2), 1);
    assert_eq!(doc.available_undos(t1), 2);

    // Undo moves the top transaction to redo.
    assert!(doc.undo());
    assert_eq!(doc.available_undos(0), 1);
    assert_eq!(doc.available_undos(t1), 1);
    assert_eq!(doc.available_redos(0), 1);
    assert_eq!(doc.available_redos(t2), 1);

    // Unknown ids report 0.
    assert_eq!(doc.available_undos(9999), 0);
    assert_eq!(doc.available_redos(9999), 0);
}

#[test]
fn undo_added_property_removes_it_and_redo_restores_its_slot() {
    let mut doc = Document::new();
    let o = doc.add_object("Obj", "App::FeaturePython");
    doc.add_property(o, "Other", Property::Integer(2), prop_status::NONE, "", "")
        .unwrap();

    doc.open_transaction_named("AddProperty");
    doc.add_property(
        o,
        "SC_Support",
        Property::Link("Base".into()),
        prop_status::HIDDEN,
        "SimpleCAD",
        "Solid used by Push/Pull",
    )
    .unwrap();
    doc.commit_transaction();
    assert!(doc.object(o).unwrap().properties.get("SC_Support").is_some());

    assert!(doc.undo());
    assert!(doc.object(o).unwrap().properties.get("SC_Support").is_none());
    // The rest of the property set is untouched.
    assert_eq!(
        doc.object(o).unwrap().properties.get("Other"),
        Some(&Property::Integer(2))
    );

    assert!(doc.redo());
    let obj = doc.object(o).unwrap();
    assert_eq!(
        obj.properties.get("SC_Support"),
        Some(&Property::Link("Base".into()))
    );
    // Status and metadata come back exactly as `addProperty` recorded them.
    assert_eq!(obj.properties.status("SC_Support"), Some(prop_status::HIDDEN));
    assert_eq!(
        obj.properties.raw_status("SC_Support"),
        Some(prop_status::HIDDEN | prop_status::DYNAMIC)
    );
    assert_eq!(obj.properties.group("SC_Support"), Some("SimpleCAD"));
    assert_eq!(
        obj.properties.doc("SC_Support"),
        Some("Solid used by Push/Pull")
    );
}

#[test]
fn undo_removed_property_restores_value_status_and_meta() {
    let mut doc = Document::new();
    let o = doc.add_object("Obj", "App::FeaturePython");
    doc.add_property(
        o,
        "SC_Support",
        Property::Link("Base".into()),
        prop_status::HIDDEN,
        "SimpleCAD",
        "Solid used by Push/Pull",
    )
    .unwrap();

    doc.open_transaction_named("RemoveProperty");
    assert!(doc.remove_property(o, "SC_Support"));
    doc.commit_transaction();
    assert!(doc.object(o).unwrap().properties.get("SC_Support").is_none());

    assert!(doc.undo());
    let obj = doc.object(o).unwrap();
    let property = obj.properties.get("SC_Support").expect("property restored");
    assert_eq!(property, &Property::Link("Base".into()));
    assert_eq!(property.type_name(), "App::PropertyLink");
    assert_eq!(obj.properties.status("SC_Support"), Some(prop_status::HIDDEN));
    assert_eq!(
        obj.properties.raw_status("SC_Support"),
        Some(prop_status::HIDDEN | prop_status::DYNAMIC)
    );
    assert_eq!(obj.properties.group("SC_Support"), Some("SimpleCAD"));
    assert_eq!(
        obj.properties.doc("SC_Support"),
        Some("Solid used by Push/Pull")
    );

    // Redo removes it again.
    assert!(doc.redo());
    assert!(doc.object(o).unwrap().properties.get("SC_Support").is_none());
}

#[test]
fn aborting_a_property_removal_and_an_addition_are_reversible() {
    let mut doc = Document::new();
    let o = doc.add_object("Obj", "App::FeaturePython");
    doc.add_property(
        o,
        "Kept",
        Property::String("v".into()),
        prop_status::NONE,
        "",
        "",
    )
    .unwrap();

    // Abort of an addition drops it again.
    doc.open_transaction();
    doc.add_property(
        o,
        "Temp",
        Property::Integer(1),
        prop_status::NONE,
        "",
        "",
    )
    .unwrap();
    assert!(doc.abort_transaction());
    assert!(doc.object(o).unwrap().properties.get("Temp").is_none());

    // Abort of a removal puts it back, value and status included.
    doc.open_transaction();
    assert!(doc.remove_property(o, "Kept"));
    assert!(doc.abort_transaction());
    assert_eq!(
        doc.object(o).unwrap().properties.get("Kept"),
        Some(&Property::String("v".into()))
    );
    assert_eq!(
        doc.object(o).unwrap().properties.raw_status("Kept"),
        Some(prop_status::DYNAMIC)
    );
}

#[test]
fn add_property_replacing_a_name_restores_the_old_slot_on_undo() {
    let mut doc = Document::new();
    let o = doc.add_object("Obj", "App::FeaturePython");
    doc.add_property(
        o,
        "Thing",
        Property::Integer(1),
        prop_status::NONE,
        "Old",
        "old",
    )
    .unwrap();

    doc.open_transaction_named("ReplaceProperty");
    doc.add_property(
        o,
        "Thing",
        Property::String("new".into()),
        prop_status::HIDDEN,
        "New",
        "new",
    )
    .unwrap();
    doc.commit_transaction();
    assert_eq!(
        doc.object(o).unwrap().properties.get("Thing"),
        Some(&Property::String("new".into()))
    );

    assert!(doc.undo());
    let obj = doc.object(o).unwrap();
    assert_eq!(obj.properties.get("Thing"), Some(&Property::Integer(1)));
    assert_eq!(obj.properties.status("Thing"), Some(prop_status::NONE));
    assert_eq!(obj.properties.group("Thing"), Some("Old"));

    assert!(doc.redo());
    assert_eq!(
        doc.object(o).unwrap().properties.get("Thing"),
        Some(&Property::String("new".into()))
    );
}

#[test]
fn undo_expression_set_and_remove() {
    let mut doc = Document::new();
    let a = doc.add_object("A", "App::Feature");
    doc.set_property(a, "Width", Property::Float(2.0)).unwrap();

    doc.open_transaction_named("Expr");
    doc.set_expression(a, "Result", "Width * 3").unwrap();
    doc.commit_transaction();
    assert_eq!(doc.recompute().unwrap().len(), 1);
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
    assert_eq!(doc.recompute().unwrap().len(), 1);
    assert_eq!(
        doc.object(a).unwrap().properties.get("Result"),
        Some(&Property::Float(6.0))
    );
}
