# Publishing an edition: FerroCAD: Parametric

Status: plan (2026-10-08); E1/E2 implemented, E3 partial (2026-10-25).
Companion to [`repackaging.md`](repackaging.md) (the edition model),
[`distribution.md`](distribution.md) (the payload/artifact mechanics) and
[`occt-bundling.md`](occt-bundling.md) (the kernel closure).

`ferrocad_parametric` runs from a source checkout (`cargo run -p
ferrocad_parametric`), but it is not shipped. This note settles how the first
edition reaches users.

## 1. What "publishing an edition" means

An edition is an application binary, not a crate ([`repackaging.md`](repackaging.md)
§4a), so it is **not** on crates.io. It ships the way the base app does: a
self-contained desktop artifact on the GitHub Release, plus (optionally, later) a
wheel. The distinction from the base artifact is not branding — it is a different
**Rust composition**:

| | base `ferrocad` | `ferrocad` edition (Parametric) |
| --- | --- | --- |
| binary crate | `ferrocad` | `ferrocad_parametric` |
| built-in modules | `ferrocad` | `ferrocad` + `Part` |
| links OCCT | no | yes (`ferrocad_occt` via `ferrocad_part_py`) |
| payload `lib/` | empty | the OCCT toolkit closure (~60 MiB) |
| `import Part` | unavailable | works, one core with `import FreeCAD` |
| license files | LGPL + PSF | + OCCT (LGPL-2.1 **with exception**) |

An edition that only changes config, scripts or branding (no new Rust module) can
share a binary and select a `HostConfig` at boot. `Parametric` is not one of those:
`Part` is a *linked* module, so it needs its own binary. That is the per-edition
"kernel composition" boundary `occt-integration.md` §4 draws.

## 2. The moving parts

Everything below already exists for the base app; the edition extends each.

1. **The binary.** `cargo xtask bundle` builds `-p ferrocad`; the edition selector
   (`--edition parametric`) builds `-p ferrocad_parametric` and stages that binary
   as `bin/ferrocad` so the launchers stay shared.
2. **The OCCT closure.** `xtask occt` ([`occt-bundling.md`](occt-bundling.md) §5)
   resolves the per-platform OCCT 7.8.1 prefix (or fetches it with micromamba) and
   records it under `target/occt-runtime/<triple>/`. `bundle --occt` walks the
   built binary's `DT_NEEDED` (`readelf -d` on Linux, the PE import table on
   Windows; macOS is blocked, see §6), copies each library that lives in the
   prefix under its exact soname into `lib/`, and records a `MANIFEST`.
3. **The launcher library path.** OCCT ships no `RUNPATH`, so the launchers add
   `lib/` beside the existing `PYTHONHOME`:
   - `packaging/linux/AppRun`: `LD_LIBRARY_PATH="$APPDIR/lib:$APPDIR/runtime/lib:…"`.
   - `packaging/macos/app.sh`: `Contents/Frameworks` + `install_name_tool` fixups
     (blocked with macOS; see the status section).
   - `packaging/windows/portable.ps1`: the `.dll`s sit beside `ferrocad.exe` and
     `ferrocad.bat` puts `bin/`/`lib/` on `PATH`.
4. **The build-time prefix.** The same prefix compiles the crates
   (`OpenCASCADE_DIR`, `OCCT_INCLUDE_DIR`) and is bundled, so there is one source
   of truth. Locally that is the OCCT 7.8.1 conda prefix the geometry CI already
   uses.
5. **Licenses.** Stage OCCT's license text and its exception under `LICENSES/`,
   as CPython's PSF text already is.
6. **The release matrix.** `release.yml` builds `ferrocad` and, in the `edition`
   job, the Parametric artifact set named for the edition
   (`ferrocad-parametric-<os>`); the edition is not the default download (yet).

## 3. Naming

- Artifact: `ferrocad-parametric-x86_64.AppImage`, `FerroCAD-Parametric.dmg`,
  `ferrocad-parametric-windows-x86_64.zip`.
- Window title: `FerroCAD: Parametric` (already set in the edition's `main`).
- The payload keeps `bin/ferrocad` (generic) so `AppRun`/`.app`/`ferrocad.bat`
  need no edition-specific path; only the outer artifact name identifies it.

## 4. Slices

Numbered like the `D` slices in [`distribution.md`](distribution.md) §7.

- **E1.** `xtask occt` + `bundle --edition <name> --occt` + the Linux `AppRun`
  library path. Build a local AppImage from the edition and run `--check-init`
  against the unpacked payload, then open a window, on Linux. *Acceptance:* the
  AppImage boots with `import Part` available and no system OCCT installed.
- **E2.** `release.yml` builds `ferrocad-parametric-*` beside `ferrocad-*`, fetching
  the per-platform OCCT prefix, and attaches both to the Release. *Acceptance:* a
  `v*` tag yields six artifacts (four while macOS is blocked, three while the
  Parametric Windows build is blocked too — see §6); the Windows one passes
  `--check-init`.
- **E3.** macOS and Windows closures (`otool -L`/`install_name_tool`; PE import
  walk) and the license staging for all three. *Acceptance:* each platform's
  `--check-init` runs with only the bundled `lib/`.
- **E4 (optional).** A wheel that bundles Part (`pip install ferrocad[parametric]`),
  subject to manylinux OCCT constraints — a separate, heavier question.

## 5. Decisions to make

- **OCCT source.** conda-forge (fast, a superset: data exchange drags
  FreeImage/OpenGL/FreeType/X11, ~107 MiB) vs. our own minimal shared build
  (~60 MiB geometry-only, [`occt-bundling.md`](occt-bundling.md) §3). Recommend
  shipping geometry-only first and adding data exchange when a workbench needs it.
- **Add or replace artifacts.** Ship both base and parametric (users pick), or make
  the parametric edition the default download. Recommend shipping both for one
  release, then deciding from download counts.
- **`cargo install`.** Now that `ferrocad_part_py` is on crates.io, the base app
  *could* gain an optional `part` feature (`cargo install ferrocad --features part`,
  needs a system OCCT). Recommend not yet: the artifacts are the supported path,
  and a kernel is not something `cargo install` should silently require.
- **Size budget.** ~60–80 MiB more per artifact for geometry-only OCCT.

## 6. Implementation status (2026-10-25)

**E1 — implemented.** `xtask` gained `occt` and the edition-aware `bundle`:

```sh
cargo xtask occt [--occt-prefix DIR] [--offline]
cargo xtask bundle --edition parametric --occt [--check-init]
cargo xtask bundle --edition parametric --occt-prefix DIR [--check-init]
```

- `--edition parametric` builds `ferrocad_parametric` and stages it as the
generic `bin/ferrocad` in `target/dist/ferrocad-parametric/`. `--occt` resolves
the pinned 7.8.1 prefix (or fetches it with micromamba; `--offline` never does)
and walks the binary's closure: `readelf -d` for `DT_NEEDED`, `ldd` for
resolution. Prefix-owned libraries are copied under their exact soname into
`lib/`, a `MANIFEST` records them, and OCCT's license and linking exception are
copied under `LICENSES/`. `packaging/linux/AppRun` puts `lib/` on
`LD_LIBRARY_PATH`.
- The walk refuses to bundle a binary linked against a different OCCT than the
prefix (for example a system 7.9 beside the 7.8.1 prefix), instead of silently
producing an artifact with missing libraries.
- `--check-init` runs the staged `bin/ferrocad --check-init` with only the staged
`lib/` and `runtime/lib` on the loader path. It never opens a window. The edition
entry point implements the flag and imports its app script, which imports
`Part`, so the check exercises the kernel and the display hooks, not just the
shared shell.
- `packaging/linux/appimage.sh --edition parametric --occt` wraps that payload as
`ferrocad-parametric-x86_64.AppImage`; the base invocation and artifact name are
unchanged.

**E2 — implemented and run for `v0.1.4`.** `.github/workflows/release.yml` has an
`edition` job that fetches OCCT 7.8.1 from conda-forge with micromamba, runs
`cargo xtask mods` and `cargo xtask python`, then
`bundle --edition parametric --occt --check-init` and the platform packager. It
uploads `ferrocad-parametric-x86_64.AppImage`; the release job waits for both the
base and edition jobs. Because macOS is blocked and the **Windows leg does not
build**, a `v*` tag yields **three** artifacts (two base, one parametric), not
six.

**Windows is blocked in the OCCT bridge.** Tracked as
[ferrocad#1](https://github.com/ferrocad/ferrocad/issues/1) (with the full
diagnosis, the attribution and the option table). The first `v0.1.4` run got
through the OCCT fetch and into `cargo build -p ferrocad_parametric` on
`windows-latest`, where `opencascade-sys 0.3.0`'s cxx bridge fails with C2440. The
cause is that OCCT's `DEFINE_STANDARD_HANDLE` makes `Handle_T` a **derived class**
on MSVC (for C++/CLI export) while `Handle(C)` stays `opencascade::handle<C>`, so
the two are distinct types there and identical on GCC/Clang.

Two instances, one fixed:

- `third_party/opencascade-sys` patches 17 allocations in six headers to construct
  `new Handle_T(…)` instead of `new opencascade::handle<T>(…)` (valid under both
  definitions, and the crate's own idiom elsewhere). `fc_curve.hxx` in this
  workspace had the same pattern once and is fixed in place.
- **Open:** cxx's `#[Self = "Type"]` shims take the member's address first
  (`void (*f)(const TopoDS_Face&, const Handle_Poly_Triangulation&) =
  &::BRepLib_ToolTriangulatedShape::ComputeNormals;`), so they only resolve when
  the bridge's `Handle_T` is OCCT's `Handle(T)`. Every such binding whose
  signature mentions a handle fails on MSVC with `cannot convert from
  'overloaded-function'`. The candidate fixes (force OCCT's compatibility
  typedef in the bridge translation units; wrap the affected functions as free
  functions; report upstream) are listed in the TODO backlog.

Until a real `windows-latest` run is green, the edition matrix stays Linux-only.
`.github/workflows/edition-windows.yml` is the manual check
(`gh workflow run edition-windows.yml`): OCCT fetch, `cargo build -p
ferrocad_occt`, `bundle --edition parametric --occt --check-init`, the portable
zip and its smoke test. When it passes, add the Windows entry back to the matrix
and delete that file; the PE closure walker and `ferrocad.bat` are already
ready.

**E3 — partial.** The Windows closure is implemented with a dependency-free PE
import reader (`crates/xtask/src/pe.rs`, unit-tested on a synthetic PE image);
`bundle --occt` copies the OCCT DLLs beside `ferrocad.exe` and `ferrocad.bat`
puts `bin/` (and `lib/`, for an alternate layout) on `PATH`. The Windows job has
not run in this sandbox, so the PE walker and that leg are **untested against a
real Windows binary**. OCCT license staging is shared by every platform. macOS
remains deliberately blocked on upstream `bite-gp-apple 1.24.402` (it imports
`cocoa` without declaring it, and the import is dead); `xtask` reports that and
does not attempt an `otool`/`install_name_tool` walk. The diagnosis, the temporary
vendored patch (`third_party/bite-gp-apple`) and the full re-enable checklist are
in [`macos.md`](macos.md); the upstream report is
[bite-gpui/bite-gpui#14](https://github.com/bite-gpui/bite-gpui/issues/14). Do not
re-enable the macOS release leg until a fixed GPUI fork lands and the checklist's
signing/closure work exists.

E4 (the bundled wheel) is still out of scope.

## 7. What does not change

The Python facade, the `mods/` workbenches and the CPython runtime bundling are
identical for both editions — only the binary and the added `lib/` differ. So the
payload layout in [`distribution.md`](distribution.md) §4 holds, with `lib/`
finally populated.
