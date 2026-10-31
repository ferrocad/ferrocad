//! The tool loadout (U4): the active slot and the topology filter.
//!
//! One store, two views: the bottom-left bar and the header's `Select ▾` pill both
//! write [`Loadout`]. The slot decides which control may arm — Slot 1 hosts the shared
//! triad, Slots 2 and 3 host the selection's manipulator — and the filter decides the
//! [`SelectionRef`] the tree, the typed tool and the arm seam see, so a face click in
//! Slot 1 selects the body.
//!
//! See `docs/project/tasks/tool-loadout.md`.

use super::*;

/// The alpha multiplier of the triad a hover preview materialises (U5): present at
/// the body's origin, but clearly not selected yet.
pub(crate) const HOVER_ARM_OPACITY: f32 = 0.30;

/// Which topology a click selects (the `Select ▾` filter).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SelectFilter {
    /// Whatever the pick reports: the pre-loadout behaviour.
    #[default]
    Auto,
    /// Whole bodies: a sub-element pick is rewritten to its body.
    Body,
    /// The first sub-element the pick reports (`FaceN`).
    Face,
    /// The first sub-element the pick reports (`EdgeN` or an edge chain).
    Edge,
}

impl SelectFilter {
    /// The filter in header and status text.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Body => "Body",
            Self::Face => "Face",
            Self::Edge => "Edge",
        }
    }

    /// Parse a filter name (`body`, `face`, `edge`, `auto`).
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "body" | "object" => Some(Self::Body),
            "face" => Some(Self::Face),
            "edge" => Some(Self::Edge),
            _ => None,
        }
    }

    /// The reference the tool and the arm seam see under this filter.
    pub(crate) fn apply(self, reference: &SelectionRef) -> SelectionRef {
        match self {
            Self::Body => SelectionRef::whole(reference.object),
            Self::Auto | Self::Face | Self::Edge => reference.clone(),
        }
    }

    /// The selected item the shell reports to the tree and the facade under this filter.
    pub(crate) fn apply_item(self, item: SelectionItem) -> SelectionItem {
        match self {
            Self::Body => SelectionItem::new(item.object),
            Self::Auto | Self::Face | Self::Edge => item,
        }
    }

    /// Every filter the `Select ▾` popover offers, in menu order.
    pub(crate) const ALL: [Self; 4] = [Self::Auto, Self::Body, Self::Face, Self::Edge];
}

/// The loadout slot: the instrument in the user's hand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The shared transform triad (translate / rotate).
    #[default]
    Move,
    /// The selected face's extrude / pocket stem.
    PushPull,
    /// The selected edge's fillet / chamfer scrubber.
    Fillet,
}

impl Slot {
    /// The slot a `1`/`2`/`3` press equips.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "1" => Some(Self::Move),
            "2" => Some(Self::PushPull),
            "3" => Some(Self::Fillet),
            _ => None,
        }
    }

    /// The discoverable hotkey and bar prefix.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Move => "1",
            Self::PushPull => "2",
            Self::Fillet => "3",
        }
    }

    /// The bar label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::PushPull => "Push/Pull",
            Self::Fillet => "Fillet",
        }
    }

    /// The topology filter a slot equips. `Select ▾` may override it afterwards.
    pub(crate) fn default_filter(self) -> SelectFilter {
        match self {
            Self::Move => SelectFilter::Body,
            Self::PushPull => SelectFilter::Face,
            Self::Fillet => SelectFilter::Edge,
        }
    }

    /// Whether this slot hosts the shared transform triad.
    pub(crate) fn wants_gizmo(self) -> bool {
        matches!(self, Self::Move)
    }

    /// Whether this slot hosts the selection's compiled or declared manipulator.
    pub(crate) fn wants_manipulator(self) -> bool {
        !self.wants_gizmo()
    }

    /// The slots in bar order.
    pub(crate) const ALL: [Self; 3] = [Self::Move, Self::PushPull, Self::Fillet];
}

/// The one loadout store: the active slot and the topology filter.
///
/// The slot is a preset for the filter, not a second copy of it: equipping a slot
/// (re)sets the filter to that slot's default, and the header's `Select ▾` overrides
/// the filter alone. The bar and the header therefore cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Loadout {
    slot: Slot,
    filter: SelectFilter,
}

impl Default for Loadout {
    fn default() -> Self {
        let slot = Slot::default();
        Self {
            slot,
            filter: slot.default_filter(),
        }
    }
}

impl Loadout {
    pub(crate) fn slot(self) -> Slot {
        self.slot
    }

    pub(crate) fn filter(self) -> SelectFilter {
        self.filter
    }

    /// Equip `slot`, resetting the filter to its default.
    pub(crate) fn set_slot(&mut self, slot: Slot) {
        self.slot = slot;
        self.filter = slot.default_filter();
    }

    /// Override the topology filter without changing the slot.
    pub(crate) fn set_filter(&mut self, filter: SelectFilter) {
        self.filter = filter;
    }

    /// The reference the arm seam sees under the active filter.
    pub(crate) fn narrow(self, reference: &SelectionRef) -> SelectionRef {
        self.filter.apply(reference)
    }

    /// Whether the filter differs from the slot's default (an explicit override).
    pub(crate) fn overridden(self) -> bool {
        self.filter != self.slot.default_filter()
    }
}

impl Shell {
    /// Equip a loadout slot (`1`/`2`/`3` or the bottom-left bar).
    ///
    /// A slot change is a **context exit**: a dirty transform session commits first,
    /// as one transaction, before the new hand narrows the handles.
    pub(crate) fn set_loadout_slot(&mut self, slot: Slot, cx: &mut Context<Self>) {
        // A command that equips a hand always arms it, including the hand already in
        // use: `Move` from the wheel brings a paused hand back.
        self.move_disarmed = false;
        if self.loadout.slot() == slot {
            self.rearm_selection(cx);
            return;
        }
        self.commit_pending_session(cx);
        self.loadout.set_slot(slot);
        self.rearm_selection(cx);
        let status = format!("tool: {}", slot.label());
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// The user's activation of a loadout slot (`1`/`2`/`3` or the bottom-left bar).
    ///
    /// Unlike a command, pressing the **active** Move hand pauses it: the selection
    /// stays, but the triad and the form leave the canvas so the model can be inspected.
    /// Pressing it again arms the hand. Other slots (and a switch back) go through
    /// [`set_loadout_slot`](Self::set_loadout_slot).
    pub(crate) fn activate_loadout_slot(&mut self, slot: Slot, cx: &mut Context<Self>) {
        if slot.wants_gizmo() && self.loadout.slot() == slot {
            if self.move_disarmed {
                self.move_disarmed = false;
                self.rearm_selection(cx);
                self.model.lock().unwrap().status = "tool: Move".to_string();
            } else {
                self.disarm_move_hand(cx);
            }
            cx.notify();
            return;
        }
        self.set_loadout_slot(slot, cx);
    }

    /// Pause the Move hand: keep the selection, drop the handles, form and ruler.
    ///
    /// A pending move is a context exit like a hand change, so it is committed first.
    fn disarm_move_hand(&mut self, cx: &mut Context<Self>) {
        if self.move_disarmed {
            return;
        }
        self.commit_pending_session(cx);
        self.move_disarmed = true;
        self.close_move_hud(cx);
        self.model.lock().unwrap().gizmo_armed = false;
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                viewport.set_manipulators(Vec::new());
                viewport.set_gizmo_idle_scale(None);
                viewport.set_gizmo(None);
                cx.notify();
            });
        }
        self.model.lock().unwrap().status = "tool: Move paused".to_string();
        cx.notify();
    }

    /// Override the topology filter from the header's `Select ▾`.
    ///
    /// Like a slot change, this is a context exit: a pending edit commits before the
    /// new scope changes what the handles target.
    pub(crate) fn set_select_filter(&mut self, filter: SelectFilter, cx: &mut Context<Self>) {
        if self.loadout.filter() == filter {
            return;
        }
        self.commit_pending_session(cx);
        self.loadout.set_filter(filter);
        self.rearm_selection(cx);
        let status = format!("select: {}", filter.label().to_lowercase());
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Commit a dirty session before a context change (a slot or filter switch).
    fn commit_pending_session(&mut self, cx: &mut Context<Self>) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.is_dirty())
        {
            self.commit_session(cx);
        }
    }

    /// Show (or hide) the idle triad for the body under the pointer (U5).
    ///
    /// Hover-to-arm is a **preview**: it writes no selection and no session; it only
    /// materialises the Move hand's triad at the body's origin so a press on a handle
    /// can promote the body and start the drag in one gesture. A latched selection
    /// owns the handles instead, so this is a no-op whenever one exists.
    pub(super) fn sync_hover_arm(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.viewport.clone() else {
            self.hover_arm = None;
            return;
        };
        let (target, over_gizmo) = {
            let viewport = viewport.read(cx);
            (
                viewport.hover_report().target.clone(),
                viewport.gizmo_hovered(),
            )
        };
        self.apply_hover_arm(target.as_ref(), over_gizmo, cx);
    }

    /// Apply a hover target to the preview state (U5).
    ///
    /// `sync_hover_arm` passes the viewport's report; a test can pass a target
    /// directly, which keeps the state machine testable without a scene pick.
    pub(super) fn apply_hover_arm(
        &mut self,
        target: Option<&SelectionRef>,
        over_gizmo: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.viewport.clone() else {
            self.hover_arm = None;
            return;
        };
        let next = if self.hover_arm_allowed() {
            match target {
                Some(reference) => Some(self.loadout.narrow(reference)),
                // The gizmo consumes the hover and the report is empty there: keep the
                // preview while the pointer travels from the body onto its handles, so
                // it does not dissolve just before the press.
                None if over_gizmo => self.hover_arm.clone(),
                None => None,
            }
        } else {
            None
        }
        .filter(|reference| self.display.object_of(reference.object).is_some());
        let previous = self.hover_arm.clone();
        if next == previous {
            return;
        }

        let hosted = next
            .as_ref()
            .map(|reference| self.arm(reference, cx).1)
            .filter(|gizmos| !gizmos.is_empty());
        let idle = hosted.is_some();
        // Nothing to show and nothing was showing: do not notify, or a hover over a
        // body no control claims would loop the observer.
        if !idle && previous.is_none() {
            return;
        }
        self.hover_arm = next.filter(|_| idle);
        let gizmos = hosted.unwrap_or_default();
        viewport.update(cx, |viewport, cx| {
            viewport.set_gizmo_idle_scale(idle.then_some(HOVER_ARM_OPACITY));
            viewport.set_manipulators(Vec::new());
            viewport.set_gizmos(gizmos);
            cx.notify();
        });
    }

    /// Whether hover-to-arm may materialise a triad right now.
    ///
    /// Only the Move hand offers it, and only with nothing latched: a selection, a
    /// session, a tool, a modal, or an open wheel all own the pointer's meaning.
    pub(super) fn hover_arm_allowed(&self) -> bool {
        self.loadout.slot().wants_gizmo()
            && self.session.is_none()
            && self.active_tool.is_none()
            && !self.modal_open()
            && self.wheel_target.is_none()
            && self.model.lock().unwrap().selection.is_empty()
    }

    /// Promote the hover preview's body to the selection when its handle is pressed
    /// (U5).
    ///
    /// The viewport queues `PreviewPhase::Begin` from a handle press. The hover
    /// preview is not a selection yet, so the press must make it one before the
    /// session opens: the commit reads the facade selection. The drag already owns the
    /// hosted gizmo, so this deliberately does **not** re-arm it; it only clears the
    /// idle dimming so the handles read as selected.
    pub(super) fn promote_hover_target(&mut self, target: u32, cx: &mut Context<Self>) {
        let Some(reference) = self.hover_arm.clone() else {
            return;
        };
        if reference.object != target {
            return;
        }
        let Some(name) = self.display.object_of(target).map(str::to_string) else {
            return;
        };
        let item = SelectionItem::with_elements(name, reference.elements);
        self.replace_selection(vec![item], cx);
        self.model.lock().unwrap().gizmo_armed = true;
        if let Some(viewport) = self.viewport.clone() {
            viewport.update(cx, |viewport, cx| {
                if viewport.set_gizmo_idle_scale(None) {
                    cx.notify();
                }
            });
        }
    }
}
