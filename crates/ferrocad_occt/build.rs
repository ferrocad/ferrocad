//! Compile the sibling cxx bridge over `opencascade-sys`'s types.
//!
//! The bridge reuses the crate's opaque OCCT types
//! (`type TopoDS_Shape = opencascade_sys::...`) so we add the missing history /
//! element-map calls without patching or forking `opencascade-sys`.
//!
//! The include directory is `OCCT_INCLUDE_DIR` when set. Otherwise this script
//! discovers it from `OCCT_ROOT`/`CASROOT`/`OpenCASCADE_DIR`, the workspace's
//! `.occt/prefix` (the prefix CI fetches), or a common system prefix. See
//! docs/architecture/occt-bundling.md.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    for var in [
        "OCCT_INCLUDE_DIR",
        "OCCT_ROOT",
        "CASROOT",
        "OpenCASCADE_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    for file in [
        "include/fc_history.hxx",
        "include/fc_topology.hxx",
        "include/fc_curve.hxx",
        "include/fc_geom2d.hxx",
        "include/fc_shapefix.hxx",
        "include/fc_chfi2d.hxx",
        "include/fc_surface_ops.hxx",
        "include/fc_shape_ops.hxx",
        "src/fc_part_ops.hxx",
        "src/bridge.rs",
        "src/shapefix.rs",
        "src/chfi2d.rs",
        "src/surface_ops.rs",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }

    let occt_include = find_include().unwrap_or_else(|| {
        panic!(
            "could not find the OCCT headers. Set OCCT_INCLUDE_DIR to the OCCT \
             'include/opencascade' directory (e.g. <prefix>/include/opencascade), or \
             put an OCCT 7.8 prefix at .occt/prefix (the CI recipe is in \
             docs/architecture/occt-bundling.md)."
        )
    });

    // The version string the Python surface reports as `Part.OCC_VERSION` is read
    // from the headers the bridge is compiled against, so it can never drift from
    // the kernel actually linked in.
    let version = occt_version(&occt_include).unwrap_or_else(|| {
        panic!(
            "{} has no OCC_VERSION_COMPLETE define",
            occt_include.join("Standard_Version.hxx").display()
        )
    });
    println!("cargo:rustc-env=FERROCAD_OCC_VERSION={version}");

    cxx_build::bridges(["src/bridge.rs", "src/shapefix.rs", "src/chfi2d.rs", "src/surface_ops.rs"])
        .include(&occt_include)
        .std("c++14")
        .compile("ferrocad-occt-bridge");
}

/// Find the directory that contains `Standard_Version.hxx`.
fn find_include() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("OCCT_INCLUDE_DIR") {
        if let Some(found) = normalize(PathBuf::from(value)) {
            return Some(found);
        }
        println!(
            "cargo:warning=OCCT_INCLUDE_DIR does not contain Standard_Version.hxx; \
             searching the known prefixes"
        );
    }

    for var in ["OCCT_ROOT", "CASROOT", "OpenCASCADE_DIR"] {
        if let Ok(value) = std::env::var(var) {
            if let Some(found) = under(Path::new(&value)) {
                return Some(found);
            }
        }
    }

    // The workspace-local prefix CI fetches. `CARGO_MANIFEST_DIR` is
    // `<workspace>/crates/ferrocad_occt`, so its grandparent is the workspace.
    if let Some(workspace) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        if let Some(found) = normalize(workspace.join(".occt/prefix")) {
            return Some(found);
        }
    }
    // Also accept `.occt/prefix` when cargo is invoked from a subdirectory.
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            if let Some(found) = normalize(ancestor.join(".occt/prefix")) {
                return Some(found);
            }
        }
    }

    // System packages (apt, brew, macports) and a few conventional roots.
    for prefix in ["/usr", "/usr/local", "/opt/homebrew", "/opt/local"] {
        if let Some(found) = normalize(PathBuf::from(prefix)) {
            return Some(found);
        }
    }
    None
}

/// A path that is a prefix, an include directory, or a CMake config directory.
fn under(path: &Path) -> Option<PathBuf> {
    if let Some(found) = normalize(path.to_path_buf()) {
        return Some(found);
    }
    for ancestor in path.ancestors() {
        if let Some(found) = normalize(ancestor.to_path_buf()) {
            return Some(found);
        }
    }
    None
}

/// Normalize `path` to a directory containing `Standard_Version.hxx`.
///
/// Accepts the include directory itself, the `include/` parent, or a prefix with
/// `include/opencascade` below it.
fn normalize(path: PathBuf) -> Option<PathBuf> {
    let candidates = [
        path.clone(),
        path.join("opencascade"),
        path.join("include").join("opencascade"),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.join("Standard_Version.hxx").is_file())
}

/// The `OCC_VERSION_COMPLETE` string (`"7.8.1"`) from `Standard_Version.hxx`.
fn occt_version(include: &Path) -> Option<String> {
    let header = fs::read_to_string(include.join("Standard_Version.hxx")).ok()?;
    for line in header.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("#define OCC_VERSION_COMPLETE") else {
            continue;
        };
        let value = rest.trim();
        return value
            .strip_prefix('"')
            .and_then(|value| value.split('"').next())
            .map(str::to_string);
    }
    None
}
