//! Unit tests for the crate's public surface, sliced by area.
//!
//! These exercise the API the `FreeCAD` Python package is built on. Tests of
//! private helpers stay next to the code they cover (e.g. `expr`, `geometry`,
//! `quantity`, `unit`, `stringhasher`).

mod documents;
mod properties;
mod quantities;
mod transactions;
