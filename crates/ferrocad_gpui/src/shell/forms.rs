//! `Shell` methods: forms.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Whether a rendered form dialog is open (`FreeCADGui.Control.showDialog`).
    pub fn form_open(&self) -> bool {
        self.form_dialog.is_some()
    }

    /// The open form dialog, for tests and for the host that drives it.
    pub fn form_dialog(&self) -> Option<&Entity<FormDialog>> {
        self.form_dialog.as_ref()
    }

    /// The result the last form left (`None` for a plain `closeDialog`).
    pub fn last_form_result(&self) -> Option<FormResult> {
        self.form_result
    }

    /// Open a parsed form as the shell's modal.
    ///
    /// This mirrors `FreeCADGui.Control.showDialog`: the form is rendered above
    /// the workspace and owns `Esc` (reject) and `Enter` (accept). When a command
    /// dialog, the palette or another form already owns the shell, nothing is
    /// opened and `false` is returned, so the caller can drop its request.
    pub fn open_form(
        &mut self,
        form: Form,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.modal_open() {
            let status = "a dialog is already open".to_string();
            self.model.lock().unwrap().status = status.clone();
            self.console_line(status, cx);
            cx.notify();
            return false;
        }
        // A header menu or a latched wheel would sit behind the modal; dismiss them
        // with the open.
        let _ = self.close_radial(cx);
        self.open_popover = None;
        self.popover_anchor = None;
        let dialog = cx.new(|_| FormDialog::new(form, name));
        let events = cx.subscribe(&dialog, |this: &mut Shell, dialog, event: &FormEvent, cx| {
            this.finish_form(&dialog, event, cx);
        });
        let opened = dialog.read(cx).name().to_string();
        self.form_dialog = Some(dialog);
        self.form_events = Some(events);
        self.form_result = None;
        self.focus_form = true;
        let status = format!("form {opened}: open");
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
        true
    }

    /// Read a `.ui` file and open it as the shell's modal.
    ///
    /// `Ok(false)` means another modal already owns the shell (see [`open_form`]);
    /// `Err` is a read/parse failure with the file named.
    pub fn open_form_path(
        &mut self,
        path: impl AsRef<Path>,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let path = path.as_ref();
        let form = Form::from_path(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(self.open_form(form, name, cx))
    }

    /// Close the open form without recording a result
    /// (`FreeCADGui.Control.closeDialog`). Returns whether one was open.
    pub fn close_form(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.form_dialog.clone() else {
            return false;
        };
        dialog.update(cx, |dialog, cx| dialog.close(cx));
        true
    }

    /// Reject the open form; the modal is released through [`FormEvent`].
    pub(super) fn reject_form(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.form_dialog.clone() {
            dialog.update(cx, |dialog, cx| dialog.reject(cx));
        }
    }

    /// Accept the open form; the modal is released through [`FormEvent`].
    pub(super) fn accept_form(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.form_dialog.clone() {
            dialog.update(cx, |dialog, cx| dialog.accept(cx));
        }
    }

    /// Release the shell's modal when the form closes, whether that was `Esc`,
    /// `Enter`, a footer button or `close_form`.
    pub(super) fn finish_form(
        &mut self,
        dialog: &Entity<FormDialog>,
        event: &FormEvent,
        cx: &mut Context<Self>,
    ) {
        // A late event from a form the shell already released must not clear a
        // newer modal.
        if self.form_dialog.as_ref().map(|dialog| dialog.entity_id()) != Some(dialog.entity_id()) {
            return;
        }
        let FormEvent::Closed(result) = event;
        let name = dialog.read(cx).name().to_string();
        self.form_dialog = None;
        self.form_events = None;
        self.form_result = *result;
        let status = match result {
            Some(FormResult::Accepted) => format!("form {name}: accepted"),
            Some(FormResult::Rejected) => format!("form {name}: rejected"),
            None => format!("form {name}: closed"),
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }

    /// Flip a bool field.
    pub(super) fn toggle_flag(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            if let Some(ValueState::Flag(flag)) = dialog.states.get_mut(index) {
                *flag = !*flag;
                dialog.error = None;
            }
        }
        cx.notify();
    }

}
