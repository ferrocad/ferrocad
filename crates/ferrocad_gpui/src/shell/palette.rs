//! `Shell` methods: palette.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Open the command palette (Ctrl+K, or the dock's Search slot).
    ///
    /// The palette is modal over the canvas, so it dismisses a header popover. It is
    /// deliberately not opened over a command dialog: that dialog owns the keyboard
    /// and its own accept/cancel path.
    pub(crate) fn open_palette(&mut self, cx: &mut Context<Self>) {
        if self.modal_open() {
            return;
        }
        // A latched wheel is a smaller layer; a modal replaces it rather than
        // surviving underneath the palette.
        let _ = self.close_radial(cx);
        self.open_popover = None;
        self.popover_anchor = None;
        let input = cx.new(|cx| TextInput::new(cx, "Search commands"));
        let change = cx.subscribe(
            &input,
            |this: &mut Shell, _input, event: &ChangeEvent, cx| {
                this.palette_query_changed(event.text.clone(), cx);
            },
        );
        // Enter in the field runs the selection; the field stops the key, so this
        // `SubmitEvent` is the palette's accept path.
        let submit = cx.subscribe(
            &input,
            |this: &mut Shell, _input, _event: &SubmitEvent, cx| {
                this.palette_accept(cx);
            },
        );
        self.palette = Some(Palette {
            input,
            query: String::new(),
            selected: 0,
            previous_focus: None,
            _change: change,
            _submit: submit,
        });
        self.focus_palette = true;
        cx.notify();
    }

    /// Close the palette; returns whether it was open. The focus that opened it is
    /// restored on the next render.
    pub(super) fn close_palette(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(palette) = self.palette.take() else {
            return false;
        };
        self.palette_restore_focus = palette.previous_focus;
        cx.notify();
        true
    }

    /// `Ctrl+K`: open the palette, or close it when it is already open.
    pub(super) fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(cx);
        } else {
            self.open_palette(cx);
        }
    }

    /// Mirror the field's text into the query and rewind the cursor: a shorter list
    /// must never leave the cursor past its end.
    pub(super) fn palette_query_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            palette.query = text;
            palette.selected = 0;
        }
        cx.notify();
    }

    /// Move the palette cursor over the filtered rows, wrapping at both ends.
    pub(super) fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let len = {
            let model = self.model.lock().unwrap();
            palette_filtered(&model.commands, &palette.query).len()
        };
        if len == 0 {
            return;
        }
        if let Some(palette) = self.palette.as_mut() {
            palette.selected =
                (palette.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    /// Enter in the palette: run the selected command, but only when it is enabled.
    /// A disabled row stays visible (like the dock's greyed entries) and says why
    /// nothing happened instead of silently doing nothing.
    pub(super) fn palette_accept(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let query = palette.query.clone();
        let selected = palette.selected;
        let target = {
            let model = self.model.lock().unwrap();
            let rows = palette_filtered(&model.commands, &query);
            let index = selected.min(rows.len().saturating_sub(1));
            rows.get(index)
                .map(|command| (command.id.clone(), command.enabled))
        };
        let Some((id, enabled)) = target else {
            return;
        };
        if !enabled {
            let status = format!("{id}: disabled");
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return;
        }
        // Run it exactly as a click would: the palette has served its purpose.
        self.close_palette(cx);
        self.run_command(&id, cx);
    }

}
