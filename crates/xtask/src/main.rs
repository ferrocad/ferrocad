//! `cargo xtask` tasks for FerroCAD.
//!
//!     cargo xtask bundle [--debug] [--out DIR] [--python | --system-python]
//!                        [--edition NAME] [--occt | --occt-prefix DIR]
//!                        [--offline] [--check-init]
//!     cargo xtask occt [--occt-prefix DIR] [--offline]
//!     cargo xtask python
//!     cargo xtask mods [--freecad TAG] [--only WB,WB]
//!     cargo xtask stage-payload [--python-only]
//!     cargo xtask unstage-payload
//!
//! `bundle` stages the platform-neutral distribution payload: the app binary, the
//! Python facade and scripts, the `mods/` workbenches, the license and, when
//! available, a bundled Python runtime. `--edition parametric` builds
//! `ferrocad_parametric` instead and stages it as the generic `bin/ferrocad`;
//! `--occt` walks the binary's shared-library closure and copies the OCCT
//! toolkits into `lib/` (beside the `.exe` on Windows). Per-platform packaging
//! (AppImage, `.app`, portable Windows) lives in `packaging/` and wraps this
//! payload; see `docs/architecture/distribution.md` and
//! `docs/architecture/edition-publishing.md`. The app binary links the PyO3
//! bindings and registers them as a built-in `ferrocad` module, so no separate
//! extension file ships.
//!
//! `occt` resolves (or, with network, fetches) the pinned OCCT 7.8.1 prefix and
//! records it under `target/occt-runtime/<triple>/`. `--check-init` boots the
//! staged payload with `bin/ferrocad --check-init`: no window, no GPU, so it works
//! on a headless runner.
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

mod closure;
mod occt;
mod pe;

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
            let opts = match Options::parse(args) {
                Ok(opts) => opts,
                Err(e) => {
                    eprintln!("xtask: {e}");
                    std::process::exit(2);
                }
            };
            if let Err(e) = bundle(&opts) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        Some("occt") => {
            let opts = match Options::parse(args) {
                Ok(opts) => opts,
                Err(e) => {
                    eprintln!("xtask: {e}");
                    std::process::exit(2);
                }
            };
            if let Err(e) = occt_task(&opts) {
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
            let opts = match Options::parse(args) {
                Ok(opts) => opts,
                Err(e) => {
                    eprintln!("xtask: {e}");
                    std::process::exit(2);
                }
            };
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
    println!("                   [--edition parametric] [--occt|--occt-prefix DIR]");
    println!("                   [--offline] [--check-init]");
    println!("cargo xtask occt [--occt-prefix DIR] [--offline]   # resolve/fetch OCCT 7.8.1");
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
    edition: Option<String>,
    occt: bool,
    occt_prefix: Option<PathBuf>,
    offline: bool,
    check_init: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut opts = Options {
            release: true,
            out: None,
            python: PythonMode::Auto,
            freecad: None,
            only: Vec::new(),
            edition: None,
            occt: false,
            occt_prefix: None,
            offline: false,
            check_init: false,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--debug" => opts.release = false,
                "--release" => opts.release = true,
                "--out" => opts.out = Some(PathBuf::from(value(&mut args, "--out")?)),
                "--python" => opts.python = PythonMode::Bundled,
                "--system-python" => opts.python = PythonMode::System,
                "--freecad" => opts.freecad = Some(value(&mut args, "--freecad")?),
                "--only" => {
                    let list = value(&mut args, "--only")?;
                    opts.only = list
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "--edition" => opts.edition = Some(value(&mut args, "--edition")?),
                "--occt" => opts.occt = true,
                "--occt-prefix" => {
                    opts.occt = true;
                    opts.occt_prefix = Some(PathBuf::from(value(&mut args, "--occt-prefix")?));
                }
                "--offline" => opts.offline = true,
                "--check-init" => opts.check_init = true,
                other => return Err(format!("unknown flag `{other}`")),
            }
        }
        Ok(opts)
    }
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} wants a value"))
}

/// `crates/xtask` -> `crates` -> workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask lives in crates/xtask")
        .to_path_buf()
}

/// An edition selects the binary crate and the payload directory. The default is
/// the base app; `parametric` is the Part/OCCT edition.
struct Edition {
    /// The short edition name used in artifact names (`ferrocad-<name>-…`).
    name: &'static str,
    /// The cargo package to build.
    package: &'static str,
    /// The bin target name (the built file's stem).
    binary: &'static str,
    /// `target/dist/<payload>`; the outer artifact name adds the platform.
    payload: &'static str,
    /// The edition links OCCT, so a bundle without `--occt` would be broken.
    requires_occt: bool,
}

impl Edition {
    fn base() -> Self {
        Self {
            name: "ferrocad",
            package: "ferrocad",
            binary: "ferrocad",
            payload: "ferrocad",
            requires_occt: false,
        }
    }

    fn select(name: Option<&str>) -> Result<Self, String> {
        match name {
            None | Some("base") | Some("ferrocad") => Ok(Self::base()),
            Some("parametric") | Some("ferrocad_parametric") | Some("ferrocad-parametric") => {
                Ok(Self {
                    name: "parametric",
                    package: "ferrocad_parametric",
                    binary: "ferrocad_parametric",
                    payload: "ferrocad-parametric",
                    requires_occt: true,
                })
            }
            Some(other) => Err(format!(
                "unknown edition `{other}`; known editions: parametric (omit --edition for the base app)"
            )),
        }
    }
}

fn bundle(opts: &Options) -> Result<(), String> {
    let root = workspace_root();
    let edition = Edition::select(opts.edition.as_deref())?;
    let profile = if opts.release { "release" } else { "debug" };
    let target = root.join("target").join(profile);

    if edition.requires_occt && !opts.occt {
        return Err(format!(
            "the {} edition links OCCT; pass --occt (resolve or fetch {}) or --occt-prefix DIR",
            edition.name,
            occt::PINNED_VERSION
        ));
    }
    let occt_prefix = if opts.occt {
        let prefix = occt::resolve(&root, opts.occt_prefix.as_deref(), opts.offline)?;
        let version = occt::version(&prefix).unwrap_or_else(|| "unknown".to_string());
        eprintln!("== OCCT {version} prefix: {} ==", prefix.display());
        if version != occt::PINNED_VERSION {
            eprintln!(
                "warning: OCCT {version} is not the pinned {}; the binary must be linked \
                 against this prefix",
                occt::PINNED_VERSION
            );
        }
        Some(prefix)
    } else {
        None
    };

    let runtime = match opts.python {
        PythonMode::System => None,
        PythonMode::Bundled => Some(ensure_runtime(&root)?),
        PythonMode::Auto => existing_runtime(&root),
    };

    eprintln!("== building {} ({profile}) ==", edition.package);
    // `--no-default-features` turns off the embedded payload: installers ship
    // `python/` and `mods/` loose beside the binary, so baking a duplicate copy
    // into it would only bloat the binary.
    let mut app = vec!["build", "-p", edition.package, "--no-default-features"];
    if opts.release {
        app.push("--release");
    }
    if opts.offline {
        app.push("--offline");
    }
    // Build the app against the bundled interpreter when there is one, so its
    // `libpython` soname matches the runtime we ship.
    let mut app_env = Vec::new();
    if let Some(rt) = &runtime {
        // python-build-standalone reports its libdir as the build-time
        // `/install/lib`, which PyO3 would use verbatim (it never consults the
        // relocated layout). Hand PyO3 an explicit config so it links the
        // runtime we ship rather than a path that only existed on the builder.
        let version =
            std::env::var("FERROCAD_PYTHON_VERSION").unwrap_or_else(|_| PYTHON_VERSION.to_string());
        let cfg = write_pyo3_config(&root, rt, &version)?;
        app_env.push((
            "PYO3_CONFIG_FILE".to_string(),
            cfg.to_string_lossy().into_owned(),
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
        .unwrap_or_else(|| root.join("target/dist").join(edition.payload));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| e.to_string())?;
    }
    for dir in ["bin", "python", "mods", "LICENSES"] {
        fs::create_dir_all(out.join(dir)).map_err(|e| e.to_string())?;
    }

    // The app binary, staged under a generic name so every launcher
    // (`AppRun`, the `.app`, `ferrocad.bat`) is edition-independent. The outer
    // artifact name is what identifies the edition.
    let built = target.join(exe_name(edition.binary));
    if !built.is_file() {
        return Err(format!(
            "`{}` did not produce {}; the build above did not create the binary",
            edition.package,
            built.display()
        ));
    }
    copy_file(&built, &out.join("bin").join(exe_name("ferrocad")))?;

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
        None => {
            eprintln!("note: no bundled Python runtime; the payload uses the system interpreter")
        }
    }

    // License text.
    copy_file(
        &root.join("LICENSE"),
        &out.join("LICENSES/LGPL-2.1-or-later.txt"),
    )?;

    if let Some(prefix) = &occt_prefix {
        eprintln!("== bundling the shared-library closure ==");
        let library_dir = if cfg!(windows) {
            // Windows finds DLLs beside the `.exe` before a system copy.
            out.join("bin")
        } else {
            out.join("lib")
        };
        let staged = closure::stage(
            &out.join("bin").join(exe_name("ferrocad")),
            &library_dir,
            prefix,
            &runtime_library_dirs(runtime.as_deref()),
        )?;
        closure::write_manifest(&out.join("MANIFEST"), &staged)?;
        eprintln!(
            "bundled {} library file(s) into {}",
            staged.libraries.len(),
            library_dir.display()
        );

        let licenses = occt::license_files(prefix);
        if licenses.is_empty() {
            eprintln!(
                "warning: no OCCT license files found under {}; stage LICENSE_LGPL_21.txt \
                 and OCCT_LGPL_EXCEPTION.txt under LICENSES/ before publishing",
                prefix.display()
            );
        }
        for (source, destination) in licenses {
            copy_file(&source, &out.join("LICENSES").join(destination))?;
        }
    }

    if opts.check_init {
        check_staged(&out, runtime.as_deref(), occt_prefix.as_deref())?;
    }

    eprintln!("staged {}", out.display());
    Ok(())
}

/// Resolve (or fetch) the OCCT prefix and record it for later offline runs.
fn occt_task(opts: &Options) -> Result<(), String> {
    let root = workspace_root();
    let prefix = occt::resolve(&root, opts.occt_prefix.as_deref(), opts.offline)?;
    let version = occt::version(&prefix).unwrap_or_else(|| "unknown".to_string());

    let marker = occt::marker_path(&root);
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&marker, format!("{}\n", prefix.display()))
        .map_err(|error| format!("write {}: {error}", marker.display()))?;

    println!("OCCT {version}: {}", prefix.display());
    if version != occt::PINNED_VERSION {
        eprintln!(
            "warning: the workspace pins OCCT {}; see docs/architecture/occt-bundling.md",
            occt::PINNED_VERSION
        );
    }
    for (source, destination) in occt::license_files(&prefix) {
        println!("license: {} -> {destination}", source.display());
    }
    println!("recorded {}", marker.display());
    Ok(())
}

/// The directories `bundle` may consult for the closure's dependencies beyond the
/// OCCT prefix. The runtime tree is already part of the payload, so these are
/// searched but never copied.
fn runtime_library_dirs(runtime: Option<&Path>) -> Vec<PathBuf> {
    match runtime {
        Some(rt) if cfg!(windows) => vec![rt.to_path_buf()],
        Some(rt) => vec![rt.join("lib")],
        None => Vec::new(),
    }
}

/// Boot the staged payload (`bin/ferrocad --check-init`) with only the bundled
/// library paths. No window and no GPU: usable on a headless release runner.
fn check_staged(
    out: &Path,
    runtime: Option<&Path>,
    occt_prefix: Option<&Path>,
) -> Result<(), String> {
    let binary = out.join("bin").join(exe_name("ferrocad"));
    if !binary.is_file() {
        return Err(format!(
            "cannot --check-init: {} is missing",
            binary.display()
        ));
    }

    let mut libraries = vec![if cfg!(windows) {
        out.join("bin")
    } else {
        out.join("lib")
    }];
    if let Some(rt) = runtime {
        libraries.push(if cfg!(windows) {
            rt.to_path_buf()
        } else {
            rt.join("lib")
        });
    }
    let library_path = closure::join_paths(&libraries);
    let python_path = closure::join_paths(&[out.join("python"), out.join("mods")]);

    let mut command = Command::new(&binary);
    command
        .arg("--check-init")
        .env("LD_LIBRARY_PATH", &library_path)
        .env("DYLD_LIBRARY_PATH", &library_path)
        .env("FERROCAD_PYTHON_PATH", out.join("python"))
        .env("FERROCAD_MODS_PATH", out.join("mods"))
        .env("PYTHONPATH", &python_path);
    if let Some(rt) = runtime {
        command.env("PYTHONHOME", rt);
    }

    // Windows resolves DLLs through PATH after the `.exe`'s own directory. The
    // build may have left the OCCT prefix on PATH, so drop it here: a missing
    // bundled DLL must fail rather than be found in the prefix.
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut path = std::ffi::OsString::new();
    if let Some(rt) = runtime {
        let runtime_bin = if cfg!(windows) {
            rt.to_path_buf()
        } else {
            rt.join("bin")
        };
        path.push(runtime_bin.as_os_str());
        path.push(separator);
    }
    let prefix = occt_prefix.and_then(|prefix| prefix.canonicalize().ok());
    for entry in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if let Some(prefix) = &prefix {
            let canonical = entry.canonicalize().unwrap_or_else(|_| entry.clone());
            if canonical.starts_with(prefix) {
                continue;
            }
        }
        path.push(entry.as_os_str());
        path.push(separator);
    }
    command.env("PATH", path);

    eprintln!("== checking the staged payload boots (--check-init) ==");
    run_command(&mut command, "staged ferrocad --check-init")
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

/// Write a PyO3 config file describing the bundled runtime, and return its path.
///
/// PyO3's interpreter probe reads `sysconfig`, and python-build-standalone reports
/// `LIBDIR=/install/lib` (its build prefix) rather than the relocated path. A
/// config file overrides the probe; only `lib_dir`/`lib_name` matter for linking
/// (PyO3 uses no Python headers). `abi3` is left out on purpose, so the crate's
/// `abi3-py3*` feature still applies (PyO3 ORs it in).
fn write_pyo3_config(root: &Path, runtime: &Path, version: &str) -> Result<PathBuf, String> {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("3");
    let minor = parts.next().unwrap_or("0");
    let path = root
        .join("target/pyo3-config")
        .join(format!("{}.txt", pbs_triple()?));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Windows ships the stable-ABI import library as `libs/python3.lib` (and
    // `libs/python3XX.lib`); Unix ships `lib/libpython3.XX.*`. FerroCAD always
    // builds with abi3, so the Windows lib name is `python3`.
    let (lib_name, lib_dir) = if cfg!(windows) {
        ("python3".to_string(), runtime.join("libs"))
    } else {
        (format!("python{major}.{minor}"), runtime.join("lib"))
    };
    let contents = format!(
        "implementation=CPython\n\
         version={major}.{minor}\n\
         shared=true\n\
         lib_name={lib_name}\n\
         lib_dir={}\n\
         executable={}\n\
         pointer_width={}\n",
        lib_dir.display(),
        python_exe_in(runtime).display(),
        std::mem::size_of::<usize>() * 8,
    );
    fs::write(&path, contents).map_err(|e| e.to_string())?;
    eprintln!("wrote PyO3 config {}", path.display());
    Ok(path)
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
        return Err(format!(
            "runtime interpreter not found at {}",
            exe.display()
        ));
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
/// them into the payload. See `docs/architecture/distribution.md`.
///
/// `src/Ext/freecad` is fetched too and copied to `mods/freecad`. The packaging
/// step puts `mods/` on `PYTHONPATH`, so `from freecad.deprecation import
/// deprecated` (used by `DraftVecUtils`) resolves at boot without the app having
/// to know about upstream's Ext directory.
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
    // The `freecad` support package lives outside `src/Mod` and is imported by
    // Draft at module scope (`freecad.deprecation`).
    sparse.arg("src/Ext");
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
        write_resource_stub(&dst, wb)?;
        eprintln!("copied {wb} -> {}", dst.display());
    }

    // `src/Ext/freecad` is a namespace package (upstream generates its
    // `__init__.py` from a CMake template). Copied beside the workbenches, it is
    // importable because `mods/` is on `PYTHONPATH` in the packaged app.
    let ext_pkg = repo.join("src").join("Ext").join("freecad");
    if ext_pkg.is_dir() {
        let dst = mods.join("freecad");
        if dst.exists() {
            fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
        }
        copy_dir_all(&ext_pkg, &dst)?;
        eprintln!("copied src/Ext/freecad -> {}", dst.display());
    } else {
        eprintln!(
            "warning: src/Ext/freecad not found at FreeCAD {tag}; \
             `from freecad.deprecation import ...` will fail"
        );
    }

    eprintln!("mods ready under {}", mods.display());
    Ok(())
}

/// Write the no-op resource module upstream generates with `pyside-rcc`.
///
/// Upstream code does `import Draft_rc` and uses a truthy `__name__` as its
/// "resources are loaded" marker; the headless path never touches the bundle.
/// The FerroCAD facade also ships `python/Draft_rc.py`, so this copy only helps
/// when the workbench directory itself is on `sys.path` (as it is in FreeCAD).
fn write_resource_stub(dir: &Path, workbench: &str) -> Result<(), String> {
    let path = dir.join(format!("{workbench}_rc.py"));
    if path.exists() {
        return Ok(());
    }
    let body = format!(
        "\"\"\"No-op resource module for {workbench} (FerroCAD).\"\"\"\n\n\n\
         def qInitResources():\n    pass\n\n\n\
         def qCleanupResources():\n    pass\n"
    );
    fs::write(&path, body).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::write_resource_stub;

    #[test]
    fn resource_stub_is_a_noop_module_and_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("ferrocad-xtask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Draft_rc.py");
        let _ = std::fs::remove_file(&path);

        write_resource_stub(&dir, "Draft").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let expected = concat!(
            "\"\"\"No-op resource module for Draft (FerroCAD).\"\"\"\n\n\n",
            "def qInitResources():\n    pass\n\n\n",
            "def qCleanupResources():\n    pass\n",
        );
        assert_eq!(body, expected);

        // A fetched tree that already ships the generated module keeps it.
        std::fs::write(&path, "sentinel").unwrap();
        write_resource_stub(&dir, "Draft").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sentinel");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
