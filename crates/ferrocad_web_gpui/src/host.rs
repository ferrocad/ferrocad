//! The browser host's half of file I/O, and the handle a page callback re-enters.
//!
//! Rust cannot open a file picker or trigger a download, so `www/index.html` owns both.
//! This module is the one place the backend and the page meet: the backend calls the
//! page to ask for a file or to deliver bytes, and the page calls
//! [`ferrocad_open`] once it has read a file. Replacing the document needs the GPUI
//! app, so the window installs an [`AsyncApp`] and a weak shell handle here at boot.

use std::cell::RefCell;

use ferrocad_shell_core::Shell;
use gpui::{AsyncApp, WeakEntity};
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
