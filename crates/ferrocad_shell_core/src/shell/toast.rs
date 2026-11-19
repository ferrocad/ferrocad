//! The transient status toast: a notification feed in the bottom-right corner.
//!
//! The shell used to keep a permanent status strip under the canvas. In a full-bleed
//! viewport that is a row of chrome paid for by every frame, so action feedback is now
//! transient: an action's status appears in a pill for a couple of seconds and then
//! fades. The strip's other jobs moved with their subjects — hover text stays in
//! [`Shell::hover_line`](crate::shell::Shell), the version and backend live in the
//! About card, and Dev Tools is in the app menu.
//!
//! The classification is deliberately a vocabulary, not a structured result: every
//! status line in the shell is a sentence, and most predate this module. Words such as
//! `failed`, `refused` and `too large` mark the sticky alerts; the action verbs mark
//! the confirmations. A status with no recognisable word is plain information and
//! still fades — the failure modes this note cares about are the ones the user can
//! act on.

use std::time::Duration;

use web_time::Instant;

/// How long the pill takes to slide the last 6 px into place.
pub(crate) const TOAST_ENTER: Duration = Duration::from_millis(160);
/// Repaint steps during the entrance: 160 ms in 40 ms steps.
pub(crate) const TOAST_ENTER_STEPS: u32 = 4;
/// How long a confirmation stays fully visible (including the entrance).
///
/// The feed is read in glances; six seconds leaves time to read the action and reach the
/// Undo chip without holding the screen, even when the user only looks at the corner
/// after the message has already arrived.
pub(crate) const TOAST_LIFE: Duration = Duration::from_millis(6_000);
/// How long the confirmation takes to fade after [`TOAST_LIFE`].
pub(crate) const TOAST_FADE: Duration = Duration::from_millis(300);
/// Repaint steps during the fade: 300 ms at 50 ms a step.
pub(crate) const TOAST_FADE_STEPS: u32 = 6;

/// What a status line means for the toast's persistence and ink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusKind {
    /// A neutral note (`tool: box`, `select: face`).
    #[default]
    Info,
    /// A completed action (`moved 1 object`, `created Box_01`).
    Success,
    /// Something the user can act on but that is not a failure (`select at least 2 profiles`).
    Warning,
    /// A refusal or solver failure (`fillet radius too large`, `over-constrained sketch`).
    Error,
}

impl StatusKind {
    /// A warning or an error persists until the user dismisses it, undoes it, or starts
    /// the next valid action.
    pub(crate) fn sticky(self) -> bool {
        matches!(self, Self::Warning | Self::Error)
    }
}

/// One showing of the status line.
#[derive(Clone, Debug)]
pub struct Toast {
    /// The status revision that produced it, and the key its fade task removes.
    pub id: u64,
    pub text: String,
    pub kind: StatusKind,
    /// Whether this action advanced the document's undo stack.
    pub edits_history: bool,
    /// The undo depth and top label **after** the action. The pill offers Undo only
    /// while they still name the stack top, so the chip can never undo someone else's
    /// action.
    pub undo_depth: usize,
    pub undo_label: String,
    /// When the pill first appeared, for the fade arithmetic.
    pub shown_at: Instant,
}

/// Drop cards until at most `max` remain.
///
/// A new message displaces the oldest **information** first: `tool: box` from three
/// actions ago is worth less than the confirmation whose Undo chip is still live. A
/// warning or an error is a solver alert the user may still need to read, so it keeps its
/// place until it is dismissed or a success clears it; only when every card is an alert
/// does the oldest one give way, which stops an unbroken run of failures from growing the
/// feed up the viewport.
pub(crate) fn trim_to_cap(toasts: &mut Vec<Toast>, max: usize) {
    while toasts.len() > max {
        let index = toasts
            .iter()
            .position(|toast| !toast.kind.sticky())
            .unwrap_or(0);
        toasts.remove(index);
    }
}

impl Toast {
    pub(crate) fn new(id: u64, text: String, kind: StatusKind, shown_at: Instant) -> Self {
        Self {
            id,
            text,
            kind,
            edits_history: false,
            undo_depth: 0,
            undo_label: String::new(),
            shown_at,
        }
    }

    /// Record that this toast's action created an undo entry at `depth`, named `label`.
    pub(crate) fn mark_history(&mut self, depth: usize, label: String) {
        self.edits_history = true;
        self.undo_depth = depth;
        self.undo_label = label;
    }

    /// Whether the chip belongs on this pill **now**: the action was undoable and its
    /// entry is still the top of the stack.
    pub(crate) fn offers_undo(&self, depth: usize, label: &str) -> bool {
        self.edits_history && self.undo_depth == depth && self.undo_label == label
    }

    /// The pill's opacity at `now`: an eased entrance, crisp for [`TOAST_LIFE`], then a
    /// linear fade.
    pub(crate) fn opacity(&self, now: Instant) -> f32 {
        let age = now.duration_since(self.shown_at);
        if age < TOAST_ENTER {
            return enter_progress(age);
        }
        if age <= TOAST_LIFE {
            1.0
        } else {
            let faded = age.saturating_sub(TOAST_LIFE).as_secs_f32();
            1.0 - (faded / TOAST_FADE.as_secs_f32()).clamp(0.0, 1.0)
        }
    }

    /// How far below its resting place the pill sits at `now`, in logical pixels.
    ///
    /// The entrance slides the last 6 px up, so the toast reads as a layer arriving over
    /// the docks rather than a row that popped into them.
    pub(crate) fn enter_offset(&self, now: Instant) -> f32 {
        let age = now.duration_since(self.shown_at);
        if age < TOAST_ENTER {
            6.0 * (1.0 - enter_progress(age))
        } else {
            0.0
        }
    }
}

/// Eased entrance progress in `0..=1` (ease-out cubic).
fn enter_progress(age: Duration) -> f32 {
    let t = (age.as_secs_f32() / TOAST_ENTER.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Capitalize the first letter of a status sentence for the HUD.
///
/// The status text is also the console transcript and the model's `status`, where the
/// established spelling is lower case; only the pill's copy is a headline.
pub(crate) fn headline(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {
            first.to_ascii_uppercase().to_string() + chars.as_str()
        }
        _ => text.to_string(),
    }
}

/// Classify a status sentence.
///
/// Order matters: a failure word wins over an action verb (`undo: nothing to undo` is an
/// error, not a confirmation), then an action verb marks success, then the acting-on
/// vocabulary marks a warning. Everything else is information.
pub(crate) fn classify(text: &str) -> StatusKind {
    let lower = text.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return StatusKind::Info;
    }
    const ERROR: &[&str] = &[
        "error",
        "failed",
        "failure",
        "refused",
        "cannot",
        "can't",
        "invalid",
        "too large",
        "too small",
        "over-constrained",
        "over constrained",
        "no active document",
        "unavailable",
        "not supported",
        "unsupported",
        "nothing to undo",
        "nothing to redo",
    ];
    if ERROR.iter().any(|word| lower.contains(word)) {
        return StatusKind::Error;
    }
    const WARNING: &[&str] = &[
        "requires",
        "needs",
        "need to",
        "select at least",
        "nothing to frame",
        "nothing to apply",
        "already",
        "missing",
        "cannot be empty",
        "must be",
    ];
    if WARNING.iter().any(|word| lower.contains(word)) {
        return StatusKind::Warning;
    }
    const SUCCESS: &[&str] = &[
        "moved",
        "created",
        "deleted",
        "removed",
        "added",
        "saved",
        "opened",
        "exported",
        "hid",
        "showed",
        "show ",
        "rotated",
        "scaled",
        "fillet",
        "chamfer",
        "fit",
        "undo",
        "redo",
        "ok",
        "active document",
        "workplane",
        "datum plane",
        "snapped",
        "applied",
        "loaded",
        "toggled",
    ];
    if SUCCESS.iter().any(|word| lower.starts_with(word)) {
        return StatusKind::Success;
    }
    StatusKind::Info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_words_are_confirmations() {
        for text in [
            "moved 1 object",
            "created Box_01",
            "saved /tmp/a.FCStd",
            "datum plane: DatumPlane",
            "snapped to Face1",
        ] {
            assert_eq!(classify(text), StatusKind::Success, "{text}");
        }
    }

    #[test]
    fn solver_failures_are_errors() {
        for text in [
            "fillet radius too large",
            "over-constrained sketch",
            "Part_Fillet: invalid radius",
            "undo: nothing to undo",
        ] {
            assert_eq!(classify(text), StatusKind::Error, "{text}");
            assert!(classify(text).sticky(), "{text} persists");
        }
    }

    #[test]
    fn unmet_requirements_warn() {
        for text in ["select at least 2 profiles", "fit: nothing to frame"] {
            assert_eq!(classify(text), StatusKind::Warning, "{text}");
            assert!(classify(text).sticky(), "{text} persists");
        }
    }

    #[test]
    fn a_neutral_note_is_information() {
        assert_eq!(classify("tool: box"), StatusKind::Info);
        assert!(!StatusKind::Info.sticky());
        assert!(!StatusKind::Success.sticky());
    }

    #[test]
    fn a_full_feed_drops_information_before_an_alert() {
        let now = Instant::now();
        let mut toasts = vec![
            Toast::new(1, "fillet radius too large".into(), StatusKind::Error, now),
            Toast::new(2, "tool: box".into(), StatusKind::Info, now),
            Toast::new(3, "tool: line".into(), StatusKind::Info, now),
        ];
        toasts.push(Toast::new(4, "tool: circle".into(), StatusKind::Info, now));
        trim_to_cap(&mut toasts, 3);
        assert_eq!(
            toasts.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            vec![1, 3, 4],
            "the oldest note gave way, the alert kept its place"
        );
    }

    #[test]
    fn a_feed_of_alerts_still_respects_the_cap() {
        let now = Instant::now();
        let mut toasts = vec![
            Toast::new(1, "fillet radius too large".into(), StatusKind::Error, now),
            Toast::new(2, "select at least 2 profiles".into(), StatusKind::Warning, now),
            Toast::new(3, "over-constrained sketch".into(), StatusKind::Error, now),
            Toast::new(4, "sketch needs a lock".into(), StatusKind::Warning, now),
        ];
        trim_to_cap(&mut toasts, 3);
        assert_eq!(
            toasts.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            vec![2, 3, 4],
            "with no information left, the oldest alert is the one that goes"
        );
    }

    #[test]
    fn a_confirmation_stays_up_long_enough_to_reach_its_chip() {
        assert_eq!(TOAST_LIFE, Duration::from_secs(6));
    }

    #[test]
    fn the_fade_is_crisp_then_linear() {
        let now = Instant::now();
        let mut toast = Toast::new(1, "moved 1 object".into(), StatusKind::Success, now);
        toast.mark_history(2, "Move".into());
        assert_eq!(toast.opacity(now), 0.0, "the entrance starts transparent");
        assert_eq!(toast.opacity(now + TOAST_ENTER), 1.0);
        assert_eq!(toast.opacity(now + TOAST_LIFE), 1.0);
        let half = toast.opacity(now + TOAST_LIFE + TOAST_FADE / 2);
        assert!((half - 0.5).abs() < 0.05, "{half}");
        assert_eq!(toast.opacity(now + TOAST_LIFE + TOAST_FADE), 0.0);
    }

    #[test]
    fn undo_only_belongs_to_the_current_top() {
        let now = Instant::now();
        let mut toast = Toast::new(1, "moved Box".into(), StatusKind::Success, now);
        toast.mark_history(2, "Move".into());
        assert!(toast.offers_undo(2, "Move"));
        assert!(!toast.offers_undo(3, "Fillet"), "a later action owns the chip");
        assert!(!toast.offers_undo(2, "Fillet"), "a different top owns the chip");
        assert!(!toast.offers_undo(1, "Move"), "the entry was undone");
    }

    #[test]
    fn an_action_that_did_not_touch_history_never_offers_undo() {
        let toast = Toast::new(1, "grid: on".into(), StatusKind::Success, Instant::now());
        assert!(!toast.offers_undo(4, "Move"));
    }

    #[test]
    fn the_entrance_slides_six_pixels_up() {
        let now = Instant::now();
        let mut toast = Toast::new(1, "moved 1 object".into(), StatusKind::Success, now);
        toast.mark_history(2, "Move".into());
        assert_eq!(toast.enter_offset(now), 6.0);
        assert_eq!(toast.enter_offset(now + TOAST_ENTER), 0.0);
    }

    #[test]
    fn only_the_pill_capitalizes_its_copy() {
        assert_eq!(headline("moved Box_01"), "Moved Box_01");
        assert_eq!(headline("App_Move: refused"), "App_Move: refused");
        assert_eq!(headline(""), "");
    }
}
