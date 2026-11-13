//! `mods/AxisOverlayJs` on the desktop page: QuickJS, the JSON kernel, no browser.
//!
//! This is the JS analogue of `tests/test_axis_overlay_coin.py`: the mod is the real
//! one shipped in `mods/`, the page is a real QuickJS engine, and the assertion is on
//! the scene the host registered — one program packet built from the **shared** axis
//! policy — not on a GPU frame.
//!
//! The backend is a renderer, not a settings store: it advertises no command and no
//! schema, and it renders the `ViewState` the shell pushes. That is what makes
//! `Settings/Axis` one form and the axis switch one switch, whichever backend is loaded.

#![cfg(feature = "desktop")]

use std::path::Path;
use std::sync::Arc;

use ferrocad_inventor::overlays;
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::view::ViewState;
use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_js::JsScriptHost;
use ferrocad_shell_js::desktop::EnginePage;

/// The mod's entry file, from this crate's directory.
fn axis_package() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/AxisOverlayJs/InitGui.js")
}

/// The JS workbenches the desktop scanner finds, in its name order. Loading the shipped
/// neighbours too is the point: they merge into `window.__ferrocadJs` after this mod, so
/// its hooks have to survive a later `install()`.
fn packages() -> Vec<std::path::PathBuf> {
    ["AxisOverlayJs", "Explode", "Fasteners"]
        .iter()
        .map(|name| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../mods")
                .join(name)
                .join("InitGui.js")
        })
        .collect()
}

/// The mod never touches the document; a call is only a capability check.
struct NoDocument;

impl DocumentApi for NoDocument {
    fn call(&self, _request: &str) -> String {
        r#"{"ok":true}"#.to_string()
    }
}

#[test]
fn a_dialog_this_mod_does_not_own_reaches_the_previous_hook() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let first = temp.path().join("First.js");
    std::fs::write(
        &first,
        r#"
export function install() {
  window.__ferrocadJs = {
    active: () => true,
    commands: () => JSON.stringify([]),
    evaluate: (code) => code,
    runCommand: () => true,
    stageMeshes: () => "[]",
    onDialogAccept: (token, _values) =>
      JSON.stringify({ ok: true, status: `first:${token}` }),
    onDialogCancel: (token) =>
      JSON.stringify({ ok: true, status: `cancel:${token}` }),
  };
}
"#,
    )
    .expect("the earlier package");

    let page = EnginePage::new(vec![first, axis_package()]).expect("both boot");
    let host = JsScriptHost::new(Arc::new(page));

    // Our mod owns no dialog, so the accept belongs to the package installed first.
    let accepted = host
        .dialog_accept("other", &serde_json::json!({}))
        .expect("the page answers");
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(accepted.status, "first:other");

    let cancelled = host.dialog_cancel("other").expect("the page answers");
    assert_eq!(cancelled.status, "cancel:other");
}

#[test]
fn the_js_axis_cross_renders_the_shared_axis_policy() {
    overlays::clear();
    let page = Arc::new(EnginePage::new(packages()).expect("the mods boot"));
    let host = JsScriptHost::with_kernel(page, Arc::new(NoDocument));

    // This backend registers no commands and no settings form: the shell's one
    // `App_ToggleAxisCross` and `Settings/Axis` own those.
    assert!(
        !host
            .commands()
            .iter()
            .any(|command| command.id.starts_with("AxisOverlayJs")),
        "a renderer is not a second settings surface"
    );

    // The page's catalog is exactly the one implementation it owns. The native and
    // Coin backends are the Python host's entries; `HostChain` merges them.
    let axis = host
        .provider_features()
        .into_iter()
        .find(|feature| feature.feature == "axis")
        .expect("the axis registry");
    assert_eq!(axis.providers.len(), 1);
    assert_eq!(axis.providers[0].backend, "js");
    assert!(!axis.providers[0].active, "nothing is loaded at boot");
    assert!(
        axis.providers[0].command.is_none(),
        "the shared form is the settings surface"
    );

    // The chain's load verb registers the overlay and claims the native cross.
    let loaded = host.set_provider("axis", "js").expect("the page answers");
    assert!(loaded.ok, "{loaded:?}");
    assert_eq!(
        loaded.view.expect("a claim").axis.unwrap().overlay,
        Some(true)
    );
    assert_eq!(overlays::named_keys(), ["axis-overlay-js"]);

    // The shared policy arrives as a view state; the overlay renders exactly those
    // values, so every backend reads the same store.
    let mut state = ViewState::default();
    state.axis.enabled = true;
    state.axis.length = 120.0;
    state.axis.width = 2.5;
    state.axis.opacity = 0.5;
    state.axis.colors[0] = [1.0, 0.25, 0.5, 1.0];
    host.on_view_state_changed(&state);

    let scenes = overlays::take_scenes();
    assert_eq!(scenes.len(), 1, "one registered overlay");
    let (queue, _) = ferrocad_inventor::compile(&scenes[0]);
    assert_eq!(queue.len(), 1, "one program packet");
    let program = queue.packets[0].state.program.as_ref().expect("a program");
    let parameters = program.parameters();
    assert_eq!(parameters.color, [1.0, 0.25, 0.5], "the shared X ink");
    assert_eq!(parameters.axis_length, 120.0);
    assert_eq!(parameters.axis_width, 2.5);
    assert_eq!(parameters.axis_opacity, 0.5);

    // The WGSL the page supplied parses and validates, without a GPU: the same check
    // the renderer runs when it compiles the program.
    let module =
        naga::front::wgsl::parse_str(program.source()).expect("the JS shader parses as WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("the JS shader validates");

    // The one shared switch controls this overlay too: turning the axes off removes it,
    // turning them back on restores it. No provider switch is involved.
    state.axis.enabled = false;
    host.on_view_state_changed(&state);
    assert!(
        overlays::named_keys().is_empty(),
        "the shared switch hides the JS overlay"
    );
    state.axis.enabled = true;
    host.on_view_state_changed(&state);
    assert_eq!(overlays::named_keys(), ["axis-overlay-js"]);

    let summary = host.axis_summary().expect("the JS summary");
    assert_eq!(summary.backend.as_deref(), Some("js"));
    assert_eq!(summary.label.as_deref(), Some("Axis Cross (JS)"));
    assert_eq!(summary.length.as_deref(), Some("120 px"));
    assert_eq!(summary.width.as_deref(), Some("2.5 px"));

    // The refresh verb restates the claim for the loaded backend.
    let refreshed = host
        .refresh_providers("axis")
        .expect("the registry answers");
    assert!(refreshed.ok, "{refreshed:?}");
    assert_eq!(
        refreshed.view.expect("a claim").axis.unwrap().overlay,
        Some(true)
    );

    // Switching to a backend this page does not implement is not its business; the
    // chain routes it to the Python host.
    assert!(host.set_provider("axis", "ferrocad").is_none());

    // The chain's unload verb removes the overlay and reports the page resting.
    let released = host.release_provider("axis").expect("the registry answers");
    assert!(released.ok, "{released:?}");
    assert_eq!(
        released.view.expect("a claim").axis.unwrap().overlay,
        Some(false)
    );
    assert!(overlays::named_keys().is_empty(), "the release removed it");
    let axis = host
        .provider_features()
        .into_iter()
        .find(|feature| feature.feature == "axis")
        .expect("the axis registry");
    assert!(!axis.providers[0].active, "the slot is empty again");

    // Dropping the host drops the overlays it owns: one page, one set of scenes.
    drop(host);
    assert!(overlays::named_keys().is_empty());
}
