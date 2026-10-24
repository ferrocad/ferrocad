//! Read-side tests for the `.FCStd` module payloads and `GuiDocument.xml`.
//!
//! The real `Part::PropertyPartShape` restore function lives in `ferrocad_part`,
//! which a bare core test does not link. These tests register a stand-in under
//! the same type names (in their own process, so the existing core tests that
//! assert an unregistered payload is skipped stay valid) and assert the reader
//! hands it the member bytes. A hand-written archive exercises the same path with
//! a stored (not deflated) ZIP and a test-owned type name.

use std::any::Any;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Once;

use ferrocad_core::fcstd::read_fcstd;
use ferrocad_core::property_types;
use ferrocad_core::{
    color_from_packed, ExtensionData, ExtensionValue, Property,
};

// ---------------------------------------------------------------------------
// Test-registered module values
// ---------------------------------------------------------------------------

/// A single-payload module value: records the bytes the reader restored.
struct Recorded {
    type_name: &'static str,
    bytes: Vec<u8>,
}

impl ExtensionData for Recorded {
    fn type_name(&self) -> &str {
        self.type_name
    }
    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(Recorded {
            type_name: self.type_name,
            bytes: self.bytes.clone(),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<Recorded>()
            .is_some_and(|o| o.type_name == self.type_name && o.bytes == self.bytes)
    }
    fn save(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        Some(vec![(String::new(), self.bytes.clone())])
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Recorded({}, {} bytes)", self.type_name, self.bytes.len())
    }
}

/// A list module value: records the entries decoded from the reader's framing.
struct RecordedList {
    type_name: &'static str,
    entries: Vec<Vec<u8>>,
}

impl ExtensionData for RecordedList {
    fn type_name(&self) -> &str {
        self.type_name
    }
    fn clone_box(&self) -> Box<dyn ExtensionData> {
        Box::new(RecordedList {
            type_name: self.type_name,
            entries: self.entries.clone(),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn eq(&self, other: &dyn ExtensionData) -> bool {
        other
            .as_any()
            .downcast_ref::<RecordedList>()
            .is_some_and(|o| o.type_name == self.type_name && o.entries == self.entries)
    }
    fn save(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        put_u64(&mut bytes, self.entries.len() as u64);
        for entry in &self.entries {
            put_u64(&mut bytes, entry.len() as u64);
            bytes.extend_from_slice(entry);
        }
        bytes
    }
    fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
        Some(
            self.entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (index.to_string(), entry.clone()))
                .collect(),
        )
    }
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RecordedList({}, {} entries)",
            self.type_name,
            self.entries.len()
        )
    }
}

fn take_u64(rest: &mut &[u8]) -> Option<u64> {
    let head: [u8; 8] = rest.get(..8)?.try_into().ok()?;
    *rest = &rest[8..];
    Some(u64::from_le_bytes(head))
}

fn restore_recorded(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    Some(Box::new(Recorded {
        type_name: "restored",
        bytes: bytes.to_vec(),
    }))
}

fn restore_list(bytes: &[u8]) -> Option<Box<dyn ExtensionData>> {
    let mut rest = bytes;
    let count = usize::try_from(take_u64(&mut rest)?).ok()?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::try_from(take_u64(&mut rest)?).ok()?;
        let entry = rest.get(..len)?.to_vec();
        rest = &rest[len..];
        entries.push(entry);
    }
    if !rest.is_empty() {
        return None;
    }
    Some(Box::new(RecordedList {
        type_name: "restored",
        entries,
    }))
}

static REGISTER: Once = Once::new();

fn register_test_types() {
    REGISTER.call_once(|| {
        for name in ["Part::PropertyPartShape", "Test::PropertyBytes"] {
            property_types::register_extension(
                name,
                move || {
                    Property::Extension(ExtensionValue::new(Box::new(Recorded {
                        type_name: name,
                        bytes: Vec::new(),
                    })))
                },
                restore_recorded,
            );
        }
        for name in ["Part::PropertyTopoShapeList", "Test::PropertyBytesList"] {
            property_types::register_extension(
                name,
                move || {
                    Property::Extension(ExtensionValue::new(Box::new(RecordedList {
                        type_name: name,
                        entries: Vec::new(),
                    })))
                },
                restore_list,
            );
        }
    });
}

fn recorded(property: &Property) -> Option<&Recorded> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<Recorded>(),
        _ => None,
    }
}

fn recorded_list(property: &Property) -> Option<&RecordedList> {
    match property {
        Property::Extension(value) => value.data().as_any().downcast_ref::<RecordedList>(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Upstream fixture
// ---------------------------------------------------------------------------

fn fixture() -> Option<PathBuf> {
    let upstream = std::env::var_os("FERROCAD_UPSTREAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../freecad-upstream")
        });
    let path = upstream.join("src/Mod/Part/parttests/TestTangentMode3-0.21.FCStd");
    path.exists().then_some(path)
}

#[test]
fn restores_part_shape_payloads_from_the_upstream_fixture() {
    register_test_types();
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let saved = read_fcstd(path.to_str().unwrap()).expect("read binary .FCStd");

    assert_eq!(saved.objects.len(), 16);
    // Nine `Shape` properties and four Pad `AddSubShape` properties reference
    // `PartShapeN.brp`; the four `Part::PropertyGeometryList` properties do not.
    let payloads: Vec<&Recorded> = saved
        .objects
        .iter()
        .flat_map(|object| object.properties.values())
        .filter_map(recorded)
        .collect();
    assert_eq!(payloads.len(), 13, "every <Part file> member is restored");
    for payload in &payloads {
        assert!(!payload.bytes.is_empty(), "a restored .brp payload is empty");
    }

    let pad = saved
        .objects
        .iter()
        .find(|object| object.name == "Pad")
        .expect("Pad");
    let shape = recorded(pad.properties.get("Shape").expect("Pad.Shape"));
    let shape = shape.expect("Pad.Shape is a restored extension");
    assert!(
        shape.bytes.starts_with(b"\nCASCADE Topology V1"),
        "PartShape.brp is ASCII BREP: {:?}",
        String::from_utf8_lossy(&shape.bytes[..shape.bytes.len().min(40)])
    );
}

#[test]
fn restores_view_state_from_the_upstream_fixture() {
    register_test_types();
    let Some(path) = fixture() else {
        eprintln!("TestTangentMode3-0.21.FCStd not found; skipping");
        return;
    };
    let saved = read_fcstd(path.to_str().unwrap()).expect("read binary .FCStd");
    let view = |name: &str| {
        saved
            .objects
            .iter()
            .find(|object| object.name == name)
            .and_then(|object| object.view.as_ref())
            .unwrap_or_else(|| panic!("'{name}' has no view state"))
    };

    // Every `<ViewProvider>` in the fixture matches an `<ObjectData>` entry.
    assert_eq!(
        saved.objects.iter().filter(|object| object.view.is_some()).count(),
        16
    );

    // Pad: hidden, opaque, the 0x727980 Part colour, one diffuse entry.
    let pad = view("Pad");
    assert!(!pad.visibility);
    assert_eq!(pad.transparency, 0);
    let pad_color = color_from_packed(0x7279_8000, true);
    assert_eq!(pad.default_color, pad_color);
    assert_eq!(pad.shape_color, Some(pad_color));
    assert_eq!(pad.effective_color(), pad_color);
    assert_eq!(pad.diffuse_color.as_deref(), Some(&[pad_color][..]));
    assert_eq!(pad.display_mode, "Flat Lines");

    // Pad002 is visible in the fixture.
    assert!(view("Pad002").visibility);

    // A sketch uses `ViewProvider2DObject`: index 1 is Wireframe, not Shaded.
    let sketch = view("Sketch001");
    assert!(!sketch.visibility);
    assert_eq!(sketch.display_mode, "Wireframe");
    assert_eq!(sketch.shape_color, Some(color_from_packed(0xCCCC_CC00, true)));

    // Origin has no ShapeColor: the classic preference default applies and no
    // explicit override is invented.
    let origin = view("Origin");
    assert!(!origin.visibility);
    assert_eq!(origin.shape_color, None);
    assert_eq!(origin.default_color, color_from_packed(0xCCCC_E600, true));
}

// ---------------------------------------------------------------------------
// Hand-written archive (stored members, test-owned extension types)
// ---------------------------------------------------------------------------

const DOCUMENT_XML: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<Document SchemaVersion="4" FileVersion="1" ProgramVersion="0.21">
  <Objects Count="1">
    <Object type="App::FeaturePython" name="Blob"/>
  </Objects>
  <ObjectData Count="1">
    <Object name="Blob">
      <Properties Count="2">
        <Property name="Shape" type="Test::PropertyBytes">
          <Part file="PartShape.brp"/>
        </Property>
        <Property name="Shapes" type="Test::PropertyBytesList">
          <ShapeList count="2">
            <TopoShape file="PartShape1.0.brp"/>
            <TopoShape file="PartShape1.1.brp"/>
          </ShapeList>
        </Property>
      </Properties>
    </Object>
  </ObjectData>
</Document>"#;

const GUI_DOCUMENT_XML: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<Document SchemaVersion="1">
  <ViewProviderData Count="1">
    <ViewProvider name="Blob">
      <Properties Count="5" TransientCount="0">
        <Property name="Visibility" type="App::PropertyBool">
          <Bool value="false"/>
        </Property>
        <Property name="Transparency" type="App::PropertyPercent">
          <Integer value="42"/>
        </Property>
        <Property name="ShapeColor" type="App::PropertyColor">
          <PropertyColor value="0x11223300"/>
        </Property>
        <Property name="DiffuseColor" type="App::PropertyColorList">
          <ColorList file="DiffuseColor"/>
        </Property>
        <Property name="DisplayMode" type="App::PropertyEnumeration">
          <Integer value="1" CustomEnum="true"/>
          <CustomEnumList count="2"><Enum value="Smooth"/><Enum value="Rough"/></CustomEnumList>
        </Property>
      </Properties>
    </ViewProvider>
  </ViewProviderData>
</Document>"#;

#[test]
fn reads_a_hand_written_archive_with_payloads_and_view_state() {
    register_test_types();

    let mut zip = ZipBuilder::new();
    zip.add("Document.xml", DOCUMENT_XML.as_bytes());
    zip.add("GuiDocument.xml", GUI_DOCUMENT_XML.as_bytes());
    zip.add("PartShape.brp", b"single payload");
    zip.add("PartShape1.0.brp", b"first entry");
    zip.add("PartShape1.1.brp", b"second entry");
    // LE u32 count = 2, then 0xFF000000 (red) and 0x00FF0000 (green).
    let mut diffuse = Vec::new();
    put_u32(&mut diffuse, 2);
    put_u32(&mut diffuse, 0xFF00_0000);
    put_u32(&mut diffuse, 0x00FF_0000);
    zip.add("DiffuseColor", &diffuse);

    let path = temp_path("handwritten.FCStd");
    std::fs::write(&path, zip.finish()).expect("write the test archive");
    let saved = read_fcstd(path.to_str().unwrap()).expect("read the test archive");
    let _ = std::fs::remove_file(&path);

    assert_eq!(saved.objects.len(), 1);
    let object = &saved.objects[0];
    assert_eq!(object.name, "Blob");

    let shape = recorded(object.properties.get("Shape").expect("Shape")).expect("single payload");
    assert_eq!(shape.bytes, b"single payload");

    let shapes = recorded_list(object.properties.get("Shapes").expect("Shapes"))
        .expect("shape list payload");
    assert_eq!(
        shapes.entries,
        vec![b"first entry".to_vec(), b"second entry".to_vec()]
    );

    let view = object.view.as_ref().expect("view state");
    assert!(!view.visibility);
    assert_eq!(view.transparency, 42);
    let color = [
        0x11 as f64 / 255.0,
        0x22 as f64 / 255.0,
        0x33 as f64 / 255.0,
        1.0,
    ];
    assert_eq!(view.shape_color, Some(color));
    assert_eq!(view.effective_color(), color);
    assert_eq!(
        view.diffuse_color,
        Some(vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
        ])
    );
    // A `CustomEnumList` resolves the stored index to its name.
    assert_eq!(view.display_mode, "Rough");
}

// ---------------------------------------------------------------------------
// Minimal stored-only ZIP writer for the hand-written case
// ---------------------------------------------------------------------------

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ferrocad-fcstd-read-{}-{name}", std::process::id()))
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

struct ZipBuilder {
    entries: Vec<(String, Vec<u8>)>,
}

impl ZipBuilder {
    fn new() -> Self {
        ZipBuilder {
            entries: Vec::new(),
        }
    }

    fn add(&mut self, name: &str, data: &[u8]) {
        self.entries.push((name.to_string(), data.to_vec()));
    }

    /// Stored (method 0) single-disk archive with no extra fields or comment.
    fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in &self.entries {
            let offset = out.len() as u32;
            let crc = crc32(data);

            put_u32(&mut out, 0x0403_4b50);
            put_u16(&mut out, 20);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u32(&mut out, crc);
            put_u32(&mut out, data.len() as u32);
            put_u32(&mut out, data.len() as u32);
            put_u16(&mut out, name.len() as u16);
            put_u16(&mut out, 0);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            put_u32(&mut central, 0x0201_4b50);
            put_u16(&mut central, 20);
            put_u16(&mut central, 20);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u32(&mut central, crc);
            put_u32(&mut central, data.len() as u32);
            put_u32(&mut central, data.len() as u32);
            put_u16(&mut central, name.len() as u16);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u16(&mut central, 0);
            put_u32(&mut central, 0);
            put_u32(&mut central, offset);
            central.extend_from_slice(name.as_bytes());
        }

        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        put_u32(&mut out, 0x0605_4b50);
        put_u16(&mut out, 0);
        put_u16(&mut out, 0);
        put_u16(&mut out, self.entries.len() as u16);
        put_u16(&mut out, self.entries.len() as u16);
        put_u32(&mut out, central.len() as u32);
        put_u32(&mut out, cd_offset);
        put_u16(&mut out, 0);
        out
    }
}
