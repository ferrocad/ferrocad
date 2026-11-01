//! Facade over the fork's OCCT bindings.
//!
//! Every OCCT call `ferrocad_occt` makes lives in `ferrocad_part_cxx`
//! ([`ferrocad/ferrocad_occt_cxx`](https://github.com/ferrocad/ferrocad_occt_cxx)):
//! the generic bindings there are `shapefix`, `ops`
//! (curves, 2-D geometry, shape queries, the Part operations) and `surface_ops`.
//! This module re-exports them so call sites keep their single `bridge::fc_...`
//! path.
//!
//! History composition (the `Modified`/`Generated` loops, direct children and the
//! reusable cut operation) is plain Rust over `opencascade-sys`, in
//! [`crate::history`].
//!
//! There is no C++ in this crate: what stays here is FreeCAD/`Part` semantics
//! (see `src/policy.rs`), the `ferrocad_geom` seam implementation and the error
//! mapping. The design is written up in
//! `docs/developer/architecture/occt-integration.md`.

pub(crate) use ferrocad_part_cxx::ops::*;
