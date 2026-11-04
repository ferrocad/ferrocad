#!/usr/bin/env sh
# Preparation for the browser host.
#
# Sourced by `crates/ferrocad_web_gpui/run.sh` (the GPUI shell). It prepares the
# one thing the host needs and should not duplicate:
#
#   1. cadrum's prebuilt OCCT 8.0.1 for `wasm32-unknown-unknown`, fetched into
#      `.occt-wasm/`;
#   2. `TKFeat`, the one toolkit that tarball prunes but `ferrocad_part` needs (35
#      translation units compiled in the cross image, cached once);
#   3. the wasm-bindgen CLI at exactly the version `Cargo.lock` pins.
#
# The caller must be at the repository root and own `set -e`; these functions assume
# it. Nothing here is host-specific, so a second host only writes its build, glue and
# serve steps.
#
# Variables set for the caller:
#   IMAGE      the digest-pinned cross image (the wasm-EH encoding must match the kernel)
#   OCCT_NAME  the kernel's directory name
#   KERNEL     the kernel's path, also what `OCCT_WASM_ROOT` must point at
#   BINDGEN    the wasm-bindgen CLI, once `prepare_wasm_bindgen` has run

IMAGE='ghcr.io/lzpel/cross-wasm32-unknown-unknown@sha256:4dc88df287da141e67588ba80ebf1d14d5cb24f5b05302c57b11b2a1e9bed1df'
OCCT_TARBALL='https://github.com/lzpel/cadrum/releases/download/occt-8_0_1_rev3/occt-8_0_1_rev3-wasm32_unknown_unknown.tar.gz'
OCCT_NAME='occt-8_0_1_rev3-wasm32_unknown_unknown'
KERNEL=".occt-wasm/$OCCT_NAME"
BINDGEN='target/wasm-bindgen/wasm-bindgen'

# The prebuilt wasm kernel, plus the TKFeat archive it prunes. Idempotent.
prepare_wasm_kernel() {
    mkdir -p .occt-wasm target/wasm-bindgen

    if [ ! -d "$KERNEL" ]; then
        echo "fetching the prebuilt wasm OCCT (8.0.1) ..." >&2
        curl -sSL -o .occt-wasm/occt.tar.gz "$OCCT_TARBALL"
        tar xzf .occt-wasm/occt.tar.gz -C .occt-wasm
        rm -f .occt-wasm/occt.tar.gz
    fi

    # `ferrocad_part` coerces its backend to `Arc<dyn GeometryBackend>`, which keeps
    # `split_shape` (BRepFeat) in the vtable, and the tarball prunes TKFeat. The build
    # itself lives in `tools/wasm-tkfeat.sh` so CI (which runs it inside the cross
    # image directly) shares one recipe. The container runs as the invoking user so
    # the archive is not root-owned.
    if [ ! -f "$KERNEL/lib/libTKFeat.a" ]; then
        echo "building TKFeat into the wasm kernel (one-off) ..." >&2
        docker run --rm --user "$(id -u):$(id -g)" \
            -v "$PWD":/work -w /work \
            -e SRC=/work/target/occt-src \
            -e OUT="/work/$KERNEL" \
            -e SYSROOT=/opt/wasi-sdk/share/wasi-sysroot \
            "$IMAGE" sh /work/tools/wasm-tkfeat.sh
        echo "built $KERNEL/lib/libTKFeat.a" >&2
    fi
}

# The wasm-bindgen CLI, at the version the lockfile pins (the CLI and the crate must
# match, or the glue's ABI check rejects the module). Idempotent.
prepare_wasm_bindgen() {
    mkdir -p target/wasm-bindgen
    version=$(awk '$0 == "name = \"wasm-bindgen\"" { found = 1 }
        found && /^version = / { gsub(/"/, "", $3); print $3; exit }' Cargo.lock)
    [ -n "$version" ] || {
        echo "could not read wasm-bindgen from Cargo.lock" >&2
        exit 1
    }

    if [ ! -x "$BINDGEN" ] || ! "$BINDGEN" --version 2>/dev/null | grep -q "$version"; then
        echo "fetching wasm-bindgen $version ..." >&2
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
        curl -sSL -o "target/wasm-bindgen/${base}.tar.gz.sha256sum" \
            "${url}/${base}.tar.gz.sha256sum"
        (cd target/wasm-bindgen && sed 's#gh-release/##' "${base}.tar.gz.sha256sum" \
            | { command -v sha256sum >/dev/null 2>&1 && sha256sum -c - || shasum -a 256 -c -; })
        tar xzf "target/wasm-bindgen/${base}.tar.gz" -C target/wasm-bindgen --strip-components=1
        rm -f "target/wasm-bindgen/${base}.tar.gz" "target/wasm-bindgen/${base}.tar.gz.sha256sum"
    fi
}
