//! Locating (and, with network, fetching) the OCCT 7.8.1 prefix.
//!
//! The same prefix compiles the edition (`OpenCASCADE_DIR`/`OCCT_INCLUDE_DIR`) and
//! is walked by `bundle --occt`, so [`resolve`] is the one source of truth. A
//! fetched prefix lives at `.occt/prefix` (the recipe in
//! `docs/developer/architecture/occt-bundling.md`); `target/occt-runtime/<triple>/prefix.txt`
//! records the prefix `xtask occt` last resolved so a later `bundle --offline`
//! finds it without repeating the search.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The OCCT release the workspace pins (`docs/developer/architecture/occt-bundling.md`).
pub const PINNED_VERSION: &str = "7.8.1";

/// `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, …
pub fn triple() -> String {
    format!(
        "{}-{}",
        std::env::consts::ARCH,
        match std::env::consts::OS {
            "linux" => "unknown-linux-gnu",
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            other => other,
        }
    )
}

/// Where `xtask occt` records the resolved prefix.
pub fn marker_path(root: &Path) -> PathBuf {
    root.join("target")
        .join("occt-runtime")
        .join(triple())
        .join("prefix.txt")
}

/// The directory holding the shared libraries (plural because a conda-forge
/// Windows prefix nests OCCT under `Library/`).
pub fn lib_dirs(prefix: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        dirs.push(prefix.join("Library").join("bin"));
        dirs.push(prefix.join("Library").join("lib"));
        dirs.push(prefix.join("bin"));
        dirs.push(prefix.join("lib"));
    } else {
        dirs.push(prefix.join("lib"));
        dirs.push(prefix.join("lib64"));
    }
    dirs
}

/// The include directory that holds `Standard_Version.hxx`.
pub fn include_dir(prefix: &Path) -> Option<PathBuf> {
    [
        prefix.join("include").join("opencascade"),
        prefix.join("Library").join("include").join("opencascade"),
        prefix.join("include"),
    ]
    .into_iter()
    .find(|candidate| candidate.join("Standard_Version.hxx").is_file())
}

/// `7.8.1`, read from the prefix's `Standard_Version.hxx`.
pub fn version(prefix: &Path) -> Option<String> {
    let include = include_dir(prefix)?;
    let text = fs::read_to_string(include.join("Standard_Version.hxx")).ok()?;
    let mut parts = [None; 3];
    let fields = [
        ("OCC_VERSION_MAJOR", 0),
        ("OCC_VERSION_MINOR", 1),
        ("OCC_VERSION_MAINTENANCE", 2),
    ];
    for line in text.lines() {
        let line = line.trim();
        for (name, index) in &fields {
            if let Some(rest) = line.strip_prefix(&format!("#define {name}")) {
                parts[*index] = parts[*index].or_else(|| rest.trim().parse::<u32>().ok());
            }
        }
    }
    match parts {
        [Some(major), Some(minor), Some(patch)] => Some(format!("{major}.{minor}.{patch}")),
        _ => None,
    }
}

/// OCCT's license text and linking exception, as `(source, destination name)`.
///
/// `bundle --occt` copies these under `LICENSES/` (the exception is part of the
/// license, so it ships beside the text).
pub fn license_files(prefix: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut found = Vec::new();
    for base in [
        prefix.join("share").join("doc").join("opencascade"),
        prefix
            .join("Library")
            .join("share")
            .join("doc")
            .join("opencascade"),
        prefix.join("share").join("licenses").join("occt"),
    ] {
        for (name, destination) in [
            ("LICENSE_LGPL_21.txt", "OCCT-LGPL-2.1.txt"),
            ("OCCT_LGPL_EXCEPTION.txt", "OCCT-LGPL-EXCEPTION.txt"),
        ] {
            let source = base.join(name);
            if source.is_file() && !found.iter().any(|(_, d)| *d == destination) {
                found.push((source, destination));
            }
        }
    }
    found
}

/// Resolve an OCCT prefix.
///
/// Order: an explicit `--occt-prefix`, the environment
/// (`FERROCAD_OCCT_PREFIX`, `OCCT_ROOT`, `CASROOT`, `OpenCASCADE_DIR`), the
/// workspace's fetched `.occt/prefix`, then the marker `xtask occt` wrote. With
/// `offline` the search stops there and reports every location it tried; without
/// it, a missing prefix is fetched with micromamba (the conda-forge recipe).
pub fn resolve(root: &Path, explicit: Option<&Path>, offline: bool) -> Result<PathBuf, String> {
    // An explicit `--occt-prefix` is a decision, not a hint: report a bad path
    // instead of falling through to the workspace prefix (which would silently
    // bundle a different OCCT than the caller asked for).
    if let Some(path) = explicit {
        return validate(path).map_err(|reason| {
            format!(
                "--occt-prefix {} is not a usable OCCT {PINNED_VERSION} prefix: {reason}",
                path.display()
            )
        });
    }

    let mut tried = Vec::new();
    let mut candidates = Vec::new();
    for variable in ["FERROCAD_OCCT_PREFIX", "OCCT_ROOT", "CASROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Some(value) = std::env::var_os("OpenCASCADE_DIR") {
        let cmake = PathBuf::from(value);
        // `.occt/prefix/lib/cmake/opencascade` -> `.occt/prefix`.
        let prefix = if cmake.ends_with("lib/cmake/opencascade") {
            cmake
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or(cmake)
        } else {
            cmake
        };
        candidates.push(prefix);
    }
    candidates.push(root.join(".occt").join("prefix"));
    if let Ok(marker) = fs::read_to_string(marker_path(root)) {
        let path = PathBuf::from(marker.trim());
        if !path.as_os_str().is_empty() {
            candidates.push(path);
        }
    }

    for candidate in candidates {
        match validate(&candidate) {
            Ok(prefix) => return Ok(prefix),
            Err(reason) => tried.push(format!("{} ({reason})", candidate.display())),
        }
    }

    if offline {
        return Err(format!(
            "no OCCT {PINNED_VERSION} prefix found; tried:\n  {}\n\
             Pass --occt-prefix DIR, set FERROCAD_OCCT_PREFIX, or fetch one with \
             `cargo xtask occt` (needs network).",
            tried.join("\n  ")
        ));
    }

    eprintln!("== fetching OCCT {PINNED_VERSION} (conda-forge) ==");
    fetch(root)?;
    validate(&root.join(".occt").join("prefix")).map_err(|reason| {
        format!(
            "fetched {}, but it is not a usable OCCT prefix: {reason}",
            root.join(".occt/prefix").display()
        )
    })
}

/// A usable prefix has a library directory and `Standard_Version.hxx`.
pub fn validate(prefix: &Path) -> Result<PathBuf, String> {
    if !prefix.is_dir() {
        return Err("no such directory".to_string());
    }
    if !lib_dirs(prefix).iter().any(|dir| dir.is_dir()) {
        return Err("no lib/ directory".to_string());
    }
    if include_dir(prefix).is_none() {
        return Err("no include/opencascade/Standard_Version.hxx".to_string());
    }
    Ok(prefix
        .canonicalize()
        .unwrap_or_else(|_| prefix.to_path_buf()))
}

/// Fetch the pinned conda-forge build into `.occt/prefix` with micromamba.
///
/// This mirrors `docs/developer/architecture/occt-bundling.md` §3.1. It needs network and
/// `curl`/`tar`; `bundle --offline` never calls it. The release workflow may fetch
/// the prefix itself and pass `--occt-prefix` instead.
fn fetch(root: &Path) -> Result<(), String> {
    let base = root.join(".occt");
    let download = base.join("micromamba.tar.bz2");
    let extract = base.join("fetch");
    let prefix = base.join("prefix");
    fs::create_dir_all(&extract).map_err(|error| error.to_string())?;

    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "x86_64") => "osx-64",
        ("macos", "aarch64") => "osx-arm64",
        ("windows", "x86_64") => "win-64",
        (os, arch) => {
            return Err(format!(
                "no micromamba build known for {os}/{arch}; fetch OCCT yourself and pass --occt-prefix"
            ));
        }
    };

    let url = format!("https://micro.mamba.pm/api/micromamba/{platform}/latest");
    run(
        Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&download)
            .arg(&url),
        "download micromamba",
    )?;
    run(
        Command::new("tar")
            .arg("-xjf")
            .arg(&download)
            .arg("-C")
            .arg(&extract),
        "unpack micromamba",
    )?;

    let mamba = find_file(&extract, "micromamba")
        .or_else(|| find_file(&extract, "micromamba.exe"))
        .ok_or_else(|| format!("micromamba was not found under {}", extract.display()))?;
    let version =
        std::env::var("FERROCAD_OCCT_VERSION").unwrap_or_else(|_| PINNED_VERSION.to_string());
    let package = format!("occt={version}");
    run(
        Command::new(&mamba)
            .args(["create", "-y", "-p"])
            .arg(&prefix)
            .args(["-c", "conda-forge", package.as_str()])
            .env("MAMBA_ROOT_PREFIX", base.join("root")),
        "micromamba create",
    )
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            return Some(path);
        }
        let found = if path.is_dir() {
            find_file(&path, name)
        } else {
            None
        };
        if let Some(found) = found {
            return Some(found);
        }
    }
    None
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("failed to run {what}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed"))
    }
}
