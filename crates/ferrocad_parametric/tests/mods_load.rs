//! The desktop boot loads the workspace's `mods/`.
//!
//! The shell's contract is "load workbenches best-effort": a broken mod is reported in
//! the console and never fatal. That makes a *silent* "no mods" possible — one wrong
//! discovery path and the window opens with the base app only — so this boots the
//! composed backend exactly as `ferrocad_parametric` does (`gui = true`, no explicit
//! paths, so `python::mods_dirs()` discovers the workspace `mods/`) and asserts three
//! things a user would notice: the scripts loaded, the interpreter named no errors, and
//! the mods' effects reached the backend the window asks — their commands in the
//! palette and their provider in the header's grid summary.
//!
//! It is its own test binary on purpose. Loading mods mutates the process-wide
//! interpreter (providers, registered commands), and `composition.rs` asserts on a
//! registry with none of them.
//!
//! `crates/ferrocad_toy/tests/grid_overlay_mods.rs` covers the same discovery for the toy
//! edition through `python::` directly; this one is the edition's own composition.

use std::sync::Once;

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_api::runtime::Runtime;
use ferrocad_shell_python::python;

static BOOT: Once = Once::new();

/// The mods directory beside the workspace's `python/` tree.
fn mods_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
}

/// The composed backend, configured the way `ferrocad_parametric` boots it.
///
/// No explicit paths: discovery is the thing under test, so `python/` and `mods/` must
/// be found from the working directory or nothing here means anything. The interpreter
/// is started with `gui = true` because a mod's `InitGui.py` is what registers its
/// commands and providers, and that is the half a window sees.
///
/// The runtime list is what the default desktop composition installs: the base app's
/// default features include `js`, so `mods/Explode`, `mods/Fasteners` and
/// `mods/AxisOverlayJs` (which ship `InitGui.js`) get the QuickJS page beside CPython.
/// The composition has to install the interpreter regardless — the regression this
/// guards installed no script half at all, so no workbench loaded and the header listed
/// only the document half's provider.
fn composed() -> Box<dyn AppBackend> {
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], &[], true);
    });
    let mut backend = ferrocad_shell_python::compose_all(&[Runtime::Cpython, Runtime::Js], &[])
        .expect("the composition keeps the halves this build has");
    backend.bootstrap().expect("the composed backend boots");
    backend
}

#[test]
fn the_desktop_boot_loads_the_workspace_mods() {
    let backend = composed();
    // The bug's signature: no script half means no `InitGui.py`, hence only the document
    // half's provider in the header.
    assert!(
        backend.script_host().is_some(),
        "the interpreter is installed even when the declared js half cannot be"
    );

    // 1. The scripts. Empty `loaded` is the "no mods" the user sees, so assert on the
    //    Python workbenches by name rather than on a count that a new package shifts.
    let report = backend.load_workbenches().expect("the loader answers");
    assert!(
        report.errors.is_empty(),
        "a mod failed to import: {:?}",
        report.errors
    );
    for script in [
        "GridOverlay/InitGui.py",
        "GridOverlayCoin/InitGui.py",
        "AxisOverlay/InitGui.py",
        "GizmoOverlay/InitGui.py",
    ] {
        assert!(
            report.loaded.iter().any(|loaded| loaded == script),
            "{script} is missing from {:?}",
            report.loaded
        );
    }

    // 2. Their commands. A mod's settings command is registered by its `InitGui`, and
    //    the palette reads `commands()`, so this is the same list the window renders.
    let commands = backend.commands().expect("the palette asks the backend");
    let ids: Vec<&str> = commands.iter().map(|row| row.id.as_str()).collect();
    for id in [
        "GridOverlay_Settings",
        "GridOverlayCoin_Settings",
        "AxisOverlay_Settings",
        // The built-in (B13c) is beside them, because the app script is installed.
        "View_GridSettings",
    ] {
        assert!(ids.contains(&id), "{id} is missing from {ids:?}");
    }

    // 3. Their provider. `grid_summary` is what the header's grid popover prints, so a
    //    registered provider that never reached the summary is the quiet version of
    //    "the mod did not load". Both mods register one; the Coin backend outranks the
    //    native one by priority, so exactly one is active and it is not always the
    //    native one — asserting "native is active" would pin the wrong thing.
    let summary = backend.grid_summary().expect("the summary answers");
    assert_eq!(summary.feature, "grid");
    let backends: Vec<&str> = summary
        .providers
        .iter()
        .map(|provider| provider.backend.as_str())
        .collect();
    assert!(
        backends.contains(&"ferrocad") && backends.contains(&"coin"),
        "both grid mods registered a provider: {summary:?}"
    );
    assert_eq!(
        summary
            .providers
            .iter()
            .filter(|provider| provider.active)
            .count(),
        1,
        "one active backend: {summary:?}"
    );

    // The document the script half made is the one the document half shows, and the
    // settings node a provider registered is listed once.
    let tree = backend.model_tree().expect("the tree answers");
    let settings: Vec<_> = tree.iter().filter(|node| node.name == "Settings").collect();
    assert_eq!(settings.len(), 1, "one Settings root: {tree:?}");
    assert!(
        settings[0]
            .objects
            .iter()
            .any(|object| object.name.eq_ignore_ascii_case("Settings/grid")),
        "the registered feature is a Settings node: {:?}",
        settings[0]
    );

    // The mods are on this machine, not in an embedded payload.
    assert!(
        mods_dir().is_dir(),
        "the workspace mods/ directory is what discovery found"
    );
}
