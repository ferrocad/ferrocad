//! The shell's selection: what the user picked, and the facade's snapshot (A2).
//!
//! The shell owns the selection; `FreeCADGui.Selection` is a **read-only snapshot**
//! of it, pushed after every change so a command's `IsActive`/`Activated` can see
//! the same thing the UI shows. The facade keeps no document state of its own
//! (see `docs/workbenches/actions-and-tools.md` §5).
//!
//! Unlike the viewport's [`ferrocad_geom::SelectionRef`], which addresses objects by
//! picking identity, an item is already resolved to a document object name: the
//! mapping through [`crate::display::DisplaySync`] happens at the shell boundary.

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

/// Push the selection to `FreeCADGui.Selection`.
///
/// A failure is reported, not fatal: the UI selection is still valid, but commands
/// will see the previous snapshot until the next successful push.
pub fn push(items: &[SelectionItem]) -> Result<(), String> {
    crate::python::set_selection(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_object_serializes_with_an_empty_element_list() {
        let item = SelectionItem::new("Slab");
        assert!(item.is("Slab"));
        assert!(!item.is("Box"));
        assert!(item.is_whole());
        assert_eq!(
            serde_json::to_string(&item).unwrap(),
            r#"{"object":"Slab","elements":[]}"#
        );
    }

    #[test]
    fn elements_round_trip_and_default_to_empty() {
        let item = SelectionItem::with_elements("Box", vec!["Face6".to_string()]);
        assert!(!item.is_whole());
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(serde_json::from_str::<SelectionItem>(&json).unwrap(), item);

        let bare: SelectionItem = serde_json::from_str(r#"{"object":"Box"}"#).unwrap();
        assert!(bare.is_whole());
    }
}
