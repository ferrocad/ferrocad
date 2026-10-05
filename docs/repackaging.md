# Repackaging strategy: one shell library, many FerroCAD editions

Status: reference (2026-10-05). Companion to [`architecture.md`](architecture.md),
[`app-shell-vision.md`](app-shell-vision.md) and [`releasing.md`](releasing.md).

## 1. The goal

Ship one shared application core and a family of **editions** built on it:

- FerroCAD (the general edition)
- FerroCAD: Architecture
- FerroCAD: Interior design
- FerroCAD: Furniture
- FerroCAD: 3D printing
- FerroCAD: Metalworks
- FerroCAD: Assembly
- and later ones

An edition is a curated bundle: a workbench set, defaults, templates, units and
branding. It is a **repackaging**, not a fork. Everything expensive (the engine,
the widget kit, the interpreter embedding, the inspector/property editor/console)
is written once and shared.

## 2. Why the base app is `ferrocad` and the shell library is `ferrocad_gpui`

We rejected `ferrocad_desktop` and `ferrocad_app`:

1. `ferrocad_desktop` bakes a **platform** into the name and closes the door on a
   future web or mobile target. Platform is a build target, not a crate identity.
2. `ferrocad_app` is backwards: in FreeCAD, `App` is the headless core, which is
   what `ferrocad_core` already is (see the App/Gui note in §5).

So the two roles are split by name:

- **`ferrocad`** is the base application: a vanilla binary that calls the shell
  library with the default configuration (`cargo run -p ferrocad`).
- **`ferrocad_gpui`** is the reusable **shell library**: it boots CPython, wires
  the window and panels, and exposes `run`/`run_with` plus `HostConfig`. The name
  says both what it is (a `bite-gpui` app shell) and that it is a layer an app is
  built *on*, not the app itself. It does not promise a platform.

## 3. Shape: a shell library plus thin edition binaries

The shape is a shell **library** plus thin binaries:

```
   ferrocad_core        ferrocad_widgets
        \                   /
         \                 /
          v               v
        +-------------------------------+
        |        ferrocad_gpui          |  library: boot CPython, Shell,
        |  (boot, Shell, HostConfig,    |  commands, HostConfig
        |   command registry)           |
        +-------------------------------+
          ^      ^        ^        ^
          |      |        |        |
   ferro-arch  ferro-furn  ferro-print  ...   thin bins, one per edition
   (HostConfig: workbenches, template, name, units, extras)
```

The shape, and where it stands:

1. **Done.** `ferrocad_gpui` is a library (`src/lib.rs`) exposing `Shell`, the
   `python` bridge, `run()` / `run_with(config)` and a `HostConfig`. `HostConfig`
   carries the application name, the Python entry module and the app's
   `python_paths`; the workbench list and startup template come later.
2. **Done (shape).** The `ferrocad` crate is the base app binary: it depends on the
   host library and calls `run()`. An edition is the same shape with a different
   `HostConfig`.
3. **Done.** `cargo run -p ferrocad` runs the general edition. `ferrocad_gpui` is
   library-only; `ferrocad` is the only application binary.

The boundary between the two crates: `ferrocad_gpui` owns the **interpreter**
(boot, `sys.path`, JSON) and ships no application script; the **app** owns the
scripts. The app's `main` sets `HostConfig.entry_module` and
`HostConfig.python_paths`; the base app names `ferrocad_shell`, and a
redistribution names its own startup module and its `Mod/` directories. See
[`architecture.md`](architecture.md) §8.

Bundling a Python runtime and packaging AppImage/`.app`/`.exe` is a separate
workstream in [`distribution.md`](distribution.md).

This mirrors how FreeCAD separates the `App` core, `Gui`, and the workbench
`Mod/` trees, but with editions layered *above* the shell instead of patched into
it.

## 4. What an edition varies, and what it shares

| Shared (never forked) | Varies per edition |
| --- | --- |
| `ferrocad_core` engine | workbench set |
| `ferrocad_widgets` kit | startup script/module and sample document |
| the `FreeCAD` facade and bindings | default units and preferences |
| shell: interpreter boot, undo/redo, inspector, property editor, console | application name, icon, about text |
| commands and their transactions | bundled third-party Python addons |
| the file format | file associations and import/export defaults |

## 5. Naming

- **Base app binary:** `ferrocad` (the general edition; `cargo run -p ferrocad`).
- **Shell library:** `ferrocad_gpui`, the `bite-gpui` application shell that every
  app and edition is built on.
- **Widget kit:** `ferrocad_widgets` (renamed from `ferrocad_ui`, which read as
  "the UI application" next to the shell).
- **Editions:** crate names `ferrocad-<edition>` (for example
  `ferrocad-architecture`), user-facing names `FerroCAD: <Edition>`. Editions are
  application binaries, so they are **not** published to crates.io.
- **The App/Gui split:** `ferrocad_core` is our App, and a future `FreeCADGui`
  extension is our Gui, so reserve `ferrocad_gui` for those Gui bindings. Because
  `ferrocad_gpui` and `ferrocad_gui` differ by one letter, prefer `ferrocad_py_gui`
  for the bindings crate if the two ever sit side by side, so the toolkit layer
  (`gpui`) and the Gui bindings (`gui`) cannot be confused.
- **Rejected:** `ferrocad_desktop` (platform in the name), `ferrocad_app`
  (backwards), `ferro-base` (collides with `FreeCAD.Base`).

## 6. Publishing

`ferrocad_core`, `ferrocad_widgets`, `ferrocad_gpui` and the `ferrocad` app crate
go to crates.io; the editions are distributed as installers/AppImages/wheels, not
as crates. `cargo install ferrocad` builds the app binary, but it still needs the
Python payload, shipped as loose files by the installer (the FreeCAD model); it is
not embedded in the binary. See [`releasing.md`](releasing.md) for the publish
order and [`distribution.md`](distribution.md) for the artifacts.

## 7. Open questions

- Where do editions live: extra members of this workspace, or a separate
  `ferrocad-distributions` repository that depends on the published crates? A
  separate repo keeps the core small and versioned, at the cost of syncing.
- Whether `HostConfig` is pure Rust, a resource file, or a Python module read at
  boot. A Python module fits the Blender-style model best; a Rust struct is
  simpler and type-checked.
- Whether an edition may add Rust-side panels, or only Python workbenches and
  configuration. Allowing Rust panels means editions compile the shell, not just
  depend on it.
