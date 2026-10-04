//! `cargo xtask` tasks for FerroCAD.
//!
//!     cargo xtask bundle [--debug] [--out DIR]
//!
//! Stages a runnable distribution directory: the app binary, the PyO3 extension,
//! the Python facade and scripts, the `mods/` workbenches, plus `AppRun` and a
//! `.desktop` entry. It uses the system CPython for now; bundling
//! python-build-standalone is a later step (see `docs/distribution.md`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        Some("appimage") => {
            let opts = Options::parse(args);
            if let Err(e) = appimage(&opts) {
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
    println!("cargo xtask bundle [--debug] [--out DIR]");
    println!("cargo xtask appimage [--debug] [--out DIR]");
}

struct Options {
    release: bool,
    out: Option<PathBuf>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut opts = Options {
            release: true,
            out: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--debug" => opts.release = false,
                "--release" => opts.release = true,
                "--out" => opts.out = args.next().map(PathBuf::from),
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

    eprintln!("== building ferrocad ({profile}) ==");
    let mut app = vec!["build", "-p", "ferrocad"];
    if opts.release {
        app.push("--release");
    }
    run_cargo(&root, &app)?;

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
    run_cargo(&root, &ext)?;

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
    copy_file(&target.join(exe_name("ferrocad")), &out.join("bin/ferrocad"))?;

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

    // License text.
    copy_file(
        &root.join("LICENSE"),
        &out.join("LICENSES/LGPL-2.1-or-later.txt"),
    )?;

    write_executable(&out.join("AppRun"), APP_RUN)?;
    fs::write(out.join("ferrocad.desktop"), DESKTOP).map_err(|e| e.to_string())?;

    eprintln!("staged {}", out.display());
    Ok(())
}

/// Stage the distribution, then wrap it in an AppImage with `appimagetool`.
///
/// The canonical tool is written in C. It is located on `PATH`, via
/// `APPIMAGETOOL`, or as `tools/appimagetool-<arch>.AppImage`. A `.AppImage` is
/// run with `--appimage-extract-and-run`, so no FUSE and no system install is
/// required. See `docs/distribution.md`.
fn appimage(opts: &Options) -> Result<(), String> {
    let root = workspace_root();

    // Always refresh the staged AppDir first.
    bundle(opts)?;

    let appdir = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join("target/dist/ferrocad"));

    let (tool, is_appimage) = find_appimagetool(&root).ok_or_else(|| {
        "appimagetool not found.\n\
         Put it on PATH, set APPIMAGETOOL, or drop \
         appimagetool-<arch>.AppImage into tools/.\n\
         It runs without FUSE via --appimage-extract-and-run.\n\
         Download: https://github.com/AppImage/appimagetool/releases"
            .to_string()
    })?;

    let out_image = root
        .join("target/dist")
        .join(format!("ferrocad-{}.AppImage", arch()));

    let mut cmd = Command::new(&tool);
    if is_appimage {
        // Avoid needing FUSE to run appimagetool itself.
        cmd.arg("--appimage-extract-and-run");
        cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    }
    cmd.env("ARCH", arch());
    cmd.arg(&appdir).arg(&out_image);
    cmd.current_dir(&root);

    eprintln!(
        "== appimagetool {} -> {} ==",
        appdir.display(),
        out_image.display()
    );
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run appimagetool: {e}"))?;
    if !status.success() {
        return Err("appimagetool failed".to_string());
    }
    eprintln!("created {}", out_image.display());
    Ok(())
}

fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// Find `appimagetool`. Returns the path and whether it is an `.AppImage`
/// (which must be invoked with `--appimage-extract-and-run`).
fn find_appimagetool(root: &Path) -> Option<(PathBuf, bool)> {
    if let Ok(path) = std::env::var("APPIMAGETOOL") {
        if !path.is_empty() {
            let path = PathBuf::from(path);
            let is_image = is_appimage_path(&path);
            return Some((path, is_image));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("appimagetool");
            if candidate.is_file() {
                return Some((candidate, false));
            }
        }
    }
    for candidate in [
        root.join("tools")
            .join(format!("appimagetool-{}.AppImage", arch())),
        root.join("tools").join("appimagetool.AppImage"),
    ] {
        if candidate.is_file() {
            return Some((candidate, true));
        }
    }
    None
}

fn is_appimage_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("AppImage"))
        .unwrap_or(false)
}

const APP_RUN: &str = "\
#!/bin/sh
# FerroCAD launcher: point the embedded interpreter at the bundled Python.
HERE=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)
export FERROCAD_PYTHON_PATH=\"$HERE/python\"
export PYTHONPATH=\"$HERE/lib:$HERE/python:$HERE/mods${PYTHONPATH:+:$PYTHONPATH}\"
exec \"$HERE/bin/ferrocad\" \"$@\"
";

const DESKTOP: &str = "\
[Desktop Entry]
Type=Application
Name=FerroCAD
Comment=Rust reimplementation of FreeCAD's App core
Exec=ferrocad
Icon=ferrocad
Terminal=false
Categories=Graphics;Engineering;Science;
";

fn run_cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(args)
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
/// artifacts (the extension is placed separately, in `lib/`).
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

fn write_executable(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|e| e.to_string())?;
    }
    Ok(())
}
