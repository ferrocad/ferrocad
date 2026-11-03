# Vendored `bite-gp-apple` 1.25.202 — temporary macOS/iOS build fix

This directory is a verbatim copy of the crates.io source for
`bite-gp-apple 1.25.202` (upstream `bite-gpui` branch `bite_v1.25.x`, commit
`19271206c7e12504f1a0b3879a85c90df5d9b4c4`, per the crate's
`[package.metadata.bite]`), except for the deletions below. `Cargo.toml.orig`,
the license files and `README.md` are unchanged.

It is wired in through the workspace root `Cargo.toml`:

```toml
[patch.crates-io]
bite-gp-apple = { path = "third_party/bite-gp-apple" }
```

## Why

`bite-gp-apple` uses `cocoa` but never declares it, so the crate cannot compile
for `cfg(any(target_os = "macos", target_os = "ios"))`:

1. `src/metal_renderer.rs` — a five-line import block whose names are unused:

   ```rust
   use cocoa::{
       base::{NO, YES},
       foundation::{NSSize, NSUInteger},
       quartzcore::AutoresizingMask,
   };
   ```

2. `src/text_system.rs` — `use cocoa::appkit::CGFloat;`, which collides with the
   `CGFloat` this file already imports from `core_graphics::base` in the next
   `use` group (`E0252: the name CGFloat is defined multiple times`).

Both appeared in 1.25.202; the first was already present in 1.24.402.

## Change

Deleted those two `use cocoa…` lines. No dependency was added. Nothing else was
changed.

## Why not just declare `cocoa`

Adding

```toml
[target.'cfg(any(target_os = "macos", target_os = "ios"))'.dependencies]
cocoa = "0.26"
```

does resolve offline (`cocoa 0.26.0` is already in `Cargo.lock` through
`bite-gp-macos`, so there are no new packages), but it does not make the crate
compile:

* `NO`, `YES`, `NSSize` and `AutoresizingMask` are **unused** — each appears
  only on its import line. `AutoresizingMask` is not what the code uses; the
  macOS-only call site uses `objc2_quartz_core::CAAutoresizingMask`
  (`src/metal_renderer.rs`, the `setAutoresizingMask` call).
* `NSUInteger` is **already imported from `metal`** at the top of the same file.
  `metal::NSUInteger` is `u64`/`u32`; `cocoa::foundation::NSUInteger` is a
  different alias, so bringing both into scope is `E0252`.
* `cocoa`'s `quartzcore` module is `#[cfg(target_os = "macos")]`, while this
  crate is gated on `any(target_os = "macos", target_os = "ios")`. Even after
  declaring `cocoa`, the `quartzcore` import would fail to build for iOS.
* Likewise `CGFloat` in `text_system.rs` is already in scope from
  `core_graphics::base`, so the `cocoa` import is redundant as well as
  undeclared.

Deleting the dead imports is therefore smaller and fixes both Apple targets.

## Verification

Run from the workspace root with `--offline`:

* `cargo metadata --offline --format-version=1` — succeeds; the
  `bite-gp-apple` package resolves to `path+file:///.../third_party/bite-gp-apple`,
  with no `cocoa` dependency.
* `cargo check -p ferrocad_gpui --offline` — passes.
* `cargo check -p ferrocad_parametric --offline` — passes.
* `cargo tree -p ferrocad_gpui --target aarch64-apple-darwin --offline -i bite-gp-apple`
  — succeeds, showing the intended `bite-gp-apple -> bite-gp-macos -> bite-gpui`
  path.

Limitation: the `aarch64-apple-darwin` std is not installed and there is no
network, so the Apple-only code was **not compiled**. A real macOS verification
still needs the `aarch64-apple-darwin` (or `x86_64-apple-darwin`) target plus the
macOS SDK, `metal`, `CoreFoundation`, `CoreGraphics`, `CoreVideo` and
`QuartzCore` frameworks, and a working linker.

An audit of all `use <crate>::` roots under `src/` found no other undeclared
Apple-target dependency in 1.25.202. (`dispatcher` is the crate's own
`mod dispatcher`.)

## Removal

Delete this directory and the `[patch.crates-io]` block once the upstream fork
ships a release that removes the dead `cocoa` imports (or otherwise compiles
cleanly on macOS and iOS). This patch is temporary and carries no version bump.
