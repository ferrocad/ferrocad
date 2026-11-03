//! Dialog specs and value validation (A3).
//!
//! A command may return a dialog **spec** instead of acting directly: the engine
//! keeps the `on_accept` callback under a single-use token, and the shell renders
//! the spec as a modal. The spec, its fields and the pure validation live in
//! [`ferrocad_shell_api::dialog`] because a command outcome crosses the backend
//! seam; this module is the shell's path to them.

pub use ferrocad_shell_api::dialog::*;
