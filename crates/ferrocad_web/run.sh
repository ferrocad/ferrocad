#!/usr/bin/env sh
# Build and serve the wasm demo. See README.md.
#
#   crates/ferrocad_web/run.sh
#   PORT=9000 crates/ferrocad_web/run.sh
#   JOBS=4 crates/ferrocad_web/run.sh
#   crates/ferrocad_web/run.sh --build-only
#
# This is the standalone canvas demo. The GPUI shell is `ferrocad_web_gpui`:
#   crates/ferrocad_web_gpui/run.sh
#
# Requirements: Docker (usable without sudo), curl, tar, git and python3. Node
# is optional (it runs the smoke test); a host Rust toolchain is not needed.
set -eu

cd "$(dirname "$0")/../.."
# shellcheck source=/dev/null
. tools/wasm-prepare.sh

CARGO_TARGET='target/wasm-cargo'
CARGO_HOME_HOST='target/wasm-cargo-home'
PORT="${PORT:-8080}"
BUILD_ONLY=0
NO_OPEN=0

usage() {
    sed -n '2,13p' "$0"
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

# The prebuilt kernel (OCCT 8.0.1, headless, wasi-sdk build), its TKFeat archive,
# and the wasm-bindgen CLI the lock pins. Shared with the GPUI shell host.
prepare_wasm_kernel
prepare_wasm_bindgen

# Build the cdylib in the digest-pinned cross image. Both caches — the build
# output and Cargo's own home (registry + git checkouts of the OCCT bindings)
# — live on the host, so repeat runs are incremental and need no network. The
# container runs as root, so hand the caches back to the invoking user.
echo "building ferrocad_web for wasm32 (first run pulls the cross image) ..."
docker run --rm \
    -v "$PWD":/work -w /work \
    -e OCCT_WASM_ROOT="/work/$KERNEL" \
    -e CARGO_HOME="/work/$CARGO_HOME_HOST" \
    -e CARGO_TARGET_DIR="/work/$CARGO_TARGET" \
    -e HTTP_PROXY -e HTTPS_PROXY -e ALL_PROXY \
    "$IMAGE" \
    cargo build --release -j "${JOBS:-2}" -p ferrocad_web --target wasm32-unknown-unknown --locked
docker run --rm -v "$PWD":/work "$IMAGE" \
    chown -R "$(id -u):$(id -g)" "/work/$CARGO_TARGET" "/work/$CARGO_HOME_HOST"

# 5. Generate the JS glue and exercise the module without a browser.
"$BINDGEN" --target web \
    --out-dir crates/ferrocad_web/www/pkg \
    "$CARGO_TARGET/wasm32-unknown-unknown/release/ferrocad_web.wasm"
if command -v node >/dev/null 2>&1; then
    node crates/ferrocad_web/www/smoke.mjs
else
    echo "node not found; skipping the smoke test"
fi

if [ "$BUILD_ONLY" = 1 ]; then
    echo "built crates/ferrocad_web/www/pkg"
    exit 0
fi

# 6. Serve it. wasm-bindgen's `--target web` output must be fetched over HTTP.
if [ "$NO_OPEN" = 0 ]; then
    (xdg-open "http://localhost:$PORT" >/dev/null 2>&1 \
        || open "http://localhost:$PORT" >/dev/null 2>&1 \
        || true) &
fi
echo "serving http://localhost:$PORT  (Ctrl-C to stop)"
cd crates/ferrocad_web/www
exec python3 -m http.server "$PORT"
