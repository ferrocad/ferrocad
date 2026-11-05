//! A host-supplied zero-copy presenter.
//!
//! [`InventorViewport`](crate::InventorViewport) normally draws and reads the frame
//! back as a [`RenderImage`](gpui::RenderImage). A host that can hand gpui a GPU
//! surface directly installs a [`Presenter`] instead: the first paint gives it the
//! window's device through [`GpuCanvasContext`], it draws the compiled views into its
//! own target, and it composites that target with `paint_texture`/`paint_surface`.
//!
//! This trait is deliberately **renderer-agnostic**: it names only gpui types and the
//! neutral [`ViewSpec`] batch vocabulary, so `ferrocad_viewport` keeps its "no wgpu,
//! no Python" boundary. The host (`ferrocad_shell_python`) implements it.

use ferrocad_graphics::{Camera, Hit, RenderQueue};
use ferrocad_inventor::ViewSpec;
use gpui::GpuCanvasContext;

/// Where a presenter is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenterStatus {
    /// The device-bound objects have not been created yet. The viewport keeps the
    /// image fallback (and requests another frame) until [`Presenter::prepare`] runs.
    Pending,
    /// The presenter has a target and can draw and composite.
    Ready,
    /// The window has no compatible device (headless, a non-wgpu renderer, …). The
    /// viewport keeps the image path for the rest of the session.
    Unsupported,
}

/// A host-owned GPU presentation path.
///
/// Called in this order:
///
/// 1. [`set_size`](Self::set_size) whenever the element's physical size changes;
/// 2. [`prepare`](Self::prepare) inside a `gpu_canvas` paint callback (where the
///    window renderer's device exists);
/// 3. [`draw`](Self::draw) from the viewport's `render` while the status is
///    [`Ready`](PresenterStatus::Ready);
/// 4. [`present`](Self::present) inside the same paint callback, after `prepare`.
///
/// A presenter that becomes [`Unsupported`](PresenterStatus::Unsupported) is never
/// consulted again; the viewport falls back to the image path.
pub trait Presenter {
    /// The current lifecycle state.
    fn status(&self) -> PresenterStatus;

    /// The target size in **physical** pixels. May be called before [`prepare`](Self::prepare);
    /// an uninitialized presenter stores it for its first allocation.
    fn set_size(&mut self, size: [u32; 2]);

    /// Create the device-bound objects on the first paint, and resize them when
    /// needed. Called inside `gpu_canvas`, once per paint.
    fn prepare(&mut self, gpu: &mut GpuCanvasContext);

    /// Draw the compiled views into the presenter's target. Return `false` to let the
    /// viewport fall back to the image path for this frame.
    fn draw(&mut self, views: &[ViewSpec<'_>]) -> bool;

    /// Composite the last drawn frame into the canvas. Called after `prepare`.
    fn present(&mut self, gpu: &mut GpuCanvasContext);

    /// The G-buffer pick on the presenter's device, when it has one.
    ///
    /// The viewport prefers this over its fallback view and the CPU ray cast: with
    /// zero-copy presentation the presenter already owns the window's device and the
    /// frame it just drew, so a pick there needs no second renderer. A presenter that
    /// cannot pick returns `None`, and the viewport falls back.
    fn pick_at(
        &mut self,
        _queue: &RenderQueue,
        _camera: &Camera,
        _device: [i32; 2],
    ) -> Option<Hit> {
        None
    }

    /// Whether [`pick_at`](Self::pick_at) is authoritative.
    ///
    /// When true, the viewport uses the presenter's answer even when it is `None`
    /// (empty space), so it does not run a second pick pass. A presenter without a
    /// pick keeps the default `false` and the viewport falls through to the backend.
    fn supports_pick(&self) -> bool {
        false
    }

    /// The pick for a **hover**, which a presenter may answer without blocking.
    ///
    /// [`pick_at`](Self::pick_at) is a GPU round trip: on a live window its blocking
    /// readback also waits for the frame's own GPU work, which made a hover pick cost
    /// tens of milliseconds and starved the frame loop on a large document. A presenter
    /// that can submit the pick and read it back later overrides this.
    ///
    /// The answer is three-valued, because "empty space" and "not yet" are different:
    ///
    /// - `Some(Some(hit))` — the texel at `device` was resolved and something is there;
    /// - `Some(None)` — the texel at `device` was resolved and it is empty space;
    /// - `None` — the pick for `device` is still in flight (or was rate-limited).
    ///
    /// The default is the synchronous pick, so a presenter that does not override this
    /// keeps exactly the behaviour it had. A click never calls this: it must act on the
    /// pixel it was released on.
    fn hover_pick_at(
        &mut self,
        queue: &RenderQueue,
        camera: &Camera,
        device: [i32; 2],
    ) -> Option<Option<Hit>> {
        Some(self.pick_at(queue, camera, device))
    }

    /// Whether a [`hover_pick_at`](Self::hover_pick_at) has been taken but not answered.
    ///
    /// The viewport asks so it can keep painting, and re-dispatch the hover, until the
    /// answer lands: a pointer that comes to rest sends no further moves, and the
    /// highlight (and the face dwell it starts) would otherwise wait for one.
    fn hover_pick_pending(&self) -> bool {
        false
    }

    /// Why the presenter is not drawing, when it has fallen back to the image path.
    ///
    /// This is technical text (the device/transport error), not a user sentence: the
    /// host shows it in a copyable diagnostics notice.
    fn status_note(&self) -> Option<&str> {
        None
    }
}
