# The wasm shell: one GPUI UI crate, two backends

Status: **task (2026-11-03). Plan only — nothing in this note is built.**
Companions: [`../research/web-wasm-host.md`](../research/web-wasm-host.md) (the
probe and the wasm findings), [`../../developer/architecture/repackaging.md`](../../developer/architecture/repackaging.md)
(the shell-library/edition shape), [`../record/tasks/dev-tools-window.md`](../record/tasks/dev-tools-window.md)
(why the inspector is a second window today) and
[`../record/tasks/viewport-event-routing.md`](../record/tasks/viewport-event-routing.md)
(occlusion and the `gpui_interop` fallback).

## 0. Decision

The Python dependency is removed from the browser target by a **physical crate
boundary**, not by a feature flag. Three crates:

| Crate | Role | Python | wasm |
| --- | --- | --- | --- |
| **`ferrocad_shell_core`** | The GPUI shell: `Shell`, chrome, panels, the viewport element, the shell view models, and the `AppBackend` trait. | **No** | Yes |
| **`ferrocad_shell_gpui`** | The native shell assembly: depends on `ferrocad_shell_core` (the native build is core + Python), owns `pyo3`, `ferrocad_inventor_py`, `ferrocad_stage_py`, the Python `AppBackend`, and the `run`/`run_with`/`HostConfig` entry points. **Renamed from `ferrocad_gpui`.** | Yes | No |
| **`ferrocad_web_gpui`** | The browser host: depends **only** on `ferrocad_shell_core`, supplies the Rust `AppBackend` over `ferrocad_core`/`ferrocad_part`/`ferrocad_occt`, and keeps the existing `wasm-bindgen` entry point. | No | Yes |

The seam is named **`AppBackend`**, in `ferrocad_shell_core::app`. It is not
`HostDocument`: it is a *view-model/provider seam* for a live workspace, not the
document API. `ferrocad_core::Document` remains the engine; the browser's
`AppBackend` reads it and returns the rows, trees and meshes the UI paints.

Naming rationale:

- `ferrocad_shell_core` names the part that is actually platform-neutral and
  therefore shared by both hosts.
- `ferrocad_shell_gpui` keeps the GPUI identity of the existing shell library and
  says "shell", but it is the **native assembly** now; the Python backend is an
  implementation detail, so `ferrocad_shell_python` would understate it.
  `ferrocad_gpui_app` was considered and rejected: "app" collides with the real
  edition binaries (`ferrocad`, `ferrocad_parametric`) and reads as a binary, not
  a shell library.
- `ferrocad_web_gpui` keeps its name: it already is the browser host, it is
  `publish = false`, and it carries the verified Chrome/WebGL2 baseline.
- `ferrocad_gpui` is retired rather than kept as a façade. The three in-tree
  consumers (`ferrocad`, `ferrocad_parametric`, and the docs) are updated in the
  same change; a façade would preserve a name that no longer describes the
  boundary. The crates.io package stops at 0.1.5 (record it in
  [`releasing.md`](../../developer/architecture/releasing.md)).

## 1. Why

The probe already proves every hard part of the browser path (OCCT wasm, wgpu,
`GpuSurface`/`paint_texture`, 40 triangles, drag/wheel captures). What is missing
is the **shell**: `ferrocad_gpui` embeds CPython, so the window, chrome, panels and
viewport element cannot be compiled for `wasm32-unknown-unknown`.

`ferrocad_gpui` calls Python synchronously from the UI thread in 66 production
lines across 14 files (`runtime/python.rs` is 40 functions + 18 types). A wasm
build cannot link any of it. Two shapes were considered:

- **(a) a `python` feature on one crate.** Smallest diff, but pyo3 stays one
  `#[cfg]` away from the wasm build. Rejected as the long-term shape.
- **(b) the crate boundary above.** The wasm app cannot name a Python crate, so
  an accidental import is a link error, not a feature-matrix accident. Chosen.

The split also pays for itself natively: the UI stops knowing about `pyo3`, and
the browser host stops being a second, divergent implementation.

## 2. The boundary

```
                 ferrocad_shell_gpui                 ferrocad_web_gpui
                 (native shell)                      (browser host)
                 pyo3 + Python AppBackend            Rust AppBackend
                        │                                   │
                        │  depends on                       │  depends only on
                        ▼                                   ▼
        ┌─────────────────────────────────────────────────────────────┐
        │                     ferrocad_shell_core                     │
        │  Shell · chrome · panels · InventorViewport · view models   │
        │  AppBackend (trait) · stage_view statics · fonts            │
        └──────────────────────────────┬──────────────────────────────┘
                                       │
             ferrocad_viewport · ferrocad_widgets · ferrocad_graphics
             ferrocad_inventor · ferrocad_stage · ferrocad_ui · bite-gpui
```

`ferrocad_shell_core` must not depend on `pyo3`, `ferrocad_inventor_py`,
`ferrocad_stage_py`, `ferrocad_part_py`, `ferrocad_py` or any other crate that
links CPython. It may depend on the pure stage/inventor/viewport crates.

The GPUI dependency stays in the core: this is a GPUI UI, not a headless model.
The core is the **platform-neutral** half, not a non-GPUI half.

## 3. The seam: `AppBackend`

### 3.1 What it is, and what it is not

It is the adapter between the GPUI UI and whichever engine is underneath. Its
methods return **UI-ready view models**, not raw document objects:

| UI need | Method result |
| --- | --- |
| Model tree panel | `Vec<DocumentNode>` — labels, names, nested object lists |
| Properties inspector | `Vec<PropRow>` — name, category, editable text, tooltip |
| 3D stage | `Vec<StageMesh>` — pre-tessellated triangles, already in display form |
| Menus / toolbar | `Vec<CommandInfo>` — id, label, enabled and checked flags |
| View state | `set_view_state(ViewState)` — the shell's grid/axis/gizmo state |

It is **not** a `Document` API. `ferrocad_core::Document` has no notion of a
properties panel or a model tree row; the browser backend is the code that turns
a `Document` into those rows and meshes. The native backend does the same through
the Python bridge (and keeps the existing `Part`/Draft behaviour).

### 3.2 Definition

```rust
pub trait AppBackend {
    fn bootstrap(&mut self) -> Bootstrap;
    fn model_tree(&self) -> Vec<DocumentNode>;
    fn properties(&self, object: &str) -> Vec<PropRow>;
    fn stage_meshes(&self) -> Vec<StageMesh>;
    fn commands(&self) -> Vec<CommandInfo>;
    fn set_view_state(&mut self, state: &ViewState);
    fn set_selection(&mut self, items: &[SelectionItem]);

    // Editing and scripted hooks. The first milestone leaves the defaults in
    // place; a backend that cannot edit still renders a read-only viewer.
    fn set_property(&mut self, object: &str, property: &str, value: &str) -> SetResult {
        SetResult { ok: false, error: "not supported".into(), view: None }
    }
    fn run_command(&mut self, id: &str) -> CommandOutcome {
        CommandOutcome::default()
    }
    fn rotation_quaternion(&self, object: &str) -> Option<[f64; 4]> {
        None
    }
    fn take_selection_requests(&mut self) -> Vec<SelectionRequest> {
        Vec::new()
    }
}
```

**No `Send`.** GPUI is single-threaded, and the backend is touched from the UI
thread only. (`HostDocument: Send` in earlier sketches was wrong for GPUI.)

### 3.3 Installation and access

Store the backend as a GPUI `Global`, like `gpui_interop` stores its pressed
surface, not in a process `static`:

```rust
struct AppBackendSlot(RefCell<Box<dyn AppBackend>>);
impl Global for AppBackendSlot {}

pub fn install_backend(cx: &mut App, backend: Box<dyn AppBackend>);
pub fn with_backend<T>(cx: &App, f: impl FnOnce(&dyn AppBackend) -> T) -> Option<T>;
pub fn with_backend_mut<T>(cx: &mut App, f: impl FnOnce(&mut dyn AppBackend) -> T) -> Option<T>;
```

The host installs the backend before `run`, so boot (`bootstrap`, `model_tree`,
`commands`) sees it. `Shell` keeps no second copy.

**Threading `cx` is the first implementation chore.** Some call sites are free
functions (`commands::list()` in `workbench/commands.rs`, the panel helpers)
without an `App`. The compiler will list them; move each call to a method that
has `ctx`, or pass `&App` explicitly. Do not add a process global to avoid this.

### 3.4 The per-frame statics stay in the core

`ferrocad_stage_py::view` and `ferrocad_inventor_py` expose pure Rust statics the
shell calls every frame (`preview_generation`, `preview_items`, `publish_camera`,
`publish_stage`, `has_stage`, `overlays_dirty`, `overlay_scenes`). They are not
Python calls. Move those statics down to `ferrocad_stage` and
`ferrocad_inventor` (or a small `stage_view` module in the core) and re-export
them from the `*_py` crates. That removes the shell's 13 references to `*_py`
without a trait, and the browser gets the same queue.

`PythonEventCallbacks` is the other Python-typed shell value: the shell's event
listener becomes `Box<dyn EventListener>` (the trait already lives in a pure
crate), with the Python implementation constructed by `ferrocad_shell_gpui` and
the browser implementation by `ferrocad_web_gpui`.

## 4. Crate layout and moves

```
crates/
  ferrocad_shell_core/          # new; from ferrocad_gpui's UI half
    src/
      lib.rs
      app.rs                    # AppBackend, AppBackendSlot, with_backend
      shell/                    # moved: shell/{mod,render,context,...}.rs
      ui/                       # moved: chrome, dev_tools, forms, ...
      panels/                   # moved
      hud/                      # moved
      workbench/                # moved
      model/                    # moved (selection, display)
      runtime/
        mod.rs                  # cfg-selects backend-only modules
        interop.rs              # the zero-copy presenter (gpu-interop)
        fonts.rs                # embedded UI font (wasm) / system (native)
        render_view.rs          # DeferredView (wasm) / ShellView alias
        stage_view.rs           # pure statics re-export (see §3.4)
  ferrocad_shell_gpui/          # renamed from ferrocad_gpui
    src/
      lib.rs                    # run/run_with/HostConfig, Python install
      python/                   # the current runtime/python.rs, split into
                                # {mod,types,embedded}.rs
      backend.rs                # PythonAppBackend: AppBackend
      events.rs                 # PythonEventCallbacks: EventListener
  ferrocad_web_gpui/            # existing probe → host
    src/
      lib.rs                    # wasm entry: install Rust backend, run core
      backend.rs                # WasmAppBackend: AppBackend
      document.rs               # ferrocad_core/Part/OCCT → view models
      commands.rs               # static CommandInfo / toggle outcomes
  ferrocad/                     # base binary: depend on ferrocad_shell_gpui
  ferrocad_parametric/          # edition: depend on ferrocad_shell_gpui
```

`HostConfig` belongs to the native assembly, but the browser host needs the same
window title/name knobs. Put the **struct** in the core with the Python/edition
fields behind the native assembly's extension (or keep two constructors); do not
put `python_paths`/`entry_module` in the core.

## 5. What the browser host supplies

A read-only viewer is enough for M2–M3:

- `bootstrap`: `version = env!("CARGO_PKG_VERSION")`, `backend = "occt-wasm"`,
  `document` from the Rust document (or `"Untitled"`).
- `model_tree`, `properties`, `stage_meshes`: from `ferrocad_core::Document` +
  `ferrocad_part` + `OcctBackend`, tessellated by `ferrocad_mesh`/the stage path.
- `commands`: a static list carrying at least the header ids (`App_ToggleGrid`,
  `App_ToggleAxisCross`, `App_ViewWorkplane`, `App_ToggleGizmo`, `Draft_Snap_Lock`)
  plus Fit/Ortho and the Dev Tools toggle, so the chrome is not empty.
- `set_view_state`: store the state; the grid/axis/gizmo producers read it.
- `set_selection`, `set_property`, `run_command`: M5.

## 6. Blockers to clear (from the research note)

| # | Blocker | Fix |
| --- | --- | --- |
| 1 | pyo3 | The crate boundary; `ferrocad_web_gpui` cannot name a Python crate. |
| 2 | `Instant::now()` panics on wasm | `web-time::Instant` in `ferrocad_viewport` (`inventor_view.rs`, `navicube.rs`), `ferrocad_widgets/wheel.rs`, optionally `ferrocad_wgpu/trace.rs`. |
| 3 | `std::env::args()` panics | `configure_trace` is native-only; the wasm entry does not call it. |
| 4 | `block_on` adapter/device creation hangs | Never create a renderer/adapter on wasm; the presenter supplies the device. |
| 5 | Shell creates its own `WgpuRenderer` before paint | `DeferredView` on wasm; the real renderer is created in `Presenter::prepare`. |
| 6 | Presenter cfg excludes wasm | Allow `cfg(all(feature = "gpu-interop", not(target_os = "macos")))`; its same-device branch is the probe's path. |
| 7 | Presenter format/MSAA | Use `surface.format()` and `SAMPLES = 1` on wasm. |
| 8 | One window only | Render tree/properties/console **in the main window** behind a `panels_open` flag; do not use `Floating`/`Dialog`. |
| 9 | Adapter enumeration | Empty on wasm (the Dev Tools GPU panel is read-only). |
| 10 | Fonts absent → `resolve_font` panic | Embed a UI font and call `add_fonts` before opening the window; set the root family. |
| 11 | WebGPU-first entry | `application_with_web_backend(WebBackendPreference::WebGl)` + `gpui::web_init()`. |
| 12 | `process::exit(1)` on boot error | Log and show an error overlay on wasm. |
| 13 | Readback image path cannot work | The presenter is mandatory; an `Unsupported` presenter shows a diagnostic, not the image path. |

The full evidence (file:line) is in [`../research/web-wasm-host.md`](../research/web-wasm-host.md).

## 7. Stages

| # | Stage | Work | Acceptance |
| --- | --- | --- | --- |
| **M0** | **Crate split, no behaviour change** | Create `ferrocad_shell_core`; move the UI; rename `ferrocad_gpui` to `ferrocad_shell_gpui`; move the Python backend behind `AppBackend`; update `ferrocad`/`ferrocad_parametric`/docs. | The current 308 `ferrocad_gpui` tests pass in their new home; the native app boots and `--check-init` is unchanged; `ferrocad_shell_core` has no pyo3 in `cargo tree`. |
| **M1** | **Compile for wasm (no window)** | `web-time`; cfg `interop`/`headless`; wasm app depends only on the core; CI check. | `cargo check -p ferrocad_web_gpui --target wasm32-unknown-unknown` in the cross image (the OCCT kernel is only needed once the document backend is wired; M1 can use the sample backend). |
| **M2** | **A shell window in Chrome** | Embedded font; wasm entry (`WebGl`, `web_init`, no `process::exit`); in-window panels; static `AppBackend`; placeholder viewport. | Browser shows the leading bar, status line, tree/properties/console, no panic; the console proves the backend was installed. |
| **M3** | **Live 3D in the shell** | `DeferredView`; presenter on wasm (format/MSAA); the sample scene through `InventorViewport`; orbit/zoom/pick. | The existing probe baseline still holds (40 triangles, `selected=Gl`, 22 027 / 26 127 / 0) with the shell around it. |
| **M4** | **Real Rust document** | `WasmAppBackend` over `ferrocad_core::Document` + `ferrocad_part` + `OcctBackend`; tree/properties/stage from the document. | A `Part::Box` + `Part::Fillet` document renders in the browser, the tree lists it, the properties show its rows. |
| **M5** | **Editing** | `set_property`, `run_command`, toggles, recompute, stage refresh; visibility. | Changing a property in the browser recomputes the document and the viewport updates. |

M0–M1 are the gate: they prove the boundary without touching the renderer. M2 is
the first user-visible demo. M3 is the regression-proof 3D step.

## 8. Non-goals

- **Python in the browser.** `pyo3`/CPython cannot run on `wasm32-unknown-unknown`;
  no app/workbench scripts, no `FreeCAD` facade, no Python tools, no `.ui`-driven
  task panels in the browser.
- **STEP/IGES/STL and `.FCStd` C++/Python serialization.** The prebuilt wasm
  kernel omits the exchange and CAF toolkits. Bytes-level native-format I/O is a
  separate task.
- **Multiple windows.** The browser backend has one window; the Dev Tools panels
  move in-window.
- **The readback image fallback** and **adapter enumeration** on wasm.
- **A feature flag as the boundary.** `python` may exist as a native convenience,
  but it is not the isolation mechanism.

## 9. Risks and open questions

- **`InventorViewport` + `Presenter` on wasm is unproven.** The probe draws a raw
  `gpu_canvas`; the viewport adds navicube text, overlays, tape and picking. M3 is
  the spike; keep the probe's assertions.
- **One-window panels.** The editor, properties and console must be laid out for
  the main window; the current `dev_tools_body` assumes a second window and
  double key handling (`panels_open` must not also route keys through the shell).
- **The `cx` threading.** `AppBackend` access needs `&App`/`&mut App` at every
  call site; if a free function genuinely has none, move the call rather than
  reintroducing a global.
- **The rename.** `ferrocad_gpui` 0.1.5 is published; `ferrocad_shell_gpui` is a
  new package. Record the discontinuity in `releasing.md` and update the edition
  manifests in the same change.
- **Test policy.** Native tests keep the Python backend (`ferrocad_shell_gpui`);
  do not run the suite with a stub. The wasm backend is tested by the browser
  harness and by `cargo check` in CI.
- **Cargo.lock churn.** No new packages are required (`web-time` and `wgpu` are
  already in the lock); the new dependency edges are the only change.

## 10. File-level checklist (M0)

- Add `crates/ferrocad_shell_core/` (manifest + `src/{lib,app}.rs` + moved
  `shell/`, `ui/`, `panels/`, `hud/`, `workbench/`, `model/`, `runtime/`).
- Rename `crates/ferrocad_gpui` → `crates/ferrocad_shell_gpui`; keep
  `src/{lib,python,backend,events}.rs`.
- Update `Cargo.toml` members and the `[workspace.dependencies]` entries.
- Update `ferrocad` and `ferrocad_parametric` imports and `Cargo.toml`s.
- Route the 66 `python::` call sites through `with_backend`/`with_backend_mut`.
- Move the per-frame statics to the pure crates and re-export from `*_py`.
- Update `docs/developer/architecture/repackaging.md`, the web probe README, and
  this task's index entries; record the rename in `releasing.md`.

## 11. References

- [`../research/web-wasm-host.md`](../research/web-wasm-host.md) — the browser
  probe, the wasm findings and the verified baseline.
- [`../../developer/architecture/repackaging.md`](../../developer/architecture/repackaging.md)
  — the shell-library/edition shape this split preserves.
- [`../record/tasks/dev-tools-window.md`](../record/tasks/dev-tools-window.md) —
  the second-window design the browser cannot take as-is.
- [`../record/tasks/viewport-event-routing.md`](../record/tasks/viewport-event-routing.md)
  — occlusion and the input-gateway fallback.
- [`../../developer/viewport/inventor-renderer.md`](../../developer/viewport/inventor-renderer.md)
  — the viewport element and the presenter.
