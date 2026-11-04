//! `Shell` methods: the native file verbs (Open and Save).
//!
//! The backend moves bytes; the host owns the dialog and the filesystem. Natively the
//! dialog is GPUI's platform prompt (xdg-desktop-portal on Linux) and the bytes go
//! through `std::fs`. The browser host owns its own file input and download instead, so
//! these verbs exist only where GPUI has a prompt.

use std::path::PathBuf;

use gpui::{PathPromptOptions, SharedString};

use crate::shell::*;

/// The command id that opens a document.
pub(in crate::shell) const OPEN: &str = "App_Open";
/// The command id that saves the active document.
pub(in crate::shell) const SAVE: &str = "App_Save";

impl Shell {
    /// `App_Open`: ask the platform for a document, then open it.
    ///
    /// The prompt is asynchronous, so the shell keeps rendering while it is up; the
    /// answer arrives on the foreground executor and goes through the same
    /// [`Shell::open_path`] a command-line argument would.
    pub(in crate::shell) fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let prompted = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Open Document")),
        });
        cx.spawn(async move |shell, cx| match prompted.await {
            // A cancelled prompt is not a failure: there is nothing to report.
            Ok(Ok(None)) => {}
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.open_path(&path, cx);
                    });
                }
            }
            Ok(Err(error)) => {
                let _ = shell.update(cx, |shell, cx| {
                    shell.file_status(format!("open: {error}"), cx);
                });
            }
            // The prompt was dropped (the window closed); nothing to do.
            Err(_) => {}
        })
        .detach();
    }

    /// `App_Save`: ask where to write, then save the active document there.
    pub(in crate::shell) fn prompt_save(&mut self, cx: &mut Context<Self>) {
        let suggested = format!("{}.FCStd", self.document_name());
        // The portal needs an absolute directory; the working directory is the one
        // thing a desktop app can always offer.
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let prompted = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn(async move |shell, cx| match prompted.await {
            Ok(Ok(None)) => {}
            Ok(Ok(Some(path))) => {
                let _ = shell.update(cx, |shell, cx| {
                    let status = match crate::app::save_path(cx, &path) {
                        Ok(()) => format!("saved {}", path.display()),
                        Err(error) => format!("save: {error}"),
                    };
                    shell.file_status(status, cx);
                });
            }
            Ok(Err(error)) => {
                let _ = shell.update(cx, |shell, cx| {
                    shell.file_status(format!("save: {error}"), cx);
                });
            }
            Err(_) => {}
        })
        .detach();
    }

    /// The active document's name, for the next save's suggested file name.
    fn document_name(&self) -> String {
        let model = self.model.lock().unwrap();
        model
            .tree
            .first()
            .map(|document| document.name.clone())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// Show a file verb's result in the status line and the console.
    pub(in crate::shell) fn file_status(&mut self, status: String, cx: &mut Context<Self>) {
        self.model.lock().unwrap().status = status.clone();
        self.console_line(status, cx);
        cx.notify();
    }
}
