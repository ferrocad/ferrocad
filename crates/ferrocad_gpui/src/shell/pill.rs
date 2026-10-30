//! The two-stage transform pill's presentation state.
//!
//! The pill has two stages (see `docs/project/tasks/transform-session.md` §3.2): a
//! **compact** readout that follows a drag, and an **expanded** row of `ΔX`/`ΔY`/`ΔZ`
//! fields once the user escalates it with `Tab`, a digit or a click. This module is the
//! state machine between the two. It holds only *which stage is showing* and *which
//! field the keyboard owns* — never the Δ values themselves, which belong to the
//! [`TransformSession`](ferrocad_stage::TransformSession) and are read through it.
//! Keeping the split means the pill cannot become a second source of truth for a
//! pending transform, which is the drift the session exists to remove (§0.2).
//!
//! It is deliberately free of gpui types, [`Shell`](super::Shell) and the session. A
//! window is not needed to decide that `Tab` expands a compact pill or that the third
//! `Tab` wraps to X, so these transitions are unit-tested here and the next slice's
//! renderer can stay a pure function of this state. §1.2's invariants describe what the
//! *session* does; this state never commits, rolls back or moves a datum.
//!
//! The focus is an axis index `0..=2` (X/Y/Z) rather than a field enum because it is
//! handed straight to
//! [`TransformSession::translate_axis`](ferrocad_stage::TransformSession::translate_axis),
//! which takes the same index. The constructors and [`PillState::set_focus`] clamp it, so
//! the range is an invariant every later caller can rely on instead of re-checking.
//!
//! [`Shell`](super::Shell) does not use any of this yet: U2's renderer is the first
//! consumer, and until it lands the tests below are the only callers.

// The next slice is the first non-test consumer; until then a non-test build would warn
// on every item here, including the constructors the renderer will call.
#![cfg_attr(not(test), allow(dead_code))]

/// Which stage of the pill is showing, and which Δ field has the keyboard.
///
/// `focus` is only *visible* while expanded, but it is kept across a collapse: the pill
/// reopens on the field the user left, which is what makes [`tab`](Self::tab) a safe
/// escalation key rather than a way to lose their place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PillState {
    /// `true` once the pill has unfolded into the three Δ fields.
    expanded: bool,
    /// The focused Δ field, `0..=2` (X/Y/Z). Every writer clamps it; see the module docs
    /// for why it is an index rather than an enum.
    focus: usize,
}

/// How many Δ fields the pill can show: X, Y and Z.
const AXES: usize = 3;

impl PillState {
    /// A compact pill, the stage a drag starts in.
    ///
    /// Focus starts on X because nothing is focused yet; the first [`expand`](Self::expand)
    /// therefore lands on `ΔX`, the axis a first drag is most likely to have moved.
    pub(super) fn compact() -> Self {
        Self {
            expanded: false,
            focus: 0,
        }
    }

    /// An expanded pill focused on `axis`, clamped to `0..=2`.
    ///
    /// This is the entry point for the digit shortcut (§3.2): pressing a digit opens the
    /// pill *and* puts the cursor in the field the user chose, in one transition, rather
    /// than opening at X and then jumping.
    pub(super) fn expanded_at(axis: usize) -> Self {
        Self {
            expanded: true,
            focus: axis.min(AXES - 1),
        }
    }

    /// Unfold the compact pill; returns whether it was compact.
    ///
    /// The focus is left alone, so re-expanding after a collapse resumes at the same
    /// field. Expanding an already-expanded pill is a no-op, which keeps a click on an
    /// open pill from resetting the cursor.
    pub(super) fn expand(&mut self) -> bool {
        if self.expanded {
            return false;
        }
        self.expanded = true;
        true
    }

    /// Fold the pill back to the compact readout; returns whether it was expanded.
    ///
    /// The focus is retained even though it is not drawn: collapse is a change of stage,
    /// not a change of subject, so the next [`expand`](Self::expand) has to land where
    /// the user was — §3.2's "`Tab` must not lose the user's place".
    pub(super) fn collapse(&mut self) -> bool {
        if !self.expanded {
            return false;
        }
        self.expanded = false;
        true
    }

    /// Move the keyboard to the next (`backward == false`) or previous Δ field.
    ///
    /// `Tab` is the **escalation key**, not a field-traversal key: on a compact pill it
    /// unfolds and leaves the focus untouched, whatever the direction. `Shift+Tab` while
    /// compact therefore also expands. The shell's escalation path already treats both
    /// directions as one key ([`escalation_key`](super::escalation_key)), and a user's
    /// `Shift+Tab` reflex must not silently do nothing while there is no field to step
    /// back from. Once expanded, the fields wrap in both directions, because no other
    /// widget sits behind them for focus to escape to.
    pub(super) fn tab(&mut self, backward: bool) -> bool {
        if !self.expanded {
            return self.expand();
        }
        self.focus = if backward {
            (self.focus + AXES - 1) % AXES
        } else {
            (self.focus + 1) % AXES
        };
        true
    }

    /// The focused Δ field, `0..=2` (X/Y/Z).
    pub(super) fn focus(&self) -> usize {
        self.focus
    }

    /// Whether the three Δ fields are showing.
    pub(super) fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Focus `axis`, clamped to `0..=2`; returns whether the focus moved.
    ///
    /// This does **not** expand: escalation belongs to [`expand`](Self::expand),
    /// [`expanded_at`](Self::expanded_at) and [`tab`](Self::tab), so a click on a field
    /// of an open pill moves the cursor without also deciding that the pill should open.
    /// A caller that means "show the pill and focus this axis" uses
    /// [`expanded_at`](Self::expanded_at) instead, which is one transition rather than
    /// two.
    pub(super) fn set_focus(&mut self, axis: usize) -> bool {
        let axis = axis.min(AXES - 1);
        if axis == self.focus {
            return false;
        }
        self.focus = axis;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compact_pill_starts_on_x() {
        let pill = PillState::compact();
        assert!(!pill.is_expanded());
        assert_eq!(pill.focus(), 0);
    }

    #[test]
    fn tab_expands_a_compact_pill_without_moving_the_focus() {
        let mut pill = PillState::compact();
        assert!(pill.tab(false));
        assert!(pill.is_expanded());
        assert_eq!(pill.focus(), 0, "escalation must not choose a field");
    }

    #[test]
    fn shift_tab_also_expands_a_compact_pill() {
        let mut pill = PillState::compact();
        assert!(pill.tab(true));
        assert!(pill.is_expanded());
        assert_eq!(pill.focus(), 0, "there is no field to step back from yet");
    }

    #[test]
    fn tab_cycles_the_fields_forward_with_wraparound() {
        let mut pill = PillState::expanded_at(0);
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), 1);
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), 2);
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), 0, "after Z the next field is X");
    }

    #[test]
    fn shift_tab_cycles_the_fields_backward_with_wraparound() {
        let mut pill = PillState::expanded_at(0);
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), 2, "before X the previous field is Z");
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), 1);
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), 0);
    }

    #[test]
    fn expand_keeps_the_focus() {
        let mut pill = PillState::expanded_at(2);
        assert!(pill.collapse());
        assert!(pill.expand());
        assert!(pill.is_expanded());
        assert_eq!(pill.focus(), 2, "the pill reopens on the field the user left");
    }

    #[test]
    fn collapse_and_expand_round_trip() {
        let mut pill = PillState::expanded_at(1);
        let before = pill;
        assert!(pill.collapse());
        assert!(!pill.is_expanded());
        assert!(pill.expand());
        assert_eq!(pill, before);
    }

    #[test]
    fn out_of_range_focus_clamps_to_the_three_axes() {
        assert_eq!(PillState::expanded_at(9).focus(), 2);
        assert_eq!(PillState::expanded_at(usize::MAX).focus(), 2);

        let mut pill = PillState::expanded_at(0);
        assert!(pill.set_focus(7));
        assert_eq!(pill.focus(), 2);
        assert!(!pill.set_focus(3), "already clamped to the same field");
        assert!(!pill.set_focus(usize::MAX));
    }

    #[test]
    fn transitions_report_a_change_only_when_they_change_something() {
        let mut pill = PillState::compact();
        assert!(!pill.collapse(), "already compact");
        assert!(!pill.set_focus(0), "already focused on X");
        assert!(pill.expand());
        assert!(!pill.expand(), "already expanded");
        assert!(!pill.set_focus(0), "already focused on X");
        assert!(pill.set_focus(1));
        assert!(!pill.set_focus(1), "already focused on Y");
        assert!(pill.collapse());
        assert!(!pill.collapse(), "already compact");
    }
}
