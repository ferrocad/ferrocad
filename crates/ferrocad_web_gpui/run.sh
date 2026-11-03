#!/usr/bin/env sh
# Build and serve the browser shell (the GPUI host). See README.md.
#
#   crates/ferrocad_web_gpui/run.sh
#   PORT=9000 crates/ferrocad_web_gpui/run.sh
#   JOBS=4 crates/ferrocad_web_gpui/run.sh
#   crates/ferrocad_web_gpui/run.sh --build-only
#   WASM_OFFLINE=1 crates/ferrocad_web_gpui/run.sh   # build with no network
#
# This is the shell host: `ferrocad_shell_core`'s window with a real
# `ferrocad_core::Document` on the wasm OCCT kernel. `crates/ferrocad_web/run.sh` is
# the older standalone canvas demo, which draws the same document without the shell.
#
# The document tessellates through the kernel, so the build happens inside the
# digest-pinned cross image (`tools/wasm-prepare.sh` fetches the kernel, the TKFeat
# archive and the wasm-bindgen CLI). Requirements: Docker (usable without sudo),
# curl, tar, git and python3. Node is not needed.
#
# `CARGO_TARGET` and `CARGO_HOME_HOST` are relative to the repository root and are
# mounted into the container; both caches are kept on the host so repeat builds are
# incremental and only the changed crates rebuild. Point `CARGO_HOME_HOST` at a cargo
# home with the lock's crates to build without network (with `WASM_OFFLINE=1`).
set -eu

cd "$(dirname "$0")/../.."
# shellcheck source=/dev/null
. tools/wasm-prepare.sh

PORT="${PORT:-8081}"
CARGO_TARGET="${CARGO_TARGET:-target/wasm-cargo}"
CARGO_HOME_HOST="${CARGO_HOME_HOST:-target/wasm-cargo-home}"
BUILD_ONLY=0
NO_OPEN=0
# `CARGO_NET_OFFLINE` takes `true`/`false`, not `1`/`0`.
OFFLINE=false
[ -z "${WASM_OFFLINE:-}" ] || OFFLINE=true

usage() {
    sed -n '2,22p' "$0"
}

for arg in "$@"; do
    case "$arg" in
        --build-only) BUILD_ONLY=1 ;;
        --no-open) NO_OPEN=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }

mkdir -p "$CARGO_TARGET" "$CARGO_HOME_HOST"
prepare_wasm_kernel
prepare_wasm_bindgen

echo "building ferrocad_web_gpui for wasm32 ..."
docker run --rm \
    -v "$PWD":/work -w /work \
    -v "$PWD/$CARGO_HOME_HOST":/cargo-home \
    -v "$PWD/$CARGO_TARGET":/cargo-target \
    -e OCCT_WASM_ROOT="/work/$KERNEL" \
    -e CARGO_HOME=/cargo-home \
    -e CARGO_TARGET_DIR=/cargo-target \
    -e CARGO_NET_OFFLINE="$OFFLINE" \
    -e HTTP_PROXY -e HTTPS_PROXY -e ALL_PROXY \
    "$IMAGE" \
    cargo build --release -j "${JOBS:-2}" -p ferrocad_web_gpui \
        --target wasm32-unknown-unknown --locked
# The container runs as root; hand the caches back to the invoking user.
docker run --rm -v "$PWD":/work "$IMAGE" \
    chown -R "$(id -u):$(id -g)" "/work/$CARGO_TARGET" "/work/$CARGO_HOME_HOST"

"$BINDGEN" --target web \
    --out-dir crates/ferrocad_web_gpui/www/pkg \
    "$CARGO_TARGET/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm"

if [ "$BUILD_ONLY" = 1 ]; then
    echo "built crates/ferrocad_web_gpui/www/pkg"
    exit 0
fi

# wasm-bindgen's `--target web` output must be fetched over HTTP.
if [ "$NO_OPEN" = 0 ]; then
    (xdg-open "http://localhost:$PORT" >/dev/null 2>&1 \
        || open "http://localhost:$PORT" >/dev/null 2>&1 \
        || true) &
fi
echo "serving http://localhost:$PORT  (Ctrl-C to stop)"
cd crates/ferrocad_web_gpui/www
exec python3 -m http.server "$PORT"
