# Vendored `bite-gp-web` 1.24.402 — wasm32 build fixes

This directory is a verbatim copy of the crates.io source for `bite-gp-web
1.24.402` (package of library `gpui_web`; upstream `bite-gpui` branch
`bite_v1.24.x`, commit `8b288f1a2a06ed463d56228823a2f28a911e8855`, per the
crate's `[package.metadata.bite]`). Only `Cargo.toml` and `src/window.rs`
differ from that copy. `Cargo.toml.orig`, the license files and `README.md` are
unchanged.

It is wired in through the workspace root `Cargo.toml`:

```toml
[patch.crates-io]
bite-gp-web = { path = "third_party/bite-gp-web" }
```

## Why

`bite-gpui` 1.24.402's `platform` feature builds the browser backend for
`wasm32-unknown-unknown`, but this crate does not compile as published:

1. **`default = ["multithreaded"]` pulls a nightly-only dependency.**
   `multithreaded` enables `wasm_thread`, whose first line is

   ```rust
   #![cfg_attr(target_arch = "wasm32", feature(stdarch_wasm_atomic_wait))]
   ```

   That is `E0554` on stable, the feature is still unstable on 1.97.1, 1.98.1
   and 1.100.0-nightly, and 0.3.3 is the newest release. A consumer cannot turn
   it off either: `bite-gpui` declares the dependency without
   `default-features = false`, and features are additive. The threaded path also
   needs `SharedArrayBuffer`, COOP/COEP headers and a `-Zbuild-std` std built
   with atomics, so it is not usable on stable regardless. The single-threaded
   path is complete (`src/dispatcher.rs:163` `#[cfg(not(feature =
   "multithreaded"))]`, `supports_threads`); it is only unreachable.
2. **`src/window.rs` imports two names `gpui_platform` does not export.**
   `PlatformAtlas` is a trait in `bite-gp-engine` (`src/atlas.rs:56`) and `Scene`
   is a struct there too (`src/scene.rs:39`); `gpui_platform` re-exports only
   `gpui_types`. Neither name is used anywhere in `window.rs`, so this is a
   plain `E0432` on every target that compiles the file.

## Change

- `Cargo.toml`: `default = ["multithreaded"]` → `default = []`. The feature is
  unchanged and still opt-in for a caller that wants it (and runs a nightly with
  a threaded std).
- `src/window.rs`: dropped `PlatformAtlas,` (line 13) and `Scene,` (line 15)
  from the `gpui_platform` import list.

No dependency was added or removed, and no behaviour changes on any target.

## Why not patch `bite-gpui` instead

`bite-gpui` is the natural place for the feature default (`gpui_web = { ...,
default-features = false }` under `cfg(target_family = "wasm")`), but vendoring
it means copying a much larger crate whose native-OS paths we already patch
separately (`bite-gp-apple`). Patching the small leaf crate keeps the vendored
surface at two crates. Same for the imports: deleting them in `gpui_web` is a
two-token change wherever it lands.

## Removing this

Drop the `[patch.crates-io]` entry once the family ships both fixes upstream:

- the default feature: <https://github.com/bite-gpui/bite-gpui/issues/15>
- the two imports: <https://github.com/bite-gpui/bite-gpui/issues/18>

## Verified

- `cargo check --target wasm32-unknown-unknown` for a scratch probe that
  depends on `bite-gpui = "=1.24.402"`, `bite-gp-wgpu = "=1.24.402"` and
  `wgpu = "29"`, with this directory and `third_party/bite-gp-wgpu` patching
  `crates-io`: **finished** on stable 1.95.0 with the `wasm32-unknown-unknown`
  std (one dead-code warning in `bite-gp-wgpu`).
- Native `cargo check -p ferrocad_gpui` is unaffected, and `wasm_thread` leaves
  `Cargo.lock` entirely (it is no longer reachable from the graph).
