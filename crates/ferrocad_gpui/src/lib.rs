//! FerroCAD shell: the reusable `bite-gpui` application shell.
//!
//! Boots an embedded CPython interpreter, wires the `FreeCAD` facade, and opens
//! the `bite-gpui` window (model inspector, property editor, Python console).
//!
//! The base application lives in the `ferrocad` crate and calls [`run`]. A
//! redistribution, such as *FerroCAD: Architecture*, is a thin binary that calls
//! [`run_with`] with its own [`HostConfig`]. The engine is reached through the
//! public `FreeCAD` Python API, exactly like a workbench (see
//! `docs/architecture.md` and `docs/architecture/repackaging.md`).

pub mod chrome;
pub mod commands;
pub mod contributions;
pub mod dev_tools;
pub mod dialog;
pub mod display;
/// Q3a: render `ferrocad_ui` forms as gpui elements and host a form dialog.
pub mod forms;
/// Headless edition rendering (`--render out.png`): model → stage → PNG.
pub mod headless;
pub mod manipulators;
pub mod python;
pub mod selection;
pub mod shell;
pub mod shortcuts;
pub mod view;

/// Zero-copy viewport presentation through gpui's wgpu window renderer.
#[cfg(all(
    feature = "gpu-interop",
    not(target_os = "macos"),
    not(target_family = "wasm")
))]
pub mod interop;

#[cfg(test)]
mod spike;

use gpui::{
    App, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions,
    application, bounds, point, prelude::*, px, size,
};

pub use commands::{CommandInfo, CommandOutcome, Surface};
pub use contributions::{Contributions, GizmoFactory, ManipulatorFactory, translate_gizmo};
pub use dev_tools::DevTools;
pub use dialog::{DialogField, DialogSpec};
pub use display::{DisplaySync, StageMesh};
pub use headless::{RenderReport, StlReport, render as render_headless, run as run_headless};
pub use selection::SelectionItem;
pub use shell::{Shell, ShellModel};
pub use shortcuts::Accelerator;
pub use view::{ProjectionName, ViewRequest, ViewState, WorkplaneAction};

/// A zero-copy viewport presenter for this build and platform, if one is available.
///
/// Returns `None` when the `gpu-interop` feature is off, or on a platform whose
/// window renderer cannot share its device (macOS, wasm). The viewport then keeps
/// the readback `RenderImage` path. Even when a presenter is returned it may report
/// itself unsupported at paint time (a headless window or a non-wgpu renderer), in
/// which case the viewport falls back as well.
pub fn zero_copy_presenter() -> Option<Box<dyn ferrocad_viewport::Presenter>> {
    #[cfg(all(
        feature = "gpu-interop",
        not(target_os = "macos"),
        not(target_family = "wasm")
    ))]
    {
        Some(interop::presenter())
    }
    #[cfg(not(all(
        feature = "gpu-interop",
        not(target_os = "macos"),
        not(target_family = "wasm")
    )))]
    {
        None
    }
}

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
    /// `docs/architecture/repackaging.md`).
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
    Shell::boot().map(|_| ())
}

/// Switch on the renderer's opt-in diagnostics.
///
/// `--trace` enables them; `FERROCAD_TRACE=1` does the same without a flag. The
/// knobs and the lines they produce are documented in `docs/viewport/render-diagnostics.md`.
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
