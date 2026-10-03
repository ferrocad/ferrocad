#!/bin/sh
# Build the FerroCAD Rust crates and package the native libraries next to the
# Python facade (python/FreeCAD).
#
#   * ferrocad_py     -> python/ferrocad.abi3.so               (PyO3 over ferrocad_core, primary)
#   * ferrocad_ctypes -> python/FreeCAD/libferrocad_ctypes.so   (C ABI, ctypes fallback)
#   * ferrocad_gen    -> python/ferrocad_gen.abi3.so            (generated skeleton, M3c)
#
# The toolchain can be project-local (see ../.toolchain/env.sh) so this needs no
# root and no system-wide Rust install.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Pick up the project-local toolchain if it exists.
if [ -f "$HERE/../.toolchain/env.sh" ]; then
    . "$HERE/../.toolchain/env.sh"
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found." >&2
    echo "Install Rust (https://rustup.rs) or source the project toolchain env." >&2
    exit 1
fi

# Allow building the abi3 extension against a CPython newer than PyO3's
# officially supported maximum (this sandbox runs CPython 3.14).
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1
export PYO3_USE_ABI3_FORWARD_COMPATIBILITY

TARGET="$HERE/target/release"

echo "== building ferrocad_core / ferrocad_py / ferrocad_gen / ferrocad_ctypes =="
# The workspace `default-members` selects exactly these four crates.
cargo build --release --offline --manifest-path "$HERE/Cargo.toml"

# Package the primary PyO3 extension (importable as `ferrocad`).
fc_src="$TARGET/libferrocad_py.so"
if [ -f "$fc_src" ]; then
    cp "$fc_src" "$HERE/python/ferrocad.abi3.so"
    echo "packaged python/ferrocad.abi3.so"
else
    echo "error: ferrocad_py extension not produced" >&2
    exit 1
fi

# Package the ctypes fallback shared library.
for name in libferrocad_ctypes.so libferrocad_ctypes.dylib ferrocad_ctypes.dll; do
    src="$TARGET/$name"
    if [ -f "$src" ]; then
        cp "$src" "$HERE/python/FreeCAD/$name"
        echo "packaged python/FreeCAD/$name"
    fi
done

# Package the generated skeleton bindings (M3c).
gen_src="$TARGET/libferrocad_gen.so"
if [ -f "$gen_src" ]; then
    cp "$gen_src" "$HERE/python/ferrocad_gen.abi3.so"
    echo "packaged python/ferrocad_gen.abi3.so"
fi

echo "build OK"
