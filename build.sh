#!/bin/sh
# Build the Rust core and package the shared library next to the Python facade.
#
# The toolchain can be project-local (see ../../.toolchain/env.sh) so this needs
# no root and no system-wide Rust install.
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

cargo build --release --offline --manifest-path "$HERE/rust/freecad-core/Cargo.toml"

packaged=0
for name in libfreecad_core.so libfreecad_core.dylib freecad_core.dll; do
    src="$HERE/rust/freecad-core/target/release/$name"
    if [ -f "$src" ]; then
        cp "$src" "$HERE/python/FreeCAD/$name"
        echo "packaged python/FreeCAD/$name"
        packaged=1
    fi
done

if [ "$packaged" -eq 0 ]; then
    echo "error: no shared library produced" >&2
    exit 1
fi

echo "build OK"
