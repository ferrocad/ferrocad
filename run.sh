#!/bin/sh
# Run the headless hello world against the Rust-backed FreeCAD module.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PYTHONPATH="$HERE/python${PYTHONPATH:+:$PYTHONPATH}"
export PYTHONPATH

exec python3 "$HERE/hello_freecad.py" "$@"
