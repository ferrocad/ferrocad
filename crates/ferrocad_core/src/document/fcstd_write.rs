//! Writing FreeCAD's binary `.FCStd` container.
//!
//! This is the counterpart of [`super::fcstd::read_fcstd`], and the backend
//! behind every `.FCStd` save: [`super::format::write`] dispatches
//! `DocumentFormat::Fcstd` to [`write_fcstd`], so `saveAs` and `save` produce a
//! real archive rather than the native JSON envelope.
//!
//! # Members
//!
//! A written archive contains, in order:
//!
//! 1. `Document.xml` — the document, its objects and their dependencies;
//! 2. `GuiDocument.xml` — one view provider per object that has a
//!    [`ViewState`](super::ViewState);
//! 3. `PartShape.brp`, `PartShape1.brp`, … and `MeshKernel.bms`,
//!    `MeshKernel1.bms`, … — one member per property whose module value supplies
//!    `ExtensionData::fcstd_members`. The member stem, element tag and extension
//!    come from `ExtensionData::fcstd_kind` (default: `PartShapeN.brp` under
//!    `<Part>`; a mesh is `MeshKernelN.bms` under `<Mesh>`). A single value is
//!    `{stem}.{extension}` (referenced by `<{tag} file>`); a list is
//!    `PartShapeN.M.brp` (referenced by `<TopoShape file>` inside
//!    `<ShapeList>`). A single value may also supply an
//!    `ExtensionData::fcstd_sidecar`; core writes it as `{stem}.<suffix>`
//!    and references it from the same element with a `FerroCADSidecar`
//!    attribute, which upstream ignores. The primary member is untouched.
//! 4. one member per file-backed value, named after its property (uniquified on
//!    collision): `FloatList`, `VectorList`, `PlacementList` and `ColorList`
//!    members, each a LE `u32` count followed by fixed-width values — exactly
//!    upstream's `SaveDocFile` framing;
//! 5. `DiffuseColor`, `DiffuseColor1`, … — one little-endian `u32` count plus
//!    one packed `0xRRGGBB00` `u32` per per-face colour, referenced by
//!    `<ColorList file>`.
//!
//! There is no thumbnail. The ZIP is single-disk and unencrypted; every member
//! is deflated, there are no data descriptors and no ZIP64 records. Member
//! numbering follows object order, so the archive is deterministic.
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
//! | `VectorList` | `<VectorList file="…">` (member: `u32` count + `f64` xyz) |
//! | `Placement` | `<PropertyPlacement Px/Py/Pz Q0..Q3 A Ox Oy Oz>` |
//! | `PlacementList` | `<PlacementList file="…">` (member: `u32` count + `f64` xyz + `f64` q0..q3) |
//! | `Rotation` | `<PropertyRotation A Ox Oy Oz>` |
//! | `RotationList` | `<PropertyRotationList count><PropertyRotation A Ox Oy Oz>…` |
//! | `Matrix` | `<PropertyMatrix a11 … a44>` (row-major) |
//! | `Link` | `<Link value=…>` |
//! | `LinkList` | `<LinkList count><Link value=…>…` |
//! | `LinkSub` | `<LinkSub value=… count><Sub value=…>…` |
//! | `LinkSubList` | `<LinkSubList count><Link obj=… sub=…>…` (one `<Link>` per subname) |
//! | `StringList` | `<StringList count><String value=…>…` |
//! | `FloatList` | `<FloatList file="…">` (member: `u32` count + `f64`s) |
//! | `IntegerList` | `<IntegerList count><I v=…>…` |
//! | `BoolList` | `<BoolList value="bitset">` (element 0 is the last char) |
//! | `ColorList` | `<ColorList file="…">` (member: `u32` count + packed `0xRRGGBBAA`) |
//! | `IntPairList` | `<IntPairList count><Pair first=… second=…>…` |
//! | `Enumeration` | `<Integer value=index>` (+ `<CustomEnumList>` when the allowed values are known) |
//! | `IntegerConstraint` | `<Integer value min max step>` |
//! | `FloatConstraint` | `<Float value min max step>` |
//! | `Extension` | module payload: `<Part file>`, `<Mesh file>`, or `<ShapeList><TopoShape file>…` |
//!
//! In addition:
//!
//! - `<Objects>` carries `ObjectDeps` derived from `Link`/`LinkList`/`LinkSub`/
//!   `LinkSubList` properties and from `Object.Property` references in expression sources.
//! - The object's [`SavedObject::expressions`] are written as
//!   `<App::PropertyExpressionEngine><ExpressionEngine><Expression path
//!   expression>`, the same shape the reader's expression pass consumes.
//!   Self-relative nested paths regain upstream's leading `.`.
//! - [`SavedObject::property_status`] is translated back to upstream's internal
//!   `Property::Status` bits and written as the `status` attribute.
//! - A static transient property (present in `property_status`, absent from
//!   `properties`) is a valueless `<_Property>` placeholder, as upstream writes
//!   it. `SavedObject` has no type map, so the placeholder omits `type`; the
//!   reader ignores the element either way and the property falls back to its
//!   constructor default.
//! - [`SavedObject::extensions`] becomes `<Extensions><Extension type name>`.
//!
//! # `GuiDocument.xml`
//!
//! One `<ViewProvider name>` per object with a view, in object order:
//!
//! | [`ViewState`](super::ViewState) | Element |
//! | --- | --- |
//! | `visibility` | `<Property name="Visibility" type="App::PropertyBool"><Bool value=…>` |
//! | `transparency` | `<Property name="Transparency" type="App::PropertyInteger"><Integer value=…>` |
//! | `effective_color()` | `<Property name="ShapeColor" type="App::PropertyColor"><PropertyColor value="0xRRGGBB00">` |
//! | `diffuse_color` (when set) | `<Property name="DiffuseColor" type="App::PropertyColorList"><ColorList file="DiffuseColorN">` |
//! | `display_mode` | `<Property name="DisplayMode" type="App::PropertyEnumeration"><Integer value="0">` |
//!
//! `DisplayMode` needs a name→index table per view-provider type; none exists in
//! the model yet, so index 0 is written (the first entry of every upstream
//! provider's mode list). `ShapeAppearance` and `ShapeMaterial` are not written.
//! `DiffuseColor` is only written when the view state has explicit per-face
//! colours; `None` means "derive from the shape" and must not be fabricated.
//!
//! # Deliberately unsupported
//!
//! - [`Property::FileIncluded`] errors with
//!   [`FcstdWriteError::UnsupportedProperty`] naming the object and property: it
//!   needs an archive member plus a transient directory.
//! - [`Property::PythonObject`] stores a base64 pickle of the assigned value, but
//!   our reader treats every `App::PropertyPythonObject` as the object's proxy
//!   state rather than as a property (see `fcstd`), so writing it as upstream's
//!   `<Python value>` would restore the wrong thing. It is skipped and reported in
//!   [`WriteReport::dropped`] instead of being written misleadingly.
//! - A module value whose `ExtensionData::fcstd_members` is `None` errors with
//!   [`FcstdWriteError::UnsupportedExtension`] naming its type.
//! - `python_state` is **not** written. FerroCAD's state is a base64 pickle of
//!   its own `{"attrs", "proxy"}` envelope (see `capture_object_state`), not the
//!   proxy pickle upstream's `<Python value="…">` field expects; writing it
//!   verbatim would make upstream restore the wrong thing. It is reported in
//!   [`WriteReport::dropped`] instead of being lost silently.
//!
//! Property type ids are derived from the value: a `String` is written as
//! `App::PropertyString` (a `PropertyFile` cannot be recovered) and a `Quantity`
//! picks `PropertyLength`/`PropertyAngle` from its unit signature (areas and
//! volumes fall back to `PropertyLength`). `SavedObject::label` is
//! authoritative: a `Label` property is always emitted from it, so a hand-built
//! `SavedDocument` whose `properties` map has no `Label` entry still round-trips.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};

use flate2::write::DeflateEncoder;
use flate2::Compression;

use super::{SavedDocument, SavedObject};
use crate::expr;
use crate::extension::FcstdKind;
use crate::geometry::Rotation;
use crate::property::Property;
use crate::quantity::Quantity;
use crate::unit::Unit;

/// The `Document` schema version this writer emits (upstream 0.21–1.x writes 4).
pub const DOCUMENT_SCHEMA_VERSION: u32 = 4;

/// The `Document` file version this writer emits.
pub const DOCUMENT_FILE_VERSION: u32 = 1;

/// The `GuiDocument` schema version this writer emits (upstream writes 1).
pub const GUI_DOCUMENT_SCHEMA_VERSION: u32 = 1;

/// `ProgramVersion` written into `Document.xml`.
pub const PROGRAM_VERSION: &str = concat!("FerroCAD ", env!("CARGO_PKG_VERSION"));

/// What a successful save could not represent.
///
/// Nothing is dropped silently: a value without an upstream `.FCStd`
/// representation is either an error (properties, extension payloads) or listed
/// here (Python state, see [`WriteReport::dropped`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WriteReport {
    dropped: Vec<String>,
}

impl WriteReport {
    /// One human-readable note per value that was skipped.
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }

    /// Whether anything was skipped.
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty()
    }
}

/// Errors raised while writing a binary `.FCStd` file.
#[derive(Debug)]
pub enum FcstdWriteError {
    /// The file or an archive member could not be written.
    Io(io::Error),
    /// The archive would need ZIP64 (over 4 GiB, or over 65535 members).
    TooLarge(String),
    /// A module-owned property has no upstream `.FCStd` representation.
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
                "cannot write '{object}.{property}' of type '{type_name}': this module \
                 value has no .FCStd payload (its ExtensionData::fcstd_members returned \
                 None), so the save would lose it"
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
/// This is what [`super::format::write`] calls for `DocumentFormat::Fcstd`. Use
/// [`write_fcstd_with_report`] to learn what was skipped.
pub fn write_fcstd(doc: &SavedDocument, path: &str) -> Result<(), FcstdWriteError> {
    write_fcstd_with_report(doc, path).map(|_| ())
}

/// Like [`write_fcstd`], but returns the report of skipped values.
pub fn write_fcstd_with_report(
    doc: &SavedDocument,
    path: &str,
) -> Result<WriteReport, FcstdWriteError> {
    let (bytes, report) = encode_fcstd_with_report(doc)?;
    std::fs::write(path, bytes)?;
    Ok(report)
}

/// Build the `.FCStd` archive in memory (the file-writing half is [`write_fcstd`]).
pub fn encode_fcstd(doc: &SavedDocument) -> Result<Vec<u8>, FcstdWriteError> {
    encode_fcstd_with_report(doc).map(|(bytes, _)| bytes)
}

/// Build the archive and return what could not be represented.
pub fn encode_fcstd_with_report(
    doc: &SavedDocument,
) -> Result<(Vec<u8>, WriteReport), FcstdWriteError> {
    let mut ctx = Context::default();
    let xml = document_xml(doc, &mut ctx)?;
    let gui_xml = gui_document_xml(doc, &mut ctx);

    let mut zip = ZipWriter::new();
    zip.add("Document.xml", xml.as_bytes())?;
    zip.add("GuiDocument.xml", gui_xml.as_bytes())?;
    for member in ctx
        .payload_members
        .iter()
        .chain(&ctx.data_members)
        .chain(&ctx.colour_members)
    {
        zip.add(&member.name, &member.data)?;
    }
    Ok((zip.finish()?, ctx.report))
}

// ---------------------------------------------------------------------------
// Document.xml
// ---------------------------------------------------------------------------

struct Member {
    name: String,
    data: Vec<u8>,
}

/// State threaded through the two XML writers: member allocators (stable,
/// deterministic numbering in object order) and the drop report.
#[derive(Default)]
struct Context {
    payload_members: Vec<Member>,
    data_members: Vec<Member>,
    colour_members: Vec<Member>,
    /// Number of payloads already written per `(stem, extension)`, reproducing
    /// upstream `Writer::getUniqueFileName`'s per-extension numbering
    /// (`MeshKernel.bms`, `MeshKernel1.bms`, …).
    payload_counts: BTreeMap<(&'static str, &'static str), usize>,
    colour_index: usize,
    report: WriteReport,
}

impl Context {
    /// Allocate the next unique stem for `kind`, upstream's `Writer::addFile`
    /// shape: the member is named after the payload and numbered when taken.
    fn add_payload_stem(&mut self, kind: &FcstdKind) -> String {
        let key = (kind.stem, kind.extension);
        let mut count = self.payload_counts.get(&key).copied().unwrap_or(0);
        let stem = loop {
            let candidate = if count == 0 {
                kind.stem.to_string()
            } else {
                format!("{}{count}", kind.stem)
            };
            count += 1;
            if !self.member_name_taken(&format!("{candidate}.{}", kind.extension)) {
                break candidate;
            }
        };
        self.payload_counts.insert(key, count);
        stem
    }

    /// Place a value in its own archive member, upstream's `Writer::addFile` shape:
    /// the member is named after the property, uniquified when taken.
    fn add_data_member(&mut self, base: &str, data: Vec<u8>) -> String {
        let mut name = base.to_string();
        let mut suffix = 1;
        while self.member_name_taken(&name) {
            name = format!("{base}{suffix}");
            suffix += 1;
        }
        self.data_members.push(Member {
            name: name.clone(),
            data,
        });
        name
    }

    fn member_name_taken(&self, name: &str) -> bool {
        name == "Document.xml"
            || name == "GuiDocument.xml"
            || self
                .payload_members
                .iter()
                .chain(&self.data_members)
                .chain(&self.colour_members)
                .any(|member| member.name == name)
    }
}

fn document_xml(doc: &SavedDocument, ctx: &mut Context) -> Result<String, FcstdWriteError> {
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

    // `<Objects>` carries the type ids and the dependency graph;
    // `<ObjectData>` carries the values.
    xml.push_str(&format!(
        "    <Objects Count=\"{}\" Dependencies=\"1\">\n",
        doc.objects.len()
    ));
    for object in &doc.objects {
        let deps = object_deps(doc, object);
        if deps.is_empty() {
            xml.push_str(&format!(
                "        <ObjectDeps Name=\"{}\" Count=\"0\"/>\n",
                escape_attr(&object.name)
            ));
        } else {
            xml.push_str(&format!(
                "        <ObjectDeps Name=\"{}\" Count=\"{}\">\n",
                escape_attr(&object.name),
                deps.len()
            ));
            for dep in deps {
                xml.push_str(&format!(
                    "            <Dep Name=\"{}\"/>\n",
                    escape_attr(&dep)
                ));
            }
            xml.push_str("        </ObjectDeps>\n");
        }
    }
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
        write_object(&mut xml, object, ctx)?;
    }
    xml.push_str("    </ObjectData>\n");
    xml.push_str("</Document>\n");
    Ok(xml)
}

/// Direct dependencies of `object`, in property order then expression order.
///
/// Links to unknown names and to the object itself are skipped: upstream only
/// ever writes existing objects, and a self-edge would make the graph cyclic.
/// Duplicates are kept, as upstream keeps one `<Dep>` per link.
fn object_deps(doc: &SavedDocument, object: &SavedObject) -> Vec<String> {
    let mut deps = Vec::new();
    for (name, value) in &object.properties {
        if name == "Label" {
            continue;
        }
        match value {
            Property::Link(target) => {
                if target != &object.name && doc.objects.iter().any(|o| &o.name == target) {
                    deps.push(target.clone());
                }
            }
            Property::LinkList(targets) => {
                for target in targets {
                    if target != &object.name && doc.objects.iter().any(|o| &o.name == target) {
                        deps.push(target.clone());
                    }
                }
            }
            Property::LinkSub(target, _) => {
                if target != &object.name && doc.objects.iter().any(|o| &o.name == target) {
                    deps.push(target.clone());
                }
            }
            Property::LinkSubList(entries) => {
                for (target, _) in entries {
                    if target != &object.name && doc.objects.iter().any(|o| &o.name == target) {
                        deps.push(target.clone());
                    }
                }
            }
            _ => {}
        }
    }
    for source in object.expressions.values() {
        if let Ok(parsed) = expr::parse(source) {
            let mut refs = Vec::new();
            expression_object_refs(&parsed, &mut refs);
            for target in refs {
                if target != object.name && doc.objects.iter().any(|o| o.name == target) {
                    deps.push(target);
                }
            }
        }
    }
    deps
}

/// Object names referenced by `Object.Property` paths in an expression
/// (self-relative `.<prop>` paths reference the object itself).
fn expression_object_refs(expr: &expr::Expr, out: &mut Vec<String>) {
    match expr {
        expr::Expr::Number(_) => {}
        expr::Expr::Var(name) => {
            if name.starts_with('.') {
                return;
            }
            if let Some((object, _)) = name.split_once('.') {
                if !object.is_empty() && !out.iter().any(|n| n == object) {
                    out.push(object.to_string());
                }
            }
        }
        expr::Expr::UnaryNeg(inner) => expression_object_refs(inner, out),
        expr::Expr::Binary(left, _, right) => {
            expression_object_refs(left, out);
            expression_object_refs(right, out);
        }
    }
}

fn write_object(
    out: &mut String,
    object: &SavedObject,
    ctx: &mut Context,
) -> Result<(), FcstdWriteError> {
    // `Label` is always regenerated from `object.label` (see the module docs).
    let properties: Vec<(&String, &Property)> = object
        .properties
        .iter()
        .filter(|(name, _)| name.as_str() != "Label")
        .collect();

    // Static transient properties have a status but no value: upstream writes a
    // valueless `_Property` so the property falls back to its constructor
    // default on restore. `SavedObject` keeps no type id for them, so `type` is
    // omitted (the reader ignores `_Property` either way).
    let placeholders: Vec<(&String, u32)> = object
        .property_status
        .iter()
        .filter(|(name, _)| name.as_str() != "Label" && !object.properties.contains_key(*name))
        .map(|(name, &status)| (name, status))
        .collect();

    let has_expressions = !object.expressions.is_empty();
    let count = properties.len() + placeholders.len() + 1 + usize::from(has_expressions);

    let extensions = if object.extensions.is_empty() {
        String::new()
    } else {
        " Extensions=\"True\"".to_string()
    };
    out.push_str(&format!(
        "        <Object name=\"{}\"{extensions}>\n",
        escape_attr(&object.name)
    ));

    if !object.extensions.is_empty() {
        out.push_str(&format!(
            "            <Extensions Count=\"{}\">\n",
            object.extensions.len()
        ));
        for extension in &object.extensions {
            // Our model stores only the registered type id; upstream's element
            // also carries the class name, which is the part after `::`.
            let short = extension.rsplit("::").next().unwrap_or(extension);
            out.push_str(&format!(
                "                <Extension type=\"{}\" name=\"{}\">\n                </Extension>\n",
                escape_attr(extension),
                escape_attr(short)
            ));
        }
        out.push_str("            </Extensions>\n");
    }

    out.push_str(&format!(
        "            <Properties Count=\"{count}\" TransientCount=\"{}\">\n",
        placeholders.len()
    ));
    for (name, status) in &placeholders {
        out.push_str(&format!(
            "                <_Property name=\"{}\" status=\"{}\"/>\n",
            escape_attr(name),
            upstream_status(*status)
        ));
    }
    out.push_str(&format!(
        "                <Property name=\"Label\" type=\"App::PropertyString\">\
         <String value=\"{}\"/></Property>\n",
        escape_attr(&object.label)
    ));
    for (name, value) in properties {
        let status = object.property_status.get(name).copied().unwrap_or(0);
        write_property(out, object, name, value, status, ctx)?;
    }
    if has_expressions {
        write_expressions(out, object);
    }
    out.push_str("            </Properties>\n");
    out.push_str("        </Object>\n");

    if object.python_state.is_some() {
        ctx.report.dropped.push(format!(
            "{}.python_state: FerroCAD stores a base64 pickle of its own \
             {{\"attrs\", \"proxy\"}} envelope, not the proxy pickle upstream's \
             <Python value> field expects; skipped",
            object.name
        ));
    }
    Ok(())
}

/// `<App::PropertyExpressionEngine>` matching
/// [`super::fcstd::read_expressions`]: nested self-relative paths regain the
/// leading `.` that our map strips.
fn write_expressions(out: &mut String, object: &SavedObject) {
    out.push_str(
        "                <Property name=\"ExpressionEngine\" type=\"App::PropertyExpressionEngine\">\n",
    );
    out.push_str(&format!(
        "                    <ExpressionEngine count=\"{}\">\n",
        object.expressions.len()
    ));
    for (path, source) in &object.expressions {
        let path = if path.contains('.') {
            format!(".{path}")
        } else {
            path.clone()
        };
        out.push_str(&format!(
            "                        <Expression path=\"{}\" expression=\"{}\"/>\n",
            escape_attr(&path),
            escape_attr(source)
        ));
    }
    out.push_str("                    </ExpressionEngine>\n");
    out.push_str("                </Property>\n");
}

/// Translate our public status mask back to upstream's internal
/// `Property::Status` bit positions (the inverse of
/// [`super::fcstd`]'s `translate_status`).
fn upstream_status(mask: u32) -> u64 {
    use crate::prop_status as ps;

    let mut raw = 0u64;
    let mut set = |flag: u32, bit: u32| {
        if mask & flag != 0 {
            raw |= 1u64 << bit;
        }
    };
    set(ps::DYNAMIC, 21); // PropDynamic
    set(ps::NOPERSIST, 22); // PropNoPersist
    set(ps::NORECOMPUTE, 23); // PropNoRecompute
    set(ps::READONLY, 24); // PropReadOnly
    set(ps::TRANSIENT, 25); // PropTransient
    set(ps::HIDDEN, 26); // PropHidden
    set(ps::OUTPUT, 27); // PropOutput
    set(ps::INPUT, 28); // PropInput
    raw
}

fn status_attr(status: u32) -> String {
    let raw = upstream_status(status);
    if raw == 0 {
        String::new()
    } else {
        format!(" status=\"{raw}\"")
    }
}

fn write_property(
    out: &mut String,
    object: &SavedObject,
    name: &str,
    value: &Property,
    status: u32,
    ctx: &mut Context,
) -> Result<(), FcstdWriteError> {
    let escaped = escape_attr(name);
    let status = status_attr(status);
    match value {
        Property::String(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyString\"{status}>\
             <String value=\"{}\"/></Property>\n",
            escape_attr(v)
        )),
        Property::Integer(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyInteger\"{status}>\
             <Integer value=\"{v}\"/></Property>\n"
        )),
        Property::Float(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyFloat\"{status}>\
             <Float value=\"{}\"/></Property>\n",
            number(*v)
        )),
        Property::Bool(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyBool\"{status}>\
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
                "                <Property name=\"{escaped}\" type=\"{type_name}\"{status}>\
                 <Float value=\"{}\"/></Property>\n",
                number(q.value())
            ));
        }
        Property::Vector(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyVector\"{status}>\
             <PropertyVector valueX=\"{}\" valueY=\"{}\" valueZ=\"{}\"/></Property>\n",
            number(v.x),
            number(v.y),
            number(v.z)
        )),
        // Upstream stores these lists in their own archive member: a LE `u32`
        // count followed by fixed-width `f64`s. An empty list writes
        // `file=""` and no member, as upstream's `addFile` call is skipped too.
        Property::VectorList(values) => {
            let numbers: Vec<f64> = values
                .iter()
                .flat_map(|v| [v.x, v.y, v.z])
                .collect();
            write_member_list(
                out,
                &escaped,
                "App::PropertyVectorList",
                &status,
                "VectorList",
                name,
                values.len(),
                &numbers,
                ctx,
            );
        }
        Property::Rotation(r) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyRotation\"{status}>\
             <PropertyRotation {}/></Property>\n",
            raw_rotation_attributes(r)
        )),
        Property::RotationList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyRotationList\"{status}>\n"
            ));
            out.push_str(&format!(
                "                    <PropertyRotationList count=\"{}\">\n",
                values.len()
            ));
            for r in values {
                out.push_str(&format!(
                    "                        <PropertyRotation {}/>\n",
                    raw_rotation_attributes(r)
                ));
            }
            out.push_str("                    </PropertyRotationList>\n");
            out.push_str("                </Property>\n");
        }
        Property::Matrix(m) => {
            let mut attributes = String::new();
            for row in 0..4 {
                for column in 0..4 {
                    attributes.push_str(&format!(
                        " a{}{}=\"{}\"",
                        row + 1,
                        column + 1,
                        number(m.m[row * 4 + column])
                    ));
                }
            }
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyMatrix\"{status}>\
                 <PropertyMatrix{attributes}/></Property>\n"
            ));
        }
        Property::PlacementList(values) => {
            let numbers: Vec<f64> = values
                .iter()
                .flat_map(|p| {
                    let rotation = &p.rotation;
                    [
                        p.base.x,
                        p.base.y,
                        p.base.z,
                        // Upstream's quaternion is x, y, z, w; ours is [w, x, y, z].
                        rotation.q[1],
                        rotation.q[2],
                        rotation.q[3],
                        rotation.q[0],
                    ]
                })
                .collect();
            write_member_list(
                out,
                &escaped,
                "App::PropertyPlacementList",
                &status,
                "PlacementList",
                name,
                values.len(),
                &numbers,
                ctx,
            );
        }
        Property::Placement(p) => {
            // Our quaternion is [w, x, y, z]; upstream stores Q0=x … Q3=w.
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyPlacement\"{status}>\
                 <PropertyPlacement Px=\"{}\" Py=\"{}\" Pz=\"{}\" {}/></Property>\n",
                number(p.base.x),
                number(p.base.y),
                number(p.base.z),
                rotation_attributes(&p.rotation)
            ));
        }
        Property::Link(v) => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyLink\"{status}>\
             <Link value=\"{}\"/></Property>\n",
            escape_attr(v)
        )),
        Property::LinkList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyLinkList\"{status}>\n"
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
                "                <Property name=\"{escaped}\" type=\"App::PropertyLinkSub\"{status}>\n"
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
        Property::LinkSubList(entries) => {
            // Upstream stores one flat `<Link>` per sub-element and regroups
            // consecutive entries naming the same object on read; a whole-object
            // entry is `<Link obj="…" sub=""/>`.
            let count: usize = entries
                .iter()
                .map(|(_, subs)| subs.len().max(1))
                .sum();
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyLinkSubList\"{status}>\n"
            ));
            out.push_str(&format!("                    <LinkSubList count=\"{count}\">\n"));
            for (target, subs) in entries {
                if subs.is_empty() {
                    out.push_str(&format!(
                        "                        <Link obj=\"{}\" sub=\"\"/>\n",
                        escape_attr(target)
                    ));
                } else {
                    for sub in subs {
                        out.push_str(&format!(
                            "                        <Link obj=\"{}\" sub=\"{}\"/>\n",
                            escape_attr(target),
                            escape_attr(sub)
                        ));
                    }
                }
            }
            out.push_str("                    </LinkSubList>\n");
            out.push_str("                </Property>\n");
        }
        Property::StringList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyStringList\"{status}>\n"
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
        Property::FloatList(values) => {
            write_member_list(
                out,
                &escaped,
                "App::PropertyFloatList",
                &status,
                "FloatList",
                name,
                values.len(),
                values,
                ctx,
            );
        }
        Property::IntegerList(values) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyIntegerList\"{status}>\n"
            ));
            out.push_str(&format!(
                "                    <IntegerList count=\"{}\">\n",
                values.len()
            ));
            for v in values {
                out.push_str(&format!("                        <I v=\"{v}\"/>\n"));
            }
            out.push_str("                    </IntegerList>\n");
            out.push_str("                </Property>\n");
        }
        Property::BoolList(values) => {
            // Upstream writes `boost::dynamic_bitset` as a string: the *last* char
            // is element 0.
            let bitset: String = values
                .iter()
                .rev()
                .map(|v| if *v { '1' } else { '0' })
                .collect();
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyBoolList\"{status}>\
                 <BoolList value=\"{bitset}\"/></Property>\n"
            ));
        }
        Property::ColorList(colors) => {
            // `PropertyColorList::SaveDocFile`: a LE `u32` count, then one packed
            // `0xRRGGBBAA` per colour.
            if colors.is_empty() {
                out.push_str(&format!(
                    "                <Property name=\"{escaped}\" type=\"App::PropertyColorList\"{status}>\n                    <ColorList file=\"\"/>\n                </Property>\n"
                ));
            } else {
                let mut data = Vec::with_capacity(4 + 4 * colors.len());
                put_u32(&mut data, colors.len() as u32);
                for color in colors {
                    put_u32(&mut data, super::color_to_packed(*color));
                }
                let member = ctx.add_data_member(name, data);
                out.push_str(&format!(
                    "                <Property name=\"{escaped}\" type=\"App::PropertyColorList\"{status}>\n                    <ColorList file=\"{}\"/>\n                </Property>\n",
                    escape_attr(&member)
                ));
            }
        }
        Property::IntPairList(pairs) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyIntPairList\"{status}>\n"
            ));
            out.push_str(&format!(
                "                    <IntPairList count=\"{}\">\n",
                pairs.len()
            ));
            for (first, second) in pairs {
                out.push_str(&format!(
                    "                        <Pair first=\"{first}\" second=\"{second}\"/>\n"
                ));
            }
            out.push_str("                    </IntPairList>\n");
            out.push_str("                </Property>\n");
        }
        // A constraint persists its value and its range (upstream writes `min`,
        // `max` and `step` when the constraint is user-settable).
        Property::IntegerConstraint {
            value,
            min,
            max,
            step,
        } => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyIntegerConstraint\"{status}>\
             <Integer value=\"{value}\" min=\"{min}\" max=\"{max}\" step=\"{step}\"/></Property>\n"
        )),
        Property::FloatConstraint {
            value,
            min,
            max,
            step,
        } => out.push_str(&format!(
            "                <Property name=\"{escaped}\" type=\"App::PropertyFloatConstraint\"{status}>\
             <Float value=\"{}\" min=\"{}\" max=\"{}\" step=\"{}\"/></Property>\n",
            number(*value),
            number(*min),
            number(*max),
            number(*step)
        )),
        Property::Enumeration(values, index) => {
            out.push_str(&format!(
                "                <Property name=\"{escaped}\" type=\"App::PropertyEnumeration\"{status}>\n"
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
            // The module owns the encoding, the member kind and the element form;
            // core only allocates a unique stem and places the bytes verbatim.
            let Some(members) = value.data().fcstd_members() else {
                return Err(FcstdWriteError::UnsupportedExtension {
                    object: object.name.clone(),
                    property: name.to_string(),
                    type_name: type_name.to_string(),
                });
            };
            let kind = value.data().fcstd_kind();
            let stem = ctx.add_payload_stem(&kind);

            if members.len() == 1 && members[0].0.is_empty() {
                let member = format!("{stem}.{}", kind.extension);
                ctx.payload_members.push(Member {
                    name: member.clone(),
                    data: members.into_iter().next().unwrap().1,
                });
                // A sidecar is module metadata (a lineage, for Part) that must
                // not contaminate the upstream-compatible payload member. Upstream
                // reads only the attributes it knows, so the extra reference is
                // ignored; our reader follows it when present.
                let sidecar = value
                    .data()
                    .fcstd_sidecar()
                    .map(|(suffix, data)| {
                        let mut sidecar = format!("{stem}.{suffix}");
                        let mut serial = 1;
                        while ctx.member_name_taken(&sidecar) {
                            sidecar = format!("{stem}.{suffix}{serial}");
                            serial += 1;
                        }
                        ctx.payload_members.push(Member {
                            name: sidecar.clone(),
                            data,
                        });
                        sidecar
                    });
                let sidecar = match sidecar {
                    Some(name) => format!(" FerroCADSidecar=\"{}\"", escape_attr(&name)),
                    None => String::new(),
                };
                out.push_str(&format!(
                    "                <Property name=\"{escaped}\" type=\"{}\"{status}>\
                     <{tag} file=\"{member}\"{sidecar}/></Property>\n",
                    escape_attr(type_name),
                    tag = kind.tag,
                ));
            } else {
                out.push_str(&format!(
                    "                <Property name=\"{escaped}\" type=\"{}\"{status}>\n",
                    escape_attr(type_name)
                ));
                out.push_str(&format!(
                    "                    <ShapeList count=\"{}\">\n",
                    members.len()
                ));
                for (suffix, data) in members {
                    let member = format!("{stem}.{suffix}.{}", kind.extension);
                    ctx.payload_members.push(Member {
                        name: member.clone(),
                        data,
                    });
                    out.push_str(&format!(
                        "                        <TopoShape file=\"{member}\"/>\n"
                    ));
                }
                out.push_str("                    </ShapeList>\n");
                out.push_str("                </Property>\n");
            }
        }
        // A `PythonObject` is read back as proxy state rather than as a property
        // (see `fcstd`) and has no compatible `<Python value>` form here, so it is
        // reported as dropped instead of writing a misleading payload.
        Property::PythonObject(_) => {
            ctx.report.dropped.push(format!(
                "{}.{}: App::PropertyPythonObject is read back as proxy state, not a \
                 property; skipped rather than written as a misleading <Python value>",
                object.name, name
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

// ---------------------------------------------------------------------------
// GuiDocument.xml
// ---------------------------------------------------------------------------

fn gui_document_xml(doc: &SavedDocument, ctx: &mut Context) -> String {
    let views: Vec<(&SavedObject, &super::ViewState)> = doc
        .objects
        .iter()
        .filter_map(|object| object.view.as_ref().map(|view| (object, view)))
        .collect();

    let mut xml = String::new();
    xml.push_str("<?xml version='1.0' encoding='utf-8'?>\n");
    xml.push_str(&format!(
        "<Document SchemaVersion=\"{GUI_DOCUMENT_SCHEMA_VERSION}\">\n"
    ));
    xml.push_str(&format!(
        "    <ViewProviderData Count=\"{}\">\n",
        views.len()
    ));
    for (object, view) in views {
        let colour = hex_colour(view.effective_color());
        let diffuse = view.diffuse_color.as_ref();
        let count = 4 + usize::from(diffuse.is_some());

        xml.push_str(&format!(
            "        <ViewProvider name=\"{}\">\n",
            escape_attr(&object.name)
        ));
        xml.push_str(&format!(
            "            <Properties Count=\"{count}\" TransientCount=\"0\">\n"
        ));
        xml.push_str(&format!(
            "                <Property name=\"Visibility\" type=\"App::PropertyBool\">\
             <Bool value=\"{}\"/></Property>\n",
            if view.visibility { "true" } else { "false" }
        ));
        xml.push_str(&format!(
            "                <Property name=\"Transparency\" type=\"App::PropertyInteger\">\
             <Integer value=\"{}\"/></Property>\n",
            view.transparency
        ));
        xml.push_str(&format!(
            "                <Property name=\"ShapeColor\" type=\"App::PropertyColor\">\
             <PropertyColor value=\"{colour}\"/></Property>\n"
        ));
        if let Some(colors) = diffuse {
            let member = if ctx.colour_index == 0 {
                "DiffuseColor".to_string()
            } else {
                format!("DiffuseColor{}", ctx.colour_index)
            };
            ctx.colour_index += 1;

            // Upstream `PropertyColorList::SaveDocFile`: a little-endian u32
            // count, then one packed `0xRRGGBB00` u32 per colour. (Verified
            // against TestTangentMode3-0.21.FCStd: count 1 then 00 80 79 72,
            // i.e. 0x72798000.)
            let mut data = Vec::with_capacity(4 * (colors.len() + 1));
            put_u32(&mut data, colors.len() as u32);
            for color in colors {
                put_u32(&mut data, packed_rgb(*color));
            }
            ctx.colour_members.push(Member {
                name: member.clone(),
                data,
            });

            xml.push_str(&format!(
                "                <Property name=\"DiffuseColor\" type=\"App::PropertyColorList\">\
                 <ColorList file=\"{member}\"/></Property>\n"
            ));
        }
        // DisplayMode is an index into the view provider's mode list. Our model
        // stores the name, not the index, and no per-type table exists yet, so
        // write 0 (the first mode of every upstream provider).
        xml.push_str(
            "                <Property name=\"DisplayMode\" type=\"App::PropertyEnumeration\">\
             <Integer value=\"0\"/></Property>\n",
        );
        xml.push_str("            </Properties>\n");
        xml.push_str("        </ViewProvider>\n");
    }
    xml.push_str("    </ViewProviderData>\n");
    xml.push_str("</Document>\n");
    xml
}

/// `0xRRGGBB00` as upstream writes `PropertyColor` values (alpha byte zero).
fn hex_colour(color: super::Color) -> String {
    format!("0x{:08X}", packed_rgb(color))
}

/// Pack an RGBA colour as `0xRRGGBB00`: the RGB bytes, with the alpha byte
/// cleared (upstream's colour members ignore alpha here).
fn packed_rgb(color: super::Color) -> u32 {
    super::color_to_packed(color) & 0xFFFF_FF00
}

fn is_angle(q: &Quantity) -> bool {
    let deg = Unit::parse("deg").expect("'deg' is a known unit");
    q.sig() == deg.sig
}

/// The shared `Q0..Q3 A Ox Oy Oz` attribute set of `PropertyPlacement` and
/// `PropertyRotation` (upstream stores Q0=x … Q3=w; ours is `[w, x, y, z]`).
fn rotation_attributes(rotation: &Rotation) -> String {
    let axis = rotation.axis();
    format!(
        "Q0=\"{}\" Q1=\"{}\" Q2=\"{}\" Q3=\"{}\" A=\"{}\" Ox=\"{}\" Oy=\"{}\" Oz=\"{}\"",
        number(rotation.q[1]),
        number(rotation.q[2]),
        number(rotation.q[3]),
        number(rotation.q[0]),
        number(rotation.angle()),
        number(axis.x),
        number(axis.y),
        number(axis.z)
    )
}

/// `PropertyRotation` persists the raw axis and angle only (no quaternion).
fn raw_rotation_attributes(rotation: &Rotation) -> String {
    let axis = rotation.axis();
    format!(
        "A=\"{}\" Ox=\"{}\" Oy=\"{}\" Oz=\"{}\"",
        number(rotation.angle()),
        number(axis.x),
        number(axis.y),
        number(axis.z)
    )
}

/// Write a file-backed numeric list, upstream's `SaveDocFile` shape: a LE `u32`
/// `count`, then `values` as LE `f64`s in a member named after the property.
///
/// An empty list writes `file=""` and allocates no member, matching upstream's
/// `getSize() ? addFile(…) : ""`.
#[allow(clippy::too_many_arguments)]
fn write_member_list(
    out: &mut String,
    escaped: &str,
    type_name: &str,
    status: &str,
    tag: &str,
    name: &str,
    count: usize,
    values: &[f64],
    ctx: &mut Context,
) {
    out.push_str(&format!(
        "                <Property name=\"{escaped}\" type=\"{type_name}\"{status}>\n"
    ));
    if values.is_empty() {
        out.push_str(&format!("                    <{tag} file=\"\"/>\n"));
    } else {
        let mut data = Vec::with_capacity(4 + 8 * values.len());
        put_u32(&mut data, count as u32);
        for value in values {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let member = ctx.add_data_member(name, data);
        out.push_str(&format!(
            "                    <{tag} file=\"{}\"/>\n",
            escape_attr(&member)
        ));
    }
    out.push_str("                </Property>\n");
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
        fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
            Some(vec![(String::new(), self.0.clone())])
        }
        fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "FakeShape({} bytes)", self.0.len())
        }
    }

    /// A module value with the Mesh `.FCStd` kind: `MeshKernelN.bms` under
    /// `<Mesh>`. The payload bytes are opaque to core.
    struct FakeMesh(Vec<u8>);

    impl ExtensionData for FakeMesh {
        fn type_name(&self) -> &str {
            "Mesh::PropertyMeshKernel"
        }
        fn clone_box(&self) -> Box<dyn ExtensionData> {
            Box::new(FakeMesh(self.0.clone()))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn eq(&self, other: &dyn ExtensionData) -> bool {
            other
                .as_any()
                .downcast_ref::<FakeMesh>()
                .is_some_and(|o| o.0 == self.0)
        }
        fn save(&self) -> Vec<u8> {
            self.0.clone()
        }
        fn fcstd_members(&self) -> Option<Vec<(String, Vec<u8>)>> {
            Some(vec![(String::new(), self.0.clone())])
        }
        fn fcstd_kind(&self) -> FcstdKind {
            FcstdKind::new("MeshKernel", "Mesh", "bms")
        }
        fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "FakeMesh({} bytes)", self.0.len())
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

    /// A `PropertyPythonObject` is read back as proxy state, not as a property,
    /// so it must be reported as dropped rather than written as a `<Python value>`
    /// that upstream would restore as the wrong thing.
    #[test]
    fn reports_a_python_object_property_as_dropped() {
        let mut saved = SavedDocument {
            format: "ferrocad".to_string(),
            version: 1,
            name: "Python".to_string(),
            objects: Vec::new(),
        };
        let mut o = object("Box", "Box");
        o.properties.insert(
            "Proxy".into(),
            Property::PythonObject("gASVBAAAAAAAAAB9lC4=".into()),
        );
        saved.objects.push(o);

        let (bytes, report) = encode_fcstd_with_report(&saved).expect("encode");
        assert!(
            report
                .dropped()
                .iter()
                .any(|note| note.contains("Box.Proxy")),
            "the dropped Python value is reported: {:?}",
            report.dropped()
        );
        let xml = String::from_utf8(read_member(&bytes, "Document.xml")).unwrap();
        assert!(!xml.contains("App::PropertyPythonObject"), "{xml}");
        assert!(!xml.contains("<Python"), "{xml}");
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
            "Support".into(),
            Property::LinkSubList(vec![
                ("Tool".into(), vec!["Face1".into(), "Face2".into()]),
                ("Box".into(), Vec::new()),
            ]),
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
            ["Document.xml", "GuiDocument.xml", "PartShape.brp"]
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
        assert!(xml.contains("<Objects Count=\"1\" Dependencies=\"1\">"));
        assert!(xml.contains("<ObjectDeps Name=\"Box\" Count=\"0\"/>"));

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
        assert!(xml.contains("type=\"App::PropertyLinkSubList\">"));
        assert!(xml.contains("<LinkSubList count=\"3\">"));
        assert!(xml.contains("<Link obj=\"Tool\" sub=\"Face1\"/>"));
        assert!(xml.contains("<Link obj=\"Tool\" sub=\"Face2\"/>"));
        assert!(xml.contains("<Link obj=\"Box\" sub=\"\"/>"));
        assert!(xml.contains("<StringList count=\"2\">"));
        assert!(xml.contains("<Integer value=\"2\" CustomEnum=\"true\"/>"));
        assert!(xml.contains("<CustomEnumList count=\"3\">"));
        // A constructor-default enum writes only the index.
        assert!(xml.contains("<Property name=\"MapMode\" type=\"App::PropertyEnumeration\">"));
        assert!(xml.contains("<Integer value=\"5\"/>"));
        assert!(xml.contains("<Part file=\"PartShape.brp\"/>"));
        assert!(!xml.contains("ExpressionEngine"), "no expressions in this sample");
        assert!(
            !xml.contains("App::PropertyPythonObject"),
            "python_state is reported, not written"
        );

        // The XML parses (a cheap well-formedness check).
        roxmltree::Document::parse(&xml).expect("well-formed Document.xml");

        let gui = read_member(&bytes, "GuiDocument.xml");
        let gui = String::from_utf8(gui).unwrap();
        roxmltree::Document::parse(&gui).expect("well-formed GuiDocument.xml");
        assert!(gui.contains("<ViewProviderData Count=\"0\">"));

        // The shape member holds the module bytes verbatim.
        assert_eq!(read_member(&bytes, "PartShape.brp"), b"fake brep payload\n");
    }

    /// A module payload's kind owns both the member name and the element tag,
    /// and numbering is per kind (a Part shape does not consume a Mesh number).
    #[test]
    fn a_module_kind_selects_the_member_and_element() {
        let mut saved = SavedDocument {
            format: "ferrocad".to_string(),
            version: 1,
            name: "Kinds".to_string(),
            objects: Vec::new(),
        };
        let mut part = object("Box", "Box");
        part.properties.insert(
            "Shape".into(),
            Property::Extension(ExtensionValue::new(Box::new(FakeShape(b"brep".to_vec())))),
        );
        let mut first = object("Mesh1", "Mesh1");
        first.properties.insert(
            "Mesh".into(),
            Property::Extension(ExtensionValue::new(Box::new(FakeMesh(vec![1, 2, 3])))),
        );
        let mut second = object("Mesh2", "Mesh2");
        second.properties.insert(
            "Mesh".into(),
            Property::Extension(ExtensionValue::new(Box::new(FakeMesh(vec![4, 5, 6])))),
        );
        saved.objects = vec![part, first, second];

        let bytes = encode_fcstd(&saved).expect("encode");
        let names: Vec<String> = members(&bytes).into_iter().map(|(name, ..)| name).collect();
        assert_eq!(
            names,
            [
                "Document.xml",
                "GuiDocument.xml",
                "PartShape.brp",
                "MeshKernel.bms",
                "MeshKernel1.bms",
            ]
        );

        let xml = String::from_utf8(read_member(&bytes, "Document.xml")).unwrap();
        assert!(xml.contains("<Part file=\"PartShape.brp\"/>"), "{xml}");
        assert!(xml.contains("<Mesh file=\"MeshKernel.bms\"/>"), "{xml}");
        assert!(xml.contains("<Mesh file=\"MeshKernel1.bms\"/>"), "{xml}");
        assert_eq!(read_member(&bytes, "MeshKernel.bms"), [1, 2, 3]);
        assert_eq!(read_member(&bytes, "MeshKernel1.bms"), [4, 5, 6]);
    }

    #[test]
    fn numbers_keep_exact_round_trip_text() {
        assert_eq!(number(0.125), "0.125");
        // A fixed six-decimal format would lose this value.
        assert_eq!(number(1e-20), "0.00000000000000000001");
        assert_eq!(number(-0.0), "-0");
    }
}
