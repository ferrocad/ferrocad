//! FerroCAD's native shell assembly: the GPUI UI plus its embedded-CPython backend.
//!
//! [`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core) is the
//! platform-neutral half (window, chrome, panels, the viewport element and the
//! `AppBackend` seam). This crate adds the engine: it boots the embedded CPython
//! interpreter, wires the `FreeCAD` facade, installs the composed backend (the Rust
//! document half with the interpreter as its script half), and opens the window.
//! Headless rendering lives here too, because it drives the same interpreter.
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
//! backend.rs  the desktop engine: `PythonScriptHost` composed with the Rust half
//! script_host.rs  PythonScriptHost: ScriptHost   (the scripting half)
//! python.rs   the interpreter and JSON boundary
//! events.rs   the SoEventCallback listener
//! headless.rs offscreen render / STL export
//! ```
//!
//! Everything from `ferrocad_shell_core` is re-exported here, so an edition keeps
//! its `ferrocad_shell_python::{Shell, commands, selection, …}` paths.

pub mod backend;
pub mod events;
pub mod headless;
pub mod mods;
pub mod python;
pub mod script_host;
pub mod settings;

pub use ferrocad_shell_core::*;

#[cfg(test)]
mod spike;

use gpui::{
    App, FramePipeline, GpuPreference, StandardImmediatePipeline, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowId, WindowOptions,
    application, bounds, point, prelude::*, px, size,
};

pub use backend::{compose, compose_all, compose_with, engine, install, install_composed, install_with};
pub use events::PythonEventCallbacks;
pub use ferrocad_shell_api::Runtime;
use ferrocad_shell_api::Platform;
pub use headless::{RenderReport, StlReport, render as render_headless, run as run_headless};
pub use script_host::PythonScriptHost;

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
    /// The document to open at boot instead of showing only the sample document.
    ///
    /// The shell boots the app script first (which creates its sample), then opens
    /// this file, makes it active and drops the sample. The title, model tree,
    /// properties and viewport all follow the opened document. `None` is the normal
    /// sample-document start.
    pub open_document: Option<std::path::PathBuf>,
    /// The mod set this boot loads (`--addons=`, repeatable). Empty loads every mod the
    /// payload or the checkout makes visible; naming any narrows the boot to exactly those
    /// (B16). One list feeds the `--script-host` derivation, the Python loader, the wasm
    /// bundles and the provider registry.
    ///
    /// A bundle is sandboxed: its only capability is the document API. An addon that
    /// will not load is reported and skipped.
    pub addons: Vec<std::path::PathBuf>,
    /// Which script halves to install (`--script-host=`, `FERROCAD_SCRIPT_HOST`).
    ///
    /// Empty is the edition's engine, CPython. One entry is the common case; more than one
    /// installs a chain whose **first** entry is the primary (B8c), and each entry has to
    /// name a different language — `cpython,pywasm` is one host with two interpreters and is
    /// refused. A configuration this build cannot serve is reported and replaced by
    /// [`Runtime::Plain`].
    pub script_hosts: Vec<Runtime>,
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
            addons: Vec::new(),
            script_hosts: Vec::new(),
        }
    }
}

/// Run the base application (the general FerroCAD edition).
pub fn run() {
    run_with(HostConfig::default());
}

/// Which adapter the window's renderer draws on.
///
/// `FERROCAD_GPU=low` (`integrated`), `high` (`discrete`) or `auto`. Unset means `high`.
///
/// gpui's own default is `LowPower` on Linux — the integrated adapter — and on a hybrid
/// machine that is also the card that composites every presented frame. Measured on the
/// 900a document (1.25 M triangles, Intel UHD 630 + GTX 1050 Ti): the integrated adapter
/// held the window at 12–15 fps and made a blocking pick's readback 100–160 ms; the
/// discrete one gave 17–19 fps and 3.3–4.4 ms. Layout was identical on both, because it is
/// CPU — which is what said the adapter was the variable. A device with no discrete
/// adapter ranks the integrated one next, so this is a preference, not a pin.
///
/// This is the only lever that moves the *window*: `enable_viewport`'s adapter choice
/// reaches the offscreen and image paths only, because the window compositor keeps its own
/// adapter. `FERROCAD_TIMING` prints the one that was actually picked.
fn gpu_preference() -> GpuPreference {
    let setting = std::env::var("FERROCAD_GPU").unwrap_or_default();
    match setting.trim().to_ascii_lowercase().as_str() {
        "low" | "integrated" | "low-power" => GpuPreference::LowPower,
        "auto" => GpuPreference::Auto,
        "" | "high" | "discrete" | "high-performance" => GpuPreference::HighPerformance,
        other => {
            eprintln!("ferrocad: unknown FERROCAD_GPU {other:?} (low, high or auto); using high");
            GpuPreference::HighPerformance
        }
    }
}

/// The window's frame pipeline: the standard one, or the pass accounting under
/// `FERROCAD_TIMING`.
///
/// The timing decorator forwards every pass, so it draws exactly the frames the default
/// pipeline does; it is only installed when somebody is reading the numbers.
fn frame_pipeline(_window: WindowId) -> Box<dyn FramePipeline> {
    if ferrocad_shell_core::runtime::frame_timing::enabled() {
        Box::new(ferrocad_shell_core::runtime::frame_timing::pipeline())
    } else {
        Box::new(StandardImmediatePipeline)
    }
}

/// Run the application with an edition's configuration.
pub fn run_with(config: HostConfig) {
    configure_trace();
    // The mod set first: `--addons` selects it (every mod when unnamed), and one list
    // feeds the host derivation, the grants, the Python loader and the bundles — so the
    // flag cannot select a host without also selecting the mod that asked for it.
    let roots = mod_roots(&config);
    // The scripting configuration: it decides which halves `install` composes, and a value
    // the command line got wrong is reported now rather than after the boot.
    let runtimes = if config.script_hosts.is_empty() {
        match Runtime::from_env_list() {
            Ok(Some(runtimes)) => runtimes,
            Ok(None) => derived_runtimes(&roots),
            Err(error) => {
                eprintln!("ferrocad: {error}");
                derived_runtimes(&roots)
            }
        }
    } else {
        config.script_hosts.clone()
    };
    // B10b: a manifest that asks for more than the installed engine can give is said once
    // and then granted as far as it goes. Refusing is not the policy yet.
    for warning in mods::under_granted(&mods::requests(&roots), &runtimes) {
        eprintln!("ferrocad: {warning}");
    }
    // The wasm workbenches ride beside whatever script half booted, from the same set.
    let addons = mods::addon_packs(&roots);
    if !addons.is_empty() {
        eprintln!(
            "ferrocad: {} addon path(s): {}",
            addons.len(),
            addons
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Hand the app's scripts to the interpreter bridge before it boots. The
    // interpreter belongs to the shell; the scripts belong to the app — and the *set* is
    // the one resolved above, so the loader scans exactly the selected mods.
    python::configure(
        &config.entry_module,
        &config.python_paths,
        &roots,
        true,
    );
    python::configure_startup_document(config.open_document.as_deref());

    // What this boot composed, for the About card. The shell cannot derive it — the
    // halves and bundles are chosen here — so it is installed beside the backend and
    // boot copies it into the model.
    let composition = Composition {
        app: config.app_name.clone(),
        platform: Platform::current().id().to_string(),
        runtimes: runtimes
            .iter()
            .copied()
            .filter(|runtime| *runtime != Runtime::Plain && installable(*runtime))
            .map(|runtime| runtime.id().to_string())
            .collect(),
        addons: addons
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect(),
    };

    let app = application()
        .with_gpu_preference(gpu_preference())
        .with_frame_pipeline(frame_pipeline);
    app.run(move |cx: &mut App| {
        ferrocad_shell_core::app::install_composition(cx, composition);
        backend::install_composed(cx, &runtimes, &addons);
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

/// Whether this build can install `runtime`.
///
/// The same feature switches `backend::compose_all` skips a half on, so the About card
/// reports the halves that installed rather than the set a mod tree asked for (B14: a
/// `mods/` tree declaring JavaScript on an edition built without the `js` feature derives
/// `js`, and the composer skips it). `Plain` needs no engine.
fn installable(runtime: Runtime) -> bool {
    match runtime {
        Runtime::Plain | Runtime::Cpython => true,
        Runtime::Pywasm => cfg!(feature = "pywasm"),
        Runtime::Js => cfg!(feature = "js"),
    }
}

/// The mod set a boot loads, as the roots every scanner takes.
///
/// `--addons` names the set when it is given — that is what makes it a *selection*:
/// every mod when it names none, exactly those otherwise. Without the flag the payload's
/// directories win over discovery, because a packaged app's `python/` sits nowhere near
/// the working directory and the loader would otherwise scan an empty list. Both
/// `--check-init` and the window resolve this the same way, so the diagnostic covers the
/// boot it is checking.
fn mod_roots(config: &HostConfig) -> Vec<std::path::PathBuf> {
    if !config.addons.is_empty() {
        return config.addons.clone();
    }
    if !config.mods_paths.is_empty() {
        return config.mods_paths.clone();
    }
    python::mods_dirs()
}

/// The script hosts the mod directories ask for (B9a).
///
/// `mods/<Name>/` already says what it needs — `InitGui.py`, `InitGui.js` or `addon.wasm`
/// — so the set is derived rather than typed. No mods keeps the edition's engine (CPython),
/// because that is what a run without mods has always had; `--script-host=plain` is still
/// how to ask for no console at all.
fn derived_runtimes(mods_dirs: &[std::path::PathBuf]) -> Vec<Runtime> {
    let kinds = mods::entry_kinds(mods_dirs);
    if kinds.is_empty() {
        // No mod at all: the edition's engine, as it has always been.
        return vec![Runtime::Cpython];
    }

    let derived = Runtime::hosts_for(&kinds, Platform::current());
    let declared = kinds
        .iter()
        .map(|kind| kind.id())
        .collect::<Vec<_>>()
        .join(", ");
    if derived.is_empty() {
        // The set decided, and what it asked for needs no script half (a wasm-only mod set
        // on the desktop). Installing a console nobody asked for would hide that.
        eprintln!("ferrocad: mods declare {declared}; no script host needed");
        return Vec::new();
    }
    // Only say it when the derivation decided something: the common case is the default.
    if derived.len() != 1 || derived[0] != Runtime::Cpython {
        eprintln!(
            "ferrocad: mods declare {declared}; installing {}",
            derived
                .iter()
                .map(|runtime| runtime.id())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    derived
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
    // The mod set before the interpreter, exactly as `run_with` does it: the loader must
    // scan the set this boot derived the hosts from, or `--check-init --addons X` would
    // report a composition the interpreter then does not match.
    let roots = mod_roots(&config);
    python::configure(&config.entry_module, &config.python_paths, &roots, false);
    python::configure_startup_document(config.open_document.as_deref());
    // The **same composition** `run_with` installs, from the same mod set and derived the
    // same way. `--check-init` is what CI and a packager run, so "the payload booted" has
    // to include "the script halves the mod set declares were composed": composing a bare
    // `engine()` here is how a derivation that asked for a half the build lacks went
    // unnoticed while it took the working interpreter down with it (fixed in `compose_all`).
    let runtimes = if config.script_hosts.is_empty() {
        match Runtime::from_env_list() {
            Ok(Some(runtimes)) => runtimes,
            Ok(None) => derived_runtimes(&roots),
            Err(error) => {
                eprintln!("ferrocad: {error}");
                derived_runtimes(&roots)
            }
        }
    } else {
        config.script_hosts.clone()
    };
    let addons = mods::addon_packs(&roots);
    let mut backend = backend::compose_all(&runtimes, &addons)?;
    let model = Shell::boot_with(&mut *backend)?;
    // `--check-init` is the one boot a person can read from a terminal (the window's
    // console lives inside the app), so print the two lines that answer "did the mods
    // load": the workbench count the shell put in its console, and the provider registry
    // the header's popovers list. "Nothing loaded" and "only the native provider" are
    // otherwise indistinguishable from a clean boot here.
    let workbenches: Vec<String> = model
        .lock()
        .map(|model| {
            model
                .console
                .iter()
                .filter(|line| line.starts_with("workbenches:") || line.starts_with("workbench "))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for line in workbenches {
        println!("ferrocad: {line}");
    }
    for feature in backend.provider_features().unwrap_or_default() {
        let backends: Vec<&str> = feature
            .providers
            .iter()
            .map(|provider| provider.backend.as_str())
            .collect();
        println!("ferrocad: provider {}: {}", feature.feature, backends.join(", "));
    }
    Ok(())
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
