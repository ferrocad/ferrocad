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
