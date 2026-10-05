# Bundling OCCT

Status: spike report (2026-10-06), companion to [`distribution.md`](distribution.md)
(the CPython/bundling model) and [`occt-history-spike.md`](occt-history-spike.md)
(why OCCT, and the binding). Prototype: [`../spikes/occt-bundling/`](../spikes/occt-bundling/).

We bundle OCCT the way we bundle CPython: **fetch/produce a per-platform runtime once,
stage only what the binary loads, and have the launcher point at it.** No user installs
OCCT, and `cargo build` never depends on a system OCCT.

## 1. The CPython model, applied

| | CPython (existing) | OCCT (this spike) |
| --- | --- | --- |
| Source | python-build-standalone tarball | a per-platform OCCT prefix (get it one of three ways, §3) |
| Fetch | `cargo xtask python` → `target/python-runtime/<triple>/` | `xtask occt` (planned) → `target/occt-runtime/<triple>/` |
| Staged as | `runtime/` (libpython + stdlib) | `lib/` (the toolkit `.so`s) |
| Launcher sets | `PYTHONHOME` | `LD_LIBRARY_PATH` (`DYLD_LIBRARY_PATH` / `PATH` elsewhere) |
| License file | PSF | OCCT LGPL-2.1-with-exception |
| `cargo install` path | payload embedded, unpacked on first run | same, OCCT libs included in the payload |

The staged payload grows by the toolkit set only — not by all of OCCT.

## 2. What gets staged (the closure)

The correct seed is the **built binary's `DT_NEEDED`**, not the toolkit list in
`opencascade-sys`'s `build.rs`: the linker keeps only the toolkits the app references.
[`spikes/occt-bundling/stage_occt.py`](../spikes/occt-bundling/stage_occt.py) walks
`DT_NEEDED` transitively and copies each dependency that lives in the prefix, under its
exact soname.

Measured against conda-forge OCCT 7.8.1 on Linux x86_64:

| Seed | Libraries | Size | Note |
| --- | --- | --- | --- |
| spike binary `DT_NEEDED` | 15 | 59.8 MiB | 13 OCCT + `libstdc++`, `libgcc_s`; verified to run |
| pure modeling toolkits | 31 | 63.8 MiB | no GL, no FreeImage, no X11 |
| + data exchange & OCAF | 81 | 107 MiB | pulls TKV3d/TKService → OpenEXR, FreeImage, FreeType, X11, OpenMP |
| whole conda `lib/` | 150 | 135 MiB | every codec's dev symlinks |

Two findings worth keeping:

- **Seed from the binary.** It drops the difference between "23 toolkits linked" and
  "6 toolkits used".
- **Data exchange drags visualization.** `TKXCAF` hard-depends on `TKV3d` → `TKService`
  → FreeImage/FreeType/X11. STEP/IGES colours cost the whole imaging stack unless we
  build OCCT ourselves without it. A geometry-only build is ~60 MiB and has none of it.

Transport: OCCT ships **no `RUNPATH`** (`readelf -d libTKBRep.so.7.8` shows none), so the
launcher must set the library path — the same shape as `PYTHONHOME`.

## 3. Where OCCT comes from

Same three options as before, now with a bundling lens:

| Source | Fit | Cost |
| --- | --- | --- |
| **our own minimal shared build** (CI, cached) | best: we choose the modules and options, skip FreeImage/OpenGL/FreeType/OpenEXR | a one-time build per platform (slow), then cached like python-build-standalone |
| conda-forge prebuilt (`occt=7.8.1`) | fast to obtain; a superset (visualization + codecs) | minutes to fetch; heavier bundle unless pruned |
| system packages (apt/brew/vcpkg) | dev convenience | not redistributable-complete; version drift |

**Recommendation:** produce **our own minimal shared OCCT** in a dedicated CI job and
publish it as a per-platform artifact, mirroring python-build-standalone. Then the same
prefix is used to *compile* (`OpenCASCADE_DIR`, `OCCT_INCLUDE_DIR`) and to *bundle*
(`xtask occt`). conda-forge stays the fast local path (§3 of
[`occt-history-spike.md`](occt-history-spike.md#7-the-rust-binding-spike)), not the
release source. The minimal build drops the data-exchange→visualization chain: if we do
not need STEP colours yet, we can ship geometry first and add data exchange later,
accepting the imaging stack only when it is asked for.

## 4. Payload layout

```
ferrocad.AppImage
├── AppRun                  sets PYTHONHOME + LD_LIBRARY_PATH, exec bin/ferrocad
├── bin/ferrocad
├── lib/                    OCCT toolkit .so   (new; AppRun adds it to LD_LIBRARY_PATH)
├── python/FreeCAD/         the pure-Python facade
├── mods/                   workbenches
├── runtime/                python-build-standalone (libpython + stdlib)
└── LICENSES/               LGPL-3.0 (FerroCAD) + PSF (CPython) + OCCT exception
```

`lib/` rather than nesting under `runtime/`, because `runtime/` is CPython's own
prefix. On macOS the OCCT `.dylib`s go under `Contents/Frameworks/` and get
`install_name_tool` fixups; on Windows the `.dll`s sit beside `ferrocad.exe`. The
closure logic is identical; only the import walker differs (`otool -L` vs PE imports).

## 5. Launcher and `xtask`

- **`xtask occt`** (planned): ensure a prefix exists at `target/occt-runtime/<triple>/`
  — fetch the published artifact, or build it from `occt-sys`'s bundled source with
  `BUILD_LIBRARY_TYPE=Shared` and the `USE_*` options off.
- **`xtask bundle`** stages it: run the closure on the built `bin/ferrocad`, copy the
  result to `lib/`, and record a `MANIFEST`. This is the same "stage the payload" step
  that already copies `runtime/`.
- **`AppRun` / `.app` launcher / `ferrocad.bat`** export the library path next to the
  existing `PYTHONHOME`.
- **`cargo install` self-extractor** (the `ferrocad` crate's embedded payload) must
  include `lib/` too, or the installed binary will not find OCCT.

Build-time env in CI/dev is `OpenCASCADE_DIR` (CMake config) and `OCCT_INCLUDE_DIR`
(the sibling bridge's header path); runtime is the library path. All three are set from
the same fetched prefix, so there is one source of truth.

## 6. License

OCCT is **LGPL-2.1-or-later with an exception** permitting static/dynamic linking
without the LGPL's usual relinking obligation. It combines cleanly with FerroCAD's
LGPL-3.0-or-later (see [`rewrite-strategy.md`](rewrite-strategy.md)). Ship the OCCT
license text (and the exception) under `LICENSES/`, exactly as we ship the PSF text for
CPython. Confirm the exact exception wording at adoption time.

## 7. What this leaves open

- Whether the release OCCT is our minimal shared build (§3 recommendation) or a pruned
  conda prefix; and whether to ship data exchange (and thus the imaging stack) in v1.
- Static (`builtin`) OCCT as an alternative release path: no `lib/` to bundle, one
  artifact per platform, but a slower CI and a bigger binary.
- macOS/Windows closure implementations (`install_name_tool` fix-ups; DLL search order).
