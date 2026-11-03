//! FerroCAD's browser host: the real shell in a GPUI window on WebGL2.
//!
//! This is no longer a probe. It installs the actual
//! [`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core) window —
//! leading bar, viewport header, status line, model inspector, property editor and
//! console — and supplies a Rust [`AppBackend`](ferrocad_shell_core::AppBackend)
//! instead of CPython. There is no interpreter, no OCCT kernel and no 3D scene yet:
//! the canvas shows the shell's placeholder, and the document is a small in-memory
//! sample. The live 3D viewport is the next milestone (M3).
//!
//! GPUI's web platform starts with an empty font database and panics when it cannot
//! resolve one, so the shell's bundled DejaVu Sans is registered before the window
//! opens (see `ferrocad_shell_core::fonts`).

#![cfg(target_family = "wasm")]

mod backend;

use gpui::{
    App, TitlebarOptions, WebBackendPreference, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, application_with_web_backend, bounds, point, prelude::*, px,
    size,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() {
    // Logging and the panic hook; must run before the application.
    gpui::web_init();

    // WebGL2 on purpose: browser WebGPU is absent on Linux desktop Chrome, and
    // `Auto` would probe for it first.
    application_with_web_backend(WebBackendPreference::WebGl).run(|cx: &mut App| {
        // A font before any text: GPUI resolves fonts during layout and panics if
        // there is nothing to fall back to.
        ferrocad_shell_core::fonts::install(cx);
        // The Rust engine. Boot reads it, so it must be installed first.
        ferrocad_shell_core::app::install_backend(cx, Box::new(backend::SampleBackend::new()));

        let model = match ferrocad_shell_core::Shell::boot(cx) {
            Ok(model) => model,
            Err(error) => {
                // Never abort the module: the page stays usable and the reason is in
                // the browser console.
                log::error!("ferrocad shell: boot failed: {error}");
                return;
            }
        };

        let window = cx.open_window(window_options(), move |window, cx| {
            let shell = cx.new(|cx| ferrocad_shell_core::Shell::from_model(model, cx));
            // The console lives in the side panel, so the main window focuses its
            // root: `Esc` and the command shortcuts bubble from there.
            let root = shell.read(cx).root_focus_handle();
            window.focus(&root, cx);
            shell
        });
        match window {
            Ok(_) => cx.activate(true),
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
