//! Docked panels and transient surfaces: properties, palette, popovers, dialogs, forms,
//! the loadout bar and Dev Tools.
//!
//! Every surface here is drawn by the shell and closes with `Esc` unless it owns a
//! pending value (see [`crate::shell::context`]).

pub mod devtools;
pub mod dialogs;
pub mod forms;
pub mod loadout;
pub mod palette;
pub mod popovers;
pub mod properties;
