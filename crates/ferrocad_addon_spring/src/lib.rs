//! The spring workbench as a **wasm bundle**: parameters in the document, geometry in the
//! bundle.
//!
//! This is the shape a sandboxed addon is *for*. The document stores four numbers
//! (`Radius`, `Pitch`, `Turns`, `Wire`) on an `App::FeaturePython` object that has no
//! `Shape` at all; the bundle reads them through the doorway, computes the helix itself,
//! and answers the display hook with the mesh. Nothing links an engine: a bundle that
//! grew its own copy of `ferrocad_core` would edit a document nobody draws.
//!
//! Two layers, as every bundle has:
//!
//! - [`spring`] is the workbench: commands, the object it owns, the tube it draws. Pure,
//!   with the doorway as a closure, so it is a normal `cargo test` on the host against the
//!   real `ferrocad_doc_api`.
//! - `abi` (wasm only) is the wiring: the exports the loader calls, over
//!   [`ferrocad_addon`]'s memory contract.

pub mod spring;

#[cfg(target_family = "wasm")]
mod abi;
