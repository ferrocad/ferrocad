# Vendored `bite-gp-wgpu` 1.24.402 — wasm32 build and interop fixes

This directory is a copy of the crates.io source for `bite-gp-wgpu 1.24.402`
(package of library `gpui_wgpu`; upstream `bite-gpui` branch `bite_v1.24.x`,
commit `8b288f1a2a06ed463d56228823a2f28a911e8855`, per the crate's
`[package.metadata.bite]`). Changed from that copy:

| File | Change |
| --- | --- |
| `Cargo.toml` | new `wasm-interop` feature (see 2) |
| `src/gpui_wgpu.rs` | `mod web_renderer;` under `cfg(target_family = "wasm")` |
| `src/web_renderer.rs` | **new**: `impl GpuRenderer for WgpuRenderer` for the browser |
| `src/imported_texture.rs` | the `ImportedTextureExt` impl is opt-in on wasm |
| `src/wgpu_context.rs` | `session_for`'s non-Linux `cfg` excludes wasm |
| `src/wgpu_renderer.rs` | `core()`, `WgpuRendererCore.resources`, `WgpuResources`/`WgpuRendererCore` and their `device`/`queue` fields are `pub(crate)` |

Everything else (`Cargo.toml.orig`, license files, `README.md`, the other
sources) is unchanged.

It is wired in through the workspace root `Cargo.toml` together with
[`../bite-gp-web`](../bite-gp-web/PATCHES.md):

```toml
[patch.crates-io]
bite-gp-web = { path = "third_party/bite-gp-web" }
bite-gp-wgpu = { path = "third_party/bite-gp-wgpu" }
```

## Why

GPUI's browser backend (`gpui_web`) builds this crate and its `WgpuRenderer` for
`wasm32-unknown-unknown`. Three things do not account for that target, and one
more is needed before a browser window can composite externally rendered
textures.

### 1. `SessionGpuId` is imported under one gate and used under another

`src/wgpu_context.rs:1-4` imports the GPU identity types under
`#[cfg(not(target_family = "wasm"))]`, but the non-Linux `session_for` at `:196`
was gated `#[cfg(not(target_os = "linux"))]`. wasm is `not(linux)`, so the
function compiles while its return type does not:

```
error[E0425]: cannot find type `SessionGpuId` in this scope
 --> .../bite-gp-wgpu-1.24.402/src/wgpu_context.rs:197:45
   note: found an item that was configured out
error[E0433]: cannot find type `SessionGpuId` in this scope
```

Its only caller, `adapter_info` (`:133`), is already
`#[cfg(not(target_family = "wasm"))]`.

**Change:** `#[cfg(not(target_os = "linux"))]` →
`#[cfg(all(not(target_os = "linux"), not(target_family = "wasm")))]`.

### 2. The imported-texture impl cannot meet its `Send + Sync` bound on wasm

`src/imported_texture.rs:35` implements `ImportedTextureExt` for
`wgpu::TextureView` by calling `ImportedTextureHandle::new`, whose signature
requires `impl Any + Send + Sync` (`bite-gp-engine/src/custom_render.rs:21`).
On wasm, wgpu's handles are `Send + Sync` only when its build script's
`send_sync` cfg holds — `any(native, all(feature =
"fragile-send-sync-non-atomic-wasm", not(target_feature = "atomics")))` — so a
consumer that resolves `wgpu` with default features (which include the `webgpu`
backend) gets `Rc`-backed handles and 46 × `E0277`.

Nothing in `gpui_web` imports a texture, so a plain browser build does not need
the impl; a browser that *composites* a wgpu viewport does. It is therefore a
feature rather than a `cfg`:

**Change:** `Cargo.toml` gains `wasm-interop = []`; the impl is
`#[cfg(any(not(target_family = "wasm"), feature = "wasm-interop"))]`.

Enabling `wasm-interop` requires the consumer to enable wgpu's
`fragile-send-sync-non-atomic-wasm` too (verified: that alone makes
`wgpu::TextureView`/`wgpu::Texture` `Send + Sync` on wasm32-unknown-unknown,
with or without the `webgpu` backend).

### 3. No `GpuRenderer` impl for the browser

`impl GpuRenderer for WgpuRenderer` lives in `platform_renderer.rs`, which is
`#[cfg(not(any(target_family = "wasm", target_os = "macos")))]`
(`gpui_wgpu.rs:5-6`). `GpuCanvasContext::try_device::<R>` requires `R:
GpuRenderer` (`bite-gp-authoring/src/elements/gpu_canvas.rs:108`), so on wasm
there is no way for a `gpu_canvas` producer to reach the window's device — which
is exactly what `gpui_interop`'s same-device `WgpuSurface` needs.

The native impl reads the shared `WgpuContext` slot; the browser renderer has no
such slot (`new_from_surface` passes `None`, and `gpu_context()` is
`#[cfg(not(target_family = "wasm"))]`). The device and queue live in the
renderer core on both targets.

**Change:** `src/web_renderer.rs` (wasm-only) implements the trait against
`WgpuRendererCore::resources`, returning the same
`(Arc<wgpu::Device>, Arc<wgpu::Queue>)` the native impl returns. Reaching it
needs `pub(crate)` on `core()`, on `WgpuRendererCore.resources` and on the
`WgpuResources` device/queue fields — hence the visibility lines in the table
above. No behaviour changes: the fields were already private to the module.

## Using the browser interop path

A browser producer enables both halves:

```toml
gpui_wgpu = { package = "bite-gp-wgpu", version = "=1.24.402", features = ["wasm-interop"] }
wgpu = { version = "29", features = ["fragile-send-sync-non-atomic-wasm"] }
```

Then the same code the native same-device path uses works unchanged:
`GpuCanvasContext::try_device::<WgpuRenderer>()` → `WgpuSurface::new(&device, …)`
→ render into `surface.view()` → `view().to_imported_handle()` →
`gpu.paint_texture(…)`. FerroCAD's probe
(`crates/ferrocad_web_gpui`) does exactly this and composites a live 320×320
texture in Chrome on WebGL2.

## Removing this

Drop the `[patch.crates-io]` entry once the family ships the fixes upstream:

- the `cfg` mismatch: <https://github.com/bite-gpui/bite-gpui/issues/16>
- the `Send + Sync` bound: <https://github.com/bite-gpui/bite-gpui/issues/17>
- the missing browser `GpuRenderer`: <https://github.com/bite-gpui/bite-gpui/issues/19>

## Verified

- `cargo check --target wasm32-unknown-unknown` for a probe depending on
  `bite-gpui = "=1.24.402"`, this crate and `wgpu = "29"`: finished on stable
  1.95.0 (one dead-code warning: `WgpuRenderer::gpu_preference` is unread on
  wasm).
- The probe in `crates/ferrocad_web_gpui`: headless Chrome with SwiftShader
  selects `requested=WebGl, selected=Gl`; the first paint logs
  `painted 320x320 (frame 1)`; two screenshots at different virtual times show
  different colours in the square, so the surface is re-rendered and
  re-composited rather than cached.
- Native `cargo check -p ferrocad_gpui` / `cargo build -p ferrocad_parametric`:
  unchanged (the first two changes are wasm-only `cfg`s; the third is a new
  wasm-only module; the visibility widening changes no behaviour).
