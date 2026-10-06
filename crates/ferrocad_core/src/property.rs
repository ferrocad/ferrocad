//! Typed properties and a name → property container.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::extension::ExtensionValue;
use crate::geometry::{Matrix4, Placement, Rotation, Vector3};
use crate::quantity::Quantity;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Property {
    String(String),
    Float(f64),
    Bool(bool),
    Integer(i64),
    Quantity(Quantity),
    FloatList(Vec<f64>),
    IntegerList(Vec<i64>),
    StringList(Vec<String>),
    BoolList(Vec<bool>),
    Vector(Vector3),
    VectorList(Vec<Vector3>),
    Placement(Placement),
    PlacementList(Vec<Placement>),
    Rotation(Rotation),
    RotationList(Vec<Rotation>),
    Matrix(Matrix4),
    /// A link to another object, stored by name (POC simplification).
    Link(String),
    /// A list of links to other objects, stored by name (POC simplification).
    LinkList(Vec<String>),
    /// A link plus sub-element names, stored by object name (POC simplification).
    LinkSub(String, Vec<String>),
    /// RGBA colors in the 0..1 range (`App::PropertyColorList`).
    ColorList(Vec<[f64; 4]>),
    /// An arbitrary Python object, stored as a base64-encoded pickle
    /// (`App::PropertyPythonObject`, POC simplification).
    PythonObject(String),
    /// An included file, stored as its path in the document's transient dir
    /// (`App::PropertyFileIncluded`).
    FileIncluded(String),
    /// A list of integer pairs (`App::PropertyIntPairList`).
    IntPairList(Vec<(i64, i64)>),
    /// An enumeration: allowed values + selected index (`App::PropertyEnumeration`).
    Enumeration(Vec<String>, usize),
    /// An integer with a `[min, max]` range and step (`App::PropertyIntegerConstraint`).
    IntegerConstraint {
        value: i64,
        min: i64,
        max: i64,
        step: i64,
    },
    /// A float with a `[min, max]` range and step (`App::PropertyFloatConstraint`).
    FloatConstraint {
        value: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    /// A module-owned value (e.g. `Part::PropertyPartShape`). Core stores it opaquely;
    /// the owning module clones, compares and serialises it. See [`crate::extension`].
    Extension(ExtensionValue),
}

impl Property {
    /// FreeCAD-style type id for this property.
    pub fn type_name(&self) -> &str {
        match self {
            Property::String(_) => "App::PropertyString",
            Property::Float(_) => "App::PropertyFloat",
            Property::Bool(_) => "App::PropertyBool",
            Property::Integer(_) => "App::PropertyInteger",
            Property::Quantity(_) => "App::PropertyLength",
            Property::FloatList(_) => "App::PropertyFloatList",
            Property::IntegerList(_) => "App::PropertyIntegerList",
            Property::StringList(_) => "App::PropertyStringList",
            Property::BoolList(_) => "App::PropertyBoolList",
            Property::Vector(_) => "App::PropertyVector",
            Property::VectorList(_) => "App::PropertyVectorList",
            Property::Placement(_) => "App::PropertyPlacement",
            Property::PlacementList(_) => "App::PropertyPlacementList",
            Property::Rotation(_) => "App::PropertyRotation",
            Property::RotationList(_) => "App::PropertyRotationList",
            Property::Matrix(_) => "App::PropertyMatrix",
            Property::Link(_) => "App::PropertyLink",
            Property::LinkList(_) => "App::PropertyLinkList",
            Property::LinkSub(_, _) => "App::PropertyLinkSub",
            Property::ColorList(_) => "App::PropertyColorList",
            Property::PythonObject(_) => "App::PropertyPythonObject",
            Property::FileIncluded(_) => "App::PropertyFileIncluded",
            Property::IntPairList(_) => "App::PropertyIntPairList",
            Property::Enumeration(_, _) => "App::PropertyEnumeration",
            Property::IntegerConstraint { .. } => "App::PropertyIntegerConstraint",
            Property::FloatConstraint { .. } => "App::PropertyFloatConstraint",
            Property::Extension(value) => value.type_name(),
        }
    }
}

/// Public `PropertyType` flag bits (mirror `FreeCAD.PropertyType`).
pub mod prop_status {
    pub const NONE: u32 = 0;
    pub const READONLY: u32 = 1;
    pub const TRANSIENT: u32 = 2;
    pub const HIDDEN: u32 = 4;
    pub const OUTPUT: u32 = 8;
    pub const NORECOMPUTE: u32 = 16;
    pub const NOPERSIST: u32 = 32;
    /// Internal-only `Prop_Input` (reported by `getTypeOfProperty`).
    pub const INPUT: u32 = 64;
    /// Internal-only marker for a property added at runtime via `addProperty`
    /// (upstream `Property::PropDynamic`). It is deliberately absent from
    /// `status_names` / `status_from_name`, which describe the public
    /// `PropertyType` mask only.
    pub const DYNAMIC: u32 = 128;

    /// Properties with this bit are never written to the file. Note that
    /// `Prop_Transient` only suppresses persistence for *static* properties;
    /// dynamically added ones are still saved (FreeCAD `PropertyContainer::Save`).
    pub const NOT_PERSISTED: u32 = NOPERSIST;
}

/// The `getTypeOfProperty` / `getPropertyStatus` text names for a status mask.
/// Order follows the upstream `getTypeOfProperty` docstring.
pub fn status_names(status: u32) -> Vec<&'static str> {
    let mut names = Vec::new();
    if status & prop_status::HIDDEN != 0 {
        names.push("Hidden");
    }
    if status & prop_status::NORECOMPUTE != 0 {
        names.push("NoRecompute");
    }
    if status & prop_status::NOPERSIST != 0 {
        names.push("NoPersist");
    }
    if status & prop_status::OUTPUT != 0 {
        names.push("Output");
    }
    if status & prop_status::READONLY != 0 {
        names.push("ReadOnly");
    }
    if status & prop_status::TRANSIENT != 0 {
        names.push("Transient");
    }
    if status & prop_status::INPUT != 0 {
        names.push("Input");
    }
    names
}

/// Map a status text name to its bit (case-insensitive).
pub fn status_from_name(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "readonly" => Some(prop_status::READONLY),
        "transient" => Some(prop_status::TRANSIENT),
        "hidden" => Some(prop_status::HIDDEN),
        "output" => Some(prop_status::OUTPUT),
        "norecompute" => Some(prop_status::NORECOMPUTE),
        "nopersist" => Some(prop_status::NOPERSIST),
        "input" => Some(prop_status::INPUT),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct PropertyContainer {
    props: BTreeMap<String, Property>,
    /// Property name → status bitmask. May carry the internal `DYNAMIC` bit,
    /// which the public accessors mask out.
    status: BTreeMap<String, u32>,
    /// Property name → group shown in the UI (`getGroupOfProperty`).
    group: BTreeMap<String, String>,
    /// Property name → documentation (`getDocumentationOfProperty`).
    doc: BTreeMap<String, String>,
}

impl PropertyContainer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, value: Property) {
        let name = name.into();
        self.status.entry(name.clone()).or_insert(prop_status::NONE);
        self.props.insert(name, value);
    }

    /// Insert a value together with its status mask (used by `addProperty` and
    /// by document restore). The mask is stored verbatim: callers that want the
    /// `DYNAMIC` bit set must include it.
    pub fn set_with_status(&mut self, name: impl Into<String>, value: Property, status: u32) {
        let name = name.into();
        self.status.insert(name.clone(), status);
        self.props.insert(name, value);
    }

    /// The public `PropertyType` status mask for a property, or `None` if the
    /// property does not exist. The internal `DYNAMIC` bit is masked out.
    pub fn status(&self, name: &str) -> Option<u32> {
        self.status.get(name).map(|s| s & !prop_status::DYNAMIC)
    }

    /// The raw status mask, including the internal `DYNAMIC` bit (used when
    /// persisting, so dynamic-ness survives a save/restore round trip).
    pub fn raw_status(&self, name: &str) -> Option<u32> {
        self.status.get(name).copied()
    }

    /// Record a property's group and documentation (`addProperty` metadata).
    pub fn set_meta(&mut self, name: impl Into<String>, group: &str, doc: &str) {
        let name = name.into();
        self.group.insert(name.clone(), group.to_string());
        self.doc.insert(name, doc.to_string());
    }

    /// The group a property belongs to (`getGroupOfProperty`), if it exists.
    pub fn group(&self, name: &str) -> Option<&str> {
        self.props.contains_key(name).then(|| {
            self.group.get(name).map(String::as_str).unwrap_or("")
        })
    }

    /// A property's documentation (`getDocumentationOfProperty`), if it exists.
    pub fn doc(&self, name: &str) -> Option<&str> {
        self.props.contains_key(name).then(|| {
            self.doc.get(name).map(String::as_str).unwrap_or("")
        })
    }

    /// Replace a property's *public* status mask; `None` if the property does
    /// not exist. The internal `DYNAMIC` bit is preserved.
    pub fn set_status(&mut self, name: &str, status: u32) -> Option<u32> {
        if !self.props.contains_key(name) {
            return None;
        }
        let prev = self.status.get(name).copied().unwrap_or(prop_status::NONE);
        let dynamic = prev & prop_status::DYNAMIC;
        let new = (status & !prop_status::DYNAMIC) | dynamic;
        self.status.insert(name.to_string(), new);
        Some(prev & !prop_status::DYNAMIC)
    }

    pub fn get(&self, name: &str) -> Option<&Property> {
        self.props.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.props.get_mut(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Property> {
        self.status.remove(name);
        self.group.remove(name);
        self.doc.remove(name);
        self.props.remove(name)
    }

    pub fn clear(&mut self) {
        self.props.clear();
        self.status.clear();
        self.group.clear();
        self.doc.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Property)> {
        self.props.iter()
    }

    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.props.keys()
    }
}
