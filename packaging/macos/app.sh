#!/bin/sh
# Build a FerroCAD .app bundle (and a .dmg when hdiutil is available).
#
#   packaging/macos/app.sh [--debug]
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../.." && pwd)
cd "$ROOT"

if [ -f "$ROOT/../.toolchain/env.sh" ]; then
    . "$ROOT/../.toolchain/env.sh"
fi

cargo xtask bundle "$@"

PAYLOAD="$ROOT/target/dist/ferrocad"
APP="$ROOT/target/dist/FerroCAD.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

# Bundle metadata and launcher. `ferrocad` sets the environment and execs
# `ferrocad-bin`, because a Finder-launched app has no wrapper script otherwise.
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
cp "$PAYLOAD/bin/ferrocad" "$APP/Contents/MacOS/ferrocad-bin"
cp "$HERE/ferrocad" "$APP/Contents/MacOS/ferrocad"
chmod +x "$APP/Contents/MacOS/ferrocad-bin" "$APP/Contents/MacOS/ferrocad"

# Payload, placed per macOS bundle conventions.
cp -R "$PAYLOAD/python" "$APP/Contents/Resources/python"
cp -R "$PAYLOAD/mods" "$APP/Contents/Resources/mods"
cp -R "$PAYLOAD/LICENSES" "$APP/Contents/Resources/LICENSES"
cp "$PAYLOAD/lib/ferrocad.abi3.so" "$APP/Contents/Frameworks/"

if [ -f "$HERE/FerroCAD.icns" ]; then
    cp "$HERE/FerroCAD.icns" "$APP/Contents/Resources/"
else
    echo "note: packaging/macos/FerroCAD.icns is missing; the app will have no icon" >&2
fi

if command -v hdiutil >/dev/null 2>&1; then
    DMG="$ROOT/target/dist/FerroCAD.dmg"
    rm -f "$DMG"
    hdiutil create -volname FerroCAD -srcfolder "$APP" -ov -format UDZO "$DMG"
    echo "created $DMG"
else
    echo "note: hdiutil not available; skipped the .dmg (bundle is at $APP)" >&2
fi

echo "created $APP"
