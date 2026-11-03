//! The shell's presentation runtime.
//!
//! [`interop`] (when the `gpu-interop` feature is on and the platform has a wgpu
//! window renderer) presents the viewport zero-copy through gpui's wgpu device. On
//! the web it is the only presentation path, because the readback image path cannot
//! complete there. The platform-specific embedding and headless paths live in the
//! native assembly (`ferrocad_shell_gpui`).
//!
//! [`render_view`] names the view the viewport element owns: the readback renderer
//! natively, a size-only placeholder on the web (the presenter owns the real one).

#[cfg(all(feature = "gpu-interop", not(target_os = "macos")))]
pub mod interop;
pub mod render_view;
