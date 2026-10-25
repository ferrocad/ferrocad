# macOS

Status: **blocked upstream** (2026-10-25). The Linux and Windows artifacts ship;
macOS does not build yet. This page records the diagnosis, the temporary patch in
this tree, and the checklist for turning the platform back on.

## 1. The blocker

`bite-gp-apple` (the GPUI Metal backend) does not compile for
`cfg(any(target_os = "macos", target_os = "ios"))`. The cause is a **dead import**,
not a missing feature or a missing manifest entry:

```rust
// bite-gp-apple 1.24.402, src/metal_renderer.rs
use cocoa::{
    base::{NO, YES},
    foundation::{NSSize, NSUInteger},
    quartzcore::AutoresizingMask,
};
```

`Cargo.toml` declares `block2`, `core-foundation`, `core-graphics`, `core-video`,
`dispatch2`, `mach2`, `metal`, `objc2`, `objc2-foundation`,
`objc2-quartz-core`, … and **not** `cocoa`. Adding `cocoa` does not help:

- `NO`, `YES`, `NSSize` and `AutoresizingMask` are unused; the `setAutoresizingMask`
  call site uses `objc2_quartz_core::CAAutoresizingMask`.
- `NSUInteger` is already imported from `metal` in the same file, so declaring
  `cocoa` produces `E0252` (the name is defined twice), not a working build.
- `cocoa`'s `quartzcore` module is `#[cfg(target_os = "macos")]`, so the import
  would still fail for iOS.

`1.25.202` has the same import plus a second instance in `src/text_system.rs`
(`use cocoa::appkit::CGFloat;`, while the file already imports `CGFloat` from
`core_graphics::base` — the same duplicate-import error).

Reported upstream as [bite-gpui/bite-gpui#14](https://github.com/bite-gpui/bite-gpui/issues/14)
with the evidence and the one-line-per-file fix.

### The temporary patch in this repository

`third_party/bite-gp-apple/` is a verbatim copy of the 1.24.402 crate with that
five-line import deleted, wired in from the workspace root:

```toml
[patch.crates-io]
bite-gp-apple = { path = "third_party/bite-gp-apple" }
```

`third_party/bite-gp-apple/PATCHES.md` has the full rationale, the verification
performed here, and the removal condition. The patch adds no dependency and no
lock package, and nothing changes on Linux or Windows (the crate is not selected
there). The family stays on 1.24.402: 1.24.401 does not compile for
`*-pc-windows-msvc`, and the caret ranges would otherwise float the transitive
`bite-gp-*` crates to 1.25.x and split the wgpu device.

Delete the directory and the `[patch]` block once upstream ships a release that
compiles on macOS.

## 2. What a re-enable needs

Ordered by what blocks what. Everything here is **unverified** — no macOS host, no
Apple SDK, and no `aarch64-apple-darwin` Rust target were available when this was
written.

### 2.1 Build

- `bite-gp-apple/build.rs` shells out to `xcrun -sdk macosx metal` and
  `xcrun … metallib`, so the runner needs the Xcode command-line tools.
- `build.sh` and the `ci.yml` headless job copy `libferrocad_py.so`; on macOS the
  artifact is `target/debug/libferrocad_py.dylib` (copied to
  `python/ferrocad.abi3.so` — Python accepts a Mach-O dylib under that name).
- `.cargo/config.toml` uses `$ORIGIN` in `[build] rustflags` and sets
  `LD_LIBRARY_PATH` in `[env]`. dyld expands neither: use `@loader_path` /
  `@executable_path` and `DYLD_LIBRARY_PATH`. Cargo `[env]` values also lose to a
  real environment variable, so a Homebrew OCCT must be exported explicitly.
- The conda OCCT prefix has the same `lib/` + `include/opencascade` shape on
  `osx-arm64`, and `ferrocad_occt/build.rs` already searches `/opt/homebrew` and
  `/opt/local`, so no path rewrite is expected (unconfirmed).

### 2.2 The OCCT closure and signing (the hard part)

`crates/xtask/src/closure.rs` implements ELF (`readelf -d`/`ldd`) and PE walking
and **returns an error** on macOS. A `stage_macho` needs:

1. seed from `otool -L` / `otool -D`, recurse into every copied toolkit;
2. copy prefix-owned dylibs under the loader's exact basename (mind the
   `libTKernel.7.8.dylib` → `libTKernel.7.8.1.dylib` symlink convention);
3. `install_name_tool -id @rpath/<name>` on each copied dylib and
   `install_name_tool -change <old> @rpath/<name>` on each dependent file *and* the
   binary;
4. add `@executable_path/../Resources/lib` (or `Contents/Frameworks`) to the
   binary and `@loader_path` to the dylibs;
5. `codesign --force --sign -` every modified Mach-O, then the `.app`;
6. verify no `/opt/conda`, `.occt` or Homebrew path remains in any load command.

Whether conda's `osx-arm64` dylibs use absolute install names, `@rpath` or bare
basenames decides how much of step 3 is needed — that is the first thing to look
at on a Mac (`otool -D` on one `libTK*`).

`--check-init` ordering matters: `xtask bundle --check-init` runs before
`packaging/macos/app.sh` does its `libpython` fixup, and `DYLD_LIBRARY_PATH` cannot
override an absolute `libpython` load command. On macOS the check should run after
the `.app` is assembled (or the fixups should move into `xtask`).

### 2.3 Packaging

`packaging/macos/app.sh` predates `lib/`, `MANIFEST` and the edition selector, so
it needs:

- an `--edition parametric` argument and the matching payload/artifact names
  (compare `packaging/linux/appimage.sh` and `packaging/windows/portable.ps1`);
- copying `lib/` and `MANIFEST` (it copies only `python`, `mods`, `LICENSES`,
  `runtime`);
- signing the bundle and every dylib, not just `Contents/MacOS/ferrocad-bin`;
  `codesign --verify --deep --strict` must pass. "Ad-hoc signed" in the current
  docs overstates what the script does;
- the launcher (`packaging/macos/ferrocad`) adding `lib/` to
  `DYLD_LIBRARY_PATH` as a fallback, though rpath fixups are the real answer;
- an `Info.plist` version from `Cargo.toml` instead of the hardcoded `0.1.0`, an
  `.icns` (`packaging/icon/ferrocad.svg` is the only icon source), and an artifact
  name that carries the architecture and edition.

`packaging/release-notes.sh` extracts a `.dmg` with `7z`, but `release.yml`'s
release job installs only `unzip` — add `p7zip-full` when a `.dmg` returns.

### 2.4 Workflow and CI

- Re-add a `build` matrix leg on `macos-14` (arm64; `xtask`'s
  `python-build-standalone` triple is host-derived, so one runner is one
  architecture). No `crt-static` (Windows only); set
  `MACOSX_DEPLOYMENT_TARGET` to match `Info.plist`.
- The `linux-desktop-deps` action and the OCCT micromamba step are already
  `if: runner.os == 'Linux'`; a macOS OCCT fetch would need `osx-arm64`.
- Add a macOS leg to `ci.yml` (extension build + `cargo test -p ferrocad_gpui`
  and friends) and a macOS `geometry` leg so `opencascade-sys` is exercised.
- The edition job mirrors `build` with `packaging/macos/app.sh --edition
  parametric --occt`; the release job already globs everything under `dist/`.

## 3. Unknown without hardware

- Whether `bite-gp-apple` builds after the import removal — or whether further
  Apple-only errors follow.
- Whether `xcrun metal`/`metallib` succeed with the runner's Xcode.
- Whether the 64 `#[gpui::test]` tests in `ferrocad_gpui` run headless on a
  GitHub macOS VM (Metal device / WindowServer availability).
- Whether the Metal backend renders a real window (titlebar, client decorations).
- Whether conda-forge publishes `occt=7.8.1` for `osx-arm64`, and what its dylib
  install names look like.
- Whether the pinned `python-build-standalone` `aarch64-apple-darwin` asset
  exists and links through PyO3's config.
- Whether `--check-init` passes with `DYLD_LIBRARY_PATH` cleared, whether
  `hdiutil` → UDZO → Linux `7z` round-trips, and what Gatekeeper does with an
  ad-hoc bundle (users will need `xattr -dr com.apple.quarantine`).
