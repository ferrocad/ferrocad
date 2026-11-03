//! The view the shell's viewport element renders through, per platform.
//!
//! Natively the element owns its renderer ([`ferrocad_wgpu::WgpuRenderer`]) and
//! presents through the readback image path, or through the zero-copy presenter when
//! the window's device can be shared.
//!
//! The browser cannot have that renderer: creating one outside a paint means
//! `block_on`, and `block_on` never returns on wasm. There the element is built with
//! [`DeferredView`], which only records its size; the real renderer belongs to the
//! presenter, which is created on the first paint from the window's own device (the
//! same negotiation the native `gpu-interop` path makes).
//!
//! [`DeferredView`] deliberately draws nothing: a paint where the presenter is not
//! ready yet is an empty canvas, and the presenter reports why if it never becomes
//! ready. It is **not** an image fallback — the readback path cannot complete on the
//! web (`map_async` plus `poll` never resolves).

#[cfg(target_family = "wasm")]
mod deferred {
    use ferrocad_inventor::{Camera, ImageData, InventorView, RenderQueue};

    /// A view with no renderer of its own.
    ///
    /// It answers the size the element asks for — so the camera viewport and the
    /// presenter's target agree — and refuses every draw. The presenter is the only
    /// renderer in a browser window.
    #[derive(Debug, Clone)]
    pub struct DeferredView {
        size: [u32; 2],
    }

    /// Why a [`DeferredView`] cannot draw.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct NoView;

    impl std::fmt::Display for NoView {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("this host has no renderer of its own; the presenter draws the viewport")
        }
    }

    impl std::error::Error for NoView {}

    impl DeferredView {
        /// A view at a bootstrap size. The element resizes it on its first frame.
        pub fn new(size: [u32; 2]) -> Self {
            Self { size }
        }
    }

    impl InventorView for DeferredView {
        /// Never shared: there is no target to hand to a host.
        type Surface = ();
        type Error = NoView;

        fn size(&self) -> [u32; 2] {
            self.size
        }

        fn resize(&mut self, size: [u32; 2]) {
            self.size = size;
        }

        fn draw(&mut self, _queue: &RenderQueue, _camera: &Camera) -> Result<(), Self::Error> {
            Err(NoView)
        }

        fn borrow_surface(&self) -> Option<&Self::Surface> {
            None
        }

        fn image(&mut self) -> Result<ImageData, Self::Error> {
            Err(NoView)
        }
    }
}

#[cfg(target_family = "wasm")]
pub use deferred::{DeferredView, NoView};

/// The renderer the shell's viewport element owns.
///
/// Native: the readback renderer, as it always was. Wasm: a size-only placeholder,
/// because the browser's only renderer lives inside the zero-copy presenter.
#[cfg(target_family = "wasm")]
pub type ShellView = DeferredView;

/// The renderer the shell's viewport element owns.
#[cfg(not(target_family = "wasm"))]
pub type ShellView = ferrocad_wgpu::WgpuRenderer;
