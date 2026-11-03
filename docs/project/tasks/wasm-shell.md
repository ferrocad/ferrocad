# The wasm shell: one GPUI UI crate, two backends

Status: **task (2026-11-03). M0–M4 done (2026-11-03); M5 planned.**
Companions: [`../research/web-wasm-host.md`](../research/web-wasm-host.md) (the
probe and the wasm findings), [`../../developer/architecture/repackaging.md`](../../developer/architecture/repackaging.md)
(the shell-library/edition shape), [`../record/tasks/dev-tools-window.md`](../record/tasks/dev-tools-window.md)
(why the inspector is a second window today) and
[`../record/tasks/viewport-event-routing.md`](../record/tasks/viewport-event-routing.md)
(occlusion and the `gpui_interop` fallback).

## 0. Decision

The Python dependency is removed from the browser target by a **physical crate
boundary**, not by a feature flag. Four crates:

| Crate | Role | Python | wasm |
| --- | --- | --- | --- |
| **`ferrocad_shell_api`** | The **contract**: the `AppBackend` trait and every view model it returns (bootstrap, tree rows, property rows, stage meshes, commands, dialogs, view requests, selection items). Depends only on pure crates. | **No** | Yes |
| **`ferrocad_shell_core`** | The GPUI shell: `Shell`, chrome, panels, the viewport element, the shell view models, and the backend slot/helpers. | **No** | Yes |
| **`ferrocad_shell_gpui`** | The native shell assembly: depends on `ferrocad_shell_core` (the native build is core + Python), owns `pyo3`, `ferrocad_inventor_py`, `ferrocad_stage_py`, the Python `AppBackend`, and the `run`/`run_with`/`HostConfig` entry points. **Renamed from `ferrocad_shell_gpui`.** | Yes | No |
| **`ferrocad_web_gpui`** | The browser host: depends **only** on `ferrocad_shell_core`, supplies the Rust `AppBackend` over `ferrocad_core`/`ferrocad_part`/`ferrocad_occt`, and keeps the existing `wasm-bindgen` entry point. | No | Yes |

`ferrocad_shell_api` was added during M0, after the original three-crate plan
turned out to be untestable: the shell's unit tests live in `ferrocad_shell_core` and
install the native backend from `ferrocad_shell_gpui`, so a dev-dependency cycle
(legal in Cargo) builds the core crate **twice** — once as the test harness and
once as a dependency. The duplicate copies make the trait object and every payload
type distinct, so `Box<dyn AppBackend>` from the host cannot be installed in the
test build. Moving the contract into its own crate gives both copies one definition
each. See §9.

The seam is named **`AppBackend`**, in `ferrocad_shell_api::app` and re-exported
by `ferrocad_shell_core::app`. It is not `HostDocument`: it is a *view-model/provider
seam* for a live workspace, not the document API. `ferrocad_core::Document` remains
the engine; the browser's `AppBackend` reads it and returns the rows, trees and
meshes the UI paints.

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
is the **shell**: `ferrocad_shell_gpui` embeds CPython, so the window, chrome, panels and
viewport element cannot be compiled for `wasm32-unknown-unknown`.

`ferrocad_shell_gpui` calls Python synchronously from the UI thread in 66 production
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

The trait lives in `ferrocad_shell_api::app`; the shape below is the M0 form (every
method has a default, so a read-only backend implements only what it can answer).

```rust
pub trait AppBackend {
    fn bootstrap(&mut self) -> Result<Bootstrap, String>;
    fn model_tree(&self) -> Result<Vec<DocumentNode>, String>;
    fn properties(&self, object: &str) -> Result<Vec<PropRow>, String>;
    fn stage_meshes(&self) -> Result<Vec<StageMesh>, String>;
    fn commands(&self) -> Result<Vec<CommandInfo>, String>;
    fn set_view_state(&self, state: &ViewState) -> Result<(), String>;
    fn set_selection(&self, items: &[SelectionItem]) -> Result<(), String>;

    // Editing and scripted hooks. The defaults leave a backend that cannot edit
    // rendering a read-only viewer.
    fn set_property(&mut self, object: &str, property: &str, value: &str) -> Result<SetResult, String>;
    fn run_command(&mut self, id: &str) -> Result<CommandOutcome, String>;
    fn rotation_quaternion(&self, object: &str) -> Option<[f64; 4]>;
    fn take_selection_requests(&mut self) -> Result<Vec<SelectionRequest>, String>;
}
```

**No `Send`.** GPUI is single-threaded, and the backend is touched from the UI
thread only. (`HostDocument: Send` in earlier sketches was wrong for GPUI.)

### 3.3 Installation and access

The trait is in `ferrocad_shell_api`; the slot and the `cx` helpers are in
`ferrocad_shell_core::app`. Store the backend as a GPUI `Global`, like
`gpui_interop` stores its pressed surface, not in a process `static`:

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

### 3.4 The per-frame statics stay in the pure crates

`ferrocad_stage_py::view` and `ferrocad_inventor_py` expose pure Rust statics the
shell calls every frame (`preview_generation`, `preview_items`, `publish_camera`,
`publish_stage`, `has_stage`, `overlays_dirty`, `overlay_scenes`). They are not
Python calls. **M0 moved them down**: the published camera/stage/preview state is
now `ferrocad_stage::view`, the overlay registry is
`ferrocad_inventor::overlays`, and both `*_py` crates delegate to them. The shell
reads them through `ferrocad_shell_core::stage_view`; the browser gets the same
queue.

`PythonEventCallbacks` is the other Python-typed shell value: the shell's event
listener is `Box<dyn EventListener>` (the trait already lives in a pure crate),
with the Python implementation in `ferrocad_shell_gpui::events` and the browser
implementation supplied by `ferrocad_web_gpui`.

## 4. Crate layout and moves

```
crates/
  ferrocad_shell_api/           # new: the shell/engine contract (no gpui, no Python)
    src/
      lib.rs
      app.rs                    # AppBackend + NoopListener
      types.rs                  # Bootstrap, PropRow, DocumentNode, …
      commands.rs               # CommandInfo, CommandOutcome, Surface, grouping
      dialog.rs                 # DialogSpec, DialogField, Kind, ValueState
      view.rs                   # ViewState, ViewRequest, ProjectionName, …
      selection.rs              # SelectionItem
      display.rs                # StageMesh
  ferrocad_shell_core/          # new; from ferrocad_shell_gpui's UI half
    src/
      lib.rs
      app.rs                    # AppBackendSlot, install_backend, with_backend, helpers
      types.rs / model/view.rs  # re-exports of the api crate, same paths as before
      shell/                    # moved: shell/{mod,render,context,...}.rs
      ui/                       # moved: chrome, dev_tools, forms, ...
      panels/                   # moved (inside shell/)
      hud/                      # moved (inside shell/)
      workbench/                # moved (commands keeps the four bridge fns)
      model/                    # moved (display keeps DisplaySync; selection keeps push)
      stage_view.rs             # re-export of the pure statics (see §3.4)
      fonts.rs                  # bundled DejaVu Sans + add_fonts (wasm)        [M2]
      assets/                   # DejaVuSans.ttf + licence                      [M2]
      runtime/
        mod.rs                  # cfg-selects the interop module
        interop.rs              # the zero-copy presenter (gpu-interop)
        render_view.rs          # DeferredView (wasm) / ShellView alias      [M3]
  ferrocad_shell_gpui/          # renamed from ferrocad_gpui (M0)
    src/
      lib.rs                    # run/run_with/HostConfig, backend install
      python.rs                 # the old runtime/python.rs (still one file at M0)
      backend.rs                # PythonAppBackend: AppBackend, scripted tools
      events.rs                 # PythonEventCallbacks: EventListener
      headless.rs               # offscreen render / STL export (native only)
  ferrocad_web_gpui/            # existing probe → host
    src/
      lib.rs                    # wasm entry: CTORS, fonts, backend, WebGl, window
      document.rs               # DocumentBackend over ferrocad_core/Part/OCCT  [M4]
    www/                        # the browser harness (index.html, pkg/)
  ferrocad/                     # base binary: depend on ferrocad_shell_gpui
  ferrocad_parametric/          # edition: depend on ferrocad_shell_gpui
```

`HostConfig` stays in `ferrocad_shell_gpui` at M0 (its fields are the app's Python
script configuration). The browser host needs only a window title; M2 can add a
small `WindowConfig` to the core rather than moving `HostConfig`.

## 5. What the browser host supplies

**M2 shipped `SampleBackend`**, which made the shell real without any kernel. **M4
replaces it with `DocumentBackend`** (`ferrocad_web_gpui/src/document.rs`), over a
real `ferrocad_core::Document` on the Part workbench and the wasm OCCT kernel:

- `bootstrap`: `version = env!("CARGO_PKG_VERSION")`, `backend = "rust"`,
  `document = "Sample"`; a document that failed to build reports the reason rather
  than booting empty.
- **The document** (M4): a `Part::Box` (10 mm, centred through its `Placement`) and a
  `Part::Fillet` on `Edge1` with radius 3 mm, through `Document::add_object` /
  `set_property` / `recompute` — no hand-built geometry. The log names both shapes
  (`Box · 12 triangles`, `Fillet · 40 triangles`), which is the probe's baseline and
  the proof that the fillet really rebuilt from its `LinkSub`.
- `model_tree`: `Document::object_ids()` with each object's name, label and type id.
- `properties`: a `Label` row plus every property-container entry, with the editor
  kind from its type id (this document's lengths are `Property::Float`, so
  `Length`/`Width`/`Height`/`Radius` are presented as `quantity` rows in mm, as the
  desktop Part workbench does), the `status_names` mask (`Output`, …), and expression
  rows read-only. `set_property` writes the value and recomputes; a feature the kernel
  refuses keeps its last good shape and reports why.
- `stage_meshes`: every visible object with a `Shape`, tessellated by
  `ferrocad_part::backend()` at 0.2 mm and copied into `StageMesh` (positions, normals,
  triangles, edges, and the `parts` names a pick reports). The entity id is the object
  id + 1, the origin is the placement's base point, and the colour is the object's view
  colour (default: the Part steel grey).
- `commands`: the five header ids plus `Dev_ToggleTools` and a dock `App_Fit`.
  `Grid`/`Axes`/`Gizmo`/`Snap` report `checked` from the stored state and return a
  `ViewRequest`; `Fit` reports that the document is framed on load and `WP` that no
  workplane command is wired yet.
- `set_view_state`: stores the shell's state so the next command list agrees with
  the toggles.
- `evaluate`: reports that the browser build has no interpreter (the console is
  still a real transcript, and `hello` names the Rust document backend).

## 6. Blockers to clear (from the research note)

| # | Blocker | Fix | Status |
| --- | --- | --- | --- |
| 1 | pyo3 | The crate boundary; `ferrocad_web_gpui` cannot name a Python crate. | **M0 done** |
| 2 | `Instant::now()` panics on wasm | `web-time::Instant` in `ferrocad_viewport` (`inventor_view.rs`, `navicube.rs`), `ferrocad_widgets/wheel.rs`, `ferrocad_wgpu/trace.rs`. | **M1 done** (a runtime panic, not a compile one) |
| 3 | `std::env::args()` panics | `configure_trace` is native-only; the wasm entry does not call it. | **M1 done** (no wasm entry calls it) |
| 4 | `block_on` adapter/device creation hangs | Never create a renderer/adapter on wasm; the presenter supplies the device. | **M3 done** (`enable_viewport` skips `new_with_choice`) |
| 5 | Shell creates its own `WgpuRenderer` before paint | `DeferredView` on wasm; the real renderer is created in `Presenter::prepare`. | **M3 done** (`runtime/render_view.rs`, `ShellView`) |
| 6 | Presenter cfg excludes wasm | Allow `cfg(all(feature = "gpu-interop", not(target_os = "macos")))`; its same-device branch is the probe's path. | **M3 done** |
| 7 | Presenter format/MSAA | Use `surface.format()` and `SAMPLES = 1` on wasm. | **M3 done** |
| 8 | One window only | Render tree/properties/console **in the main window** behind a `panels_open` flag; do not use `Floating`/`Dialog`. | **M2 done** (`panels_sidebar`) |
| 9 | Adapter enumeration | Empty on wasm (the Dev Tools GPU panel is read-only). | **M2 done** (`open_dev_tools` is native-only) |
| 10 | Fonts absent → `resolve_font` panic | Embed a UI font and call `add_fonts` before opening the window. | **M2 done** (DejaVu Sans; its family is in GPUI's fallback stack, so no per-element family is needed) |
| 11 | WebGPU-first entry | `application_with_web_backend(WebBackendPreference::WebGl)` + `gpui::web_init()`. | **M2 done** |
| 12 | `process::exit(1)` on boot error | Log and return; the page stays usable. | **M2 done** |
| 13 | Readback image path cannot work | The presenter is mandatory; an `Unsupported` presenter shows a diagnostic, not the image path. | **M3 done** (the presenter is the only renderer; `DeferredView` draws nothing) |
| 14 | G-buffer picking needs a synchronous readback | `pick::read_texel` uses `map_async` + `poll(wait_indefinitely)`, which cannot complete in a browser. The presenter reports `supports_pick() == false` on wasm, so the viewport uses its CPU ray cast — pure math over the same queue and camera. | **M3 done** (clicking a box selects it) |
| 15 | The document links the OCCT kernel | The host must build inside `ghcr.io/lzpel/cross-wasm32-unknown-unknown` (the C++ and the kernel agree on the wasm-EH encoding), with `OCCT_WASM_ROOT` pointing at the fetched `occt-8_0_1_rev3-wasm32_unknown_unknown`, and `ferrocad_wasm::install()` + `__wasm_call_ctors()` before the first kernel call. | **M4 done** (`DocumentBackend`; the `wasm` CI job links the host) |

The full evidence (file:line) is in [`../research/web-wasm-host.md`](../research/web-wasm-host.md).

## 7. Stages

| # | Stage | Work | Acceptance |
| --- | --- | --- | --- |
| **M0** | **Crate split, no behaviour change** — **done 2026-11-03** | Create `ferrocad_shell_api` and `ferrocad_shell_core`; move the UI; rename `ferrocad_gpui` to `ferrocad_shell_gpui`; move the Python backend behind `AppBackend`; move the per-frame statics into the pure crates; update `ferrocad`/`ferrocad_parametric`/`ferrocad_toy`/`xtask`/docs. | 250 core + 52 api + 6 shell_gpui tests pass (the old 308); the native app boots and `--check-init` prints `ferrocad: init OK`; `cargo tree -p ferrocad_shell_core -e normal` has no pyo3. |
| **M1** | **Compile for wasm (no window)** — **done 2026-11-03** | `web-time` in the three crates that read a clock; `interop` stays cfg'd off wasm and `headless` stays native; the browser host depends on the core (plus gpui); CI check. | `cargo check -p ferrocad_web_gpui --target wasm32-unknown-unknown --locked` passes (CI `wasm` job); the result links no OCCT and no CPython. |
| **M2** | **A shell window in Chrome** — **done 2026-11-03** | Bundled DejaVu Sans + `fonts::install`; WebGL entry (`web_init`, no `process::exit`); `panels_open` in-window sidebar (Model/Properties/Console stacked); `SampleBackend` (two boxes, editable rows, header commands); placeholder viewport. | Headless Chrome shows the leading bar, viewport header, status line, tree, properties and console, with no panic. The console prints `FerroCAD browser shell · Rust backend (no CPython)`; clicking `Box` fills the property pane and the status line; `Grid` toggles on (dot, status and console agree). |
| **M3** | **Live 3D in the shell** — **done 2026-11-03** | `DeferredView` + `ShellView`; presenter allowed on wasm (`surface.format()`, `SAMPLES = 1`); `enable_viewport` on wasm; `SampleBackend::stage_meshes` (hand-tessellated boxes); CPU picking instead of the G-buffer. | Headless Chrome shows both boxes through `InventorViewport` with the shell around them; `?drag=1` orbits (75 434 px differ), `?wheel=1` zooms (42 132 px), and a click on a box selects it — tree row, properties, status line and gizmo. 308 native tests still pass. |
| **M4** | **Real Rust document** — **done 2026-11-03** | `DocumentBackend` over `ferrocad_core::Document` + `ferrocad_part` + `OcctBackend`; tree, property rows and stage all read the document; `set_property` recomputes. The host links the prebuilt wasm kernel (`ferrocad_wasm` stubs + `__wasm_call_ctors`). | Headless Chrome shows `Sample`: `Box` (`Part::Box`) and `Fillet` (`Part::Fillet`) in the tree, the fillet's real rows (`Base` `Box: Edge1`, `Radius 3.0 mm`, `Shape` `Output`), and the filleted box in the viewport. The console logs `Box · 12 triangles` and `Fillet · 40 triangles` — the probe's baseline. The `wasm` CI job links the host. |
| **M5** | **Editing** | `set_property`, `run_command`, toggles, recompute, stage refresh; visibility. | Changing a property in the browser recomputes the document and the viewport updates. |

M0–M4 are done: the boundary, the wasm build, a real shell in Chrome, the live 3D
viewport in it, and a real document behind both. M5 makes a viewport drag edit the
document.

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

- **`InventorViewport` + `Presenter` on wasm is proven (M3).** The element draws the
  sample stage through the presenter's same-device target, with the navicube, the
  axis cross and the overlay labels. Two things observed:
  - **GPU picking is unusable in the browser.** `pick_at` reads a texel back with
    `map_async` + `poll(wait_indefinitely)`; the poll is a no-op on the web, so the
    read fails and the presenter would report "empty space" for every click. The
    presenter therefore claims `supports_pick() == false` on wasm and the viewport
    uses its CPU ray cast. This is a browser fact, not a presenter defect.
  - **Two validation warnings at startup.** For the first frames the window surface
    is still being sized by the DOM's resize observer, so a clip rect (the 148 px
    navicube labels) can exceed it: `gpui_wgpu` logs
    `Scissor Rect … is not contained in the render target` twice and recovers. The
    same run also logs `Failed to poll device during resize: Timeout`, which the
    probe had too. Neither affects the rendered frame; neither is a panic.
- **The document bring-up (M4).** Three things are load-bearing:
  - **The build needs the cross image and the kernel.** `ferrocad_occt`'s build
    script compiles C++ against `OCCT_WASM_ROOT`; the host C++ and the prebuilt
    kernel only link if both are built by
    `ghcr.io/lzpel/cross-wasm32-unknown-unknown` (wasm-EH encoding). `cargo check` on
    the host cannot see a link failure, so the `wasm` CI job **builds** the host, not
    just checks it. `ferrocad_wasm::install()` and `__wasm_call_ctors()` must run
    before the first kernel call, which is why the backend is constructed inside
    `main` rather than in a `static`.
  - **The lengths are `Property::Float`, not `Property::PropertyLength`.** The Rust
    Part primitives store them as plain floats, so `DocumentBackend` presents
    `Length`/`Width`/`Height`/`Radius` as `quantity` rows in mm itself; reading the
    type id alone would give a unitless `float` row. If Part moves to typed lengths,
    the name-based mapping in `editor_kind` can go.
  - **Both objects are drawn.** `Box` and `Fillet` are both visible in FreeCAD's
    sense, and the fillet's shape is the filleted box, so the stage holds two nearly
    coincident solids (as the desktop does). Hiding the base is a document edit, not a
    display rule; M5 can offer it.
- **One-window panels.** The editor, properties and console must be laid out for
  the main window; the current `dev_tools_body` assumes a second window and
  double key handling (`panels_open` must not also route keys through the shell).
- **The `cx` threading.** `AppBackend` access needs `&App`/`&mut App` at every
  call site; if a free function genuinely has none, move the call rather than
  reintroducing a global.
- **The rename.** `ferrocad_shell_gpui` 0.1.5 is published; `ferrocad_shell_gpui` is a
  new package. Record the discontinuity in `releasing.md` and update the edition
  manifests in the same change.
- **Test policy.** Native tests keep the Python backend (`ferrocad_shell_gpui`);
  do not run the suite with a stub. The wasm backend is tested by the browser
  harness and by the `wasm` CI job, which now builds the host against the kernel.
- **The test-build cycle (discovered in M0).** `ferrocad_shell_core`'s tests need a
  real backend, so it dev-depends on `ferrocad_shell_gpui`, which depends on
  `ferrocad_shell_core`. Cargo permits that, but builds the core crate **twice**
  (test harness + dependency). Everything crossing the seam must therefore live in
  `ferrocad_shell_api`, including the `AppBackendSlot` key: the tests install the
  host's backend through `ferrocad_shell_core::app::install_backend`, not through
  `ferrocad_shell_gpui::backend::install` (that would write a different `Global`).
  Verify the isolation with `cargo tree -p ferrocad_shell_core -e normal`; the
  default `cargo tree` includes the dev dependency and will list `pyo3`. The
  dev-dependency is **path-only on purpose**: a packaging build strips it from the
  normalized manifest, so the crates.io order stays `ferrocad_shell_api` →
  `ferrocad_shell_core` → `ferrocad_shell_gpui` with no publish cycle.
- **Cargo.lock churn.** No new third-party packages are required (`web-time`,
  `wgpu` and `bite-gp-wgpu` are already in the lock); the new dependency edges are
  the only change (`ferrocad_web_gpui` now names `bite-gp-wgpu` so its
  `wasm-interop` feature is on, which the presenter's imported-texture path needs).

## 10. File-level checklist (M0) — **done (2026-11-03)**

- [x] Add `crates/ferrocad_shell_api/` (the `AppBackend` trait and every view model).
- [x] Add `crates/ferrocad_shell_core/` (manifest + `src/{lib,app}.rs` + moved
  `shell/`, `ui/`, `workbench/`, `model/`, `runtime/`; panels/hud stay inside
  `shell/`).
- [x] Rename `crates/ferrocad_shell_gpui` → `crates/ferrocad_shell_gpui`; keep
  `src/{lib,python,backend,events}.rs` and the native `headless.rs`.
- [x] Update `Cargo.toml` members and the `[workspace.dependencies]` entries.
- [x] Update `ferrocad`, `ferrocad_parametric`, `ferrocad_toy` and `xtask`
  imports/manifests, and `contracts/editions.json`.
- [x] Route the `python::` call sites through the `AppBackend` helpers (the two
  direct `with_gil` tool sites now sit in `ferrocad_shell_gpui::backend`).
- [x] Move the per-frame statics to `ferrocad_stage::view` /
  `ferrocad_inventor::overlays`; the `*_py` crates delegate.
- [x] Update `docs/developer/architecture/repackaging.md`, the web probe README, and
  this task's index entries; record the rename in `releasing.md`.
- [x] Validation: the 308 tests (250 core + 52 api + 6 shell_gpui) pass;
  `ferrocad_parametric --check-init` prints `ferrocad: init OK`;
  `cargo xtask editions --check` is clean; `cargo tree -p ferrocad_shell_core -e
  normal` has no `pyo3`.

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
