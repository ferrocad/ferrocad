# Bundling OCCT

Status: spike report (2026-10-06), companion to [`distribution.md`](distribution.md)
(the CPython/bundling model) and [`../../project/research/occt-history-spike.md`](../../project/research/occt-history-spike.md)
(why OCCT, and the binding). Prototype: [`../../../spikes/occt-bundling`](../../../spikes/occt-bundling).

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
[`spikes/occt-bundling/stage_occt.py`](../../../spikes/occt-bundling/stage_occt.py) walks
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
[`../../project/research/occt-history-spike.md#7-the-rust-binding-spike`](../../project/research/occt-history-spike.md#7-the-rust-binding-spike)), not the
release source. The minimal build drops the data-exchange→visualization chain: if we do
not need STEP colours yet, we can ship geometry first and add data exchange later,
accepting the imaging stack only when it is asked for.

### 3.1 Local development: no exports needed

Fetch the same 7.8.1 prefix CI uses into the workspace once:

```sh
mkdir -p .occt
curl -sSL https://micro.mamba.pm/api/micromamba/linux-64/latest | tar -xj -C .occt
export MAMBA_ROOT_PREFIX="$PWD/.occt/root"
.occt/bin/micromamba create -y -p "$PWD/.occt/prefix" -c conda-forge occt=7.8.1
```

`.cargo/config.toml` then points `OpenCASCADE_DIR`, `OCCT_INCLUDE_DIR` and
`LD_LIBRARY_PATH` at `.occt/prefix` with `relative = true`, so
`cargo build -p ferrocad_parametric` and `cargo run -p ferrocad_parametric` work
without exporting anything:

```toml
[env]
OpenCASCADE_DIR = { value = ".occt/prefix/lib/cmake/opencascade", relative = true }
OCCT_INCLUDE_DIR = { value = ".occt/prefix/include/opencascade", relative = true }
CMAKE_POLICY_VERSION_MINIMUM = "3.5"
LD_LIBRARY_PATH = { value = ".occt/prefix/lib", relative = true }
```

Cargo does not read a `.env` file; `[env]` is the equivalent. A variable already set
in the shell wins, which keeps a custom or system OCCT easy to use.

`ferrocad_occt/build.rs` is also self-discovering: with `OCCT_INCLUDE_DIR` unset it
tries `OCCT_ROOT`, `CASROOT`, `OpenCASCADE_DIR`, the workspace `.occt/prefix`, and
common system prefixes (`/usr`, `/usr/local`, `/opt/homebrew`, `/opt/local`). It
falls back with a warning if `OCCT_INCLUDE_DIR` is set but does not contain
`Standard_Version.hxx`, and its failure message names the expected directory. If a
direct `target/debug/ferrocad_parametric` (rather than `cargo run`) cannot find the
TK libraries, add `.occt/prefix/lib` to `LD_LIBRARY_PATH`; OCCT ships no RUNPATH.

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
LGPL-3.0-or-later (see [`rewrite-strategy.md`](../../project/direction/rewrite-strategy.md)). Ship the OCCT
license text (and the exception) under `LICENSES/`, exactly as we ship the PSF text for
CPython. Confirm the exact exception wording at adoption time.

## 7. What this leaves open

- Whether the release OCCT is our minimal shared build (§3 recommendation) or a pruned
  conda prefix; and whether to ship data exchange (and thus the imaging stack) in v1.
- Static (`builtin`) OCCT as an alternative release path: no `lib/` to bundle, one
  artifact per platform, but a slower CI and a bigger binary.
- The macOS closure (`install_name_tool` fix-ups, `otool`/signing). The Windows half is
  implemented and exercised; see [`edition-publishing.md`](edition-publishing.md) §6 and
  [`macos.md`](macos.md).

## 8. Why 7.8.1, and what OCCT 8.x would need (2026-10-25)

OCCT 8.0 exists — `V8_0_0` (2026-05-07) and `V8_0_1` (2026-07-30) — and conda-forge
publishes both for `linux-64`, `linux-aarch64`, `linux-ppc64le`, `osx-64`,
`osx-arm64` **and `win-64`**. Availability is not the problem.

**8.0 does remove the legacy handle aliases.** In 7.8.1 every class header ends with
`DEFINE_STANDARD_HANDLE(Geom_Surface, Geom_Geometry)`, which on MSVC defines
`Handle_Geom_Surface` as a *derived class* (the C++/CLI export hack) instead of the
alias it is on GCC/Clang — the type split that made the Windows build fail. In
`V8_0_1`, `Geom_Surface.hxx`, `Geom_Curve.hxx` and `Poly_Triangulation.hxx` no longer
invoke the macro at all; the API spells handles `occ::handle<T>` (a new alias for
`opencascade::handle<T>` in `Standard_Handle.hxx`). The macro itself survives only for
user code, marked deprecated. So a future major upgrade does dissolve the problem the
vendored `opencascade-sys` patch works around.

**It cannot be adopted today.** `opencascade-sys 0.3.0` is written against 7.8:

- `build.rs` pins `const OCCT_VERSION: (u8, u8) = (7, 8)` and panics when the found
  header's major version is not 7, so a conda 8.0 prefix is rejected outright;
- its bridge headers name `Handle_Geom_Surface`, `Handle_Poly_Triangulation`, …, which
  8.0 no longer defines anywhere, so the bridge would not compile even without the
  version gate;
- upstream has no OCCT 8 support and no tracking issue (latest release 0.3.0).

An 8.x migration is therefore a **binding port**, not a version bump: every
handle-typed bridge signature in `opencascade-sys` moves to `occ::handle<T>`/`Handle(T)`
(which is the same rewrite as PR #234's alternative, but mandatory and wider), plus
whatever else 8.0 removed that we cannot see until the crate compiles. Our own layer is
small — `crates/ferrocad_occt/include` uses six `Handle_Geom_Curve` and 27 deprecated
`Standard_Boolean`/`Standard_True`/`Standard_False` spellings, which are warnings only —
but the geometry conformance suite would have to be re-measured against 8.0 anyway.

**Decision:** release on 7.8.1 with the vendored `opencascade-sys` (0.3.0 + upstream
PR #234; see [`edition-publishing.md`](edition-publishing.md) §6). Revisit OCCT 8 when a
published `opencascade-sys` supports it — at that point both `third_party/` patches
(handle aliases and the MSVC bridge) should be droppable in one step. Tracked upstream
as [bschwind/opencascade-rs#237](https://github.com/bschwind/opencascade-rs/issues/237),
which asks for 8.x support and points out that spelling the bridge's handles
`opencascade::handle<T>` (what 8.0 uses, and what `occ::handle<T>` aliases) fixes MSVC
*and* 8.0 in one change; we offered to run an `occt=8.0.1` CI leg against a branch to
measure what else 8.0 removed.
