//! Read an upstream FreeCAD 0.21 binary `.FCStd` and check the attachments of
//! the three sketches in `TestTangentMode3-0.21.FCStd`.
//!
//! Expected values mirror upstream `Mod/Part/parttests/regression_tests.py`
//! (`test_TangentMode3_Issue24254`): the fixture stores the effective
//! `Placement` of each sketch, which is what FreeCAD derives from the attachment.

use std::path::{Path, PathBuf};

use ferrocad_core::fcstd::read_fcstd;
use ferrocad_core::{prop_status, Document, Placement, Property, Quantity, Unit, Vector3};

fn fixture() -> Option<PathBuf> {
    let upstream = std::env::var_os("FERROCAD_UPSTREAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../freecad-upstream")
        });
    let path = upstream.join("src/Mod/Part/parttests/TestTangentMode3-0.21.FCStd");
    path.exists().then_some(path)
}

fn placement(doc: &Document, name: &str) -> Placement {
    let id = doc
        .get_by_name(name)
        .unwrap_or_else(|| panic!("object '{name}' was not loaded"));
    let object = doc.object(id).expect("object id points at a live object");
    match object.properties.get("Placement") {
        Some(Property::Placement(p)) => *p,
        Some(other) => panic!("'{name}.Placement' has unexpected type: {other:?}"),
        None => panic!("'{name}' has no Placement property"),
    }
}

fn check_plane(
    doc: &Document,
    name: &str,
    expected_base: Vector3,
    expected_normal: Vector3,
    expected_xaxis: Vector3,
) {
    let placement = placement(doc, name);
    let rot = placement.rotation.to_matrix();

    let normal = rot.transform(&Vector3::new(0.0, 0.0, 1.0));
    let xaxis = rot.transform(&Vector3::new(1.0, 0.0, 0.0));

    assert!(
        placement.base.distance(&expected_base) < 1e-9,
        "{name}: base {:?}, expected {:?}",
        placement.base,
        expected_base
    );
    assert!(
        normal.angle(&expected_normal) < 1e-9,
        "{name}: normal {normal:?}, expected {expected_normal:?}"
    );
    assert!(
        xaxis.angle(&expected_xaxis) < 1e-9,
        "{name}: x-axis {xaxis:?}, expected {expected_xaxis:?}"
    );
}

#[test]
fn reads_0_21_fcstd_and_reproduces_tangent_planes() {
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let path = path.to_str().expect("fixture path is UTF-8");

    let saved = read_fcstd(path).expect("read binary .FCStd");
    assert_eq!(saved.name, "TestTangentMode3-0.21");
    assert_eq!(saved.objects.len(), 16, "all <ObjectData> entries are read");

    let doc = Document::from_saved(&saved);

    let r = 10.0_f64;
    let cos30 = 30.0_f64.to_radians().cos();

    // Upstream expectations, regressions_tests.py test_TangentMode3_Issue24254.
    check_plane(
        &doc,
        "Sketch001",
        Vector3::new(-r, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
    );
    check_plane(
        &doc,
        "Sketch002",
        Vector3::new(5.0, -7.5, 5.0 * cos30),
        Vector3::new(0.5, -0.75, cos30 / 2.0),
        Vector3::new(0.0, 0.5, cos30),
    );
    check_plane(
        &doc,
        "Sketch003",
        Vector3::new(7.5, 5.0, 5.0 * cos30),
        Vector3::new(0.75, 0.5, cos30 / 2.0),
        Vector3::new(0.5, 0.0, -cos30),
    );
}

#[test]
fn ignores_module_owned_payloads_and_reads_basic_properties() {
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let saved = read_fcstd(path.to_str().unwrap()).expect("read binary .FCStd");

    let sketch = saved
        .objects
        .iter()
        .find(|o| o.name == "Sketch001")
        .expect("Sketch001");
    assert_eq!(sketch.type_id, "Sketcher::SketchObject");
    assert_eq!(sketch.label, "Sketch001");

    // `Part::PropertyPartShape` points at a separate .brp member and is skipped.
    assert!(!sketch.properties.contains_key("Shape"));
    // Persisted scalar properties are read.
    assert_eq!(
        sketch.properties.get("AttacherType"),
        Some(&Property::String("Attacher::AttachEnginePlane".to_string()))
    );
    assert_eq!(
        sketch.properties.get("MapReversed"),
        Some(&Property::Bool(false))
    );
}

/// The F4 reader gaps, exercised against real upstream 0.21 XML.
#[test]
fn reads_lengths_angles_enumerations_links_and_expressions() {
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let saved = read_fcstd(path.to_str().unwrap()).expect("read binary .FCStd");
    let object = |name: &str| {
        saved
            .objects
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("object '{name}' was not read"))
    };

    // `MapMode` is a constructor-default enumeration: only the index is in the
    // file, so the allowed-value list is empty.
    let sketch = object("Sketch001");
    assert_eq!(
        sketch.properties.get("MapMode"),
        Some(&Property::Enumeration(Vec::new(), 6))
    );
    // `Support` is a LinkSubList; it flattens to the linked object names.
    assert_eq!(
        sketch.properties.get("Support"),
        Some(&Property::LinkList(vec![
            "Sketch".to_string(),
            "XY_Plane".to_string()
        ]))
    );
    // The status attribute is upstream's internal mask: 8 = Hidden (1<<3),
    // 134217728 = PropOutput (1<<27).
    assert_eq!(
        sketch.property_status.get("MapPathParameter"),
        Some(&prop_status::HIDDEN)
    );
    assert_eq!(
        sketch.property_status.get("Label"),
        Some(&prop_status::OUTPUT)
    );
    assert!(!sketch.properties.contains_key("Shape"));

    // Both link-sub forms are present: an empty `count="0"` one and one with a
    // named sub-element.
    let pad = object("Pad");
    assert_eq!(
        pad.properties.get("Profile"),
        Some(&Property::LinkSub("Sketch001".to_string(), Vec::new()))
    );
    assert_eq!(
        pad.properties.get("ReferenceAxis"),
        Some(&Property::LinkSub(
            "Sketch001".to_string(),
            vec!["N_Axis".to_string()]
        ))
    );

    // PropertyLength / PropertyAngle keep their unit signature.
    let sphere = object("Sphere");
    assert_eq!(
        sphere.properties.get("Radius"),
        Some(&Property::Quantity(Quantity::new(8.0, Unit::Millimeter)))
    );
    let Some(Property::Quantity(angle)) = sphere.properties.get("Angle1") else {
        panic!("Sphere.Angle1 is not a quantity: {:?}", sphere.properties.get("Angle1"));
    };
    assert_eq!(angle.sig(), Unit::parse("deg").unwrap().sig);
    assert_eq!(angle.value(), -90.0);
    let Some(Property::Quantity(angle3)) = sphere.properties.get("Angle3") else {
        panic!("Sphere.Angle3 is not a quantity");
    };
    assert_eq!(angle3.value(), 360.0);

    // The expression engine is object state, not a property, and upstream's
    // self-relative paths are stored without the leading dot.
    let sketch2 = object("Sketch002");
    assert_eq!(
        sketch2
            .expressions
            .get("AttachmentOffset.Base.x")
            .map(String::as_str),
        Some("cos(30) * sin(30) * 10")
    );
    assert_eq!(
        sketch2
            .expressions
            .get("AttachmentOffset.Base.z")
            .map(String::as_str),
        Some("cos(30) * cos(30) * 10")
    );
    assert!(!sketch2.properties.contains_key("ExpressionEngine"));
}

/// A `<LinkList>` of real names (not a flattened LinkSubList).
#[test]
fn reads_link_lists() {
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let saved = read_fcstd(path.to_str().unwrap()).expect("read binary .FCStd");
    let body = saved
        .objects
        .iter()
        .find(|o| o.name == "Body")
        .expect("Body");
    assert_eq!(
        body.properties.get("Group"),
        Some(&Property::LinkList(vec![
            "Sketch".to_string(),
            "Sketch001".to_string(),
            "Sketch002".to_string(),
            "Sketch003".to_string(),
            "Sphere".to_string(),
            "Pad".to_string(),
            "Pad001".to_string(),
            "Pad002".to_string(),
        ]))
    );
}
