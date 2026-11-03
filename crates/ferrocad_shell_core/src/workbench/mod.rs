//! The workbench-facing registry: commands, contributions and manipulators.
//!
//! A workbench registers commands and overlays here; the shell names no workbench and
//! reaches them only through these traits and data types.

pub mod commands;
pub mod contributions;
pub mod manipulators;
