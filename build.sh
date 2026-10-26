#!/bin/sh
# Build the FerroCAD Rust crates and package the native libraries next to the
# Python facade (python/FreeCAD).
#
#   * ferrocad_py     -> python/ferrocad.abi3.so               (PyO3 over ferrocad_core, primary)
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

echo "== building ferrocad_core / ferrocad_py / ferrocad_gen =="
# The workspace `default-members` selects exactly these four crates. The
# `extension-module` feature (opt-in, see the crates' manifests) makes PyO3 skip
# linking libpython, which is what a packaged extension module needs.
cargo build --release --offline --manifest-path "$HERE/Cargo.toml" \
    --features ferrocad_py/extension-module,ferrocad_gen/extension-module

# Package the primary PyO3 extension (importable as `ferrocad`).
fc_src="$TARGET/libferrocad_py.so"
if [ -f "$fc_src" ]; then
    cp "$fc_src" "$HERE/python/ferrocad.abi3.so"
    echo "packaged python/ferrocad.abi3.so"
else
    echo "error: ferrocad_py extension not produced" >&2
    exit 1
fi

# Package the generated skeleton bindings (M3c).
gen_src="$TARGET/libferrocad_gen.so"
if [ -f "$gen_src" ]; then
    cp "$gen_src" "$HERE/python/ferrocad_gen.abi3.so"
    echo "packaged python/ferrocad_gen.abi3.so"
fi

# Package the modern-stack Python surface (`ferrocad.stage`), importable as the
# top-level `ferrocad_stage` extension module. Built separately because it is not a
# workspace `default-member`.
echo "== building ferrocad_stage_py (ferrocad.stage) =="
cargo build --release --offline --manifest-path "$HERE/Cargo.toml" \
    -p ferrocad_stage_py --features ferrocad_stage_py/extension-module

stage_src="$TARGET/libferrocad_stage_py.so"
if [ -f "$stage_src" ]; then
    cp "$stage_src" "$HERE/python/ferrocad_stage.abi3.so"
    echo "packaged python/ferrocad_stage.abi3.so"
fi

# Package the `.ui` proxy layer (`FreeCADGui.PySideUic.loadUi`, Q2a).
echo "== building ferrocad_ui_py (FreeCADGui.PySideUic) =="
cargo build --release --offline --manifest-path "$HERE/Cargo.toml" \
    -p ferrocad_ui_py --features ferrocad_ui_py/extension-module

ui_py_src="$TARGET/libferrocad_ui_py.so"
if [ -f "$ui_py_src" ]; then
    cp "$ui_py_src" "$HERE/python/ferrocad_ui_py.abi3.so"
    echo "packaged python/ferrocad_ui_py.abi3.so"
fi

# Package the combined App/Part/Mesh/MeshPart extension. Unlike the standalone
# `ferrocad.abi3.so`, one `.so` links every module, so `import Part`,
# `import Mesh` and `import FreeCAD` share one document registry (see
# docs/architecture/occt-integration.md §4). It needs OCCT; a workspace without an
# OCCT prefix keeps the core-only bindings and the Part/Mesh shims report the gap.
if [ -d "$HERE/.occt/prefix" ] || [ -n "${OpenCASCADE_DIR:-}" ] \
    || [ -n "${OCCT_INCLUDE_DIR:-}" ] || [ -n "${OCCT_ROOT:-}" ] || [ -n "${CASROOT:-}" ]; then
    echo "== building ferrocad_combined_py (App + Part + Mesh) =="
    if cargo build --release --offline --manifest-path "$HERE/Cargo.toml" \
        -p ferrocad_combined_py --features ferrocad_combined_py/extension-module; then
        combined_src="$TARGET/libferrocad_combined_py.so"
        if [ -f "$combined_src" ]; then
            cp "$combined_src" "$HERE/python/ferrocad_combined.abi3.so"
            echo "packaged python/ferrocad_combined.abi3.so"
        fi
    else
        echo "warning: ferrocad_combined_py not built; Part/Mesh stay unavailable" >&2
    fi
else
    echo "== skipping ferrocad_combined_py (no OCCT prefix) =="
fi

echo "build OK"
