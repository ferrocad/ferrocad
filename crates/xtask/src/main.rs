//! `cargo xtask` tasks for FerroCAD.
//!
//!     cargo xtask bundle [--debug] [--out DIR] [--python | --system-python]
//!     cargo xtask python
//!
//! `bundle` stages the platform-neutral distribution payload: the app binary, the
//! PyO3 extension, the Python facade and scripts, the `mods/` workbenches, the
//! license and, when available, a bundled Python runtime. Per-platform packaging
//! (AppImage, `.app`, portable Windows) lives in `packaging/` and wraps this
//! payload; see `docs/distribution.md`.
//!
//! `python` fetches a `python-build-standalone` runtime and caches it under
//! `target/python-runtime/`. When a runtime is present, `bundle` copies it into
//! the payload as `runtime/` and builds the app against it, so the artifact does
//! not depend on the system CPython.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned `python-build-standalone` release and CPython version. Override with
/// `FERROCAD_PBS_DATE` / `FERROCAD_PYTHON_VERSION`.
const PBS_DATE: &str = "20261003";
const PYTHON_VERSION: &str = "3.14.8";

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
}

struct Options {
    release: bool,
    out: Option<PathBuf>,
    python: PythonMode,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut opts = Options {
            release: true,
            out: None,
            python: PythonMode::Auto,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--debug" => opts.release = false,
                "--release" => opts.release = true,
                "--out" => opts.out = args.next().map(PathBuf::from),
                "--python" => opts.python = PythonMode::Bundled,
                "--system-python" => opts.python = PythonMode::System,
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
    let mut app = vec!["build", "-p", "ferrocad"];
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

    eprintln!("== building the ferrocad extension ({profile}) ==");
    let mut ext = vec![
        "build",
        "-p",
        "ferrocad_py",
        "--features",
        "extension-module",
    ];
    if opts.release {
        ext.push("--release");
    }
    run_cargo(&root, &ext, &[])?;

    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join("target/dist/ferrocad"));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| e.to_string())?;
    }
    for dir in ["bin", "lib", "python", "mods", "LICENSES"] {
        fs::create_dir_all(out.join(dir)).map_err(|e| e.to_string())?;
    }

    // The app binary.
    copy_file(
        &target.join(exe_name("ferrocad")),
        &out.join("bin").join(exe_name("ferrocad")),
    )?;

    // The PyO3 extension, named for import as `ferrocad`.
    let ext_src = find_extension(&target)?;
    copy_file(&ext_src, &out.join("lib").join(module_file_name()))?;

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

fn module_file_name() -> &'static str {
    if cfg!(windows) {
        "ferrocad.pyd"
    } else {
        "ferrocad.abi3.so"
    }
}

fn find_extension(dir: &Path) -> Result<PathBuf, String> {
    for name in ["libferrocad_py.so", "libferrocad_py.dylib", "ferrocad_py.dll"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "extension library not found in {} (was ferrocad_py built?)",
        dir.display()
    ))
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
