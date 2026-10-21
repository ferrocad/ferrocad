//! Zero-copy viewport presentation through gpui's window renderer.
//!
//! The viewport renders into a GPU target on the **window's own wgpu device** and
//! hands the frame to gpui without a CPU copy:
//!
//! - Linux: a `WgpuSharedBuffer` exports the frame as a dma-buf and
//!   `paint_surface` imports it (the same-adapter, cross-device shape). Where the
//!   device cannot export (a GL adapter, or a driver without the Vulkan external
//!   memory extensions), it falls back to a `WgpuSurface` sampled with
//!   `paint_texture` on the same device.
//! - Windows/FreeBSD: the same-device `WgpuSurface` path, which requires the window
//!   itself to be running gpui's wgpu renderer.
//!
//! Either way the renderer runs on `Adapter::wgpu()`: it must be the window
//! renderer's device, because a wgpu texture cannot be exported and a texture from
//! another device cannot be sampled by gpui.
//!
//! # The presenting ring
//!
//! gpui samples the target after the canvas callback returns, and may still be
//! sampling it while the application paints the next frame. `gpui_interop` warns
//! that a buffer "must not be written again until the renderer has sampled the
//! frame (keep a ring of buffers for a producer that renders continuously)". A
//! single output written on every paint races that sampling; the Coin grid's
//! full-screen fragment shader makes frames long enough for the race to show as an
//! intermittent black frame.
//!
//! The presenter therefore owns [`RING_DEPTH`] targets allocated together and
//! resized together, and cycles frames through them. A slot is reused only when
//!
//! 1. the renderer has written the slot's **release** descriptor (the dma-buf
//!    path: gpui signals it from `on_submitted_work_done` once the submission that
//!    sampled the buffer has completed), or
//! 2. [`RING_DEPTH`] submissions have gone into the other slots since this one was
//!    last written. The same-device path has no release descriptor, so it always
//!    uses this conservative rule.
//!
//! A paint with nothing new to show repaints the most recent complete frame rather
//! than an empty canvas; see [`GpuiInteropPresenter::repaint`].
//!
//! This module is compiled only where gpui's wgpu renderer is available (not macOS
//! or wasm). Everywhere else [`zero_copy_presenter`](crate::zero_copy_presenter)
//! returns `None` and the viewport keeps its readback image path.

use std::sync::Arc;

#[cfg(target_os = "linux")]
use std::collections::VecDeque;

use ferrocad_inventor::{Camera, Hit, RenderQueue, ViewSpec};
use ferrocad_viewport::{Presenter, PresenterStatus};
use ferrocad_wgpu::{GpuContext, WgpuRenderer as FerrocadRenderer, trace};
use gpui::{Corners, GpuCanvasContext, ImportedTextureHandle};
#[cfg(target_os = "linux")]
use gpui::DmaBufHandle;
#[cfg(target_os = "linux")]
use gpui_interop::WgpuSharedBuffer;
use gpui_interop::{WgpuSurface, attach};
use gpui_wgpu::wgpu;
use gpui_wgpu::{ImportedTextureExt as _, WgpuRenderer as WindowRenderer};

#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream;

/// The viewport target's sRGB format, matching `ferrocad_wgpu`'s `TARGET_FORMAT`
/// and the imported-texture contract.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// MSAA count for the scene target. Matches the image path's 4× default.
const SAMPLES: u32 = 4;

/// How many targets the presenter cycles through.
///
/// Two would be the minimum for a producer that never waits; three leaves room for
/// a frame that is still queued behind gpui's own scene submission (the grid's
/// full-screen shader lengthens exactly that window). With the dma-buf release
/// signal a free slot is usually available sooner; the ring depth is the safety
/// net when the signal is absent (same-device path) or delayed.
const RING_DEPTH: usize = 3;

/// Build the host presenter. Installing it is always safe: if the window has no
/// compatible device, it reports [`PresenterStatus::Unsupported`] and the viewport
/// keeps the image path.
pub fn presenter() -> Box<dyn Presenter> {
    Box::new(GpuiInteropPresenter::new())
}

/// Where a frame goes once it has been rendered.
enum Output {
    /// The exported dma-buf shape (Linux): render into `view()`, submit through
    /// `WgpuSharedBuffer`, and hand gpui the fence-carrying handle.
    #[cfg(target_os = "linux")]
    Exported(Box<WgpuSharedBuffer>),
    /// The same-device shape: gpui samples the target's texture directly.
    SameDevice(WgpuSurface),
}

impl Output {
    fn view(&self) -> &wgpu::TextureView {
        match self {
            #[cfg(target_os = "linux")]
            Self::Exported(buffer) => buffer.view(),
            Self::SameDevice(surface) => surface.view(),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Exported(_) => "dma-buf + sync_file",
            Self::SameDevice(_) => "same-device view",
        }
    }

    /// Whether the slot can carry a consumer-release signal (dma-buf only).
    fn exported(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            matches!(self, Self::Exported(_))
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

/// One ring slot: a render target shared by every frame that uses the slot.
struct Slot {
    output: Output,
}

impl Slot {
    fn new(output: Output) -> Self {
        Self { output }
    }
}

/// The completion signal for **one submission** of a dma-buf slot.
///
/// `DmaBufHandle::release` is "a descriptor the renderer signals once it has
/// finished sampling the buffer". The renderer only writes eight bytes to it, so
/// a non-blocking `UnixStream` pair is enough to observe it without a new
/// dependency: the writer's `OwnedFd` travels in the handle, and the reader is
/// read (never blocked on) before a slot is reused.
///
/// One signal per submission rather than one per slot is deliberate: a signal can
/// arrive just after the presenter looked, so a reused descriptor could be
/// mistaken for a release of the *new* content. A fresh descriptor per submission
/// makes every signal unambiguous. At most [`RING_DEPTH`] are consulted; a few
/// replaced ones are retired briefly so a late write still finds an open peer.
#[cfg(target_os = "linux")]
struct ReleaseSignal {
    reader: UnixStream,
    writer: Arc<OwnedFd>,
}

#[cfg(target_os = "linux")]
impl ReleaseSignal {
    fn new() -> std::io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        let writer: OwnedFd = writer.into();
        Ok(Self {
            reader,
            writer: Arc::new(writer),
        })
    }

    /// The descriptor the renderer writes to; the slot keeps a clone, so the
    /// reader never sees EOF while this signal is alive.
    fn writer(&self) -> Arc<OwnedFd> {
        self.writer.clone()
    }

    /// Consume a pending consumer signal; true if the renderer has sampled the
    /// submission this descriptor belongs to.
    fn take_signalled(&self) -> bool {
        let mut sink = [0u8; 8];
        match (&self.reader).read(&mut sink) {
            Ok(read) => read > 0,
            // A broken descriptor is not evidence of a release: fall back to the
            // conservative ring distance.
            Err(_) => false,
        }
    }
}

/// The slot-selection state shared by every frame.
///
/// Pure in-memory bookkeeping, so the reuse rule is testable without a GPU. The
/// `submissions` counter is the clock: `last_written` holds the submission number
/// that last wrote each slot.
struct Ring {
    next: usize,
    submissions: u64,
    last_written: [Option<u64>; RING_DEPTH],
}

/// A slot chosen for the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Choice {
    slot: usize,
    /// Submissions since the slot was last written, `None` on its first use.
    distance: Option<u64>,
    /// The slot was chosen because the consumer released it, not because the ring
    /// wrapped.
    via_release: bool,
}

impl Ring {
    fn new() -> Self {
        Self {
            next: 0,
            submissions: 0,
            last_written: [None; RING_DEPTH],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// The most recent submission number (zero before the first frame).
    fn submissions(&self) -> u64 {
        self.submissions
    }

    /// Choose the next slot to write.
    ///
    /// First pass: round-robin from `next`, taking an untouched slot or one the
    /// consumer has released. Second pass: the conservative rule — a slot is safe
    /// once [`RING_DEPTH`] submissions have happened since it was last written,
    /// which guarantees the consumer's submission for that content was enqueued
    /// long before this one.
    fn choose(&self, released: &[bool; RING_DEPTH]) -> Option<Choice> {
        let now = self.submissions + 1;
        for offset in 0..RING_DEPTH {
            let slot = (self.next + offset) % RING_DEPTH;
            match self.last_written[slot] {
                None => {
                    return Some(Choice {
                        slot,
                        distance: None,
                        via_release: false,
                    });
                }
                Some(last) if released[slot] => {
                    return Some(Choice {
                        slot,
                        distance: Some(now - last),
                        via_release: true,
                    });
                }
                Some(_) => {}
            }
        }
        for offset in 0..RING_DEPTH {
            let slot = (self.next + offset) % RING_DEPTH;
            if let Some(last) = self.last_written[slot] {
                let distance = now - last;
                if distance >= RING_DEPTH as u64 {
                    return Some(Choice {
                        slot,
                        distance: Some(distance),
                        via_release: false,
                    });
                }
            }
        }
        None
    }

    /// Record that `slot` was written, returning its submission number.
    fn record(&mut self, slot: usize) -> u64 {
        self.submissions += 1;
        self.last_written[slot] = Some(self.submissions);
        self.next = (slot + 1) % RING_DEPTH;
        self.submissions
    }
}

/// A frame that was successfully handed to gpui, kept so a paint without a new
/// frame can repeat it instead of showing an empty canvas.
#[derive(Clone)]
enum Presented {
    #[cfg(target_os = "linux")]
    DmaBuf(DmaBufHandle),
    SameDevice(ImportedTextureHandle),
}

/// The host's zero-copy presenter: a renderer on the window's wgpu device plus the
/// ring of surfaces it hands to gpui.
pub struct GpuiInteropPresenter {
    status: PresenterStatus,
    /// The element's physical size. It can arrive before the device does.
    size: [u32; 2],
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    renderer: Option<FerrocadRenderer>,
    slots: Vec<Slot>,
    /// The release signal of each slot's most recent submission, until that slot
    /// is written again (dma-buf path only).
    #[cfg(target_os = "linux")]
    in_flight: Vec<Option<ReleaseSignal>>,
    /// Observed consumer releases, one per slot, reset when that slot is written.
    /// Each signal belongs to exactly one submission (see [`ReleaseSignal`]), so
    /// this flag cannot be contaminated by a late signal from older content.
    #[cfg(target_os = "linux")]
    released: [bool; RING_DEPTH],
    /// Readers of recently replaced submissions, kept only so a renderer's late
    /// write still finds its peer (a closed peer would log an `EPIPE`). They are
    /// never consulted; the oldest are dropped once the list is long enough for
    /// any in-flight scene to have completed.
    #[cfg(target_os = "linux")]
    retired: VecDeque<ReleaseSignal>,
    ring: Ring,
    /// The last frame handed to gpui per slot, and the submission that produced it.
    /// `None` means the slot has no frame that is still safe to repaint.
    presented: Vec<Option<Presented>>,
    presented_at: Vec<Option<u64>>,
    /// The frame encoded by `draw`, submitted by `present` (the dma-buf path needs
    /// the command buffer so it can register its export fence on the submission).
    commands: Option<(usize, wgpu::CommandBuffer)>,
    /// Why the next `present` has no new frame, for the trace.
    pending_reason: Option<&'static str>,
    /// A draw failed; present the last complete frame, then let the image path take
    /// over. Retiring here rather than in `draw` is what keeps the current paint
    /// from rendering an empty canvas.
    disable_after_present: Option<&'static str>,
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
            slots: Vec::new(),
            #[cfg(target_os = "linux")]
            in_flight: Vec::new(),
            #[cfg(target_os = "linux")]
            released: [false; RING_DEPTH],
            #[cfg(target_os = "linux")]
            retired: VecDeque::new(),
            ring: Ring::new(),
            presented: Vec::new(),
            presented_at: Vec::new(),
            commands: None,
            pending_reason: None,
            disable_after_present: None,
        }
    }

    /// Allocate the ring. Every slot has the same size, and an export failure in
    /// any slot falls back to the same-device shape for the whole ring rather than
    /// leaving a mixed set.
    fn create_outputs(device: &wgpu::Device, size: [u32; 2]) -> Option<Vec<Output>> {
        #[cfg(target_os = "linux")]
        {
            match WgpuSharedBuffer::new(device, size[0], size[1], FORMAT) {
                Ok(first) => {
                    let mut outputs = vec![Output::Exported(Box::new(first))];
                    for index in 1..RING_DEPTH {
                        match WgpuSharedBuffer::new(device, size[0], size[1], FORMAT) {
                            Ok(buffer) => outputs.push(Output::Exported(Box::new(buffer))),
                            Err(error) => {
                                eprintln!(
                                    "ferrocad: dma-buf ring slot {index} unavailable \
                                     ({error:#}); using the same-device view for the ring"
                                );
                                return allocate_ring(|_| {
                                    WgpuSurface::new(device, size[0], size[1])
                                        .ok()
                                        .map(Output::SameDevice)
                                });
                            }
                        }
                    }
                    return Some(outputs);
                }
                Err(error) => {
                    eprintln!(
                        "ferrocad: dma-buf viewport unavailable ({error:#}); \
                         using the same-device view"
                    );
                }
            }
        }
        allocate_ring(|_| {
            WgpuSurface::new(device, size[0], size[1])
                .ok()
                .map(Output::SameDevice)
        })
    }

    /// Install a freshly allocated ring, forgetting every consumer state about the
    /// old targets (they may still be in flight, so they are simply dropped).
    fn install_slots(&mut self, outputs: Vec<Output>) {
        debug_assert_eq!(outputs.len(), RING_DEPTH);
        let release = outputs.iter().any(Output::exported);
        let label = outputs.first().map(Output::label).unwrap_or("none");
        self.slots = outputs.into_iter().map(Slot::new).collect();
        #[cfg(target_os = "linux")]
        {
            self.in_flight = (0..RING_DEPTH).map(|_| None).collect();
            self.released = [false; RING_DEPTH];
            self.retired.clear();
        }
        self.ring.reset();
        self.presented = vec![None; RING_DEPTH];
        self.presented_at = vec![None; RING_DEPTH];
        trace::presenter_ring(label, RING_DEPTH, self.size, release);
    }

    /// Reallocate the renderer's MSAA/depth targets and every ring slot after a
    /// resize. All slots change size together, so no old-size target can be
    /// sampled as if it were the new one.
    fn rebuild(&mut self) {
        let Some(device) = self.device.as_ref() else {
            return;
        };
        // A command buffer encoded for the old target must not reach the new one.
        self.commands = None;
        self.pending_reason = Some("target rebuilt");
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize(self.size);
        }
        match Self::create_outputs(device, self.size) {
            Some(outputs) => self.install_slots(outputs),
            None => {
                self.slots.clear();
                #[cfg(target_os = "linux")]
                {
                    self.in_flight.clear();
                    self.retired.clear();
                }
                self.presented.clear();
                self.presented_at.clear();
                self.status = PresenterStatus::Unsupported;
                trace::presenter_disabled("ring allocation failed on resize");
            }
        }
    }

    /// Which slots the consumer has released since their last write.
    ///
    /// Observing consumes the descriptor's pending bytes and latches the answer
    /// until the slot is written again, so a release spotted this frame is not
    /// forgotten if another slot is chosen first.
    fn release_states(&mut self) -> [bool; RING_DEPTH] {
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut released = [false; RING_DEPTH];
        #[cfg(target_os = "linux")]
        {
            for slot in 0..RING_DEPTH {
                let signalled = self
                    .in_flight
                    .get(slot)
                    .and_then(Option::as_ref)
                    .is_some_and(ReleaseSignal::take_signalled);
                self.released[slot] |= signalled;
                released[slot] = self.released[slot];
            }
        }
        released
    }

    /// Repaint the most recent complete frame.
    ///
    /// Returns `false` when no slot holds a frame that is still valid (nothing has
    /// been presented yet, or every presented slot was invalidated by a failure).
    fn repaint(&mut self, gpu: &mut GpuCanvasContext, reason: &'static str) -> bool {
        let Some(slot) = newest_presented(&self.presented_at) else {
            return false;
        };
        let Some(presented) = self.presented[slot].clone() else {
            return false;
        };
        match presented {
            #[cfg(target_os = "linux")]
            Presented::DmaBuf(handle) => gpu.paint_surface(handle),
            Presented::SameDevice(handle) => {
                gpu.paint_texture(handle, Corners::default(), 1.0, false)
            }
        }
        let age = self
            .ring
            .submissions()
            .saturating_sub(self.presented_at[slot].unwrap_or(0));
        trace::presenter_repaint(slot, age, reason);
        true
    }

    /// Submit the encoded frame into `slot` and hand it to gpui.
    fn paint_encoded(
        &mut self,
        gpu: &mut GpuCanvasContext,
        slot: usize,
        commands: wgpu::CommandBuffer,
    ) -> Result<(), ()> {
        let queue = self.queue.as_ref().cloned().ok_or(())?;
        let slot_ref = self.slots.get_mut(slot).ok_or(())?;
        let presented = match &mut slot_ref.output {
            #[cfg(target_os = "linux")]
            Output::Exported(buffer) => {
                // One descriptor per submission: see `ReleaseSignal`.
                let release = match ReleaseSignal::new() {
                    Ok(signal) => Some(signal),
                    Err(error) => {
                        eprintln!(
                            "ferrocad: viewport release signal unavailable ({error}); \
                             slots are reused after {RING_DEPTH} submissions"
                        );
                        None
                    }
                };
                let mut handle = buffer.submit(&queue, Some(commands)).map_err(|error| {
                    eprintln!("ferrocad: dma-buf submit failed: {error:#}");
                })?;
                if let Some(release) = release.as_ref() {
                    handle.release = Some(release.writer());
                }
                let presented = Presented::DmaBuf(handle.clone());
                gpu.paint_surface(handle);
                if let Some(previous) = std::mem::replace(&mut self.in_flight[slot], release) {
                    self.retired.push_back(previous);
                    while self.retired.len() > RING_DEPTH * 2 {
                        self.retired.pop_front();
                    }
                }
                self.released[slot] = false;
                presented
            }
            Output::SameDevice(surface) => {
                queue.submit(Some(commands));
                let handle = surface.view().to_imported_handle().map_err(|error| {
                    eprintln!("ferrocad: imported viewport texture failed: {error:#}");
                })?;
                let presented = Presented::SameDevice(handle.clone());
                gpu.paint_texture(handle, Corners::default(), 1.0, false);
                presented
            }
        };
        self.presented[slot] = Some(presented);
        self.presented_at[slot] = Some(self.ring.submissions());
        Ok(())
    }
}

impl Default for GpuiInteropPresenter {
    fn default() -> Self {
        Self::new()
    }
}

impl Presenter for GpuiInteropPresenter {
    fn status(&self) -> PresenterStatus {
        self.status
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

        // The window's renderer, and so its device, exists only during paint.
        let Ok(interop) = attach(gpu.try_device::<WindowRenderer>()) else {
            self.status = PresenterStatus::Unsupported;
            return;
        };
        let Some((device, queue)) = interop.adapter().wgpu() else {
            self.status = PresenterStatus::Unsupported;
            return;
        };

        let size = self.size;
        let renderer = FerrocadRenderer::with_sample_count(
            GpuContext::new(device.as_ref().clone(), queue.as_ref().clone()),
            size,
            SAMPLES,
        );
        let Some(outputs) = Self::create_outputs(&device, size) else {
            self.status = PresenterStatus::Unsupported;
            return;
        };
        eprintln!(
            "ferrocad: viewport presenting through {} ({} buffers)",
            outputs.first().map(Output::label).unwrap_or("no target"),
            outputs.len()
        );

        self.device = Some(device);
        self.queue = Some(queue);
        self.renderer = Some(renderer);
        self.install_slots(outputs);
        self.status = PresenterStatus::Ready;
    }

    fn draw(&mut self, views: &[ViewSpec<'_>]) -> bool {
        if self.status != PresenterStatus::Ready || self.slots.len() != RING_DEPTH {
            return false;
        }
        let released = self.release_states();
        let Some(choice) = self.ring.choose(&released) else {
            let reason = "every ring slot may still be in flight";
            self.pending_reason = Some(reason);
            trace::presenter_skip(RING_DEPTH, reason);
            // A repaint is still a presented frame from the viewport's point of
            // view; fall through to the image path only when there is none.
            return newest_presented(&self.presented_at).is_some();
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        let encoded = renderer.encode_views(self.slots[choice.slot].output.view(), views);
        match encoded {
            Ok(commands) => {
                let submission = self.ring.record(choice.slot);
                self.commands = Some((choice.slot, commands));
                self.pending_reason = None;
                self.disable_after_present = None;
                trace::presenter_slot(
                    trace::current_frame(),
                    choice.slot,
                    RING_DEPTH,
                    choice.distance,
                    choice.via_release,
                    submission,
                );
                true
            }
            Err(error) => {
                eprintln!("ferrocad: zero-copy encode failed: {error:?}");
                self.pending_reason = Some("encode failed");
                self.disable_after_present.get_or_insert("encode failed");
                false
            }
        }
    }

    fn present(&mut self, gpu: &mut GpuCanvasContext) {
        if let Some((slot, commands)) = self.commands.take() {
            match self.paint_encoded(gpu, slot, commands) {
                Ok(()) => {
                    // A failed draw can still leave an earlier encoded frame in
                    // `commands`; honour its retirement after presenting it.
                    if let Some(reason) = self.disable_after_present.take() {
                        self.status = PresenterStatus::Unsupported;
                        trace::presenter_disabled(reason);
                    }
                    return;
                }
                Err(()) => {
                    // The slot's content is not trustworthy (the submit may have
                    // partially happened), so drop it from the repaint set.
                    if let Some(presented) = self.presented.get_mut(slot) {
                        *presented = None;
                    }
                    if let Some(at) = self.presented_at.get_mut(slot) {
                        *at = None;
                    }
                    #[cfg(target_os = "linux")]
                    if let Some(signal) = self.in_flight.get_mut(slot) {
                        *signal = None;
                    }
                    #[cfg(target_os = "linux")]
                    {
                        self.released[slot] = false;
                    }
                    let reason = "present failed";
                    if !self.repaint(gpu, reason) {
                        trace::presenter_blank(reason);
                    }
                    self.status = PresenterStatus::Unsupported;
                    trace::presenter_disabled(reason);
                    return;
                }
            }
        }

        // No new frame: repeat the last complete one rather than paint an empty
        // canvas. This is also the path a paint after `rebuild` takes.
        let reason = self
            .pending_reason
            .take()
            .unwrap_or("no frame drawn since the last present");
        if !self.repaint(gpu, reason) {
            trace::presenter_blank(reason);
        }
        if let Some(reason) = self.disable_after_present.take() {
            self.status = PresenterStatus::Unsupported;
            trace::presenter_disabled(reason);
        }
    }

    fn pick_at(&mut self, queue: &RenderQueue, camera: &Camera, device: [i32; 2]) -> Option<Hit> {
        if self.status != PresenterStatus::Ready {
            return None;
        }
        // The renderer already lives on the window's device, so this is the same
        // G-buffer pass the image path uses, just without a second device.
        self.renderer.as_mut()?.pick_at(queue, camera, device)
    }

    fn supports_pick(&self) -> bool {
        self.status == PresenterStatus::Ready && self.renderer.is_some()
    }
}

/// Allocate exactly [`RING_DEPTH`] items, or none at all.
///
/// A partially allocated ring is never installed: an all-or-nothing result keeps
/// every slot at the same size and on the same transport.
fn allocate_ring<T>(allocate: impl FnMut(usize) -> Option<T>) -> Option<Vec<T>> {
    (0..RING_DEPTH).map(allocate).collect()
}

/// The most recently presented slot, if any slot still holds a valid frame.
fn newest_presented(presented_at: &[Option<u64>]) -> Option<usize> {
    presented_at
        .iter()
        .enumerate()
        .filter_map(|(slot, submission)| submission.map(|submission| (slot, submission)))
        .max_by_key(|&(_, submission)| submission)
        .map(|(slot, _)| slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_releases() -> [bool; RING_DEPTH] {
        [false; RING_DEPTH]
    }

    #[test]
    fn the_ring_fills_every_slot_round_robin() {
        let mut ring = Ring::new();
        let released = no_releases();
        for expected in 0..RING_DEPTH {
            let choice = ring.choose(&released).expect("an untouched slot is free");
            assert_eq!(choice.slot, expected);
            assert_eq!(choice.distance, None);
            assert!(!choice.via_release);
            assert_eq!(ring.record(choice.slot), expected as u64 + 1);
        }
        let wrapped = ring.choose(&released).expect("the ring wraps");
        assert_eq!(wrapped.slot, 0);
        assert_eq!(wrapped.distance, Some(RING_DEPTH as u64));
        assert!(!wrapped.via_release);
    }

    #[test]
    fn a_slot_is_not_reused_before_the_ring_depth() {
        let mut ring = Ring::new();
        let released = no_releases();
        let mut distances = Vec::new();
        for _ in 0..(RING_DEPTH * 4) {
            let choice = ring.choose(&released).expect("distance rule always frees one");
            distances.push(choice.distance);
            ring.record(choice.slot);
        }
        for distance in distances {
            assert!(
                distance.is_none_or(|distance| distance >= RING_DEPTH as u64),
                "a slot was reused after {distance:?} submissions; depth is {RING_DEPTH}"
            );
        }
    }

    #[test]
    fn a_slow_consumer_still_leaves_a_full_ring_of_frames() {
        // Model a consumer that never releases: the producer must be able to keep
        // drawing, but only after every other slot has been submitted, so the
        // buffer being sampled is never the one being overwritten within the ring.
        let mut ring = Ring::new();
        let released = no_releases();
        for frame in 0..12 {
            let choice = ring.choose(&released).expect("the ring keeps producing");
            assert_eq!(choice.slot, frame % RING_DEPTH);
            assert!(choice.distance.is_none_or(|d| d >= RING_DEPTH as u64));
            ring.record(choice.slot);
        }
    }

    #[test]
    fn an_explicit_release_allows_early_reuse() {
        let mut ring = Ring::new();
        let released = no_releases();
        for _ in 0..RING_DEPTH {
            let slot = ring.choose(&released).unwrap().slot;
            ring.record(slot);
        }
        // `next` is 0 again. Slot 1's consumer reported completion, so it may be
        // reused after two submissions instead of waiting for the ring to wrap.
        let mut released = no_releases();
        released[1] = true;
        let choice = ring.choose(&released).expect("a released slot is free");
        assert_eq!(choice.slot, 1);
        assert_eq!(choice.distance, Some(2));
        assert!(choice.via_release);
    }

    #[test]
    fn all_slots_in_flight_reports_none() {
        let mut ring = Ring::new();
        ring.submissions = 4;
        ring.last_written = [Some(3), Some(3), Some(3)];
        assert_eq!(ring.choose(&no_releases()), None);
    }

    #[test]
    fn a_reset_makes_every_slot_eligible_again() {
        let mut ring = Ring::new();
        let released = no_releases();
        for _ in 0..RING_DEPTH {
            let slot = ring.choose(&released).unwrap().slot;
            ring.record(slot);
        }
        ring.reset();
        assert_eq!(ring.submissions(), 0);
        for expected in 0..RING_DEPTH {
            let choice = ring.choose(&released).expect("reset clears every distance");
            assert_eq!(choice.slot, expected);
            assert_eq!(choice.distance, None);
            ring.record(choice.slot);
        }
    }

    #[test]
    fn a_resize_allocates_every_slot_or_none() {
        let mut attempts = 0;
        let slots = allocate_ring(|_| {
            attempts += 1;
            Some(attempts)
        })
        .expect("all slots allocate");
        assert_eq!(slots, (1..=RING_DEPTH as u32).collect::<Vec<_>>());

        let mut attempts = 0;
        let failed = allocate_ring(|_| {
            attempts += 1;
            (attempts < 2).then_some(attempts)
        });
        assert_eq!(failed, None);
        assert_eq!(
            attempts, 2,
            "allocation stops at the first failure; no partial ring is installed"
        );
    }

    #[test]
    fn the_fallback_picks_the_newest_valid_frame() {
        assert_eq!(newest_presented(&[None, Some(4), Some(7)]), Some(2));
        assert_eq!(newest_presented(&[Some(9), None, None]), Some(0));
        assert_eq!(newest_presented(&[None, None, None]), None);
    }

    #[test]
    fn a_presenter_without_a_device_keeps_the_image_path() {
        let mut presenter = GpuiInteropPresenter::new();
        assert_eq!(presenter.status(), PresenterStatus::Pending);
        assert!(!presenter.draw(&[]), "draw is impossible before prepare");
        assert!(!presenter.supports_pick());
        presenter.set_size([1280, 720]);
        assert_eq!(presenter.size, [1280, 720]);
        // Nothing was presented, so the blank path has nothing to repaint.
        assert_eq!(newest_presented(&presenter.presented_at), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_release_signal_reports_and_drains_a_consumer_signal() {
        use std::io::Write;

        let signal = ReleaseSignal::new().expect("a socket pair is available");
        assert!(!signal.take_signalled());
        let mut writer = std::fs::File::from(signal.writer().try_clone().unwrap());
        writer.write_all(&1u64.to_ne_bytes()).unwrap();
        assert!(signal.take_signalled(), "the write is the consumer signal");
        assert!(!signal.take_signalled(), "observing consumed the signal");
    }

}
