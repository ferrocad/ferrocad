//! Reading FreeCAD's binary `.FCStd` container.
//!
//! An `.FCStd` file is a ZIP archive whose `Document.xml` member holds the
//! document's objects, `GuiDocument.xml` holds their view providers, and
//! `PartShapeN.brp`/`DiffuseColorN`/property-named members hold values that are
//! not stored inline. This module implements just enough of ZIP and of
//! FreeCAD's XML schema to open upstream documents.
//!
//! Supported ZIP subset: single-disk archives, entries stored (`method = 0`) or
//! deflated (`method = 8`), unencrypted. ZIP64 archives return
//! [`FcstdError::Zip64`]; other methods return [`FcstdError::UnsupportedCompression`].
//!
//! # Module payloads
//!
//! A property whose value lives in a separate member is rebuilt through the
//! property-type registry rather than interpreted here:
//!
//! - `<Part file="PartShapeN.brp"/>` — the member's bytes go to
//!   [`property_types::restore_extension`] for the property's type (e.g.
//!   `Part::PropertyPartShape`).
//! - `<ShapeList count="…"><TopoShape file="PartShapeN.M.brp"/>…</ShapeList>` —
//!   the members are framed as `u64` LE count, then a `u64` LE length plus bytes
//!   per entry, and the framing goes to `restore_extension`. That is the byte
//!   layout a module list's `ExtensionData::save` produces (see
//!   `Part::PropertyTopoShapeList`), so the reader and the writer are inverse
//!   without core naming a module type.
//!
//! The value is only restored when its module has registered a restore function;
//! an unregistered module type (its crate is not linked) is **skipped**, exactly
//! like any other unsupported property. No placeholder value is invented: a
//! `Part::PropertyPartShape` never becomes a string or an empty extension. A
//! missing member that a property does reference is an error, not a silent drop.
//!
//! # `GuiDocument.xml`
//!
//! When present, each `<ViewProvider name="…">` is matched to the object of the
//! same name and becomes [`SavedObject::view`](super::SavedObject::view):
//! `Visibility` (`Bool`), `Transparency` (`Integer`, also upstream's
//! `App::PropertyPercent`), `ShapeColor` (`PropertyColor`, hex or decimal packed
//! `0xRRGGBB00`), `DiffuseColor`/`DisplayColor` (`ColorList file="…"`, a LE `u32`
//! count followed by one packed `0xRRGGBB00` per colour) and `DisplayMode` (an
//! index into the provider's mode list, or a `CustomEnumList`/`String` when one
//! is stored). Persisted colours are opaque RGBA (`alpha = 1.0`) because the
//! stored alpha byte is unused.
//!
//! Property types read from `Document.xml`:
//! `App::PropertyString`, `App::PropertyInteger`, `App::PropertyFloat`,
//! `App::PropertyBool`, `App::PropertyVector`, `App::PropertyPlacement`,
//! `App::PropertyLink` (and `LinkHidden`), `App::PropertyLinkList` (and
//! `LinkListHidden`), `App::PropertyLinkSub`, `App::PropertyLinkSubList`,
//! `App::PropertyStringList`, `App::PropertyFloatList`, `App::PropertyIntegerList`,
//! `App::PropertyBoolList`, `App::PropertyIntPairList`, `App::PropertyVectorList`,
//! `App::PropertyPlacementList`, `App::PropertyRotation` (and `RotationList`),
//! `App::PropertyMatrix`, `App::PropertyColor`/`ColorList`,
//! `App::PropertyIntegerConstraint`, `App::PropertyFloatConstraint`,
//! `App::PropertyEnumeration`, `App::PropertyLength`, `App::PropertyAngle` and
//! `App::PropertyExpressionEngine`. `App::PropertyPythonObject` is read into
//! [`SavedObject::python_state`] rather than into a property. `_Property`
//! placeholders (static transient properties with no value) contribute their
//! `status` only. Everything else is ignored.
//!
//! File-backed values are read from their members exactly as upstream writes
//! them: `FloatList` (LE `u32` count + `f64`s), `VectorList` (`u32` + `f64` xyz),
//! `PlacementList` (`u32` + `f64` xyz + `f64` q0..q3) and `ColorList` (`u32` +
//! packed `0xRRGGBBAA`). An empty `file` attribute, or an in-memory parse with no
//! archive, yields an empty value.
//!
//! Lossiness, by design:
//!
//! - `PropertyLength` and `PropertyAngle` both become [`Property::Quantity`]:
//!   the unit signature distinguishes them (mm vs deg), so the value and the
//!   physical dimension survive, but [`Property::type_name`] reports
//!   `App::PropertyLength` for both.
//! - `PropertyEnumeration` stores only the selected index. The allowed values
//!   are the object's constructor defaults, so the [`Property::Enumeration`]
//!   value list stays empty unless the file carries a `CustomEnumList`.
//! - `PropertyLinkSubList` is flattened to [`Property::LinkList`]: the object
//!   order survives, the per-link sub-element names do not.
//! - `PropertyColor` (one value) becomes a one-element [`Property::ColorList`],
//!   so re-saving writes `App::PropertyColorList`.
//! - `PropertyRotation` stores axis and angle, not the quaternion; the restored
//!   rotation is the same rotation up to floating-point rounding.
//! - `PropertyBoolList` is a `boost::dynamic_bitset` string whose last character
//!   is element 0.
//! - Upstream's `status` attribute encodes its internal `Property::Status`
//!   bits, not our public mask. The flags that have a public equivalent are
//!   translated into [`crate::prop_status`] bits; ephemeral ones are dropped.
//! - `PropertyPythonObject` stores upstream's base64 pickle verbatim; it is not
//!   the `{"attrs", "proxy"}` envelope FerroCAD's own `capture_object_state`
//!   produces.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

use super::{SavedDocument, SavedObject, ViewState};
use crate::extension::ExtensionValue;
use crate::geometry::{Matrix4, Placement, Rotation, Vector3};
use crate::property::Property;
use crate::property_types;
use crate::quantity::Quantity;
use crate::unit::Unit;

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
    /// `GuiDocument.xml` is not valid XML.
    GuiXml(String),
    /// `Document.xml` is not UTF-8.
    Encoding(String),
    /// `GuiDocument.xml` is not UTF-8.
    GuiEncoding(String),
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
            FcstdError::GuiXml(msg) => write!(f, "malformed GuiDocument.xml: {msg}"),
            FcstdError::Encoding(msg) => write!(f, "Document.xml is not UTF-8: {msg}"),
            FcstdError::GuiEncoding(msg) => write!(f, "GuiDocument.xml is not UTF-8: {msg}"),
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

    /// Whether the archive has a member with this exact name.
    fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
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
/// the file name when opening). Generic properties come from `Document.xml`;
/// module payloads (`PartShapeN.brp`) are rebuilt through the property-type
/// registry when the owning module is loaded, and `GuiDocument.xml` supplies the
/// view state. Skipped values and the payload/registry rule are described in the
/// module documentation.
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

    let mut saved = parse_document(xml, name, Some(&archive))?;

    // The view providers are optional: documents saved without a GUI session
    // have no `GuiDocument.xml` (and every object keeps `view: None`).
    if archive.contains(GUI_DOCUMENT) {
        let gui_bytes = archive.read(GUI_DOCUMENT)?;
        let gui = std::str::from_utf8(&gui_bytes)
            .map_err(|e| FcstdError::GuiEncoding(e.to_string()))?;
        parse_gui_document(gui, &mut saved, &archive)?;
    }

    Ok(saved)
}

const GUI_DOCUMENT: &str = "GuiDocument.xml";

// ---------------------------------------------------------------------------
// Document.xml
// ---------------------------------------------------------------------------

fn parse_document(
    xml: &str,
    name: String,
    archive: Option<&ZipArchive<'_>>,
) -> Result<SavedDocument, FcstdError> {
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
            let mut property_status = BTreeMap::new();
            let mut expressions = BTreeMap::new();
            let mut python_state: Option<String> = None;

            if let Some(props) = child(object, "Properties") {
                // Static transient properties are valueless `_Property`
                // placeholders; only their status is persisted.
                for prop in element_children(props, "_Property") {
                    let (Some(prop_name), Some(status)) =
                        (prop.attribute("name"), prop.attribute("status"))
                    else {
                        continue;
                    };
                    let mask = translate_status(status);
                    if mask != crate::prop_status::NONE {
                        property_status.insert(prop_name.to_string(), mask);
                    }
                }
                // Only persistable `<Property>` children are read; `<_Property>`
                // entries are transient placeholders with no value.
                for prop in element_children(props, "Property") {
                    let Some(prop_name) = prop.attribute("name") else {
                        continue;
                    };
                    let prop_type = prop.attribute("type").unwrap_or("");

                    if let Some(status) = prop.attribute("status") {
                        let mask = translate_status(status);
                        if mask != crate::prop_status::NONE {
                            property_status.insert(prop_name.to_string(), mask);
                        }
                    }

                    if prop_name == "Label" && prop_type == "App::PropertyString" {
                        if let Some(value) = child_attr(prop, "String", "value") {
                            label = value.to_string();
                        }
                    }

                    match prop_type {
                        // Expressions live in a property-shaped element but are
                        // stored on the object in our model (`ExpressionEngine`).
                        "App::PropertyExpressionEngine" => {
                            read_expressions(prop, &mut expressions);
                            continue;
                        }
                        // A Python object is the object's proxy state, not a
                        // property. `Proxy` is upstream's conventional name; any
                        // other PythonObject property is used only as a fallback
                        // because `SavedObject` has a single slot.
                        "App::PropertyPythonObject" => {
                            if let Some(value) = child_attr(prop, "Python", "value") {
                                if prop_name == "Proxy" || python_state.is_none() {
                                    python_state = Some(value.to_string());
                                }
                            }
                            continue;
                        }
                        _ => {}
                    }

                    if let Some(value) = parse_property(prop_type, prop, archive)? {
                        properties.insert(prop_name.to_string(), value);
                    }
                }
            }

            saved_objects.push(SavedObject {
                name: object_name.to_string(),
                label,
                type_id,
                properties,
                property_status,
                expressions,
                extensions: BTreeSet::new(),
                python_state,
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

fn parse_property(
    prop_type: &str,
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Option<Property>, FcstdError> {
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
        "App::PropertyVectorList" => Some(Property::VectorList(parse_vector_list(node, archive)?)),
        "App::PropertyPlacementList" => {
            Some(Property::PlacementList(parse_placement_list(node, archive)?))
        }
        "App::PropertyRotation" => parse_rotation(node)?.map(Property::Rotation),
        "App::PropertyRotationList" => Some(Property::RotationList(parse_rotation_list(node)?)),
        "App::PropertyMatrix" => parse_matrix(node)?.map(Property::Matrix),
        "App::PropertyLink" | "App::PropertyLinkHidden" => {
            child_attr(node, "Link", "value").map(|v| Property::Link(v.to_string()))
        }
        "App::PropertyLinkList" | "App::PropertyLinkListHidden" => {
            Some(Property::LinkList(parse_link_list(node)))
        }
        "App::PropertyLinkSub" | "App::PropertyLinkSubHidden" => {
            parse_link_sub(node).map(|(name, subs)| Property::LinkSub(name, subs))
        }
        // Our model has no LinkSubList variant. The object names are kept in
        // order; the per-link `<Link sub="…">` element names are dropped.
        "App::PropertyLinkSubList" => Some(Property::LinkList(parse_link_sub_list(node))),
        "App::PropertyStringList" => Some(Property::StringList(parse_string_list(node))),
        "App::PropertyFloatList" => Some(Property::FloatList(parse_float_list(node, archive)?)),
        "App::PropertyIntegerList" => Some(Property::IntegerList(parse_integer_list(node)?)),
        "App::PropertyBoolList" => Some(Property::BoolList(parse_bool_list(node))),
        "App::PropertyIntPairList" => Some(Property::IntPairList(parse_int_pair_list(node)?)),
        // `PropertyColor` is one value, `PropertyColorList` a file-backed list of
        // them; both map onto our colour-list value.
        "App::PropertyColor" | "App::PropertyColorList" | "App::PropertyColourList" => {
            Some(Property::ColorList(parse_color_list(node, archive)?))
        }
        // The range is persisted, but older files (and FerroCAD before this arm)
        // may omit it; fall back to the widest range then.
        "App::PropertyIntegerConstraint" => child_attr(node, "Integer", "value")
            .map(|v| parse_i64(v, "Integer"))
            .transpose()?
            .map(|value| Property::IntegerConstraint {
                value,
                min: constraint_attr(node, "Integer", "min", i64::MIN as f64) as i64,
                max: constraint_attr(node, "Integer", "max", i64::MAX as f64) as i64,
                step: constraint_attr(node, "Integer", "step", 1.0) as i64,
            }),
        "App::PropertyFloatConstraint" => child_attr(node, "Float", "value")
            .map(|v| parse_f64(v, "Float"))
            .transpose()?
            .map(|value| Property::FloatConstraint {
                value,
                min: constraint_attr(node, "Float", "min", f64::NEG_INFINITY),
                max: constraint_attr(node, "Float", "max", f64::INFINITY),
                step: constraint_attr(node, "Float", "step", 1.0),
            }),
        // Both length and angle are a bare `<Float>`; the property type carries
        // the unit, which we preserve as the quantity's unit signature so the
        // writer can emit the original type again.
        "App::PropertyLength" | "App::PropertyAngle" => child_attr(node, "Float", "value")
            .map(|v| parse_f64(v, "Float"))
            .transpose()?
            .map(|v| Property::Quantity(Quantity::new(v, quantity_unit(prop_type)))),
        // Only the index is stored in the file; the allowed values come from the
        // object's constructor default unless the file has a `CustomEnumList`.
        "App::PropertyEnumeration" => child_attr(node, "Integer", "value")
            .map(|v| parse_i64(v, "Integer"))
            .transpose()?
            .map(|i| Property::Enumeration(custom_enum_list(node), i.max(0) as usize)),
        _ => None,
    };
    if value.is_some() {
        return Ok(value);
    }
    // No core mapping: a module-owned value may still be in a separate member.
    read_extension(prop_type, node, archive)
}

/// Rebuild a module-owned property from the members a `<Part>`/`<ShapeList>`
/// element references, delegating the bytes to the type registry.
///
/// `None` means the property type has no registered restore function (the module
/// is not linked) or the element carries no payload; the caller then skips the
/// property rather than inventing a value.
fn read_extension(
    prop_type: &str,
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Option<Property>, FcstdError> {
    let Some(archive) = archive else {
        // In-memory parsing (`Document.xml` without its archive) has no members.
        return Ok(None);
    };

    let bytes = if let Some(file) = child_attr(node, "Part", "file") {
        archive.read(file)?
    } else if let Some(list) = child(node, "ShapeList") {
        read_shape_list_payload(archive, list)?
    } else {
        return Ok(None);
    };

    Ok(property_types::restore_extension(prop_type, &bytes)
        .map(|data| Property::Extension(ExtensionValue::new(data))))
}

/// Frame per-entry shape members the way `ShapeListProperty::save` stores them:
/// a `u64` LE count, then a `u64` LE length and the bytes for each entry.
fn read_shape_list_payload(
    archive: &ZipArchive<'_>,
    list: roxmltree::Node<'_, '_>,
) -> Result<Vec<u8>, FcstdError> {
    let files: Vec<&str> = element_children(list, "TopoShape")
        .filter_map(|entry| entry.attribute("file"))
        .collect();

    let mut bytes = Vec::new();
    push_u64(&mut bytes, files.len() as u64);
    for file in files {
        let member = archive.read(file)?;
        push_u64(&mut bytes, member.len() as u64);
        bytes.extend_from_slice(&member);
    }
    Ok(bytes)
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

// ---------------------------------------------------------------------------
// GuiDocument.xml
// ---------------------------------------------------------------------------

/// FreeCAD's classic `DefaultShapeColor` (`0xCCCCE6FF`), used when a view
/// provider does not persist `ShapeColor`. Alpha zero means "unset", i.e.
/// opaque.
const DEFAULT_SHAPE_COLOR: u32 = 0xCCCC_E6FF;

/// `ViewProviderPartExt`'s display modes (the same list as
/// `ferrocad_py::DISPLAY_MODES`).
const PART_DISPLAY_MODES: &[&str] = &["Flat Lines", "Shaded", "Wireframe", "Points"];

/// `ViewProvider2DObject`'s modes, used by `Sketcher::*` objects. The 2D provider
/// deliberately omits `Shaded`, so index 1 is `Wireframe` (verified against
/// `TestTangentMode3-0.21.FCStd`, where every sketch stores `DisplayMode = 1`).
const DISPLAY_2D_MODES: &[&str] = &["Flat Lines", "Wireframe", "Points"];

/// Read the `<ViewProvider>` elements into each object's [`ViewState`].
///
/// A provider that names an object absent from `Document.xml` is ignored; an
/// object without a provider keeps `view: None` ("no view object was created").
fn parse_gui_document(
    xml: &str,
    saved: &mut SavedDocument,
    archive: &ZipArchive<'_>,
) -> Result<(), FcstdError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| FcstdError::GuiXml(e.to_string()))?;
    let root = doc.root_element();
    let Some(data) = child(root, "ViewProviderData") else {
        return Ok(());
    };

    for provider in element_children(data, "ViewProvider") {
        let Some(name) = provider.attribute("name") else {
            continue;
        };
        let Some(object) = saved.objects.iter_mut().find(|object| object.name == name) else {
            continue;
        };
        object.view = Some(read_view_provider(provider, &object.type_id, archive)?);
    }
    Ok(())
}

fn read_view_provider(
    provider: roxmltree::Node<'_, '_>,
    type_id: &str,
    archive: &ZipArchive<'_>,
) -> Result<ViewState, FcstdError> {
    let fallback = super::color_from_packed(DEFAULT_SHAPE_COLOR, true);
    let mut view = ViewState::new(0, fallback);
    let Some(props) = child(provider, "Properties") else {
        return Ok(view);
    };
    let property = |name: &str| {
        element_children(props, "Property").find(|prop| prop.attribute("name") == Some(name))
    };

    if let Some(value) = property("Visibility").and_then(|p| child_attr(p, "Bool", "value")) {
        view.visibility = value == "true" || value == "1";
    }
    if let Some(value) = property("Transparency")
        .and_then(|p| child_attr(p, "Integer", "value"))
    {
        view.transparency = parse_i64(value, "Transparency")?.clamp(0, 100) as i32;
    }
    if let Some(color) = property("ShapeColor").and_then(|p| child_attr(p, "PropertyColor", "value")) {
        let color = super::color_from_packed(parse_packed_color(color)?, true);
        // The file does not distinguish the preference default from an explicit
        // override; the persisted colour is both the effective and the default
        // colour after restore, so the appearance getters stay consistent.
        view.default_color = color;
        view.shape_color = Some(color);
    }
    // Draft view providers call the same property `DisplayColor`.
    let diffuse = property("DiffuseColor").or_else(|| property("DisplayColor"));
    if let Some(file) = diffuse.and_then(|p| child_attr(p, "ColorList", "file")) {
        view.diffuse_color = Some(read_colour_list(archive, file)?);
    }
    if let Some(mode) = property("DisplayMode").and_then(|p| display_mode_name(p, type_id)) {
        view.display_mode = mode;
    }
    Ok(view)
}

/// A `PropertyColor` value: `0xRRGGBB00` (any case) or decimal.
fn parse_packed_color(text: &str) -> Result<u32, FcstdError> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse::<u32>(),
    };
    parsed.map_err(|e| FcstdError::Number(format!("PropertyColor value = {text:?}: {e}")))
}

/// Read a `PropertyColorList` member: a LE `u32` count, then one packed
/// `0xRRGGBB00` per colour. The stored alpha byte is unused, so every colour is
/// restored opaque (`alpha = 1.0`).
fn read_colour_list(
    archive: &ZipArchive<'_>,
    member: &str,
) -> Result<Vec<super::Color>, FcstdError> {
    let data = archive.read(member)?;
    let count = u32_at(&data, 0)
        .ok_or_else(|| {
            FcstdError::BadZip(format!(
                "colour member '{member}' is shorter than its count field"
            ))
        })? as usize;
    let needed = count
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or_else(|| FcstdError::BadZip(format!("colour member '{member}' count overflows")))?;
    if data.len() < needed {
        return Err(FcstdError::BadZip(format!(
            "colour member '{member}' holds {count} colours but only {} bytes",
            data.len()
        )));
    }

    let mut colors = Vec::with_capacity(count);
    for index in 0..count {
        let packed = u32_at(&data, 4 + index * 4).expect("length checked above");
        colors.push(super::color_from_packed(packed, true));
    }
    Ok(colors)
}

fn display_mode_name(node: roxmltree::Node<'_, '_>, type_id: &str) -> Option<String> {
    if let Some(value) = child_attr(node, "String", "value") {
        return Some(value.to_string());
    }
    let index = child_attr(node, "Integer", "value")?.parse::<usize>().ok()?;
    if let Some(list) = child(node, "CustomEnumList") {
        let names: Vec<&str> = element_children(list, "Enum")
            .filter_map(|entry| entry.attribute("value"))
            .collect();
        if let Some(name) = names.get(index) {
            return Some((*name).to_string());
        }
    }
    let modes = if type_id.starts_with("Sketcher::") {
        DISPLAY_2D_MODES
    } else {
        PART_DISPLAY_MODES
    };
    modes.get(index).map(|name| (*name).to_string())
}

/// The unit implied by a quantity-valued property type.
fn quantity_unit(prop_type: &str) -> Unit {
    match prop_type {
        "App::PropertyAngle" => Unit::parse("deg").expect("'deg' is a known unit"),
        _ => Unit::Millimeter,
    }
}

fn parse_link_sub(node: roxmltree::Node<'_, '_>) -> Option<(String, Vec<String>)> {
    let link = child(node, "LinkSub")?;
    let name = link.attribute("value").unwrap_or_default().to_string();
    let subs = element_children(link, "Sub")
        .filter_map(|n| n.attribute("value"))
        .map(str::to_string)
        .collect();
    Some((name, subs))
}

fn parse_link_list(node: roxmltree::Node<'_, '_>) -> Vec<String> {
    let Some(list) = child(node, "LinkList") else {
        return Vec::new();
    };
    element_children(list, "Link")
        .filter_map(|n| n.attribute("value"))
        .map(str::to_string)
        .collect()
}

/// `PropertyLinkSubList` entries are `<Link obj="…" sub="…"/>` (note `obj`,
/// unlike the `value` attribute of `<Link>` in a plain `LinkList`).
fn parse_link_sub_list(node: roxmltree::Node<'_, '_>) -> Vec<String> {
    let Some(list) = child(node, "LinkSubList") else {
        return Vec::new();
    };
    element_children(list, "Link")
        .filter_map(|n| n.attribute("obj"))
        .map(str::to_string)
        .collect()
}

fn custom_enum_list(node: roxmltree::Node<'_, '_>) -> Vec<String> {
    let Some(list) = child(node, "CustomEnumList") else {
        return Vec::new();
    };
    element_children(list, "Enum")
        .filter_map(|n| n.attribute("value"))
        .map(str::to_string)
        .collect()
}

/// Read `<ExpressionEngine><Expression path="…" expression="…"/>…` entries
/// into the object's expression map.
fn read_expressions(node: roxmltree::Node<'_, '_>, out: &mut BTreeMap<String, String>) {
    let Some(engine) = child(node, "ExpressionEngine") else {
        return;
    };
    for expr in element_children(engine, "Expression") {
        let (Some(path), Some(source)) = (expr.attribute("path"), expr.attribute("expression"))
        else {
            continue;
        };
        // Upstream paths are self-relative (".AttachmentOffset.Base.x") or bare
        // ("RefElevation"); our map is keyed without the leading dot.
        let key = path.strip_prefix('.').unwrap_or(path);
        if !key.is_empty() {
            out.insert(key.to_string(), source.to_string());
        }
    }
}

/// Translate upstream's internal `Property::Status` bitmask into our public
/// `PropertyType` mask.
///
/// Upstream persists one bit per internal flag (`Property.h`): 1 Immutable,
/// 2 ReadOnly, 3 Hidden, 4 Transient, 7 Output, 11 NoRecompute, 12 Input,
/// 21 PropDynamic, 22 PropNoPersist, 23 PropNoRecompute, 24 PropReadOnly,
/// 25 PropTransient, 26 PropHidden, 27 PropOutput, 28 PropInput. Ephemeral bits
/// (`Touched`, `NoModify`, `Busy`, `Ordered`, …) have no equivalent in the
/// public mask and are dropped, as are user-defined bits.
fn translate_status(text: &str) -> u32 {
    use crate::prop_status as ps;

    let Ok(raw) = text.parse::<u64>() else {
        return ps::NONE;
    };
    let bit = |n: u32| raw & (1u64 << n) != 0;

    let mut mask = ps::NONE;
    if bit(1) || bit(2) || bit(24) {
        mask |= ps::READONLY; // Immutable, ReadOnly, PropReadOnly
    }
    if bit(3) || bit(26) {
        mask |= ps::HIDDEN; // Hidden, PropHidden
    }
    if bit(4) || bit(25) {
        mask |= ps::TRANSIENT; // Transient, PropTransient
    }
    if bit(7) || bit(27) {
        mask |= ps::OUTPUT; // Output, PropOutput
    }
    if bit(11) || bit(23) {
        mask |= ps::NORECOMPUTE; // NoRecompute, PropNoRecompute
    }
    if bit(12) || bit(28) {
        mask |= ps::INPUT; // Input, PropInput
    }
    if bit(21) {
        mask |= ps::DYNAMIC; // PropDynamic
    }
    if bit(22) {
        mask |= ps::NOPERSIST; // PropNoPersist
    }
    mask
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
    Ok(Some(placement_from_element(placement)?))
}

/// One `<PropertyPlacement …/>` element (also the entry shape of a
/// `<PropertyPlacementList>`).
fn placement_from_element(node: roxmltree::Node<'_, '_>) -> Result<Placement, FcstdError> {
    let base = Vector3::new(
        parse_f64(required_attr(node, "Px")?, "PropertyPlacement@Px")?,
        parse_f64(required_attr(node, "Py")?, "PropertyPlacement@Py")?,
        parse_f64(required_attr(node, "Pz")?, "PropertyPlacement@Pz")?,
    );
    Ok(Placement::new(base, rotation_from_element(node)?))
}

fn parse_rotation(node: roxmltree::Node<'_, '_>) -> Result<Option<Rotation>, FcstdError> {
    let Some(rotation) = child(node, "PropertyRotation") else {
        return Ok(None);
    };
    Ok(Some(rotation_from_element(rotation)?))
}

/// FreeCAD stores the quaternion as Q0=x, Q1=y, Q2=z, Q3=w; ours is [w,x,y,z].
fn rotation_from_element(node: roxmltree::Node<'_, '_>) -> Result<Rotation, FcstdError> {
    if node.attribute("Q0").is_some() {
        let quaternion = [
            parse_f64(required_attr(node, "Q3")?, "PropertyRotation@Q3")?,
            parse_f64(required_attr(node, "Q0")?, "PropertyRotation@Q0")?,
            parse_f64(required_attr(node, "Q1")?, "PropertyRotation@Q1")?,
            parse_f64(required_attr(node, "Q2")?, "PropertyRotation@Q2")?,
        ];
        return Ok(Rotation {
            q: quaternion,
            raw_axis: None,
        });
    }
    // `PropertyRotation` persists the raw axis and angle only (no quaternion).
    let axis = Vector3::new(
        parse_f64(required_attr(node, "Ox")?, "PropertyRotation@Ox")?,
        parse_f64(required_attr(node, "Oy")?, "PropertyRotation@Oy")?,
        parse_f64(required_attr(node, "Oz")?, "PropertyRotation@Oz")?,
    );
    let angle = parse_f64(required_attr(node, "A")?, "PropertyRotation@A")?;
    let mut rotation = Rotation::from_axis_angle(&axis, angle);
    rotation.raw_axis = Some(axis.normalize());
    Ok(rotation)
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

fn parse_float_list(
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Vec<f64>, FcstdError> {
    if let Some(member) = child_attr(node, "FloatList", "file") {
        return read_f64_member(archive, member, 1);
    }
    // Force-XML fallback: `<F v>` (or the older `<Float value>`).
    let Some(list) = child(node, "FloatList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("F") || n.has_tag_name("Float"))
        .map(|n| {
            let value = n
                .attribute("v")
                .or_else(|| n.attribute("value"))
                .unwrap_or_default();
            parse_f64(value, "FloatList entry")
        })
        .collect()
}

fn parse_integer_list(node: roxmltree::Node<'_, '_>) -> Result<Vec<i64>, FcstdError> {
    let Some(list) = child(node, "IntegerList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("I") || n.has_tag_name("Integer"))
        .map(|n| {
            let value = n
                .attribute("v")
                .or_else(|| n.attribute("value"))
                .unwrap_or_default();
            parse_i64(value, "IntegerList entry")
        })
        .collect()
}

/// `PropertyBoolList` is a `boost::dynamic_bitset` string: the last character
/// is element 0.
fn parse_bool_list(node: roxmltree::Node<'_, '_>) -> Vec<bool> {
    let Some(list) = child(node, "BoolList") else {
        return Vec::new();
    };
    if let Some(bitset) = list.attribute("value") {
        return bitset.chars().rev().map(|c| c == '1').collect();
    }
    // Older inline form.
    list.children()
        .filter(|n| n.has_tag_name("Bool"))
        .filter_map(|n| n.attribute("value"))
        .map(|v| v == "true" || v == "1")
        .collect()
}

fn parse_int_pair_list(node: roxmltree::Node<'_, '_>) -> Result<Vec<(i64, i64)>, FcstdError> {
    let Some(list) = child(node, "IntPairList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("Pair"))
        .map(|n| {
            Ok((
                parse_i64(required_attr(n, "first")?, "Pair@first")?,
                parse_i64(required_attr(n, "second")?, "Pair@second")?,
            ))
        })
        .collect()
}

fn parse_vector_list(
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Vec<Vector3>, FcstdError> {
    if let Some(member) = child_attr(node, "VectorList", "file") {
        let values = read_f64_member(archive, member, 3)?;
        return Ok(values
            .chunks_exact(3)
            .map(|v| Vector3::new(v[0], v[1], v[2]))
            .collect());
    }
    // Force-XML fallback.
    let Some(list) = child(node, "VectorList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("PropertyVector"))
        .map(|n| {
            Ok(Vector3::new(
                parse_f64(required_attr(n, "valueX")?, "VectorList@valueX")?,
                parse_f64(required_attr(n, "valueY")?, "VectorList@valueY")?,
                parse_f64(required_attr(n, "valueZ")?, "VectorList@valueZ")?,
            ))
        })
        .collect()
}

fn parse_placement_list(
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Vec<Placement>, FcstdError> {
    if let Some(member) = child_attr(node, "PlacementList", "file") {
        let values = read_f64_member(archive, member, 7)?;
        return Ok(values
            .chunks_exact(7)
            .map(|v| {
                Placement::new(
                    Vector3::new(v[0], v[1], v[2]),
                    // Upstream's quaternion is x, y, z, w; ours is [w, x, y, z].
                    Rotation {
                        q: [v[6], v[3], v[4], v[5]],
                        raw_axis: None,
                    },
                )
            })
            .collect());
    }
    // Force-XML fallback.
    let Some(list) = child(node, "PlacementList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("PropertyPlacement"))
        .map(placement_from_element)
        .collect()
}

/// Read a file-backed numeric list: a LE `u32` count then `stride` `f64`s per
/// entry (upstream `SaveDocFile`). A missing archive (in-memory parse) or an
/// empty `file` attribute yields an empty value, as upstream's empty lists do.
fn read_f64_member(
    archive: Option<&ZipArchive<'_>>,
    member: &str,
    stride: usize,
) -> Result<Vec<f64>, FcstdError> {
    if member.is_empty() {
        return Ok(Vec::new());
    }
    let Some(archive) = archive else {
        return Ok(Vec::new());
    };
    let data = archive.read(member)?;
    let count = u32_at(&data, 0)
        .ok_or_else(|| {
            FcstdError::BadZip(format!(
                "value member '{member}' is shorter than its count field"
            ))
        })? as usize;
    let needed = count
        .checked_mul(stride)
        .and_then(|values| values.checked_mul(8))
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or_else(|| FcstdError::BadZip(format!("value member '{member}' overflows")))?;
    if data.len() < needed {
        return Err(FcstdError::BadZip(format!(
            "value member '{member}' holds {count} entries but only {} bytes",
            data.len()
        )));
    }
    let mut out = Vec::with_capacity(count * stride);
    for index in 0..count * stride {
        let start = 4 + index * 8;
        let bytes: [u8; 8] = data[start..start + 8]
            .try_into()
            .expect("length checked above");
        out.push(f64::from_le_bytes(bytes));
    }
    Ok(out)
}

fn parse_rotation_list(node: roxmltree::Node<'_, '_>) -> Result<Vec<Rotation>, FcstdError> {
    let Some(list) = child(node, "PropertyRotationList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("PropertyRotation"))
        .map(rotation_from_element)
        .collect()
}

/// A `PropertyMatrix` persists its 16 values as `a11` … `a44`, row-major.
fn parse_matrix(node: roxmltree::Node<'_, '_>) -> Result<Option<Matrix4>, FcstdError> {
    let Some(matrix) = child(node, "PropertyMatrix") else {
        return Ok(None);
    };
    let mut values = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            let name = format!("a{}{}", row + 1, column + 1);
            values[row * 4 + column] = parse_f64(
                required_attr(matrix, &name)?,
                &format!("PropertyMatrix@{name}"),
            )?;
        }
    }
    Ok(Some(Matrix4::from_values(values)))
}

fn parse_color_list(
    node: roxmltree::Node<'_, '_>,
    archive: Option<&ZipArchive<'_>>,
) -> Result<Vec<super::Color>, FcstdError> {
    // `App::PropertyColor` stores a single value, not a `<ColorList>` wrapper.
    if let Some(value) = child_attr(node, "PropertyColor", "value") {
        return Ok(vec![super::color_from_packed(parse_packed_color(value)?, false)]);
    }
    // `PropertyColorList::SaveDocFile`: LE `u32` count, then `0xRRGGBBAA` values.
    if let Some(member) = child_attr(node, "ColorList", "file") {
        if member.is_empty() {
            return Ok(Vec::new());
        }
        let Some(archive) = archive else {
            return Ok(Vec::new());
        };
        let data = archive.read(member)?;
        let count = u32_at(&data, 0)
            .ok_or_else(|| {
                FcstdError::BadZip(format!(
                    "colour member '{member}' is shorter than its count field"
                ))
            })? as usize;
        let needed = count
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(4))
            .ok_or_else(|| FcstdError::BadZip(format!("colour member '{member}' overflows")))?;
        if data.len() < needed {
            return Err(FcstdError::BadZip(format!(
                "colour member '{member}' holds {count} colours but only {} bytes",
                data.len()
            )));
        }
        let mut colors = Vec::with_capacity(count);
        for index in 0..count {
            let packed = u32_at(&data, 4 + index * 4).expect("length checked above");
            colors.push(super::color_from_packed(packed, false));
        }
        return Ok(colors);
    }
    // Force-XML fallback: inline entries.
    let Some(list) = child(node, "ColorList") else {
        return Ok(Vec::new());
    };
    list.children()
        .filter(|n| n.has_tag_name("PropertyColor") || n.has_tag_name("Color"))
        .map(|n| {
            let value = required_attr(n, "value")?;
            Ok(super::color_from_packed(parse_packed_color(value)?, false))
        })
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

/// A constraint bound (`min`/`max`/`step`), defaulting when an older file omits it.
fn constraint_attr(node: roxmltree::Node<'_, '_>, tag: &str, attr: &str, default: f64) -> f64 {
    child_attr(node, tag, attr)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop_status;

    fn read(xml: &str) -> SavedDocument {
        parse_document(xml, "Fixture".to_string(), None).expect("parse Document.xml")
    }

    /// Hand-written from the upstream 0.21 element forms (see
    /// `TestTangentMode3-0.21.FCStd` and `FC_site_simple-102.FCStd`).
    const NEW_TYPES: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<Document SchemaVersion="4" FileVersion="1" ProgramVersion="0.21">
  <Objects Count="1">
    <Object type="App::FeaturePython" name="Obj"/>
  </Objects>
  <ObjectData Count="1">
    <Object name="Obj">
      <Properties Count="13">
        <Property name="Label" type="App::PropertyString"><String value="A label"/></Property>
        <Property name="Length" type="App::PropertyLength"><Float value="12.5"/></Property>
        <Property name="Angle" type="App::PropertyAngle"><Float value="-90"/></Property>
        <Property name="MapMode" type="App::PropertyEnumeration"><Integer value="5"/></Property>
        <Property name="UnitSystem" type="App::PropertyEnumeration" status="1">
          <Integer value="1" CustomEnum="true"/>
          <CustomEnumList count="2"><Enum value="mm"/><Enum value="m"/></CustomEnumList>
        </Property>
        <Property name="ReferenceAxis" type="App::PropertyLinkSub" status="33554433">
          <LinkSub value="Sketch001" count="1"><Sub value="N_Axis"/></LinkSub>
        </Property>
        <Property name="Profile" type="App::PropertyLinkSub">
          <LinkSub value="" count="0"></LinkSub>
        </Property>
        <Property name="Group" type="App::PropertyLinkList">
          <LinkList count="2"><Link value="A"/><Link value="B"/></LinkList>
        </Property>
        <Property name="Support" type="App::PropertyLinkSubList">
          <LinkSubList count="2"><Link obj="Sketch" sub="Vertex1"/><Link obj="XY_Plane" sub=""/></LinkSubList>
        </Property>
        <Property name="Tags" type="App::PropertyStringList">
          <StringList count="2"><String value="x"/><String value="y"/></StringList>
        </Property>
        <Property name="ExpressionEngine" type="App::PropertyExpressionEngine" status="67108864">
          <ExpressionEngine count="2">
            <Expression path=".AttachmentOffset.Base.x" expression="cos(30) * 10"/>
            <Expression path="RefElevation" expression=".Elevation.Value"/>
          </ExpressionEngine>
        </Property>
        <Property name="Proxy" type="App::PropertyPythonObject">
          <Python value="bnVsbA==" encoded="yes" module="ArchSite" class="_Site"/>
        </Property>
        <Property name="Shape" type="Part::PropertyPartShape">
          <Part file="PartShape.brp"/>
        </Property>
      </Properties>
    </Object>
  </ObjectData>
</Document>"#;

    #[test]
    fn reads_the_new_property_types() {
        let saved = read(NEW_TYPES);
        assert_eq!(saved.objects.len(), 1);
        let o = &saved.objects[0];
        assert_eq!(o.type_id, "App::FeaturePython");
        assert_eq!(o.label, "A label");
        assert_eq!(
            o.properties.get("Label"),
            Some(&Property::String("A label".to_string()))
        );

        assert_eq!(
            o.properties.get("Length"),
            Some(&Property::Quantity(Quantity::new(12.5, Unit::Millimeter)))
        );
        let Some(Property::Quantity(angle)) = o.properties.get("Angle") else {
            panic!("Angle is not a quantity: {:?}", o.properties.get("Angle"));
        };
        assert_eq!(angle.unit().sig, Unit::parse("deg").unwrap().sig);
        assert_eq!(angle.value(), -90.0);

        // Without a `CustomEnumList` only the index survives.
        assert_eq!(
            o.properties.get("MapMode"),
            Some(&Property::Enumeration(Vec::new(), 5))
        );
        assert_eq!(
            o.properties.get("UnitSystem"),
            Some(&Property::Enumeration(vec!["mm".to_string(), "m".to_string()], 1))
        );

        assert_eq!(
            o.properties.get("ReferenceAxis"),
            Some(&Property::LinkSub(
                "Sketch001".to_string(),
                vec!["N_Axis".to_string()]
            ))
        );
        assert_eq!(
            o.properties.get("Profile"),
            Some(&Property::LinkSub(String::new(), Vec::new()))
        );
        assert_eq!(
            o.properties.get("Group"),
            Some(&Property::LinkList(vec!["A".to_string(), "B".to_string()]))
        );
        // LinkSubList is flattened to the object names, in order.
        assert_eq!(
            o.properties.get("Support"),
            Some(&Property::LinkList(vec![
                "Sketch".to_string(),
                "XY_Plane".to_string()
            ]))
        );
        assert_eq!(
            o.properties.get("Tags"),
            Some(&Property::StringList(vec!["x".to_string(), "y".to_string()]))
        );

        // Module payloads are still skipped.
        assert!(!o.properties.contains_key("Shape"));

        // Expressions are keyed without the self-relative leading dot.
        assert_eq!(
            o.expressions.get("AttachmentOffset.Base.x").map(String::as_str),
            Some("cos(30) * 10")
        );
        assert_eq!(
            o.expressions.get("RefElevation").map(String::as_str),
            Some(".Elevation.Value")
        );

        // The proxy pickle becomes Python state, not a property.
        assert!(!o.properties.contains_key("Proxy"));
        assert_eq!(o.python_state.as_deref(), Some("bnVsbA=="));
    }

    #[test]
    fn translates_upstream_property_status_bits() {
        let saved = read(NEW_TYPES);
        let status = &saved.objects[0].property_status;
        // 33554433 = PropTransient (1<<25) + Touched (1<<0).
        assert_eq!(status.get("ReferenceAxis"), Some(&prop_status::TRANSIENT));
        // 67108864 = PropHidden (1<<26).
        assert_eq!(status.get("ExpressionEngine"), Some(&prop_status::HIDDEN));
        // 1 = Touched only; there is no public equivalent.
        assert!(!status.contains_key("UnitSystem"));
    }

    #[test]
    fn translate_status_maps_every_public_bit() {
        // PropReadOnly | PropHidden | PropOutput = 1<<24 | 1<<26 | 1<<27.
        assert_eq!(
            translate_status("218103808"),
            prop_status::READONLY | prop_status::HIDDEN | prop_status::OUTPUT
        );
        assert_eq!(translate_status("2097152"), prop_status::DYNAMIC);
        assert_eq!(translate_status("4194304"), prop_status::NOPERSIST);
        assert_eq!(translate_status("8388608"), prop_status::NORECOMPUTE);
        assert_eq!(translate_status("16777216"), prop_status::READONLY);
        assert_eq!(translate_status("268435456"), prop_status::INPUT);
        assert_eq!(translate_status("not-a-number"), prop_status::NONE);
    }
}
