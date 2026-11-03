//! The shell's selection: what the user picked, and the engine's snapshot (A2).
//!
//! The shell owns the selection; the engine's selection facade (natively
//! `FreeCADGui.Selection`) is a **snapshot** of it, pushed after every change so a
//! command's `IsActive`/`Activated` can see the same thing the UI shows. The write
//! direction is queued, not synchronous: the facade records
//! `addSelection`/`removeSelection`/`clearSelection` and
//! `Shell::drain_selection_requests` applies them to this state, which is what makes
//! a scripted selection reach the viewport. The facade keeps no document state of
//! its own (see `docs/developer/workbenches/actions-and-tools.md` §5).
//!
//! The item type lives in [`ferrocad_shell_api::selection`] because it crosses the
//! backend seam; the mapping through [`crate::display::DisplaySync`] happens here.

use gpui::App;

/// The item vocabulary, defined with the rest of the seam's data.
pub use ferrocad_shell_api::selection::SelectionItem;

/// Push the selection to the engine's selection facade (native: `FreeCADGui.Selection`).
///
/// A failure is reported, not fatal: the UI selection is still valid, but commands
/// will see the previous snapshot until the next successful push.
pub fn push(cx: &App, items: &[SelectionItem]) -> Result<(), String> {
    crate::app::set_selection(cx, items)
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
