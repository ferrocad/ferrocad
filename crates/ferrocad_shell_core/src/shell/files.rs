//! `Shell` methods: the file verbs (Open and Save).
//!
//! The shell owns both command ids on every platform; the **host** owns the picker and
//! the filesystem. A host supplies a [`FilePicker`](crate::files::FilePicker) — GPUI's
//! platform prompt natively, the page's `<input type=file>` and download in a browser
//! — and the shell decides what happens to the result: a path goes to the backend's
//! `open_path`/`save_path` (so a filesystem-aware engine keeps the real file name), or
//! the host finishes the job itself and calls [`Shell::open_document`].

use crate::files::{FilePicker, OpenOutcome, SaveOutcome};
use crate::shell::*;

/// The command id that opens a document.
pub(in crate::shell) const OPEN: &str = "App_Open";
/// The command id that saves the active document.
pub(in crate::shell) const SAVE: &str = "App_Save";

impl Shell {
    /// Install the host's file picker (`App_Open`/`App_Save`).
    ///
    /// Call before the window opens. Native shells already start with
    /// [`NativeFilePicker`](crate::files::NativeFilePicker); a browser host replaces
    /// it with the page's picker.
    pub fn set_file_picker(&mut self, picker: Box<dyn FilePicker>) {
        self.file_picker = Some(picker);
    }

    /// Whether a file picker is installed. The shell reports the missing half rather
    /// than silently doing nothing.
    pub fn has_file_picker(&self) -> bool {
        self.file_picker.is_some()
    }

    /// `App_Open`: ask the host for a document, then open it.
    ///
    /// The request is asynchronous, so the shell keeps rendering while the picker is
    /// up; the answer arrives on the foreground executor. A path goes through the same
    /// [`Shell::open_path`] a command-line argument does.
    pub(in crate::shell) fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.file_picker.as_mut() else {
            self.file_status("open: this host has no file picker".to_string(), cx);
            return;
        };
        let task = picker.open(cx);
        cx.spawn(async move |shell, cx| match task.await {
            Ok(OpenOutcome::Path(path)) => {
                let _ = shell.update(cx, |shell, cx| {
                    shell.open_path(&path, cx);
                });
            }
            Ok(OpenOutcome::Bytes { name, bytes }) => {
                let _ = shell.update(cx, |shell, cx| {
                    shell.open_document(&name, &bytes, cx);
                });
            }
            // The host owns the picker: it calls `open_document` when the file arrives.
            Ok(OpenOutcome::Delegated | OpenOutcome::Cancelled) => {}
            Err(error) => {
                let _ = shell.update(cx, |shell, cx| shell.file_status(error, cx));
            }
        })
        .detach();
    }

    /// `App_Save`: ask the host where to write, then save there.
    pub(in crate::shell) fn prompt_save(&mut self, cx: &mut Context<Self>) {
        let suggested = format!("{}.FCStd", self.document_name());
        let Some(picker) = self.file_picker.as_mut() else {
            self.file_status("save: this host has no file picker".to_string(), cx);
            return;
        };
        let task = picker.save(cx, &suggested);
        cx.spawn(async move |shell, cx| match task.await {
            Ok(SaveOutcome::Path(path)) => {
                let _ = shell.update(cx, |shell, cx| {
                    let status = match crate::app::save_path(cx, &path) {
                        Ok(()) => format!("saved {}", path.display()),
                        Err(error) => format!("save: {error}"),
                    };
                    shell.file_status(status, cx);
                });
            }
            Ok(SaveOutcome::Delegated) => {
                let _ = shell.update(cx, |shell, cx| shell.deliver_download(cx));
            }
            Ok(SaveOutcome::Cancelled) => {}
            Err(error) => {
                let _ = shell.update(cx, |shell, cx| shell.file_status(error, cx));
            }
        })
        .detach();
    }

    /// Serialize the document and hand the bytes to a picker that owns the
    /// destination (a browser download).
    fn deliver_download(&mut self, cx: &mut Context<Self>) {
        let name = self.document_name();
        let bytes = match crate::app::save_document(cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.file_status(format!("save: {error}"), cx);
                return;
            }
        };
        let Some(picker) = self.file_picker.as_mut() else {
            self.file_status("save: this host has no file picker".to_string(), cx);
            return;
        };
        let status = match picker.deliver(&name, &bytes) {
            Ok(()) => format!("saved {name} ({} bytes)", bytes.len()),
            Err(error) => format!("save: {error}"),
        };
        self.file_status(status, cx);
    }

    /// The active document's name, for the next save's suggested file name.
    fn document_name(&self) -> String {
        let model = self.model.lock().unwrap();
        if model.document.is_empty() {
            "Untitled".to_string()
        } else {
            model.document.clone()
        }
    }

    /// Show a file verb's result in the status line and the console.
    pub(in crate::shell) fn file_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }
}
