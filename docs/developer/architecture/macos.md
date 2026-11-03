# macOS

Status: **not shipped** (2026-11-03). The Linux and Windows artifacts ship. The
Apple target compiles against `bite-gpui` 1.25.203; what remains is the Mach-O
closure, signing and packaging path, which is unverified. This page is the
checklist for turning the platform on.

## 1. What still blocks a release

The compile blocker is gone. `bite-gp-apple` used to import `cocoa` without
declaring it, which broke `cfg(any(target_os = "macos", target_os = "ios"))`;
1.25.203 removed those imports (along with the duplicate font imports and the
two feature-gated `gpui_macos`/`metal_renderer` errors found at the same time),
and the upstream macOS CI jobs compile the family.

What is still missing is the packaging path:

- the Mach-O closure (`otool -L` / `otool -D`, `install_name_tool`, signing)
  that `xtask` implements for ELF and PE but not Mach-O;
- the `.app`/`.dmg` assembly, signing and verification;
- a macOS CI leg and the release matrix leg.

The family stays on the same exact 1.25.203 release: its members declare caret
requirements, so a plain `cargo update` floats some of them while `bite-gpui`
stays put and splits the wgpu device between the window renderer and
`ferrocad_wgpu`.

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
   `libTKernel.8.0.dylib` → `libTKernel.8.0.1.dylib` symlink convention);
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
- Add a macOS leg to `ci.yml` (extension build + `cargo test -p ferrocad_shell_gpui`
  and friends) and a macOS `geometry` leg so `opencascade-sys` is exercised.
- The edition job mirrors `build` with `packaging/macos/app.sh --edition
  parametric --occt`; the release job already globs everything under `dist/`.

## 3. Unknown without hardware

- Whether the Apple target really builds and runs here: the upstream macOS CI
  jobs compile it, but no macOS host or SDK has been available in this
  workspace.
- Whether `xcrun metal`/`metallib` succeed with the runner's Xcode.
- Whether the 64 `#[gpui::test]` tests in `ferrocad_shell_gpui` run headless on a
  GitHub macOS VM (Metal device / WindowServer availability).
- Whether the Metal backend renders a real window (titlebar, client decorations).
- Whether conda-forge publishes `occt=8.0.1` for `osx-arm64`, and what its dylib
  install names look like.
- Whether the pinned `python-build-standalone` `aarch64-apple-darwin` asset
  exists and links through PyO3's config.
- Whether `--check-init` passes with `DYLD_LIBRARY_PATH` cleared, whether
  `hdiutil` → UDZO → Linux `7z` round-trips, and what Gatekeeper does with an
  ad-hoc bundle (users will need `xattr -dr com.apple.quarantine`).
