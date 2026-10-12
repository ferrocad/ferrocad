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
use crate::pixels::Rect;
use ferrocad_graphics::Hit;

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

    /// Resize the view's render target to `size`, in physical pixels.
    ///
    /// A host calls this when the element it presents into changes size, so the target
    /// (and the host's camera viewport) stay at the element's physical size. The default
    /// is a no-op, for backends with a fixed target.
    fn resize(&mut self, size: [u32; 2]) {
        let _ = size;
    }

    /// Draw a compiled queue into the view's own target.
    fn draw(&mut self, queue: &RenderQueue, camera: &Camera) -> Result<(), Self::Error>;

    /// Borrow the shared render target for the host to composite (zero-copy).
    ///
    /// `None` when the backend cannot share its target; the host then falls back to
    /// [`image`](Self::image).
    fn borrow_surface(&self) -> Option<&Self::Surface>;

    /// The last drawn frame read back as an RGBA image (the MVP path).
    fn image(&mut self) -> Result<ImageData, Self::Error>;

    /// Draw a frame of one or more views. The first is the main view; later ones are
    /// **overlays** ([`ViewSpec::overlay`]), each confined to its [`ViewSpec::rect`].
    ///
    /// The default draws only the main view (a backend without overlay support still
    /// renders the scene). `ferrocad_wgpu` overrides it to render every view in one
    /// pass.
    fn draw_frame(&mut self, views: &[ViewSpec<'_>]) -> Result<(), Self::Error> {
        match views.first() {
            Some(view) => self.draw(view.queue, view.camera),
            None => Ok(()),
        }
    }

    /// Whether the backend can pick with a hardware G-buffer pass (see
    /// [`pick_at`](Self::pick_at)). When `false` (the default), the host falls back to
    /// the CPU ray cast in [`ferrocad_graphics::pick`].
    fn supports_pick(&self) -> bool {
        false
    }

    /// The [`Hit`] under a **device** pixel (bottom-left origin), via an offscreen
    /// G-buffer pass, or `None` when nothing is there.
    ///
    /// `queue`/`camera` are the main scene view — the same pair
    /// [`draw`](Self::draw) receives — so a backend can re-draw it into its pick targets
    /// and read a single texel. The default returns `None`; a host must consult
    /// [`supports_pick`](Self::supports_pick) to tell "unsupported" from "empty".
    fn pick_at(&mut self, queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
        let _ = (queue, camera, device);
        None
    }
}

/// One view in a multi-view frame: a compiled queue, its camera, and — for overlays —
/// the device-pixel rectangle it is confined to.
#[derive(Clone, Copy)]
pub struct ViewSpec<'a> {
    /// The compiled queue to draw.
    pub queue: &'a RenderQueue,
    /// The camera to draw it with.
    pub camera: &'a Camera,
    /// The device-pixel rectangle to confine the draw to (`None` = the whole target).
    /// Bottom-left origin, like [`Rect`].
    pub rect: Option<Rect>,
    /// Whether this is an overlay: drawn on top of the main view with depth off and
    /// alpha blending, and not clearing the target.
    pub overlay: bool,
}
