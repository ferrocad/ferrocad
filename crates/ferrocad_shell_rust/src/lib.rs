//! The Rust document backend: a real document, with no scripting runtime.
//!
//! [`RustAppBackend`] implements the shell's
//! [`AppBackend`](ferrocad_shell_api::app::AppBackend) over
//! `ferrocad_core::Document`, with Part features computed by the OCCT kernel. It is
//! the backend both the browser host (`ferrocad_web_gpui`) and a desktop edition
//! install — that is why this crate names no window system and no interpreter:
//!
//! ```text
//! ferrocad_shell_rust -> ferrocad_core, ferrocad_part, ferrocad_occt, ferrocad_geom,
//!                        ferrocad_stage, ferrocad_shell_api, serde_json
//!                        must NOT -> gpui, pyo3, wasm-bindgen
//! ```
//!
//! The sample document is a `Part::Box` with a `Part::Fillet` on one edge: the
//! smallest document that exercises a dependency (the fillet recomputes after the
//! box), a parametric feature (`Radius`) and a real tessellation.
//!
//! # What is real, and what is not
//!
//! Real: `Document::recompute`, the Part object types, the kernel, tessellation into
//! `StageMesh`es, picking (the shell's CPU ray cast over that stage), property edits
//! through [`AppBackend::set_property`](ferrocad_shell_api::app::AppBackend::set_property)
//! followed by a recompute, the undo history and file I/O at the byte level.
//!
//! Not yet: expressions, binary file I/O, and anything scripted — there is no
//! interpreter in this crate, so `evaluate` says so rather than pretending. A
//! browser host owns the file picker and the download; it installs
//! [`RustAppBackend::with_file_hooks`] for the two file verbs, and a native host
//! leaves them out because its shell intercepts `App_Open`/`App_Save` itself.

pub mod backend;

pub use backend::RustAppBackend;
