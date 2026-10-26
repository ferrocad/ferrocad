#!/bin/sh
# Run the headless hello world against the Rust-backed FreeCAD module.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PYTHONPATH="$HERE/python${PYTHONPATH:+:$PYTHONPATH}"
export PYTHONPATH
# `python/ferrocad_image.abi3.so` links OCCT, which ships no RUNPATH. The app
# launcher sets this from the packaged `lib/`; a development checkout uses the
# prefix fetched into `.occt/`.
if [ -d "$HERE/.occt/prefix/lib" ]; then
    LD_LIBRARY_PATH="$HERE/.occt/prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LD_LIBRARY_PATH
fi

exec python3 "$HERE/hello_freecad.py" "$@"
