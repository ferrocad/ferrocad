//! Typed properties and a name → property container.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geometry::{Matrix4, Placement, Vector3};
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
    Matrix(Matrix4),
    /// A link to another object, stored by name (POC simplification).
    Link(String),
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
            Property::Matrix(_) => "App::PropertyMatrix",
            Property::Link(_) => "App::PropertyLink",
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
