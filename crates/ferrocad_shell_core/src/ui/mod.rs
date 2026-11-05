//! The gpui surfaces: the window chrome, dialogs, forms and keyboard shortcuts.
//!
//! These modules draw and route input; they hold no document state of their own and
//! read the shell models through [`crate::model`].

pub mod chrome;
pub(crate) mod command_icons;
pub mod dev_tools;
pub mod dialog;
pub mod forms;
pub mod shortcuts;
