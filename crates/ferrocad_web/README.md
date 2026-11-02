# ferrocad_web

The real OCCT kernel, compiled to WebAssembly, drawing a **`Part` document** on a
`<canvas>`: `Document::recompute` runs a `Part::Box` and a `Part::Fillet` through
the same `GeometryBackend` seam the desktop app uses, and the recomputed shape is
tessellated and drawn every animation frame.

The page drives the feature: a radius slider rebuilds the document, and a radius
the kernel refuses is **reported, not fatal** — the box that is still valid stays
on screen and the reason is shown (`OCCT operation \`fillet\` did not complete`).
That is the point of the failure model in
[`operation-failure-modelling.md`](../../docs/project/tasks/operation-failure-modelling.md):
what used to abort the process is now a value.

**Drag** with the primary button to orbit; the **wheel** zooms. WebGL2 is the
required path; WebGPU is used when the browser offers it.

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
| `src/lib.rs` | `#[wasm_bindgen(start)]`, the camera pose, the page API (`set_fillet_radius`, `fillet_status`, `triangles`) |
| `src/scene.rs` | the `Part` document: `Part::Box` + `Part::Fillet`, `Document::recompute`, tessellation, failure state |
| `src/wasi.rs` | 13 link-time WASI stubs, so the module imports no `wasi_snapshot_preview1` |
| `www/index.html` | the page: a `<canvas id="cad">`, the radius slider, the status line |
| `www/smoke.mjs` | Node check: zero WASI imports, 40 triangles, a refused fillet is reported and keeps the last good shape |
| `run.sh` | the one command above |

The layers, bottom-up: `ferrocad_occt` (links the prebuilt static OCCT libraries)
implements `ferrocad_geom::GeometryBackend`; `ferrocad_part` registers its object
types against it and `ferrocad_core`'s `Document` recomputes them; `ferrocad_stage`
turns the result into draw packets; `ferrocad_wgpu` renders them (it gained
`with_sample_count_and_format` so it can draw straight into a canvas texture);
`ferrocad_web_viewport` — the wasm analogue of the GPUI `ferrocad_viewport` — owns
the canvas surface, device and input. Orbit and zoom are
`ferrocad_graphics::Camera::{orbit, dolly}`, the same maths the GPUI viewport uses,
so the two behave alike. See
[`docs/project/research/web-wasm-host.md`](../../docs/project/research/web-wasm-host.md)
for the measurements and for what is still unverified.

The prebuilt wasm kernel omits `TKFeat`, so `GeometryBackend::split_shape` (which
`BRepFeat_SplitShape` implements) is a reported error on wasm rather than a link
failure for every consumer of the backend. Nothing here calls it.

## Limits

This is a canvas, not an application: no resize observer, picking, text or GPUI
shell, and the scale factor is fixed at 1 (a HiDPI canvas would upscale the
backing store in the host). MSAA is off (one sample). The canvas is 960×600 and
CSS-scaled.

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
