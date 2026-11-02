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
| conda-forge prebuilt (`occt=8.0.1=novtk_*`) | fast to obtain; a superset (extra codecs, OpenGL) | ~205 MB installed. **Pin the `novtk` build**: the `all` variant depends on VTK and pulls Python/X11 into the prefix (~3.5 GB) |
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

Fetch the same 8.0.1 prefix CI uses into the workspace once (~205 MB installed):

```sh
mkdir -p .occt
curl -sSL https://micro.mamba.pm/api/micromamba/linux-64/latest | tar -xj -C .occt
export MAMBA_ROOT_PREFIX="$PWD/.occt/root"
.occt/bin/micromamba create -y -p "$PWD/.occt/prefix" -c conda-forge 'occt=8.0.1=novtk_*'
```

Pin the build string: conda-forge publishes an `all` variant too, and that one depends on
VTK, which drags Python and X11 into the prefix (3.5 GB instead of 205 MB). The `novtk`
build still ships all 51 `TK*` libraries, including data exchange and OCAF.

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

The exception removes the legal obstacle to static linking, not the practical ones: the
shared build is what we ship, what the prefix provides, and what keeps `cargo build` to
minutes. Our fork therefore links the prefix's `.so` by default; `--features builtin`
opts into the vendored static build (it compiles the whole kernel from source).

## 7. What this leaves open

- Whether the release OCCT is our minimal shared build (§3 recommendation) or a pruned
  conda prefix; and whether to ship data exchange (and thus the imaging stack) in v1.
- Static (`builtin`) OCCT as an alternative release path: no `lib/` to bundle, one
  artifact per platform, but a slower CI and a bigger binary. It is opt-in
  (`--features builtin`); the fork's default is the prefix's shared libraries.
- The macOS closure (`install_name_tool` fix-ups, `otool`/signing). The Windows half is
  implemented and exercised; see [`edition-publishing.md`](edition-publishing.md) §6 and
  [`macos.md`](macos.md).

## 8. Why OCCT 8.0.1 (2026-11-01)

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
user code, marked deprecated. So a future major upgrade does dissolve the problem our
fork's MSVC patch works around.

**What it took** (fork tag `v0.3.0-ferrocad.23`):

- `build.rs`: C++17 (8.0's minimum) and a 7.8+/8.x version range. The OCCT probe now
  requires CMake 3.15.
- `crates/opencascade-sys/include/occt8_handle_compat.hxx`: 34 `Handle_*` aliases and 13
  container typedefs that 8.0 no longer declares, gated on `OCC_VERSION_MAJOR >= 8`.
- `class TopoDS` became `namespace TopoDS`, and `NCollection_List::Size()` returns
  `size_t`: both are bound through free wrappers, and a Rust `TopoDS` module keeps the
  call spelling.
- Verified on Linux: the fork compiles on 7.8.1 and 8.0.1, and `ferrocad_occt`'s 78 tests
  pass on both. Windows (O4) and the full geometry conformance suite are still to run.

**Pin the `novtk` build.** conda-forge publishes two 8.0.1 variants: `all` (depends on
VTK, pulling Python and X11 — 3.5 GB installed) and `novtk` (205 MB, still all 51 `TK*`
libraries). CI, packaging and the local recipe use `occt=8.0.1=novtk_*`.

**Link by versioned file name.** The bindings emit
`cargo:rustc-link-lib=dylib:+verbatim=libTKernel.so.8.0` (the name is discovered in the
probed prefix) instead of `-lTKernel`. With the plain name, search-path order decides:
a distribution's OCCT dev package (Ubuntu's `libocct-*-dev` 7.9 puts
`/usr/lib/x86_64-linux-gnu` on the link line through an unrelated crate's
pkg-config) satisfies `-lTKernel` *before* `.occt/prefix/lib`, so 8.0 headers meet 7.9
libraries and the link fails on `Standard_Failure::what()`, the new `NCollection_*`
signatures and several vtables. `-l:libTKernel.so.8.0` matches exactly one file, so the
prefix wins regardless of order; the linker still records the library's own SONAME, so
the `$ORIGIN`-relative rpath keeps working at run time. Fork `v0.3.0-ferrocad.27`.
Check with `readelf -d target/debug/ferrocad_parametric | grep TK`: every entry must
name `.so.8.0`, never `.so.7.9`.

The measured cost, the audit and what remains unverified are in
[`../../project/research/occt-8-upgrade.md`](../../project/research/occt-8-upgrade.md)
and [`../../project/research/occt-8-api-audit.md`](../../project/research/occt-8-api-audit.md).

### 8.1 The prebuilt wasm kernel

The browser build does not use the conda prefix: `wasm32-unknown-unknown` links
cadrum's prebuilt OCCT 8.0.1 (`occt-8_0_1_rev3-wasm32_unknown_unknown`, fetched into
`.occt-wasm/` by `crates/ferrocad_web/run.sh`) and must be compiled inside
`ghcr.io/lzpel/cross-wasm32-unknown-unknown` so the C++ and the kernel agree on the
wasm-EH encoding. It ships **19 headless `TK*` libraries** — `TKernel`, `TKMath`,
`TKG2d`, `TKG3d`, `TKGeomBase`, `TKGeomAlgo`, `TKBRep`, `TKTopAlgo`, `TKPrim`,
`TKBO`, `TKBool`, `TKFillet`, `TKOffset`, `TKMesh`, `TKShHealing`, `TKDE`,
`TKDECascade`, `TKDESTEP`, `TKXSBase` — plus the `cadrum_*` C++ runtime.

It omits the data-exchange and application-framework libraries the desktop prefix has
(`TKDESTL`, `TKDEIGES`, `TKCAF`, `TKLCAF`, `TKXCAF`) as well as **`TKFeat`**, which
`ferrocad_occt` needs: it binds `BRepFeat_SplitShape` for
`GeometryBackend::split_shape`, and `ferrocad_part::register(Arc<OcctBackend>)`
materialises the whole trait vtable, so that binding is reachable even when nothing
calls it. Rebuilding OCCT for that is unnecessary — `TKFeat` is 35 translation units
(`src/ModelingAlgorithms/TKFeat/{BRepFeat,LocOpe}/*.cxx`) that compile against the
kernel's own headers. `crates/ferrocad_web/run.sh` sparse-clones those sources into
`target/occt-src` (tag `V8_0_1`, ~1 MB) and compiles them in the same cross image
with the kernel's flags, writing `.occt-wasm/<kernel>/lib/libTKFeat.a`; the step is
skipped once the archive exists. The fork links `-lTKFeat` only when the archive is
present, and otherwise emits a `cargo:warning` naming this recipe instead of failing
the link with barrel-shifted `BRepFeat` symbols. A host that points `OCCT_WASM_ROOT`
at a bare cadrum tarball and builds without `run.sh` has to run that recipe itself
for `Part.makeSplitShape` to work in the browser. The Node smoke test calls the
demo's exported `split_check()` (a 1×2×2 box face cut by a shifted edge → one
direct and one left piece), so the archive is proven by behaviour and not only by
linking.

**Decision:** ship on **8.0.1** with
[`ferrocad/ferrocad_occt_cxx`](https://github.com/ferrocad/ferrocad_occt_cxx)
(`v0.3.0-ferrocad.29`), pinned from the workspace via `[patch.crates-io]`. The upstream
tracking issue is
[bschwind/opencascade-rs#237](https://github.com/bschwind/opencascade-rs/issues/237).
