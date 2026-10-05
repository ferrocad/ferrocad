#!/bin/sh
# Generate the GitHub release body from the built artifacts.
#
#   packaging/release-notes.sh <dist-dir>
#
# `<dist-dir>` is what `actions/download-artifact` produced: one subdirectory per
# artifact. `release-notes.md` holds the prose and an `<!--ARTIFACTS-->` marker;
# this script substitutes a table measured from the actual files, so the counts,
# the bundled-library name and the launcher cannot drift from what shipped.
#
# Requires: 7z (p7zip-full) for the DMG, unzip for the zip, and the ability to
# execute the AppImage for `--appimage-extract` (no FUSE needed).
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
DIST=${1:?usage: release-notes.sh <dist-dir>}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

count() { find "$1" -type f 2>/dev/null | wc -l | tr -d ' '; }

# The main CPython shared library / import DLL inside a `runtime/` tree.
runtime_lib() {
    lib=$(ls "$1/lib" 2>/dev/null |
        grep -E '^libpython[0-9]+\.[0-9]+\.(so(\.[0-9]+)*|dylib)$' |
        sort | tail -1 || true)
    if [ -z "$lib" ]; then
        lib=$(ls "$1" 2>/dev/null | grep -E '^python[0-9][0-9]+\.dll$' | head -1 || true)
    fi
    printf '%s' "${lib:-unknown}"
}

# row <artifact> <facade-dir> <draft-dir> <runtime-dir> <launcher>
row() {
    printf '| `%s` | %s files | %s files | `%s` | `%s` |\n' \
        "$1" "$(count "$2")" "$(count "$3")" "$(runtime_lib "$4")" "$5"
}

appimage_row() {
    img=$(CDPATH= cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")
    out="$work/appimage"
    mkdir -p "$out"
    chmod +x "$img"
    (cd "$out" && "$img" --appimage-extract >/dev/null 2>&1)
    root="$out/squashfs-root"
    row "$(basename "$img")" "$root/python" "$root/mods/Draft" "$root/runtime" "AppRun"
}

dmg_row() {
    out="$work/dmg"
    mkdir -p "$out"
    7z x -y -o"$out" "$1" >/dev/null 2>&1
    app=$(find "$out" -maxdepth 1 -name '*.app' | head -1)
    res="$app/Contents/Resources"
    row "$(basename "$1")" "$res/python" "$res/mods/Draft" "$res/runtime" "Contents/MacOS/ferrocad"
}

zip_row() {
    out="$work/zip"
    mkdir -p "$out"
    unzip -q -o "$1" -d "$out"
    row "$(basename "$1")" "$out/python" "$out/mods/Draft" "$out/runtime" "ferrocad.bat"
}

{
    printf '| Artifact | Python facade | Draft workbench | Bundled CPython | Launcher |\n'
    printf '| --- | --- | --- | --- | --- |\n'
    find "$DIST" -name '*.AppImage' -print | sort |
        while IFS= read -r f; do appimage_row "$f"; done
    find "$DIST" -name '*.dmg' -print | sort |
        while IFS= read -r f; do dmg_row "$f"; done
    find "$DIST" -name '*.zip' -print | sort |
        while IFS= read -r f; do zip_row "$f"; done
} >"$work/table.md"

sed -e "/<!--ARTIFACTS-->/r $work/table.md" -e '/<!--ARTIFACTS-->/d' "$HERE/release-notes.md"
