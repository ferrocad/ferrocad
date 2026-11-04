//! The shell's viewport bridge: context gating, event dispatch, picking and the view.
//!
//! [`context`] owns the interaction context that every action entry point consults;
//! [`dispatch`] routes commands and view requests; [`picking`] resolves clicks;
//! [`viewport`] arms the stage, manipulators and gizmos on the viewport.

pub mod context;
pub mod dispatch;
pub mod document;
pub mod picking;
pub mod viewport;
