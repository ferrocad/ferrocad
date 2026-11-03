//! The selection vocabulary the shell and its engine adapter share.
//!
//! Unlike the viewport's [`ferrocad_geom::SelectionRef`], which addresses objects by
//! picking identity, an item is already resolved to a document object name: the
//! mapping through the shell's `DisplaySync` happens at the shell boundary.

use serde::{Deserialize, Serialize};

/// One selected object plus the sub-elements picked on it.
///
/// An empty `elements` means the whole object. The order is the click order; the
/// last item is the **primary** selection (the property editor's subject).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionItem {
    /// The document object's name (`"Slab"`).
    pub object: String,
    /// The kernel names the viewport reported (`["Face6"]`); empty for the whole.
    #[serde(default)]
    pub elements: Vec<String>,
}

impl SelectionItem {
    /// The whole object.
    pub fn new(object: impl Into<String>) -> Self {
        Self {
            object: object.into(),
            elements: Vec::new(),
        }
    }

    /// An object with the picked sub-elements.
    pub fn with_elements(object: impl Into<String>, elements: Vec<String>) -> Self {
        Self {
            object: object.into(),
            elements,
        }
    }

    /// Whether this item is `name`.
    pub fn is(&self, name: &str) -> bool {
        self.object == name
    }

    /// Whether the whole object is selected (no sub-element).
    pub fn is_whole(&self) -> bool {
        self.elements.is_empty()
    }
}
