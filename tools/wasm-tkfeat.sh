#!/usr/bin/env sh
# Build TKFeat, the one OCCT toolkit the prebuilt wasm kernel tarball prunes.
#
# `ferrocad_part` coerces its backend to `Arc<dyn GeometryBackend>`, which keeps
# `split_shape` (BRepFeat) in the vtable, so a link of anything downstream of
# `ferrocad_part` needs `libTKFeat.a`. OCCT is not rebuilt: the toolkit compiles
# against the kernel's own headers with the same wasi-sdk, and the caller must run
# this inside the cross image so the wasm-EH encoding matches the kernel.
#
# Callers:
#   - `tools/wasm-prepare.sh` runs it through `docker run` for the local hosts;
#   - the `wasm` CI job (already a container job) runs it directly.
#
# Environment (all optional):
#   SRC           OCCT source root; sparse-cloned here when absent
#   OUT           the wasm kernel prefix (the directory holding include/ and lib/)
#   SYSROOT       wasi-sdk sysroot
#   OCCT_GIT_TAG  the OCCT release to fetch (V8_0_1)
#
# Idempotent: an existing `$OUT/lib/libTKFeat.a` is kept.

set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${SRC:-$ROOT/target/occt-src}"
OUT="${OUT:-$ROOT/.occt-wasm/occt-8_0_1_rev3-wasm32_unknown_unknown}"
SYSROOT="${SYSROOT:-/opt/wasi-sdk/share/wasi-sysroot}"
OCCT_GIT_TAG="${OCCT_GIT_TAG:-V8_0_1}"

if [ -f "$OUT/lib/libTKFeat.a" ]; then
    exit 0
fi

if [ ! -d "$SRC/src/ModelingAlgorithms/TKFeat" ]; then
    rm -rf "$SRC"
    git -c advice.detachedHead=false clone --depth 1 --branch "$OCCT_GIT_TAG" \
        --filter=blob:none --sparse \
        https://github.com/Open-Cascade-SAS/OCCT.git "$SRC"
    git -C "$SRC" sparse-checkout set src/ModelingAlgorithms/TKFeat
fi

OBJ="$(mktemp -d)"
trap 'rm -rf "$OBJ"' EXIT
mkdir -p "$OUT/lib"

for f in "$SRC"/src/ModelingAlgorithms/TKFeat/BRepFeat/*.cxx \
         "$SRC"/src/ModelingAlgorithms/TKFeat/LocOpe/*.cxx; do
    clang++ --target=wasm32-wasip1 --sysroot="$SYSROOT" \
        -fwasm-exceptions -fexceptions -mllvm -wasm-use-legacy-eh=true \
        -D_WASI_EMULATED_PROCESS_CLOCKS -D_WASI_EMULATED_SIGNAL \
        -D_WASI_EMULATED_MMAN -D_WASI_EMULATED_GETPID \
        -D_USE_MATH_DEFINES -DFEATURE_COLOR -std=c++17 -O2 \
        -I "$OUT/include/opencascade" \
        -I "$SRC/src/ModelingAlgorithms/TKFeat" \
        -c "$f" -o "$OBJ/$(basename "$f" .cxx).o"
done
ar rcs "$OUT/lib/libTKFeat.a" "$OBJ"/*.o
