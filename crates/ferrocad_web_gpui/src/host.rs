//! The browser host's half of file I/O, and the handle a page callback re-enters.
//!
//! Rust cannot open a file picker or trigger a download, so `www/index.html` owns both.
//! This module is the one place the backend and the page meet: the shell's
//! [`WebFilePicker`] asks the page for a file (open) or hands it bytes (save), and the
//! page calls [`ferrocad_open`] once it has read a file. Replacing the document needs
//! the GPUI app, so the window installs an [`AsyncApp`] and a weak shell handle here at
//! boot.

use std::cell::RefCell;

use ferrocad_shell_core::Shell;
use ferrocad_shell_core::files::{FilePicker, FileTask, OpenOutcome, SaveOutcome};
use gpui::{App, AsyncApp, WeakEntity};
use wasm_bindgen::prelude::*;

/// Page functions, defined by `www/index.html` before the module's entry point runs.
#[wasm_bindgen]
extern "C" {
    /// Open the page's `<input type="file">` picker (a user gesture must be active).
    #[wasm_bindgen(js_namespace = ["window", "__ferrocad"], js_name = requestOpen, catch)]
    fn js_request_open() -> Result<(), JsValue>;

    /// Deliver bytes to the page as a download named after the document.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocad"], js_name = download, catch)]
    fn js_download(name: &str, bytes: &[u8]) -> Result<(), JsValue>;
}

/// Let the page open its file picker. Returns why it could not.
pub fn request_open() -> Result<(), JsValue> {
    js_request_open()
}

/// Hand the page bytes to save as a file.
pub fn download(name: &str, bytes: &[u8]) -> Result<(), JsValue> {
    js_download(name, bytes)
}

/// The page's file picker, as the shell's [`FilePicker`].
///
/// Open and save both **delegate**: the page owns the `<input type=file>` and the
/// download, and a user gesture cannot be awaited across the wasm boundary. Save's
/// bytes come back through [`FilePicker::deliver`], and open's document arrives through
/// [`ferrocad_open`].
pub struct WebFilePicker;

impl FilePicker for WebFilePicker {
    fn open(&mut self, _cx: &App) -> FileTask<Result<OpenOutcome, String>> {
        // The request runs now, in the same tick as the click or accelerator: the page
        // needs an active user gesture to open its picker.
        let result = request_open().map_err(|error| format!("open: {error:?}"));
        Box::pin(async move { result.map(|()| OpenOutcome::Delegated) })
    }

    fn save(&mut self, _cx: &App, _suggested: &str) -> FileTask<Result<SaveOutcome, String>> {
        // A download needs no picker; the shell serializes the document and calls
        // `deliver`. The suggested name is already the document's.
        Box::pin(async { Ok(SaveOutcome::Delegated) })
    }

    fn deliver(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        download(name, bytes).map_err(|error| format!("download: {error:?}"))
    }
}

/// The live app and shell, so a page callback can re-enter GPUI.
struct Host {
    app: AsyncApp,
    shell: WeakEntity<Shell>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Remember the app and shell once the window exists.
pub fn install(app: AsyncApp, shell: WeakEntity<Shell>) {
    HOST.with(|host| *host.borrow_mut() = Some(Host { app, shell }));
}

/// Run `f` against the live host, if the window exists.
fn with_host<T>(f: impl FnOnce(&Host) -> T) -> Option<T> {
    HOST.with(|host| host.borrow().as_ref().map(f))
}

/// Replace the shell's document with bytes the page read.
///
/// The page calls this after its file picker or a drop; `name` is the file's own name,
/// which the title and the next save use. Returns the status line's text, so a failed
/// open reports why rather than appearing to succeed.
#[wasm_bindgen]
pub fn ferrocad_open(name: &str, bytes: &[u8]) -> Result<String, JsValue> {
    let result = with_host(|host| {
        let Some(shell) = host.shell.upgrade() else {
            return Err("the window was closed".to_string());
        };
        Ok(host
            .app
            .update(|cx| shell.update(cx, |shell, cx| shell.open_document(name, bytes, cx))))
    })
    .unwrap_or_else(|| Err("the shell is not running yet".to_string()));
    result.map_err(|error| JsValue::from_str(&error))
}
