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
# asserts on the document the doorway returns. Without a browser or a build it prints
# `skipped: …` and exits 0, so it never blocks a developer who has not built.
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

if [ "$FAILED" != 0 ]; then
    echo "smoke check FAILED" >&2
    exit 1
fi

echo "smoke check passed: both shapes tessellate, the stage draws the fillet alone, the wasm workbench creates Toy, no panic or JS error"
