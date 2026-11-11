//! `--addons <package>` selects the mod set: every mod by default, exactly those when
//! named.
//!
//! The flag used to narrow only the wasm bundles: the runtime derivation and
//! `load_workbenches` always scanned the whole `mods/` directory, so `--addons
//! mods/GridOverlay` still loaded every Python workbench — and a *directory of packages*
//! was the only spelling that made sense, because a package directory produced "no wasm
//! addons under …". A mod set is one selection for all three consumers, which is what this
//! pins.
//!
//! Its own binary: `python::configure` sets the mod paths once per process.

use std::sync::Once;

use ferrocad_shell_api::EntryKind;
use ferrocad_shell_api::runtime::Runtime;
use ferrocad_shell_python::{mods, python};

static BOOT: Once = Once::new();

/// The one package this boot selects.
fn grid_overlay() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
        .join("GridOverlay")
}

fn configured() -> std::path::PathBuf {
    let package = grid_overlay();
    BOOT.call_once(|| {
        ferrocad_combined_py::register_builtins();
        python::configure("ferrocad_parametric", &[], std::slice::from_ref(&package), true);
    });
    package
}

#[test]
fn a_package_directory_selects_that_mod_alone() {
    let package = configured();

    // 1. The derivation sees one Python mod — not the JavaScript half the whole directory
    //    asks for (`mods/Explode` and `mods/Fasteners` ship `InitGui.js`). That is the
    //    whole point: a selection must not install a host for a mod it did not select.
    let kinds = mods::entry_kinds(std::slice::from_ref(&package));
    assert_eq!(kinds, [EntryKind::Python], "one mod, one language");

    // And the same set derives one host.
    assert_eq!(
        Runtime::hosts_for(&kinds, ferrocad_shell_api::runtime::Platform::Desktop),
        [Runtime::Cpython]
    );

    // 2. The loader scans the selection, not the directory it sits in.
    let mut backend = ferrocad_shell_python::backend::compose_all(&[Runtime::Cpython], &[])
        .expect("the interpreter composes");
    backend.bootstrap().expect("the composed backend boots");
    let report = backend.load_workbenches().expect("the loader answers");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.loaded,
        ["GridOverlay/Init.py", "GridOverlay/InitGui.py"],
        "only the selected package"
    );

    // 3. Its provider registered, which is what the header reads.
    let features = backend.provider_features().expect("the registry answers");
    assert!(
        features
            .iter()
            .any(|feature| feature.feature == "grid"
                && feature.providers.iter().any(|provider| provider.backend == "ferrocad")),
        "GridOverlay registered the native grid provider: {features:?}"
    );
}
