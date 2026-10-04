//! FerroCAD app shell (S1): a `bite-gpui` window that drives the document model
//! through an embedded CPython interpreter.
//!
//! See `docs/app-shell-vision.md`. This binary is the umbrella the 3D viewport
//! and workbench panels later dock into; for now it is the skeleton: a titled
//! window, the inspector / viewport-placeholder / property-editor / console
//! layout, and the live interpreter wiring.

mod input;
mod python;
mod shell;

#[cfg(test)]
mod spike;

use gpui::{
    App, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowOptions, application, bounds, point, prelude::*, px, size,
};

use crate::shell::Shell;

/// Serializes tests that drive the shared embedded CPython instance.
#[cfg(test)]
pub(crate) static PYTHON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn main() {
    let model = match Shell::boot() {
        Ok(model) => model,
        Err(e) => {
            eprintln!("ferrocad: failed to start: {e}");
            std::process::exit(1);
        }
    };

    application().run(move |cx: &mut App| {
        let model = model.clone();
        cx.open_window(window_options(), move |window, cx| {
            let shell = cx.new(|cx| Shell::from_model(model.clone(), cx));
            let console = shell.read(cx).console_focus_handle(cx);
            window.focus(&console, cx);
            shell
        })
        .unwrap();
    });
}

fn window_options() -> WindowOptions {
    WindowOptions {
        // Client-side decorations: we draw the title bar and the resize grips
        // ourselves, which is what GNOME/Wayland requires.
        titlebar: Some(TitlebarOptions {
            title: Some("FerroCAD".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(WindowDecorations::Client),
        is_movable: true,
        is_resizable: true,
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::Windowed(bounds(
            point(px(80.), px(80.)),
            size(px(1180.), px(760.)),
        ))),
        ..Default::default()
    }
}
