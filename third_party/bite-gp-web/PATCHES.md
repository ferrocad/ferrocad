# Vendored `bite-gp-web` 1.25.202 — wasm32 build fixes

This directory is a verbatim copy of the crates.io source for `bite-gp-web
1.25.202` (package of library `gpui_web`; upstream `bite-gpui` branch
`bite_v1.25.x`, commit `19271206c7e12504f1a0b3879a85c90df5d9b4c4`, per the
crate's `[package.metadata.bite]`). Only `Cargo.toml` and `src/window.rs`
differ from that copy. `Cargo.toml.orig`, the license files and `README.md` are
unchanged.

It is wired in through the workspace root `Cargo.toml`:

```toml
[patch.crates-io]
bite-gp-web = { path = "third_party/bite-gp-web" }
```

## Why

`bite-gpui`'s `platform` feature builds the browser backend for
`wasm32-unknown-unknown`, but this crate does not compile as published:

1. **`default = ["multithreaded"]` pulls a nightly-only dependency.**
   `multithreaded` enables `wasm_thread`, whose first line is

   ```rust
   #![cfg_attr(target_arch = "wasm32", feature(stdarch_wasm_atomic_wait))]
   ```

   That is `E0554` on stable, and 0.3.3 is the newest release. Features are
   additive, so a consumer cannot turn it off; the threaded path also needs
   `SharedArrayBuffer`, COOP/COEP headers and a `-Zbuild-std` std built with
   atomics. The single-threaded path is complete
   (`src/dispatcher.rs` `#[cfg(not(feature = "multithreaded"))]`,
   `supports_threads`); it is only unreachable.

   **1.25.202 makes this worse**: `bite-gpui` 1.25.202 moved `multithreaded`
   into *its* default features (`default = ["platform", "multithreaded"]`,
   where 1.24.402 had `default = ["platform"]`), so `gpui_web/multithreaded`
   is turned on by the framework's own default and this crate's default no
   longer matters on its own. A wasm consumer must **also** take `bite-gpui`
   with `default-features = false, features = ["platform"]`; FerroCAD does
   that in `crates/ferrocad_web_gpui/Cargo.toml`. Both halves are needed: this
   patch keeps the leaf from enabling threads, the consumer's feature list
   keeps the framework from doing it on the leaf's behalf.
2. **`src/window.rs` imports two names `gpui_platform` does not export.**
   `PlatformAtlas` is a trait in `bite-gp-engine` and `Scene` is a struct
   there; `gpui_platform` re-exports only `gpui_types`. Neither name is used
   anywhere in `window.rs`, so this is a plain `E0432` on every target that
   compiles the file.

## Change

- `Cargo.toml`: `default = ["multithreaded"]` → `default = []`. The feature is
  unchanged and still opt-in for a caller that wants it (and runs a nightly with
  a threaded std).
- `src/window.rs`: dropped `PlatformAtlas,` and `Scene,` from the
  `gpui_platform` import list.

No dependency was added or removed, and no behaviour changes on any target.

## Why not patch `bite-gpui` instead

`bite-gpui` is the natural place for the feature default, but vendoring it
means copying a much larger crate whose native-OS paths we already patch
separately (`bite-gp-apple`). Patching the small leaf crate keeps the vendored
surface to three small crates. Same for the imports: deleting them in
`gpui_web` is a two-token change wherever it lands.

## Removing this

Drop the `[patch.crates-io]` entry once the family ships both fixes upstream:

- the default feature: <https://github.com/bite-gpui/bite-gpui/issues/15>
- the two imports: <https://github.com/bite-gpui/bite-gpui/issues/18>

## Verified

- `cargo build --release -p ferrocad_web_gpui --target wasm32-unknown-unknown`
  in the cross image: finished with stable 1.95.0 and `wasm_thread` no longer
  in the build graph.
- Native `cargo check -p ferrocad_gpui` is unaffected.
