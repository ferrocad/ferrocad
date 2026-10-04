//! FerroCAD shell: the reusable `bite-gpui` application shell.
//!
//! Boots an embedded CPython interpreter, wires the `FreeCAD` facade, and opens
//! the `bite-gpui` window (model inspector, property editor, Python console).
//!
//! The base application lives in the `ferrocad` crate and calls [`run`]. A
//! redistribution, such as *FerroCAD: Architecture*, is a thin binary that calls
//! [`run_with`] with its own [`HostConfig`]. The engine is reached through the
//! public `FreeCAD` Python API, exactly like a workbench (see
//! `docs/architecture.md` and `docs/repackaging.md`).

pub mod python;
pub mod shell;

#[cfg(test)]
mod spike;

use gpui::{
    App, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
    application, bounds, point, prelude::*, px, size,
};

pub use shell::{Shell, ShellModel};

/// Serializes tests that drive the shared embedded CPython instance.
#[cfg(test)]
pub(crate) static PYTHON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Per-edition configuration.
///
/// The base app uses [`HostConfig::default`]; a redistribution (FerroCAD:
/// Architecture, Furniture, ...) supplies its own application name, its Python
/// entry module and, later, its workbench (`Mod/`) directories and startup
/// template.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The application name shown in the OS window title.
    pub app_name: String,
    /// The app's Python script and mod directories, prepended to `sys.path`.
    /// Empty means "discover the development `python/` directory".
    pub python_paths: Vec<std::path::PathBuf>,
    /// The app's Python entry module, imported at boot. It must expose the shell
    /// data functions (`bootstrap`, `model_tree`, `properties`, `set_property`,
    /// `evaluate`). The library ships no copy of this script: it lives with the
    /// app (see `docs/repackaging.md`).
    pub entry_module: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            app_name: "FerroCAD".to_string(),
            python_paths: Vec::new(),
            entry_module: "ferrocad_shell".to_string(),
        }
    }
}

/// Run the base application (the general FerroCAD edition).
pub fn run() {
    run_with(HostConfig::default());
}

/// Run the application with an edition's configuration.
pub fn run_with(config: HostConfig) {
    // Hand the app's scripts to the interpreter bridge before it boots. The
    // interpreter belongs to the shell; the scripts belong to the app.
    python::configure(&config.entry_module, &config.python_paths);

    let model = match Shell::boot() {
        Ok(model) => model,
        Err(e) => {
            eprintln!("ferrocad: failed to start: {e}");
            std::process::exit(1);
        }
    };

    application().run(move |cx: &mut App| {
        let model = model.clone();
        cx.open_window(window_options(&config), move |window, cx| {
            let shell = cx.new(|cx| Shell::from_model(model.clone(), cx));
            let console = shell.read(cx).console_focus_handle(cx);
            window.focus(&console, cx);
            shell
        })
        .unwrap();
    });
}

fn window_options(config: &HostConfig) -> WindowOptions {
    WindowOptions {
        // Client-side decorations: we draw the title bar and the resize grips
        // ourselves, which is what GNOME/Wayland requires.
        titlebar: Some(TitlebarOptions {
            title: Some(config.app_name.clone().into()),
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
