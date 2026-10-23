//! Writing FreeCAD's binary `.FCStd` container.
//!
//! This is the counterpart of [`super::fcstd::read_fcstd`], and like it a
//! **direct-call API**: [`write_fcstd`] is deliberately *not* wired into
//! [`super::format::write`] or `saveAs` (that is checklist item 8). The JSON
//! placeholder in the dispatch is untouched.
//!
//! # Members
//!
//! A written archive contains, in order:
//!
//! 1. `Document.xml` — the document and its objects;
//! 2. `PartShape.brp`, `PartShape1.brp`, … — one member per
//!    `Part::PropertyPartShape` value, containing the bytes returned by
//!    [`ExtensionData::save`] (ASCII BREP for the Part module) verbatim.
//!
//! There is no `GuiDocument.xml`, no thumbnail and no colour array: `view` is
//! not written yet (checklist item 7), so a reopened document has no view state.
//! The ZIP is single-disk and unencrypted; every member is deflated, there are
//! no data descriptors and no ZIP64 records.
//!
//! # `Document.xml`
//!
//! The emitted property elements are exactly those the reader understands:
//!
//! | [`Property`] | Element |
//! | --- | --- |
//! | `String` | `<Property type="App::PropertyString"><String value=…/></Property>` |
//! | `Integer` | `<Integer value=…>` |
//! | `Bool` | `<Bool value="true|false">` |
//! | `Float` | `<Float value=…>` |
//! | `Quantity` (length) | `App::PropertyLength` + `<Float value=…>` |
//! | `Quantity` (angle) | `App::PropertyAngle` + `<Float value=…>` |
//! | `Vector` | `<PropertyVector valueX="…" valueY="…" valueZ="…">` |
//! | `Placement` | `<PropertyPlacement Px/Py/Pz Q0..Q3 A Ox Oy Oz>` |
//! | `Link` | `<Link value=…>` |
//! | `LinkList` | `<LinkList count><Link value=…>…` |
//! | `LinkSub` | `<LinkSub value=… count><Sub value=…>…` |
//! | `StringList` | `<StringList count><String value=…>…` |
//! | `Enumeration` | `<Integer value=index>` (+ `<CustomEnumList>` when the allowed values are known) |
//! | `Extension` (`Part::PropertyPartShape`) | `<Part file="PartShapeN.brp"/>` |
//!
//! Deliberately **unsupported** for now (an error, never a wrong or silent
//! member): every other [`Property`] variant (`Matrix`, `Rotation`, colour,
//! constraint and list-of-geometry values, the non-shape extensions such as
//! `Part::PropertyTopoShapeList`) and every other extension type. Expressions,
//! `python_state`, `extensions`, `property_status` (the `status` attribute) and
//! the valueless `_Property` placeholders for static transient properties are not
//! written at all yet (item 6); they are documented here as dropped rather than
//! silently invented.
//!
//! `SavedObject::label` is authoritative: a `Label` property is always emitted
//! from it, so a hand-built `SavedDocument` whose `properties` map has no
//! `Label` entry still round-trips. Property type ids are derived from the
//! value: a `String` is written as `App::PropertyString` (a `PropertyFile`
//! cannot be recovered) and a `Quantity` picks `PropertyLength`/`PropertyAngle`
//! from its unit signature (areas and volumes fall back to `PropertyLength`).

use std::fmt;
use std::io::{self, Write};

use flate2::write::DeflateEncoder;
use flate2::Compression;

use super::{SavedDocument, SavedObject};
use crate::property::Property;
use crate::quantity::Quantity;
use crate::unit::Unit;

/// The `Document` schema version this writer emits (upstream 0.21–1.x writes 4).
pub const DOCUMENT_SCHEMA_VERSION: u32 = 4;

/// The `Document` file version this writer emits.
pub const DOCUMENT_FILE_VERSION: u32 = 1;

/// `ProgramVersion` written into `Document.xml`.
pub const PROGRAM_VERSION: &str = concat!("FerroCAD ", env!("CARGO_PKG_VERSION"));

/// The only module payload member that is written today.
const PART_SHAPE_TYPE: &str = "Part::PropertyPartShape";

/// Errors raised while writing a binary `.FCStd` file.
#[derive(Debug)]
pub enum FcstdWriteError {
    /// The file or an archive member could not be written.
    Io(io::Error),
    /// The archive would need ZIP64 (over 4 GiB, or over 65535 members).
    TooLarge(String),
    /// A module-owned property is not representable as an `.FCStd` member yet.
    UnsupportedExtension {
        object: String,
        property: String,
        type_name: String,
    },
    /// A core property type has no `Document.xml` mapping yet.
    UnsupportedProperty {
        object: String,
        property: String,
        type_name: String,
    },
}

impl fmt::Display for FcstdWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FcstdWriteError::Io(e) => write!(f, "I/O error: {e}"),
            FcstdWriteError::TooLarge(msg) => {
                write!(f, "cannot write a non-ZIP64 .FCStd archive: {msg}")
            }
            FcstdWriteError::UnsupportedExtension {
                object,
                property,
                type_name,
            } => write!(
                f,
                "cannot write '{object}.{property}' of type '{type_name}': only \
                 '{PART_SHAPE_TYPE}' has a .FCStd member yet (shape lists and other \
                 module payloads are checklist item 6)"
            ),
            FcstdWriteError::UnsupportedProperty {
                object,
                property,
                type_name,
            } => write!(
                f,
                "cannot write '{object}.{property}' of type '{type_name}': this \
                 property type has no .FCStd Document.xml mapping yet"
            ),
        }
    }
}

impl std::error::Error for FcstdWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FcstdWriteError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FcstdWriteError {
    fn from(e: io::Error) -> Self {
        FcstdWriteError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Write `doc` to `path` as a binary `.FCStd` archive.
///
/// This is a direct API: [`super::format::write`] still emits the native JSON
/// placeholder for `DocumentFormat::Fcstd`.
pub fn write_fcstd(doc: &SavedDocument, path: &str) -> Result<(), FcstdWriteError> {
    let bytes = encode_fcstd(doc)?;
    std::fs::write(path, bytes).map_err(FcstdWriteError::Io)
}

/// Build the `.FCStd` archive in memory (the file-writing half is [`write_fcstd`]).
pub fn encode_fcstd(doc: &SavedDocument) -> Result<Vec<u8>, FcstdWriteError> {
    let mut shape_members: Vec<Member> = Vec::new();
    let mut shape_index = 0usize;
    let xml = document_xml(doc, &mut shape_members, &mut shape_index)?;

    let mut zip = ZipWriter::new();
    zip.add("Document.xml", xml.as_bytes())?;
    for member in &shape_members {
        zip.add(&member.name, &member.data)?;
    }
    zip.finish()
}

// ---------------------------------------------------------------------------
// Document.xml
// ---------------------------------------------------------------------------

struct Member {
    name: String,
    data: Vec<u8>,
}

fn document_xml(
    doc: &SavedDocument,
    shapes: &mut Vec<Member>,
    shape_index: &mut usize,
) -> Result<String, FcstdWriteError> {
    let mut xml = String::new();
    xml.push_str("<?xml version='1.0' encoding='utf-8'?>\n");
    xml.push_str(&format!(
        "<Document SchemaVersion=\"{DOCUMENT_SCHEMA_VERSION}\" \
         FileVersion=\"{DOCUMENT_FILE_VERSION}\" ProgramVersion=\"{PROGRAM_VERSION}\">\n"
    ));

    // The document model has no UID/comment/dates; the name is the label.
    xml.push_str("    <Properties Count=\"1\">\n");
    xml.push_str(&format!(
        "        <Property name=\"Label\" type=\"App::PropertyString\">\
         <String value=\"{}\"/></Property>\n",
        escape_attr(&doc.name)
    ));
    xml.push_str("    </Properties>\n");

    // `<Objects>` carries the type ids; `<ObjectData>` carries the values.
    xml.push_str(&format!("    <Objects Count=\"{}\">\n", doc.objects.len()));
    for object in &doc.objects {
        xml.push_str(&format!(
            "        <Object type=\"{}\" name=\"{}\"/>\n",
            escape_attr(&object.type_id),
            escape_attr(&object.name)
        ));
    }
    xml.push_str("    </Objects>\n");

    xml.push_str(&format!("    <ObjectData Count=\"{}\">\n", doc.objects.len()));
    for object in &doc.objects {
        write_object(&mut xml, object, shapes, shape_index)?;
    }
    xml.push_str("    </ObjectData>\n");
    xml.push_str("</Document>\n");
    Ok(xml)
}

fn write_object(
    out: &mut String,
    object: &SavedObject,
    shapes: &mut Vec<Member>,
    shape_index: &mut usize,
) -> Result<(), FcstdWriteError> {
    // `Label` is always regenerated from `object.label` (see the module docs).
    let properties: Vec<(&String, &Property)> = object
        .properties
        .iter()
        .filter(|(name, _)| name.as_str() != "Label")
        .collect();

    out.push_str(&format!(
        "        <Object name=\"{}\">\n",
        escape_attr(&object.name)
    ));
    out.push_str(&format!(
        "            <Properties Count=\"{}\">\n",
        properties.len() + 1
    ));
    out.push_str(&format!(
        "                <Property name=\"Label\" type=\"App::PropertyString\">\
         <String value=\"{}\"/></Property>\n",
        escape_attr(&object.label)
    ));
    for (name, value) in properties {
        write_property(out, object, name, value, shapes, shape_index)?;
    }
    out.push_str("            </Properties>\n");
    out.push_str("        </Object>\n");
    Ok(())
}

fn write_property(
    out: &mut String,
    object: &SavedObject,
    name: &str,
    value: &Property,
    shapes: &mut Vec<Member>,
    shape_index: &mut usize,
) -> Result<(), FcstdWriteError> {
    let escaped = escape_attr(name);
    match value {
        Property::String(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyString\">\
             <String value=\"{}\"/></Property>\n",
            escape_attr(v)
        )),
        Property::Integer(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyInteger\">\
             <Integer value=\"{v}\"/></Property>\n"
        )),
        Property::Float(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyFloat\">\
             <Float value=\"{}\"/></Property>\n",
            number(*v)
        )),
        Property::Bool(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyBool\">\
             <Bool value=\"{}\"/></Property>\n",
            if *v { "true" } else { "false" }
        )),
        Property::Quantity(q) => {
            // The type is inferred from the unit signature: the reader keeps an
            // angle's degree signature, so `PropertyAngle` round-trips.
            let type_name = if is_angle(q) {
                "App::PropertyAngle"
            } else {
                "App::PropertyLength"
            };
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"{type_name}\">\
                 <Float value=\"{}\"/></Property>\n",
                number(q.value())
            ));
        }
        Property::Vector(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyVector\">\
             <PropertyVector valueX=\"{}\" valueY=\"{}\" valueZ=\"{}\"/></Property>\n",
            number(v.x),
            number(v.y),
            number(v.z)
        )),
        Property::Placement(p) => {
            let rotation = &p.rotation;
            let axis = rotation.axis();
            // Our quaternion is [w, x, y, z]; upstream stores Q0=x … Q3=w.
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyPlacement\">\
                 <PropertyPlacement Px=\"{}\" Py=\"{}\" Pz=\"{}\" Q0=\"{}\" Q1=\"{}\" \
                 Q2=\"{}\" Q3=\"{}\" A=\"{}\" Ox=\"{}\" Oy=\"{}\" Oz=\"{}\"/></Property>\n",
                number(p.base.x),
                number(p.base.y),
                number(p.base.z),
                number(rotation.q[1]),
                number(rotation.q[2]),
                number(rotation.q[3]),
                number(rotation.q[0]),
                number(rotation.angle()),
                number(axis.x),
                number(axis.y),
                number(axis.z)
            ));
        }
        Property::Link(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyLink\">\
             <Link value=\"{}\"/></Property>\n",
            escape_attr(v)
        )),
        Property::LinkList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyLinkList\">\n"
            ));
            out.push_str(&format!("                    <LinkList count=\"{}\">\n", values.len()));
            for v in values {
                out.push_str(&format!(
                    "                        <Link value=\"{}\"/>\n",
                    escape_attr(v)
                ));
            }
            out.push_str("                    </LinkList>\n");
            out.push_str("                </Property>\n");
        }
        Property::LinkSub(target, subs) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyLinkSub\">\n"
            ));
            out.push_str(&format!(
                "                    <LinkSub value=\"{}\" count=\"{}\">\n",
                escape_attr(target),
                subs.len()
            ));
            for sub in subs {
                out.push_str(&format!(
                    "                        <Sub value=\"{}\"/>\n",
                    escape_attr(sub)
                ));
            }
            out.push_str("                    </LinkSub>\n");
            out.push_str("                </Property>\n");
        }
        Property::StringList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyStringList\">\n"
            ));
            out.push_str(&format!(
                "                    <StringList count=\"{}\">\n",
                values.len()
            ));
            for v in values {
                out.push_str(&format!(
                    "                        <String value=\"{}\"/>\n",
                    escape_attr(v)
                ));
            }
            out.push_str("                    </StringList>\n");
            out.push_str("                </Property>\n");
        }
        Property::Enumeration(values, index) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyEnumeration\">\n"
            ));
            if values.is_empty() {
                out.push_str(&format!(
                    "                    <Integer value=\"{index}\"/>\n"
                ));
            } else {
                // Upstream only stores the list for custom enumerations.
                out.push_str(&format!(
                    "                    <Integer value=\"{index}\" CustomEnum=\"true\"/>\n"
                ));
                out.push_str(&format!(
                    "                    <CustomEnumList count=\"{}\">\n",
                    values.len()
                ));
                for v in values {
                    out.push_str(&format!(
                        "                        <Enum value=\"{}\"/>\n",
                        escape_attr(v)
                    ));
                }
                out.push_str("                    </CustomEnumList>\n");
            }
            out.push_str("                </Property>\n");
        }
        Property::Extension(value) => {
            let type_name = value.type_name();
            if type_name != PART_SHAPE_TYPE {
                return Err(FcstdWriteError::UnsupportedExtension {
                    object: object.name.clone(),
                    property: name.to_string(),
                    type_name: type_name.to_string(),
                });
            }
            let member = if *shape_index == 0 {
                "PartShape.brp".to_string()
            } else {
                format!("PartShape{}.brp", *shape_index)
            };
            *shape_index += 1;
            // The module owns the encoding; core places the bytes verbatim.
            let data = value.data().save();
            shapes.push(Member {
                name: member.clone(),
                data,
            });
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"{PART_SHAPE_TYPE}\">\
                 <Part file=\"{member}\"/></Property>\n"
            ));
        }
        other => {
            return Err(FcstdWriteError::UnsupportedProperty {
                object: object.name.clone(),
                property: name.to_string(),
                type_name: other.type_name().to_string(),
            })
        }
    }
    Ok(())
}

fn is_angle(q: &Quantity) -> bool {
    let deg = Unit::parse("deg").expect("'deg' is a known unit");
    q.sig() == deg.sig
}

/// Shortest round-trip float text (`1e-20` stays exact, unlike a fixed format).
fn number(v: f64) -> String {
    format!("{v}")
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ZIP writer
// ---------------------------------------------------------------------------

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;

const VERSION_NEEDED: u16 = 20; // 2.0: deflate
const METHOD_DEFLATE: u16 = 8;
/// DOS timestamp 1980-01-01 00:00:00; ZIP has no time zone and FreeCAD ignores it.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

/// CRC-32 (IEEE 802.3), computed locally so the crate gains no dependency.
const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = CRC_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    entries: usize,
}

impl ZipWriter {
    fn new() -> Self {
        ZipWriter {
            out: Vec::new(),
            central: Vec::new(),
            entries: 0,
        }
    }

    fn add(&mut self, name: &str, data: &[u8]) -> Result<(), FcstdWriteError> {
        let name = name.as_bytes();
        if name.len() > u16::MAX as usize {
            return Err(FcstdWriteError::TooLarge(format!(
                "member name is {} bytes",
                name.len()
            )));
        }

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let compressed = encoder.finish()?;

        let crc = crc32(data);
        let compressed_size = u32_len(compressed.len(), "compressed member size")?;
        let uncompressed_size = u32_len(data.len(), "member size")?;
        let offset = u32_len(self.out.len(), "archive offset")?;
        let name_len = name.len() as u16;

        put_u32(&mut self.out, LOCAL_SIG);
        put_u16(&mut self.out, VERSION_NEEDED);
        put_u16(&mut self.out, 0); // flags: no descriptor, no encryption
        put_u16(&mut self.out, METHOD_DEFLATE);
        put_u16(&mut self.out, DOS_TIME);
        put_u16(&mut self.out, DOS_DATE);
        put_u32(&mut self.out, crc);
        put_u32(&mut self.out, compressed_size);
        put_u32(&mut self.out, uncompressed_size);
        put_u16(&mut self.out, name_len);
        put_u16(&mut self.out, 0); // extra field length
        self.out.extend_from_slice(name);
        self.out.extend_from_slice(&compressed);

        put_u32(&mut self.central, CENTRAL_SIG);
        put_u16(&mut self.central, VERSION_NEEDED); // version made by
        put_u16(&mut self.central, VERSION_NEEDED);
        put_u16(&mut self.central, 0);
        put_u16(&mut self.central, METHOD_DEFLATE);
        put_u16(&mut self.central, DOS_TIME);
        put_u16(&mut self.central, DOS_DATE);
        put_u32(&mut self.central, crc);
        put_u32(&mut self.central, compressed_size);
        put_u32(&mut self.central, uncompressed_size);
        put_u16(&mut self.central, name_len);
        put_u16(&mut self.central, 0); // extra
        put_u16(&mut self.central, 0); // comment
        put_u16(&mut self.central, 0); // disk number
        put_u16(&mut self.central, 0); // internal attributes
        put_u32(&mut self.central, 0); // external attributes
        put_u32(&mut self.central, offset);
        self.central.extend_from_slice(name);

        self.entries += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>, FcstdWriteError> {
        if self.entries > u16::MAX as usize {
            return Err(FcstdWriteError::TooLarge(format!(
                "{} members (the limit is {})",
                self.entries,
                u16::MAX
            )));
        }
        let cd_offset = u32_len(self.out.len(), "central directory offset")?;
        let cd_size = u32_len(self.central.len(), "central directory size")?;
        let entries = self.entries as u16;

        self.out.extend_from_slice(&self.central);
        put_u32(&mut self.out, EOCD_SIG);
        put_u16(&mut self.out, 0); // this disk
        put_u16(&mut self.out, 0); // disk with the central directory
        put_u16(&mut self.out, entries);
        put_u16(&mut self.out, entries);
        put_u32(&mut self.out, cd_size);
        put_u32(&mut self.out, cd_offset);
        put_u16(&mut self.out, 0); // comment length
        Ok(self.out)
    }
}

fn u32_len(n: usize, what: &str) -> Result<u32, FcstdWriteError> {
    u32::try_from(n)
        .map_err(|_| FcstdWriteError::TooLarge(format!("{what} does not fit in 32 bits ({n})")))
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{ExtensionData, ExtensionValue};
    use crate::geometry::{Placement, Rotation, Vector3};
    use std::any::Any;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Read;

    struct FakeShape(Vec<u8>);

    impl ExtensionData for FakeShape {
        fn type_name(&self) -> &str {
            "Part::PropertyPartShape"
        }
        fn clone_box(&self) -> Box<dyn ExtensionData> {
            Box::new(FakeShape(self.0.clone()))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn eq(&self, other: &dyn ExtensionData) -> bool {
            other
                .as_any()
                .downcast_ref::<FakeShape>()
                .is_some_and(|o| o.0 == self.0)
        }
        fn save(&self) -> Vec<u8> {
            self.0.clone()
        }
        fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "FakeShape({} bytes)", self.0.len())
        }
    }

    fn object(name: &str, label: &str) -> SavedObject {
        SavedObject {
            name: name.to_string(),
            label: label.to_string(),
            type_id: "App::FeaturePython".to_string(),
            properties: BTreeMap::new(),
            property_status: BTreeMap::new(),
            expressions: BTreeMap::new(),
            extensions: BTreeSet::new(),
            python_state: None,
            view: None,
        }
    }

    /// Walk the central directory: `(name, method, compressed, uncompressed, offset)`.
    fn members(data: &[u8]) -> Vec<(String, u16, u32, u32, u32)> {
        let eocd = data
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("End of Central Directory record");
        let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
        let cd_offset =
            u32::from_le_bytes(data[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let mut out = Vec::new();
        let mut pos = cd_offset;
        for _ in 0..count {
            assert_eq!(&data[pos..pos + 4], b"PK\x01\x02");
            let method = u16::from_le_bytes([data[pos + 10], data[pos + 11]]);
            let csize = u32::from_le_bytes(data[pos + 20..pos + 24].try_into().unwrap());
            let usize_ = u32::from_le_bytes(data[pos + 24..pos + 28].try_into().unwrap());
            let name_len = u16::from_le_bytes([data[pos + 28], data[pos + 29]]) as usize;
            let extra_len = u16::from_le_bytes([data[pos + 30], data[pos + 31]]) as usize;
            let comment_len = u16::from_le_bytes([data[pos + 32], data[pos + 33]]) as usize;
            let offset = u32::from_le_bytes(data[pos + 42..pos + 46].try_into().unwrap());
            let name = String::from_utf8(data[pos + 46..pos + 46 + name_len].to_vec()).unwrap();
            out.push((name, method, csize, usize_, offset));
            pos += 46 + name_len + extra_len + comment_len;
        }
        out
    }

    fn read_member(data: &[u8], wanted: &str) -> Vec<u8> {
        let (_, method, csize, _, offset) = members(data)
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

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    fn sample() -> SavedDocument {
        let mut prop = object("Box", "A \"box\" & <tag>");
        prop.properties.insert(
            "Name".into(),
            Property::String("a \"quoted\" & <tagged> name".into()),
        );
        prop.properties.insert("Count".into(), Property::Integer(-7));
        prop.properties.insert("Ratio".into(), Property::Float(0.125));
        prop.properties.insert("Enabled".into(), Property::Bool(true));
        prop.properties.insert(
            "Size".into(),
            Property::Quantity(Quantity::new(12.5, Unit::Millimeter)),
        );
        prop.properties.insert(
            "Tilt".into(),
            Property::Quantity(Quantity::new(30.0, Unit::parse("deg").unwrap())),
        );
        prop.properties.insert(
            "Direction".into(),
            Property::Vector(Vector3::new(1.0, 2.0, 3.0)),
        );
        prop.properties.insert(
            "Placement".into(),
            Property::Placement(Placement::new(
                Vector3::new(1.0, 2.0, 3.0),
                Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), 0.5),
            )),
        );
        prop.properties.insert("Target".into(), Property::Link("Tool".into()));
        prop.properties.insert(
            "Group".into(),
            Property::LinkList(vec!["Box".into(), "Tool".into()]),
        );
        prop.properties.insert(
            "ReferenceAxis".into(),
            Property::LinkSub("Tool".into(), vec!["N_Axis".into(), "Face1".into()]),
        );
        prop.properties.insert(
            "Tags".into(),
            Property::StringList(vec!["one".into(), "two".into()]),
        );
        prop.properties.insert(
            "Mode".into(),
            Property::Enumeration(vec!["Zero".into(), "One".into(), "Two".into()], 2),
        );
        prop.properties.insert("MapMode".into(), Property::Enumeration(Vec::new(), 5));
        prop.properties.insert(
            "Shape".into(),
            Property::Extension(ExtensionValue::new(Box::new(FakeShape(
                b"fake brep payload\n".to_vec(),
            )))),
        );

        SavedDocument {
            format: super::super::native::default_format(),
            version: super::super::native::default_version(),
            name: "Doc & <x>".to_string(),
            objects: vec![prop],
        }
    }

    #[test]
    fn emits_document_xml_the_reader_understands() {
        let bytes = encode_fcstd(&sample()).expect("encode");
        let entries = members(&bytes);
        assert_eq!(
            entries.iter().map(|(name, ..)| name.as_str()).collect::<Vec<_>>(),
            ["Document.xml", "PartShape.brp"]
        );
        for (name, method, ..) in &entries {
            assert_eq!(*method, 8, "{name} is deflated");
        }

        let xml = String::from_utf8(read_member(&bytes, "Document.xml")).unwrap();
        assert!(xml.starts_with("<?xml version='1.0' encoding='utf-8'?>\n"), "{xml}");
        assert!(xml.contains(&format!(
            "<Document SchemaVersion=\"{DOCUMENT_SCHEMA_VERSION}\" FileVersion=\"{DOCUMENT_FILE_VERSION}\" ProgramVersion=\"{PROGRAM_VERSION}\">"
        )));
        assert!(xml.contains("<String value=\"Doc &amp; &lt;x&gt;\"/>"));
        assert!(xml.contains("<Object type=\"App::FeaturePython\" name=\"Box\"/>"));
        assert!(xml.contains("<Object name=\"Box\">"));

        // Label comes from `SavedObject.label`, so it survives XML escaping.
        assert!(xml.contains("<String value=\"A &quot;box&quot; &amp; &lt;tag&gt;\"/>"));
        assert!(xml.contains("<String value=\"a &quot;quoted&quot; &amp; &lt;tagged&gt; name\"/>"));
        assert!(xml.contains("type=\"App::PropertyInteger\"><Integer value=\"-7\"/>"));
        assert!(xml.contains("type=\"App::PropertyFloat\"><Float value=\"0.125\"/>"));
        assert!(xml.contains("type=\"App::PropertyBool\"><Bool value=\"true\"/>"));
        assert!(xml.contains("type=\"App::PropertyLength\"><Float value=\"12.5\"/>"));
        assert!(xml.contains("type=\"App::PropertyAngle\"><Float value=\"30\"/>"));
        assert!(xml.contains(
            "<PropertyVector valueX=\"1\" valueY=\"2\" valueZ=\"3\"/>"
        ));
        assert!(xml.contains("<PropertyPlacement Px=\"1\" Py=\"2\" Pz=\"3\" Q0=\""));
        assert!(xml.contains("Q3=\""));
        assert!(xml.contains("type=\"App::PropertyLink\"><Link value=\"Tool\"/>"));
        assert!(xml.contains("<LinkList count=\"2\">"));
        assert!(xml.contains("<LinkSub value=\"Tool\" count=\"2\">"));
        assert!(xml.contains("<Sub value=\"Face1\"/>"));
        assert!(xml.contains("<StringList count=\"2\">"));
        assert!(xml.contains("<Integer value=\"2\" CustomEnum=\"true\"/>"));
        assert!(xml.contains("<CustomEnumList count=\"3\">"));
        // A constructor-default enum writes only the index.
        assert!(xml.contains("<Property name=\"MapMode\" type=\"App::PropertyEnumeration\">"));
        assert!(xml.contains("<Integer value=\"5\"/>"));
        assert!(xml.contains("<Part file=\"PartShape.brp\"/>"));
        assert!(!xml.contains("ExpressionEngine"), "expressions are item 6");
        assert!(
            !xml.contains("App::PropertyPythonObject"),
            "python_state is item 6"
        );

        // The XML parses (a cheap well-formedness check).
        roxmltree::Document::parse(&xml).expect("well-formed Document.xml");

        // The shape member holds the module bytes verbatim.
        assert_eq!(read_member(&bytes, "PartShape.brp"), b"fake brep payload\n");
    }

    #[test]
    fn numbers_keep_exact_round_trip_text() {
        assert_eq!(number(0.125), "0.125");
        // A fixed six-decimal format would lose this value.
        assert_eq!(number(1e-20), "0.00000000000000000001");
        assert_eq!(number(-0.0), "-0");
    }
}
