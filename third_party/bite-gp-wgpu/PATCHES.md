# Vendored `bite-gp-wgpu` 1.25.202 — wasm32 build and interop fixes

This directory is a copy of the crates.io source for `bite-gp-wgpu 1.25.202`
(package of library `gpui_wgpu`; upstream `bite-gpui` branch `bite_v1.25.x`,
commit `19271206c7e12504f1a0b3879a85c90df5d9b4c4`, per the crate's
`[package.metadata.bite]`). Changed from that copy:

| File | Change |
| --- | --- |
| `Cargo.toml` | new `wasm-interop` feature (see 3) |
| `src/gpui_wgpu.rs` | `mod web_renderer;` under `cfg(target_family = "wasm")` |
| `src/web_renderer.rs` | **new**: `impl GpuRenderer for WgpuRenderer` for the browser |
| `src/imported_texture.rs` | the `ImportedTextureExt` impl is opt-in on wasm |
| `src/wgpu_context.rs` | `session_for`'s non-Linux `cfg` excludes wasm |
| `src/wgpu_renderer.rs` | Linux-only imports/helpers and the dma-buf release path are `cfg`-gated (see 1); `core()`, `WgpuRendererCore.resources`, `WgpuResources`/`WgpuRendererCore` and their `device`/`queue` fields are `pub(crate)` |

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
`wasm32-unknown-unknown`. Four things do not account for that target, and one
more is needed before a browser window can composite externally rendered
textures.

### 1. Linux-only code is compiled on wasm

1.24.402 already had a `cfg` mismatch (fix 2 below). 1.25.202 added three more
dma-buf/`OwnedFd` paths that are unconditional:

* `use std::os::fd::OwnedFd;` at module scope — `std::os::fd` does not exist on
  wasm, so the import alone is `E0432`.
* `fn yuv_to_rgb(..)` uses `gpui_engine::{YuvMatrix, YuvRange}`; the function is
  only called from the Linux dmabuf path, but it was compiled everywhere.
* the writeback path (`record_frame`) and `release_unpresented` collect
  `Self::surface_releases(scene)` and signal `crate::dmabuf::signal_release`,
  whose module is `#[cfg(target_os = "linux")]`.

**Change:** `#[cfg(target_os = "linux")]` on the `OwnedFd` import, on
`yuv_to_rgb`, on `surface_releases`, on the `releases` binding and release
block in `record_frame`, and around the body of `release_unpresented` (with
`let _ = scene;` on the other targets, so the parameter stays used). No
behaviour changes on Linux; wasm simply does not compile the code it has no
dma-buf source for.

### 2. `SessionGpuId` is imported under one gate and used under another

`src/wgpu_context.rs` imports the GPU identity types under
`#[cfg(not(target_family = "wasm"))]`, but the non-Linux `session_for` was
gated `#[cfg(not(target_os = "linux"))]`. wasm is `not(linux)`, so the function
compiles while its return type does not:

```
error[E0425]: cannot find type `SessionGpuId` in this scope
 note: found an item that was configured out
```

Its only caller, `adapter_info`, is already `#[cfg(not(target_family =
"wasm"))]`.

**Change:** `#[cfg(not(target_os = "linux"))]` →
`#[cfg(all(not(target_os = "linux"), not(target_family = "wasm")))]`.

### 3. The imported-texture impl cannot meet its `Send + Sync` bound on wasm

`src/imported_texture.rs` implements `ImportedTextureExt` for
`wgpu::TextureView` by calling `ImportedTextureHandle::new`, whose signature
requires `impl Any + Send + Sync` (`bite-gp-engine/src/custom_render.rs`). On
wasm, wgpu's handles are `Send + Sync` only when its build script's `send_sync`
cfg holds — `any(native, all(feature = "fragile-send-sync-non-atomic-wasm",
not(target_feature = "atomics")))` — so a consumer that resolves `wgpu` with
default features gets `Rc`-backed handles and 46 × `E0277`.

Nothing in `gpui_web` imports a texture, so a plain browser build does not need
the impl; a browser that *composites* a wgpu viewport does. It is therefore a
feature rather than a `cfg`:

**Change:** `Cargo.toml` gains `wasm-interop = []`; the impl is
`#[cfg(any(not(target_family = "wasm"), feature = "wasm-interop"))]`.

Enabling `wasm-interop` requires the consumer to enable wgpu's
`fragile-send-sync-non-atomic-wasm` too (verified: that alone makes
`wgpu::TextureView`/`wgpu::Texture` `Send + Sync` on wasm32-unknown-unknown,
with or without the `webgpu` backend).

### 4. No `GpuRenderer` impl for the browser

`impl GpuRenderer for WgpuRenderer` lives in `platform_renderer.rs`, which is
`#[cfg(not(any(target_family = "wasm", target_os = "macos")))]`
(`gpui_wgpu.rs`). `GpuCanvasContext::try_device::<R>` requires `R:
GpuRenderer` (`bite-gp-authoring/src/elements/gpu_canvas.rs`), so on wasm there
is no way for a `gpu_canvas` producer to reach the window's device — which is
exactly what `gpui_interop`'s same-device `WgpuSurface` needs.

The native impl reads the shared `WgpuContext` slot; the browser renderer has no
such slot (`new_from_surface` passes `None`, and `gpu_context()` is gated out).
The device and queue live in the renderer core on both targets.

**Change:** `src/web_renderer.rs` (wasm-only) implements the trait against
`WgpuRendererCore::resources`, returning the same
`(Arc<wgpu::Device>, Arc<wgpu::Queue>)` the native impl returns. Reaching it
needs `pub(crate)` on `core()`, on `WgpuRendererCore.resources` and on the
`WgpuResources` device/queue fields — hence the visibility lines in the table
above. No behaviour changes: the fields were already private to the module.

## Using the browser interop path

A browser producer enables both halves:

```toml
gpui_wgpu = { package = "bite-gp-wgpu", version = "=1.25.202", features = ["wasm-interop"] }
wgpu = { version = "30", features = ["fragile-send-sync-non-atomic-wasm"] }
```

Then the same code the native same-device path uses works unchanged:
`GpuCanvasContext::try_device::<WgpuRenderer>()` → `WgpuSurface::new(&device, …)`
→ render into `surface.view()` → `view().to_imported_handle()` →
`gpu.paint_texture(…)`. FerroCAD's probe
(`crates/ferrocad_web_gpui`) does exactly this and composites a live 320×320
texture in Chrome on WebGL2.

## Removing this

Drop the `[patch.crates-io]` entry once the family ships the fixes upstream:

- the wasm-unsafe imports: <https://github.com/bite-gpui/bite-gpui/issues/21>
- the `cfg` mismatch: <https://github.com/bite-gpui/bite-gpui/issues/16>
- the `Send + Sync` bound: <https://github.com/bite-gpui/bite-gpui/issues/17>
- the missing browser `GpuRenderer`: <https://github.com/bite-gpui/bite-gpui/issues/19>

## Verified

- `cargo build --release -p ferrocad_web_gpui --target wasm32-unknown-unknown`
  in the cross image: finished on stable 1.95.0 (dead-code warnings only:
  `surface_params` and `gpu_preference` are unread on wasm).
- The probe in `crates/ferrocad_web_gpui`: headless Chrome with SwiftShader
  selects `requested=WebGl, selected=Gl` and paints the 40-triangle document.
- Native `cargo check -p ferrocad_gpui` and
  `cargo build -p ferrocad_parametric`: unchanged (fixes 1–2 are wasm-only
  `cfg`s, 3 is feature-gated, 4 is a new wasm-only module; the visibility
  widening changes no behaviour).
