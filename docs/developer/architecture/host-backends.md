# Hosts and backends: one shell, one contract, four runtimes

Status: implemented (2026-11-06). The shell/backend split, the `ScriptHost` seam, the
file verbs and all four runtime configurations are in the tree and covered by CI —
including the native one, which since S9 is the *same composition* as the two browser
ones (`RustAppBackend` + a `ScriptHost`), not a second `AppBackend`.
Task record: [`host-backends.md`](../../project/record/tasks/host-backends.md) — the
plan that delivered them, kept for its stage history and its `file:line` evidence.

Companion to [`../architecture.md`](../architecture.md) (the crate map and the
dependency rules), [`mod-events.md`](../../project/tasks/mod-events.md) (the mod-facing
event contract, still open), [`repackaging.md`](repackaging.md) (the shell-library and
edition shape) and [`property-types.md`](property-types.md) (the property registry).

## 1. The model

Three things are regularly confused, so they are separated here:

- A **host** owns the process entry point, the window and the platform. It picks a
  backend and a `FilePicker`, then boots the shell.
- A **backend** owns a document and, optionally, an interpreter. It is named by the
  **runtime it embeds**, never by the platform it happens to run on.
- The **shell** (`ferrocad_shell_core`) owns the UI — chrome, panels, viewport,
  `DisplaySync`, the file verbs — and knows only the traits.

Platform is therefore a property of the host, and the document API is identical
everywhere. That is what lets the browser and a desktop edition install the *same*
`RustAppBackend`.

![The four layers: the hosts (native desktop, browser wasm) that own the entry point,
window and FilePicker; the seam (`ferrocad_shell_core` over `AppBackend` plus an
optional `ScriptHost`); one `RustAppBackend` composed with one `ScriptHost` per
runtime (CPython, RustPython, JS, or none); and the shared
`ferrocad_core`/`ferrocad_part` engine](../images/host-backends.svg)

The two browser configurations differ in **who runs the user's code**, not in what the
shell sees. Both are `RustAppBackend` plus a script half —
`with_script_host(Box::new(JsScriptHost::new()))` and
`…(Box::new(PywasmScriptHost::new()))` — and neither reimplements a document.

Crates today:

| Role | Crate | Notes |
| --- | --- | --- |
| Contract | `ferrocad_shell_api` | `AppBackend`, `ScriptHost`, and every view model |
| UI shell | `ferrocad_shell_core` | platform-neutral GPUI; no engine, no interpreter |
| Backend · no runtime | `ferrocad_shell_rust` | `RustAppBackend`; desktop *or* wasm |
| Backend · CPython | `ferrocad_shell_python` | `RustAppBackend` + `PythonScriptHost`; the crate is host **and** script half |
| Backend · JS modules | `ferrocad_shell_js` | `JsScriptHost`; wasm only |
| Backend · wasm Python | `ferrocad_shell_pywasm` | `PywasmScriptHost`; RustPython |
| Host · browser | `ferrocad_web_gpui` | `cdylib`, page bridge, a pure host |
| Host · native | `ferrocad_shell_python` | `run`/`run_with`/`check_init`; the edition binaries add `--render`/`--check-init` |

The crate is named for the runtime it embeds, not for the fact that it is also a
host: `ferrocad_shell_python` boots CPython *and* exports `run`/`run_with`/`check_init`.
It was `ferrocad_shell_gpui` until the four backends had landed, when the column names
wanted to read as a set. If a pure-Rust desktop edition ever needs to link no CPython,
the **host** half moves out rather than the backend being renamed again — a host takes
a platform name, and `ferrocad_shell_native` is the obvious candidate.

## 2. The seam

Two traits, both in `ferrocad_shell_api::app`, both all-`&self`:

```rust
pub trait AppBackend {
    // boot, document, properties, stage, view, selection, commands, dialogs,
    // manipulators — the document half
    fn script_host(&self) -> Option<&dyn ScriptHost> { None }
}

pub trait ScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String>;
    fn load_workbenches(&self) -> Result<WorkbenchReport, String>;
    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String>;
    fn tool_command(&self, name: &str, verb: &str) -> bool;
    fn inventor_scene(&self) -> Result<Scene, String>;
    fn event_listener(&self) -> Box<dyn EventListener>;
    fn commands(&self) -> Vec<CommandInfo> { Vec::new() }
    fn run_command(&self, id: &str) -> Option<CommandOutcome> { None }
    // boot and paths: bootstrap, open_startup_document, hello, open_path, save_path
    // inside the document: stage_meshes, apply_interaction, add_datum_plane
    // script-owned nodes: extra_tree_nodes, extra_properties, set_extra_property
    // mirrors: on_selection_changed, take_selection_requests, on_view_state_changed
    // providers, dialogs, the GPU preference, manipulators, the appearance preset
}
```

Rules that keep the split honest:

- The scripting methods stay on `AppBackend` **defaulted and forwarding** through
  `script_host()`, so a document-only backend reports "scripting is not available in
  this build" and every existing call site is unchanged.
- `script_host()` is `None` for a backend with no interpreter. `RustAppBackend` returns
  `None` unless a host composed one in with `with_script_host`.
- `commands()`/`run_command()` let a script contribute verbs to the palette: the
  document's `commands()` appends the script's list (dropping an id the catalogue
  already provides, so a built-in is never listed twice), and `run_command` asks the
  script only for ids it does not own.
- The hooks that let a script **own part of a document answer** return `Option`:
  `None` means "not mine, answer it yourself". `RustAppBackend` asks them before its
  own answer for the five merged methods — `model_tree` (appends
  `extra_tree_nodes`), `properties`/`set_property` (the `Settings/*` namespace, which
  is not in the document at all), `dialog_accept`/`dialog_cancel` (tokens it did not
  mint). That ordering is what keeps `Settings/Grid` resolvable without turning every
  property read into an interpreter round-trip.
- Three capabilities stayed with the script half *inside* the document: `stage_meshes`
  (the edition's display hook — `DisplayMode`, a datum plane's glass card, the toy
  edition's non-OCCT meshes), `apply_interaction` (a workbench manipulator that knows
  its own feature type) and `add_datum_plane`. Each answers first; the document half
  keeps its own behaviour as the fallback.
- `inventor_scene` and `event_listener` live on `ScriptHost` because they are scripting
  capabilities, not rendering paths. They are the compatibility route for workbenches
  that build their own Coin scene (`ActiveView.getSceneGraph().addChild(...)`, dispatch
  through `SoEventCallback`), and a document-only backend gets the no-op listener.

## 3. The file verbs belong to the host

`App_Open`/`App_Save` are the shell's on every platform. The host installs a
`ferrocad_shell_core::files::FilePicker`; the shell prompts, then hands the result to
the backend, and only a *path* result reaches `open_path`/`save_path`. Those two ask
`ScriptHost` first: the formats are the app's (`.FCStd`, the native `.ferrocad`) and
`saveAs` is what records the name a later save returns to. A host with no filesystem
of its own (the browser's JS half) declines, and the backend's byte path answers.

- `NativeFilePicker` — GPUI's `prompt_for_paths`/`prompt_for_new_path`, which is the
  xdg desktop portal on Linux, plus plain `std::fs`.
- `WebFilePicker` — delegates to the page's `<input>` and download; `ferrocad_open`
  re-enters through `Shell::open_document` when the bytes arrive.

A backend therefore carries no file hooks and no platform knowledge.

## 4. The four configurations

![The four runtime configurations behind one shell: RustAppBackend alone (no
runtime), RustAppBackend + PythonScriptHost (embedded CPython, native), RustAppBackend
+ JsScriptHost (wasm modules) and RustAppBackend + PywasmScriptHost (wasm Python), all
on one `AppBackend`/`ScriptHost` seam](../images/host-backends-columns.svg)

### Rust, no runtime

`ferrocad_shell_rust::RustAppBackend` owns a real `ferrocad_core::Document` and
tessellates through `ferrocad_part`/OCCT. It implements `AppBackend` only — never
`ScriptHost` — so it is a viewer plus the document edits the shell itself performs.
Platform-neutral by rule: no gpui, no pyo3, no wasm-bindgen. Same crate in the browser
and on a desktop edition.

### Embedded CPython (native)

Since **S9** this is the same composition as the two browser ones:

```text
ferrocad_shell_rust::RustAppBackend                (document half)
      + ferrocad_shell_python::PythonScriptHost   (script half)
      = the desktop backend
```

Both halves point at **one document** — the one in
`ferrocad_core::application::application()`, the registry `ferrocad_py` binds
`FreeCAD.ActiveDocument` to. `RustAppBackend` reaches it through `DocSource::SharedRegistry`,
which caches no handle (a script may open or close documents at run time) and always
releases the registry lock before locking a document. A re-open of an open name
replaces the contents *inside* the existing `Arc<Mutex<Document>>`, so a `PyDocument`
wrapper stays valid and undo history belongs to the document rather than to a copy.

Why the registry rather than a copy: the shell's `model_tree`, `properties`,
`set_property`, `commands` and stage now read and mutate that document **in Rust**,
while the same object is what a workbench sees as `FreeCAD.ActiveDocument`. Two copies
would have to be kept in step; one shared handle cannot drift. The JSON bridge
(`ferrocad_shell_python/src/python.rs`) is still there for everything the script owns —
the `Settings/*` nodes, the provider registry, commands a workbench registered, its
dialogs, the display hook, manipulators — which is the point of the split: a document
question does not pay for an interpreter round-trip.

**The boot belongs to the script half.** `Shell::boot_with` asks `bootstrap()` before it
reads the model tree and before it loads a workbench, so `ScriptHost` gained
`bootstrap`, `open_startup_document`, `hello`, `open_path` and `save_path`. The document
half consults them first and builds its sample document only when the script declines
*and* the registry is empty — which is why `from_shared_registry()` deliberately builds
nothing.

**The split image.** `PythonAppBackend` remains for the harnesses that do not link the
`ferrocad` extension: they import `python/ferrocad.abi3.so`, which carries its own
`ferrocad_core` and therefore its own registry. Answering everything through Python is
correct there — there is no document half in that image to disagree with it. A host
that links the modules as built-ins (every edition binary does;
`ferrocad_combined_py::register_builtins`) gets the composition. `application_id()`
exists to make the difference visible; the composition tests run in
`ferrocad_parametric/tests/composition.rs` for exactly that reason.

### JS modules (wasm)

`ferrocad_shell_js::JsScriptHost` implements `ScriptHost` over the page's
`window.__ferrocadJs` — an ES module the page loads (`?js=1` imports it before the wasm
module boots, because the command list is read once at startup). The crate calls out;
it never sees the document, which is why it must not depend on `ferrocad_core` at all.

The JS backend is the composition `RustAppBackend::with_script_host(JsScriptHost)`.

### wasm Python (RustPython)

`ferrocad_shell_pywasm::PywasmScriptHost` runs **RustPython**, a Python VM written in
Rust: no C ABI and no browser Python download. It includes
`python/ferrocad_shell/__init__.py` and its `catalog.json` **as source**, so the module
the interpreter runs is the one the native assembly imports.

The bindings are the one thing that does not carry over. Both Python backends reach the
engine through a native module registered in their interpreter — `ferrocad` (PyO3) under
CPython, `_ferrocad` (a `rustpython-vm` module) here — and only that module touches
`ferrocad_core`. The script above it is byte-identical:

```text
CPython:    ferrocad_shell → imports `ferrocad`  → ferrocad_py (PyO3 C-ABI)   → ferrocad_core
RustPython: ferrocad_shell → imports `_ferrocad` → _ferrocad (rustpython-vm)  → ferrocad_core
```

`bootstrap.py` supplies the `FreeCAD` facade over a document snapshot and the four
stdlib modules `wasm32-unknown-unknown` cannot import (`contextlib`, `io`, `pathlib`,
`traceback` — the target has no POSIX module). The snapshot's `kind` and display text
come from `ferrocad_core::present`, so the property pane cannot drift between the two
Python backends.

Two operational facts: `rustpython-vm` needs `host_env` **off** to build for
`wasm32-unknown-unknown`, and its compiler recurses, so a host raises the stack before
`PywasmScriptHost::new` (the tests use a 16 MiB worker thread). Unlike the JS half this
crate is not `cfg(wasm)`: RustPython has no `window` dependency, so it also builds
natively and its `properties()` test runs in CI without a browser.

No host installs it yet — the browser bundle does not pay for an interpreter until
something needs it.

## 5. One display path

```text
Document ──(backend)──▶ Vec<StageMesh> ──(DisplaySync)──▶ Stage ──▶ draw
                          ▲
   ferrocad_core::{drawable, effective_color, origin}(document)  ← the shared rules
```

`ferrocad_shell_api::display` holds only `StageMesh`, the payload that crosses the seam.
The rules that decide *which* objects exist in the picture are document questions, so
they live in `ferrocad_core` and both hosts call the same functions:

- `drawable(document)` — an object a feature consumes is not drawn on its own; the
  feature's shape replaces it.
- `effective_color(object)` — the view colour, else the module default.
- `origin(object)` — the placement origin a `StageMesh` carries, which the triad hugs
  and a rotate pivot turns about.

The backend keeps `tessellate` (it owns the kernel) and assembles the `StageMesh`.

A script half with a **display hook** answers `stage_meshes` first (see §2): the
edition decides the `DisplayMode` (wireframe), a datum plane's translucent card, and
the toy edition's procedural meshes — none of which the document knows. The Rust
tessellation above is what a host with no hook gets: the browser, and a full-Rust
desktop.

## 6. Property presentation

`ferrocad_core::present` decides the editor kind, the display text and the text→value
coercion; `ferrocad_py` exposes exactly those to Python as `propertyKindOf`,
`formatProperty` and `coercePropertyText`. The rules the old duplicate tables got wrong:

- the kind comes from the **value**, never the property name, so a `Quantity` carries a
  unit and a module float cannot diverge from a Part length;
- one spelling of each value (`True`, `80.0`, `10 mm`, `#428cd9`, `Box: Edge1`);
- a colour edit keeps the alpha, and a multi-colour list refuses a single-colour edit
  instead of truncating it;
- a constraint clamps, an integer refuses `3.7`, and a bare float still takes `25 mm`.

## 7. The command catalogue is data

`ferrocad_shell_api::catalog` is the one table of built-in ids, labels, accelerators and
surfaces; `cargo run -p ferrocad_shell_api --example catalog` generates
`python/ferrocad_shell/catalog.json`, and `GetResources()` reads that JSON. Python still
owns behaviour and dynamic `addCommand`; the table owns only the built-ins, and a drift
test on each side fails when the two disagree.

## 8. Dependency rules

```text
ferrocad_shell_rust   -> ferrocad_core, ferrocad_part, ferrocad_occt,
                         ferrocad_stage, ferrocad_shell_api, serde_json
                         must NOT -> gpui, pyo3, wasm-bindgen
ferrocad_shell_python   -> ferrocad_shell_core, ferrocad_py, pyo3, gpui
                         may    -> ferrocad_shell_rust
ferrocad_web_gpui     -> ferrocad_shell_core, ferrocad_shell_rust,
                         ferrocad_shell_js, wasm-bindgen
                         must NOT -> pyo3, ferrocad_py, ferrocad_inventor_py
ferrocad_shell_api    -> ferrocad_stage, ferrocad_geom, ferrocad_types
                         must NOT -> ferrocad_occt, gpui, pyo3
ferrocad_shell_js     -> ferrocad_shell_api, ferrocad_inventor,
                         ferrocad_stage, wasm-bindgen
                         must NOT -> gpui, pyo3, ferrocad_py, ferrocad_shell_rust,
                                     ferrocad_core, ferrocad_part
ferrocad_shell_pywasm -> ferrocad_shell_api, ferrocad_core, ferrocad_inventor,
                         rustpython-vm, rustpython-stdlib, rustpython-pylib
                         must NOT -> gpui, pyo3, ferrocad_py, ferrocad_shell_rust,
                                     ferrocad_part, ferrocad_occt
```

The asymmetry between the two script halves is the shape of each backend.
`ferrocad_shell_js` must not name `ferrocad_core` at all: it calls out to the page and
reports what comes back, so the document stays `RustAppBackend`'s, and a core edge would
put a second document implementation one `use` away. `ferrocad_shell_pywasm` does name
`ferrocad_core`, because the rows it feeds the interpreter come from `present` and its
snapshot is a `Document`; what it must not name is the kernel.

`ferrocad_shell_pywasm` also cannot forbid `wasm-bindgen`: `rustpython-vm`'s `wasmbind`
feature is what gives `getrandom` a `wasm32-unknown-unknown` backend. The crate must
never *call* it, which is the rule that matters.

Both script halves are checked in CI with `cargo tree`; `ferrocad_shell_rust` has the
same guard for gpui/pyo3/wasm-bindgen.

## 9. Platform facts

**Dialogs.** GPUI ships `prompt_for_paths`/`prompt_for_new_path`, backed by
xdg-desktop-portal on Linux and `Unsupported` on the web. `PathPromptOptions` has no
extension filters, so the shell validates the suffix after the pick, and a missing portal
is reported as a status line rather than an error.

**The web has no synchronous readback.** The wgpu presenter answers
`supports_pick() == true` on native and picks from an id buffer; on wasm it reports
`false`, so the viewport falls back to its CPU ray cast over the same queue and camera.
That is deliberate, not a regression: `map_async` cannot complete inside a frame.

**The wasm-Python runtime.** RustPython was chosen over Pyodide (a large download, a
JS-mediated binding layer and COOP/COEP) and MicroPython (least compatible with
`python/`). None of the three can load the `*_py` PyO3 modules, which settle the "reuse
the bindings" question: the layer that is shared is the script plus `ferrocad_core`.

## 10. Known costs and what is deferred

- **The CPython mesh hop, closed.** The host no longer round-trips the scene through
  JSON, and the producer no longer builds a Python object per number:
  `ferrocad_shell_python::python::stage_meshes` calls `stage_meshes_typed` and reads
  the rows through PyO3, and a provider may pack its arrays as `bytes` — the Part hook
  does, through `Shape.mesh(binary=True)` (`f32` triples for `positions`, `u32` for the
  index arrays). `stage_meshes()` and the docs tooling still get the JSON shape, because
  the text path unpacks the same buffers. A repeated sync does not re-mesh either: the
  `ShapeProperty` memoises its tessellation per shape handle, so an unchanged document
  costs a copy instead of a kernel pass. What remains is the *first* pass: `BRepMesh`
  spends ~100 µs per face whether or not the shape is already triangulated, so only
  meshing fewer faces (progress, not a coarser deflection) can shorten it. The cost is
  paid per edit — a drag preview repaints from the cached stage — not per frame.
  See [`opening-a-large-model.md`](../../project/tasks/opening-a-large-model.md).
- **CPU picking on wasm.** The fallback transforms every vertex of every packet into a
  fresh buffer, then scans every triangle, with no bounding-box pre-reject or BVH. Fine
  for the sample; the first thing to fix when a large model runs in the browser.
- **The `ferrocad_shell_python` split** (host and CPython backend in separate crates) is
  deferred until a pure-Rust desktop edition exists, because only that edition needs to
  link no CPython.
- **`ScriptHost::overlays()`** returning stage-native scenes is the documented
  replacement for `inventor_scene`; until then the Coin compatibility path stays.
- **`ferrocad_shell_pywasm` is not installed by any host**, and `mod-events` (the fixed
  low-frequency event set a mod subscribes to) is still an open task.
