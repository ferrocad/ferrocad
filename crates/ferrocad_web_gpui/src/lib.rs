//! FerroCAD's browser host: the real shell in a GPUI window on WebGL2.
//!
//! This is no longer a probe. It installs the actual
//! [`ferrocad_shell_core`](https://crates.io/crates/ferrocad_shell_core) window —
//! leading bar, viewport header, status line, model inspector, property editor and
//! console — and supplies the engine through
//! [`ferrocad_shell_web`](https://docs.rs/ferrocad_shell_web): a real
//! `ferrocad_core::Document` whose geometry is built by the OCCT kernel compiled to
//! wasm, so the tree, the property rows and the viewport all come from one document.
//! The document half is the same `RustAppBackend` crate a desktop edition installs;
//! this host adds the window, the page bridge, the file picker and the choice of
//! scripting runtime (`?runtime=plain|pywasm|js`, see `ferrocad_shell_web`).
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

use ferrocad_shell_web::Runtime;
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
        // backend needs nothing else. The scripting runtime is the page's choice;
        // `ferrocad_shell_web` composes it and logs which one booted. A runtime that
        // cannot boot (a `?js=1` page with no module, an interpreter that fails)
        // costs the console, not the window.
        let (backend, runtimes) = match ferrocad_shell_web::page_backend() {
            Ok((backend, runtimes)) => (backend, runtimes),
            Err(error) => {
                log::error!("ferrocad shell: {error}; the document runs alone");
                (ferrocad_shell_web::plain(), vec![Runtime::Plain])
            }
        };
        // A chain logs in order; the first name is the primary.
        log::info!(
            "ferrocad: scripting runtime {}",
            runtimes
                .iter()
                .map(|runtime| runtime.id())
                .collect::<Vec<_>>()
                .join(",")
        );
        ferrocad_shell_core::app::install_backend(cx, backend);

        let model = match ferrocad_shell_core::Shell::boot(cx) {
            Ok(model) => model,
            Err(error) => {
                // Never abort the module: the page stays usable and the reason is in
                // the browser console.
                log::error!("ferrocad shell: boot failed: {error}");
                return;
            }
        };

        // The headless capture cannot scrub the dress-up arc or drag the push/pull
        // stem; `?manipulator=` sends the commit such a gesture would, before the
        // first frame, so the capture asserts on the document instead of on pixels.
        probe_manipulator(cx);

        // The page needs the shell back after a file picker or a drop; GPUI has no
        // global app handle outside a callback, so keep both here.
        let async_app = cx.to_async();
        let shell_slot: Rc<RefCell<Option<gpui::Entity<ferrocad_shell_core::Shell>>>> =
            Rc::new(RefCell::new(None));
        let slot = shell_slot.clone();
        let window = cx.open_window(window_options(), move |window, cx| {
            let shell = cx.new(|cx| {
                let mut shell = ferrocad_shell_core::Shell::from_model(model, cx);
                // The workbench surface a browser session gets: the same handles a
                // desktop edition contributes (the parametric edition adds these two
                // manipulators to `ferrocad::base_contributions`), and no interpreter
                // is involved on either side — `RustAppBackend::apply_interaction`
                // applies the commits itself.
                shell.set_contributions(contributions());
                shell
            });
            // The page owns the `<input type=file>` and the download; install its
            // picker before the first Open/Save can run.
            shell.update(cx, |shell, _cx| {
                shell.set_file_picker(Box::new(host::WebFilePicker))
            });
            // The viewport: the element itself is the shell's, and on the web its
            // renderer is the presenter's (see `Shell::enable_viewport`).
            shell.update(cx, |shell, cx| {
                shell.enable_viewport(cx);
                // After the viewport: `?gesture=` arms a manipulator and needs one to
                // arm it on.
                probe_gesture(shell, cx);
            });
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

/// The manipulation a browser session gets.
///
/// The base contributions plus Part's two manipulators — the same composition
/// `ferrocad_parametric`'s `main` installs, built here because the web host has no
/// edition crate and no app script. Every factory is plain Rust over `ferrocad_stage`,
/// so nothing here needs an interpreter; the commits they emit are applied by
/// `ferrocad_shell_rust`'s document half.
fn contributions() -> ferrocad_shell_core::Contributions {
    ferrocad_shell_core::base_contributions()
        .with_manipulator(ferrocad_part_gui::fillet_manipulator)
        .with_manipulator(ferrocad_part_gui::push_pull_manipulator)
}

/// `?manipulator=<id>:<action>:<object>:<element>:<values>` — one workbench commit,
/// through the function a released handle calls.
///
/// The viewport's bridge resolves the pick, asks the manipulator registry, and sends
/// what the scrub produced to [`ferrocad_shell_core::app::apply_interaction`]. A
/// headless capture can do neither of the first two, so this supplies the third: the
/// commit is the real one, and everything after it — the document edit, the feature
/// object, the stage — is the arm under test. `<element>` may be empty, and `<values>`
/// is a comma-separated list (`2` for a fillet, `3` or `-2` for a push/pull).
fn probe_manipulator(cx: &mut gpui::App) {
    let query = ferrocad_shell_web::page_query();
    let Some(spec) = ferrocad_shell_web::parameter(&query, "manipulator") else {
        return;
    };
    let fields: Vec<&str> = spec.split(':').collect();
    let [manipulator, action, object, element, values] = fields.as_slice() else {
        log::error!(
            "ferrocad probe: ?manipulator wants <id>:<action>:<object>:<element>:<values>, got {spec:?}"
        );
        return;
    };
    let values: Vec<f64> = values
        .split(',')
        .filter(|value| !value.is_empty())
        .filter_map(|value| value.parse().ok())
        .collect();

    let mut commit = ferrocad_stage::InteractionCommit::new(*manipulator, 0, *action);
    if !element.is_empty() {
        commit = commit.with_element(*element);
    }
    if !values.is_empty() {
        commit = commit.with_values(values);
    }

    match ferrocad_shell_core::app::apply_interaction(cx, &commit, object) {
        Ok(result) => log::info!(
            "ferrocad probe: {manipulator} {action} {object} -> ok={} {}",
            result.ok, result.error
        ),
        Err(error) => log::error!("ferrocad probe: {manipulator} {action} {object} -> {error}"),
    }
    // The stage is the answer a capture asserts on: `stage_meshes` logs
    // `ferrocad: stage draws …`, which shows the feature replacing its base.
    if let Err(error) = ferrocad_shell_core::app::stage_meshes(cx) {
        log::error!("ferrocad probe: stage: {error}");
    }
}

/// `?gesture=<slot>:<object>:<element>:<pixels>` — one manipulator gesture, aimed where
/// the manipulator drew its handle.
///
/// `?manipulator=` sends the commit a handle *would* emit; this drives the handle. The
/// slot is the hand an arrow key equips (`2` push/pull, `3` dress-up), because the default
/// hand hosts the triad rather than a manipulator. Then the selection arms the handle
/// through the shell's click handler, the press lands on the stroke's far end, the
/// manipulator's own `drag_begin`/`drag_update`/`drag_end` produce the value, and the
/// commit goes through the shell's interaction path. Only the pointer device is missing —
/// which is the one thing a headless capture cannot have. `pixels` is how far the pointer
/// is pulled outward.
fn probe_gesture(
    shell: &mut ferrocad_shell_core::Shell,
    cx: &mut gpui::Context<ferrocad_shell_core::Shell>,
) {
    let query = ferrocad_shell_web::page_query();
    let Some(spec) = ferrocad_shell_web::parameter(&query, "gesture") else {
        return;
    };
    let fields: Vec<&str> = spec.split(':').collect();
    let [slot, object, element, pixels] = fields.as_slice() else {
        log::error!(
            "ferrocad probe: ?gesture wants <slot>:<object>:<element>:<pixels>, got {spec:?}"
        );
        return;
    };
    let Ok(pixels) = pixels.parse::<f64>() else {
        log::error!("ferrocad probe: ?gesture pixels are not a number, got {spec:?}");
        return;
    };
    let element = (!element.is_empty()).then_some(*element);
    match shell.probe_manipulator(slot, object, element, pixels, cx) {
        Ok([x, y]) => log::info!(
            "ferrocad probe: gesture slot {slot} {object} {} pulled {pixels} from {x},{y}",
            element.unwrap_or("-")
        ),
        Err(error) => log::error!(
            "ferrocad probe: gesture slot {slot} {object} {} failed: {error}",
            element.unwrap_or("-")
        ),
    }
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
