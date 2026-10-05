//! Locating (and, when needed, unpacking) the app's Python payload.
//!
//! FerroCAD boots an embedded CPython interpreter against a `FreeCAD` facade and
//! a set of workbench (`mods/`) scripts. In a development checkout or an
//! installer those live on disk; for `cargo install` they do not, because Cargo
//! installs a binary and nothing else. This module bridges that gap: the payload
//! is baked into the binary at compile time (`build.rs` + `include_dir!`) and,
//! the first time no on-disk copy is found, unpacked into the per-user data
//! directory and handed to the shell.
//!
//! Resolution is deliberately shared with [`ferrocad_gpui::python`]:
//!
//! | Order | Source | Handled by |
//! | ----- | ------ | ---------- |
//! | 1 | `FERROCAD_PYTHON_PATH` / `FERROCAD_MODS_PATH` | `find_python_dir` |
//! | 2 | loose payload beside the executable | `find_python_dir` |
//! | 3 | development checkout (walk up from cwd) | `find_python_dir` |
//! | 4 | embedded payload, extracted to the data dir | this module |
//!
//! [`resolve`] only contributes case 4: it asks the shell whether cases 1-3
//! already apply, and extracts the embedded copy only when they do not. The
//! extracted directory is versioned, so upgrading the app never reuses a stale
//! payload. Set `FERROCAD_DATA_DIR` to redirect it (used by tests and portable
//! distributions).

use std::path::PathBuf;

/// The `python/` and `mods/` directories the shell should boot from.
///
/// Empty vectors mean "the shell already discovers the payload itself" (cases
/// 1-3 above); non-empty values are an explicit, extracted payload.
#[derive(Debug, Default, Clone)]
pub struct Payload {
    /// Directories prepended to `sys.path` (the facade directory).
    pub python_paths: Vec<PathBuf>,
    /// Workbench directories scanned at boot. Empty means "discover".
    pub mods_paths: Vec<PathBuf>,
}

/// Resolve the payload for this run.
///
/// Returns empty vectors when the shell can find a payload on its own, so the
/// development and installer paths are untouched.
pub fn resolve() -> Payload {
    // Cases 1-3: anything the shell can already find wins. The embedded copy is
    // a last resort, never a preference (a developer editing `python/` must see
    // their edits, not a stale extraction).
    if ferrocad_gpui::python::find_python_dir().is_ok() {
        return Payload::default();
    }
    #[cfg(have_embedded_python)]
    if let Some(payload) = extract_embedded() {
        return payload;
    }
    // Nothing on disk and nothing embedded: let the shell report the error with
    // its usual message.
    Payload::default()
}

/// Is `dir` a facade directory, i.e. does it contain `FreeCAD/__init__.py`?
#[cfg(have_embedded_python)]
fn is_facade(dir: &std::path::Path) -> bool {
    dir.join("FreeCAD").join("__init__.py").is_file()
}

/// The per-user directory under which an embedded payload is unpacked.
///
/// Overridable with `FERROCAD_DATA_DIR`. Follows the platform convention:
/// `%APPDATA%` on Windows, `Application Support` on macOS, `XDG_DATA_HOME`
/// (or `~/.local/share`) elsewhere.
#[cfg(have_embedded_python)]
fn data_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FERROCAD_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("ferrocad"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/ferrocad"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("ferrocad"))
    }
}

/// Extract the embedded payload into the data directory, if it is not already
/// there, and return the paths to use. Returns `None` if extraction fails, so
/// the caller can fall back to the shell's own discovery.
#[cfg(have_embedded_python)]
fn extract_embedded() -> Option<Payload> {
    let root = data_root()?.join("payload").join(env!("CARGO_PKG_VERSION"));
    let python = root.join("python");
    let mods = root.join("mods");
    let marker = root.join(".complete");

    if !marker.is_file() {
        // Fresh extraction, or a previous run that was interrupted. `extract`
        // fails if a destination file already exists, so start from a clean
        // tree; the marker is only written once every file has landed.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).ok()?;
        embedded_python().extract(&python).ok()?;
        #[cfg(have_embedded_mods)]
        embedded_mods().extract(&mods).ok()?;
        std::fs::write(&marker, b"").ok()?;
    }

    if !is_facade(&python) {
        return None;
    }
    let mut payload = Payload {
        python_paths: vec![python],
        ..Payload::default()
    };
    // Only point the shell at `mods/` if it was actually embedded; otherwise let
    // it discover a loose one.
    if mods.is_dir() {
        payload.mods_paths = vec![mods];
    }
    Some(payload)
}

/// The `python/` facade, embedded at compile time.
#[cfg(have_embedded_python)]
fn embedded_python() -> &'static include_dir::Dir<'static> {
    static DIR: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/python");
    &DIR
}

/// The `mods/` workbenches, embedded at compile time.
#[cfg(have_embedded_mods)]
fn embedded_mods() -> &'static include_dir::Dir<'static> {
    static DIR: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/mods");
    &DIR
}
