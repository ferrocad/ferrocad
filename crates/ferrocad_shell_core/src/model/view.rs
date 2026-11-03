//! View requests and the shell's presentation state.
//!
//! The data and its `apply` behaviour live in [`ferrocad_shell_api::view`] because a
//! command outcome carries a `ViewRequest` across the backend seam. This module
//! re-exports them so the shell's `crate::view::ViewState` path is unchanged.
//!
//! A command that changes only presentation returns `{"view": {...}}` instead of
//! editing the document. The shell owns the state, applies what the viewport support
//! (the projection, the grid and its steps), and mirrors the state to the engine's
//! view facade so a checkable command's `IsChecked` can read it.
//!
//! A `workplane` request is an **action**, not state: it activates, clears or looks
//! down the active workplane. The shell performs it on the viewport and reports the
//! outcome.

pub use ferrocad_shell_api::view::*;
