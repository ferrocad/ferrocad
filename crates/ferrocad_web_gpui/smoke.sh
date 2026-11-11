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

cleanup() {
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
        timeout 180 "$CHROME" $CHROME_FLAGS --screenshot="$TMP/screenshot.png" \
            "$BASE$1" >>"$log" 2>&1 || true
    else
        "$CHROME" $CHROME_FLAGS --screenshot="$TMP/screenshot.png" \
            "$BASE$1" >>"$log" 2>&1 || true
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

    for log in settings settings_write settings_js settings_pywasm settings_command settings_command_js; do
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

if [ "$FAILED" != 0 ]; then
    echo "smoke check FAILED" >&2
    exit 1
fi

echo "smoke check passed: both shapes tessellate, the stage draws the fillet alone, the wasm workbench creates Toy, both Part manipulator commits and gestures build their feature, the plain runtime has the grid card, no panic or JS error"
