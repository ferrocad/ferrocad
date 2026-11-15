//! FerroCAD: Sculpt — the edition binary and its presentation half.
//!
//! The binary (`src/main.rs`) composes the app; this library carries what the edition
//! contributes to the interaction seam. Today that is one manipulator: the **cage
//! point grip** ([`cage_manipulator`]), which grabs a control point of a Sculpt cage
//! and commits its new position as `cage_point/move`. The app script
//! (`python/ferrocad_sculpt`) publishes the control points to the stage and applies
//! the commit, so the manipulator names no document and the script names no viewport.
//!
//! See [`cage`] for the contract, and `docs/developer/workbenches/sculpt.md` for the
//! bundle's design.

pub mod cage;

pub use cage::{CAGE_POINT_ID, CAGE_POINT_MOVE, CagePointGrip, ControlPoint, cage_manipulator};
