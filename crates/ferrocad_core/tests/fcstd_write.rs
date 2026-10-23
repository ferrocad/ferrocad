//! Round-trip and ZIP-structure tests for the direct-call `.FCStd` writer
//! ([`ferrocad_core::fcstd_write::write_fcstd`]).
//!
//! The writer is not wired into `format::write`/`saveAs` until checklist item 8,
//! so these tests call it directly and read the result back with `read_fcstd`.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ferrocad_core::fcstd::read_fcstd;
use ferrocad_core::fcstd_write::{write_fcstd, FcstdWriteError};
use ferrocad_core::{
    Document, ExtensionData, ExtensionValue, Placement, Property, Quantity, Rotation, Unit,
    Vector3,
};

/// A stand-in for the Part module's shape property: arbitrary bytes that the
/// writer must place in a `.brp` member verbatim.
struct FakeShape(&'static str, Vec<u8>);

impl ExtensionData for FakeShape {
    fn type_name(&self) -> &str {
        self.0
    }
    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(FakeShape(self.0, self.1.clone()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<FakeShape>()
            .is_some_and(|o| o.0 == self.0 && o.1 == self.1)
    }
    fn save(&self) -> Vec<u8> {
        self.1.clone()
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FakeShape({}, {} bytes)", self.0, self.1.len())
    }
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ferrocad-fcstd-write-{}-{name}",
        std::process::id()
    ))
}

/// Build the document described in the task: scalars, a vector, a placement,
/// links, an enumeration and a fake `Part::PropertyPartShape` extension.
fn sample() -> Document {
    let mut doc = Document::new();
    let box_id = doc.add_object("Box", "App::FeaturePython");
    assert!(doc.set_label(box_id, "A box"));
    doc.set_property(box_id, "Name", Property::String("a \"quoted\" & <tagged> name".into()))
        .unwrap();
    doc.set_property(box_id, "Count", Property::Integer(-7)).unwrap();
    doc.set_property(box_id, "Ratio", Property::Float(0.125)).unwrap();
    doc.set_property(box_id, "Enabled", Property::Bool(true)).unwrap();
    doc.set_property(
        box_id,
        "Size",
        Property::Quantity(Quantity::new(12.5, Unit::Millimeter)),
    )
    .unwrap();
    doc.set_property(
        box_id,
        "Tilt",
        Property::Quantity(Quantity::new(30.0, Unit::parse("deg").unwrap())),
    )
    .unwrap();
    doc.set_property(box_id, "Direction", Property::Vector(Vector3::new(1.0, 2.0, 3.0)))
        .unwrap();
    doc.set_property(
        box_id,
        "Placement",
        Property::Placement(Placement::new(
            Vector3::new(1.0, 2.0, 3.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), 0.5),
        )),
    )
    .unwrap();
    doc.set_property(box_id, "Target", Property::Link("Tool".into())).unwrap();
    doc.set_property(
        box_id,
        "Group",
        Property::LinkList(vec!["Box".into(), "Tool".into()]),
    )
    .unwrap();
    doc.set_property(
        box_id,
        "ReferenceAxis",
        Property::LinkSub("Tool".into(), vec!["N_Axis".into(), "Face1".into()]),
    )
    .unwrap();
    doc.set_property(
        box_id,
        "Tags",
        Property::StringList(vec!["one".into(), "two".into()]),
    )
    .unwrap();
    doc.set_property(
        box_id,
        "Mode",
        Property::Enumeration(vec!["Zero".into(), "One".into(), "Two".into()], 2),
    )
    .unwrap();
    doc.set_property(
        box_id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape(
            "Part::PropertyPartShape",
            b"DBRep_DrawableShape\nfake brep payload\n".to_vec(),
        )))),
    )
    .unwrap();

    let tool_id = doc.add_object("Tool", "App::FeaturePython");
    doc.set_property(
        tool_id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape(
            "Part::PropertyPartShape",
            b"fake brep payload 1\n".to_vec(),
        )))),
    )
    .unwrap();
    doc
}

/// Parse the central directory and return `(name, method, compressed size,
/// uncompressed size, local header offset)` for each member, in order.
fn central_directory(data: &[u8]) -> Vec<(String, u16, u32, u32, u32)> {
    let eocd = data
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .expect("End of Central Directory record");
    let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let cd_size = u32::from_le_bytes(data[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
    let cd_offset =
        u32::from_le_bytes(data[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    assert_eq!(eocd, cd_offset + cd_size, "central directory must precede the EOCD");
    assert_eq!(eocd + 22, data.len(), "no archive comment");

    let mut entries = Vec::new();
    let mut pos = cd_offset;
    for _ in 0..count {
        assert_eq!(&data[pos..pos + 4], b"PK\x01\x02", "central entry signature");
        let method = u16::from_le_bytes([data[pos + 10], data[pos + 11]]);
        let csize = u32::from_le_bytes(data[pos + 20..pos + 24].try_into().unwrap());
        let usize_ = u32::from_le_bytes(data[pos + 24..pos + 28].try_into().unwrap());
        let name_len = u16::from_le_bytes([data[pos + 28], data[pos + 29]]) as usize;
        let extra_len = u16::from_le_bytes([data[pos + 30], data[pos + 31]]) as usize;
        let comment_len = u16::from_le_bytes([data[pos + 32], data[pos + 33]]) as usize;
        let local = u32::from_le_bytes(data[pos + 42..pos + 46].try_into().unwrap());
        let name = String::from_utf8(data[pos + 46..pos + 46 + name_len].to_vec()).unwrap();
        assert_eq!(&name, name.trim(), "member names are not padded");
        entries.push((name, method, csize, usize_, local));
        pos += 46 + name_len + extra_len + comment_len;
    }
    assert_eq!(pos, eocd, "central directory walk covers the recorded size");
    entries
}

#[test]
fn writes_a_real_zip_that_the_reader_can_open() {
    let doc = sample();
    let saved = doc.to_saved("Round trip");
    let path = temp_path("roundtrip.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write .FCStd");

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04", "a real ZIP local header");

    // Document.xml first, then one member per Part::PropertyPartShape, in
    // object/property order (Box.Shape, Tool.Shape).
    let entries = central_directory(&bytes);
    let names: Vec<&str> = entries.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(names, ["Document.xml", "PartShape.brp", "PartShape1.brp"]);
    for (name, method, csize, usize_, _) in &entries {
        assert_eq!(*method, 8, "{name} is deflated");
        assert!(*usize_ > 0, "{name} has content");
        assert!(*csize > 0, "{name} has compressed content");
    }

    // Read it back: the scalars, vector, placement, links and enumeration
    // survive; the shape property is skipped exactly as for upstream files.
    let read = read_fcstd(path.to_str().unwrap()).expect("read back");
    assert_eq!(
        read.name,
        Path::new(&path).file_stem().unwrap().to_str().unwrap()
    );
    assert_eq!(read.objects.len(), 2);

    let object = read
        .objects
        .iter()
        .find(|o| o.name == "Box")
        .expect("Box round-trips");
    assert_eq!(object.type_id, "App::FeaturePython");
    assert_eq!(object.label, "A box");

    assert_eq!(
        object.properties.get("Name"),
        Some(&Property::String("a \"quoted\" & <tagged> name".into()))
    );
    assert_eq!(object.properties.get("Count"), Some(&Property::Integer(-7)));
    assert_eq!(object.properties.get("Ratio"), Some(&Property::Float(0.125)));
    assert_eq!(object.properties.get("Enabled"), Some(&Property::Bool(true)));
    assert_eq!(
        object.properties.get("Size"),
        Some(&Property::Quantity(Quantity::new(12.5, Unit::Millimeter)))
    );
    let Some(Property::Quantity(tilt)) = object.properties.get("Tilt") else {
        panic!("Tilt did not round-trip as a quantity");
    };
    assert_eq!(tilt.sig(), Unit::parse("deg").unwrap().sig);
    assert_eq!(tilt.value(), 30.0);
    assert_eq!(
        object.properties.get("Direction"),
        Some(&Property::Vector(Vector3::new(1.0, 2.0, 3.0)))
    );
    assert_eq!(
        object.properties.get("Placement"),
        Some(&Property::Placement(Placement::new(
            Vector3::new(1.0, 2.0, 3.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), 0.5),
        )))
    );
    assert_eq!(
        object.properties.get("Target"),
        Some(&Property::Link("Tool".into()))
    );
    assert_eq!(
        object.properties.get("Group"),
        Some(&Property::LinkList(vec!["Box".into(), "Tool".into()]))
    );
    assert_eq!(
        object.properties.get("ReferenceAxis"),
        Some(&Property::LinkSub(
            "Tool".into(),
            vec!["N_Axis".into(), "Face1".into()]
        ))
    );
    assert_eq!(
        object.properties.get("Tags"),
        Some(&Property::StringList(vec!["one".into(), "two".into()]))
    );
    assert_eq!(
        object.properties.get("Mode"),
        Some(&Property::Enumeration(
            vec!["Zero".into(), "One".into(), "Two".into()],
            2
        ))
    );
    assert!(
        !object.properties.contains_key("Shape"),
        "module payloads stay out of the property map"
    );

    // The reader's label comes from the emitted Label property, not the native
    // `SavedObject.label` field.
    assert_eq!(
        object.properties.get("Label"),
        Some(&Property::String("A box".into()))
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_shape_lists_with_a_clear_error() {
    let mut doc = Document::new();
    let id = doc.add_object("Body", "App::FeaturePython");
    doc.set_property(
        id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape(
            "Part::PropertyTopoShapeList",
            vec![1, 2, 3],
        )))),
    )
    .unwrap();
    let saved = doc.to_saved("Shapes");
    let path = temp_path("shapelist.FCStd");

    let error = write_fcstd(&saved, path.to_str().unwrap()).expect_err("shape lists error");
    let message = error.to_string();
    assert!(matches!(error, FcstdWriteError::UnsupportedExtension { .. }));
    assert!(message.contains("Body.Shape"), "{message}");
    assert!(message.contains("Part::PropertyTopoShapeList"), "{message}");
    assert!(!path.exists(), "no partial file is written");
}

#[test]
fn rejects_unrepresentable_properties_and_names_them() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(id, "Samples", Property::FloatList(vec![1.0, 2.0]))
        .unwrap();
    let saved = doc.to_saved("Lists");
    let path = temp_path("unsupported.FCStd");

    let error = write_fcstd(&saved, path.to_str().unwrap()).expect_err("unsupported property");
    let message = error.to_string();
    assert!(matches!(error, FcstdWriteError::UnsupportedProperty { .. }));
    assert!(message.contains("Box.Samples"), "{message}");
    assert!(message.contains("App::PropertyFloatList"), "{message}");
    assert!(!path.exists());
}

/// The property map of a written document must not contain internal state that
/// the reader cannot consume.
#[test]
fn written_document_keeps_the_native_name_and_label_separate() {
    let doc = sample();
    let saved = doc.to_saved("Round trip");
    // The native `label` is authoritative even if a stale Label property exists.
    let path = temp_path("label.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).unwrap();
    let read = read_fcstd(path.to_str().unwrap()).unwrap();
    let object = read.objects.iter().find(|o| o.name == "Box").unwrap();
    assert_eq!(object.label, "A box");
    // The reader stores no internal fields as properties.
    assert_eq!(
        object.properties.keys().cloned().collect::<Vec<_>>(),
        vec![
            "Count".to_string(),
            "Direction".to_string(),
            "Enabled".to_string(),
            "Group".to_string(),
            "Label".to_string(),
            "Mode".to_string(),
            "Name".to_string(),
            "Placement".to_string(),
            "Ratio".to_string(),
            "ReferenceAxis".to_string(),
            "Size".to_string(),
            "Tags".to_string(),
            "Target".to_string(),
            "Tilt".to_string(),
        ]
    );
    // Sanity: serialising twice produces identical bytes (deterministic output).
    let first = std::fs::read(&path).unwrap();
    write_fcstd(&saved, path.to_str().unwrap()).unwrap();
    let second = std::fs::read(&path).unwrap();
    assert_eq!(first, second);
    let _ = std::fs::remove_file(&path);
}

/// A `SavedDocument` whose property status map is non-empty must still write:
/// statuses are not emitted yet, but they are not a reason to fail either.
#[test]
fn property_status_does_not_block_writing() {
    // Keep this test honest: `property_status` is a `SavedObject` field, so the
    // writer must simply ignore it until item 6.
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(id, "Value", Property::Integer(1)).unwrap();
    let mut saved = doc.to_saved("Status");
    // Nameable only through field access; the integration surface has no
    // `SavedObject` re-export.
    let mut status = BTreeMap::new();
    status.insert("Value".to_string(), ferrocad_core::prop_status::OUTPUT);
    saved.objects[0].property_status = status;

    let path = temp_path("status.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write with statuses");
    let read = read_fcstd(path.to_str().unwrap()).unwrap();
    assert_eq!(read.objects[0].properties.get("Value"), Some(&Property::Integer(1)));
    let _ = std::fs::remove_file(&path);
}
