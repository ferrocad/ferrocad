//! The toy workbench as a **wasm bundle**: shape B with an import, not a JS global.
//!
//! This is `www/addons/toy.js` compiled, and the two paths reach the document the same
//! way — through JSON calls — so the host cannot tell them apart:
//!
//! ```text
//! JS bundle:   page ──▶ window.ferrocad.call(request)
//! wasm bundle: this module ──▶ import "ferrocad" "call" (ptr, len) ──▶ the loader ──▶ the host
//! ```
//!
//! What crosses the module boundary is **text and nothing else**: no `Document`, no
//! `ferrocad_core` link, no second registry. A bundle with its own copy of the engine
//! would edit a document nobody draws.
//!
//! Two layers, deliberately:
//!
//! - [`toy`] is the workbench: commands, the object it owns, its mesh. Pure, with the
//!   doorway as a closure, so it is a normal `cargo test` on the host against the real
//!   `ferrocad_doc_api`.
//! - `abi` (wasm only) is the wiring: the exports the loader calls and the `ferrocad.call`
//!   import, with the `(ptr, len)` / packed-`u64` memory contract.
//!
//! See [`../../docs/project/research/js-addons.md`](../../docs/project/research/js-addons.md)
//! for the shapes compared and the loader.

pub mod toy;

#[cfg(target_family = "wasm")]
mod abi;
