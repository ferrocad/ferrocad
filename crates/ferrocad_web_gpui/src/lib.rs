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
mod mods;
mod prefs;
mod settings;

use std::cell::RefCell;
use std::rc::Rc;

use ferrocad_shell_api::prefs::PrefsStore as _;
use ferrocad_shell_web::Runtime;
use settings::WebSettings;
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
        //
        // B17a: the page's refusals (`localStorage`) are read here, before the packs are
        // fetched, because they decide which packs the fetch asks for.
        let prefs_store = prefs::LocalStoragePrefs;
        let prefs = prefs_store.load();
        let (backend, runtimes) = match ferrocad_shell_web::page_backend_with_settings(Some(
            Box::new(WebSettings::default()),
        )) {
            Ok((backend, runtimes)) => (backend, runtimes),
            Err(error) => {
                log::error!("ferrocad shell: {error}; the document runs alone");
                (
                    ferrocad_shell_web::plain_with_settings(Some(Box::new(WebSettings::default()))),
                    vec![Runtime::Plain],
                )
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
        ferrocad_shell_core::app::install_composition(
            cx,
            ferrocad_shell_core::Composition {
                // The page is not an edition, so it has no edition name to show.
                app: "FerroCAD".to_string(),
                platform: "browser".to_string(),
                runtimes: runtimes
                    .iter()
                    .filter(|runtime| **runtime != Runtime::Plain)
                    .map(|runtime| runtime.id().to_string())
                    .collect(),
                // `?addons=` is the page's bundle request; the loader fetches them, so the
                // card reports the request the page is running.
                addons: ferrocad_shell_web::parameter(
                    &ferrocad_shell_web::page_query(),
                    "addons",
                )
                .into_iter()
                .collect(),
            },
        );
        ferrocad_shell_core::app::install_backend(cx, backend);
        // The About card's controls: what this page read, and the store a click writes
        // through. The pack names arrive with the fetched index (`note_prefs_mods`).
        ferrocad_shell_core::app::install_prefs(
            cx,
            ferrocad_shell_core::PrefsReport {
                prefs: prefs.clone(),
                mods: Vec::new(),
                store: String::new(),
            },
            Some(Box::new(prefs_store)),
        );

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

        // A settings card is drawn into the canvas like the tree, so a capture cannot
        // read it back out of the DOM; `?settings=` logs the rows the pane would show.
        probe_settings(cx);

        // The palette needs synthetic keystrokes and a capture has none; `?command=`
        // runs one id through the backend so a row a runtime only lists can be checked.
        probe_command(cx);

        // A console line is a keystroke sequence a capture cannot type; `?script=` runs
        // one snippet in the console namespace, so a document write through the Python
        // half (B18b item 5) is reachable without a pointer or a keyboard.
        probe_script(cx);

        // Which providers this page has is a configuration fact, not a pixel: log it.
        probe_providers(cx);

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
                if let Some(shell) = shell_slot.borrow().as_ref().cloned() {
                    host::install(async_app.clone(), shell.downgrade());
                    // B19c: the page has no filesystem, so the packs are fetched after
                    // the window is up and handed to the script half one at a time.
                    // Only a Python half can run `Init.py`/`InitGui.py`, and a page
                    // that wants no fetch says `?mods=0`.
                    let runs_python = runtimes.iter().any(|runtime| {
                        ferrocad_shell_api::Language::of(*runtime)
                            == Some(ferrocad_shell_api::Language::Python)
                    });
                    if runs_python {
                        let value = ferrocad_shell_web::parameter(
                            &ferrocad_shell_web::page_query(),
                            "mods",
                        )
                        .unwrap_or_else(|| "1".to_string());
                        if !ferrocad_shell_web::mods::is_off(&value) {
                            let index = ferrocad_shell_web::mods::index_url(&value).to_string();
                            mods::start(async_app, shell.downgrade(), index, prefs.clone());
                        }
                    }
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

/// `?providers=1` — the provider registry, as the header's popovers and Dev Tools read it.
///
/// The wasm half has the mod provider registry (B18a step 5), and the shipped settings
/// clients (`mods/GridOverlay`, `AxisOverlay`, `GizmoOverlay`) do register there since
/// B18b. This probe runs at boot, though, before the page fetches a pack, so it reports
/// the native provider alone. With an empty registry at that instant the facade's
/// summaries are the closed empty case, which `RustAppBackend` treats as "not mine": the
/// header names the native provider that draws the ground, and `registry` (the document
/// half's `provider_features`) agrees. After the fetch both halves list the mods.
fn probe_providers(cx: &mut gpui::App) {
    let query = ferrocad_shell_web::page_query();
    if ferrocad_shell_web::parameter(&query, "providers").is_none() {
        return;
    }
    let grid = ferrocad_shell_core::app::grid_summary(cx).unwrap_or_default();
    let axis = ferrocad_shell_core::app::axis_summary(cx).unwrap_or_default();
    let features = ferrocad_shell_core::app::provider_features(cx).unwrap_or_default();
    // Each summary names its provider through `label` (the display name the popover
    // prints) or `backend` (the id); `switch` is that popover's switch list, and
    // `registry` is what Dev Tools lists. They differ on purpose: one provider means
    // nothing to switch to, but it is still a registered provider.
    let name = |label: Option<&str>, backend: Option<&str>| {
        label.or(backend).unwrap_or("none").to_string()
    };
    let registry: Vec<String> = features
        .iter()
        .map(|feature| format!("{}={}", feature.feature, feature.providers.len()))
        .collect();
    log::info!(
        "ferrocad probe: providers grid={} switch={} axis={} switch={} registry={}",
        name(grid.label.as_deref(), grid.backend.as_deref()),
        grid.providers.len(),
        name(axis.label.as_deref(), axis.backend.as_deref()),
        axis.providers.len(),
        if registry.is_empty() {
            "none".to_string()
        } else {
            registry.join(",")
        }
    );
}

/// `?settings=<node>` or `?settings=<node>.<property>=<value>` — the settings card.
///
/// The two calls the property pane makes for a card, and nothing else: a read is
/// `app::properties`, a write is `app::set_property` followed by the request the
/// shell applies. The shell paints the tree and the card into the canvas, so a
/// headless capture cannot read either from the DOM; this logs what the card would
/// render, which is the surface `?runtime=plain` and `?runtime=js` did not have
/// before the grid card moved into the document half.
fn probe_settings(cx: &mut gpui::App) {
    let query = ferrocad_shell_web::page_query();
    let Some(spec) = ferrocad_shell_web::parameter(&query, "settings") else {
        return;
    };
    let Some((key, value)) = spec.split_once('=') else {
        let node = spec.trim();
        match ferrocad_shell_core::app::properties(cx, node) {
            Ok(rows) => log::info!(
                "ferrocad probe: settings {node} -> {} row(s): {}",
                rows.len(),
                rows.iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(error) => log::error!("ferrocad probe: settings {node} -> {error}"),
        }
        // Rows without a tree node are unreachable: the card opens from the model
        // tree (or a `Settings/…` command), so the node has to be listed.
        match ferrocad_shell_core::app::model_tree(cx) {
            Ok(tree) => log::info!(
                "ferrocad probe: tree {}",
                tree.iter()
                    .map(|document| document.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(error) => log::error!("ferrocad probe: tree -> {error}"),
        }
        return;
    };
    // The pane's key format: `commit_property` splits `<object>.<property>`.
    let Some((object, property)) = key.trim().rsplit_once('.') else {
        log::error!(
            "ferrocad probe: ?settings wants <node> or <node>.<property>=<value>, got {spec:?}"
        );
        return;
    };
    match ferrocad_shell_core::app::set_property(cx, object, property, value.trim()) {
        Ok(result) => {
            log::info!(
                "ferrocad probe: settings {key}={} -> ok={} {}",
                value.trim(),
                result.ok,
                result.error
            );
            // The request is the half a capture can check: the shell applies it and the
            // viewport follows, so a row that returns no request would be a dead edit.
            if let Some(view) = result.view {
                log::info!("ferrocad probe: settings view {view:?}");
            }
        }
        Err(error) => log::error!(
            "ferrocad probe: settings {key}={} -> {error}",
            value.trim()
        ),
    }
}

/// `?command=<id>` — run one command through the backend, before the first frame.
///
/// A palette row is reached with synthetic keystrokes and a capture has none, so this
/// is how a verb a runtime only lists is exercised. The outcome is the shell's to apply
/// (a `select` opens the settings card); the directive is logged, because that is the
/// part a capture can assert on — the card itself is painted into the canvas.
fn probe_command(cx: &mut gpui::App) {
    let query = ferrocad_shell_web::page_query();
    let Some(id) = ferrocad_shell_web::parameter(&query, "command") else {
        return;
    };
    let id = id.trim();
    match ferrocad_shell_core::app::run_command(cx, id) {
        Ok(outcome) => log::info!(
            "ferrocad probe: command {id} -> ok={} status={:?} select={:?}",
            outcome.ok, outcome.status, outcome.select
        ),
        Err(error) => log::error!("ferrocad probe: command {id} -> {error}"),
    }
}

/// `?script=<python>` — one console snippet, before the first frame.
///
/// A capture has no keyboard, so this is how a **write** is exercised in the browser: the
/// snippet runs in the same namespace the console uses (so `FreeCAD`/`App`/`doc` are
/// there), and its outcome is logged. `%0A`, `%27` and the rest let a URL carry the code
/// (`ferrocad_shell_web::parameter` decodes them).
fn probe_script(cx: &mut gpui::App) {
    let query = ferrocad_shell_web::page_query();
    let Some(code) = ferrocad_shell_web::parameter(&query, "script") else {
        return;
    };
    match ferrocad_shell_core::app::evaluate(cx, &code) {
        Ok(result) => log::info!(
            "ferrocad probe: script -> ok={} output={:?}",
            result.ok,
            result.output
        ),
        Err(error) => log::error!("ferrocad probe: script -> {error}"),
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
