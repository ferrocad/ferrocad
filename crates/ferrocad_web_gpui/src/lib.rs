//! FerroCAD's browser host: the real shell in a GPUI window on WebGL2.
//!
//! This is no longer a probe. It installs the actual
//! [`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core) window —
//! leading bar, viewport header, status line, model inspector, property editor and
//! console — and supplies the engine through
//! [`ferrocad_shell_rust`](https://crates.io/crates/ferrocad_shell_rust): a real
//! `ferrocad_core::Document` whose geometry is built by the OCCT kernel compiled to
//! wasm, so the tree, the property rows and the viewport all come from one document.
//! The backend is the same crate a desktop edition installs; this host adds the
//! window, the page bridge and the file picker.
//!
//! The viewport is the shell's real `InventorViewport` element; its renderer is the
//! zero-copy presenter, which negotiates with the window's own wgpu device on the
//! first paint and composites the frame with `paint_texture`. There is no readback
//! image path on the web (it needs a blocking `poll`), so a presenter that cannot
//! negotiate leaves the canvas blank and reports why.
//!
//! GPUI's web platform starts with an empty font database and panics when it cannot
//! resolve one, so the shell's bundled DejaVu Sans is registered before the window
//! opens (see `ferrocad_shell_core::fonts`).

#![cfg(target_family = "wasm")]

mod host;

use std::cell::RefCell;
use std::rc::Rc;

use ferrocad_shell_rust::RustAppBackend;
use gpui::{
    App, TitlebarOptions, WebBackendPreference, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, application_with_web_backend, bounds, point, prelude::*, px,
    size,
};
use wasm_bindgen::prelude::*;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    fn __wasm_call_ctors();
}

#[wasm_bindgen(start)]
pub fn main() {
    // Retain the WASI stubs before the kernel's objects are linked in.
    ferrocad_wasm::install();
    // The C++ runtime behind the kernel, before anything can call it.
    unsafe { __wasm_call_ctors() };
    // Logging and the panic hook; must run before the application.
    gpui::web_init();

    // WebGL2 on purpose: browser WebGPU is absent on Linux desktop Chrome, and
    // `Auto` would probe for it first.
    application_with_web_backend(WebBackendPreference::WebGl).run(|cx: &mut App| {
        // A font before any text: GPUI resolves fonts during layout and panics if
        // there is nothing to fall back to.
        ferrocad_shell_core::fonts::install(cx);
        // The engine: the real document, over the wasm OCCT kernel. Boot reads it,
        // so it must be installed first. The file verbs are the shell's, so the
        // backend needs nothing else. `?js=1` loads a page module that installs
        // `window.__ferrocadJs`; when present, the same document backend gets the JS
        // script half (S7).
        let mut backend = RustAppBackend::new();
        if ferrocad_shell_js::available() {
            backend = backend.with_script_host(Box::new(ferrocad_shell_js::JsScriptHost));
        }
        ferrocad_shell_core::app::install_backend(cx, Box::new(backend));

        let model = match ferrocad_shell_core::Shell::boot(cx) {
            Ok(model) => model,
            Err(error) => {
                // Never abort the module: the page stays usable and the reason is in
                // the browser console.
                log::error!("ferrocad shell: boot failed: {error}");
                return;
            }
        };

        // The page needs the shell back after a file picker or a drop; GPUI has no
        // global app handle outside a callback, so keep both here.
        let async_app = cx.to_async();
        let shell_slot: Rc<RefCell<Option<gpui::Entity<ferrocad_shell_core::Shell>>>> =
            Rc::new(RefCell::new(None));
        let slot = shell_slot.clone();
        let window = cx.open_window(window_options(), move |window, cx| {
            let shell = cx.new(|cx| ferrocad_shell_core::Shell::from_model(model, cx));
            // The page owns the `<input type=file>` and the download; install its
            // picker before the first Open/Save can run.
            shell.update(cx, |shell, _cx| {
                shell.set_file_picker(Box::new(host::WebFilePicker))
            });
            // The viewport: the element itself is the shell's, and on the web its
            // renderer is the presenter's (see `Shell::enable_viewport`).
            shell.update(cx, |shell, cx| shell.enable_viewport(cx));
            // The console lives in the side panel, so the main window focuses its
            // root: `Esc` and the command shortcuts bubble from there.
            let root = shell.read(cx).root_focus_handle();
            window.focus(&root, cx);
            *slot.borrow_mut() = Some(shell.clone());
            shell
        });
        match window {
            Ok(_) => {
                if let Some(shell) = shell_slot.borrow().as_ref() {
                    host::install(async_app, shell.downgrade());
                }
                cx.activate(true)
            }
            Err(error) => log::error!("ferrocad shell: no window: {error}"),
        }
    });
}

/// The browser window's options.
///
/// The web platform reports server-side decorations, so the shell's window frame takes
/// its no-title-bar branch: the leading bar (the app menu) is the top of the window.
fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("FerroCAD".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(WindowDecorations::Client),
        is_movable: false,
        is_resizable: false,
        window_bounds: Some(WindowBounds::Windowed(bounds(
            point(px(0.), px(0.)),
            size(px(1280.), px(800.)),
        ))),
        ..Default::default()
    }
}
