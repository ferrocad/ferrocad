#!/usr/bin/env python3
"""Stage the minimal OCCT runtime FerroCAD needs, the way we stage CPython.

We bundle CPython as `runtime/` (python-build-standalone) and have the launcher set
`PYTHONHOME`. OCCT is analogous: fetch/produce a per-platform OCCT prefix once, then
stage only the shared libraries the app actually links, and have the launcher set
`LD_LIBRARY_PATH` (OCCT ships no RUNPATH; verified with `readelf -d`).

This script answers "what is the file set?" concretely:

  1. start from the toolkits FerroCAD links (the *modeling* subset -- no
     visualization, no data-exchange image codecs),
  2. walk `DT_NEEDED` transitively with `readelf` (deterministic; does not rely on
     the loader finding anything),
  3. copy every dependency that lives inside the prefix, under its exact soname,
  4. report the dependencies that are *not* in the prefix (the system libraries we
     assume the base OS provides).

Linux only for now; macOS needs `otool -L` + `install_name_tool`, Windows needs a PE
import walker. The toolkit list and closure logic carry over.

Usage:
  stage_occt.py --prefix <occt-prefix> --out <dir> [--toolkits a,b,c] [--lib-subdir lib]
"""

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path

# The toolkits `opencascade-sys` links (its build.rs OCCT_LIBS), minus the ones we
# do not want: this is the modeling subset. Visualization (TKService, TKV3d) and the
# image-codec data exchange (FreeImage/OpenEXR/GL) are deliberately excluded, so the
# bundle does not drag in FreeImage/FreeType/GL.
MODELING: list[str] = [
    "TKernel", "TKMath", "TKG2d", "TKG3d", "TKGeomBase", "TKGeomAlgo", "TKTopAlgo",
    "TKBRep", "TKPrim", "TKBO", "TKBool", "TKFillet", "TKOffset", "TKShHealing",
    "TKMesh", "TKXSBase", "TKDE", "TKDESTEP", "TKDEIGES", "TKDESTL",
    "TKLCAF", "TKCAF", "TKXCAF", "TKFeat",
]


def needed(path: Path) -> list[str]:
    """`DT_NEEDED` sonames of an ELF file, via readelf (loader-independent)."""
    try:
        out = subprocess.check_output(
            ["readelf", "-d", str(path)], stderr=subprocess.DEVNULL, text=True
        )
    except (OSError, subprocess.CalledProcessError):
        return []
    names = []
    for line in out.splitlines():
        marker = "Shared library: ["
        if marker in line and line.rstrip().endswith("]"):
            names.append(line.split(marker, 1)[1].rsplit("]", 1)[0])
    return names


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--prefix", required=True, type=Path, help="OCCT prefix (has lib/)")
    ap.add_argument("--out", required=True, type=Path, help="staging directory")
    ap.add_argument("--lib-subdir", default="lib")
    ap.add_argument(
        "--binary",
        type=Path,
        help="seed from an ELF binary's DT_NEEDED instead of toolkit names (recommended: "
        "this is exactly what the app loads, and the linker drops unused OCCT libs)",
    )
    ap.add_argument(
        "--toolkits",
        default=",".join(MODELING),
        help="comma-separated seed toolkits (without the lib/.so)",
    )
    args = ap.parse_args()

    libdir = args.prefix / args.lib_subdir
    if not libdir.is_dir():
        print(f"error: {libdir} is not a directory", file=sys.stderr)
        return 2

    # Resolve a soname to a file in the prefix.
    def resolve(soname: str) -> Path | None:
        direct = libdir / soname
        if direct.exists():
            return direct
        # A bare toolkit name (no extension): pick the highest soname revision.
        cands = sorted(libdir.glob(soname + ".so*"))
        for c in reversed(cands):
            if c.is_file() or c.is_symlink():
                return c
        return None

    if args.binary:
        seeds = [n for n in needed(args.binary) if n.startswith("lib")]
    else:
        seeds = []
        for t in args.toolkits.split(","):
            base = t.strip()
            if not base:
                continue
            if not base.startswith("lib"):
                base = "lib" + base
            if ".so" in base:
                seeds.append(base)
                continue
            # A bare toolkit name: use its highest versioned soname, not the bare symlink.
            cands = sorted(libdir.glob(base + ".so.*")) or sorted(libdir.glob(base + ".so"))
            if cands:
                seeds.append(cands[-1].name)
            else:
                print(f"warning: toolkit {base} not found in {libdir}", file=sys.stderr)
    included: dict[str, Path] = {}   # staging name -> source file
    external: dict[str, str] = {}    # soname -> resolved system path (or "not found")

    queue = list(seeds)
    while queue:
        name = queue.pop()
        if name in included:
            continue
        src = resolve(name)
        if src is None:
            # Not ours: a system library. Record where it resolves (for the report).
            if name.startswith("lib") and name not in external:
                external[name] = sys_path(name)
            continue
        # Stage under the exact soname the loader will ask for.
        included[name] = src
        for dep in needed(src):
            if dep not in included:
                queue.append(dep)

    if not included:
        print("error: no toolkits matched; check --prefix/--toolkits", file=sys.stderr)
        return 2

    # Copy. De-duplicate content by real path so all symlinks to one file cost once.
    args.out.mkdir(parents=True, exist_ok=True)
    by_real: dict[Path, str] = {}
    total = 0
    for name, src in sorted(included.items()):
        real = src.resolve()
        dst = args.out / name
        if real in by_real:
            # Same content already staged under another name: link, don't copy.
            if dst.exists() or dst.is_symlink():
                dst.unlink()
            os.link(args.out / by_real[real], dst)
        else:
            if dst.exists() or dst.is_symlink():
                dst.unlink()
            dst.write_bytes(real.read_bytes())
            by_real[real] = name
            total += real.stat().st_size

    manifest = args.out / "MANIFEST.txt"
    lines = [
        f"prefix: {args.prefix}",
        f"toolkits: {','.join(seeds)}",
        f"files: {len(included)}",
        f"bytes: {total} ({total / 1_048_576:.1f} MiB, content de-duplicated)",
        "",
        "staged (soname)",
    ]
    lines += [f"  {n}" for n in sorted(included)]
    lines += ["", "system dependencies (assumed present on the target)"]
    lines += [f"  {n} -> {p}" for n, p in sorted(external.items())]
    manifest.write_text("\n".join(lines) + "\n")

    print(f"staged {len(included)} libraries, {total / 1_048_576:.1f} MiB -> {args.out}")
    print(f"system deps assumed present: {', '.join(sorted(external)) or '(none)'}")
    print(f"manifest: {manifest}")
    return 0


def sys_path(soname: str) -> str:
    """Best-effort path of a system library, for the report only."""
    for d in ("/lib", "/usr/lib", "/usr/lib/x86_64-linux-gnu", "/lib64"):
        p = Path(d) / soname
        if p.exists():
            return str(p.resolve())
    return "not found"


if __name__ == "__main__":
    sys.exit(main())
