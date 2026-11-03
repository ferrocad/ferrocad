//! The shell's presentation runtime.
//!
//! [`interop`] (when the `gpu-interop` feature and the platform allow it) presents
//! the viewport zero-copy through gpui's wgpu window renderer. The platform-specific
//! embedding and headless paths live in the native assembly (`ferrocad_shell_gpui`).

#[cfg(all(
    feature = "gpu-interop",
    not(target_os = "macos"),
    not(target_family = "wasm")
))]
pub mod interop;
