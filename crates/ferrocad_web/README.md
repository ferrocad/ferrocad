# ferrocad_web

The real OCCT kernel, compiled to WebAssembly, rendered through `ferrocad_wgpu`
on a `<canvas>`. A 10 mm box is built, one edge is filleted, and the result is
tessellated and drawn every animation frame. WebGL2 is the required path; WebGPU
is used when the browser offers it.

## Run the demo

```sh
crates/ferrocad_web/run.sh
```

Then open <http://localhost:8080> (the script opens it for you unless `--no-open`).

That is the whole workflow. On the first run the script:

1. downloads cadrum's prebuilt **OCCT 8.0.1** for `wasm32-unknown-unknown` into
   `.occt-wasm/`;
2. builds `ferrocad_web` in the digest-pinned
   `ghcr.io/lzpel/cross-wasm32-unknown-unknown` image, which carries the wasi-sdk
   toolchain whose wasm-EH encoding matches that kernel (the image is ~2.5 GB and
   is pulled on first use);
3. installs the **wasm-bindgen CLI version pinned by `Cargo.lock`** into
   `target/wasm-bindgen/`;
4. generates `www/pkg/` and runs the Node smoke test when Node is available;
5. serves `www/` with `python3 -m http.server`.

Later runs reuse `.occt-wasm/`, `target/wasm-bindgen/` and `target/wasm-cargo/`,
so only the changed crates rebuild.

Requirements: Docker (usable without `sudo`), `curl`, `tar` and `python3`. Node is
optional — it runs the smoke test. A host Rust toolchain is not needed: the
wasm-bindgen CLI version is read from `Cargo.lock` and the build happens inside
the image.

Options:

```sh
PORT=9000 crates/ferrocad_web/run.sh   # another port
JOBS=4 crates/ferrocad_web/run.sh      # more build parallelism (default 2)
crates/ferrocad_web/run.sh --no-open   # don't open a browser
crates/ferrocad_web/run.sh --build-only
```

## What is in here

| Path | What |
| --- | --- |
| `src/lib.rs` | `#[wasm_bindgen(start)]`, canvas surface, render loop |
| `src/wasi.rs` | 13 link-time WASI stubs, so the module imports no `wasi_snapshot_preview1` |
| `www/index.html` | the page: a `<canvas id="cad">` and one module script |
| `www/smoke.mjs` | Node check: zero WASI imports, 40 triangles |
| `run.sh` | the one command above |

The kernel is `ferrocad_occt` (which links the prebuilt static OCCT libraries);
the renderer is `ferrocad_wgpu`, which gained
`with_sample_count_and_format` so it can draw straight into a canvas texture. See
[`docs/project/research/web-wasm-host.md`](../../docs/project/research/web-wasm-host.md)
for the measurements and for what is still unverified.

## Limits

This is a canvas, not an application: no resize observer, picking, text or GPUI
shell. MSAA is off (one sample). The canvas is 960×600 and CSS-scaled.

Both paths have been checked in headless Chrome — WebGPU, and WebGL2 through
SwiftShader by hiding `navigator.gpu` — and both draw the filleted box. Two wgpu
29 details are load-bearing:

- The instance needs a web `DisplayHandle`. wgpu-core, the only context that can
do WebGL2, rejects `create_surface()` when there is no display handle at all
(`InstanceDescriptor::new_without_display_handle()` is the default), and the
canvas itself arrives in the window handle. `WebDisplay` in `src/lib.rs` is the
empty handle `raw-window-handle` defines for this.
- The device must request `adapter.limits()`, not `Limits::default()`: a WebGL2
adapter reports 0 for the compute limits it has no concept of, and asking for
65535 fails validation.
