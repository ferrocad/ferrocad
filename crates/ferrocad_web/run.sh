#!/usr/bin/env sh
# Build and serve the wasm demo. See README.md.
#
#   crates/ferrocad_web/run.sh
#   PORT=9000 crates/ferrocad_web/run.sh
#   JOBS=4 crates/ferrocad_web/run.sh
#   crates/ferrocad_web/run.sh --build-only
#
# Requirements: Docker (usable without sudo), curl, tar and python3. Node is
# optional (it runs the smoke test); a host Rust toolchain is not needed.
set -eu

cd "$(dirname "$0")/../.."

IMAGE='ghcr.io/lzpel/cross-wasm32-unknown-unknown@sha256:4dc88df287da141e67588ba80ebf1d14d5cb24f5b05302c57b11b2a1e9bed1df'
OCCT_TARBALL='https://github.com/lzpel/cadrum/releases/download/occt-8_0_1_rev3/occt-8_0_1_rev3-wasm32_unknown_unknown.tar.gz'
OCCT_NAME='occt-8_0_1_rev3-wasm32_unknown_unknown'
CARGO_TARGET='target/wasm-cargo'
CARGO_HOME_HOST='target/wasm-cargo-home'
PORT="${PORT:-8080}"
BUILD_ONLY=0
NO_OPEN=0

usage() {
    sed -n '2,10p' "$0"
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

mkdir -p .occt-wasm target/wasm-bindgen "$CARGO_TARGET" "$CARGO_HOME_HOST"

# 1. The prebuilt wasm kernel (OCCT 8.0.1, headless, wasi-sdk build).
if [ ! -d ".occt-wasm/$OCCT_NAME" ]; then
    echo "fetching the prebuilt wasm OCCT (8.0.1) ..."
    curl -sSL -o .occt-wasm/occt.tar.gz "$OCCT_TARBALL"
    tar xzf .occt-wasm/occt.tar.gz -C .occt-wasm
    rm -f .occt-wasm/occt.tar.gz
fi

# 2. The wasm-bindgen CLI, at exactly the version the lockfile pins (the CLI
#    and the crate must match, or the glue's ABI check rejects the module).
version=$(awk '$0 == "name = \"wasm-bindgen\"" { found = 1 }
    found && /^version = / { gsub(/"/, "", $3); print $3; exit }' Cargo.lock)
[ -n "$version" ] || { echo "could not read wasm-bindgen from Cargo.lock" >&2; exit 1; }
bindgen="target/wasm-bindgen/wasm-bindgen"
if [ ! -x "$bindgen" ] || ! "$bindgen" --version 2>/dev/null | grep -q "$version"; then
    echo "fetching wasm-bindgen $version ..."
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) host=x86_64-unknown-linux-musl ;;
        Darwin-x86_64) host=x86_64-apple-darwin ;;
        Darwin-arm64) host=aarch64-apple-darwin ;;
        *)
            echo "no prebuilt wasm-bindgen for $(uname -s)-$(uname -m)." >&2
            echo "cargo install wasm-bindgen-cli --version $version --locked" >&2
            exit 1
            ;;
    esac
    base="wasm-bindgen-${version}-${host}"
    url="https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${version}"
    curl -sSL -o "target/wasm-bindgen/${base}.tar.gz" "${url}/${base}.tar.gz"
    curl -sSL -o "target/wasm-bindgen/${base}.tar.gz.sha256sum" "${url}/${base}.tar.gz.sha256sum"
    (cd target/wasm-bindgen && sed 's#gh-release/##' "${base}.tar.gz.sha256sum" \
        | { command -v sha256sum >/dev/null 2>&1 && sha256sum -c - || shasum -a 256 -c -; })
    tar xzf "target/wasm-bindgen/${base}.tar.gz" -C target/wasm-bindgen --strip-components=1
    rm -f "target/wasm-bindgen/${base}.tar.gz" "target/wasm-bindgen/${base}.tar.gz.sha256sum"
fi

# 3. Build the cdylib in the digest-pinned cross image. Both caches — the build
#    output and Cargo's own home (registry + git checkouts of the OCCT bindings)
#    — live on the host, so repeat runs are incremental and need no network. The
#    container runs as root, so hand the caches back to the invoking user.
echo "building ferrocad_web for wasm32 (first run pulls the cross image) ..."
docker run --rm \
    -v "$PWD":/work -w /work \
    -e OCCT_WASM_ROOT="/work/.occt-wasm/$OCCT_NAME" \
    -e CARGO_HOME="/work/$CARGO_HOME_HOST" \
    -e CARGO_TARGET_DIR="/work/$CARGO_TARGET" \
    -e HTTP_PROXY -e HTTPS_PROXY -e ALL_PROXY \
    "$IMAGE" \
    cargo build --release -j "${JOBS:-2}" -p ferrocad_web --target wasm32-unknown-unknown --locked
docker run --rm -v "$PWD":/work "$IMAGE" \
    chown -R "$(id -u):$(id -g)" "/work/$CARGO_TARGET" "/work/$CARGO_HOME_HOST"

# 4. Generate the JS glue and exercise the module without a browser.
"$bindgen" --target web \
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

# 5. Serve it. wasm-bindgen's `--target web` output must be fetched over HTTP.
if [ "$NO_OPEN" = 0 ]; then
    (xdg-open "http://localhost:$PORT" >/dev/null 2>&1 \
        || open "http://localhost:$PORT" >/dev/null 2>&1 \
        || true) &
fi
echo "serving http://localhost:$PORT  (Ctrl-C to stop)"
cd crates/ferrocad_web/www
exec python3 -m http.server "$PORT"
