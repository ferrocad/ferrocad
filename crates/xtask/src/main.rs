//! `cargo xtask` tasks for FerroCAD.
//!
//!     cargo xtask bundle [--debug] [--out DIR] [--python | --system-python]
//!     cargo xtask python
//!     cargo xtask mods [--freecad TAG] [--only WB,WB]
//!     cargo xtask stage-payload [--python-only]
//!     cargo xtask unstage-payload
//!
//! `bundle` stages the platform-neutral distribution payload: the app binary, the
//! Python facade and scripts, the `mods/` workbenches, the license and, when
//! available, a bundled Python runtime. Per-platform packaging (AppImage, `.app`,
//! portable Windows) lives in `packaging/` and wraps this payload; see
//! `docs/distribution.md`. The app binary links the PyO3 bindings and registers
//! them as a built-in `ferrocad` module, so no separate extension file ships.
//!
//! `python` fetches a `python-build-standalone` runtime and caches it under
//! `target/python-runtime/`. `mods` fetches workbench scripts from a pinned
//! upstream FreeCAD revision into `mods/` (loose files, as FreeCAD ships them).
//!
//! `stage-payload` / `unstage-payload` copy that payload in beside the app
//! crate's manifest and remove it again. They exist for *publishing*: crates.io
//! gets a self-contained tarball, so `python/` and `mods/` (which live at the
//! workspace root, shared across editions) are transiently staged, then cleaned
//! up. The app crate's `build.rs` embeds them, so `cargo install ferrocad`
//! produces a working app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned `python-build-standalone` release and CPython version. Override with
/// `FERROCAD_PBS_DATE` / `FERROCAD_PYTHON_VERSION`.
const PBS_DATE: &str = "20261003";
const PYTHON_VERSION: &str = "3.14.8";

/// Pinned upstream FreeCAD revision for workbench scripts. Override with
/// `FERROCAD_FREECAD_TAG` or `--freecad`.
const FREECAD_TAG: &str = "1.1.4";
/// Workbenches fetched by default (pure-Python, no geometry kernel needed yet).
const DEFAULT_MODS: &[&str] = &["Draft"];

#[derive(Clone, Copy, PartialEq)]
enum PythonMode {
    /// Include a runtime if it is already cached.
    Auto,
    /// Ensure a runtime is fetched, then include it.
    Bundled,
    /// Do not include a runtime; use the system interpreter.
    System,
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("bundle") => {
            let opts = Options::parse(args);
            if let Err(e) = bundle(&opts) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        Some("python") => {
            let root = workspace_root();
            match ensure_runtime(&root) {
                Ok(dir) => println!("runtime ready at {}", dir.display()),
                Err(e) => {
                    eprintln!("xtask: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("mods") => {
            let opts = Options::parse(args);
            if let Err(e) = fetch_mods(&opts) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        Some("stage-payload") => {
            let python_only = args.any(|a| a == "--python-only");
            if let Err(e) = stage_payload(python_only) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        Some("unstage-payload") => {
            if let Err(e) = unstage_payload() {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        Some("help") | Some("--help") | Some("-h") | None => usage(),
        Some(other) => {
            eprintln!("xtask: unknown task `{other}`\n");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    println!("cargo xtask bundle [--debug] [--out DIR] [--python|--system-python]");
    println!("cargo xtask python   # fetch a python-build-standalone runtime");
    println!("cargo xtask mods [--freecad TAG] [--only WB,WB]   # fetch workbench scripts");
    println!("cargo xtask stage-payload [--python-only]   # stage the payload for publishing");
    println!("cargo xtask unstage-payload   # remove the staged payload");
}

struct Options {
    release: bool,
    out: Option<PathBuf>,
    python: PythonMode,
    freecad: Option<String>,
    only: Vec<String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut opts = Options {
            release: true,
            out: None,
            python: PythonMode::Auto,
            freecad: None,
            only: Vec::new(),
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--debug" => opts.release = false,
                "--release" => opts.release = true,
                "--out" => opts.out = args.next().map(PathBuf::from),
                "--python" => opts.python = PythonMode::Bundled,
                "--system-python" => opts.python = PythonMode::System,
                "--freecad" => opts.freecad = args.next(),
                "--only" => {
                    if let Some(list) = args.next() {
                        opts.only = list
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                other => eprintln!("xtask: ignoring unknown flag `{other}`"),
            }
        }
        opts
    }
}

/// `crates/xtask` -> `crates` -> workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask lives in crates/xtask")
        .to_path_buf()
}

fn bundle(opts: &Options) -> Result<(), String> {
    let root = workspace_root();
    let profile = if opts.release { "release" } else { "debug" };
    let target = root.join("target").join(profile);

    let runtime = match opts.python {
        PythonMode::System => None,
        PythonMode::Bundled => Some(ensure_runtime(&root)?),
        PythonMode::Auto => existing_runtime(&root),
    };

    eprintln!("== building ferrocad ({profile}) ==");
    // `--no-default-features` turns off the embedded payload: installers ship
    // `python/` and `mods/` loose beside the binary, so baking a duplicate copy
    // into it would only bloat the binary.
    let mut app = vec!["build", "-p", "ferrocad", "--no-default-features"];
    if opts.release {
        app.push("--release");
    }
    // Build the app against the bundled interpreter when there is one, so its
    // `libpython` soname matches the runtime we ship.
    let mut app_env = Vec::new();
    if let Some(rt) = &runtime {
        app_env.push((
            "PYO3_PYTHON".to_string(),
            python_exe_in(rt).to_string_lossy().into_owned(),
        ));
        app_env.push((
            "PYO3_USE_ABI3_FORWARD_COMPATIBILITY".to_string(),
            "1".to_string(),
        ));
    }
    run_cargo(&root, &app, &app_env)?;

    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join("target/dist/ferrocad"));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| e.to_string())?;
    }
    for dir in ["bin", "python", "mods", "LICENSES"] {
        fs::create_dir_all(out.join(dir)).map_err(|e| e.to_string())?;
    }

    // The app binary (it links the PyO3 bindings as a built-in `ferrocad`
    // module, so there is no separate extension file to stage).
    copy_file(
        &target.join(exe_name("ferrocad")),
        &out.join("bin").join(exe_name("ferrocad")),
    )?;

    // The Python sources (facade + app scripts), without build artifacts.
    copy_dir_filtered(&root.join("python"), &out.join("python"))?;

    // The workbenches.
    let mods = root.join("mods");
    if mods.is_dir() {
        copy_dir_filtered(&mods, &out.join("mods"))?;
    }

    // The bundled CPython runtime, if we have one.
    match &runtime {
        Some(rt) => {
            eprintln!("== bundling the Python runtime ==");
            copy_dir_all(rt, &out.join("runtime"))?;
        }
        None => eprintln!("note: no bundled Python runtime; the payload uses the system interpreter"),
    }

    // License text.
    copy_file(
        &root.join("LICENSE"),
        &out.join("LICENSES/LGPL-2.1-or-later.txt"),
    )?;

    eprintln!("staged {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// python-build-standalone runtime
// ---------------------------------------------------------------------------

fn pbs_triple() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        (os, arch) => Err(format!(
            "no python-build-standalone build known for {os}/{arch}"
        )),
    }
}

fn runtime_root(root: &Path) -> Result<PathBuf, String> {
    Ok(root.join("target/python-runtime").join(pbs_triple()?))
}

fn python_exe_in(runtime: &Path) -> PathBuf {
    if cfg!(windows) {
        runtime.join("python.exe")
    } else {
        runtime.join("bin").join("python3")
    }
}

fn existing_runtime(root: &Path) -> Option<PathBuf> {
    let dir = runtime_root(root).ok()?;
    if python_exe_in(&dir).is_file() {
        Some(dir)
    } else {
        None
    }
}

/// Fetch and extract the pinned runtime into `target/python-runtime/<triple>/`.
fn ensure_runtime(root: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = existing_runtime(root) {
        return Ok(dir);
    }

    let triple = pbs_triple()?;
    let version =
        std::env::var("FERROCAD_PYTHON_VERSION").unwrap_or_else(|_| PYTHON_VERSION.to_string());
    let date = std::env::var("FERROCAD_PBS_DATE").unwrap_or_else(|_| PBS_DATE.to_string());
    let asset = format!("cpython-{version}+{date}-{triple}-install_only.tar.gz");
    let url = format!(
        "https://github.com/astral-sh/python-build-standalone/releases/download/{date}/{asset}"
    );

    let cache = root.join("target/python-runtime");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let tarball = cache.join(&asset);

    if !tarball.is_file() {
        eprintln!("== downloading {url} ==");
        let status = Command::new("curl")
            .args(["-fL", "--retry", "3", "-o"])
            .arg(&tarball)
            .arg(&url)
            .status()
            .map_err(|e| format!("failed to run curl (needed to fetch the runtime): {e}"))?;
        if !status.success() {
            return Err("downloading the Python runtime failed".to_string());
        }
    }

    let tmp = cache.join(format!(".extract-{triple}"));
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    eprintln!("== extracting {asset} ==");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if !status.success() {
        return Err("extracting the Python runtime failed".to_string());
    }

    // The archive contains a single top-level `python/` directory.
    let extracted = tmp.join("python");
    if !extracted.is_dir() {
        return Err(format!(
            "unexpected archive layout: {} is missing",
            extracted.display()
        ));
    }
    let dest = runtime_root(root)?;
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::rename(&extracted, &dest).map_err(|e| format!("moving the runtime: {e}"))?;
    let _ = fs::remove_dir_all(&tmp);

    // Verify it runs.
    let exe = python_exe_in(&dest);
    if !exe.is_file() {
        return Err(format!("runtime interpreter not found at {}", exe.display()));
    }
    let out = Command::new(&exe)
        .arg("--version")
        .output()
        .map_err(|e| format!("running {}: {e}", exe.display()))?;
    let reported = String::from_utf8_lossy(&out.stdout);
    eprint!("bundled interpreter: {reported}");
    Ok(dest)
}

// ---------------------------------------------------------------------------
// workbench scripts (mods)
// ---------------------------------------------------------------------------

/// Fetch workbench scripts from a pinned upstream FreeCAD revision into `mods/`.
///
/// Uses a blobless sparse checkout, so only the selected `src/Mod/<WB>` trees are
/// downloaded. The result is loose files, as FreeCAD ships them; `bundle` copies
/// them into the payload. See `docs/distribution.md`.
fn fetch_mods(opts: &Options) -> Result<(), String> {
    let root = workspace_root();
    let tag = opts
        .freecad
        .clone()
        .or_else(|| std::env::var("FERROCAD_FREECAD_TAG").ok())
        .unwrap_or_else(|| FREECAD_TAG.to_string());
    let workbenches: Vec<String> = if opts.only.is_empty() {
        DEFAULT_MODS.iter().map(|s| s.to_string()).collect()
    } else {
        opts.only.clone()
    };

    let repo = root.join("target/freecad-src").join(&tag).join("repo");
    if !repo.join(".git").is_dir() {
        if let Some(parent) = repo.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        eprintln!("== cloning FreeCAD {tag} (blobless sparse) ==");
        run_command(
            Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "--filter=blob:none",
                    "--sparse",
                    "--branch",
                    &tag,
                    "https://github.com/FreeCAD/FreeCAD",
                ])
                .arg(&repo),
            "git clone",
        )?;
    }

    let mut sparse = Command::new("git");
    sparse.arg("-C").arg(&repo).args(["sparse-checkout", "set"]);
    for wb in &workbenches {
        sparse.arg(format!("src/Mod/{wb}"));
    }
    run_command(&mut sparse, "git sparse-checkout")?;

    let mods = root.join("mods");
    fs::create_dir_all(&mods).map_err(|e| e.to_string())?;
    for wb in &workbenches {
        let src = repo.join("src").join("Mod").join(wb);
        if !src.is_dir() {
            return Err(format!(
                "workbench `{wb}` not found at FreeCAD {tag} ({})",
                src.display()
            ));
        }
        let dst = mods.join(wb);
        if dst.exists() {
            fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
        }
        copy_dir_all(&src, &dst)?;
        eprintln!("copied {wb} -> {}", dst.display());
    }
    eprintln!("mods ready under {}", mods.display());
    Ok(())
}

fn run_command(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed"))
    }
}

// ---------------------------------------------------------------------------
// publish staging
// ---------------------------------------------------------------------------

/// Copy the workspace payload (`python/`, and `mods/` unless `--python-only`)
/// in beside the app crate's manifest so `cargo publish` carries it.
///
/// These directories live at the workspace root because they are shared across
/// FerroCAD editions and are not crate sources. crates.io, however, uploads a
/// self-contained tarball, and `cargo install` has no data-file step, so the app
/// crate's `build.rs` embeds whatever sits beside its manifest. Staging is the
/// bridge; [`unstage_payload`] removes it again so it is never committed.
fn stage_payload(python_only: bool) -> Result<(), String> {
    let root = workspace_root();
    let crate_dir = root.join("crates/ferrocad");

    let facade = root.join("python").join("FreeCAD").join("__init__.py");
    if !facade.is_file() {
        return Err(format!(
            "no Python facade at {} (run from a full checkout)",
            facade.display()
        ));
    }

    let python_dst = crate_dir.join("python");
    replace_dir(&root.join("python"), &python_dst)?;

    let mods_dst = crate_dir.join("mods");
    // Always clear a stale `mods/`, so a `--python-only` publish does not
    // silently carry a leftover copy from an earlier staging.
    if mods_dst.exists() {
        fs::remove_dir_all(&mods_dst).map_err(|e| e.to_string())?;
    }
    if python_only {
        eprintln!("note: --python-only; workbenches stay loose and will not be embedded");
    } else {
        let mods = root.join("mods");
        if mods.is_dir() {
            replace_dir(&mods, &mods_dst)?;
        } else {
            eprintln!("note: no mods/ directory; publishing without workbenches");
        }
    }
    Ok(())
}

/// Remove the staged payload.
fn unstage_payload() -> Result<(), String> {
    let crate_dir = workspace_root().join("crates/ferrocad");
    for dir in ["python", "mods"] {
        let path = crate_dir.join(dir);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
            eprintln!("removed {}", path.display());
        }
    }
    Ok(())
}

/// Copy `src` to a fresh `dst`, dropping bytecode and native build artifacts.
fn replace_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        fs::remove_dir_all(dst).map_err(|e| e.to_string())?;
    }
    copy_dir_filtered(src, dst)?;
    eprintln!("staged {} -> {}", src.display(), dst.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn run_cargo(root: &Path, args: &[&str], envs: &[(String, String)]) -> Result<(), String> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(root).args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    Ok(())
}

/// Recursively copy a directory, skipping Python bytecode and native build
/// artifacts (used for the source tree; the extension is placed separately).
fn copy_dir_filtered(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy().into_owned();
        if name == "__pycache__" || name == "target" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            copy_dir_filtered(&from, &to)?;
        } else if file_type.is_file() {
            if matches!(
                from.extension().and_then(|e| e.to_str()),
                Some("pyc" | "so" | "dylib" | "dll" | "pyd")
            ) {
                continue;
            }
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

/// Recursively copy a directory verbatim (following symlinks). Used for the
/// Python runtime, where shared libraries must be preserved.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::metadata(&from).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if meta.is_file() {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}
