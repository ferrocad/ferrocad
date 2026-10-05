//! # FerroCAD base types (`ferrocad_types`)
//!
//! The kernel-independent value types, mirroring FreeCAD's `Base` module, that are
//! shared by the document core ([`ferrocad_core`](https://crates.io/crates/ferrocad_core))
//! and, later, the geometry seam and its OCCT backend.
//!
//! They live in their own crate, below `ferrocad_core`, so a geometry backend can
//! depend on the values it needs (`Placement`, `Quantity`, …) without pulling in the
//! document model. See `docs/occt-integration.md` in the repository.
//!
//! - [`quantity`] — [`Quantity`]: a value with a unit, and FreeCAD's unit parser.
//! - [`unit`] — [`Unit`]: an 8-dimensional signature plus a scale.
//! - [`geometry`] — [`Vector3`], [`Matrix4`], [`Rotation`], [`Placement`], [`TypeId`].

pub mod geometry;
pub mod quantity;
pub mod unit;

pub use geometry::{Matrix4, Placement, Rotation, ScaleType, TypeId, Vector3};
pub use quantity::{canonical_name, parse_unit, Quantity};
pub use unit::Unit;
