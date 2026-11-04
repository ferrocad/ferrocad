#!/usr/bin/env sh
# Headless smoke check for the browser shell. See README.md, "Verified".
#
#   crates/ferrocad_web_gpui/smoke.sh
#   PORT=9000 crates/ferrocad_web_gpui/smoke.sh
#   CHROME=/path/to/chrome crates/ferrocad_web_gpui/smoke.sh
#
# Serves the already-built `www/pkg` and loads it in headless Chrome, asserting
# the documented console baseline: the box and the fillet both tessellate through
# the wasm kernel, and nothing panics. Without a browser or a build it prints
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
    echo "headless Chrome: $BASE$1"
    printf '\n=== %s ===\n' "$BASE$1" >>"$TMP/console.log"
    # `$CHROME_FLAGS` is word-split on purpose: it is a flag list, not one argument.
    # shellcheck disable=SC2086
    if command -v timeout >/dev/null 2>&1; then
        timeout 180 "$CHROME" $CHROME_FLAGS --screenshot="$TMP/screenshot.png" \
            "$BASE$1" >>"$TMP/console.log" 2>&1 || true
    else
        "$CHROME" $CHROME_FLAGS --screenshot="$TMP/screenshot.png" \
            "$BASE$1" >>"$TMP/console.log" 2>&1 || true
    fi
}

# Add a line here to cover another harness URL (`?click=`, `?select=`, `?keys=`,
# `?drag=`). Every run is held to the same console contract.
capture_urls() {
    capture "/"
    capture "/?click=950,131"
}

require_line() {
    if grep -Fq -- "$1" "$TMP/console.log"; then
        printf '  ok       %s\n' "$(grep -Fm1 -- "$1" "$TMP/console.log")"
    else
        printf '  MISSING  %s\n' "$1" >&2
        FAILED=1
    fi
}

forbid_pattern() {
    if grep -Fq -- "$1" "$TMP/console.log"; then
        printf '  FOUND    %s\n' "$1" >&2
        grep -Fm3 -- "$1" "$TMP/console.log" | sed 's/^/           /' >&2
        FAILED=1
    fi
}

check_console() {
    echo "required console lines:"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        require_line "$line"
    done <<'EOF'
ferrocad: Box · 12 triangles, 12 edges
ferrocad: Fillet · 40 triangles, 21 edges
EOF

    echo "forbidden console patterns:"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        forbid_pattern "$pattern"
    done <<'EOF'
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

if [ "$FAILED" != 0 ]; then
    echo "smoke check FAILED" >&2
    exit 1
fi

echo "smoke check passed: Box and Fillet baselines logged, no panic or JS error"
