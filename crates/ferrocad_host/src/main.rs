//! FerroCAD app shell (S1): a `bite-gpui` window that drives the document model
//! through an embedded CPython interpreter.
//!
//! See `docs/app-shell-vision.md`. This binary is the umbrella the 3D viewport
//! and workbench panels later dock into; for now it is the skeleton: a titled
//! window, the inspector / viewport-placeholder / property-editor / console
//! layout, and the live interpreter wiring.

mod python;
mod shell;

#[cfg(test)]
mod spike;

use gpui::{
    App, TitlebarOptions, WindowBounds, WindowOptions, application, bounds, point, prelude::*, px,
    size,
};

use crate::shell::Shell;

/// Serializes tests that drive the shared embedded CPython instance.
#[cfg(test)]
pub(crate) static PYTHON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn main() {
    let shell = match Shell::boot() {
        Ok(shell) => shell,
        Err(e) => {
            eprintln!("ferrocad: failed to start: {e}");
            std::process::exit(1);
        }
    };
    let model = shell.model();

    application().run(move |cx: &mut App| {
        let model = model.clone();
        cx.open_window(window_options(), move |_window, cx| {
            cx.new(|_| Shell::from_model(model.clone()))
        })
        .unwrap();
    });
}

fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("FerroCAD".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds(
            point(px(80.), px(80.)),
            size(px(1180.), px(760.)),
        ))),
        ..Default::default()
    }
}
