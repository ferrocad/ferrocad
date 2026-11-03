//! FerroCAD's native shell assembly: the GPUI UI plus its embedded-CPython backend.
//!
//! [`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core) is the
//! platform-neutral half (window, chrome, panels, the viewport element and the
//! `AppBackend` seam). This crate adds the engine: it boots the embedded CPython
//! interpreter, wires the `FreeCAD` facade, installs the [`PythonAppBackend`], and
//! opens the window. Headless rendering lives here too, because it drives the same
//! interpreter.
//!
//! The base application lives in the `ferrocad` crate and calls [`run`]. A
//! redistribution, such as *FerroCAD: Architecture*, is a thin binary that calls
//! [`run_with`] with its own [`HostConfig`]. The engine is reached through the
//! public `FreeCAD` Python API, exactly like a workbench (see
//! `docs/developer/architecture.md` and `docs/developer/architecture/repackaging.md`).
//!
//! # Module map
//!
//! ```text
//! backend.rs  PythonAppBackend: AppBackend      (the seam implementation)
//! python.rs   the interpreter and JSON boundary
//! events.rs   the SoEventCallback listener
//! headless.rs offscreen render / STL export
//! ```
//!
//! Everything from `ferrocad_shell_core` is re-exported here, so an edition keeps
//! its `ferrocad_shell_gpui::{Shell, commands, selection, …}` paths.

pub mod backend;
pub mod events;
pub mod headless;
pub mod python;

pub use ferrocad_shell_core::*;

#[cfg(test)]
mod spike;

use gpui::{
    App, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
    application, bounds, point, prelude::*, px, size,
};

pub use backend::PythonAppBackend;
pub use events::PythonEventCallbacks;
pub use headless::{RenderReport, StlReport, render as render_headless, run as run_headless};

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
    /// Workbench (`mods/`) directories scanned at boot for `Init.py`/`InitGui.py`.
    /// Empty means "discover a `mods/` directory beside `python_paths`, or
    /// `FERROCAD_MODS_PATH`".
    pub mods_paths: Vec<std::path::PathBuf>,
    /// The app's Python entry module, imported at boot. It must expose the shell
    /// data functions (`bootstrap`, `model_tree`, `properties`, `set_property`,
    /// `evaluate`) and, for the 3D viewport, `stage_meshes` and `interaction` (see
    /// `display` and `shell` in the shell helpers). Workbenches contribute display
    /// meshes and edits through the `FreeCAD` module hooks, never through this module.
    /// The library ships no copy of this script: it lives with the app (see
    /// `docs/developer/architecture/repackaging.md`).
    pub entry_module: String,
    /// The manipulators the edition's workbenches contribute (the Part edge arc, a BIM
    /// grip, …). The base app registers none, and the shell names no workbench.
    pub contributions: Contributions,
    /// A document to open at boot instead of showing only the sample document.
    ///
    /// The shell boots the app script first (which creates its sample), then opens
    /// this file, makes it active and drops the sample. The title, model tree,
    /// properties and viewport all follow the opened document. `None` is the normal
    /// sample-document start.
    pub open_document: Option<std::path::PathBuf>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            app_name: "FerroCAD".to_string(),
            python_paths: Vec::new(),
            mods_paths: Vec::new(),
            entry_module: "ferrocad_shell".to_string(),
            contributions: Contributions::default(),
            open_document: None,
        }
    }
}

/// Run the base application (the general FerroCAD edition).
pub fn run() {
    run_with(HostConfig::default());
}

/// Run the application with an edition's configuration.
pub fn run_with(config: HostConfig) {
    configure_trace();
    // Hand the app's scripts to the interpreter bridge before it boots. The
    // interpreter belongs to the shell; the scripts belong to the app.
    python::configure(
        &config.entry_module,
        &config.python_paths,
        &config.mods_paths,
        true,
    );
    python::configure_startup_document(config.open_document.as_deref());

    application().run(move |cx: &mut App| {
        backend::install(cx);
        let model = match Shell::boot(cx) {
            Ok(model) => model,
            Err(e) => {
                eprintln!("ferrocad: failed to start: {e}");
                std::process::exit(1);
            }
        };
        let model = model.clone();
        cx.open_window(window_options(&config), move |window, cx| {
            let contributions = config.contributions.clone();
            let shell = cx.new(|cx| {
                let mut shell = Shell::from_model(model.clone(), cx);
                shell.set_contributions(contributions.clone());
                shell
            });
            // Best-effort: needs a GPU. Headless tests skip this and keep the
            // placeholder (see `Shell::enable_viewport`).
            shell.update(cx, |shell, cx| shell.enable_viewport(cx));
            // The console now lives in Dev Tools, so the main window focuses its root:
            // `Esc` and the command shortcuts bubble from there.
            let root = shell.read(cx).root_focus_handle();
            window.focus(&root, cx);
            shell
        })
        .unwrap();
    });
}

/// Run [`Shell::boot`] and return without opening a window.
///
/// This is the `--check-init` path: it runs the same boot as [`run_with`] —
/// loading the bundled interpreter and every native dependency, importing the
/// entry module and scanning the workbenches — then stops. Because it never
/// creates a GPUI application, it needs no display server and no GPU, so a
/// packaged layout that is missing a DLL or an entry point fails here instead of
/// on a user's machine.
pub fn check_init(config: HostConfig) -> Result<(), String> {
    configure_trace();
    python::configure(
        &config.entry_module,
        &config.python_paths,
        &config.mods_paths,
        false,
    );
    python::configure_startup_document(config.open_document.as_deref());
    let mut backend = PythonAppBackend::new();
    Shell::boot_with(&mut backend).map(|_| ())
}

/// Switch on the renderer's opt-in diagnostics.
///
/// `--trace` enables them; `FERROCAD_TRACE=1` does the same without a flag. The
/// knobs and the lines they produce are documented in `docs/developer/viewport/render-diagnostics.md`.
fn configure_trace() {
    if ferrocad_wgpu::trace::args_enable(std::env::args().skip(1)) {
        ferrocad_wgpu::trace::enable();
    }
    // Resolve the environment decision on the main thread, before any frame, so a
    // black screen cannot start before the log does.
    if ferrocad_wgpu::trace::enabled() {
        eprintln!("ferrocad: render diagnostics on (--trace / FERROCAD_TRACE)");
    }
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
