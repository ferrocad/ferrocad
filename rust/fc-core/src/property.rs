//! Typed properties and a name → property container.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
}

impl Property {
    /// FreeCAD-style type id for this property.
    pub fn type_name(&self) -> &'static str {
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
        }
    }
}

#[derive(Debug, Default)]
pub struct PropertyContainer {
    props: BTreeMap<String, Property>,
}

impl PropertyContainer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, value: Property) {
        self.props.insert(name.into(), value);
    }

    pub fn get(&self, name: &str) -> Option<&Property> {
        self.props.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.props.get_mut(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Property> {
        self.props.remove(name)
    }

    pub fn clear(&mut self) {
        self.props.clear();
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
