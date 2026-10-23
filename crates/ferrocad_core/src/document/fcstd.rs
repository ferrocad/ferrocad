//! Reading FreeCAD's binary `.FCStd` container.
//!
//! An `.FCStd` file is a ZIP archive whose `Document.xml` member holds the
//! document's objects (and whose `PartShapeN.brp` members hold B-Rep payloads).
//! This module implements just enough of ZIP and of FreeCAD's XML schema to open
//! old upstream documents: it does **not** read shapes, meshes, sketches or any
//! other module-owned payload. Those properties are skipped, so the resulting
//! [`SavedDocument`] contains the generic properties only.
//!
//! Supported ZIP subset: single-disk archives, entries stored (`method = 0`) or
//! deflated (`method = 8`), unencrypted. ZIP64 archives return
//! [`FcstdError::Zip64`]; other methods return [`FcstdError::UnsupportedCompression`].
//!
//! Property types read from `Document.xml`:
//! `App::PropertyString`, `App::PropertyInteger`, `App::PropertyFloat`,
//! `App::PropertyBool`, `App::PropertyVector`, `App::PropertyPlacement`,
//! `App::PropertyLink` and `App::PropertyStringList`. Everything else is ignored.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

use super::{SavedDocument, SavedObject};
use crate::geometry::{Placement, Rotation, Vector3};
use crate::property::Property;

/// Errors raised while reading a binary `.FCStd` file.
#[derive(Debug)]
pub enum FcstdError {
    /// The file could not be read from disk.
    Io(io::Error),
    /// The bytes are not a ZIP archive (no End of Central Directory record).
    NotZip,
    /// The archive uses ZIP64, which this reader does not implement.
    Zip64,
    /// The archive is encrypted (ZIP general-purpose flag bit 0).
    Encrypted(String),
    /// An entry uses a compression method other than stored/deflate.
    UnsupportedCompression(u16),
    /// A required member (`Document.xml`) is missing.
    MissingEntry(String),
    /// The ZIP structure is malformed.
    BadZip(String),
    /// `Document.xml` is not valid XML.
    Xml(String),
    /// `Document.xml` is not UTF-8.
    Encoding(String),
    /// A numeric attribute could not be parsed.
    Number(String),
}

impl fmt::Display for FcstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FcstdError::Io(e) => write!(f, "I/O error: {e}"),
            FcstdError::NotZip => write!(f, "not a ZIP archive (no End of Central Directory record)"),
            FcstdError::Zip64 => write!(f, "ZIP64 archives are not supported"),
            FcstdError::Encrypted(name) => write!(f, "encrypted ZIP entry '{name}' is not supported"),
            FcstdError::UnsupportedCompression(method) => {
                write!(f, "unsupported ZIP compression method {method}")
            }
            FcstdError::MissingEntry(name) => write!(f, "missing ZIP entry '{name}'"),
            FcstdError::BadZip(msg) => write!(f, "malformed ZIP archive: {msg}"),
            FcstdError::Xml(msg) => write!(f, "malformed Document.xml: {msg}"),
            FcstdError::Encoding(msg) => write!(f, "Document.xml is not UTF-8: {msg}"),
            FcstdError::Number(msg) => write!(f, "invalid numeric value in Document.xml: {msg}"),
        }
    }
}

impl std::error::Error for FcstdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FcstdError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FcstdError {
    fn from(e: io::Error) -> Self {
        FcstdError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// ZIP
// ---------------------------------------------------------------------------

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_MIN_LEN: usize = 22;

struct ZipEntry {
    name: String,
    method: u16,
    compressed_size: usize,
    local_offset: usize,
}

struct ZipArchive<'a> {
    data: &'a [u8],
    entries: BTreeMap<String, ZipEntry>,
}

impl<'a> ZipArchive<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, FcstdError> {
        if data.len() < EOCD_MIN_LEN {
            return Err(FcstdError::NotZip);
        }

        // The End of Central Directory record sits at the end, after an optional
        // comment of at most 65535 bytes. Search backwards for its signature and
        // require the recorded comment length to reach the end of the file, so a
        // signature that happens to occur inside the comment is not mistaken for
        // the record itself.
        let search_start = data.len().saturating_sub(EOCD_MIN_LEN + u16::MAX as usize);
        let eocd = (search_start..=data.len() - EOCD_MIN_LEN).rev().find(|&i| {
            u32_at(data, i) == Some(EOCD_SIG)
                && u16_at(data, i + 20).is_some_and(|comment_len| {
                    i + EOCD_MIN_LEN + comment_len as usize == data.len()
                })
        });
        let Some(eocd) = eocd else {
            return Err(FcstdError::NotZip);
        };

        // A ZIP64 End of Central Directory Locator immediately precedes the EOCD.
        if eocd >= 20 && u32_at(data, eocd - 20) == Some(EOCD64_LOCATOR_SIG) {
            return Err(FcstdError::Zip64);
        }

        let disk = u16_at(data, eocd + 4).unwrap_or(0);
        let cd_disk = u16_at(data, eocd + 6).unwrap_or(0);
        if disk != 0 || cd_disk != 0 {
            return Err(FcstdError::BadZip(
                "multi-disk archives are not supported".to_string(),
            ));
        }

        let total_entries = u16_at(data, eocd + 10).unwrap_or(0) as usize;
        let cd_size = u32_at(data, eocd + 12).unwrap_or(0) as usize;
        let cd_offset = u32_at(data, eocd + 16).unwrap_or(0) as usize;

        if total_entries == u16::MAX as usize
            || cd_size == u32::MAX as usize
            || cd_offset == u32::MAX as usize
        {
            return Err(FcstdError::Zip64);
        }
        if cd_offset > data.len() || cd_size > data.len() - cd_offset {
            return Err(FcstdError::BadZip(
                "central directory lies outside the file".to_string(),
            ));
        }

        let mut entries = BTreeMap::new();
        let mut pos = cd_offset;
        for _ in 0..total_entries {
            if u32_at(data, pos) != Some(CENTRAL_SIG) {
                return Err(FcstdError::BadZip(
                    "bad central directory entry signature".to_string(),
                ));
            }
            let flags = u16_at(data, pos + 8)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?;
            let method = u16_at(data, pos + 10)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?;
            let compressed_size = u32_at(data, pos + 20)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?
                as usize;
            let name_len = u16_at(data, pos + 28)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?
                as usize;
            let extra_len = u16_at(data, pos + 30)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?
                as usize;
            let comment_len = u16_at(data, pos + 32)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?
                as usize;
            let local_offset = u32_at(data, pos + 42)
                .ok_or_else(|| FcstdError::BadZip("truncated central directory".to_string()))?
                as usize;

            let name_end = pos + 46 + name_len;
            let name = data
                .get(pos + 46..name_end)
                .ok_or_else(|| FcstdError::BadZip("truncated entry name".to_string()))?;
            let name = String::from_utf8_lossy(name).into_owned();

            if compressed_size == u32::MAX as usize || local_offset == u32::MAX as usize {
                return Err(FcstdError::Zip64);
            }
            if flags & 0x0001 != 0 {
                return Err(FcstdError::Encrypted(name));
            }

            entries.insert(
                name.clone(),
                ZipEntry { name, method, compressed_size, local_offset },
            );
            pos = name_end
                .checked_add(extra_len)
                .and_then(|p| p.checked_add(comment_len))
                .ok_or_else(|| FcstdError::BadZip("central directory overflow".to_string()))?;
        }

        Ok(ZipArchive { data, entries })
    }

    fn read(&self, wanted: &str) -> Result<Vec<u8>, FcstdError> {
        let entry = self
            .entries
            .get(wanted)
            .ok_or_else(|| FcstdError::MissingEntry(wanted.to_string()))?;

        let lo = entry.local_offset;
        if u32_at(self.data, lo) != Some(LOCAL_SIG) {
            return Err(FcstdError::BadZip(format!(
                "bad local header for '{}'",
                entry.name
            )));
        }
        let name_len = u16_at(self.data, lo + 26)
            .ok_or_else(|| FcstdError::BadZip("truncated local header".to_string()))?
            as usize;
        let extra_len = u16_at(self.data, lo + 28)
            .ok_or_else(|| FcstdError::BadZip("truncated local header".to_string()))?
            as usize;

        let start = lo
            .checked_add(30)
            .and_then(|p| p.checked_add(name_len))
            .and_then(|p| p.checked_add(extra_len))
            .ok_or_else(|| FcstdError::BadZip("local header overflow".to_string()))?;
        let end = start
            .checked_add(entry.compressed_size)
            .ok_or_else(|| FcstdError::BadZip("entry size overflow".to_string()))?;
        let raw = self
            .data
            .get(start..end)
            .ok_or_else(|| FcstdError::BadZip(format!("truncated data for '{}'", entry.name)))?;

        match entry.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|e| {
                        FcstdError::BadZip(format!("cannot inflate '{}': {e}", entry.name))
                    })?;
                Ok(out)
            }
            other => Err(FcstdError::UnsupportedCompression(other)),
        }
    }
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Read a binary FreeCAD `.FCStd` file into a [`SavedDocument`].
///
/// The document name is the file stem (FreeCAD derives the internal name from
/// the file name when opening). Module-owned properties such as
/// `Part::PropertyPartShape` are skipped rather than faked; callers that need
/// geometry must load the corresponding `.brp` members separately.
pub fn read_fcstd(path: &str) -> Result<SavedDocument, FcstdError> {
    let data = std::fs::read(path)?;
    let archive = ZipArchive::parse(&data)?;
    let xml_bytes = archive.read("Document.xml")?;
    let xml = std::str::from_utf8(&xml_bytes)
        .map_err(|e| FcstdError::Encoding(e.to_string()))?;

    let name = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Unnamed")
        .to_string();

    parse_document(xml, name)
}

// ---------------------------------------------------------------------------
// Document.xml
// ---------------------------------------------------------------------------

fn parse_document(xml: &str, name: String) -> Result<SavedDocument, FcstdError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| FcstdError::Xml(e.to_string()))?;
    let root = doc.root_element();
    if !root.has_tag_name("Document") {
        return Err(FcstdError::Xml(format!(
            "expected <Document>, found <{}>",
            root.tag_name().name()
        )));
    }

    // `<Objects>` lists the object types; `<ObjectData>` omits `type`.
    let mut types: BTreeMap<String, String> = BTreeMap::new();
    if let Some(objects) = child(root, "Objects") {
        for object in element_children(objects, "Object") {
            let (Some(object_name), Some(type_id)) =
                (object.attribute("name"), object.attribute("type"))
            else {
                continue;
            };
            types.insert(object_name.to_string(), type_id.to_string());
        }
    }

    let mut saved_objects = Vec::new();
    if let Some(object_data) = child(root, "ObjectData") {
        for object in element_children(object_data, "Object") {
            let Some(object_name) = object.attribute("name") else {
                continue;
            };
            let type_id = types.get(object_name).cloned().unwrap_or_default();
            let mut label = object_name.to_string();
            let mut properties = BTreeMap::new();

            if let Some(props) = child(object, "Properties") {
                // Only persistable `<Property>` children are read; `<_Property>`
                // entries are transient placeholders with no value.
                for prop in element_children(props, "Property") {
                    let Some(prop_name) = prop.attribute("name") else {
                        continue;
                    };
                    let prop_type = prop.attribute("type").unwrap_or("");

                    if prop_name == "Label" && prop_type == "App::PropertyString" {
                        if let Some(value) = child_attr(prop, "String", "value") {
                            label = value.to_string();
                        }
                    }

                    if let Some(value) = parse_property(prop_type, prop)? {
                        properties.insert(prop_name.to_string(), value);
                    }
                }
            }

            saved_objects.push(SavedObject {
                name: object_name.to_string(),
                label,
                type_id,
                properties,
                property_status: BTreeMap::new(),
                expressions: BTreeMap::new(),
                extensions: BTreeSet::new(),
                python_state: None,
                view: None,
            });
        }
    }

    Ok(SavedDocument {
        format: super::native::default_format(),
        version: super::native::default_version(),
        name,
        objects: saved_objects,
    })
}

fn parse_property(prop_type: &str, node: roxmltree::Node<'_, '_>) -> Result<Option<Property>, FcstdError> {
    let value = match prop_type {
        "App::PropertyString" => child_attr(node, "String", "value").map(|v| Property::String(v.to_string())),
        "App::PropertyInteger" => child_attr(node, "Integer", "value")
            .map(|v| parse_i64(v, "Integer"))
            .transpose()?
            .map(Property::Integer),
        "App::PropertyFloat" => child_attr(node, "Float", "value")
            .map(|v| parse_f64(v, "Float"))
            .transpose()?
            .map(Property::Float),
        "App::PropertyBool" => child_attr(node, "Bool", "value")
            .map(|v| Property::Bool(v == "true" || v == "1")),
        "App::PropertyVector" => parse_vector(node)?.map(Property::Vector),
        "App::PropertyPlacement" => parse_placement(node)?.map(Property::Placement),
        "App::PropertyLink" => {
            child_attr(node, "Link", "value").map(|v| Property::Link(v.to_string()))
        }
        "App::PropertyStringList" => Some(Property::StringList(parse_string_list(node))),
        _ => None,
    };
    Ok(value)
}

fn parse_vector(node: roxmltree::Node<'_, '_>) -> Result<Option<Vector3>, FcstdError> {
    let Some(vector) = child(node, "PropertyVector") else {
        return Ok(None);
    };
    // FreeCAD writes `valueX`/`valueY`/`valueZ`.
    let x = required_attr(vector, "valueX")?;
    let y = required_attr(vector, "valueY")?;
    let z = required_attr(vector, "valueZ")?;
    Ok(Some(Vector3::new(
        parse_f64(x, "PropertyVector@valueX")?,
        parse_f64(y, "PropertyVector@valueY")?,
        parse_f64(z, "PropertyVector@valueZ")?,
    )))
}

fn parse_placement(node: roxmltree::Node<'_, '_>) -> Result<Option<Placement>, FcstdError> {
    let Some(placement) = child(node, "PropertyPlacement") else {
        return Ok(None);
    };
    let base = Vector3::new(
        parse_f64(required_attr(placement, "Px")?, "PropertyPlacement@Px")?,
        parse_f64(required_attr(placement, "Py")?, "PropertyPlacement@Py")?,
        parse_f64(required_attr(placement, "Pz")?, "PropertyPlacement@Pz")?,
    );
    // FreeCAD stores the quaternion as Q0=x, Q1=y, Q2=z, Q3=w; ours is [w,x,y,z].
    let quaternion = [
        parse_f64(required_attr(placement, "Q3")?, "PropertyPlacement@Q3")?,
        parse_f64(required_attr(placement, "Q0")?, "PropertyPlacement@Q0")?,
        parse_f64(required_attr(placement, "Q1")?, "PropertyPlacement@Q1")?,
        parse_f64(required_attr(placement, "Q2")?, "PropertyPlacement@Q2")?,
    ];
    Ok(Some(Placement::new(
        base,
        Rotation { q: quaternion, raw_axis: None },
    )))
}

fn parse_string_list(node: roxmltree::Node<'_, '_>) -> Vec<String> {
    let Some(list) = child(node, "StringList") else {
        return Vec::new();
    };
    list.children()
        .filter(|n| n.has_tag_name("String"))
        .filter_map(|n| n.attribute("value"))
        .map(str::to_string)
        .collect()
}

fn parse_f64(text: &str, what: &str) -> Result<f64, FcstdError> {
    text.parse::<f64>()
        .map_err(|e| FcstdError::Number(format!("{what} = {text:?}: {e}")))
}

fn parse_i64(text: &str, what: &str) -> Result<i64, FcstdError> {
    text.parse::<i64>()
        .map_err(|e| FcstdError::Number(format!("{what} = {text:?}: {e}")))
}

fn required_attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Result<&'a str, FcstdError> {
    node.attribute(name).ok_or_else(|| {
        FcstdError::Xml(format!(
            "<{}> is missing the '{name}' attribute",
            node.tag_name().name()
        ))
    })
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(tag))
}

fn child_attr<'a>(node: roxmltree::Node<'a, '_>, tag: &str, attr: &str) -> Option<&'a str> {
    child(node, tag).and_then(|n| n.attribute(attr))
}

fn element_children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    node.children()
        .filter(move |n| n.is_element() && n.has_tag_name(tag))
}
