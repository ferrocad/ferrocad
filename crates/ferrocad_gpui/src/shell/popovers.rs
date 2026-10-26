//! `Shell` methods: popovers.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Whether the popover keyed `key` is the open one.
    pub(crate) fn popover_open(&self, key: &str) -> bool {
        self.open_popover.as_deref() == Some(key)
    }

    /// Open a header popover. Opening never runs the control's primary command, and
    /// replaces any other open popover (one at a time).
    pub(crate) fn open_popover(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.popover_open(key) {
            self.open_popover = Some(key.to_string());
        }
        // The Grid and Axis menus are the provider-aware popovers (T4): read their
        // summary once when they open, not on every frame. A failure leaves the rows
        // out (and reports why) rather than showing values from a previous open.
        if key == "grid" {
            match python::grid_summary() {
                Ok(summary) => self.grid_summary = Some(summary),
                Err(error) => {
                    self.grid_summary = None;
                    self.console_line(format!("grid: {error}"), cx);
                }
            }
        }
        if key == "axis" {
            match python::axis_summary() {
                Ok(summary) => self.axis_summary = Some(summary),
                Err(error) => {
                    self.axis_summary = None;
                    self.console_line(format!("axis: {error}"), cx);
                }
            }
        }
        // The dock's gizmo menu is provider-aware like Grid and Axes: the rows are the
        // registered `gizmo` backends, so read the registry once when it opens. Without
        // this the menu showed only its config rows and the two providers (native and
        // shader) were invisible.
        if key == "gizmo" {
            match python::provider_features() {
                Ok(features) => self.provider_features = Some(features),
                Err(error) => {
                    self.provider_features = None;
                    self.console_line(format!("providers: {error}"), cx);
                }
            }
        }
        cx.notify();
    }

    /// Close whichever popover is open; returns whether one was open.
    pub(crate) fn close_popover(&mut self, cx: &mut Context<Self>) -> bool {
        self.popover_anchor = None;
        if self.open_popover.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Record where the pointer pressed the control that owns the popover.
    pub(crate) fn set_popover_anchor(&mut self, position: Point<Pixels>) {
        self.popover_anchor = Some(position);
    }

    /// Open (or retarget) the modeless settings card for a `Settings/<Feature>`
    /// node.
    ///
    /// This deliberately does **not** touch the document selection: a settings
    /// panel is a tool over the model, not a selection, so the object being edited
    /// keeps its selection and its gizmo. The card is dismissed only by its own
    /// `✕` or `Esc`.
    pub(super) fn open_settings_card(&mut self, node: &str, cx: &mut Context<Self>) {
        let rows = python::properties(node).unwrap_or_default();
        match self.settings_card.as_mut() {
            Some(card) => {
                card.node = node.to_string();
                card.rows = rows;
            }
            None => {
                self.settings_card = Some(SettingsCard {
                    node: node.to_string(),
                    rows,
                    position: point(px(220.), px(96.)),
                    drag: None,
                });
            }
        }
        cx.notify();
    }

    /// Re-read the card's rows (after an edit, a reset or a provider switch).
    pub(super) fn refresh_settings_card(&mut self) {
        let Some(card) = self.settings_card.as_mut() else {
            return;
        };
        if let Ok(rows) = python::properties(&card.node) {
            card.rows = rows;
        }
    }

    /// Close the settings card; returns whether it was open.
    pub(super) fn close_settings_card(&mut self, cx: &mut Context<Self>) -> bool {
        self.row_popup = None;
        if self.settings_card.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Start dragging the card from its header.
    pub(super) fn begin_settings_drag(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(card) = &mut self.settings_card {
            card.drag = Some((pointer, card.position));
            cx.notify();
        }
    }

    /// Follow a left-button drag; moves outside the header still count because the
    /// shell root forwards its pointer moves here.
    pub(super) fn drag_settings_card(
        &mut self,
        pointer: Point<Pixels>,
        pressed: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if pressed != Some(MouseButton::Left) {
            return;
        }
        let Some(card) = &mut self.settings_card else {
            return;
        };
        let Some((start_pointer, start_position)) = card.drag else {
            return;
        };
        card.position = point(
            start_position.x + pointer.x - start_pointer.x,
            start_position.y + pointer.y - start_pointer.y,
        );
        cx.notify();
    }

    pub(super) fn end_settings_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(card) = &mut self.settings_card {
            if card.drag.take().is_some() {
                cx.notify();
            }
        }
    }

}
