//! The host's half of the file verbs: the picker `App_Open`/`App_Save` ask for.
//!
//! The shell owns both ids on every platform; a host supplies the one thing the shell
//! cannot do itself — show a picker. Natively that is GPUI's platform prompt
//! (xdg-desktop-portal on Linux) plus `std::fs`; in the browser the page owns the
//! `<input type=file>` and the download, so its picker **delegates**: it asks the page,
//! and the page calls [`Shell::open_document`] once the user has chosen a file.
//!
//! Delegation is why the outcomes are an enum rather than one future. A user gesture
//! cannot be awaited by a Rust future without routing a channel back through the wasm
//! boundary, so a host that owns the picker answers [`OpenOutcome::Delegated`] and
//! finishes the job itself. A host that owns paths answers [`OpenOutcome::Path`], and
//! the shell hands the path to the backend — the interpreter-backed backend opens the
//! file itself, so the document keeps its real file name.
//!
//! Install one with [`Shell::set_file_picker`]; `Shell::from_model` installs
//! [`NativeFilePicker`] everywhere except wasm, where the host installs its own.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use gpui::{App, PathPromptOptions, SharedString};

/// A host's file-picking future. Local (`!Send`), like every GPUI task.
pub type FileTask<T> = Pin<Box<dyn Future<Output = T>>>;

/// What an open request resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The host picked a path; the shell opens it through the backend, which keeps
    /// the document's real file name for the next save.
    Path(PathBuf),
    /// The host read the bytes itself but has no path to hand over.
    Bytes { name: String, bytes: Vec<u8> },
    /// The host owns the picker and calls [`Shell::open_document`] itself when the
    /// user chooses a file (a browser page).
    Delegated,
    /// The user cancelled, or the prompt was dropped.
    Cancelled,
}

/// What a save request resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// The host picked a path; the shell asks the backend to write it.
    Path(PathBuf),
    /// The host owns the destination (a browser download); the shell serializes the
    /// document and hands the bytes to [`FilePicker::deliver`].
    Delegated,
    /// The user cancelled, or the prompt was dropped.
    Cancelled,
}

/// The one thing a host must provide for `App_Open` and `App_Save`.
///
/// The methods take `&App` only to start the platform prompt; the returned future is
/// spawned by the shell, so a picker never needs a `Shell` handle. `deliver` is only
/// called after a [`SaveOutcome::Delegated`].
pub trait FilePicker: 'static {
    /// Ask the host for a document to open.
    fn open(&mut self, cx: &App) -> FileTask<Result<OpenOutcome, String>>;

    /// Ask the host where to write. `suggested` is a file **name**, not a path.
    fn save(&mut self, cx: &App, suggested: &str) -> FileTask<Result<SaveOutcome, String>>;

    /// Write the bytes of a `Delegated` save (a download).
    fn deliver(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
}

/// The native picker: the platform prompt, with paths handed to the backend.
pub struct NativeFilePicker;

impl FilePicker for NativeFilePicker {
    fn open(&mut self, cx: &App) -> FileTask<Result<OpenOutcome, String>> {
        let prompted = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Open Document")),
        });
        Box::pin(async move {
            match prompted.await {
                // A cancelled prompt is not a failure: there is nothing to report.
                Ok(Ok(None)) => Ok(OpenOutcome::Cancelled),
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => Ok(OpenOutcome::Path(path)),
                    None => Ok(OpenOutcome::Cancelled),
                },
                Ok(Err(error)) => Err(format!("open: {error}")),
                // The prompt was dropped (the window closed): nothing to do.
                Err(_) => Ok(OpenOutcome::Cancelled),
            }
        })
    }

    fn save(&mut self, cx: &App, suggested: &str) -> FileTask<Result<SaveOutcome, String>> {
        let suggested = suggested.to_string();
        // The portal needs an absolute directory; the working directory is the one
        // thing a desktop app can always offer.
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let prompted = cx.prompt_for_new_path(&directory, Some(&suggested));
        Box::pin(async move {
            match prompted.await {
                Ok(Ok(None)) => Ok(SaveOutcome::Cancelled),
                Ok(Ok(Some(path))) => Ok(SaveOutcome::Path(path)),
                Ok(Err(error)) => Err(format!("save: {error}")),
                Err(_) => Ok(SaveOutcome::Cancelled),
            }
        })
    }

    fn deliver(&mut self, _name: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("this host has paths; the backend writes the file".to_string())
    }
}
