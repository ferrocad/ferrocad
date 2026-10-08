# Publishing an edition: FerroCAD: Parametric

Status: plan (2026-10-08). Companion to [`repackaging.md`](repackaging.md) (the
edition model), [`distribution.md`](distribution.md) (the payload/artifact
mechanics) and [`occt-bundling.md`](occt-bundling.md) (the kernel closure).

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

1. **The binary.** `cargo xtask bundle` currently builds `-p ferrocad`. It needs
   an edition selector (`--edition parametric` → `-p ferrocad_parametric`), and it
   stages that binary as `bin/ferrocad` so the launchers stay shared.
2. **The OCCT closure.** `xtask occt` (planned, [`occt-bundling.md`](occt-bundling.md)
   §5) produces/stages a per-platform prefix at `target/occt-runtime/<triple>/`.
   `bundle --occt` walks the built binary's `DT_NEEDED` (`otool -L` / PE imports on
   the other platforms), copies each library that lives in the prefix under its
   exact soname into `lib/`, and records a `MANIFEST`.
3. **The launcher library path.** OCCT ships no `RUNPATH`, so the launchers must add
   `lib/` beside the existing `PYTHONHOME`:
   - `packaging/linux/AppRun`: `LD_LIBRARY_PATH="$APPDIR/lib:$APPDIR/runtime/lib:…"`.
   - `packaging/macos/app.sh`: `Contents/Frameworks` + `install_name_tool` fixups.
   - `packaging/windows/portable.ps1`: the `.dll`s sit beside `ferrocad.exe`.
4. **The build-time prefix.** The same prefix compiles the crates
   (`OpenCASCADE_DIR`, `OCCT_INCLUDE_DIR`) and is bundled, so there is one source
   of truth. Locally that is the OCCT 7.8.1 conda prefix the geometry CI already
   uses.
5. **Licenses.** Stage OCCT's license text and its exception under `LICENSES/`,
   as CPython's PSF text already is.
6. **The release matrix.** `release.yml` builds `ferrocad` today. Add a second
   artifact set named for the edition (`ferrocad-parametric-<os>`), or make the
   edition the default download and keep the base for `cargo install`.

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
  `v*` tag yields six artifacts; the Windows one passes `--check-init`.
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

## 6. What does not change

The Python facade, the `mods/` workbenches and the CPython runtime bundling are
identical for both editions — only the binary and the added `lib/` differ. So the
payload layout in [`distribution.md`](distribution.md) §4 holds, with `lib/`
finally populated.
