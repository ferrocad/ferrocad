//! Opt-in render diagnostics.
//!
//! Enable with `FERROCAD_TRACE=1` (any value except `0`/`false`/`off`) or the
//! shell's `--trace` flag. When disabled every entry point is an atomic load and a
//! branch: no per-frame allocation, formatting, or logging. When enabled the module
//! logs, to stderr:
//!
//! - the adapter, backend, driver, limits, target format and sample count at
//!   renderer creation;
//! - every wgpu validation/OOM/internal error and every device loss, tagged with
//!   the frame that was in flight;
//! - frame counters, view/packet/program counts, encode and submit events, and
//!   target resizes;
//! - every program pipeline creation and every group-2 bind;
//! - a black-frame probe over the pixels of a readback frame.
//!
//! The black-frame probe only runs where a readback exists (the image fallback
//! path and the tests). The zero-copy presenter hands its target straight to gpui
//! and never reads it back, so it cannot be probed without an extra GPU readback;
//! see `docs/render-diagnostics.md`.
//!
//! **Trace mode takes over wgpu's device callbacks.** A wgpu device has exactly
//! one `on_uncaptured_error` handler and one device-lost callback, and gpui's
//! window renderer installs its own (`bite-gp-wgpu`). In trace mode ours replaces
//! them on the device a renderer wraps, so the diagnostic log sees the error
//! instead of gpui's silent recorder. This is deliberate and is why trace mode is
//! a diagnosis tool, not something to ship.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// The environment variable that switches tracing on.
pub const ENV: &str = "FERROCAD_TRACE";

/// `0` = never initialised, `1` = off, `2` = on.
static STATE: AtomicU8 = AtomicU8::new(0);

/// The frame counter; every encode/submit increments it.
static FRAME: AtomicU64 = AtomicU64::new(0);

/// How many wgpu errors (uncaptured or device-lost) have been seen.
static ERRORS: AtomicU64 = AtomicU64::new(0);

/// Set when the last readback was classified black, so a recovery is logged once.
static LAST_BLACK: AtomicU8 = AtomicU8::new(0);

fn clock() -> &'static Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now)
}

/// The elapsed milliseconds since the first trace call (the log's timestamp).
pub(crate) fn elapsed_ms() -> u128 {
    clock().elapsed().as_millis()
}

/// Whether tracing is enabled. Reads the environment on first use.
pub fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let on = std::env::var_os(ENV).is_some_and(|value| truthy(&value));
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// Force tracing on (`--trace`). Idempotent.
pub fn enable() {
    STATE.store(2, Ordering::Relaxed);
}

/// Whether `args` contains the shell's `--trace` flag.
///
/// Pure, so the flag plumbing is testable without touching process state.
pub fn args_enable<I: IntoIterator<Item = String>>(args: I) -> bool {
    args.into_iter().any(|arg| arg == "--trace")
}

/// `FERROCAD_TRACE` truthiness: unset/empty/`0`/`false`/`off` are off, and any
/// other value is on.
fn truthy(value: &std::ffi::OsStr) -> bool {
    let Some(value) = value.to_str() else {
        return true;
    };
    let value = value.trim();
    !(value.is_empty()
        || value == "0"
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("off"))
}

/// Log a line with the trace prefix and the elapsed timestamp.
macro_rules! trace_line {
    ($($arg:tt)*) => {
        eprintln!("ferrocad trace: t={}ms {}", $crate::trace::elapsed_ms(), format_args!($($arg)*))
    };
}

/// Log the adapter, driver, limits and target state of a renderer.
pub fn startup(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) {
    if !enabled() {
        return;
    }
    let info = device.adapter_info();
    let limits = device.limits();
    let features = device.features();
    trace_line!(
        "gpu: adapter={:?} backend={:?} device_type={:?} driver={:?} driver_info={:?}",
        info.name,
        info.backend,
        info.device_type,
        info.driver,
        info.driver_info
    );
    trace_line!(
        "gpu: target format={color_format:?} samples={sample_count} \
         min_uniform_buffer_offset_alignment={} max_uniform_buffer_binding_size={} \
         max_texture_dimension_2d={}",
        limits.min_uniform_buffer_offset_alignment,
        limits.max_uniform_buffer_binding_size,
        limits.max_texture_dimension_2d
    );
    trace_line!("gpu: limits={limits:?}");
    trace_line!("gpu: features={features:?}");
    // This renderer has no swapchain: it resolves into an offscreen texture (the image
    // path) or the presenter's exportable texture. The window surface belongs to gpui.
    trace_line!("surface: none (offscreen target; the window swapchain is gpui's)");
    trace_line!("gpu: frame counter starts at {}", FRAME.load(Ordering::Relaxed));
}

/// Install the wgpu error and device-lost callbacks (trace mode only).
///
/// A device has one of each; installing here replaces gpui's recorder on a shared
/// window device. See the module docs.
pub fn install(device: &wgpu::Device) {
    if !enabled() {
        return;
    }
    device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        let frame = FRAME.load(Ordering::Relaxed);
        let kind = match error {
            wgpu::Error::OutOfMemory { .. } => "out-of-memory",
            wgpu::Error::Validation { .. } => "validation",
            wgpu::Error::Internal { .. } => "internal",
        };
        // `Display` carries the validation label/context; `Debug` adds the chain.
        trace_line!("frame {frame}: wgpu {kind}: {error}");
        trace_line!("frame {frame}: wgpu {kind} detail: {error:?}");
    }));
    device.set_device_lost_callback(|reason, message| {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        let frame = FRAME.load(Ordering::Relaxed);
        trace_line!("frame {frame}: device lost: reason={reason:?} message={message}");
    });
    trace_line!("callbacks: installed on_uncaptured_error and device-lost");
}

/// Start a frame: increment the counter and log the batch shape.
///
/// `label` is `"render"`, `"views"` or `"encode"`; `views` is the number of
/// [`ViewSpec`](ferrocad_inventor::ViewSpec)s, `packets` the total draw packets and
/// `programs` the program packets among them.
pub fn frame_begin(label: &str, views: usize, packets: usize, programs: usize) -> u64 {
    if !enabled() {
        return 0;
    }
    let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
    trace_line!("frame {frame}: {label} views={views} packets={packets} programs={programs}");
    frame
}

/// The most recent frame number (zero before the first frame).
pub fn current_frame() -> u64 {
    FRAME.load(Ordering::Relaxed)
}

/// Log a successful command-buffer encode.
pub fn encoded(frame: u64, views: usize) {
    if enabled() {
        trace_line!("frame {frame}: encoded views={views}");
    }
}

/// Log a queue submission.
pub fn submitted(frame: u64, label: &str, index: wgpu::SubmissionIndex) {
    if enabled() {
        trace_line!("frame {frame}: submitted {label} index={:?}", index);
    }
}

/// Log the zero-copy presenter's ring at creation and after a resize.
///
/// `release` is whether the ring's transport can carry a completion descriptor
/// the renderer signals once it has finished sampling a slot (the dma-buf path).
pub fn presenter_ring(label: &str, depth: usize, size: [u32; 2], release: bool) {
    if enabled() {
        trace_line!(
            "presenter: ring depth={depth} target={label} size={}x{} release_capable={release}",
            size[0],
            size[1]
        );
    }
}

/// Log the ring slot a frame was encoded into.
///
/// `distance` is the number of submissions since that slot was last written
/// (`None` on the slot's first use), and `via_release` says whether the slot was
/// chosen because the consumer had explicitly released it rather than because
/// the conservative ring distance was reached.
pub fn presenter_slot(
    frame: u64,
    slot: usize,
    depth: usize,
    distance: Option<u64>,
    via_release: bool,
    submission: u64,
) {
    if !enabled() {
        return;
    }
    match distance {
        Some(distance) => trace_line!(
            "frame {frame}: presenter slot={slot} depth={depth} reuse_distance={distance} \
             via_release={via_release} submission={submission}"
        ),
        None => trace_line!(
            "frame {frame}: presenter slot={slot} depth={depth} first_use=true \
             submission={submission}"
        ),
    }
}

/// Log a frame that could not be drawn because every ring slot may still be in
/// flight (no release yet, and the conservative distance not reached).
pub fn presenter_skip(depth: usize, reason: &str) {
    if enabled() {
        trace_line!("presenter: skipped a frame ({reason}); depth={depth}");
    }
}

/// Log repainting the last complete frame instead of an empty canvas.
pub fn presenter_repaint(slot: usize, age: u64, reason: &str) {
    if enabled() {
        trace_line!("presenter: repainted last frame slot={slot} age={age} ({reason})");
    }
}

/// Log a canvas paint with no frame to show at all (no frame has ever been
/// presented, or every presented slot was invalidated by a failure).
pub fn presenter_blank(reason: &str) {
    if enabled() {
        trace_line!("presenter: nothing to present ({reason})");
    }
}

/// Log retiring the presenter after the current paint; the image path takes over.
pub fn presenter_disabled(reason: &str) {
    if enabled() {
        trace_line!("presenter: disabled after present ({reason}); the image path takes over");
    }
}

/// Log the outcome of a window-surface acquisition.
///
/// `ferrocad_wgpu` never owns a `wgpu::Surface` — it renders into an offscreen
/// texture (the image path) or the presenter's exportable texture (the zero-copy
/// path) — so this is unused in-tree. It exists for a host that does drive a
/// surface, and names the same states an older wgpu exposed through
/// `SurfaceError`: `Timeout`, `Occluded`, `Outdated`, `Lost` and `Validation`.
/// (`OutOfMemory` is now reported through
/// [`Device::on_uncaptured_error`](wgpu::Device::on_uncaptured_error), which
/// [`install`] covers.)
pub fn surface_status(context: &str, status: &wgpu::CurrentSurfaceTexture) {
    if !enabled() {
        return;
    }
    let outcome = match status {
        wgpu::CurrentSurfaceTexture::Success(_) => "acquired",
        wgpu::CurrentSurfaceTexture::Suboptimal(_) => "suboptimal",
        wgpu::CurrentSurfaceTexture::Timeout => "timeout",
        wgpu::CurrentSurfaceTexture::Occluded => "occluded",
        wgpu::CurrentSurfaceTexture::Outdated => "outdated",
        wgpu::CurrentSurfaceTexture::Lost => "lost",
        wgpu::CurrentSurfaceTexture::Validation => "validation",
    };
    let frame = FRAME.load(Ordering::Relaxed);
    trace_line!("frame {frame}: surface {context}: {outcome}");
}

/// Log a view's camera: the world-space eye and the projection parameters.
///
/// An orbit changes `eye`; a zoom changes the projection's `fov_y` (perspective)
/// or `extent` (orthographic). This is what tells a reproduction script whether
/// its input actually reached the viewport.
pub fn camera(frame: u64, view: usize, camera: &ferrocad_graphics::Camera) {
    if !enabled() {
        return;
    }
    let eye = camera
        .view
        .inverse()
        .map(|view| view.transform(&ferrocad_types::Vector3::zero()))
        .unwrap_or_else(ferrocad_types::Vector3::zero);
    trace_line!(
        "frame {frame}: view {view} eye=({:.3},{:.3},{:.3}) projection={:?}",
        eye.x,
        eye.y,
        eye.z,
        camera.projection
    );
}

/// Log a target resize.
pub fn resized(old: [u32; 2], new: [u32; 2], sample_count: u32) {
    if enabled() {
        trace_line!(
            "resize: {}x{} -> {}x{} samples={sample_count}",
            old[0],
            old[1],
            new[0],
            new[1]
        );
    }
}

/// Log a program pipeline creation.
pub fn pipeline_created(
    hash: u64,
    topology: wgpu::PrimitiveTopology,
    depth_compare: wgpu::CompareFunction,
    depth_write: bool,
    sample_count: u32,
    color_format: wgpu::TextureFormat,
) {
    if enabled() {
        trace_line!(
            "program pipeline: hash={hash:016x} topology={topology:?} \
             depth_compare={depth_compare:?} depth_write={depth_write} \
             samples={sample_count} format={color_format:?}"
        );
    }
}

/// Log a program uniform-buffer creation.
pub fn program_buffer(hash: u64) {
    if enabled() {
        trace_line!(
            "program buffer: hash={hash:016x} size={} bytes",
            crate::program::PROGRAM_SIZE
        );
    }
}

/// Log a group-2 bind. `program` is the program hash, or `None` for the zeroed
/// default group an ordinary scene draw binds.
pub fn group2(
    frame: u64,
    program: Option<u64>,
    frame_slot: u32,
    uniform_slot: u32,
    dynamic_offset: u32,
) {
    if enabled() {
        match program {
            Some(hash) => trace_line!(
                "frame {frame}: group2 program={hash:016x} frame_slot={frame_slot} \
                 uniform_slot={uniform_slot} offset={dynamic_offset}"
            ),
            None => trace_line!(
                "frame {frame}: group2 default frame_slot={frame_slot} \
                 uniform_slot={uniform_slot} offset={dynamic_offset}"
            ),
        }
    }
}

/// The tone of a readback frame, as the black-frame probe sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Every sampled pixel is near-black; the mean channel values are reported.
    Black {
        /// Mean red of the sampled pixels.
        r: u8,
        /// Mean green.
        g: u8,
        /// Mean blue.
        b: u8,
    },
    /// At least one sampled pixel is lit.
    Lit,
}

/// Classify an RGBA8 frame by sampling a grid of pixels (at most 5 x 5).
///
/// Returns `None` for an empty buffer or a buffer too short for its stated size.
/// A frame is black when every sampled pixel's brightest channel is below
/// [`BLACK_LEVEL`]. The sample excludes a one-pixel border so a composited
/// overlay in a corner cannot hide a black scene.
pub fn classify_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Tone> {
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if width == 0 || height == 0 || rgba.len() < expected {
        return None;
    }
    // Five evenly spaced columns/rows inside the frame, without duplicates.
    let axis = |extent: u32| {
        let lo = if extent > 2 { 1 } else { 0 };
        let hi = if extent > 2 { extent - 2 } else { extent - 1 };
        if hi <= lo {
            return vec![lo];
        }
        let steps = 5u32.min(hi - lo + 1);
        (0..steps)
            .map(|i| lo + ((hi - lo) * i) / (steps - 1))
            .collect::<Vec<_>>()
    };
    let xs = axis(width);
    let ys = axis(height);

    let mut samples = 0u32;
    let mut sum = [0u64; 3];
    let mut brightest = 0u8;
    for &y in &ys {
        for &x in &xs {
            let index = ((y as usize * width as usize) + x as usize) * 4;
            let pixel = &rgba[index..index + 3];
            let max = pixel.iter().copied().max().unwrap_or(0);
            brightest = brightest.max(max);
            sum[0] += u64::from(pixel[0]);
            sum[1] += u64::from(pixel[1]);
            sum[2] += u64::from(pixel[2]);
            samples += 1;
        }
    }
    if samples == 0 {
        return None;
    }
    if brightest < BLACK_LEVEL {
        Some(Tone::Black {
            r: (sum[0] / u64::from(samples)) as u8,
            g: (sum[1] / u64::from(samples)) as u8,
            b: (sum[2] / u64::from(samples)) as u8,
        })
    } else {
        Some(Tone::Lit)
    }
}

/// The brightest channel below which a sampled pixel counts as black.
pub const BLACK_LEVEL: u8 = 8;

/// Probe a readback frame and log black/recovery transitions (trace mode only).
///
/// Returns the classification, so callers and tests can use it too.
pub fn inspect_readback(width: u32, height: u32, rgba: &[u8]) -> Option<Tone> {
    // Zero-cost when disabled: the classification itself allocates a small sample
    // grid, so it must not run on the normal frame path.
    if !enabled() {
        return None;
    }
    let tone = classify_rgba(width, height, rgba)?;
    let frame = FRAME.load(Ordering::Relaxed);
    match tone {
        Tone::Black { r, g, b } => {
            LAST_BLACK.store(1, Ordering::Relaxed);
            trace_line!("frame {frame}: black (r={r},g={g},b={b}) size={width}x{height}");
        }
        Tone::Lit => {
            if LAST_BLACK.swap(0, Ordering::Relaxed) == 1 {
                trace_line!("frame {frame}: recovered (no longer black)");
            }
        }
    }
    Some(tone)
}

/// How many wgpu errors and device losses trace mode has logged.
pub fn error_count() -> u64 {
    ERRORS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pixel: [u8; 4], width: u32, height: u32) -> Vec<u8> {
        pixel.repeat((width * height) as usize)
    }

    #[test]
    fn an_all_black_frame_is_black_with_its_mean_channels() {
        let rgba = frame([0, 0, 0, 255], 8, 8);
        assert_eq!(
            classify_rgba(8, 8, &rgba),
            Some(Tone::Black { r: 0, g: 0, b: 0 })
        );
    }

    #[test]
    fn the_clear_colour_is_not_black() {
        // The renderer clears to linear (0.1, 0.1, 0.12), which is ~89 in sRGB.
        let rgba = frame([89, 89, 95, 255], 64, 48);
        assert_eq!(classify_rgba(64, 48, &rgba), Some(Tone::Lit));
    }

    #[test]
    fn one_lit_pixel_keeps_the_frame_lit() {
        let mut rgba = frame([0, 0, 0, 255], 8, 8);
        // The sample grid includes the interior pixels, not the border.
        let index = ((4 * 8) + 4) * 4;
        rgba[index..index + 3].copy_from_slice(&[200, 10, 10]);
        assert_eq!(classify_rgba(8, 8, &rgba), Some(Tone::Lit));
    }

    #[test]
    fn a_short_buffer_is_not_classified() {
        assert_eq!(classify_rgba(8, 8, &[0; 4]), None);
        assert_eq!(classify_rgba(0, 8, &[]), None);
    }

    #[test]
    fn trace_env_values_parse() {
        use std::ffi::OsStr;
        assert!(truthy(OsStr::new("1")));
        assert!(truthy(OsStr::new("yes")));
        assert!(!truthy(OsStr::new("0")));
        assert!(!truthy(OsStr::new("false")));
        assert!(!truthy(OsStr::new("OFF")));
        assert!(!truthy(OsStr::new("")));
    }

    #[test]
    fn the_trace_flag_is_recognised() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(args_enable(args(&["--trace"])));
        assert!(args_enable(args(&["--check-init", "--trace"])));
        assert!(!args_enable(args(&["--check-init"])));
        assert!(!args_enable(args(&["trace"])));
    }
}
