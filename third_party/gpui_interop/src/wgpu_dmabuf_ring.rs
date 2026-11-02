//! A ring of [`WgpuSharedBuffer`]s for a producer that renders continuously.
//!
//! [`WgpuSharedBuffer::submit`] only *starts* the write. Handing that handle to
//! GPUI while the GPU is still working lets the consumer's 16 ms acquire-fence
//! poll time out and drop the surface — a black frame. The same device does not
//! help: the poll is on the CPU, before GPUI's draw is submitted.
//!
//! This ring removes both halves of that failure:
//!
//! - it holds several slots, so a new frame never writes the buffer GPUI is
//!   sampling. A slot is reused only after the consumer's release descriptor is
//!   signalled, or after [`RELEASE_TIMEOUT`] if a release was lost;
//! - [`WgpuSharedBufferRing::present`] returns a frame whose acquire fence has
//!   **already** signalled, with the fence removed so GPUI imports immediately.
//!   If no submitted frame is ready, it returns the last good one instead, so the
//!   producer can run long without the viewport going black.
//!
//! The producer renders when [`begin`](Self::begin) returns a frame and submits
//! it; the consumer asks [`present`](Self::present) on its own paint cadence.
//! A frame that is still running is skipped, not waited for.
//!
//! ```no_run
//! # use gpui_interop::WgpuSharedBufferRing;
//! # fn draw(
//! #     ring: &mut WgpuSharedBufferRing,
//! #     device: &wgpu::Device,
//! #     queue: &wgpu::Queue,
//! #     encode: impl FnOnce(&wgpu::TextureView) -> wgpu::CommandBuffer,
//! #     gpu: &mut gpui::GpuCanvasContext,
//! # ) -> anyhow::Result<()> {
//! if let Some(frame) = ring.begin() {
//!     let commands = encode(frame.view());
//!     frame.submit(queue, Some(commands))?;
//! }
//! if let Some(handle) = ring.present() {
//!     gpu.paint_surface(handle);
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use gpui_engine::DmaBufHandle;

use crate::wgpu_dmabuf::WgpuSharedBuffer;

/// How long a slot waits for the consumer's release before it is reclaimed.
///
/// Long enough for a busy paint, short enough that a lost release does not stop
/// the producer for good. A slot whose frame was never presented also becomes
/// reusable after this, because its release will never be signalled.
const RELEASE_TIMEOUT: Duration = Duration::from_millis(250);

/// A small ring of exportable frames for a producer that renders continuously.
///
/// The ring owns the [`WgpuSharedBuffer`]s and the release `eventfd` GPUI writes
/// once it has sampled a slot. It is the producer-side counterpart of
/// `paint_surface`: render into [`begin`](Self::begin)'s view, submit it, and
/// hand [`present`](Self::present)'s result to GPUI.
pub struct WgpuSharedBufferRing {
    slots: Vec<Slot>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    /// Submitted frames, oldest first. A frame leaves when it is presented or
    /// expires; until then its slot is not reused.
    pending: VecDeque<(usize, DmaBufHandle)>,
    /// The last frame GPUI was shown. Kept so a slow producer can repeat it
    /// instead of presenting a black or unfinished buffer.
    last_good: Option<(usize, DmaBufHandle)>,
}

struct Slot {
    buffer: WgpuSharedBuffer,
    release: Option<Arc<OwnedFd>>,
    submitted_at: Option<Instant>,
    handed_at: Option<Instant>,
}

impl Slot {
    /// Whether the producer may write this slot again.
    fn available(&self, now: Instant) -> bool {
        let Some(release) = &self.release else {
            return true;
        };
        if signalled(release) {
            return true;
        }
        self.handed_at
            .or(self.submitted_at)
            .is_some_and(|at| now.duration_since(at) >= RELEASE_TIMEOUT)
    }
}

/// One free slot, handed out by [`WgpuSharedBufferRing::begin`].
///
/// The borrow stops the producer from starting a second frame until this one is
/// submitted, so two frames can never address the same slot.
pub struct RingFrame<'a> {
    ring: &'a mut WgpuSharedBufferRing,
    index: usize,
}

impl RingFrame<'_> {
    /// The texture view to render this frame through.
    pub fn view(&self) -> &wgpu::TextureView {
        self.ring.slots[self.index].buffer.view()
    }

    /// Submit the commands that rendered into [`view`](Self::view), attaching
    /// the acquire fence and a fresh release descriptor to the slot.
    pub fn submit(
        self,
        queue: &wgpu::Queue,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> Result<()> {
        self.ring.submit(self.index, queue, commands)
    }
}

impl WgpuSharedBufferRing {
    /// A three-slot ring: one presented, one GPUI may still be sampling, one free.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        Self::with_slots(device, width, height, format, 3)
    }

    /// A ring with `slots` exportable images. Two is the minimum; three leaves
    /// room for a frame in flight while the consumer samples the previous one.
    pub fn with_slots(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        slots: usize,
    ) -> Result<Self> {
        if slots < 2 {
            bail!("a shared-buffer ring needs at least two slots, but {slots} were asked for");
        }
        let mut ring = Self {
            slots: Vec::with_capacity(slots),
            width,
            height,
            format,
            pending: VecDeque::new(),
            last_good: None,
        };
        for _ in 0..slots {
            ring.slots.push(Slot {
                buffer: WgpuSharedBuffer::new(device, width, height, format)?,
                release: None,
                submitted_at: None,
                handed_at: None,
            });
        }
        Ok(ring)
    }

    /// The ring's size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The surface format the slots were allocated in.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// How many slots the ring holds.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the ring has no slots (never true for a constructed ring).
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Frames submitted but not yet presented.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Reallocate every slot after a resize. Frames in flight are dropped; the
    /// caller presents again once the new size has rendered.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Result<()> {
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        let count = self.slots.len();
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            slots.push(Slot {
                buffer: WgpuSharedBuffer::new(device, width, height, self.format)?,
                release: None,
                submitted_at: None,
                handed_at: None,
            });
        }
        self.slots = slots;
        self.pending.clear();
        self.last_good = None;
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// A free slot to render into, or `None` when every slot is in flight.
    ///
    /// `None` is a skipped producer frame, not an error: call
    /// [`present`](Self::present) anyway, which repeats the last good frame.
    pub fn begin(&mut self) -> Option<RingFrame<'_>> {
        let now = Instant::now();
        self.expire(now);
        let last = self.last_good.as_ref().map(|(index, _)| *index);
        let index = (0..self.slots.len()).find(|&index| {
            Some(index) != last
                && !self.pending.iter().any(|(pending, _)| *pending == index)
                && self.slots[index].available(now)
        })?;
        Some(RingFrame { ring: self, index })
    }

    /// The handle to paint this paint.
    ///
    /// The newest submitted frame whose acquire fence has already signalled, with
    /// that fence removed so GPUI imports it without polling. If none is ready,
    /// the last good frame — so a frame that runs long never turns into a black
    /// viewport. `None` only before the first frame is ready.
    pub fn present(&mut self) -> Option<DmaBufHandle> {
        let now = Instant::now();
        self.expire(now);
        let ready = self
            .pending
            .iter()
            .rposition(|(_, handle)| fence_ready(handle));
        if let Some(position) = ready {
            // A frame older than the one being shown has been overtaken; drop it
            // rather than presenting it later and jumping backwards. Its slot
            // becomes reusable through the release timeout.
            self.pending.drain(..position);
            let (index, mut handle) = self.pending.pop_front().expect("just found");
            handle.acquire_fence = None;
            self.slots[index].handed_at = Some(now);
            self.last_good = Some((index, handle.clone()));
            return Some(handle);
        }
        self.last_good.as_ref().map(|(_, handle)| handle.clone())
    }

    /// The last frame GPUI was shown, without a fence. For a paused producer that
    /// still needs to repaint its canvas.
    pub fn last(&self) -> Option<DmaBufHandle> {
        self.last_good.as_ref().map(|(_, handle)| handle.clone())
    }

    fn submit(
        &mut self,
        index: usize,
        queue: &wgpu::Queue,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> Result<()> {
        let slot = &mut self.slots[index];
        let mut handle = slot.buffer.submit(queue, commands)?;
        let release = release_descriptor()?;
        handle.release = Some(Arc::clone(&release));
        slot.release = Some(release);
        slot.submitted_at = Some(Instant::now());
        slot.handed_at = None;
        self.pending.push_back((index, handle));
        Ok(())
    }

    /// Drop submitted frames that have been waiting longer than the release
    /// timeout. Their release will never come, so their slots become reusable
    /// through the same timeout.
    fn expire(&mut self, now: Instant) {
        if self.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        for (index, handle) in pending {
            let expired = self.slots[index]
                .submitted_at
                .is_some_and(|at| now.duration_since(at) >= RELEASE_TIMEOUT);
            if !expired {
                self.pending.push_back((index, handle));
            }
        }
    }
}

/// An eventfd the renderer writes once it has sampled the slot.
fn release_descriptor() -> Result<Arc<OwnedFd>> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `eventfd` returned a fresh descriptor, whose ownership moves here.
    Ok(Arc::new(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Whether a descriptor has become readable, without blocking.
fn signalled(fd: &OwnedFd) -> bool {
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        match unsafe { libc::poll(&mut descriptor, 1, 0) } {
            1.. => return true,
            0 => return false,
            _ => {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
        }
    }
}

/// Whether a submitted frame's acquire fence has already signalled.
fn fence_ready(handle: &DmaBufHandle) -> bool {
    handle
        .acquire_fence
        .as_ref()
        .is_none_or(|fence| signalled(fence))
}
