# ferrocad_web_gpui

A GPUI window in the browser, compositing a FerroCAD wgpu scene through
[`gpui_interop`](https://crates.io/crates/gpui_interop).

This is step 2's probe: the smallest thing that exercises the seam
`ferrocad_gpui`'s interop presenter needs on `wasm32-unknown-unknown`. It has no
document, no CPython and no OCCT — see "What it does not have".

## What it proves

The cube on the page is drawn by **`ferrocad_wgpu`** — the renderer the canvas
demo uses — from **`ferrocad_stage`** draw packets, into a texture on the GPUI
window's own device, and composited by GPUI:

1. `GpuCanvasContext::try_device::<gpui_wgpu::WgpuRenderer>()` answers in the
   browser, i.e. the window's renderer lends its `wgpu::Device` and `Queue`. That
   needs `impl GpuRenderer for WgpuRenderer` on wasm, which upstream gates out —
   see `third_party/bite-gp-wgpu/PATCHES.md` (change 3).
2. A `gpui_interop::WgpuSurface` allocated on that device takes
   `WgpuRenderer::render(&surface.view(), &camera, &packets)` unchanged: the
   renderer renders into any texture view, so the offscreen target needs no
   special path.
3. `surface.view().to_imported_handle()` typechecks on wasm and `paint_texture`
   composites the result into the scene. That needs the `wasm-interop` feature on
   our vendored `bite-gp-wgpu` plus wgpu's `fragile-send-sync-non-atomic-wasm`
   (change 2 in the same file).

The camera orbits a little every frame, so a screenshot shows a live surface
rather than a cached texture.

The scene is a hand-built cube in the stage's own vocabulary (`ShapeItem` with a
`Tessellation`), because the point here is the GPU seam, not the document layer.
Swapping in a real document means swapping `cube_packets()` for
`ferrocad_stage`'s output from a recomputed document — the renderer call does not
change.

## Run it

```sh
rustup target add wasm32-unknown-unknown
cargo build -p ferrocad_web_gpui --release --target wasm32-unknown-unknown

# wasm-bindgen CLI must match Cargo.lock; fetch the pinned release into
# target/wasm-bindgen/ (crates/ferrocad_web/run.sh does the same for the demo).
target/wasm-bindgen/wasm-bindgen --target web \
    --out-dir crates/ferrocad_web_gpui/www/pkg \
    target/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm

cd crates/ferrocad_web_gpui/www && python3 -m http.server 8081
```

Then open <http://localhost:8081>. No Docker and no OCCT prefix: GPUI and the
renderer are pure Rust, so this builds with the host toolchain. First build after
touching `ferrocad_wgpu` takes a few minutes; later ones are incremental.

## What it does not have

- **No text.** GPUI panics when it has to resolve a font and none was added
  (`DefaultTextSystem::resolve_font`), and loading fonts is `ferrocad_text`'s job.
  Status goes to the browser console through the logger `gpui::web_init`
  installs.
- **No `ferrocad_gpui`.** The shell embeds CPython (`pyo3 auto-initialize`), which
  does not build for `wasm32-unknown-unknown`; a wasm shell needs a separate crate
  or a `no-python` feature first.
- **No document or OCCT.** The cube is literal geometry; `ferrocad_part` and the
  kernel are the next layer up.
- **No image fallback and no picking.** The native presenter degrades to a
  readback image when the device is unavailable; that path needs an async
  `map_async` on WebGL2 and is not covered here.

## Verified

Headless Chrome, WebGL2 through ANGLE/SwiftShader (`--use-angle=swiftshader
--enable-unsafe-swiftshader`):

```
[INFO] gpui_wgpu::wgpu_context: Browser graphics initialized: requested=WebGl, selected=Gl, adapter="ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device ...))"
[INFO] ferrocad probe: 2 packets, 320x320, frame 1
```

The screenshot shows the shaded cube with its twelve edges. Two captures at
different virtual times differ in 1245 pixels, all inside the 320×320 canvas
bounding box, which is the camera orbit — a live surface, not a cached frame.
