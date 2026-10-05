# Releasing FerroCAD (working note)

Status: reference (2026-10-05). Companion to
[`python-bindings.md`](python-bindings.md), [`milestones.md`](milestones.md),
[`architecture.md`](architecture.md) and [`repackaging.md`](repackaging.md). It records
how the workspace reaches its consumers: the Rust crates on crates.io and the
`FreeCAD`-namespace Python distribution on PyPI.

**Status (2026-10-06).** `ferrocad_core` `0.1.0`, `ferrocad_py` `0.1.0`,
`ferrocad_widgets` `0.1.0`, `ferrocad_gpui` `0.1.1` and `ferrocad` `0.1.1` are
on crates.io. Versions are **per crate**: a release bumps only the crates whose
sources changed, and the publish workflow skips the rest (their version is
already on crates.io). `[workspace.package] version` is the release/product
version the git tag is checked against. The same tag produces three
self-contained desktop artifacts on the GitHub Releases page (§5). The widget kit
was renamed from `ferrocad_ui` before release, because that name read as "the UI
application" next to `ferrocad_gpui`. `ferrocad_gui` is reserved for the future
Gui bindings; the per-edition binaries are not published as crates (see
[`repackaging.md`](repackaging.md)).

## 1. What ships, and where

FerroCAD is one workspace that produces two kinds of deliverable.

| Crate | Artifact | Channel | Published? |
| --- | --- | --- | --- |
| `ferrocad` | lib + bin | crates.io | yes (app binary; embeds the payload) |
| `ferrocad_core` | `rlib` | crates.io | yes |
| `ferrocad_py` | `rlib` + `cdylib` (module `ferrocad`) | crates.io and PyPI (maturin) | yes (both) |
| `ferrocad_widgets` | `rlib` | crates.io | yes |
| `ferrocad_gpui` | `rlib` | crates.io | yes |
| `ferrocad_gen` | `cdylib` (generated skeleton) | PyPI, via the wheel | no |
| `xtask` | binary | build tool | no |

The Python distribution is named **`ferrocad`**, but its import namespace is
**`FreeCAD`** (see `python/FreeCAD`). `ferrocad` is also the name of the private
PyO3 backend module built from `crates/ferrocad_py`; a user only ever writes
`import FreeCAD`.

`ferrocad_gen` is a `cdylib`: it is loaded as an extension module, never linked as
a Rust library, so it has no business on crates.io. Five crates are published:
`ferrocad_core`, `ferrocad_py` (also the wheel's backend module), `ferrocad_widgets`,
`ferrocad_gpui`, and the `ferrocad` app crate (`cargo install ferrocad` builds the
binary; see §2.4).

## 2. crates.io: the library crates

`ferrocad_core` is the pure-Rust engine; `ferrocad_widgets` re-exports the reusable
`bite-gpui` widgets; `ferrocad_gpui` is the application shell library built on them
(it links libpython, because it embeds CPython). All are ordinary `rlib`s with no
path-only dependencies.

### 2.1 Publishing order

Dependencies are published before their dependents:

1. `ferrocad_core` (depends only on crates.io crates).
2. `ferrocad_py` (depends on `ferrocad_core`; `rlib` + `cdylib`).
3. `ferrocad_widgets` (depends on `bite-gpui`; independent of the core crates).
4. `ferrocad_gpui` (depends on `ferrocad_widgets`).
5. `ferrocad` (the app binary; depends on `ferrocad_gpui` and `ferrocad_py`).

The workspace pins internal dependencies with **both** a `path` (for local
development) and a `version` (which the registry requires):

```toml
# [workspace.dependencies]
ferrocad_core    = { path = "crates/ferrocad_core",    version = "0.1.0" }
ferrocad_py      = { path = "crates/ferrocad_py",      version = "0.1.0" }
ferrocad_widgets = { path = "crates/ferrocad_widgets", version = "0.1.0" }
ferrocad_gpui    = { path = "crates/ferrocad_gpui",    version = "0.1.1" }
```

Each dependency's `version` requirement must be satisfied by the version that
crate will publish at, and each crate's own `[package] version` is explicit (or
inherited from `[workspace.package]`) accordingly. A release that changes only
`ferrocad_gpui` and `ferrocad` (as this one does) leaves the other three pinned
at their published `0.1.0`, so the workflow republishes only the two.

Crates that consume them write `ferrocad_core.workspace = true`. A `path`
dependency **without** a `version` makes `cargo publish` refuse the crate, which
is why the version is not optional.

### 2.2 Metadata the registry wants

`cargo publish` requires, per crate:

- a `description` (present),
- a `license` or `license-file` (both here use the SPDX expression
  `LGPL-2.1-or-later`; the full text is in `../LICENSE`),
- a `version` on every path dependency (see above).

`README.md`, `keywords`, `categories`, `documentation`, `repository` and
`homepage` are not hard errors but are what makes the crate page useful. Each
crate inherits `readme`, `repository`, `homepage` and `rust-version` from
`[workspace.package]` via `.workspace = true`, so the root `README.md` is
packaged into both. (`license` and `license-file` are mutually exclusive, so we
declare the SPDX id and keep the text as a repo file.)

### 2.3 Dry run, then publish

Always package-verify first. `cargo package` builds the crate *from the
tarball*, which is the only way to catch a dependency that resolves locally but
not on the registry:

```sh
. ../.toolchain/env.sh
cargo package -p ferrocad_core --allow-dirty   # builds the packaged crate
cargo package -p ferrocad_widgets   --allow-dirty

cargo publish -p ferrocad_core --dry-run
cargo publish -p ferrocad_widgets   --dry-run
```

`--allow-dirty` is only for testing before a commit; a real publish requires a
clean tree. Once the dry runs pass, publish in order:

```sh
cargo publish -p ferrocad_core
cargo publish -p ferrocad_widgets
```

New versions require a bump in `[workspace.package] version`, which every crate
inherits.

### 2.4 `cargo install ferrocad`

`cargo install ferrocad` builds a **self-contained** base app binary. The Python
facade and workbenches live outside any crate (in the workspace `python/` and
`mods/` trees), but crates.io requires a self-contained tarball and `cargo install`
has no data-file step. So the app crate carries the payload:

- the `ferrocad` tarball includes `python/` and, when staged, `mods/` (an explicit
  `include` allowlist in `crates/ferrocad/Cargo.toml`);
- `build.rs` and `include_dir!` bake them into the binary;
- on first run the app unpacks them to the per-user data directory and boots the
  interpreter from there.

Because the directories are not crate sources, they are staged only for the
publish:

```sh
cargo xtask stage-payload        # also: --python-only to keep the crate small
cargo publish -p ferrocad
cargo xtask unstage-payload
```

The app still prefers a payload it finds on disk (an installer's launcher, a
`python/` beside the binary, or a development checkout), so the embedded copy is
a fallback, never a stale override. The resolution order and the exact layout are
in [`distribution.md`](distribution.md) §8.

A binary installed from a `--python-only` publish behaves as before: it looks
next to the executable or via `FERROCAD_PYTHON_PATH`. That remains the development
path when the crate was published without workbenches.

### CI publishing

`.github/workflows/crates.yml` publishes on a `v*` tag. It verifies the tag
against `[workspace.package] version`, runs the tests, fetches the workbench
scripts (`cargo xtask mods`), and publishes `ferrocad_core`, `ferrocad_py`,
`ferrocad_widgets`, `ferrocad_gpui` and `ferrocad` in dependency order, skipping
any version already on crates.io (re-running a tag is safe). It stages the payload
(`cargo xtask stage-payload`) just before publishing `ferrocad` and unstages it in
an `EXIT` trap, so a failed job never leaves the copies behind.

It needs a repository secret named **`CARGO_REGISTRY_TOKEN`** (a crates.io API
token with publish rights). The other workspace crates set `publish = false`: they
ship in the wheel (`ferrocad_gen`) or the installer (`xtask` is a build tool).

## 3. PyPI: the `FreeCAD` distribution

The wheel is built with [maturin](https://www.maturin.rs/) from
`pyproject.toml`:

- `manifest-path = "crates/ferrocad_py/Cargo.toml"` compiles the PyO3 crate;
- `module-name = "ferrocad"` makes it importable as the backend module;
- `python-source = "python"` ships the pure-Python `FreeCAD`/`FreeCADGui`
  facade;
- `features = ["extension-module"]` tells PyO3 **not** to link libpython, which
  is correct for an extension module loaded into an existing interpreter.

`build.sh` is the offline, no-maturin equivalent used by the sandbox: it builds
the crates and copies the `.so`s next to `python/FreeCAD`.

### 3.1 The `abi3` and forward-compatibility notes

The extension is built with PyO3 `abi3-py310`, so one wheel covers CPython 3.10
and later. The sandbox runs CPython 3.14, which is newer than PyO3 0.25
officially targets, so builds set:

```sh
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1
```

This is a build-time escape hatch, not something a downstream user needs; the
resulting `abi3` wheel is ordinary.

### 3.2 `extension-module` is opt-in

`extension-module` is deliberately **not** a hard dependency feature of
`ferrocad_py`/`ferrocad_gen`. Enabling it globally would stop the workspace from
linking libpython, and `ferrocad_gpui` embeds CPython and needs those symbols.
Maturin and `build.sh` enable it only for the extension build.

## 4. The desktop application: what was chosen

How the desktop app is delivered is settled. It ships as three self-contained
artifacts on the GitHub Releases page, each bundling the pinned
`python-build-standalone` runtime:

| Platform | Artifact | Built by |
| --- | --- | --- |
| Linux | `ferrocad-x86_64.AppImage` | `packaging/linux/appimage.sh` (`appimagetool`) |
| macOS | `FerroCAD.dmg` (arm64) | `packaging/macos/app.sh` (`hdiutil`) |
| Windows | `ferrocad-windows-x86_64.zip` | `packaging/windows/portable.ps1` |

The trade is settled in favour of **bundling CPython**: a larger artifact
(~70-80 MB) that depends on nothing on the user's machine. Users who want extra
Python packages can still drop them under the app's `mods/` and `python/`.

`cargo install ferrocad` is the second channel: a self-contained binary whose
payload is embedded and unpacked on first run, because `cargo install` has no
data-file step. The two channels ship the same payload differently (loose files
vs. embedded); see [`distribution.md`](distribution.md) §6 and §8.

## 5. Cutting a release

`v0.1.1` exercised the whole path. The order:

1. Bump `[workspace.package] version` (the release version the tag is checked
   against), and bump only the crates whose sources changed; the rest keep their
   published version so the workflow skips them.
2. Push the tag `vX.Y.Z`. Three workflows run:
   - `crates.yml` publishes the changed crates in dependency order, staging the
     Python payload into the app crate for the `ferrocad` publish.
   - `release.yml` fetches the runtime, runs `packaging/*` to build the three
     artifacts, and attaches them to a GitHub Release.
   - `ci.yml` runs the headless and window tests.
3. The release body is generated by `packaging/release-notes.sh`, which measures
   the actual artifacts and fills the `<!--ARTIFACTS-->` marker in
   `packaging/release-notes.md`.

Outcome for `v0.1.1`: `ferrocad_core`/`ferrocad_py`/`ferrocad_widgets` `0.1.0` and
`ferrocad_gpui`/`ferrocad` `0.1.1` on crates.io, plus three artifacts
(`ferrocad-x86_64.AppImage`, `FerroCAD.dmg`, `ferrocad-windows-x86_64.zip`) on the
GitHub Release. The macOS artifact is arm64-only and ad-hoc signed; Windows is an
unsigned zip.
