#!/bin/sh
# Build the Rust core(s) and package the native libraries next to the Python
# facade.
#
#   * freecad-py   -> python/FreeCAD/_core.abi3.so   (PyO3, primary)
#   * freecad-core -> python/FreeCAD/libfreecad_core.so (C ABI, ctypes fallback)
#   * fc-python    -> python/fc.abi3.so              (PyO3 over fc-core)
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

echo "== building freecad-py (PyO3) =="
cargo build --release --offline --manifest-path "$HERE/rust/freecad-py/Cargo.toml"

echo "== building freecad-core (C ABI fallback) =="
cargo build --release --offline --manifest-path "$HERE/rust/freecad-core/Cargo.toml"

echo "== building fc-python (PyO3 over fc-core) =="
cargo build --release --offline --manifest-path "$HERE/rust/fc-python/Cargo.toml"

# Package the PyO3 extension (importable as FreeCAD._core; `.abi3.so` is a
# recognised extension suffix).
py_src="$HERE/rust/freecad-py/target/release/libfreecad_py.so"
if [ -f "$py_src" ]; then
    cp "$py_src" "$HERE/python/FreeCAD/_core.abi3.so"
    echo "packaged python/FreeCAD/_core.abi3.so"
else
    echo "error: PyO3 extension not produced" >&2
    exit 1
fi

# Package the ctypes fallback shared library.
for name in libfreecad_core.so libfreecad_core.dylib freecad_core.dll; do
    src="$HERE/rust/freecad-core/target/release/$name"
    if [ -f "$src" ]; then
        cp "$src" "$HERE/python/FreeCAD/$name"
        echo "packaged python/FreeCAD/$name"
    fi
done

# Package the fc-python bindings over fc-core.
fc_src="$HERE/rust/fc-python/target/release/libfc_python.so"
if [ -f "$fc_src" ]; then
    cp "$fc_src" "$HERE/python/fc.abi3.so"
    echo "packaged python/fc.abi3.so"
fi

echo "build OK"
