//! What the shell's own `render` costs, under `FERROCAD_TIMING`.
//!
//! The frame pipeline's `layout` pass is one number for three jobs: sync the model,
//! build the element tree, and let taffy solve and prepaint it. They want different
//! fixes, so one number cannot say which — and the shell's `render` is the part of
//! that pass this crate owns.
//!
//! GPUI renders a window's root view on every frame it draws
//! (`ViewElement::request_layout` has no dirty check unless the view was embedded
//! with `AnyView::cached`), so this is the per-frame cost even when nothing changed.
//! Splitting it here is what makes "cache the chrome" a measurement rather than a
//! guess: `prep` is the model snapshot and the syncs before any element exists, and
//! `tree` is everything the tree is built from.
//!
//! Nothing here changes a frame: two clock reads and a counter, and nothing at all
//! unless the same switch the rest of the diagnosis uses is set.

use std::sync::Mutex;
use std::time::Duration;

use web_time::Instant;

/// A second of shell renders, averaged over the frames it drew.
#[derive(Default)]
struct Second {
    started: Option<Instant>,
    frames: u32,
    /// Entry to exit, which is this view's share of the `layout` pass.
    total: Duration,
    /// Entry to the end of the model sync: the part that is not tree building.
    prep: Duration,
    worst: Duration,
}

impl Second {
    fn per_frame(&self, total: Duration) -> f64 {
        total.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1))
    }
}

/// The render's start, or `None` when the timing is off.
pub(super) fn start() -> Option<Instant> {
    std::env::var_os("FERROCAD_TIMING")
        .is_some()
        .then(Instant::now)
}

/// A mark on the same switch as `start`, so a disabled run reads no clock at all.
pub(super) fn mark(start: Option<Instant>) -> Option<Instant> {
    start.map(|_| Instant::now())
}

/// Count one render and print the second it completes.
///
/// Called at the end of `render`, so `total` is the whole cost of building the
/// window's element tree — what `layout` has left once this is subtracted is taffy
/// and prepaint.
pub(super) fn finish(start: Option<Instant>, prep: Option<Instant>) {
    let (Some(start), Some(prep)) = (start, prep) else {
        return;
    };
    let now = Instant::now();
    let total = now - start;
    let prep = prep - start;

    let Some(second) = second() else {
        return;
    };
    // The lock is held across a print and nothing else; a poisoned cell still has a
    // usable second, so the error case is the value, not a panic.
    let mut second = second.lock().unwrap_or_else(|error| error.into_inner());
    let started = *second.started.get_or_insert(now);
    second.frames += 1;
    second.total += total;
    second.prep += prep;
    second.worst = second.worst.max(total);
    if now - started < Duration::from_secs(1) {
        return;
    }

    eprintln!(
        "ferrocad: shell render avg {:.1} ms (prep {:.1}, tree {:.1}), worst {:.1} ms ({} frames)",
        second.per_frame(second.total),
        second.per_frame(second.prep),
        second.per_frame(second.total - second.prep),
        second.worst.as_secs_f64() * 1000.0,
        second.frames
    );
    *second = Second {
        started: Some(now),
        ..Second::default()
    };
}

/// The running second, or `None` when the timing is off.
///
/// The check comes first, and the lock is taken only on the `FERROCAD_TIMING` path —
/// the same shape as the viewport's frame counter, and the reason this is a function
/// of its own.
fn second() -> Option<&'static Mutex<Second>> {
    use std::sync::{Mutex, OnceLock};

    if !enabled() {
        return None;
    }
    static SECOND: OnceLock<Mutex<Second>> = OnceLock::new();
    Some(SECOND.get_or_init(|| Mutex::new(Second::default())))
}

/// Whether the pass accounting is on.
///
/// The same environment variable the viewport's line and the frame pipeline's
/// decorator read, so one switch diagnoses the whole frame.
fn enabled() -> bool {
    std::env::var_os("FERROCAD_TIMING").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_averages_the_frames_it_drew() {
        let second = Second {
            started: None,
            frames: 4,
            total: Duration::from_millis(80),
            prep: Duration::from_millis(8),
            worst: Duration::from_millis(30),
        };
        assert_eq!(second.per_frame(second.total), 20.0);
        assert_eq!(second.per_frame(second.prep), 2.0);
        // The tree is what is left once the model sync is out.
        assert_eq!(second.per_frame(second.total - second.prep), 18.0);
    }

    #[test]
    fn no_frames_divides_by_one_rather_than_zero() {
        let second = Second::default();
        assert_eq!(second.per_frame(Duration::ZERO), 0.0);
    }
}
