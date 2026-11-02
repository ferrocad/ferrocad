# ferrocad_web_gpui

A GPUI window in the browser, compositing a wgpu-rendered surface through
[`gpui_interop`](https://crates.io/crates/gpui_interop).

This is step 2's probe: the smallest thing that exercises the seam
`ferrocad_gpui`'s interop presenter needs on `wasm32-unknown-unknown`. It has no
document, no CPython, no OCCT and no image path — see "What it does not have".

## What it proves

The square on the page is not a GPUI rectangle. It is a `wgpu` texture rendered
by our own command encoder, on the window renderer's own device, composited by
GPUI:

1. `GpuCanvasContext::try_device::<WgpuRenderer>()` answers in the browser, i.e.
   the window's renderer lends its `wgpu::Device` and `Queue`. That needs
   `impl GpuRenderer for WgpuRenderer` on wasm, which upstream gates out — see
   `third_party/bite-gp-wgpu/PATCHES.md` (change 3).
2. A `gpui_interop::WgpuSurface` allocated on that device can be rendered into
   with an ordinary wgpu encoder (no dma-buf, no fence, no cross-device import).
3. `surface.view().to_imported_handle()` typechecks on wasm and
   `paint_texture` composites the result into the scene. That needs the
   `wasm-interop` feature on our vendored `bite-gp-wgpu` plus wgpu's
   `fragile-send-sync-non-atomic-wasm` (change 2 in the same file).

The colour moves every frame, so a screenshot shows a live surface rather than a
cached texture.

## Run it

```sh
rustup target add wasm32-unknown-unknown
cargo build -p ferrocad_web_gpui --release --target wasm32-unknown-unknown

# wasm-bindgen CLI must match Cargo.lock; crates/ferrocad_web/run.sh fetches this
# pinned version into target/wasm-bindgen/ if it is not there yet.
target/wasm-bindgen/wasm-bindgen --target web \
    --out-dir crates/ferrocad_web_gpui/www/pkg \
    target/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm

cd crates/ferrocad_web_gpui/www && python3 -m http.server 8081
```

Then open <http://localhost:8081>. No Docker and no OCCT prefix: GPUI is pure
Rust, so this builds with the host toolchain.

## What it does not have

- **No text.** GPUI panics when it has to resolve a font and none was added
  (`DefaultTextSystem::resolve_font`), and loading fonts is `ferrocad_text`'s job,
  not this probe's. Status goes to the browser console through the logger
  `gpui::web_init` installs.
- **No `ferrocad_gpui`.** The shell embeds CPython (`pyo3 auto-initialize`), which
  does not build for `wasm32-unknown-unknown`; a wasm shell needs a separate
  crate or a `no-python` feature first.
- **No image fallback and no picking.** The native presenter degrades to a
  readback image when the device is unavailable; that path needs an async
  `map_async` on WebGL2 and is not covered here.

## Verified

Headless Chrome, WebGL2 through ANGLE/SwiftShader (`--use-angle=swiftshader
--enable-unsafe-swiftshader`):

```
[INFO] gpui_wgpu::wgpu_context: Browser graphics initialized: requested=WebGl, selected=Gl, adapter="ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device ...))"
[INFO] ferrocad probe: painted 320x320 (frame 1)
```

Two screenshots at different virtual times differ in the square's colour
(`(218, 173, 230)` vs `(220, 173, 228)` at the centre), which is what "live"
means here.
