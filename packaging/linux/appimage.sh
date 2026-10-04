#!/bin/sh
# Build a FerroCAD AppImage from the staged payload.
#
# Requirements: appimagetool, the canonical tool (written in C). It is found on
# PATH, via APPIMAGETOOL, or as tools/appimagetool-<arch>.AppImage. An .AppImage
# tool is run with --appimage-extract-and-run, so FUSE is not required.
#
#   packaging/linux/appimage.sh [--debug]
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../.." && pwd)
cd "$ROOT"

if [ -f "$ROOT/../.toolchain/env.sh" ]; then
    . "$ROOT/../.toolchain/env.sh"
fi

# Stage the payload (pass through flags such as --debug).
cargo xtask bundle "$@"

ARCH=$(uname -m)
APPDIR="$ROOT/target/dist/ferrocad"
OUT="$ROOT/target/dist/ferrocad-$ARCH.AppImage"

# AppImage glue: the AppDir needs AppRun, a .desktop entry and an icon.
cp "$HERE/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"
cp "$HERE/ferrocad.desktop" "$APPDIR/ferrocad.desktop"
if [ -f "$HERE/ferrocad.png" ]; then
    cp "$HERE/ferrocad.png" "$APPDIR/ferrocad.png"
else
    echo "note: packaging/linux/ferrocad.png is missing; the AppImage will have no icon" >&2
fi

# Locate appimagetool.
TOOL="${APPIMAGETOOL:-}"
if [ -z "$TOOL" ]; then
    if command -v appimagetool >/dev/null 2>&1; then
        TOOL=appimagetool
    elif [ -f "$ROOT/tools/appimagetool-$ARCH.AppImage" ]; then
        TOOL="$ROOT/tools/appimagetool-$ARCH.AppImage"
    elif [ -f "$ROOT/tools/appimagetool.AppImage" ]; then
        TOOL="$ROOT/tools/appimagetool.AppImage"
    else
        echo "appimagetool not found." >&2
        echo "Put it on PATH, set APPIMAGETOOL, or drop appimagetool-<arch>.AppImage into tools/." >&2
        echo "Download: https://github.com/AppImage/appimagetool/releases" >&2
        exit 1
    fi
fi

RUN=""
case "$TOOL" in
    *.AppImage | *.appimage)
        RUN="--appimage-extract-and-run"
        APPIMAGE_EXTRACT_AND_RUN=1
        export APPIMAGE_EXTRACT_AND_RUN
        ;;
esac

echo "== appimagetool $APPDIR -> $OUT =="
ARCH="$ARCH" "$TOOL" $RUN "$APPDIR" "$OUT"
echo "created $OUT"
