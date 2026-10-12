//! The view boundary: how a host presents and exports a rendered scene.
//!
//! `bite-gpui` presents custom GPU content through a *surface* primitive
//! (`PaintSurface`, CoreVideo-backed on macOS) that is not yet implemented for its
//! wgpu backend, and through `RenderImage` (CPU pixels) for the image path. Until a
//! shared wgpu surface exists, a host needs both:
//!
//! - a **shareable GPU surface** it can composite zero-copy (a `wgpu::Texture`, a
//!   dmabuf, a `CVPixelBuffer`, …), and
//! - a **readback image** for the MVP fallback, which the host wraps as a
//!   `RenderImage` and paints.
//!
//! [`InventorView`] is that capability, expressed in renderer-agnostic terms via
//! associated types, so it lives here without depending on `gpui` or `wgpu`. A
//! backend (`ferrocad_wgpu` today) implements it; the `ferrocad_gpui` glue will
//! adapt `Surface`/`ImageData` to gpui's surface/`RenderImage` types.
//!
//! ```text
//! InventorView ──┬─ borrow_surface() → host composites the shared texture (zero-copy)
//!                └─ image()          → host wraps RGBA pixels as a gpui RenderImage
//! ```

use crate::batch::RenderQueue;
use crate::camera::Camera;

/// A CPU-side RGBA8 image, row-major, top-left origin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageData {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8.
    pub rgba: Vec<u8>,
}

/// A view onto an invented scene that a host can present.
///
/// The name mirrors FreeCAD's `ViewProvider`/`View3DInventorViewer` role — the
/// thing that owns a render target for an invented scene — but is a host-facing
/// capability rather than a scene-graph node.
pub trait InventorView {
    /// The backend's shareable render target (e.g. a `wgpu::Texture`, a dmabuf).
    type Surface;
    /// The backend's error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The target's size in physical pixels.
    fn size(&self) -> [u32; 2];

    /// Draw a compiled queue into the view's own target.
    fn draw(&mut self, queue: &RenderQueue, camera: &Camera) -> Result<(), Self::Error>;

    /// Borrow the shared render target for the host to composite (zero-copy).
    ///
    /// `None` when the backend cannot share its target; the host then falls back to
    /// [`image`](Self::image).
    fn borrow_surface(&self) -> Option<&Self::Surface>;

    /// The last drawn frame read back as an RGBA image (the MVP path).
    fn image(&mut self) -> Result<ImageData, Self::Error>;
}
