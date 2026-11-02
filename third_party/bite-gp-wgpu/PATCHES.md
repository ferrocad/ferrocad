# Vendored `bite-gp-wgpu` 1.24.402 — wasm32 build fixes

This directory is a verbatim copy of the crates.io source for `bite-gp-wgpu
1.24.402` (package of library `gpui_wgpu`; upstream `bite-gpui` branch
`bite_v1.24.x`, commit `8b288f1a2a06ed463d56228823a2f28a911e8855`, per the
crate's `[package.metadata.bite]`). Only `src/wgpu_context.rs` and
`src/imported_texture.rs` differ from that copy. `Cargo.toml.orig`, the license
files and `README.md` are unchanged.

It is wired in through the workspace root `Cargo.toml` together with
[`../bite-gp-web`](../bite-gp-web/PATCHES.md):

```toml
[patch.crates-io]
bite-gp-web = { path = "third_party/bite-gp-web" }
bite-gp-wgpu = { path = "third_party/bite-gp-wgpu" }
```

## Why

GPUI's browser backend (`gpui_web`) depends on this crate on
`wasm32-unknown-unknown`, and two `cfg`s do not account for that target.

### 1. `SessionGpuId` is imported under one gate and used under another

`src/wgpu_context.rs:1-4` imports `SessionGpuId` (and the other GPU identity
types) under `#[cfg(not(target_family = "wasm"))]`, but the non-Linux
`session_for` at `:196` was gated `#[cfg(not(target_os = "linux"))]`. wasm is
`not(linux)`, so the function compiles while its return type does not:

```
error[E0425]: cannot find type `SessionGpuId` in this scope
 --> .../bite-gp-wgpu-1.24.402/src/wgpu_context.rs:197:45
   note: found an item that was configured out
error[E0433]: cannot find type `SessionGpuId` in this scope
```

Its only caller, `adapter_info` (`:133`), is already
`#[cfg(not(target_family = "wasm"))]`, so gating the function is enough.

**Change:** `#[cfg(not(target_os = "linux"))]` →
`#[cfg(all(not(target_os = "linux"), not(target_family = "wasm")))]`.

### 2. `ImportedTextureExt for wgpu::TextureView` cannot satisfy `Send + Sync` on wasm

`src/imported_texture.rs:35` implements `ImportedTextureExt` for
`wgpu::TextureView` by calling `ImportedTextureHandle::new`, whose signature
requires `impl Any + Send + Sync` (`bite-gp-engine/src/custom_render.rs:21`).
On wasm, wgpu's handle types are `Send + Sync` only when its build script's
`send_sync` cfg holds, i.e. the `webgpu` backend is off **and**
`fragile-send-sync-non-atomic-wasm` is on:

```rust
send_sync: { any(native, all(feature = "fragile-send-sync-non-atomic-wasm", not(target_feature = "atomics"))) }
```

This crate's own wasm dependency is correct (`default-features = false,
features = ["webgl"]`), but a consumer's default `wgpu = "29"` re-enables
`webgpu` through feature unification, so the handles become `Rc`-backed and the
impl produces 46 × `E0277` (`dyn DynInstance cannot be shared between threads
safely`, `Rc<webgpu::DropGuard> cannot be sent between threads safely`).

`gpui_web` never imports a texture — the module is only used by the native
renderer's same-device/dma-buf path — so the impl is gated for now.

**Change:** `#[cfg(not(target_family = "wasm"))]` on the
`impl ImportedTextureExt for wgpu::TextureView` block.

## If the imported-texture path is needed on wasm

FerroCAD wants it: `gpui_interop`'s `WgpuSurface` is the same-device path for a
browser viewport (`view.to_imported_handle()` → `paint_texture`). There are two
ways to re-enable it, and the choice belongs to whoever lands that:

- **Enable the escape hatch.** Give the wasm `wgpu` dependency
  `fragile-send-sync-non-atomic-wasm` and keep `webgpu` off. Verified on
  wasm32-unknown-unknown/stable 1.95:
  `wgpu::TextureView` and `wgpu::Texture` become `Send + Sync`. Then delete the
  gate here.
- **Fix the bound upstream.** Make `ImportedTextureHandle::new`'s bound
  wasm-aware (`Arc<dyn Any>` where handles are `Rc`-backed) in
  `bite-gp-engine`. Cleaner, but a shared public type.

## Removing this

Drop the `[patch.crates-io]` entry once the family ships both fixes upstream:

- the `cfg` mismatch: <https://github.com/bite-gpui/bite-gpui/issues/16>
- the `Send + Sync` bound: <https://github.com/bite-gpui/bite-gpui/issues/17>

## Verified

- `cargo check --target wasm32-unknown-unknown` for a scratch probe that
  depends on `bite-gpui = "=1.24.402"`, `bite-gp-wgpu = "=1.24.402"` and
  `wgpu = "29"`, with this directory and `third_party/bite-gp-web` patching
  `crates-io`: **finished** on stable 1.95.0 with the `wasm32-unknown-unknown`
  std (one dead-code warning: `WgpuRenderer::gpu_preference` is unread on wasm).
- `gpui_interop` 0.1.3 with `features = ["gpui", "wgpu"]` also checks clean for
  wasm with this patch set — it only *documents* the `to_imported_handle` call,
  so the gate does not break it; the caller is what needs the fix above.
- Native `cargo check -p ferrocad_gpui` (Linux) is unaffected: the first change
  is a wasm-only `cfg`, the second keeps native behaviour byte-for-byte.
