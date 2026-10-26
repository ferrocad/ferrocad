//! `Shell` methods: properties.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Get the field for a property, creating it (and its submit subscription) on
    /// first use. The field is re-seeded from the model when the value changed
    /// underneath and the user is not editing it.
    pub(super) fn ensure_prop_input(
        &mut self,
        key: &str,
        value: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.prop_inputs.get_mut(key) {
            let focused = input.state.read(cx).focus_handle(cx).is_focused(window);
            if input.seeded != value && !focused {
                input
                    .state
                    .update(cx, |state, cx| state.set_text(value.to_string(), cx));
                input.seeded = value.to_string();
            }
            return input.state.clone();
        }

        let state = cx.new(|cx| {
            let mut state = TextInput::new(cx, "");
            state.set_text(value.to_string(), cx);
            state
        });
        let key = key.to_string();
        let key_for_submit = key.clone();
        let submit = cx.subscribe(
            &state,
            move |this: &mut Shell, _emitter, event: &SubmitEvent, cx| {
                this.commit_property(&key_for_submit, &event.text, cx);
            },
        );
        self.prop_inputs.insert(
            key,
            PropInput {
                state: state.clone(),
                seeded: value.to_string(),
                _submit: submit,
            },
        );
        state
    }

    /// Commit a property field: one filtered edit inside one transaction.
    pub(super) fn commit_property(&mut self, key: &str, text: &str, cx: &mut Context<Self>) {
        let Some((object, property)) = key.split_once('.') else {
            return;
        };
        let status = match python::set_property(object, property, text) {
            Ok(result) if result.ok => {
                // The edit is done, so a palette that asked for it can close (the
                // typed-hex path and the swatch path share this commit).
                self.row_popup = None;
                // The field is re-seeded from the refreshed row on the next render.
                // Seeding it from `text` here would hide a palette pick: the text
                // field still shows the old hex, and the "value changed underneath"
                // check would then think they agree.
                //
                // A settings edit can switch the provider or change the spacing
                // the Grid popover advertises; re-read the summary so an open
                // popover follows the row the user just changed. The axis cross's
                // provider and geometry follow the same rule.
                if key.starts_with("Settings/") {
                    self.grid_summary = python::grid_summary().ok();
                    self.axis_summary = python::axis_summary().ok();
                    self.provider_features = python::provider_features().ok();
                }
                // A settings row can ask for the change to be drawn (a palette, a grid
                // spacing, a provider). Applying it here is what makes an edit visible
                // immediately rather than at the next unrelated repaint.
                if let Some(request) = result.view.clone() {
                    if let Err(error) = self.apply_view(&request, cx) {
                        self.console_line(format!("view: {error}"), cx);
                    }
                }
                self.refresh(cx);
                self.sync_stage(cx);
                self.rearm_selection(cx);
                // The card owns its own rows, so a settings edit has to re-read
                // them here as well as refresh the model's properties.
                self.refresh_settings_card();
                format!("set {key}")
            }
            Ok(result) => format!("property error: {}", result.error),
            Err(e) => format!("property error: {e}"),
        };
        self.model.lock().unwrap().status = status;
        cx.notify();
    }

    /// Start dragging a property row's scrub handle: the gesture begins at the
    /// press, and the window root forwards the moves while the button is held.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_scrub(
        &mut self,
        key: &str,
        label: &str,
        unit: &str,
        start: f64,
        at: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.scrub = Some(ScrubDrag {
            key: key.to_string(),
            label: label.to_string(),
            unit: unit.to_string(),
            scrub: scrub::Scrub::new(start, at),
            anchor: at,
            window,
        });
        cx.notify();
    }

    /// Continue a scrub from a window-root mouse move.
    pub(super) fn drag_scrub(
        &mut self,
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if pressed != Some(MouseButton::Left) {
            return;
        }
        let Some(drag) = self.scrub.as_mut() else {
            return;
        };
        drag.scrub.drag_to(position);
        cx.notify();
    }

    /// Finish a scrub: one commit on release, or nothing if the pointer never moved.
    pub(super) fn end_scrub(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.scrub.take() else {
            return;
        };
        if drag.scrub.changed() {
            let text = drag.scrub.text(&drag.unit);
            self.commit_property(&drag.key, &text, cx);
        }
        cx.notify();
    }

    /// Open the colour palette under a property row.
    ///
    /// Opening is deliberately not a toggle: an outside press dismisses the
    /// overlay before the swatch's click is dispatched, so a toggle here would
    /// immediately reopen it (the same rule as the header popovers).
    pub(super) fn open_color_popup(
        &mut self,
        key: &str,
        label: &str,
        value: &str,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: label.to_string(),
            kind: RowPopupKind::Color,
            value: value.to_string(),
            range: None,
            unit: String::new(),
            choices: Vec::new(),
            dialog_field: None,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Open the range slider for a numeric property with declared bounds.
    /// `unit` is carried into the readout and the commit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_slider_popup(
        &mut self,
        key: &str,
        label: &str,
        value: &str,
        min: f64,
        max: f64,
        unit: &str,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: label.to_string(),
            kind: RowPopupKind::Slider,
            value: value.to_string(),
            range: Some(RowRange { min, max }),
            unit: unit.to_string(),
            choices: Vec::new(),
            dialog_field: None,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Open the dropdown for an enumeration row or a dialog field.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_choice_popup(
        &mut self,
        key: &str,
        value: &str,
        choices: Vec<String>,
        dialog_field: Option<usize>,
        anchor: Point<Pixels>,
        window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.row_popup = Some(RowPopup {
            key: key.to_string(),
            label: String::new(),
            kind: RowPopupKind::Choices,
            value: value.to_string(),
            range: None,
            unit: String::new(),
            choices,
            dialog_field,
            anchor,
            track: None,
            window,
            draft: None,
            dragging: false,
        });
        cx.notify();
    }

    /// Pick a dialog enum's option from the dropdown.
    pub(super) fn set_dialog_choice(&mut self, index: usize, choice: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            let position = dialog
                .spec
                .fields
                .get(index)
                .and_then(|field| field.values.iter().position(|value| value == choice));
            if let Some(position) = position {
                if let Some(state) = dialog.states.get_mut(index) {
                    *state = ValueState::Choice(position);
                    dialog.error = None;
                }
            }
        }
        self.row_popup = None;
        cx.notify();
    }

    /// Dismiss the row overlay, if one is open.
    pub(super) fn close_row_popup(&mut self, cx: &mut Context<Self>) {
        if self.row_popup.take().is_some() {
            cx.notify();
        }
    }

}
