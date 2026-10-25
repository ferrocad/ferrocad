//! The shared-library closure of a built edition binary.
//!
//! OCCT ships no `RUNPATH`, so a packaged app must carry every OCCT toolkit the
//! binary actually references. The seed is the binary's own import table (not the
//! toolkit list in `opencascade-sys`'s `build.rs`): the linker keeps only what the
//! app uses. Linux walks `DT_NEEDED` with `readelf -d`; Windows reads the PE
//! import directory with [`crate::pe`]. macOS is deliberately blocked until the
//! upstream GPUI fork works (see `docs/architecture/edition-publishing.md`).
//!
//! Only libraries that resolve inside the OCCT prefix are copied: system
//! libraries (libc, libm, …) stay the host's responsibility, and the bundled
//! CPython runtime is already copied separately.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{occt, pe};

/// One bundled shared library.
#[derive(Debug)]
pub struct StagedLibrary {
    /// The exact soname (or DLL filename) the loader asks for.
    pub name: String,
    /// Where it came from (recorded in `MANIFEST`).
    pub source: PathBuf,
    pub size: u64,
}

#[derive(Debug)]
pub struct Closure {
    pub libraries: Vec<StagedLibrary>,
}

/// Walk `binary` and copy each prefix-owned dependency into `dest`.
///
/// `dest` is `payload/lib` on Linux/macOS and `payload/bin` on Windows (the DLLs
/// sit beside `ferrocad.exe` so the default search order finds them before a
/// system copy). `runtime_dirs` are searched, but never copied: that tree is
/// already part of the payload.
pub fn stage(
    binary: &Path,
    dest: &Path,
    prefix: &Path,
    runtime_dirs: &[PathBuf],
) -> Result<Closure, String> {
    if !binary.is_file() {
        return Err(format!(
            "no built binary at {}; build the edition first",
            binary.display()
        ));
    }
    fs::create_dir_all(dest).map_err(|error| format!("create {}: {error}", dest.display()))?;

    if cfg!(windows) {
        stage_pe(binary, dest, prefix, runtime_dirs)
    } else if cfg!(target_os = "macos") {
        Err(
            "the macOS closure is blocked on the pinned bite-gp-apple 1.24.402 \
             (it uses `cocoa` without declaring it); see \
             docs/architecture/edition-publishing.md. Build the Linux/Windows \
             editions meanwhile."
                .to_string(),
        )
    } else {
        stage_elf(binary, dest, prefix, runtime_dirs)
    }
}

/// Write a deterministic `MANIFEST` listing what was bundled.
pub fn write_manifest(path: &Path, closure: &Closure) -> Result<(), String> {
    let mut libraries: Vec<&StagedLibrary> = closure.libraries.iter().collect();
    libraries.sort_by(|left, right| left.name.cmp(&right.name));

    let mut text = String::from("# FerroCAD bundled shared libraries\n");
    text.push_str("# soname\tbytes\tbuilt-from\n");
    for library in libraries {
        text.push_str(&format!(
            "{}\t{}\t{}\n",
            library.name,
            library.size,
            library.source.display()
        ));
    }
    fs::write(path, text).map_err(|error| format!("write {}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// Linux / ELF
// ---------------------------------------------------------------------------

fn stage_elf(
    binary: &Path,
    dest: &Path,
    prefix: &Path,
    runtime_dirs: &[PathBuf],
) -> Result<Closure, String> {
    let prefix = prefix
        .canonicalize()
        .unwrap_or_else(|_| prefix.to_path_buf());
    let mut search = vec![dest.to_path_buf()];
    search.extend(occt::lib_dirs(prefix.as_path()));
    search.extend(runtime_dirs.iter().cloned());

    let mut queue: VecDeque<(String, PathBuf)> = elf_needed(binary)?
        .into_iter()
        .map(|name| (name, binary.to_path_buf()))
        .collect();
    let mut visited = BTreeSet::new();
    let mut libraries = Vec::new();
    let mut missing_occt = Vec::new();

    while let Some((needed, from)) = queue.pop_front() {
        let name = file_name(&needed);
        if !visited.insert(name.clone()) {
            continue;
        }
        if dest.join(&name).is_file() {
            continue;
        }
        match resolve(&name, &search) {
            Some(path) => {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if canonical.starts_with(&prefix) {
                    let size = fs::metadata(&canonical)
                        .map_err(|error| error.to_string())?
                        .len();
                    fs::copy(&canonical, dest.join(&name)).map_err(|error| {
                        format!(
                            "copy {} -> {}: {error}",
                            canonical.display(),
                            dest.join(&name).display()
                        )
                    })?;
                    libraries.push(StagedLibrary {
                        name: name.clone(),
                        source: canonical.clone(),
                        size,
                    });
                    for dependency in elf_needed(&canonical)? {
                        queue.push_back((dependency, canonical.clone()));
                    }
                }
            }
            None if looks_like_occt(&name) => missing_occt.push((name, from)),
            None => {}
        }
    }

    if !missing_occt.is_empty() {
        let version =
            occt::version(prefix.as_path()).unwrap_or_else(|| "an unknown version".to_string());
        let list = missing_occt
            .iter()
            .map(|(name, from)| format!("  {name} (needed by {})", from.display()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "the binary references OCCT libraries the prefix does not provide:\n{list}\n\
             The prefix at {} is OCCT {version}. A binary linked against a different \
             OCCT (for example a system 7.9 beside a 7.8.1 prefix) cannot be bundled. \
             Rebuild with OpenCASCADE_DIR/OCCT_INCLUDE_DIR pointing at this prefix \
             and check `readelf -d {} | grep TK`.",
            prefix.display(),
            binary.display()
        ));
    }

    verify_elf(binary, dest, runtime_dirs)?;
    Ok(Closure { libraries })
}

/// The `DT_NEEDED` sonames of an ELF file.
fn elf_needed(path: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("readelf")
        .arg("-d")
        .arg(path)
        .output()
        .map_err(|error| {
            format!(
                "failed to run readelf (needed to walk {}): {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "readelf -d {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut needed = Vec::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("(NEEDED)") else {
            continue;
        };
        if let (Some(start), Some(end)) = (rest.find('['), rest.find(']')) {
            needed.extend((end > start).then(|| rest[start + 1..end].to_string()));
        }
    }
    Ok(needed)
}

/// Prove the staged `lib/` really satisfies the binary, without running it.
///
/// `ldd` invokes the dynamic loader with `LD_TRACE_LOADED_OBJECTS`, so this loads
/// and resolves every dependency but never runs `main` (no display, no GPU). The
/// OCCT check does not trust a system copy: every OCCT soname must be in `dest`,
/// which is what makes the artifact self-contained.
fn verify_elf(binary: &Path, dest: &Path, runtime_dirs: &[PathBuf]) -> Result<(), String> {
    let mut path_entries = vec![dest.to_path_buf()];
    path_entries.extend(runtime_dirs.iter().cloned());
    let library_path = join_paths(&path_entries);

    let output = Command::new("ldd")
        .arg(binary)
        .env("LD_LIBRARY_PATH", &library_path)
        .output()
        .map_err(|error| format!("failed to run ldd for {}: {error}", binary.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.contains("not a dynamic executable") {
        return Err(format!("{} is not a dynamic executable", binary.display()));
    }
    let unresolved = text
        .lines()
        .filter(|line| line.contains("=> not found"))
        .map(str::trim)
        .collect::<Vec<_>>();

    let not_bundled = elf_needed(binary)?
        .into_iter()
        .filter(|name| looks_like_occt(name) && !dest.join(file_name(name)).is_file())
        .collect::<Vec<_>>();

    let mut problems = Vec::new();
    if !not_bundled.is_empty() {
        problems.push(format!(
            "OCCT libraries are not in {}: {}",
            dest.display(),
            not_bundled.join(", ")
        ));
    }
    if !unresolved.is_empty() {
        problems.push(format!(
            "the loader cannot resolve:\n  {}",
            unresolved.join("\n  ")
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the staged payload is not self-contained (LD_LIBRARY_PATH={library_path}):\n{}",
            problems.join("\n")
        ))
    }
}

// ---------------------------------------------------------------------------
// Windows / PE
// ---------------------------------------------------------------------------

fn stage_pe(
    binary: &Path,
    dest: &Path,
    prefix: &Path,
    runtime_dirs: &[PathBuf],
) -> Result<Closure, String> {
    let prefix = prefix
        .canonicalize()
        .unwrap_or_else(|_| prefix.to_path_buf());
    let mut search = vec![dest.to_path_buf()];
    search.extend(occt::lib_dirs(prefix.as_path()));
    search.extend(runtime_dirs.iter().cloned());

    let mut queue: VecDeque<String> = pe_imports(binary)?.into();
    let mut visited = BTreeSet::new();
    let mut libraries = Vec::new();
    let mut missing_occt = Vec::new();

    while let Some(import) = queue.pop_front() {
        let key = import.to_ascii_lowercase();
        if !visited.insert(key.clone()) {
            continue;
        }
        if resolve_ci(&[dest.to_path_buf()], &key).is_some() {
            continue;
        }
        match resolve_ci(&search, &key) {
            Some(path) => {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if canonical.starts_with(&prefix) {
                    let name = canonical
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .ok_or_else(|| format!("no file name in {}", canonical.display()))?;
                    let size = fs::metadata(&canonical)
                        .map_err(|error| error.to_string())?
                        .len();
                    fs::copy(&canonical, dest.join(&name)).map_err(|error| {
                        format!(
                            "copy {} -> {}: {error}",
                            canonical.display(),
                            dest.join(&name).display()
                        )
                    })?;
                    libraries.push(StagedLibrary {
                        name,
                        source: canonical.clone(),
                        size,
                    });
                    queue.extend(pe_imports(&canonical)?);
                }
            }
            None if looks_like_occt(&key) => missing_occt.push(import),
            None => {}
        }
    }

    if !missing_occt.is_empty() {
        let version =
            occt::version(prefix.as_path()).unwrap_or_else(|| "an unknown version".to_string());
        return Err(format!(
            "the binary imports OCCT DLLs the prefix does not provide: {}\n\
             The prefix at {} is OCCT {version}. Rebuild against that prefix \
             (OpenCASCADE_DIR/OCCT_INCLUDE_DIR) before bundling.",
            missing_occt.join(", "),
            prefix.display()
        ));
    }

    verify_pe(binary, dest)?;
    Ok(Closure { libraries })
}

fn pe_imports(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    pe::imports(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn verify_pe(binary: &Path, dest: &Path) -> Result<(), String> {
    let not_bundled = pe_imports(binary)?
        .into_iter()
        .filter(|name| looks_like_occt(name) && resolve_ci(&[dest.to_path_buf()], name).is_none())
        .collect::<Vec<_>>();
    if not_bundled.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the staged payload is not self-contained: OCCT DLLs are not in {}: {}",
            dest.display(),
            not_bundled.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn file_name(needed: &str) -> String {
    Path::new(needed)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| needed.to_string())
}

/// OCCT's toolkits are `libTK*.so`/`.dylib` or `TK*.dll`; only these are a hard
/// bundling error when the prefix lacks them (a missing libc is the host's job).
fn looks_like_occt(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("libtk") || (lower.starts_with("tk") && lower.ends_with(".dll"))
}

fn resolve(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Case-insensitive lookup (Windows filenames are case-insensitive).
fn resolve_ci(dirs: &[PathBuf], wanted: &str) -> Option<PathBuf> {
    let wanted = wanted.to_ascii_lowercase();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().to_ascii_lowercase() == wanted {
                return Some(entry.path());
            }
        }
    }
    None
}

pub fn join_paths(paths: &[PathBuf]) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}

#[cfg(test)]
mod tests {
    use super::{looks_like_occt, stage};
    use std::path::Path;

    #[test]
    fn recognises_occt_library_names() {
        assert!(looks_like_occt("libTKernel.so.7.8"));
        assert!(looks_like_occt("libTKMath.dylib"));
        assert!(looks_like_occt("TKMath.dll"));
        assert!(looks_like_occt("tkmath.DLL"));
        assert!(!looks_like_occt("libpython3.14.so.1.0"));
        assert!(!looks_like_occt("libstdc++.so.6"));
        assert!(!looks_like_occt("KERNEL32.dll"));
    }

    /// Exercise the real walker against a built binary, without `bundle`
    /// rebuilding anything. Off unless `FERROCAD_XTASK_CLOSURE_BINARY` is set, so
    /// CI does not depend on a prebuilt artifact:
    ///
    /// ```sh
    /// FERROCAD_XTASK_CLOSURE_BINARY=target/debug/ferrocad \
    /// FERROCAD_OCCT_PREFIX=.occt/prefix \
    ///   cargo test -p xtask --offline closure_walks_a_real_binary
    /// ```
    ///
    /// Set `FERROCAD_XTASK_CLOSURE_EXPECT_ERROR=1` for a binary linked against a
    /// different OCCT than the prefix (the 7.9-vs-7.8.1 mismatch in AGENTS.md);
    /// the test then requires the walk to fail with the clear OCCT message.
    #[test]
    fn closure_walks_a_real_binary() {
        let Ok(binary) = std::env::var("FERROCAD_XTASK_CLOSURE_BINARY") else {
            return;
        };
        let prefix = std::env::var("FERROCAD_OCCT_PREFIX")
            .expect("set FERROCAD_OCCT_PREFIX with FERROCAD_XTASK_CLOSURE_BINARY");
        let dest = std::env::temp_dir().join(format!(
            "ferrocad-closure-{}-{}",
            std::process::id(),
            std::env::var("FERROCAD_XTASK_CLOSURE_EXPECT_ERROR").is_ok()
        ));
        let _ = std::fs::remove_dir_all(&dest);

        let result = stage(Path::new(&binary), &dest, Path::new(&prefix), &[]);

        if std::env::var("FERROCAD_XTASK_CLOSURE_EXPECT_ERROR").is_ok() {
            let error = result.expect_err("expected the closure walk to fail");
            eprintln!("{error}");
            assert!(error.contains("OCCT"), "{error}");
            assert!(error.contains("does not provide"), "{error}");
        } else {
            let closure = result.expect("closure walk failed");
            eprintln!(
                "bundled {} library file(s): {}",
                closure.libraries.len(),
                closure
                    .libraries
                    .iter()
                    .map(|library| library.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            assert!(!closure.libraries.is_empty(), "no prefix libraries found");
        }

        let _ = std::fs::remove_dir_all(&dest);
    }
}
