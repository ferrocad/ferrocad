#!/bin/sh
# Build a FerroCAD AppImage from the staged payload.
#
# Requirements: appimagetool, the canonical tool (written in C). It is found on
# PATH, via APPIMAGETOOL, or as tools/appimagetool-<arch>.AppImage. An .AppImage
# tool is run with --appimage-extract-and-run, so FUSE is not required.
#
#   packaging/linux/appimage.sh [--debug] [--edition parametric] [--occt]
#                               [--check-init]
#
# The base app's artifact is ferrocad-<arch>.AppImage; an edition's is
# ferrocad-<edition>-<arch>.AppImage (see docs/architecture/edition-publishing.md).
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../.." && pwd)
cd "$ROOT"

if [ -f "$ROOT/../.toolchain/env.sh" ]; then
    . "$ROOT/../.toolchain/env.sh"
fi

# `bundle` owns the payload directory name; mirror its edition spelling here so
# the launcher and the artifact agree. Every flag (including --occt and
# --check-init) is passed through unchanged.
EDITION=""
want_edition=0
for arg in "$@"; do
    if [ "$want_edition" = 1 ]; then
        EDITION=$arg
        want_edition=0
        continue
    fi
    case "$arg" in
        --edition) want_edition=1 ;;
        --edition=*) EDITION=${arg#--edition=} ;;
    esac
done
case "$EDITION" in
    "" | ferrocad | base) EDITION="" ;;
    ferrocad_parametric | ferrocad-parametric) EDITION=parametric ;;
esac

cargo xtask bundle "$@"

ARCH=$(uname -m)
if [ -n "$EDITION" ]; then
    APPDIR="$ROOT/target/dist/ferrocad-$EDITION"
    OUT="$ROOT/target/dist/ferrocad-$EDITION-$ARCH.AppImage"
else
    APPDIR="$ROOT/target/dist/ferrocad"
    OUT="$ROOT/target/dist/ferrocad-$ARCH.AppImage"
fi

if [ ! -d "$APPDIR" ]; then
    echo "no staged payload at $APPDIR (did bundle --edition $EDITION fail?)" >&2
    exit 1
fi

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
