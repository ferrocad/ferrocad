//! Zero-copy viewport presentation through gpui's window renderer.
//!
//! The viewport renders into a GPU target on the **window's own wgpu device** and
//! hands the frame to gpui without a CPU copy:
//!
//! - Linux: a `WgpuSharedBufferRing` exports each frame as a dma-buf and
//!   `paint_surface` imports it (the same-adapter, cross-device shape). If the
//!   device cannot export (a GL adapter, or a driver without the Vulkan external
//!   memory extensions), the presenter reports
//!   [`PresenterStatus::Unsupported`] and the viewport keeps its image path.
//! - Windows/FreeBSD: a single `WgpuSurface` sampled with `paint_texture` on the
//!   same device, which requires the window itself to be running gpui's wgpu
//!   renderer.
//!
//! Either way the renderer runs on `Adapter::wgpu()`: it must be the window
//! renderer's device, because a wgpu texture cannot be exported and a texture from
//! another device cannot be sampled by gpui.
//!
//! # Why the ring, and what it guarantees
//!
//! `WgpuSharedBuffer::submit` only *starts* the write. Handing that handle to gpui
//! while the GPU is still working lets gpui's 16 ms acquire-fence poll time out and
//! drop the surface — an intermittent black frame. The same device does not help:
//! the poll is on the CPU, before gpui's draw is submitted. The Coin grid's
//! full-screen fragment shader makes frames long enough for that race to show.
//!
//! This module previously hand-rolled a ring and a "reuse after `RING_DEPTH`
//! submissions" distance rule. A distance heuristic is not a synchronisation
//! primitive: a frame can stay in the consumer's queue for more submissions than
//! the ring is deep, and the race returns. `gpui_interop 0.2.0`'s
//! `WgpuSharedBufferRing` is the upstream fix (bite-gpui/gpui_interop#1), and this
//! module now uses it directly. It guarantees:
//!
//! - `begin` hands out a free slot, or `None` when every slot is in flight. `None`
//!   is a **skipped producer frame**, not an error: the painter still calls
//!   `present`, which repeats the last good frame.
//! - A slot is recycled only once the consumer's release descriptor has been
//!   signalled, or after a 250 ms fallback for a release that was lost. The old
//!   distance rule is gone.
//! - `present` returns only a frame whose acquire fence has **already** signalled,
//!   with the fence removed, so gpui imports it without polling. If no submitted
//!   frame is ready, it returns the last good frame; a frame overtaken by a newer
//!   one is dropped rather than shown late.
//! - `last` is the fence-free frame for a paused repaint, when the producer has not
//!   drawn since the previous paint.
//!
//! The presenter therefore renders with `begin` → `encode` → `submit` in
//! [`Presenter::draw`], and composites with `present` (or `last` for a paused
//! repaint) in [`Presenter::present`].
//!
//! # The same-device path
//!
//! A `WgpuSurface` needs no ring: gpui's renderer samples it on the same device and
//! queue the producer submits to, so queue ordering already keeps the consumer's
//! sample after the frame's render. A *cross-device* producer would not have that
//! ordering and would have to gate reuse on the consumer's release, which is what
//! the dma-buf ring does. This module keeps one surface for the same-device path and
//! repaints its last imported handle when no new frame was drawn.
//!
//! This module is compiled only where gpui's wgpu renderer is available (not macOS).
//! Natively the viewport has its own renderer as well, and a presenter that reports
//! `Unsupported` leaves the image path in place. In a browser window the presenter is
//! the *only* renderer: the element's own is a size-only placeholder, and the
//! readback path cannot complete, so an unsupported presenter means an empty canvas
//! (reported through `status_note`).
//!
//! The wasm shape is the same-device path with two differences: the colour format
//! comes from the surface (`WgpuSurface::format`) and the target is single-sampled,
//! because MSAA on a WebGL2 render target is not the tested path.
use std::sync::Arc;

use ferrocad_inventor::{Camera, Hit, RenderQueue, ViewSpec};
use ferrocad_viewport::{Presenter, PresenterStatus};
use ferrocad_wgpu::{GpuContext, WgpuRenderer as FerrocadRenderer, trace};

// The pass accounting is native-only (it installs a `FramePipeline` on the desktop
// application), so the ring-skip diagnostic that reads its switch is too.
#[cfg(not(target_family = "wasm"))]
use crate::runtime::frame_timing;
#[cfg(target_os = "linux")]
use gpui::DmaBufHandle;
use gpui::GpuCanvasContext;
#[cfg(not(target_os = "linux"))]
use gpui::{Corners, ImportedTextureHandle};
#[cfg(target_os = "linux")]
use gpui_interop::WgpuSharedBufferRing;
#[cfg(not(target_os = "linux"))]
use gpui_interop::WgpuSurface;
use gpui_interop::attach;
#[cfg(not(target_os = "linux"))]
use gpui_wgpu::ImportedTextureExt as _;
use gpui_wgpu::WgpuRenderer as WindowRenderer;
use gpui_wgpu::wgpu;

/// The viewport target's sRGB format, matching `ferrocad_wgpu`'s `TARGET_FORMAT`
/// and the imported-texture contract.
///
/// The same-device path (Windows, FreeBSD and the web) takes the format from the
/// surface it allocates instead, so the two cannot disagree.
#[cfg(target_os = "linux")]
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// MSAA count for the scene target. Matches the image path's 4× default.
///
/// WebGL2 render targets are single-sampled here: MSAA is the untested path in the
/// browser, and one sample always works.
#[cfg(not(target_family = "wasm"))]
const SAMPLES: u32 = 4;
#[cfg(target_family = "wasm")]
const SAMPLES: u32 = 1;

/// The sample count the scene target is built with.
///
/// `FERROCAD_MSAA` overrides it, because 4× is a native-only cost the browser build
/// does not pay (see [`SAMPLES`]): on a large document it multiplies the fill and the
/// target's memory, and whether that is the frame cost is worth measuring rather than
/// guessing. `1` is the browser's setting.
fn samples() -> u32 {
    let requested = std::env::var("FERROCAD_MSAA")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(SAMPLES);
    requested.max(1)
}

/// What happens when the presenter cannot present, for the log line.
#[cfg(target_family = "wasm")]
const FALLBACK: &str = "the viewport stays blank";
#[cfg(not(target_family = "wasm"))]
const FALLBACK: &str = "using the image path";

/// Build the host presenter. Installing it is always safe: if the window has no
/// compatible device, it reports [`PresenterStatus::Unsupported`] and the viewport
/// keeps the image path.
pub fn presenter() -> Box<dyn Presenter> {
    Box::new(GpuiInteropPresenter::new())
}

/// The host's zero-copy presenter: a renderer on the window's wgpu device plus the
/// target(s) it hands to gpui.
pub struct GpuiInteropPresenter {
    status: PresenterStatus,
    /// The element's physical size. It can arrive before the device does.
    size: [u32; 2],
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    renderer: Option<FerrocadRenderer>,
    /// Linux: the crate's dma-buf ring, which owns the exportable slots and their
    /// release descriptors.
    #[cfg(target_os = "linux")]
    ring: Option<WgpuSharedBufferRing>,
    /// Whether [`Presenter::draw`] submitted a frame in this paint. `present` uses
    /// the ring's `present` in that case and `last` for a paused repaint.
    #[cfg(target_os = "linux")]
    drew: bool,
    /// The same-device path's single surface. No ring is needed: gpui samples it on
    /// the same device and queue the producer submits to.
    #[cfg(not(target_os = "linux"))]
    surface: Option<WgpuSurface>,
    /// The frame encoded by `draw`, submitted by `present` (same-device path).
    #[cfg(not(target_os = "linux"))]
    commands: Option<wgpu::CommandBuffer>,
    /// The last same-device frame handed to gpui, kept so a paint without a new
    /// frame can repeat it instead of showing an empty canvas.
    #[cfg(not(target_os = "linux"))]
    presented: Option<ImportedTextureHandle>,
    /// Why the next `present` has no new frame, for the trace.
    pending_reason: Option<&'static str>,
    /// A draw failed; present the last complete frame, then let the image path take
    /// over. Retiring here rather than in `draw` is what keeps the current paint
    /// from rendering an empty canvas.
    disable_after_present: Option<&'static str>,
    /// The technical reason the image path is in use, when it is. Shown in the shell's
    /// copyable diagnostics notice; `None` while the presenter is accelerated.
    note: Option<String>,
}

impl GpuiInteropPresenter {
    /// A presenter in the [`Pending`](PresenterStatus::Pending) state.
    pub fn new() -> Self {
        Self {
            status: PresenterStatus::Pending,
            size: [640, 480],
            device: None,
            queue: None,
            renderer: None,
            #[cfg(target_os = "linux")]
            ring: None,
            #[cfg(target_os = "linux")]
            drew: false,
            #[cfg(not(target_os = "linux"))]
            surface: None,
            #[cfg(not(target_os = "linux"))]
            commands: None,
            #[cfg(not(target_os = "linux"))]
            presented: None,
            pending_reason: None,
            disable_after_present: None,
            note: None,
        }
    }

    /// Reallocate the renderer's MSAA/depth targets and the presentation targets
    /// after a resize.
    fn rebuild(&mut self) {
        let Some(device) = self.device.as_ref().cloned() else {
            return;
        };
        // A command buffer encoded for the old target must not reach the new one.
        self.pending_reason = Some("target rebuilt");
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize(self.size);
        }

        #[cfg(target_os = "linux")]
        {
            self.drew = false;
            let Some(ring) = self.ring.as_mut() else {
                self.status = PresenterStatus::Unsupported;
                return;
            };
            match ring.resize(&device, self.size[0], self.size[1]) {
                Ok(()) => trace::presenter_ring("dma-buf + sync_file", ring.len(), self.size, true),
                Err(error) => {
                    eprintln!("ferrocad: dma-buf ring resize failed: {error:#}");
                    self.note = Some(format!("dma-buf ring resize: {error:#}"));
                    self.status = PresenterStatus::Unsupported;
                    trace::presenter_disabled("ring resize failed");
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            // An encoded frame and an imported handle both belong to the old target.
            self.commands = None;
            self.presented = None;
            let Some(surface) = self.surface.as_mut() else {
                self.status = PresenterStatus::Unsupported;
                return;
            };
            match surface.resize(&device, self.size[0], self.size[1]) {
                Ok(_) => trace::presenter_ring("same-device view", 1, self.size, false),
                Err(error) => {
                    eprintln!("ferrocad: same-device view resize failed: {error:#}");
                    self.note = Some(format!("same-device surface resize: {error:#}"));
                    self.status = PresenterStatus::Unsupported;
                    trace::presenter_disabled("surface resize failed");
                }
            }
        }
    }

    /// The handle to paint this paint (Linux dma-buf path).
    ///
    /// A paint after [`Presenter::draw`] asks the ring for the newest fence-free
    /// frame (repeating the last good one when nothing is ready); a paused repaint
    /// uses the last good frame without advancing the ring.
    #[cfg(target_os = "linux")]
    fn present_frame(&mut self) -> Option<DmaBufHandle> {
        let drew = std::mem::replace(&mut self.drew, false);
        match self.ring.as_mut() {
            Some(ring) if drew => ring.present(),
            Some(ring) => ring.last(),
            None => None,
        }
    }
}

impl Default for GpuiInteropPresenter {
    fn default() -> Self {
        Self::new()
    }
}

/// Time from submitting a frame to the GPU reporting it finished.
///
/// The only frame cost no CPU timer can see: `Presenter::draw` returns as soon as the
/// command buffer is submitted, so a render that takes 60 ms on the device looks instant
/// in `prepare` and `paint` while the compositor, which samples the result, waits for it
/// and delays the next frame. This is what separates "no pass owns the time" from "the
/// pass is on the GPU".
#[cfg(target_os = "linux")]
fn note_gpu_frame(queue: &wgpu::Queue) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static TOTAL_NANOS: AtomicU64 = AtomicU64::new(0);
    static COUNT: AtomicU64 = AtomicU64::new(0);
    static WORST_NANOS: AtomicU64 = AtomicU64::new(0);
    static WINDOW: Mutex<Option<Instant>> = Mutex::new(None);

    if !frame_timing::enabled() {
        return;
    }
    let submitted = Instant::now();
    queue.on_submitted_work_done(move || {
        let nanos = submitted.elapsed().as_nanos() as u64;
        TOTAL_NANOS.fetch_add(nanos, Ordering::Relaxed);
        WORST_NANOS.fetch_max(nanos, Ordering::Relaxed);
        COUNT.fetch_add(1, Ordering::Relaxed);
    });

    let Ok(mut window) = WINDOW.lock() else {
        return;
    };
    let Some(start) = *window else {
        *window = Some(submitted);
        return;
    };
    if submitted.duration_since(start) < Duration::from_secs(1) {
        return;
    }
    let frames = COUNT.swap(0, Ordering::Relaxed).max(1);
    let total = TOTAL_NANOS.swap(0, Ordering::Relaxed);
    let worst = WORST_NANOS.swap(0, Ordering::Relaxed);
    eprintln!(
        "ferrocad: gpu render avg {:.1} ms (worst {:.1} ms, {} frames)",
        total as f64 / frames as f64 / 1e6,
        worst as f64 / 1e6,
        frames
    );
    *window = Some(submitted);
}

impl Presenter for GpuiInteropPresenter {
    fn status(&self) -> PresenterStatus {
        self.status
    }

    fn status_note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    fn set_size(&mut self, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if size == self.size {
            return;
        }
        self.size = size;
        if self.status == PresenterStatus::Ready {
            self.rebuild();
        }
    }

    fn prepare(&mut self, gpu: &mut GpuCanvasContext) {
        if self.status != PresenterStatus::Pending {
            return;
        }

        // A diagnostic, not a mode: `FERROCAD_PRESENTER=image` keeps the fallback the
        // presenter replaced, which renders and reads the frame back instead of handing a
        // dma-buf to the window. The two paths disagree about almost nothing except *how*
        // the frame reaches the compositor, so running both separates the dma-buf hand-off
        // and the window's frame pacing from everything above them. Compare the `gap` in
        // the window line between the two runs.
        if std::env::var("FERROCAD_PRESENTER").is_ok_and(|value| value == "image") {
            self.note = Some("FERROCAD_PRESENTER=image: the dma-buf ring is off".to_string());
            self.status = PresenterStatus::Unsupported;
            trace::presenter_disabled("FERROCAD_PRESENTER=image");
            if std::env::var_os("FERROCAD_TIMING").is_some() {
                eprintln!("ferrocad: presenting through the image path (FERROCAD_PRESENTER=image)");
            }
            return;
        }

        // The window's renderer, and so its device, exists only during paint.
        let Ok(interop) = attach(gpu.try_device::<WindowRenderer>()) else {
            self.note = Some(
                "the window renderer lent no GPU device (gpui's window renderer is not wgpu)"
                    .to_string(),
            );
            self.status = PresenterStatus::Unsupported;
            trace::presenter_disabled("no window device");
            return;
        };
        let Some((device, queue)) = interop.adapter().wgpu() else {
            self.note = Some(
                "the window renderer's device is not a wgpu device, so it cannot export a \
                 dma-buf or sample the producer's target"
                    .to_string(),
            );
            self.status = PresenterStatus::Unsupported;
            trace::presenter_disabled("window device is not wgpu");
            return;
        };

        let size = self.size;

        // The frame rate ceiling is the *window's* renderer: it composites the dma-buf, and
        // it may have picked a different adapter than the one FerroCAD's settings name
        // (those only reach the offscreen/image path). Name it once, on the timing switch.
        if std::env::var_os("FERROCAD_TIMING").is_some() {
            let info = device.adapter_info();
            eprintln!(
                "ferrocad: window adapter {:?} ({:?}, {:?})",
                info.name, info.device_type, info.backend
            );
        }

        // Allocate the presentation target first: the same-device path takes the
        // renderer's colour format from it, so the two cannot disagree (WebGPU
        // canvases forbid sRGB surface formats, and a renderer built for the wrong
        // one fails validation instead of drawing).

        #[cfg(target_os = "linux")]
        {
            match WgpuSharedBufferRing::new(&device, size[0], size[1], FORMAT) {
                Ok(ring) => {
                    eprintln!(
                        "ferrocad: viewport presenting through dma-buf ring ({} slots)",
                        ring.len()
                    );
                    trace::presenter_ring("dma-buf + sync_file", ring.len(), size, true);
                    self.ring = Some(ring);
                }
                Err(error) => {
                    eprintln!("ferrocad: dma-buf viewport unavailable ({error:#}); {FALLBACK}");
                    self.note = Some(format!("dma-buf ring: {error:#}"));
                    self.status = PresenterStatus::Unsupported;
                    trace::presenter_disabled("dma-buf ring unavailable");
                    return;
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            match WgpuSurface::new(&device, size[0], size[1]) {
                Ok(surface) => {
                    eprintln!("ferrocad: viewport presenting through same-device view");
                    trace::presenter_ring("same-device view", 1, size, false);
                    self.surface = Some(surface);
                }
                Err(error) => {
                    eprintln!(
                        "ferrocad: same-device viewport unavailable ({error:#}); \
                         {FALLBACK}"
                    );
                    self.note = Some(format!("same-device surface: {error:#}"));
                    self.status = PresenterStatus::Unsupported;
                    trace::presenter_disabled("surface allocation failed");
                    return;
                }
            }
        }

        // The renderer draws into the target above, so it is built for the same
        // colour format and the same device.
        #[cfg(target_os = "linux")]
        let color_format = FORMAT;
        #[cfg(not(target_os = "linux"))]
        let color_format = self
            .surface
            .as_ref()
            .map(WgpuSurface::format)
            .unwrap_or(WgpuSurface::DEFAULT_FORMAT);
        let renderer = FerrocadRenderer::with_sample_count_and_format(
            GpuContext::new(device.as_ref().clone(), queue.as_ref().clone()),
            size,
            samples(),
            color_format,
        );

        self.device = Some(device);
        self.queue = Some(queue);
        self.renderer = Some(renderer);
        self.note = None;
        self.status = PresenterStatus::Ready;
    }

    fn draw(&mut self, views: &[ViewSpec<'_>]) -> bool {
        if self.status != PresenterStatus::Ready {
            return false;
        }

        #[cfg(target_os = "linux")]
        {
            self.drew = false;
            let Some(queue) = self.queue.as_ref().cloned() else {
                return false;
            };
            let Some(ring) = self.ring.as_mut() else {
                return false;
            };
            let Some(frame) = ring.begin() else {
                // Every slot is in flight: this is a skipped producer frame, not an
                // error. `present` repeats the last good frame.
                let depth = ring.len();
                let reason = "every ring slot may still be in flight";
                self.pending_reason = Some(reason);
                trace::presenter_skip(depth, reason);
                // A skipped frame is the ring pacing the producer. The slots come back
                // early from the consumer's release signal and late from the timeout, so
                // `depth / timeout` is the frame rate a broken handshake would hold us to
                // — which is the first thing to rule out when frames are slow and no pass
                // owns the time.
                if frame_timing::enabled() {
                    frame_timing::note_presenter_skip();
                    eprintln!("ferrocad: ring skip (no free slot of {depth})");
                }
                return ring.last().is_some();
            };
            let Some(renderer) = self.renderer.as_mut() else {
                return false;
            };
            match renderer.encode_views(frame.view(), views) {
                Ok(commands) => match frame.submit(queue.as_ref(), Some(commands)) {
                    Ok(()) => {
                        note_gpu_frame(queue.as_ref());
                        self.drew = true;
                        self.pending_reason = None;
                        self.disable_after_present = None;
                        true
                    }
                    Err(error) => {
                        eprintln!("ferrocad: dma-buf submit failed: {error:#}");
                        self.pending_reason = Some("submit failed");
                        self.disable_after_present.get_or_insert("submit failed");
                        ring.last().is_some()
                    }
                },
                Err(error) => {
                    // The frame was not submitted; release the slot before asking
                    // for the last complete one.
                    drop(frame);
                    eprintln!("ferrocad: zero-copy encode failed: {error:?}");
                    self.pending_reason = Some("encode failed");
                    self.disable_after_present.get_or_insert("encode failed");
                    ring.last().is_some()
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let Some(renderer) = self.renderer.as_mut() else {
                return false;
            };
            let Some(surface) = self.surface.as_ref() else {
                return false;
            };
            match renderer.encode_views(surface.view(), views) {
                Ok(commands) => {
                    self.commands = Some(commands);
                    self.pending_reason = None;
                    self.disable_after_present = None;
                    true
                }
                Err(error) => {
                    eprintln!("ferrocad: zero-copy encode failed: {error:?}");
                    self.pending_reason = Some("encode failed");
                    self.disable_after_present.get_or_insert("encode failed");
                    self.presented.is_some()
                }
            }
        }
    }

    fn present(&mut self, gpu: &mut GpuCanvasContext) {
        if self.status != PresenterStatus::Ready {
            return;
        }

        #[cfg(target_os = "linux")]
        {
            match self.present_frame() {
                Some(handle) => gpu.paint_surface(handle),
                None => {
                    let reason = self
                        .pending_reason
                        .take()
                        .unwrap_or("no completed frame yet");
                    trace::presenter_blank(reason);
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let mut painted = false;
            if let Some(commands) = self.commands.take() {
                let Some(queue) = self.queue.as_ref() else {
                    return;
                };
                let Some(surface) = self.surface.as_ref() else {
                    return;
                };
                queue.submit(Some(commands));
                match surface.view().to_imported_handle() {
                    Ok(handle) => {
                        self.presented = Some(handle.clone());
                        gpu.paint_texture(handle, Corners::default(), 1.0, false);
                        painted = true;
                    }
                    Err(error) => {
                        eprintln!("ferrocad: imported viewport texture failed: {error:#}");
                        // The handle is not trustworthy for a repaint either.
                        self.presented = None;
                        self.disable_after_present.get_or_insert("import failed");
                    }
                }
            }

            // No new frame: repeat the last complete one rather than paint an empty
            // canvas. This is also the path a paint after `rebuild` takes.
            if !painted {
                if let Some(handle) = self.presented.clone() {
                    gpu.paint_texture(handle, Corners::default(), 1.0, false);
                } else {
                    let reason = self
                        .pending_reason
                        .take()
                        .unwrap_or("no frame drawn since the last present");
                    trace::presenter_blank(reason);
                }
            }
        }

        if let Some(reason) = self.disable_after_present.take() {
            self.status = PresenterStatus::Unsupported;
            trace::presenter_disabled(reason);
        }
    }

    fn pick_at(&mut self, queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
        // The G-buffer pick reads a texel back with a blocking `poll`, which a browser
        // cannot complete. `supports_pick` reports that, and the viewport falls back to
        // its CPU ray cast; this guard keeps a caller that ignores it from hanging.
        #[cfg(target_family = "wasm")]
        {
            let _ = (queue, camera, device);
            None
        }
        #[cfg(not(target_family = "wasm"))]
        {
            if self.status != PresenterStatus::Ready {
                return None;
            }
            // The renderer already lives on the window's device, so this is the same
            // G-buffer pass the image path uses, just without a second device.
            self.renderer.as_mut()?.pick_at(queue, camera, device)
        }
    }

    fn hover_pick_at(
        &mut self,
        queue: &RenderQueue,
        camera: &Camera,
        device: [i32; 2],
    ) -> Option<Option<Hit>> {
        // Same guard as `pick_at`: a browser cannot complete a mapping on this turn, and
        // `supports_pick` is false there anyway.
        #[cfg(target_family = "wasm")]
        {
            let _ = (queue, camera, device);
            None
        }
        #[cfg(not(target_family = "wasm"))]
        {
            if self.status != PresenterStatus::Ready {
                return None;
            }
            // The deferred pick never blocks on the window device's queue.
            self.renderer.as_mut()?.hover_pick_at(queue, camera, device)
        }
    }

    fn hover_pick_pending(&self) -> bool {
        // Same guard as `hover_pick_at`: a browser never has one in flight.
        #[cfg(target_family = "wasm")]
        {
            false
        }
        #[cfg(not(target_family = "wasm"))]
        {
            self.renderer
                .as_ref()
                .is_some_and(|renderer| renderer.hover_pick_pending())
        }
    }

    fn supports_pick(&self) -> bool {
        // A browser pick has no synchronous readback: the pick targets live on the GPU
        // and `map_async` only completes on a later event-loop turn. Reporting `false`
        // here is what makes the viewport use its CPU ray cast instead, which is pure
        // math over the same queue and camera.
        #[cfg(target_family = "wasm")]
        {
            false
        }
        #[cfg(not(target_family = "wasm"))]
        {
            self.status == PresenterStatus::Ready && self.renderer.is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The ring and surface paths need a wgpu device, so they are exercised by
    // `gpui_interop`'s own GPU tests and by a live window, not here. These tests
    // cover the device-free state machine that decides whether a paint has a frame
    // at all and whether the presenter has left `Pending`.

    #[test]
    fn a_presenter_without_a_device_keeps_the_image_path() {
        let mut presenter = GpuiInteropPresenter::new();
        assert_eq!(presenter.status(), PresenterStatus::Pending);
        assert!(!presenter.draw(&[]), "draw is impossible before prepare");
        assert!(!presenter.supports_pick());
        presenter.set_size([1280, 720]);
        assert_eq!(presenter.size, [1280, 720]);
        assert_eq!(
            presenter.status(),
            PresenterStatus::Pending,
            "a size change alone must not make the presenter Ready"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_paint_before_the_first_present_has_no_handle() {
        let mut presenter = GpuiInteropPresenter::new();
        // There is no ring before `prepare`, so a paint must yield no handle at all
        // rather than an empty or placeholder one.
        assert!(
            presenter.present_frame().is_none(),
            "a fresh presenter must not fabricate a frame"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_paused_repaint_does_not_consume_a_draw() {
        let mut presenter = GpuiInteropPresenter::new();
        // `drew` is only set by a successful `draw`; without a ring the result stays
        // `None` and the flag is cleared, so the next paint remains a paused repaint.
        assert!(presenter.present_frame().is_none());
        assert!(!presenter.drew);
        assert!(presenter.present_frame().is_none());
        assert!(!presenter.drew);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_same_device_presenter_starts_pending_and_has_no_handle() {
        let presenter = GpuiInteropPresenter::new();
        assert_eq!(presenter.status(), PresenterStatus::Pending);
        assert!(presenter.surface.is_none());
        assert!(presenter.presented.is_none());
        assert!(presenter.commands.is_none());
    }
}
