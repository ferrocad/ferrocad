//! Enable the embedded-payload code paths when the payload is present *and* the
//! `embedded-payload` feature is on.
//!
//! `cargo install` installs a binary, not data files, so the app must carry its
//! Python facade and workbenches with it; there is no way to ship them *beside*
//! the executable through crates.io. The publish staging step (see `xtask`) and
//! the resulting tarball transiently copy the workspace's `python/` and `mods/`
//! in beside this manifest. A normal development checkout has neither, and
//! `xtask bundle` builds with `--no-default-features`, so both cases stay
//! un-embedded and the app discovers a loose payload instead.
//!
//! Two independent flags keep the payloads separable: a Python-only (tiny)
//! payload does not force the multi-megabyte workbenches on.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Watch the payload dirs unconditionally, so staging or removing them
    // between builds re-runs this script and flips the cfgs.
    println!("cargo:rerun-if-changed=python");
    println!("cargo:rerun-if-changed=mods");
    // Declare the custom cfgs so the lint does not fire when they are unset.
    println!("cargo:rustc-check-cfg=cfg(have_embedded_python)");
    println!("cargo:rustc-check-cfg=cfg(have_embedded_mods)");

    // Installers (`xtask bundle`) pass `--no-default-features`: they ship the
    // same files loose next to the binary, and embedding them would duplicate
    // them for no benefit.
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_PAYLOAD").is_none() {
        return;
    }

    emit_if_present("python", "have_embedded_python");
    emit_if_present("mods", "have_embedded_mods");
}

/// Set `cfg` when `dir` exists beside the manifest.
fn emit_if_present(dir: &str, cfg: &str) {
    if std::path::Path::new(dir).is_dir() {
        println!("cargo:rustc-cfg={cfg}");
    }
}
