# Distribution: running and packaging FerroCAD

Status: reference (2026-10-05). Companion to [`repackaging.md`](repackaging.md),
[`releasing.md`](releasing.md) and [`architecture.md`](architecture.md). It answers:
how does the base app run, how is it packaged for end users, and can we ship a
Python interpreter and packaging scripts.

## 1. Two modes, one codebase

There are two ways to start the application, and they call the **same**
`ferrocad_gpui::run()`:

1. **Development / direct run.** `cargo run -p ferrocad` starts the base app and
   uses the *system* CPython (whatever `python3` and `libpython` the machine has).
   Fast to iterate; not something an end user installs.
2. **Bundled distribution.** A self-contained artifact (AppImage, `.app`, `.exe`)
   that carries its own CPython, the PyO3 extension, the `FreeCAD` facade, the
   app's startup scripts and its `Mod/` workbenches. The end user installs nothing
   else.

The only difference is *which interpreter, extension and scripts are found at
boot*. The app declares those through `HostConfig` (`entry_module` and
`python_paths`), so the code does not need to know which mode it is in.

## 2. Can we redistribute CPython? Yes.

CPython is licensed under the **PSF License Agreement** (PSF-2.0 since 3.8; older
parts under a BSD-style PSF-1.x). It is permissive and GPL-compatible: you may
redistribute it, including modified, as long as the copyright notice and license
text are retained. So shipping `libpython3.x.so` and the stdlib inside an
AppImage/`.app`/`.exe` is fine; just include the license (for example a
`LICENSES/` directory with the PSF text alongside FerroCAD's LGPL text).

Nothing about FerroCAD's LGPL-2.1-or-later license conflicts with this: the two
are combined, not merged, and we link no LGPL Qt.

### Bundling options

| Option | What it gives | Cost |
| --- | --- | --- |
| System Python | nothing to bundle | depends on the user's distro; version drift; not "self-contained" |
| **python-build-standalone** | a relocatable CPython (libpython + stdlib) per platform; the builds uv/rye use | ~30-50 MB per platform; download at build time |
| PyOxidizer | statically embeds CPython + stdlib into one binary | more machinery; harder for users to add third-party packages |
| Briefcase / PyInstaller | a frozen Python app | aimed at pure-Python apps; awkward around a Rust host |

**Chosen: python-build-standalone.** It matches the embedding model we have (we
link libpython and point `PYTHONHOME` at the bundle) and keeps the stdlib as real
files, so installing third-party addons into a user directory keeps working. It is
fetched by `cargo xtask python` and staged by `bundle` (see §4). PyOxidizer would
be the alternative only if we decided addons were out of scope.

## 3. Can we distribute packaging scripts? Yes, and we should.

Build scripts are code; shipping `packaging/appimage/build.sh`, a `.desktop` file,
or an `xtask` is exactly what Tauri, cargo-dist and many desktop apps do. Three
things make it work:

- **Reproducibility:** pin the python-build-standalone release and the tool
  versions, so a script produces the same artifact later.
- **CI builds the artifacts** per platform; the scripts are the source of truth.
- The scripts are **not** the artifact; they run (in CI or locally) to produce one.

## 4. The two approaches are not alternatives; they are layers

- A **`ferrocad` binary that runs directly** (`cargo run -p ferrocad`): the real
  entry point and the development path.
- A **packaging tool** (`cargo ferrocad bundle`, or a `cargo xtask bundle`): the
  release path. It packages that same binary.

The tool consumes the binary; the two never compete. A developer uses the first,
a release uses the second.

A bundled Linux AppImage would look like this:

```
ferrocad.AppImage
├── AppRun                        -> sets PYTHONHOME/FERROCAD_PYTHON_PATH, exec bin/ferrocad
├── ferrocad.desktop, ferro.png
├── bin/ferrocad                  Rust bin (crate `ferrocad` -> ferrocad_gpui::run_with)
│                                 links the PyO3 bindings; `ferrocad` is a built-in module
├── mods/                         the workbenches (FreeCAD's Mod/ equivalent)
├── python/FreeCAD/               the pure-Python facade
├── runtime/                      python-build-standalone (libpython + stdlib)
└── LICENSES/                     LGPL-2.1 (FerroCAD) + PSF (CPython)
```

The shell already resolves the facade via `FERROCAD_PYTHON_PATH` (see
`crates/ferrocad_gpui/src/python.rs`); the bundle sets it (and `PYTHONHOME`) at
`AppRun`, and the app adds its `mods/` directory through `HostConfig.mods_paths`,
so no code change is needed to point at the bundled Python and scripts. There is
no `lib/ferrocad.abi3.so` in the app: the binary registers the PyO3 module as a
built-in `ferrocad`, so the interpreter finds it without a file on disk (see §8).

### Staging and packaging today

`cargo xtask bundle` (the `xtask` crate) builds the app and stages a
**platform-neutral payload** (release by default; `--debug` for speed):

```
target/dist/ferrocad/
├── bin/ferrocad[.exe]        the app binary (PyO3 module built in)
├── python/                   FreeCAD, FreeCADGui, ferrocad_shell, ferrocad_spike
├── mods/                     workbenches
├── runtime/                  bundled CPython (when fetched)
└── LICENSES/
```

Per-platform scripts in `packaging/` wrap that payload and add the launcher
(`AppRun`, the `.app` launcher, `ferrocad.bat`):

| Platform | Script | Artifact |
| --- | --- | --- |
| Linux | `packaging/linux/appimage.sh` | `ferrocad-<arch>.AppImage` |
| macOS | `packaging/macos/app.sh` | `FerroCAD.app` (and `.dmg`) |
| Windows | `packaging/windows/portable.ps1` | portable folder + `.zip` |

Each script runs `cargo xtask bundle` first. When a `python-build-standalone`
runtime is present (fetched once with `cargo xtask python`, or forced with
`bundle --python`), it is staged as `runtime/` and the app is built against it, so
the artifact is self-contained; the launchers set `PYTHONHOME` and the library
path. Without it, the system interpreter is used. The macOS and Windows scripts
are written but not run here, because this project builds on Linux.

On a `v*` tag, `.github/workflows/release.yml` runs these three on a matrix,
verifies the tag against the workspace version, and publishes a GitHub Release
with the artifacts.

## 5. What the `ferrocad` crate delivers

- `cargo run -p ferrocad` runs the base app (the general edition).
- `cargo install ferrocad` installs a **self-contained binary**: the Python facade
  and workbenches are embedded at compile time and unpacked to the per-user data
  directory on first run (see §8). This is the one distribution path with no
  installer to place data files.
- It depends on the reusable `ferrocad_gpui` library, which boots CPython and
  provides the shell (`run_with(HostConfig)`).
- The app owns its Python: `HostConfig.entry_module` names the startup module and
  `HostConfig.python_paths` the script and mod directories. In the repository the
  base app names `ferrocad_shell`; in a bundle it names its startup scripts and
  `mods/`.
- An edition is the same shape with its own name, scripts and mods.

The Python-native route (`pip install ferrocad`, the maturin wheel) is orthogonal:
it delivers the `FreeCAD` package into an existing interpreter, not a whole
application. See [`releasing.md`](releasing.md).

## 6. Platform targets (planned)

| Target | Artifact | Tooling |
| --- | --- | --- |
| Linux | AppImage | `appimagetool` in CI; AppRun + `.desktop` |
| macOS | `.app` in a `.dmg` | `hdiutil`/`create-dmg`; codesign + notarize |
| Windows | portable `.exe` or MSIX | WiX/MSIX, or a zip |

**Status (v0.1.1).** The release workflow builds and publishes the Linux AppImage
and the macOS `.dmg`, each bundling the pinned `python-build-standalone` runtime
(the build job runs `cargo xtask python` before `bundle`, so the host is compiled
against that interpreter and the runtime ships as `runtime/`). The Windows leg is
omitted from the matrix: its packaging script (`packaging/windows/portable.ps1`)
is written, but the upstream `bite-gp-windows` 1.21.0 crate does not compile
(`unresolved import gpui`), and a failing matrix job skips the whole release.
Re-add the matrix entry once that crate builds. See `.github/workflows/release.yml`.

Two things make the runtime bundle work despite PyO3:

- PyO3's interpreter probe reads `sysconfig`, and `python-build-standalone`
  reports `LIBDIR=/install/lib` (its build prefix), so PyO3 would link a path that
  never existed on a user machine. `cargo xtask bundle` writes a PyO3 config
  (`PYO3_CONFIG_FILE`) with the relocated `lib_dir`/`lib_name`/`executable`.
- On macOS, PyO3 adds no rpath for a non-framework Python, so `packaging/macos/
  app.sh` repoints `ferrocad-bin` at the bundled `libpython` with
  `install_name_tool` (`@rpath/libpython3.14.dylib` + an
  `@executable_path/../Resources/runtime/lib` rpath) and re-signs it ad-hoc. On
  Linux the `soname` is bare, so the launcher's `LD_LIBRARY_PATH` suffices.

The macOS artifact is currently **arm64 only** (`macos-latest`), and the `.app` is
ad-hoc signed, not Developer ID signed/notarized: a DMG downloaded from the
release carries the quarantine attribute, so the first launch needs the usual
`xattr -dr com.apple.quarantine /Applications/FerroCAD.app` or a right-click Open.
Signing and notarization remain future work.

The CI jobs also install the GPUI stack's Linux system libraries
(`libfontconfig1-dev`, `pkg-config`, `libxcb1-dev`, `libxkbcommon-dev`,
`libxkbcommon-x11-dev`): the `rlib` crates need fontconfig's headers, and linking
the `ferrocad` binary additionally needs xcb and xkbcommon. The shell tests build
the PyO3 extension (`cargo build -p ferrocad_py --features extension-module`) into
`python/` first, because the `FreeCAD` facade imports it.
| Any (Python users) | wheel (`pip install ferrocad`) | maturin |

### AppImage tooling

`packaging/linux/appimage.sh` shells out to **`appimagetool`** on the staged
AppDir. It locates the tool on `PATH`, via `APPIMAGETOOL`, or as
`tools/appimagetool-<arch>.AppImage`, and runs a `.AppImage` with
`--appimage-extract-and-run`, so **no FUSE and no system install are needed**.

The canonical `AppImage/appimagetool` is written in C, as is the type-2 runtime
(the ELF preamble every AppImage needs). The Rust options were reviewed and not
adopted as the default:

| Crate | Notes |
| --- | --- |
| `appimage` | Apache-2.0/MIT library, but last updated 2022 (unmaintained) |
| `appimagetool` | MIT, but no public repository and ~455 downloads; not dependable |
| `cargo-appimage` | GPL-3.0; fine to run, but adds a GPL binary to the toolchain |
| `cargo-packager` | Apache-2.0, actively maintained, but it builds its own AppDir and is opinionated about layout |

Revisit `cargo-packager` if we want one Rust tool for AppImage, `.dmg` and `.msi`
(D6) and are willing to adapt our staged layout to its config model.

## 7. Roadmap slices

- **D1 (done).** Promote the shell to a library (`ferrocad_gpui`) and make
  `ferrocad` the base app binary; `HostConfig` carries `entry_module` and
  `python_paths`.
- **D2 (done).** A `mods/` directory sits next to the repository `python/` tree,
  and the packaging step copies both into the distribution as loose files.
- **D3.** Document the dev path: which system Python is found, and
  `FERROCAD_PYTHON_PATH`.
- **D4 (done).** `cargo xtask bundle` stages the platform-neutral payload
  (binary + extension + facade + app scripts + mods).
- **D5 (done).** `packaging/linux/appimage.sh` wraps the payload into an AppImage
  (shells out to `appimagetool`; no FUSE).
- **D6 (scripts written).** `packaging/macos/app.sh` (`.app` + `.dmg`) and
  `packaging/windows/portable.ps1` (portable folder + `.zip`); untested on their
  platforms. An MSIX/WiX installer is a later step.
- **D8 (done).** `cargo xtask python` fetches a pinned `python-build-standalone`
  runtime; `bundle` stages it as `runtime/` and builds the app against it, and the
  launchers set `PYTHONHOME`/the library path. The payload is self-contained.
- **D9 (done).** `.github/workflows/release.yml`: on a `v*` tag, verify the tag
  against the workspace version and run the tests, build the three artifacts with
  the pinned runtime, and attach them to a GitHub Release.
- **D10 (done).** `cargo xtask mods` fetches a pinned upstream FreeCAD revision
  with a blobless sparse checkout and stages the supported (pure-Python) `Mod/`
  scripts into `mods/`; the default set is `Draft`. `bundle` ships them loose.
- **D11 (done).** `cargo install ferrocad` yields a working app: the payload is
  embedded into the binary and unpacked on first run, and the PyO3 module is
  registered as a built-in so no extension file needs to sit beside the binary.
  `cargo xtask stage-payload` / `unstage-payload` stage the workspace `python/`
  and `mods/` into the app crate for `cargo publish`; `bundle` builds with
  `--no-default-features` so installers do not embed a duplicate copy. See §8.

## 8. Where the scripts live, and how a `cargo install` binary finds them

The Python sources live in one canonical tree, `python/`, at the repository root
(the maturin `python-source`). They are shared across editions, so they are **not**
moved into any crate's source tree. Packaging copies them:

- the wheel: maturin bundles `python/` (`python-source = "python"`);
- an app installer/AppImage: `cargo xtask bundle` copies `python/` and `mods/` into
  the distribution layout as loose files, and the launcher points at them with
  `FERROCAD_PYTHON_PATH` / `FERROCAD_MODS_PATH`;
- `cargo install`: there is no installer and no data-file step, so the payload is
  embedded in the binary and unpacked on first run.

The three paths share one resolution order in the shell
(`crates/ferrocad_gpui/src/python.rs::find_python_dir`), so the code never needs to
know which one it is in:

| Order | Source | Who provides it |
| --- | --- | --- |
| 1 | `FERROCAD_PYTHON_PATH` / `FERROCAD_MODS_PATH` | an installer's launcher |
| 2 | `python/` beside the executable (`<dir>/python` or `<dir>/../python`) | a bundle |
| 3 | `python/` walked up from the working directory | a development checkout |
| 4 | embedded payload, extracted to the data dir | `cargo install` |

### The `cargo install` case

Cargo installs a binary and nothing else. A `.crate` tarball is self-contained, so
`build.rs` cannot reach `../../python`; and even if it could, a binary has no place
to put loose files. So the app crate (`crates/ferrocad`) carries the payload:

- `build.rs` sets `have_embedded_python` / `have_embedded_mods` when `python/` /
  `mods/` exist beside the manifest **and** the `embedded-payload` feature is on
  (it is a default feature; `bundle` builds with `--no-default-features`);
- `include_dir!` bakes the trees into the binary;
- on startup, `ferrocad::payload::resolve` asks the shell whether orders 1-3 apply,
  and only when they do not does it unpack the embedded copy to
  `~/.local/share/ferrocad/payload/<version>/` (platform-appropriate; override with
  `FERROCAD_DATA_DIR`). The extraction is versioned and marked with `.complete`, so
  an interrupted unpack is retried and an upgrade never reuses a stale tree.

The workspace `python/` and `mods/` are not crate sources, so they are staged into
`crates/ferrocad/` only for the duration of `cargo publish`:

```
cargo xtask stage-payload     # copy python/ (and mods/) beside the app crate
cargo publish -p ferrocad     # the `include` allowlist packs them into the .crate
cargo xtask unstage-payload   # remove the copies again
```

The app crate's `Cargo.toml` lists `/python/**` and `/mods/**` in `include` (the
workspace `.gitignore` excludes them, but an explicit `include` still packs them)
and the staged directories are gitignored. `stage-payload --python-only` keeps a
publish small by leaving the multi-megabyte workbenches out. `.github/workflows/
crates.yml` runs this staging around the `ferrocad` publish and unstages even if
the job fails.

This is the FreeCAD model extended to `cargo`: the crate is self-contained, the
workbenches stay dynamic on disk (so a user can add a workbench), and the source
of truth for the scripts is still the repository `python/` tree.

### Provenance of the workbench scripts

Our own scripts (the `FreeCAD` facade shim, `FreeCADGui`, `ferrocad_shell`) live in
the repository `python/` tree and are copied into the bundle as-is.

Real workbench scripts (FreeCAD's `Mod/`) are a separate question:

- **Fetch from upstream FreeCAD at packaging time.** Pin a FreeCAD revision and
  copy the supported scripts into `mods/`. This keeps our crate clean and the
  licence boundary clear. The caveat is that most upstream mods are C++-backed
  (`Part`, `Mesh`, `Sketcher`, ...) and will not run without the geometry kernels,
  so only pure-Python workbenches are candidates for now. `cargo xtask mods`
  (D10, done) does this with a blobless sparse checkout; the default set is
  `Draft`, and `--only` picks others.
- **Carry them in the `ferrocad` crate.** The publish staging step does this: the
  fetched `mods/` tree is packed into the `.crate`, so `cargo install ferrocad`
  ships `Draft` and unpacks it on first run. The cost is the crate size (~6 MB
  compressed) and the binary size; `stage-payload --python-only` opts out.

Either way the scripts end up as loose files in `mods/` on disk, as FreeCAD does.

### Loading

At boot the host calls `load_workbenches`, which executes each `mods/*/Init.py` and
`InitGui.py`. It is **best-effort**: a script that raises is recorded with its
traceback (logged to stderr and shown in the console) and the app keeps running,
as FreeCAD isolates workbench load failures. The host passes the directories via
`HostConfig.mods_paths`, `FERROCAD_MODS_PATH`, or a `mods/` directory beside the
facade.

Today most upstream workbenches fail at load for a known reason: the facade does
not yet expose the APIs they expect (for example `FreeCAD.addImportType`, or the
`FreeCADGui.Workbench` base and command registry). They fail *individually and
harmlessly*; widening the facade is what turns them on, one API at a time.
