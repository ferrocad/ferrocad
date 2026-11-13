#!/usr/bin/env sh
# Headless smoke check for the browser shell. See README.md, "Verified".
#
#   crates/ferrocad_web_gpui/smoke.sh
#   PORT=9000 crates/ferrocad_web_gpui/smoke.sh
#   CHROME=/path/to/chrome crates/ferrocad_web_gpui/smoke.sh
#
# Serves the already-built `www/pkg` and loads it in headless Chrome, asserting
# the documented console baseline: both shapes tessellate through the wasm kernel,
# the stage draws the fillet **instead of** the box it consumed, a save/open round
# trip keeps both objects, and nothing panics. A second capture installs the wasm
# workbench (`?runtime=js&addons=wasm`), runs its command through the palette and
# asserts on the document the doorway returns. A third pair sends Part's two
# manipulator commits (`?manipulator=`), which no pointer gesture in a capture can
# reach, and asserts that each feature replaced the body it consumed. Without a
# browser or a build it prints `skipped: …` and exits 0, so it never blocks a
# developer who has not built.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../.."

PORT="${PORT:-8099}"
WWW="crates/ferrocad_web_gpui/www"
BASE="http://127.0.0.1:$PORT"

FAILED=0
SERVER_PID=
TMP=
SMOKE_SETTINGS="$WWW/capture-settings-smoke.json"

cleanup() {
    rm -f "$SMOKE_SETTINGS"
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    [ -n "$TMP" ] || return 0
    if [ "$FAILED" = 0 ]; then
        rm -rf "$TMP"
    else
        echo "console log kept at $TMP/console.log" >&2
    fi
}
trap cleanup EXIT

find_chrome() {
    if [ -n "${CHROME:-}" ]; then
        [ -x "$CHROME" ] || return 1
        printf '%s\n' "$CHROME"
        return 0
    fi
    for name in google-chrome google-chrome-stable chromium; do
        if command -v "$name" >/dev/null 2>&1; then
            command -v "$name"
            return 0
        fi
    done
    return 1
}

wait_for_server() {
    tries=0
    while [ "$tries" -lt 100 ]; do
        if python3 -c \
            'import socket,sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), 0.2).close()' \
            "$PORT" 2>/dev/null; then
            return 0
        fi
        tries=$((tries + 1))
        sleep 0.1
    done
    return 1
}

start_server() {
    python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$WWW" \
        >"$TMP/server.log" 2>&1 &
    SERVER_PID=$!
    wait_for_server || return 1
    # If the port was already taken, the probe connects to the other listener
    # while our server has exited.
    kill -0 "$SERVER_PID" 2>/dev/null
}

# The flags come from the verified headless capture in README.md. The screenshot
# makes Chrome produce a rendered frame; the image itself is not asserted.
CHROME_FLAGS="--headless=new --no-sandbox --disable-dev-shm-usage
--use-angle=swiftshader --enable-unsafe-swiftshader
--window-size=1280,800 --virtual-time-budget=20000
--enable-logging=stderr --v=0"

capture() {
    log="${2:-$TMP/console.log}"
    echo "headless Chrome: $BASE$1"
    printf '\n=== %s ===\n' "$BASE$1" >>"$log"
    # `$CHROME_FLAGS` is word-split on purpose: it is a flag list, not one argument.
    # shellcheck disable=SC2086
    if command -v timeout >/dev/null 2>&1; then
        timeout 180 "$CHROME" $CHROME_FLAGS --user-data-dir="$PROFILE" \
            --screenshot="$TMP/screenshot.png" "$BASE$1" >>"$log" 2>&1 || true
    else
        "$CHROME" $CHROME_FLAGS --user-data-dir="$PROFILE" \
            --screenshot="$TMP/screenshot.png" "$BASE$1" >>"$log" 2>&1 || true
    fi
}

# Add a line here to cover another harness URL (`?click=`, `?select=`, `?keys=`,
# `?drag=`, `?save=`). Every run is held to the same console contract; the
# round-trip line is asserted separately because only `?reopen=1` saves.
capture_urls() {
    capture "/"
    capture "/?click=950,131"
    capture "/?reopen=1"
    # S7: the page's ES module registers one command and receives one viewer event.
    capture "/?js=1&click=430,380"
    capture "/?js=1&keys=ctrl-k;d;e;m;o;enter"
}

# The wasm workbench leg: `?runtime=js` installs the page's script half, `?addons=wasm`
# instantiates `www/addons/toy.wasm`, and the palette runs `Toy_Slab`. The shell draws
# its tree into the canvas, so `?probe=1` reads the document back through the doorway;
# the assertions are on that JSON, not on the DOM.
capture_workbench() {
    capture "/?runtime=js&addons=wasm&keys=ctrl-k;t;o;y;enter&probe=1" "$TMP/addon.log"
}

# The workbench manipulation leg: `?manipulator=` sends the commit a released handle
# sends, which a headless capture cannot produce (no scrub, no drag). One capture per
# manipulator, because they build different features.
capture_manipulator() {
    # `?js=1` installs the page's document API so `?probe=1` can read the result back;
    # the commit still lands on the document half (`JsScriptHost` does not claim
    # `apply_interaction`).
    capture "/?js=1&manipulator=dress_up:fillet:Box:Edge1:2&probe=1" "$TMP/fillet.log"
    capture "/?js=1&manipulator=push_pull:extrude:Box:Face1:3&probe=1" "$TMP/push_pull.log"
}

# The gesture leg: `?gesture=` arms the manipulator through the click handler, presses
# on the stroke the manipulator drew and pulls it outward, so the pointer path the
# commit probe skips is exercised too.
capture_gesture() {
    # The sample's `Fillet` is what the stage draws (it consumed `Box`), so it is the
    # object with an entity to select a sub-element on.
    capture "/?js=1&gesture=3:Fillet:Edge1:60&probe=1" "$TMP/gesture_fillet.log"
    capture "/?js=1&gesture=2:Fillet:Face1:40&probe=1" "$TMP/gesture_push_pull.log"
}

# The settings leg: `?runtime=plain` installs no script half, so the grid card comes
# from the document half. The tree and the card are painted into the canvas, so
# `?settings=` logs the rows the pane would show and the request an edit returns.
capture_settings() {
    capture "/?runtime=plain&settings=Settings/Grid" "$TMP/settings.log"
    capture "/?runtime=plain&settings=Settings/Grid.minor=20" "$TMP/settings_write.log"
    # The page's script half does not implement `Settings/*`, and the wasm-Python half
    # does not either, so the document half fills the card in on both — the runtimes the
    # grid used to have no card under.
    capture "/?runtime=js&settings=Settings/Grid" "$TMP/settings_js.log"
    capture "/?runtime=pywasm&settings=Settings/Grid" "$TMP/settings_pywasm.log"
    # The header row is a catalogue built-in, so the runtimes with no script half list
    # it and its verb names the same node the tree does.
    capture "/?runtime=plain&command=View_GridSettings" "$TMP/settings_command.log"
    capture "/?runtime=js&command=View_GridSettings" "$TMP/settings_command_js.log"
    # Which providers each runtime has. The browser ships no `mods/` and has no filesystem
    # for `load_workbenches` to scan, so a Python provider mod cannot load here and the
    # registry is empty by construction — the grid card is the document half's.
    capture "/?runtime=pywasm&providers=1" "$TMP/providers_pywasm.log"
    capture "/?runtime=pywasm,js&providers=1" "$TMP/providers_chain.log"
    # A whole settings file: the page fetches and installs it before the module
    # boots, then the card is read from the hydrated store. The file is served
    # from the page's own directory, as a capture passes the desktop's store.
    printf '%s\n' \
        '{"version":1,"values":[{"root":"User parameter","group":"BaseApp/Preferences/Mods/grid/ferrocad","name":"enabled","kind":"bool","value":true}]}' \
        >"$SMOKE_SETTINGS"
    capture "/?runtime=plain&settings_file=capture-settings-smoke.json&settings=Settings/Grid" "$TMP/settings_file.log"
}

# The fetched mod packs (B19c): the page has no filesystem, so it walks
# `www/mods/index.json` after the first frame. Which scripts *succeed* depends on the
# facade's contract (B18a/B18b), so the assertion is that delivery works — the index
# parsed and a named pack reached the interpreter — not on a specific pass count.
capture_mods() {
    capture "/?mods=1" "$TMP/mods.log"
}

# The write leg (B18b item 5): a browser console cannot be typed into, so `?script=` runs
# one snippet in the console namespace before the first frame. The snippet creates an
# object and sets a property through the document doorway, then prints the document's
# object names and the value it assigned — the registry, not a copy, is what answers.
capture_write() {
    capture "/?runtime=pywasm&script=obj%20%3D%20App.ActiveDocument.addObject%28%27App%3A%3AFeatureTest%27%2C%20%27Probe%27%29%0Aobj.Float%20%3D%202.5%0Aprint%28%5Bo.Name%20for%20o%20in%20App.ActiveDocument.Objects%5D%2C%20obj.Float%29" "$TMP/write.log"
}

check_write() {
    LOG="$TMP/write.log"
    echo "pywasm document write console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true'
    require_line 'Probe'
    require_line '2.5'

    echo "pywasm document write forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
read-only
ok=false
panicked at
unreachable
EOF
}

check_mods() {
    LOG="$TMP/mods.log"
    echo "fetched mod pack console lines:"
    require_line 'ferrocad: mods: '
    require_line 'pack(s) from'
    require_line 'ferrocad: mod GridOverlay:'
    # A Coin skin loads with its pack, resources included (B21). The summary line needs
    # all six fetches inside the capture's budget; this one is early in index order and
    # is the same evidence — the pack ran, and nothing in it raised.
    require_line 'ferrocad: mod AxisOverlayCoin: 2 script(s) loaded, 0 failed'

    echo "fetched mod pack forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
mods index:
HTTP 404
DoesNotExist
panicked at
unreachable
EOF
}

require_line() {
    if grep -Fq -- "$1" "$LOG"; then
        printf '  ok       %s\n' "$(grep -Fm1 -- "$1" "$LOG")"
    else
        printf '  MISSING  %s\n' "$1" >&2
        FAILED=1
    fi
}

forbid_pattern() {
    if grep -Fq -- "$1" "$LOG"; then
        printf '  FOUND    %s\n' "$1" >&2
        grep -Fm3 -- "$1" "$LOG" | sed 's/^/           /' >&2
        FAILED=1
    fi
}

check_console() {
    LOG="$TMP/console.log"
    echo "required console lines:"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        require_line "$line"
    done <<'EOF'
ferrocad: Box · 12 triangles, 12 edges
ferrocad: Fillet · 40 triangles, 21 edges
ferrocad: stage draws Fillet
ferrocad js: registered Demo_JS
ferrocad js: run Demo_JS
ferrocad js: event
EOF

    echo "round-trip console lines:"
    require_line 'ferrocad harness: save Sample'
    require_line 'ferrocad harness: reopened -> opened reopened.FCStd (2 object(s))'

    echo "forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
ferrocad: stage draws Box
did not tessellate
panicked at
RuntimeError
unreachable
EOF
}

# The workbench must not merely register: the command has to reach the document.
# `?probe=1` logs the doorway's `{"call":"document"}` answer, whose object row
# carries the name and the type id, so the assertion is about the document the shell
# is showing rather than about a `console.log` in the addon.
check_workbench() {
    LOG="$TMP/addon.log"
    echo "wasm workbench console lines:"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        require_line "$line"
    done <<'EOF'
ferrocad: scripting runtime js
ferrocad toy (wasm): registered Toy_Slab, Toy_Taller
ferrocad toy (wasm): document read-write
ferrocad toy (wasm): run Toy_Slab
ferrocad toy (wasm): document.edited
ferrocad toy (wasm): document.recomputed
EOF

    echo "wasm workbench document probe:"
    require_line '"name":"Toy"'
    require_line '"type_id":"App::FeaturePython"'

    echo "wasm workbench forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
no document doorway
did not tessellate
panicked at
RuntimeError
unreachable
EOF
}

# The manipulation leg. `ferrocad probe:` is the host echoing the commit's verdict;
# `ferrocad: stage draws …` is the backend's own line for the document after it, which
# is the interesting half: the feature exists and replaces the body it consumed.
check_manipulator() {
    LOG="$TMP/fillet.log"
    echo "dress-up console lines:"
    require_line 'ferrocad probe: dress_up fillet Box -> ok=true'
    require_line 'ferrocad: stage draws Fillet, Box_Fillet'
    # The document, not just the stage: the feature exists with the radius the commit
    # sent. `?probe=1` reads it through the page's document API.
    require_line '"name":"Box_Fillet"'
    require_line '"type_id":"Part::Fillet"'
    require_line '"value":"2 mm"'

    LOG="$TMP/push_pull.log"
    echo "push/pull console lines:"
    require_line 'ferrocad probe: push_pull extrude Box -> ok=true'
    require_line 'ferrocad: stage draws Fillet, Box_Fuse'
    # The sweep is a document object too; it is hidden, so the stage draws the boolean
    # only, and the boolean is what carries the shape.
    require_line '"name":"Box_Extrusion"'
    require_line '"type_id":"Part::Extrusion"'
    require_line '"name":"Box_Fuse"'
    require_line '"type_id":"Part::Fuse"'

    for log in fillet push_pull; do
        LOG="$TMP/$log.log"
        echo "$log forbidden console patterns:"
        while IFS= read -r pattern; do
            [ -n "$pattern" ] || continue
            forbid_pattern "$pattern"
        done <<'EOF'
panicked at
RuntimeError
unreachable
is not available in this build
no sub-shape
not a planar face
EOF
    done
}

# The settings card, on the runtime that has no interpreter. The rows are the
# document half's answer from the shell's own presentation state; the write is the
# request the shell applies, which is what makes a row edit visible.
check_settings() {
    LOG="$TMP/settings.log"
    echo "settings card console lines (plain runtime):"
    require_line 'ferrocad: scripting runtime plain'
    require_line 'ferrocad probe: settings Settings/Grid -> 10 row(s): enabled, color, opacity, adaptive, minor_style, major_width, minor_width, minor, major, radius'
    require_line 'ferrocad probe: tree Sample, Settings'

    LOG="$TMP/settings_write.log"
    echo "settings write console lines (plain runtime):"
    require_line 'ferrocad probe: settings Settings/Grid.minor=20 -> ok=true'
    require_line 'grid_minor: Some(20.0)'

    LOG="$TMP/settings_js.log"
    echo "settings card console lines (js runtime):"
    require_line 'ferrocad: scripting runtime js'
    require_line 'ferrocad probe: settings Settings/Grid -> 10 row(s): enabled, color, opacity, adaptive, minor_style, major_width, minor_width, minor, major, radius'
    require_line 'ferrocad probe: tree Sample, Settings'

    # The interpreter that owns `Settings/*` natively (CPython) is not in the browser, so
    # the wasm-Python half gets the same document-half card as the other two.
    LOG="$TMP/settings_pywasm.log"
    echo "settings card console lines (pywasm runtime):"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: settings Settings/Grid -> 10 row(s): enabled, color, opacity, adaptive, minor_style, major_width, minor_width, minor, major, radius'
    require_line 'ferrocad probe: tree Sample, Settings'

    LOG="$TMP/settings_command.log"
    echo "grid settings command console lines (plain runtime):"
    require_line 'ferrocad probe: command View_GridSettings -> ok=true status="grid settings" select=Some("Settings/grid")'

    LOG="$TMP/settings_command_js.log"
    echo "grid settings command console lines (js runtime):"
    require_line 'ferrocad probe: command View_GridSettings -> ok=true status="grid settings" select=Some("Settings/grid")'

    LOG="$TMP/settings_file.log"
    echo "whole settings file console lines:"
    require_line 'ferrocad harness: settings capture-settings-smoke.json 1 group(s)'
    require_line 'ferrocad probe: settings Settings/Grid -> 10 row(s)'

    # The wasm facade answers `view`, and with no provider registered its summary is the
    # closed empty case — so the document half names the native provider that actually
    # draws the ground (B18a step 5 adds the registry). The chain leg also proves the two
    # halves compose.
    LOG="$TMP/providers_pywasm.log"
    echo "provider console lines (pywasm runtime):"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: providers grid=FerroCAD (native) switch=0 axis=FerroCAD (native) switch=0 registry=grid=1,axis=1'

    LOG="$TMP/providers_chain.log"
    echo "provider console lines (pywasm,js chain):"
    require_line 'ferrocad: scripting runtime pywasm,js'
    require_line 'ferrocad probe: providers grid=FerroCAD (native) switch=0 axis=FerroCAD (native) switch=0 registry=grid=1,axis=1'

    for log in settings settings_write settings_js settings_pywasm settings_command settings_command_js settings_file providers_pywasm providers_chain; do
        LOG="$TMP/$log.log"
        echo "$log forbidden console patterns:"
        while IFS= read -r pattern; do
            [ -n "$pattern" ] || continue
            forbid_pattern "$pattern"
        done <<'EOF'
panicked at
RuntimeError
unreachable
property error
is not available in this build
EOF
    done
}

check_gesture() {
    LOG="$TMP/gesture_fillet.log"
    echo "dress-up gesture console lines:"
    require_line 'ferrocad probe: gesture slot 3 Fillet Edge1 pulled 60 from'
    # A feature replaces the object it consumed, so the stage draws the new feature
    # alone — the sample's `Fillet` is gone from the stage, exactly as `Box` went when
    # the sample fillet consumed it.
    require_line 'ferrocad: stage draws Fillet_Fillet'
    require_line '"name":"Fillet_Fillet"'

    LOG="$TMP/gesture_push_pull.log"
    echo "push/pull gesture console lines:"
    require_line 'ferrocad probe: gesture slot 2 Fillet Face1 pulled 40 from'
    require_line 'ferrocad: stage draws Fillet_Fuse'
    require_line '"name":"Fillet_Fuse"'

    for log in gesture_fillet gesture_push_pull; do
        LOG="$TMP/$log.log"
        echo "$log forbidden console patterns:"
        while IFS= read -r pattern; do
            [ -n "$pattern" ] || continue
            forbid_pattern "$pattern"
        done <<'EOF'
not in the stage
nothing took the press
interaction error
panicked at
RuntimeError
unreachable
EOF
    done
}

if [ ! -d "$WWW/pkg" ]; then
    echo "skipped: $WWW/pkg is missing (build it with crates/ferrocad_web_gpui/run.sh --build-only)"
    exit 0
fi

if ! CHROME="$(find_chrome)"; then
    echo "skipped: no Chrome/Chromium found (set CHROME=/path/to/chrome)"
    exit 0
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/ferrocad-smoke.XXXXXX")"
# One private profile for every capture. Without `--user-data-dir`, headless Chrome
# creates `$XDG_CACHE_HOME/google-chrome-headless/scoped_dir*` (its disk and wasm
# code cache, ~80 MB per run) and leaves it behind when the process is killed. The
# profile lives under `$TMP`, so a successful run removes it with the rest.
PROFILE="$TMP/chrome-profile"
mkdir -p "$PROFILE"

# The packs the page fetches are generated from `mods/` (the directory is ignored, so a
# fresh checkout has none). Regenerate them, so the mods leg asserts on the tree rather
# than on a stale artifact.
python3 tools/mods_pack.py >/dev/null

if ! start_server; then
    echo "failed: the static server did not start on port $PORT (see $TMP/server.log)" >&2
    FAILED=1
    exit 1
fi

capture_urls
check_console

capture_workbench
check_workbench

capture_manipulator
check_manipulator

capture_gesture
check_gesture

capture_settings
check_settings

capture_mods
check_mods

capture_write
check_write

# The kernel leg (B18b item 2): the browser's Python builds a real solid with the OCCT
# kernel and puts it in the document, then reads it back through the facade. The model
# tree is painted into the canvas, so the assertion is the script's own output.
capture_kernel() {
    capture "/?runtime=pywasm&script=import%20Part%0Ab%20%3D%20Part.makeBox%2810%2C%2020%2C%2030%29%0APart.show%28b%2C%20%27Kernel%27%29%0Aprint%28round%28App.ActiveDocument.getObject%28%27Kernel%27%29.Shape.Volume%2C%201%29%2C%20App.ActiveDocument.getObject%28%27Kernel%27%29.Shape.ShapeType%29" "$TMP/kernel.log"
}

check_kernel() {
    LOG="$TMP/kernel.log"
    echo "pywasm kernel console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true output="6000.0 Solid"'

    echo "pywasm kernel forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
NameError
TypeError
read-only
panicked at
unreachable
EOF
}

capture_kernel
check_kernel

# The faces leg (B18b item 2): the browser's Python builds a wire with `makePolygon`, turns
# it into a `Part.Face`, shows it, and paints that face. Wire/face construction is the
# first path that runs `ferrocad_curves` in the browser, the paint is the
# `ShapeAppearance` write-back the viewport draws, and the last line is the B-Rep explorer
# classifying the face and its edge through the same kernel calls `ferrocad_part_py` makes.
capture_faces() {
    capture "/?runtime=pywasm&script=import%20Part%2C%20FreeCAD%0Awire%20%3D%20Part.makePolygon%28%5BFreeCAD.Vector%280%2C%200%2C%200%29%2C%20FreeCAD.Vector%2810%2C%200%2C%200%29%2C%20FreeCAD.Vector%2810%2C%2010%2C%200%29%5D%2C%20True%29%0Apanel%20%3D%20Part.show%28Part.Face%28wire%29%2C%20%27Panel%27%29%0Apanel.ViewObject.ShapeAppearance%5B0%5D.DiffuseColor%20%3D%20%280.2%2C%200.4%2C%200.6%2C%201.0%29%0Aprint%28round%28panel.Shape.Area%2C%201%29%2C%20tuple%28round%28v%2C%202%29%20for%20v%20in%20panel.ViewObject.ShapeAppearance%5B0%5D.DiffuseColor%29%29%0Aprint%28panel.Shape.Faces%5B0%5D.Surface.TypeId%2C%20panel.Shape.Edges%5B0%5D.Curve.TypeId%29%0A" "$TMP/faces.log"
}

check_faces() {
    LOG="$TMP/faces.log"
    echo "pywasm faces console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true output="50.0 (0.2, 0.4, 0.6, 1.0)\nPart::GeomPlane Part::GeomLine"'

    echo "pywasm faces forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
NameError
TypeError
ValueError
read-only
panicked at
unreachable
EOF
}

capture_faces
check_faces

# The coin leg (B20): the browser's Python imports the `pivy.coin` facade, builds a
# scene, and registers it on the same pull bridge the shell drains — the three facts that
# say the Coin workbenches have their surface on this runtime too.
capture_coin() {
    capture "/?runtime=pywasm&script=from%20pivy%20import%20coin%0Aimport%20FerroCADGui%0Anode%20%3D%20coin.SoSeparator%28%29%0Acoords%20%3D%20coin.SoCoordinate3%28%29%0Acoords.point.setValues%280%2C%203%2C%20%5B%280.0%2C%200.0%2C%200.0%29%2C%20%281.0%2C%200.0%2C%200.0%29%2C%20%280.0%2C%201.0%2C%200.0%29%5D%29%0Aface%20%3D%20coin.SoFaceSet%28%29%0Aface.numVertices.setValue%283%29%0Anode.addChild%28coords%29%0Anode.addChild%28face%29%0AFerroCADGui.addOverlay%28node%29%0Aprint%28node.getNumChildren%28%29%2C%20coords.point.getNum%28%29%2C%20coin.SoPickStyle.UNPICKABLE%29%0A" "$TMP/coin.log"
}

check_coin() {
    LOG="$TMP/coin.log"
    echo "pywasm coin console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true output="2 3 2"'

    echo "pywasm coin forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
NameError
TypeError
ImportError
AttributeError
panicked at
unreachable
EOF
}

capture_coin
check_coin

# The stage leg (B18b item 2): the browser's Python builds a `ShapeItem`, snaps through
# `SnapContext`, and asks the shell for the Draft tool bridge — the three facts that say
# `ferrocad_stage` and `python/FreeCADGui/draft.py` are installed on this host.
capture_stage() {
    capture "/?runtime=pywasm&script=import%20ferrocad_stage%20as%20s%0Aimport%20ferrocad_shell%0Aitem%20%3D%20s.ShapeItem%287%29%0Aitem.set_shape%28%5B%280%2C%200%2C%200%29%2C%20%281%2C%200%2C%200%29%2C%20%280%2C%201%2C%200%29%5D%2C%20%5B%5D%2C%20%5B0%2C%201%2C%202%5D%2C%20%5B%5D%29%0Actx%20%3D%20s.SnapContext%28s.Workplane.world%28%29%2C%201.0%2C%2010.0%29%0Aprint%28item.entity_id%2C%20len%28item.triangles%29%2C%20ctx.snap_point%28%280.4%2C%200.6%2C%200.0%29%2C%20%5B%5D%2C%200.0%29.point%2C%20type%28ferrocad_shell.tool_bridge%28%27draft%27%29%29.__name__%29%0A" "$TMP/stage.log"
}

check_stage() {
    LOG="$TMP/stage.log"
    echo "pywasm stage console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true output="7 3 (0.0, 1.0, 0.0) DraftTool"'

    echo "pywasm stage forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
NameError
ModuleNotFoundError
ImportError
TypeError
panicked at
unreachable
EOF
}

capture_stage
check_stage

# The forms leg (B18b item 2): `PySideUic.loadUi`'s module is installed on the browser's
# Python too, so a workbench panel is the same proxy tree here as on the desktop.
capture_forms() {
    capture "/?runtime=pywasm&script=import%20ferrocad_ui_py%0Asource%20%3D%20b%27%3Cui%20version%3D%224.0%22%3E%3Cclass%3EProbe%3C%2Fclass%3E%3Cwidget%20class%3D%22QDialog%22%20name%3D%22Form%22%3E%3Cproperty%20name%3D%22windowTitle%22%3E%3Cstring%3EProbe%3C%2Fstring%3E%3C%2Fproperty%3E%3Clayout%20class%3D%22QVBoxLayout%22%20name%3D%22layout%22%3E%3Citem%3E%3Cwidget%20class%3D%22QSpinBox%22%20name%3D%22spin%22%3E%3Cproperty%20name%3D%22value%22%3E%3Cnumber%3E7%3C%2Fnumber%3E%3C%2Fproperty%3E%3C%2Fwidget%3E%3C%2Fitem%3E%3C%2Flayout%3E%3C%2Fwidget%3E%3C%2Fui%3E%27%0Aform%20%3D%20ferrocad_ui_py.load_ui%28source%2C%20%27probe.ui%27%29%0Aform.spin.setValue%2842%29%0Aprint%28form.windowTitle%28%29%2C%20form.spin.value%28%29%2C%20form.findChild%28%27spin%27%29.objectName%28%29%29%0A" "$TMP/forms.log"
}

check_forms() {
    LOG="$TMP/forms.log"
    echo "pywasm forms console lines:"
    require_line 'ferrocad: scripting runtime pywasm'
    require_line 'ferrocad probe: script -> ok=true output="Probe 42 spin"'

    echo "pywasm forms forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
ModuleNotFoundError
ImportError
TypeError
AttributeError
panicked at
unreachable
EOF
}

capture_forms
check_forms

if [ "$FAILED" != 0 ]; then
    echo "smoke check FAILED" >&2
    exit 1
fi

echo "smoke check passed: both shapes tessellate, the stage draws the fillet alone, the wasm workbench creates Toy, both Part manipulator commits and gestures build their feature, the plain runtime has the grid card, the fetched mod packs reach the interpreter, a pywasm script creates and edits a document object, the pywasm kernel builds a solid, the pywasm faces leg builds, paints and classifies a face, the pywasm coin facade builds and registers an overlay, the pywasm stage binds and the Draft bridge loads, the pywasm form loader reads a panel, no panic or JS error"
