# Repackaging strategy: one shell library, many FerroCAD editions

Status: reference (2026-10-05). Companion to [`architecture.md`](../architecture.md),
[`app-shell-vision.md`](../../project/direction/app-shell-vision.md) and [`releasing.md`](releasing.md).

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

## 2. Why the base app is `ferrocad`, the shell is `ferrocad_shell_core`, and the native assembly is `ferrocad_shell_gpui`

We rejected `ferrocad_desktop` and `ferrocad_app`:

1. `ferrocad_desktop` bakes a **platform** into the name and closes the door on a
   future web or mobile target. Platform is a build target, not a crate identity.
2. `ferrocad_app` is backwards: in FreeCAD, `App` is the headless core, which is
   what `ferrocad_core` already is (see the App/Gui note in §5).

So the two roles are split by name:

- **`ferrocad`** is the base application: a vanilla binary that calls the shell
  library with the default configuration (`cargo run -p ferrocad`).
- **`ferrocad_shell_core`** is the reusable, platform-neutral **shell library**: the
  window, chrome, panels, viewport element and view models. The name says what it
  is (a `bite-gpui` app shell) and that it is a layer an app is built *on*, not the
  app itself. It promises no platform: the browser host compiles it directly.
- **`ferrocad_shell_gpui`** is the **native assembly**: `ferrocad_shell_core` plus
  the embedded-CPython `AppBackend`, plus `run`/`run_with` and `HostConfig`. An
  edition depends on this crate.

**Landed (2026-11-03).** The split is described in
[`wasm-shell.md`](../../project/tasks/wasm-shell.md) and is now structural:
`ferrocad_shell_api` owns the `AppBackend` trait and every view model it returns,
`ferrocad_shell_core` owns the GPUI UI and the `cx`-threaded helpers, and
`ferrocad_shell_gpui` (renamed from `ferrocad_gpui`) owns CPython. The browser host
`ferrocad_web_gpui` depends on the core only. `ferrocad_gpui` stops at 0.1.5; see
[`releasing.md`](releasing.md) for the package discontinuity. The browser host is
already a real shell (M2): the same `ferrocad_shell_core` window runs in Chrome on
WebGL2 with a Rust `AppBackend`, one in-window inspector column instead of the
Dev Tools window, and a bundled font. It also has the live viewport (M3) and a real
`ferrocad_core::Document` on the wasm OCCT kernel (M4).

## 3. Shape: a shell library plus thin edition binaries

The shape is a shell **library** plus thin binaries:

```
   ferrocad_core        ferrocad_widgets
        \                   /
         \                 /
          v               v
        +-------------------------------+
        |     ferrocad_shell_core       |  library: Shell, chrome, panels,
        |  (window, panels, viewport,   |  AppBackend helpers, view models
        |   AppBackend helpers)         |
        +-------------------------------+
          ^             ^
     ferrocad_shell_gpui   ferrocad_web_gpui     the two hosts
     (CPython backend)     (Rust backend)
          ^
   ferro-arch  ferro-furn  ferro-print  ...   thin bins, one per edition
   (HostConfig: workbenches, template, name, units, extras)
```

The shape, and where it stands:

1. **Done.** `ferrocad_shell_core` exposes `Shell` and the view models;
   `ferrocad_shell_gpui` adds the `python` bridge, `run()` / `run_with(config)` and
   a `HostConfig`. `HostConfig` carries the application name, the Python entry
   module and the app's `python_paths`; the workbench list and startup template
   come later.
2. **Done (shape).** The `ferrocad` crate is the base app binary: it depends on the
   host library and calls `run()`. An edition is the same shape with a different
   `HostConfig`.
3. **Done.** `cargo run -p ferrocad` runs the general edition. The shell is
   library-only; `ferrocad` is the base application binary.
4. **Started.** `ferrocad_parametric` is the first edition binary: it registers the
   OCCT-backed Part module and delegates to `ferrocad::run_edition`, which selects the
   edition's app script (`python/ferrocad_parametric`). That script imports `Part`,
   registers the module's display/dress-up hooks and seeds the demo solid. See §4a for
   why a Part-enabled app must be an edition, not the base crate.

### 4a. Editions carry the OCCT-backed workbenches

The base `ferrocad` app is **published to crates.io**, and `cargo install ferrocad`
should stay a clean build with no kernel: linking the Part workbench pulls in OCCT,
which a `cargo install` user may not have. The OCCT-backed crates *are* published
(`ferrocad_occt`, `ferrocad_part`, `ferrocad_part_py`), so a kernel is one optional
dependency away — but an edition is the right home for it: editions own the
workbench set and the window branding, and the base app stays minimal. So a
Part-enabled application is `ferrocad_parametric`, an edition.

The edition is thin: it registers the extra built-in module(s), names its app script
and sets its window title. The registration goes through one shared list
(`ferrocad_combined_py::register_builtins()`, which appends `ferrocad`, `Part`,
`Mesh` and `MeshPart`), so the edition and the host-Python extension cannot drift.
See [`kernel-state-sharing.md`](kernel-state-sharing.md).

```rust
// crates/ferrocad_parametric/src/main.rs
fn main() {
    // One process, one core: the four modules are appended to the built-in table
    // before the interpreter initialises.
    ferrocad_combined_py::register_builtins();

    let contributions = ferrocad::base_contributions()
        .with_manipulator(ferrocad_part_gui::fillet_manipulator)
        .with_manipulator(ferrocad_part_gui::push_pull_manipulator);
    ferrocad::run_edition_with_document(
        "FerroCAD: Parametric",
        "ferrocad_parametric",
        contributions,
        None,
    );
}
```

The app script is ordinary Python, re-exporting the base shell's entry points and
telling the shell how the edition's objects are displayed and how a manipulator's
commit is applied (the module hooks in [`architecture.md`](../architecture.md) §9):

```python
# python/ferrocad_parametric/__init__.py
import FreeCAD
import FerroCAD
import Part
from ferrocad_shell import *          # the shell's data functions

FerroCAD.register_display_provider(_display_mesh)
FerroCAD.register_interaction_provider(_interaction)
```

Linking the module into the **same image** is required: both `import Part` and
`import FreeCAD` must see one `ferrocad_core` (see [`occt-integration.md`](occt-integration.md)
§4). Because the edition links OCCT, shipping it (AppImage/`.dmg`/zip) also means
staging the OCCT libraries beside the binary — [`occt-bundling.md`](occt-bundling.md).
That is why the packaged artifacts still build the base `ferrocad` binary for now;
the edition is run from a source checkout (`cargo run -p ferrocad_parametric`).
[`edition-publishing.md`](edition-publishing.md) plans the step that ships it.

`ferrocad_toy` is the **minimal** edition, and the reference for a new workbench: it
links no kernel, its app script creates a typed object and generates its mesh from the
object's properties, and it contributes a manipulator **and** a gizmo through the same
`Contributions` registry. Run it with `cargo run -p ferrocad_toy`; see
[`toy-workbench.md`](../workbenches/toy-workbench.md).

The boundary between the two halves: `ferrocad_shell_gpui` owns the **interpreter**
(boot, `sys.path`, JSON) and ships no application script; the **app** owns the
scripts. The app's `main` sets `HostConfig.entry_module` and
`HostConfig.python_paths` (`run_edition` for an edition, `run_as` for the base app);
the base app names `ferrocad_shell`, and a redistribution names its own startup module
and its `Mod/` directories. See [`architecture.md`](../architecture.md) §8–§9.
`ferrocad_shell_core` names neither: it reaches the engine only through the
`AppBackend` installed by the host.

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
- **Shell library:** `ferrocad_shell_core`, the platform-neutral `bite-gpui`
  application shell that every app and edition is built on.
- **Native assembly:** `ferrocad_shell_gpui` (the core plus the embedded-CPython
  `AppBackend` and the `run` entry points).
- **Shell/engine contract:** `ferrocad_shell_api` (the `AppBackend` trait and the
  view models; no gpui, no CPython).
- **Widget kit:** `ferrocad_widgets` (renamed from `ferrocad_ui`, which read as
  "the UI application" next to the shell).
- **Editions:** crate names `ferrocad-<edition>` (for example
  `ferrocad-architecture`), user-facing names `FerroCAD: <Edition>`. Editions are
  application binaries, so they are **not** published to crates.io.
- **The App/Gui split:** `ferrocad_core` is our App, and a future `FreeCADGui`
  extension is our Gui, so reserve `ferrocad_gui` for those Gui bindings. Because
  `ferrocad_shell_gpui` and `ferrocad_gui` differ by two letters, prefer
  `ferrocad_py_gui` for the bindings crate if the two ever sit side by side, so the
  toolkit layer (`gpui`) and the Gui bindings (`gui`) cannot be confused.
- **Rejected:** `ferrocad_desktop` (platform in the name), `ferrocad_app`
  (backwards), `ferro-base` (collides with `FreeCAD.Base`).

## 6. Publishing

`ferrocad_shell_api`, `ferrocad_shell_core`, `ferrocad_shell_gpui` and the
`ferrocad` app crate go to crates.io; the editions are distributed as
installers/AppImages/wheels, not as crates. `cargo install ferrocad` builds the
app binary, but it still needs the Python payload, shipped as loose files by the
installer (the FreeCAD model); it is not embedded in the binary. See [`releasing.md`](releasing.md) for the publish
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
