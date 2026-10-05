# Spike: bundling OCCT

How we ship OCCT with the app, modeled on how we ship CPython: fetch/produce a
per-platform runtime once, stage only what the binary loads into the payload, and have
the launcher point at it (`PYTHONHOME` for CPython, `LD_LIBRARY_PATH` for OCCT).

`stage_occt.py` is the prototype of that "stage only what's needed" step, and the
numbers it produced are the finding. See [`../../docs/occt-bundling.md`](../../docs/occt-bundling.md).

## The tool

```sh
# From an OCCT prefix (conda-forge, a source build, a system install):
python3 stage_occt.py --prefix <occt-prefix> --out <dir>

# Recommended: seed from the built binary's DT_NEEDED, so you stage exactly the
# OCCT libraries the linker kept (it drops the unused ones).
python3 stage_occt.py --prefix <pref> --out out-bin --binary target/release/ferrocad
```

It walks `DT_NEEDED` transitively with `readelf` (loader-independent), copies each
dependency that lives in the prefix under its **exact soname**, de-duplicates content
by real path, and writes a `MANIFEST.txt` of the staged files and the *system*
libraries it assumes the base OS provides. Linux only for now; macOS
(`otool -L`) and Windows (PE imports) are the platform-specific parts.

## Numbers (conda-forge OCCT 7.8.1, Linux x86_64)

| Seed | Libraries | Size | What it drags in |
| --- | --- | --- | --- |
| the spike binary's `DT_NEEDED` | 15 | 59.8 MiB | 13 OCCT libs + `libstdc++`, `libgcc_s` |
| pure modeling toolkits | 31 | 63.8 MiB | OCCT + `libstdc++`, `libgcc_s` |
| + data exchange & OCAF | 81 | 107 MiB | OpenEXR, FreeImage (+ tiff/jpeg/raw/openjpeg/webp/jpegxr/lcms2), FreeType, fontconfig, X11, OpenMP |
| whole conda OCCT lib dir | 150 | 135 MiB | the above plus every codec's dev symlinks |

Two things fall out:

1. **Seed from the binary, not the toolkit list.** `opencascade-sys`'s `build.rs` links
   23 toolkits, but the linker keeps only the ones the app references (the spike needed
   6: `TKernel`, `TKMath`, `TKBRep`, `TKPrim`, `TKFillet`, `TKBO`). Seeding from
   `DT_NEEDED` stages 15 files instead of 81.
2. **Visualization and image codecs arrive through data exchange.** `TKXCAF` (needed
   for STEP/IGES colours) hard-depends on `TKV3d` → `TKService` → FreeImage/FreeType/X11.
   So any app that links `TKDESTEP`/`TKDEIGES`/`TKXCAF` pays for the whole imaging stack,
   unless we build OCCT ourselves without it. A geometry-only build avoids it entirely
   (the 60 MiB row has no GL, no FreeImage, no X11).

The staged runtime is real: the spike binary runs against `out-bin/` with only
`LD_LIBRARY_PATH` set, and produces the same element map as against the prefix.

## Why `LD_LIBRARY_PATH`

OCCT ships **no `RUNPATH`** (`readelf -d libTKBRep.so.7.8` shows none), so the loader
cannot find its siblings on its own. That is exactly the CPython situation with
`PYTHONHOME`: the launcher must set the environment, and `AppRun` already has the hook.
