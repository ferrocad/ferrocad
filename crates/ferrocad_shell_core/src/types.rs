//! The shell's view models.
//!
//! They live in [`ferrocad_shell_api::types`] because they cross the backend seam;
//! the shell re-exports them here so `crate::types::PropRow` and the public
//! `ferrocad_shell_core::PropRow` paths keep working.

pub use ferrocad_shell_api::types::*;
