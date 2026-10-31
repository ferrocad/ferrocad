//! The embedding runtime: the CPython bridge, the zero-copy presenter and headless runs.
//!
//! [`python`] owns the interpreter and the JSON boundary to the app's scripts;
//! [`headless`] renders a document to a file without a window; [`interop`] (when the
//! `gpu-interop` feature and the platform allow it) presents the viewport zero-copy
//! through gpui's wgpu window renderer.

pub mod headless;
#[cfg(all(
    feature = "gpu-interop",
    not(target_os = "macos"),
    not(target_family = "wasm")
))]
pub mod interop;
pub mod python;
