//! Round-trip and ZIP-structure tests for the `.FCStd` writer
//! ([`ferrocad_core::fcstd_write::write_fcstd`]) — the backend `saveAs(.FCStd)`
//! and `format::write` now dispatch to.
//!
//! These tests call it directly and read the result back with `read_fcstd`.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ferrocad_core::fcstd::read_fcstd;
use ferrocad_core::fcstd_write::{
    write_fcstd, write_fcstd_with_report, FcstdWriteError, WriteReport,
};
use ferrocad_core::{
    prop_status, Document, ExtensionData, ExtensionValue, Placement, Property, Quantity, Rotation,
    Unit, Vector3, ViewState,
};

/// A stand-in for a Part module payload: the members the writer must place in
/// `.brp` members verbatim.
struct FakeShape {
    type_name: &'static str,
    members: Vec<(String, Vec<u8>)>,
    sidecar: Option<(String, Vec<u8>)>,
}

impl FakeShape {
    /// One payload, written as `PartShapeN.brp` + `<Part file>`.
    fn single(type_name: &'static str, data: &[u8]) -> Self {
        FakeShape {
            type_name,
            members: vec![(String::new(), data.to_vec())],
            sidecar: None,
        }
    }

    /// A single payload plus a module sidecar member (see `fcstd_sidecar`).
    fn with_sidecar(
        type_name: &'static str,
        data: &[u8],
        suffix: &str,
        sidecar: &[u8],
    ) -> Self {
        FakeShape {
            type_name,
            members: vec![(String::new(), data.to_vec())],
            sidecar: Some((suffix.to_string(), sidecar.to_vec())),
        }
    }

    /// A shape list: one payload per entry, written as `PartShapeN.M.brp` +
    /// `<TopoShape file>` inside `<ShapeList>`.
    fn list(type_name: &'static str, entries: &[&[u8]]) -> Self {
        FakeShape {
            type_name,
            members: entries
                .iter()
                .enumerate()
                .map(|(index, data)| (index.to_string(), data.to_vec()))
                .collect(),
            sidecar: None,
        }
    }
}

impl ExtensionData for FakeShape {
    fn type_name(&self) -> &str {
        self.type_name
    }
    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(FakeShape {
            type_name: self.type_name,
            members: self.members.clone(),
            sidecar: self.sidecar.clone(),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<FakeShape>()
            .is_some_and(|o| o.type_name == self.type_name && o.members == self.members)
    }
    fn save(&self) -> Vec<u8> {
        self.members.iter().flat_map(|(_, data)| data.clone()).collect()
    }
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        Some(self.members.clone())
    }
    fn fcstd_sidecar(&self) -> Option<(String, Vec<u8>)> {
        self.sidecar.clone()
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FakeShape({}, {} members)", self.type_name, self.members.len())
    }
}

/// A module value with no `.FCStd` representation (the default hook is `None`).
struct Unsupported(u8);

impl ExtensionData for Unsupported {
    fn type_name(&self) -> &str {
        "Test::PropertyWidget"
    }
    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(Unsupported(self.0))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<Unsupported>()
            .is_some_and(|o| o.0 == self.0)
    }
    fn save(&self) -> Vec<u8> {
        vec![self.0]
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unsupported({})", self.0)
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
        Property::Extension(ExtensionValue::new(Box::new(FakeShape::single(
            "Part::PropertyPartShape",
            b"DBRep_DrawableShape\nfake brep payload\n",
        )))),
    )
    .unwrap();

    let tool_id = doc.add_object("Tool", "App::FeaturePython");
    doc.set_property(
        tool_id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape::single(
            "Part::PropertyPartShape",
            b"fake brep payload 1\n",
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

/// Inflate one member from a written archive.
fn read_member(data: &[u8], wanted: &str) -> Vec<u8> {
    let (_, method, csize, _, offset) = central_directory(data)
        .into_iter()
        .find(|(name, ..)| name == wanted)
        .unwrap_or_else(|| panic!("no member '{wanted}'"));
    let lo = offset as usize;
    assert_eq!(&data[lo..lo + 4], b"PK\x03\x04");
    let name_len = u16::from_le_bytes([data[lo + 26], data[lo + 27]]) as usize;
    let extra_len = u16::from_le_bytes([data[lo + 28], data[lo + 29]]) as usize;
    let start = lo + 30 + name_len + extra_len;
    let raw = &data[start..start + csize as usize];
    match method {
        8 => {
            use std::io::Read;
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(raw)
                .read_to_end(&mut out)
                .expect("inflate member");
            out
        }
        0 => raw.to_vec(),
        other => panic!("unexpected method {other}"),
    }
}

fn member_names(data: &[u8]) -> Vec<String> {
    central_directory(data).into_iter().map(|(name, ..)| name).collect()
}

fn read_document_xml(data: &[u8]) -> String {
    String::from_utf8(read_member(data, "Document.xml")).expect("Document.xml is UTF-8")
}

#[test]
fn writes_a_real_zip_that_the_reader_can_open() {
    let doc = sample();
    let saved = doc.to_saved("Round trip");
    let path = temp_path("roundtrip.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write .FCStd");

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04", "a real ZIP local header");

    // Document.xml and GuiDocument.xml first, then one member per
    // Part::PropertyPartShape, in object/property order (Box.Shape, Tool.Shape).
    let entries = central_directory(&bytes);
    let names: Vec<&str> = entries.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Document.xml",
            "GuiDocument.xml",
            "PartShape.brp",
            "PartShape1.brp"
        ]
    );
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

/// A single module value can carry a sidecar member (module metadata, e.g. a
/// lineage). The `.brp` member stays raw and the extra reference is an attribute
/// upstream ignores.
#[test]
fn writes_a_sidecar_beside_the_raw_payload() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(
        id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape::with_sidecar(
            "Part::PropertyPartShape",
            b"raw brep\n",
            "lineage",
            b"sidecar envelope\n",
        )))),
    )
    .unwrap();

    let saved = doc.to_saved("Sidecar");
    let path = temp_path("sidecar.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write with sidecar");
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    // The upstream-compatible member comes first; the sidecar rides beside it,
    // so the single shape is still `<Part file>`, never a `<ShapeList>`.
    assert_eq!(
        member_names(&bytes),
        [
            "Document.xml",
            "GuiDocument.xml",
            "PartShape.brp",
            "PartShape.lineage"
        ]
    );
    assert_eq!(read_member(&bytes, "PartShape.brp"), b"raw brep\n");
    assert_eq!(
        read_member(&bytes, "PartShape.lineage"),
        b"sidecar envelope\n"
    );

    let xml = read_document_xml(&bytes);
    assert!(xml.contains(
        r#"<Part file="PartShape.brp" FerroCADSidecar="PartShape.lineage"/>"#
    ));
    roxmltree::Document::parse(&xml).expect("well-formed Document.xml");
}

/// A shape list becomes one `.brp` member per entry, each referenced by its own
/// `<TopoShape file>`, while a preceding single shape keeps `PartShape.brp`.
#[test]
fn writes_one_member_per_shape_list_entry() {
    let mut doc = Document::new();
    let box_id = doc.add_object("Box", "Part::Feature");
    doc.set_property(
        box_id,
        "Shape",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape::single(
            "Part::PropertyPartShape",
            b"single shape\n",
        )))),
    )
    .unwrap();

    let body_id = doc.add_object("Body", "App::FeaturePython");
    doc.set_property(
        body_id,
        "Shapes",
        Property::Extension(ExtensionValue::new(Box::new(FakeShape::list(
            "Part::PropertyTopoShapeList",
            &[b"first entry\n", b"second entry\n"],
        )))),
    )
    .unwrap();

    let saved = doc.to_saved("Shape list");
    let path = temp_path("shapelist.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write shape list");

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        member_names(&bytes),
        [
            "Document.xml",
            "GuiDocument.xml",
            "PartShape.brp",
            "PartShape1.0.brp",
            "PartShape1.1.brp"
        ]
    );
    assert_eq!(read_member(&bytes, "PartShape1.0.brp"), b"first entry\n");
    assert_eq!(read_member(&bytes, "PartShape1.1.brp"), b"second entry\n");

    let xml = read_document_xml(&bytes);
    assert!(xml.contains("<Part file=\"PartShape.brp\"/>"), "{xml}");
    assert!(
        xml.contains("<ShapeList count=\"2\">"),
        "shape list element: {xml}"
    );
    assert!(
        xml.contains("<TopoShape file=\"PartShape1.0.brp\"/>"),
        "first entry reference: {xml}"
    );
    assert!(
        xml.contains("<TopoShape file=\"PartShape1.1.brp\"/>"),
        "second entry reference: {xml}"
    );
    roxmltree::Document::parse(&xml).expect("well-formed Document.xml");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn writes_expressions_and_object_dependencies() {
    let mut doc = Document::new();
    let tool_id = doc.add_object("Tool", "App::FeaturePython");
    doc.set_property(tool_id, "Value", Property::Integer(2)).unwrap();

    let box_id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(box_id, "Target", Property::Link("Tool".into()))
        .unwrap();
    doc.set_expression(box_id, "Width", "Tool.Value * 2").unwrap();
    doc.set_expression(box_id, "AttachmentOffset.Base.x", "30 * 2")
        .unwrap();

    let saved = doc.to_saved("Expressions");
    let path = temp_path("expressions.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write expressions");
    let bytes = std::fs::read(&path).unwrap();
    let xml = read_document_xml(&bytes);

    // Expressions use the element form `read_expressions` consumes.
    assert!(
        xml.contains(
            "<Property name=\"ExpressionEngine\" type=\"App::PropertyExpressionEngine\">"
        ),
        "{xml}"
    );
    assert!(xml.contains("<ExpressionEngine count=\"2\">"), "{xml}");
    assert!(
        xml.contains("<Expression path=\"Width\" expression=\"Tool.Value * 2\"/>"),
        "{xml}"
    );
    // A nested self-relative path regains upstream's leading dot.
    assert!(
        xml.contains("<Expression path=\".AttachmentOffset.Base.x\" expression=\"30 * 2\"/>"),
        "{xml}"
    );

    // Box depends on Tool twice: the Link and the expression reference.
    assert!(
        xml.contains("<ObjectDeps Name=\"Box\" Count=\"2\">"),
        "{xml}"
    );
    assert!(xml.contains("<Dep Name=\"Tool\"/>"), "{xml}");
    assert!(
        xml.contains("<ObjectDeps Name=\"Tool\" Count=\"0\"/>"),
        "{xml}"
    );

    // Expressions survive the round trip through the reader.
    let read = read_fcstd(path.to_str().unwrap()).unwrap();
    let box_read = read.objects.iter().find(|o| o.name == "Box").unwrap();
    assert_eq!(
        box_read.expressions.get("Width").map(String::as_str),
        Some("Tool.Value * 2")
    );
    assert_eq!(
        box_read
            .expressions
            .get("AttachmentOffset.Base.x")
            .map(String::as_str),
        Some("30 * 2")
    );

    let _ = std::fs::remove_file(&path);
}

/// Statuses are translated back to upstream's internal bits; a static transient
/// property (status without a value) becomes a valueless `_Property`.
#[test]
fn writes_property_statuses_and_transient_placeholders() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(id, "Value", Property::Integer(1)).unwrap();
    let mut saved = doc.to_saved("Status");
    saved.objects[0]
        .property_status
        .insert("Value".to_string(), prop_status::READONLY | prop_status::OUTPUT);
    saved.objects[0]
        .property_status
        .insert("_GroupTouched".to_string(), prop_status::TRANSIENT);

    let path = temp_path("status.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write with statuses");
    let bytes = std::fs::read(&path).unwrap();
    let xml = read_document_xml(&bytes);

    // ReadOnly = 1<<24, Output = 1<<27, Transient = 1<<25 (upstream bits).
    assert!(xml.contains("status=\"150994944\""), "{xml}");
    assert!(
        xml.contains("<_Property name=\"_GroupTouched\" status=\"33554432\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("TransientCount=\"1\""),
        "the placeholder is counted as transient: {xml}"
    );
    roxmltree::Document::parse(&xml).expect("well-formed Document.xml");

    let read = read_fcstd(path.to_str().unwrap()).unwrap();
    let round_tripped = &read.objects[0];
    assert_eq!(
        round_tripped.property_status.get("Value"),
        Some(&(prop_status::READONLY | prop_status::OUTPUT))
    );
    assert_eq!(
        round_tripped.property_status.get("_GroupTouched"),
        Some(&prop_status::TRANSIENT)
    );
    assert_eq!(
        round_tripped.properties.get("Value"),
        Some(&Property::Integer(1))
    );
    assert!(
        !round_tripped.properties.contains_key("_GroupTouched"),
        "a static transient placeholder has no value"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn writes_object_extensions() {
    let mut doc = Document::new();
    let id = doc.add_object("Group", "App::DocumentObjectGroup");
    assert!(doc.add_extension(id, "App::GroupExtensionPython"));

    let saved = doc.to_saved("Extensions");
    let path = temp_path("extensions.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write extensions");
    let bytes = std::fs::read(&path).unwrap();
    let xml = read_document_xml(&bytes);

    assert!(
        xml.contains("<Object name=\"Group\" Extensions=\"True\">"),
        "{xml}"
    );
    assert!(xml.contains("<Extensions Count=\"1\">"), "{xml}");
    assert!(
        xml.contains(
            "<Extension type=\"App::GroupExtensionPython\" name=\"GroupExtensionPython\">"
        ),
        "{xml}"
    );
    roxmltree::Document::parse(&xml).expect("well-formed Document.xml");

    let _ = std::fs::remove_file(&path);
}

/// `GuiDocument.xml` maps the headless view state and writes the `DiffuseColor`
/// member as a little-endian `u32` count plus packed `0xRRGGBB00` values.
#[test]
fn writes_gui_document_and_diffuse_colour_bytes() {
    let mut doc = Document::new();
    let box_id = doc.add_object("Box", "Part::Feature");
    let mut view = ViewState::new(35, [0.8, 0.8, 0.8, 1.0]);
    view.visibility = false;
    view.diffuse_color = Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.5]]);
    doc.object_mut(box_id).unwrap().view = Some(view);
    // An object without a view has no view provider.
    let _tool_id = doc.add_object("Tool", "Part::Feature");

    let saved = doc.to_saved("Gui");
    let path = temp_path("gui.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write GuiDocument");
    let bytes = std::fs::read(&path).unwrap();
    assert!(member_names(&bytes).contains(&"GuiDocument.xml".to_string()));
    assert!(member_names(&bytes).contains(&"DiffuseColor".to_string()));

    let gui = String::from_utf8(read_member(&bytes, "GuiDocument.xml")).unwrap();
    roxmltree::Document::parse(&gui).expect("well-formed GuiDocument.xml");
    assert!(gui.contains("<ViewProviderData Count=\"1\">"), "{gui}");
    assert!(gui.contains("<ViewProvider name=\"Box\">"), "{gui}");
    assert!(!gui.contains("ViewProvider name=\"Tool\""), "{gui}");
    assert!(
        gui.contains(
            "<Property name=\"Visibility\" type=\"App::PropertyBool\"><Bool value=\"false\"/>"
        ),
        "{gui}"
    );
    assert!(
        gui.contains(
            "<Property name=\"Transparency\" type=\"App::PropertyInteger\">\
             <Integer value=\"35\"/>"
        ),
        "{gui}"
    );
    // 0.8 rounds to 0xCC in every channel; the alpha byte is written as 00.
    assert!(
        gui.contains(
            "<Property name=\"ShapeColor\" type=\"App::PropertyColor\">\
             <PropertyColor value=\"0xCCCCCC00\"/>"
        ),
        "{gui}"
    );
    assert!(
        gui.contains(
            "<Property name=\"DiffuseColor\" type=\"App::PropertyColorList\">\
             <ColorList file=\"DiffuseColor\"/>"
        ),
        "{gui}"
    );
    assert!(
        gui.contains(
            "<Property name=\"DisplayMode\" type=\"App::PropertyEnumeration\">\
             <Integer value=\"0\"/>"
        ),
        "{gui}"
    );

    // count = 2 (LE u32), then 0xFF000000 and 0x00FF0000 (LE u32 each).
    assert_eq!(
        read_member(&bytes, "DiffuseColor"),
        [2, 0, 0, 0, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0x00]
    );

    let _ = std::fs::remove_file(&path);
}

/// `python_state` is FerroCAD's own envelope, not upstream's proxy pickle: it is
/// skipped and reported, never written as `<Python value>`.
#[test]
fn reports_python_state_as_dropped() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(id, "Value", Property::Integer(1)).unwrap();
    let mut saved = doc.to_saved("Python state");
    saved.objects[0].python_state = Some("gASVBAAAAAAAAAB9lCiMBWF0dHJzlH2Uc4wFcHJveHl9lS4=".into());

    let path = temp_path("python.FCStd");
    let report: WriteReport = write_fcstd_with_report(&saved, path.to_str().unwrap())
        .expect("write with Python state");
    let dropped = report.dropped();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("Box.python_state"), "{dropped:?}");
    assert!(dropped[0].contains("Python value"), "{dropped:?}");
    assert!(!report.is_empty());

    let bytes = std::fs::read(&path).unwrap();
    let xml = read_document_xml(&bytes);
    assert!(!xml.contains("App::PropertyPythonObject"), "{xml}");
    assert!(!xml.contains("<Python"), "{xml}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_unrepresentable_properties_and_names_them() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(
        id,
        "Script",
        Property::FileIncluded("/tmp/script.py".to_string()),
    )
    .unwrap();
    let saved = doc.to_saved("Files");
    let path = temp_path("unsupported.FCStd");

    let error = write_fcstd(&saved, path.to_str().unwrap()).expect_err("unsupported property");
    let message = error.to_string();
    assert!(matches!(error, FcstdWriteError::UnsupportedProperty { .. }));
    assert!(message.contains("Box.Script"), "{message}");
    assert!(message.contains("App::PropertyFileIncluded"), "{message}");
    assert!(!path.exists());
}

/// The list, geometry and constraint variants the Part conformance documents use
/// (link arrays queue `PropertyPlacementList` and `PropertyIntegerConstraint`).
#[test]
fn writes_and_reads_lists_geometry_and_constraints() {
    let mut doc = Document::new();
    let id = doc.add_object("Values", "App::FeaturePython");
    let placement = |x: f64| {
        Placement::new(
            Vector3::new(x, 2.0, 3.0),
            Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), x),
        )
    };
    let rotations = vec![
        Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 0.25),
        Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), 0.5),
    ];
    let cases: Vec<(&str, Property)> = vec![
        ("Floats", Property::FloatList(vec![0.5, -1.25, 1e-20])),
        ("Integers", Property::IntegerList(vec![-3, 9])),
        ("Bools", Property::BoolList(vec![true, false])),
        ("Pairs", Property::IntPairList(vec![(1, 2), (-3, 4)])),
        (
            "Vectors",
            Property::VectorList(vec![Vector3::new(1.0, 2.0, 3.0), Vector3::new(-1.0, 0.5, 0.0)]),
        ),
        (
            "Placements",
            Property::PlacementList(vec![placement(0.5), placement(1.5)]),
        ),
        (
            "Transform",
            Property::Matrix(ferrocad_core::Matrix4::from_values([
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                15.0, 16.0,
            ])),
        ),
        (
            "Colours",
            Property::ColorList(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0]]),
        ),
    ];
    for (name, value) in &cases {
        doc.set_property(id, *name, value.clone()).unwrap();
    }
    doc.set_property(id, "Tilt", Property::Rotation(rotations[0]))
        .unwrap();
    doc.set_property(id, "Tilts", Property::RotationList(rotations.clone()))
        .unwrap();
    doc.set_property(
        id,
        "Occurrences",
        Property::IntegerConstraint {
            value: 12,
            min: 1,
            max: 1000,
            step: 1,
        },
    )
    .unwrap();
    doc.set_property(
        id,
        "Ratio",
        Property::FloatConstraint {
            value: 0.25,
            min: 0.0,
            max: 1.0,
            step: 0.05,
        },
    )
    .unwrap();

    let path = temp_path("lists.FCStd");
    write_fcstd(&doc.to_saved("Lists"), path.to_str().unwrap()).expect("lists are representable");
    let saved = read_fcstd(path.to_str().unwrap()).expect("read back");
    let values = &saved.objects[0].properties;

    for (name, expected) in &cases {
        assert_eq!(values.get(*name), Some(expected), "{name}");
    }
    // A rotation persists axis + angle, so compare applied directions.
    let close = |a: &Rotation, b: &Rotation| {
        let probe = Vector3::new(1.0, 2.0, 3.0);
        b.to_matrix().transform(&probe).distance(&a.to_matrix().transform(&probe)) < 1e-12
    };
    let Some(Property::Rotation(tilt)) = values.get("Tilt") else {
        panic!("Tilt did not survive: {:?}", values.get("Tilt"));
    };
    assert!(close(&rotations[0], tilt));
    let Some(Property::RotationList(tilts)) = values.get("Tilts") else {
        panic!("Tilts did not survive: {:?}", values.get("Tilts"));
    };
    assert_eq!(tilts.len(), rotations.len());
    for (read, expected) in tilts.iter().zip(&rotations) {
        assert!(close(expected, read));
    }
    // The range is persisted as upstream writes it.
    assert_eq!(
        values.get("Occurrences"),
        Some(&Property::IntegerConstraint {
            value: 12,
            min: 1,
            max: 1000,
            step: 1,
        })
    );
    assert_eq!(
        values.get("Ratio"),
        Some(&Property::FloatConstraint {
            value: 0.25,
            min: 0.0,
            max: 1.0,
            step: 0.05,
        })
    );
    let _ = std::fs::remove_file(&path);
}

/// A module value without a payload hook still fails by name (shape lists are
/// supported now, arbitrary extensions are not).
#[test]
fn unsupported_extension_errors_by_name() {
    let mut doc = Document::new();
    let id = doc.add_object("Body", "App::FeaturePython");
    doc.set_property(
        id,
        "Widget",
        Property::Extension(ExtensionValue::new(Box::new(Unsupported(7)))),
    )
    .unwrap();
    let saved = doc.to_saved("Widgets");
    let path = temp_path("widget.FCStd");

    let error = write_fcstd(&saved, path.to_str().unwrap()).expect_err("unsupported extension");
    let message = error.to_string();
    assert!(matches!(error, FcstdWriteError::UnsupportedExtension { .. }));
    assert!(message.contains("Body.Widget"), "{message}");
    assert!(message.contains("Test::PropertyWidget"), "{message}");
    assert!(message.contains("fcstd_members"), "{message}");
    assert!(!path.exists(), "no partial file is written");
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

/// A non-zero status map is written (not a reason to fail) and read back.
#[test]
fn property_status_does_not_block_writing() {
    let mut doc = Document::new();
    let id = doc.add_object("Box", "App::FeaturePython");
    doc.set_property(id, "Value", Property::Integer(1)).unwrap();
    let mut saved = doc.to_saved("Status");
    let mut status = BTreeMap::new();
    status.insert("Value".to_string(), ferrocad_core::prop_status::OUTPUT);
    saved.objects[0].property_status = status;

    let path = temp_path("status-output.FCStd");
    write_fcstd(&saved, path.to_str().unwrap()).expect("write with statuses");
    let read = read_fcstd(path.to_str().unwrap()).unwrap();
    assert_eq!(read.objects[0].properties.get("Value"), Some(&Property::Integer(1)));
    assert_eq!(
        read.objects[0].property_status.get("Value"),
        Some(&ferrocad_core::prop_status::OUTPUT)
    );
    let _ = std::fs::remove_file(&path);
}
