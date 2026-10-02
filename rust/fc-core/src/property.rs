//! Typed properties and a name → property container.

use std::collections::BTreeMap;

use crate::quantity::Quantity;

#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    String(String),
    Float(f64),
    Bool(bool),
    Quantity(Quantity),
}

impl Property {
    /// FreeCAD-style type id for this property.
    pub fn type_name(&self) -> &'static str {
        match self {
            Property::String(_) => "App::PropertyString",
            Property::Float(_) => "App::PropertyFloat",
            Property::Bool(_) => "App::PropertyBool",
            Property::Quantity(_) => "App::PropertyLength",
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
