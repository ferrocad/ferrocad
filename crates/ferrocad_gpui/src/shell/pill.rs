//! The Move pill's keyboard state.
//!
//! The pill is one shape now (see `docs/project/tasks/transform-session.md` §3.2): a
//! single row of `ΔX`/`ΔY`/`ΔZ` fields, shown whenever Slot 1 has a Move target. This
//! module holds only *which field, if any, the keyboard owns* — never the Δ values
//! themselves, which belong to the [`TransformSession`](ferrocad_stage::TransformSession)
//! and are read through it. Keeping the split means the pill cannot become a second
//! source of truth for a pending transform, which is the drift the session exists to
//! remove (§0.2).
//!
//! **Resting is the default.** Opening the HUD, or releasing a drag, leaves every field
//! unfocused, so `1`/`2`/`3` still equip loadout slots and the viewport keeps its own
//! keys. Focus comes only from an explicit act: `Tab`, repurposing a digit, a click in a
//! field, or a click on a ruler tag. [`PillState::release`] gives it back.
//!
//! The focus is an axis index `0..=2` (X/Y/Z) rather than a field enum because it is
//! handed straight to
//! [`TransformSession::translate_axis`](ferrocad_stage::TransformSession::translate_axis),
//! which takes the same index. The constructors and [`PillState::focus_axis`] clamp it, so
//! the range is an invariant every later caller can rely on instead of re-checking.
//!
//! The state also remembers the axis of the last gesture ([`remember_axis`]): `Tab` then
//! opens on the field the user was just dragging instead of always starting at `ΔX`.
//!
//! [`Shell`](super::Shell) owns one, renders it (`shell/render.rs`) and routes the
//! escalation keys through it (`shell.rs`); it is also where a typed edit opens or
//! updates the session.

/// Which Δ field the keyboard owns, if any, and the axis the last gesture moved.
///
/// `focus == None` is the resting state: the fields render with their chrome hairline
/// and no caret, and the shell's loadout keys and the viewport keep the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PillState {
    /// The focused Δ field, `0..=2` (X/Y/Z). Every writer clamps it; see the module docs
    /// for why it is an index rather than an enum.
    focus: Option<usize>,
    /// The axis the last drag (or armed handle) moved. `Tab` opens here.
    last: usize,
    /// `true` when the fields show world coordinates (`X/Y/Z`) instead of offsets
    /// (`ΔX/ΔY/ΔZ`). The preview and the commit stay deltas either way.
    absolute: bool,
}

/// How many Δ fields the pill can show: X, Y and Z.
const AXES: usize = 3;

impl PillState {
    /// The resting state: the fields are visible but no input owns the keyboard.
    ///
    /// This is what a freshly opened HUD gets, and what it returns to after a commit,
    /// a cancel or `Enter`.
    pub(super) fn resting() -> Self {
        Self {
            focus: None,
            last: 0,
            absolute: false,
        }
    }

    /// Resting, remembering `axis` as the field `Tab` should open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn resting_at(axis: usize) -> Self {
        Self {
            focus: None,
            last: axis.min(AXES - 1),
            absolute: false,
        }
    }

    /// Focused on `axis`, clamped to `0..=2`.
    ///
    /// This is the entry point for the digit shortcut and for a click on a ruler tag:
    /// they open the field the user chose in one transition rather than focusing X and
    /// then jumping.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn focused_at(axis: usize) -> Self {
        let axis = axis.min(AXES - 1);
        Self {
            focus: Some(axis),
            last: axis,
            absolute: false,
        }
    }

    /// The focused Δ field, if the keyboard owns one.
    pub(super) fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// Whether any field owns the keyboard.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn has_focus(&self) -> bool {
        self.focus.is_some()
    }

    /// The axis `Tab` opens on while resting: the last gesture's axis.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn last_axis(&self) -> usize {
        self.last
    }

    /// Remember `axis` as the last gesture's axis, clamped to `0..=2`.
    ///
    /// This does **not** move the focus: a drag must never steal the keyboard from a
    /// field the user is typing in. It only decides where the next `Tab` lands.
    pub(super) fn remember_axis(&mut self, axis: usize) {
        self.last = axis.min(AXES - 1);
    }

    /// Whether the fields show absolute coordinates rather than offsets.
    pub(super) fn absolute(&self) -> bool {
        self.absolute
    }

    /// Flip between `ΔX/ΔY/ΔZ` and `X/Y/Z`; returns the new state.
    ///
    /// The mode outlives a session and a focus change: a user who works in world
    /// coordinates keeps that form when the HUD reopens.
    pub(super) fn toggle_absolute(&mut self) -> bool {
        self.absolute = !self.absolute;
        self.absolute
    }

    /// Focus `axis`, clamped to `0..=2`; returns whether the focus moved.
    ///
    /// A caller that means "show the pill and focus this axis" uses
    /// [`focused_at`](Self::focused_at) instead, which is one transition rather than two.
    pub(super) fn focus_axis(&mut self, axis: usize) -> bool {
        let axis = axis.min(AXES - 1);
        if self.focus == Some(axis) {
            return false;
        }
        self.focus = Some(axis);
        true
    }

    /// Give the keyboard back to the viewport; returns whether a field had it.
    ///
    /// The remembered axis survives, so the next `Tab` reopens where the user was.
    pub(super) fn release(&mut self) -> bool {
        self.focus.take().is_some()
    }

    /// `Tab` / `Shift+Tab`: open on the remembered axis, then cycle through the fields.
    ///
    /// While resting, both directions land on the last gesture's axis: the first `Tab` is
    /// an act of *entering* the form, not of stepping through it, and a user whose last
    /// drag was on Y expects `ΔY` rather than `ΔX`. Once a field is focused the fields
    /// wrap in both directions, because no other widget sits behind them for focus to
    /// escape to.
    pub(super) fn tab(&mut self, backward: bool) -> bool {
        let Some(focus) = self.focus else {
            self.focus = Some(self.last);
            return true;
        };
        self.focus = Some(if backward {
            (focus + AXES - 1) % AXES
        } else {
            (focus + 1) % AXES
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pill_rests_without_a_focused_field() {
        let pill = PillState::resting();
        assert!(!pill.has_focus());
        assert_eq!(pill.focus(), None);
        assert_eq!(pill.last_axis(), 0);
    }

    #[test]
    fn tab_opens_on_the_remembered_axis() {
        let mut pill = PillState::resting();
        pill.remember_axis(1);
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), Some(1), "the first Tab enters at the drag axis");
    }

    #[test]
    fn shift_tab_also_opens_on_the_remembered_axis() {
        let mut pill = PillState::resting();
        pill.remember_axis(2);
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), Some(2), "there is no field to step back from yet");
    }

    #[test]
    fn tab_cycles_the_fields_forward_with_wraparound() {
        let mut pill = PillState::focused_at(0);
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), Some(1));
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), Some(2));
        assert!(pill.tab(false));
        assert_eq!(pill.focus(), Some(0), "after Z the next field is X");
    }

    #[test]
    fn shift_tab_cycles_the_fields_backward_with_wraparound() {
        let mut pill = PillState::focused_at(0);
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), Some(2), "before X the previous field is Z");
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), Some(1));
        assert!(pill.tab(true));
        assert_eq!(pill.focus(), Some(0));
    }

    #[test]
    fn release_gives_the_keyboard_back_and_keeps_the_memory() {
        let mut pill = PillState::focused_at(2);
        assert!(pill.release());
        assert!(!pill.has_focus());
        assert!(!pill.release(), "already resting");
        assert_eq!(pill.last_axis(), 2);
        pill.tab(false);
        assert_eq!(pill.focus(), Some(2), "the pill reopens where the user left");
    }

    #[test]
    fn remembering_a_drag_axis_never_steals_the_focus() {
        let mut pill = PillState::focused_at(0);
        pill.remember_axis(2);
        assert_eq!(pill.focus(), Some(0), "a drag cannot move focus out of a field");
        assert_eq!(pill.last_axis(), 2);
        pill.tab(false);
        assert_eq!(pill.focus(), Some(1), "an already-focused pill keeps cycling");
    }

    #[test]
    fn out_of_range_axes_clamp_to_the_three_fields() {
        assert_eq!(PillState::focused_at(9).focus(), Some(2));
        assert_eq!(PillState::focused_at(usize::MAX).focus(), Some(2));
        assert_eq!(PillState::resting_at(7).last_axis(), 2);

        let mut pill = PillState::focused_at(0);
        assert!(pill.focus_axis(7));
        assert_eq!(pill.focus(), Some(2));
        assert!(!pill.focus_axis(3), "already clamped to the same field");
        assert!(!pill.focus_axis(usize::MAX));

        pill.remember_axis(usize::MAX);
        assert_eq!(pill.last_axis(), 2);
    }

    #[test]
    fn the_coordinate_mode_starts_relative_and_toggles_back() {
        let mut pill = PillState::resting();
        assert!(!pill.absolute(), "a new HUD nudges by an offset");
        assert!(pill.toggle_absolute());
        assert!(pill.absolute());
        assert!(!pill.toggle_absolute());
        assert!(!pill.absolute());
    }

    #[test]
    fn the_coordinate_mode_outlives_focus_and_release() {
        let mut pill = PillState::focused_at(1);
        pill.toggle_absolute();
        pill.release();
        assert!(pill.absolute(), "giving the keyboard back keeps the mode");
        pill.tab(false);
        assert!(pill.absolute(), "and so does focusing a field");
        assert_eq!(pill.focus(), Some(1));
    }

    #[test]
    fn transitions_report_a_change_only_when_they_change_something() {
        let mut pill = PillState::resting();
        assert!(!pill.release(), "already resting");
        assert!(pill.focus_axis(0));
        assert!(!pill.focus_axis(0), "already focused on X");
        assert!(pill.focus_axis(1));
        assert!(!pill.focus_axis(1), "already focused on Y");
        assert!(pill.release());
        assert!(!pill.release(), "already resting");
    }
}
