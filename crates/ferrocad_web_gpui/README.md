# ferrocad_web_gpui

A GPUI window in the browser, compositing a FerroCAD **`Part` document** through
[`gpui_interop`](https://crates.io/crates/gpui_interop).

This is step 2's probe: the smallest thing that puts the real product path —
document, OCCT kernel, stage, renderer — inside a GPUI window on WebGL2, and
composites the result without a CPU copy. It has no CPython and no shell; see
"What it does not have".

## What it proves

The filleted box on the page comes from `Document::recompute`, not from literal
geometry:

1. `Document` with `Part::Box` and `Part::Fillet` is recomputed by the OCCT kernel
   compiled to wasm (`ferrocad_occt`), exactly as `ferrocad_web`'s demo does.
2. The resulting shape is tessellated and turned into `ferrocad_stage` draw
   packets (shaded faces + edges: 2 packets, 40 triangles for the 10 mm box).
3. `ferrocad_wgpu` — the renderer the canvas demo uses — draws them into a
   `gpui_interop::WgpuSurface` created on the GPUI window's own device.
   `WgpuRenderer::render` takes any texture view, so the offscreen target needed
   no renderer changes.
4. `surface.view().to_imported_handle()` + `paint_texture` composite it into the
   GPUI scene. That needs the `wasm-interop` feature on our vendored
   `bite-gp-wgpu` and wgpu's `fragile-send-sync-non-atomic-wasm`; see
   `third_party/bite-gp-wgpu/PATCHES.md`.
5. The window's device comes from
   `GpuCanvasContext::try_device::<gpui_wgpu::WgpuRenderer>()`, which needs a
   wasm `GpuRenderer` impl that upstream gates out (change 3 in the same file).
6. **Pointer input is `gpui_interop`'s own gateway**: `surface_input(canvas, ..)`
   reports `SurfaceEvent`s in physical pixels from the canvas's top-left corner
   and founds GPUI's drag on the down, so a drag keeps orbiting after the pointer
   leaves the canvas. The wheel is the view's own handler because the gateway's
   vocabulary has no scroll; it calls `Camera::dolly`, the same verb the canvas
   demo binds to its DOM listener.

The camera starts at the demo pose and moves only when the pointer does, so the
probe draws a live surface rather than a cached texture.

The 13 WASI link stubs the wasm kernel needs live in `crates/ferrocad_wasm`
(shared with `ferrocad_web`); `ferrocad_wasm::install()` keeps them linked.

## Run it

Linking OCCT needs the wasi-sdk toolchain whose wasm-EH encoding matches the
prebuilt kernel, so — like `ferrocad_web` — this builds inside
`ghcr.io/lzpel/cross-wasm32-unknown-unknown`. If you have never run the demo,
`crates/ferrocad_web/run.sh --build-only` once first: it fetches the kernel into
`.occt-wasm/` and the pinned wasm-bindgen CLI.

```sh
docker run --rm -v "$PWD":/work -w /work \
    -e OCCT_WASM_ROOT=/work/.occt-wasm/occt-8_0_1_rev3-wasm32_unknown_unknown \
    -e CARGO_HOME=/work/target/wasm-cargo-home \
    -e CARGO_TARGET_DIR=/work/target/wasm-cargo \
    ghcr.io/lzpel/cross-wasm32-unknown-unknown \
    cargo build --release -p ferrocad_web_gpui --target wasm32-unknown-unknown

# The container runs as root; hand the caches back.
docker run --rm -v "$PWD":/work ghcr.io/lzpel/cross-wasm32-unknown-unknown \
    chown -R "$(id -u):$(id -g)" /work/target/wasm-cargo /work/target/wasm-cargo-home

target/wasm-bindgen/wasm-bindgen --target web \
    --out-dir crates/ferrocad_web_gpui/www/pkg \
    target/wasm-cargo/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm

cd crates/ferrocad_web_gpui/www && python3 -m http.server 8081
```

Then open <http://localhost:8081>. The module is ~32 MB (GPUI + wgpu + OCCT);
the first load takes a moment.

## What it does not have

- **No text.** GPUI panics when it has to resolve a font and none was added
  (`DefaultTextSystem::resolve_font`), and loading fonts is `ferrocad_text`'s job.
  Status goes to the browser console through the logger `gpui::web_init`
  installs.
- **No `ferrocad_gpui`.** The shell embeds CPython (`pyo3 auto-initialize`), which
  does not build for `wasm32-unknown-unknown`; a wasm shell needs a separate crate
  or a `no-python` feature first.
- **No image fallback and no picking.** The native presenter degrades to a
  readback image when the device is unavailable; that path needs an async
  `map_async` on WebGL2 and is not covered here.

## Verified

Headless Chrome, WebGL2 through ANGLE/SwiftShader (`--use-angle=swiftshader
--enable-unsafe-swiftshader`):

```
[INFO] ferrocad probe: document tessellated to 40 triangles
[INFO] ferrocad probe: 2 packets from the document
[INFO] gpui_wgpu::wgpu_context: Browser graphics initialized: requested=WebGl, selected=Gl, adapter="ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device ...))"
[INFO] ferrocad probe: 2 packets, 320x320, frame 1
```

The screenshot shows the 10 mm box with a 3 mm fillet on `Edge1` — the rounded
lower-left edge — shaded with its twelve edges.

Pointer input was checked the same way: `index.html` honours `?drag=1`, which
synthesises one `pointerdown`/`pointermove`×12/`pointerup` sequence on the canvas
(the only reason that page has any scripting beyond `init()`). A capture of
`/?drag=1` differs from the same page without the drag in 22 027 pixels, all
inside the canvas, with the box turned from a front view to a corner view — the
gateway delivered the whole gesture.

`ferrocad_web`'s smoke test still passes after the WASI-stub extraction (40
triangles, refused fillet reported, `split: 1 direct, 1 left`).
