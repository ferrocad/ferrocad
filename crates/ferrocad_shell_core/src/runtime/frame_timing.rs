//! A [`FramePipeline`] decorator that times the window's own passes.
//!
//! The viewport's `FERROCAD_TIMING` line reports what `ferrocad_viewport` spends inside
//! `render`. A frame is more than that: the window gathers the element tree, lays it out,
//! paints the whole application UI — the viewport's `gpu_canvas` callback, and so the
//! dma-buf hand-off, run inside that paint — and closes the frame. All of it is below the
//! viewport, so none of it appeared in the numbers that led here.
//!
//! [`FramePipeline`] is the seam that sees those passes. This decorator forwards **every**
//! pass to [`StandardImmediatePipeline`] — the pipeline the application draws with when
//! nothing is installed — so the frames are exactly the ones it already draws and only
//! the accounting is new. Install it with
//! [`Application::with_frame_pipeline`](gpui::Application::with_frame_pipeline).
//!
//! The present is deliberately *below* the seam (the GPU submit and the swap belong to
//! the platform), so a second whose passes are cheap and whose cadence is not names the
//! compositor, not the UI.

use std::time::{Duration, Instant};

use gpui::{
    App, FocusId, FramePipeline, PreparedRoots, StandardImmediatePipeline, Window, WindowMetrics,
};

/// Whether the pass accounting is on.
///
/// The same switch as the viewport's line, so one environment variable turns on the
/// whole diagnosis.
pub fn enabled() -> bool {
    std::env::var_os("FERROCAD_TIMING").is_some()
}

/// The pipeline an application installs: the standard one, with a timer on each pass.
pub fn pipeline() -> TimingPipeline<StandardImmediatePipeline> {
    TimingPipeline::new(StandardImmediatePipeline)
}

/// One second of a window's passes, averaged over the frames it drew.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Frames drawn in the second.
    pub frames: u32,
    /// Times the pipeline was asked whether to draw, drawn or deferred.
    pub asks: u32,
    /// Asks it passed over.
    pub deferred: u32,
    /// Mean interval between drawn frames.
    pub interval_ms: f64,
    pub evaluate_ms: f64,
    pub layout_ms: f64,
    pub paint_ms: f64,
    pub finish_ms: f64,
    pub complete_ms: f64,
    pub end_ms: f64,
    /// The slowest single `paint_roots` in the second.
    pub worst_paint_ms: f64,
}

impl Snapshot {
    /// The passes one frame is made of.
    ///
    /// The difference between this and [`interval_ms`](Self::interval_ms) is the time no
    /// pass owns: the platform's present, the swap, and the gaps between passes.
    pub fn passes_ms(&self) -> f64 {
        self.evaluate_ms
            + self.layout_ms
            + self.paint_ms
            + self.finish_ms
            + self.complete_ms
            + self.end_ms
    }

    /// The line printed once a second.
    pub fn line(&self) -> String {
        format!(
            "ferrocad: window fps {:.1}, interval {:.1} ms, passes {:.1} ms \
             (evaluate {:.1}, layout {:.1}, paint {:.1} worst {:.1}, finish {:.1}, end {:.1}), \
             deferred {}/{} asks",
            if self.interval_ms > 0.0 {
                1000.0 / self.interval_ms
            } else {
                0.0
            },
            self.interval_ms,
            self.passes_ms(),
            self.evaluate_ms,
            self.layout_ms,
            self.paint_ms,
            self.worst_paint_ms,
            self.finish_ms,
            self.end_ms,
            self.deferred,
            self.asks
        )
    }
}

/// A [`FramePipeline`] that times the passes a frame is made of.
///
/// Every pass is forwarded to `inner`; nothing about the frame changes. The counters
/// reset once a second and print [`Snapshot::line`].
pub struct TimingPipeline<P = StandardImmediatePipeline> {
    inner: P,
    started: Instant,
    last_frame: Option<Instant>,
    frames: u32,
    asks: u32,
    deferred: u32,
    intervals: Duration,
    evaluate: Duration,
    layout: Duration,
    paint: Duration,
    finish: Duration,
    complete: Duration,
    end: Duration,
    worst_paint: Duration,
}

impl TimingPipeline<StandardImmediatePipeline> {
    /// The timing decorator over the standard pipeline.
    pub fn installed() -> Self {
        pipeline()
    }
}

impl<P: FramePipeline> TimingPipeline<P> {
    /// Wrap `inner`.
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            started: Instant::now(),
            last_frame: None,
            frames: 0,
            asks: 0,
            deferred: 0,
            intervals: Duration::ZERO,
            evaluate: Duration::ZERO,
            layout: Duration::ZERO,
            paint: Duration::ZERO,
            finish: Duration::ZERO,
            complete: Duration::ZERO,
            end: Duration::ZERO,
            worst_paint: Duration::ZERO,
        }
    }

    /// The pipeline this one wraps.
    pub fn inner(&self) -> &P {
        &self.inner
    }

    /// What the current second has measured, as per-frame averages.
    fn snapshot(&self, _now: Instant) -> Snapshot {
        let frames = self.frames.max(1);
        let per_frame = |total: Duration| total.as_secs_f64() * 1000.0 / f64::from(frames);
        Snapshot {
            frames: self.frames,
            asks: self.asks,
            deferred: self.deferred,
            interval_ms: if self.frames == 0 {
                0.0
            } else {
                self.intervals.as_secs_f64() * 1000.0 / f64::from(self.frames)
            },
            evaluate_ms: per_frame(self.evaluate),
            layout_ms: per_frame(self.layout),
            paint_ms: per_frame(self.paint),
            finish_ms: per_frame(self.finish),
            complete_ms: per_frame(self.complete),
            end_ms: per_frame(self.end),
            worst_paint_ms: self.worst_paint.as_secs_f64() * 1000.0,
        }
    }

    /// Print the second that just ended, and start the next one.
    ///
    /// Called from the two passes the platform always runs, so a window that is asked
    /// for frames but defers all of them still reports.
    fn report(&mut self, now: Instant) {
        if now - self.started < Duration::from_secs(1) {
            return;
        }
        let snapshot = self.snapshot(now);
        if snapshot.frames > 0 || snapshot.asks > 0 {
            eprintln!("{}", snapshot.line());
        }
        self.started = now;
        self.frames = 0;
        self.asks = 0;
        self.deferred = 0;
        self.intervals = Duration::ZERO;
        self.evaluate = Duration::ZERO;
        self.layout = Duration::ZERO;
        self.paint = Duration::ZERO;
        self.finish = Duration::ZERO;
        self.complete = Duration::ZERO;
        self.end = Duration::ZERO;
        self.worst_paint = Duration::ZERO;
    }
}

impl<P: FramePipeline> FramePipeline for TimingPipeline<P> {
    fn should_render(&mut self, is_dirty: bool, metrics: &WindowMetrics) -> bool {
        let now = Instant::now();
        self.report(now);
        self.asks += 1;
        let draw = self.inner.should_render(is_dirty, metrics);
        if !draw {
            self.deferred += 1;
        }
        draw
    }

    fn begin_frame(&mut self, window: &mut Window<'_>, cx: &mut App) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.intervals += now - last;
        }
        self.last_frame = Some(now);
        self.frames += 1;
        let started = Instant::now();
        self.inner.begin_frame(window, cx);
        self.end += started.elapsed();
    }

    fn evaluate_roots(&mut self, window: &mut Window<'_>, cx: &mut App) -> PreparedRoots {
        let started = Instant::now();
        let roots = self.inner.evaluate_roots(window, cx);
        self.evaluate += started.elapsed();
        roots
    }

    fn layout_roots(&mut self, window: &mut Window<'_>, roots: &mut PreparedRoots, cx: &mut App) {
        let started = Instant::now();
        self.inner.layout_roots(window, roots, cx);
        self.layout += started.elapsed();
    }

    fn paint_roots(&mut self, window: &mut Window<'_>, roots: PreparedRoots, cx: &mut App) {
        let started = Instant::now();
        self.inner.paint_roots(window, roots, cx);
        let painted = started.elapsed();
        self.paint += painted;
        self.worst_paint = self.worst_paint.max(painted);
    }

    fn finish_frame(&mut self, window: &mut Window<'_>, cx: &mut App) {
        let started = Instant::now();
        self.inner.finish_frame(window, cx);
        self.finish += started.elapsed();
    }

    fn complete_frame(&mut self, window: &mut Window<'_>, cx: &mut App) -> Option<FocusId> {
        let started = Instant::now();
        let focus = self.inner.complete_frame(window, cx);
        self.complete += started.elapsed();
        focus
    }

    fn end_frame(
        &mut self,
        window: &mut Window<'_>,
        cx: &mut App,
        focus_before_listeners: Option<FocusId>,
    ) {
        let started = Instant::now();
        self.inner.end_frame(window, cx, focus_before_listeners);
        self.end += started.elapsed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_sums_the_passes_it_measures() {
        let mut pipeline = TimingPipeline::installed();
        let now = Instant::now();
        pipeline.frames = 2;
        pipeline.asks = 5;
        pipeline.deferred = 3;
        pipeline.intervals = Duration::from_millis(200);
        pipeline.evaluate = Duration::from_millis(2);
        pipeline.layout = Duration::from_millis(4);
        pipeline.paint = Duration::from_millis(10);
        pipeline.finish = Duration::from_millis(1);
        pipeline.complete = Duration::from_millis(1);
        pipeline.end = Duration::from_millis(1);
        pipeline.worst_paint = Duration::from_millis(9);

        let snapshot = pipeline.snapshot(now);
        // Two frames: every total is halved into a per-frame average.
        assert_eq!(snapshot.frames, 2);
        assert_eq!(snapshot.interval_ms, 100.0);
        assert_eq!(snapshot.evaluate_ms, 1.0);
        assert_eq!(snapshot.paint_ms, 5.0);
        assert_eq!(snapshot.worst_paint_ms, 9.0, "the worst is not averaged");
        assert_eq!(snapshot.passes_ms(), 9.5);
        assert_eq!((snapshot.deferred, snapshot.asks), (3, 5));

        // The line names every pass, and the cadence it is compared against.
        let line = snapshot.line();
        assert!(
            line.starts_with("ferrocad: window fps 10.0, interval 100.0 ms"),
            "{line}"
        );
        assert!(line.contains("passes 9.5 ms"), "{line}");
        assert!(line.contains("deferred 3/5 asks"), "{line}");
    }

    #[test]
    fn a_second_with_no_drawn_frames_reports_its_deferrals() {
        let mut pipeline = TimingPipeline::installed();
        let now = Instant::now();
        pipeline.asks = 60;
        pipeline.deferred = 60;

        let snapshot = pipeline.snapshot(now);
        assert_eq!(snapshot.frames, 0);
        assert_eq!(snapshot.interval_ms, 0.0);
        assert_eq!(
            snapshot.passes_ms(),
            0.0,
            "no frame drew, so nothing is divided by one"
        );
        assert!(snapshot.line().contains("deferred 60/60 asks"));
    }
}
