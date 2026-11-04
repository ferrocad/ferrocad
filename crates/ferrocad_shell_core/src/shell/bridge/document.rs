//! `Shell` methods: whole-document I/O.
//!
//! The host owns the file picker and the download; the shell owns what happens after a
//! document is replaced: the model, the selection, the stage and the popovers all
//! describe the old document.

use crate::shell::*;

impl Shell {
    /// Replace the active document with bytes the host decoded (a browser file picker
    /// or a dropped file).
    ///
    /// Returns the status line's text. The host reports it (the browser console) and
    /// the shell shows it in the status line and the transcript.
    pub fn open_document(&mut self, name: &str, bytes: &[u8], cx: &mut Context<Self>) -> String {
        self.replace_document(|cx| crate::app::open_document(cx, name, bytes), cx)
    }

    /// Replace the active document with the file at `path`.
    ///
    /// The native file verbs use this, and the path stays a path: a backend that owns
    /// a filesystem-aware engine opens the file itself, so the document keeps its real
    /// file name (and a later save goes back to it rather than prompting again).
    pub fn open_path(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> String {
        self.replace_document(|cx| crate::app::open_path(cx, path), cx)
    }

    /// The one place a document is replaced.
    ///
    /// Everything that described the old document is reset before the backend is
    /// asked, and re-read after — so a failed open leaves an empty shell rather than
    /// a half-replaced one.
    fn replace_document(
        &mut self,
        open: impl FnOnce(&mut App) -> Result<crate::types::OpenedDocument, String>,
        cx: &mut Context<Self>,
    ) -> String {
        // A modal, a form or a pending move belongs to the document being replaced.
        if self.modal_open() || self.form_dialog.is_some() {
            return "open: finish or cancel the open dialog first".to_string();
        }
        if self.session.is_some() {
            // The same rollback `Esc` performs: the pending move is not written.
            self.drop_session("open discarded the pending move", cx);
        }
        self.open_popover = None;
        self.row_popup = None;
        let _ = self.close_settings_card(cx);
        if self.active_tool.is_some() {
            self.cancel_tool(cx);
        }
        // Clear the selection while it still names the old document's objects.
        self.clear_selection(cx);

        let status = match open(cx) {
            Ok(opened) => {
                {
                    let mut model = self.model.lock().unwrap();
                    model.title = format!("FerroCAD — {}", opened.document);
                    model.status =
                        format!("opened {} ({} object(s))", opened.document, opened.objects);
                }
                self.refresh(cx);
                self.sync_stage(cx);
                self.rearm_selection(cx);
                // A document of a different size must not open from inside or
                // outside it.
                let fit = crate::view::ViewRequest {
                    fit: Some(true),
                    ..Default::default()
                };
                let _ = self.apply_view(&fit, cx);
                format!("opened {} ({} object(s))", opened.document, opened.objects)
            }
            Err(error) => format!("open: {error}"),
        };
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status.clone(), cx);
        cx.notify();
        status
    }

    /// The active document as bytes, for the host to save.
    pub fn save_document(&self, cx: &App) -> Result<Vec<u8>, String> {
        crate::app::save_document(cx)
    }
}
