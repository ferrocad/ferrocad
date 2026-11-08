# Hosts and backends: one shell, one contract, four runtimes

Status: implemented (2026-11-06). The shell/backend split, the `ScriptHost` seam, the
file verbs and all four runtime configurations are in the tree and covered by CI —
including the native one, which since S9 is the *same composition* as the two browser
ones (`RustAppBackend` + a `ScriptHost`), not a second `AppBackend`. The browser
installs all four through one switch (`ferrocad_shell_web`, `?runtime=plain|pywasm|js`).
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
`ferrocad_core`/`ferrocad_part` engine, which the JS half and wasm addons reach
through `ferrocad_doc_api`'s JSON doorway](../images/host-backends.svg)

The two browser configurations differ in **who runs the user's code**, not in what the
shell sees. Both are `RustAppBackend` plus a script half —
`with_script_host(Box::new(JsScriptHost))` and
`…(Box::new(PywasmScriptHost::live()?))` — and neither reimplements a document. The
browser host does not choose either by hand: `ferrocad_shell_web` does, from the page's
query string.

Crates today:

| Role | Crate | Notes |
| --- | --- | --- |
| Contract | `ferrocad_shell_api` | `AppBackend`, `ScriptHost`, and every view model |
| Document doorway | `ferrocad_doc_api` | the JSON calls a JS/wasm addon shares; core-only |
| UI shell | `ferrocad_shell_core` | platform-neutral GPUI; no engine, no interpreter |
| Backend · no runtime | `ferrocad_shell_rust` | `RustAppBackend`; desktop *or* wasm |
| Backend · CPython | `ferrocad_shell_python` | `RustAppBackend` + `PythonScriptHost`; the crate is host **and** script half |
| Backend · JS modules | `ferrocad_shell_js` | `JsScriptHost`; wasm only |
| Backend · wasm Python | `ferrocad_shell_pywasm` | `PywasmScriptHost`; RustPython |
| Browser assembly | `ferrocad_shell_web` | the runtime switch: `?runtime=plain\|pywasm\|js` |
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
- `commands()`/`run_command()` let a script contribute verbs to the palette. The
  document's `commands()` starts from the catalogue, takes each catalogue row's live
  `enabled`/`checked` from the script row of the same id (`IsActive`/`IsChecked`), and
  appends the ids the catalogue does not list — the metadata stays the catalogue's.
  `run_command` follows `catalog::executor`: the four presentation verbs are the
  script's, the edits are the document's, and a non-catalogue id goes to the script when
  the registry lists it.
- The hooks that let a script **own part of a document answer** return `Option`:
  `None` means "not mine, answer it yourself". `RustAppBackend` asks them before its
  own answer for the five merged methods — `model_tree` (appends
  `extra_tree_nodes`), `properties`/`set_property` (the `Settings/*` namespace, which
  is not in the document at all), `dialog_accept`/`dialog_cancel` (tokens it did not
  mint). That ordering is what keeps `Settings/Grid` resolvable without turning every
  property read into an interpreter round-trip.
- Three capabilities stayed with the script half *inside* the document: `stage_meshes`
  (the edition's display hook — `DisplayMode`, a datum plane's glass card, the toy
  edition's non-OCCT meshes), `apply_interaction` (every manipulator commit, the generic
  `transform`/`apply` included: the edition's provider owns the edit and its undo
  label) and `add_datum_plane`. Each answers first; the document half keeps its own
  behaviour as the fallback for a commit the script disclaims — which is what makes
  drag-to-move work in `?runtime=plain` and in an app that loads no edition mod.
- `inventor_scene` and `event_listener` live on `ScriptHost` because they are scripting
  capabilities, not rendering paths. They are the compatibility route for workbenches
  that build their own Coin scene (`ActiveView.getSceneGraph().addChild(...)`, dispatch
  through `SoEventCallback`), and a document-only backend gets the no-op listener.

### Who owns a Gui verb

The split follows FreeCAD's own two applications, and this is the rule the
[`backend-parity`](../../project/tasks/backend-parity.md) work is closing toward:

| layer | seam | owns |
| --- | --- | --- |
| App | `AppBackend` | documents, objects, property rows, meshes, edits, undo history, visibility, the selection mirror |
| Gui | `ScriptHost` | the command registry and execution, `Settings/*`, providers and their claims, dialogs, tools, the scene and event listener, view state |
| chrome | `ferrocad_shell_core` | window, tree, panes, viewport |
| fallback | the Rust arms | Gui verbs **only** when no `ScriptHost` is installed (`?runtime=plain`) |

![The split drawn out: `ferrocad_shell_core` installs one `AppBackend` (the document
half, `RustAppBackend`) and an optional `ScriptHost` (the Gui half); underneath, a
"who runs what" table names the executor of each catalogue row; the bottom band is
the one `ferrocad_core` registry both halves read](../images/backend-ownership.svg)

The table in that figure is the same rule spelled out per command: the document owns
the edits and the history, the script owns the Gui policy (which workplane, which grid
state) and the workbench's own ids, and `catalog::executor` is the single place that
says which of them answers a given id.

So a backend with a script half should not *implement* a Gui verb the script also
implements: it asks the script first and keeps its own arm for the plain runtime. B2
made the four presentation verbs (`App_ViewWorkplane`, the grid/axis/gizmo toggles) and
the live `enabled`/`checked` behind every catalogue row work that way, and B4 gave the
script's interaction providers the manipulator commits (the generic transform
included). `ScriptHost::claims` is the opt-in a mod uses to take over a document-owned
command id; the composition's surface is asserted in
`crates/ferrocad_parametric/tests/composition_surface.rs`.

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
on one `AppBackend`/`ScriptHost` seam; the browser picks among the wasm ones, and
none, through `?runtime=`](../images/host-backends-columns.svg)

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
question does not pay for an interpreter round-trip. The tree is the one read that spans
both: it lists every document the registry holds (`Application::documents()`), marks the
active one, and switches with `FreeCAD.setActiveDocument` — so the tree, the console and
`ActiveDocument` always name the same document.

**The boot belongs to the script half.** `Shell::boot_with` asks `bootstrap()` before it
reads the model tree and before it loads a workbench, so `ScriptHost` gained
`bootstrap`, `open_startup_document`, `hello`, `open_path`, `save_path` and
`activate_document`. The document
half consults them first and builds its sample document only when the script declines
*and* the registry is empty — which is why `from_shared_registry()` deliberately builds
nothing.

**One core, linked.** Every harness that boots the composition registers the modules as
built-ins (`ferrocad_combined_py::register_builtins`, or `ferrocad_py` alone where no
kernel is needed), so the interpreter and the document half share one `ferrocad_core`
and therefore one registry. The retired `PythonAppBackend` existed for the opposite
case — a test binary importing `python/ferrocad.abi3.so`, which carries its own core —
and B5 of [`backend-parity`](../../project/tasks/backend-parity.md) removed it: the
shell suite now links `ferrocad_py` (`register_test_core`) and installs
`ferrocad_shell_python::engine()`. A linked host must never import the combined
extension over its built-in; the `Part`/`Mesh`/`MeshPart` shims and
`ferrocad_combined_py::install` both refuse that image. `application_id()` exists to
make a split visible; the composition tests run in
`ferrocad_parametric/tests/composition.rs` and `composition_surface.rs` for exactly
that reason.

### JS modules (wasm)

`ferrocad_shell_js::JsScriptHost` implements `ScriptHost` over a page — the browser's
`window.__ferrocadJs` is an ES module the page loads (`?js=1` imports it before the wasm
module boots, because the command list is read once at startup). The crate calls out and
never links the document.

**The page is a trait, not a browser.** [`Page`] is the eight-call contract; `WebPage`
implements it over `window` and is the only `cfg(wasm)` piece, so the crate builds
natively and its tests drive a fake page (the command merge, the display hook, the
doorway and the event mapping are all covered without a browser). A desktop host supplies
the same trait over an embedded engine — `rquickjs`, decided in B7.

**The document doorway.** A JS workbench has to create objects and set properties, and a
second `ferrocad_core` in the page would be a second document. So the host hands the JS
half a `DocumentApi` (`ferrocad_doc_api`), which `Page::install_kernel` turns into
`window.ferrocad.call(request)` before the first command runs. The page sees JSON, the
host's one document answers, and Part arrives because the host registered it — the
doorway itself is core-only. See
[`../../project/research/js-addons.md`](../../project/research/js-addons.md) for the
protocol, the three shapes (static link, doorway, copied engine) and what is not built
(a display hook, a wasm bundle).

The JS backend is the composition
`RustAppBackend::from_shared_registry().with_script_host(JsScriptHost::with_kernel(..))`.
The registry source is what the doorway reads.

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

`bootstrap.py` supplies the `FreeCAD` facade over the **host's active document** and the
four stdlib modules `wasm32-unknown-unknown` cannot import (`contextlib`, `io`,
`pathlib`, `traceback` — the target has no POSIX module). The facade asks the bridge for
the rows on every access (`FreeCAD.ActiveDocument` is a live view, not a snapshot taken
at import), so the console follows an open or a close. The rows' `kind` and display text
come from `ferrocad_core::present`, so the property pane cannot drift between the two
Python backends.

The facade is **read-only**: `setattr` on an object raises rather than silently missing
the document, and recompute, the `Shape` API and every write live in the document half.
That is the price of not sharing the `*_py` bindings, and the code says so where a
script would notice.

Two operational facts: `rustpython-vm` needs `host_env` **off** to build for
`wasm32-unknown-unknown`, and its compiler recurses, so a host raises the stack before
`PywasmScriptHost::new` (the tests use a 16 MiB worker thread; the browser sets the same
value as a link argument in `ferrocad_web_gpui/build.rs`, because `wasm-ld`'s default is
1 MiB).
No `window` dependency: RustPython is plain Rust, so this crate also builds natively and
its tests run in CI without a browser (as the JS half's host logic now does — only its
`WebPage` is `cfg(wasm)`).

**Installed by the browser, chosen by the page.** `ferrocad_shell_web` is the assembly:
it composes `RustAppBackend` with no script half, with `JsScriptHost`, or with
`PywasmScriptHost`, from the query string (`?runtime=plain|pywasm|js`, with `?js=1`
and `?pywasm=1` as the short forms). The wasm-Python configuration must use
`from_shared_registry()`: the facade reads `FreeCAD.ActiveDocument` from the same
registry the document half answers from, and that pairing is what makes the console and
the model tree one document, including after an open. The plain and JS
configurations build their sample directly, as they always have. A runtime that fails
to boot is logged and the host falls back to the document alone, so a surface that
cannot start never costs the window.

### Which host ships which half

The seam has four scripting halves; each host serves three. `plain + pywasm` is the
intersection, and that is what makes the two-VM comparison a gate for both hosts at once:

| script half | native desktop | browser | why |
| --- | --- | --- | --- |
| none (`plain`) | ✓ | ✓ | no engine; the Rust arms answer |
| `PythonScriptHost` (CPython) | ✓ default | ✗ | PyO3 needs a native CPython C-ABI |
| `PywasmScriptHost` (RustPython) | ✓ builds; installed by no desktop host | ✓ | the only half both hosts can run |
| `JsScriptHost` | ✗ | ✓ | the half *is* `window.__ferrocadJs` |

Three decisions are separate even though each host currently folds them into one: what a
bundle **links**, what the host **installs** at boot, and the **default**. The browser
installs any of its three from a query string; the desktop installs exactly one
(`backend::install` → `engine()`). Giving the desktop the same choice is one target-neutral
`Runtime` (`Cpython` off wasm, `Js` on wasm) plus `--script-host=`, with a fallback to
`plain` per configuration and no change to the default.

Two engines are missing for the two ✗s, and they are different dependencies:

- **a JS engine**, for the JS half — `ferrocad_shell_js` is
  `#![cfg(target_family = "wasm")]` and binds its eight page calls to
  `window.__ferrocadJs` at the call site, so the first step is a page trait, not an engine
  (`boa`, `rquickjs`, `deno_core`);
- **a wasm engine**, for wasm addons — `ferrocad_addon_toy` already speaks a wasm import
  module (`ferrocad.call`) plus `(ptr, len)` exports, so **`wasmi`** hosts it by mapping
  that import onto `ferrocad_doc_api`'s doorway, with the addon's JS glue
  (`loader.js`, `toy.js`) re-done in Rust. An interpreter rather than a JIT: smaller, fuel
  metering for third-party mod code, and no executable memory.

Neither is linked today, so the release binary pays for neither. The decision trail, the
costs (unmeasured size, the 16 MiB stack, `host_env` off natively, the read-only facade)
and the order to build them are **B7** in
[`backend-parity.md`](../../project/tasks/backend-parity.md).

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
ferrocad_web_gpui     -> ferrocad_shell_core, ferrocad_shell_web, wasm-bindgen
                         must NOT -> pyo3, ferrocad_py, ferrocad_inventor_py
ferrocad_shell_web    -> ferrocad_shell_rust, ferrocad_shell_pywasm,
                         ferrocad_shell_js (wasm), ferrocad_doc_api,
                         wasm-bindgen, js-sys (wasm)
                         must NOT -> gpui, pyo3, ferrocad_py, ferrocad_inventor_py
ferrocad_doc_api      -> ferrocad_core, ferrocad_shell_api, serde_json
                         must NOT -> ferrocad_part, ferrocad_occt, gpui, pyo3
                         (Part arrives at run time through the object registry)
ferrocad_shell_api    -> ferrocad_stage, ferrocad_geom, ferrocad_types
                         must NOT -> ferrocad_occt, gpui, pyo3
ferrocad_shell_js     -> ferrocad_shell_api, ferrocad_inventor,
                         ferrocad_stage, wasm-bindgen
                         must NOT -> gpui, pyo3, ferrocad_py, ferrocad_shell_rust,
                                     ferrocad_core, ferrocad_part
ferrocad_shell_pywasm -> ferrocad_shell_api, ferrocad_core, ferrocad_inventor,
                         ferrocad_doc_api, rustpython-vm, rustpython-stdlib,
                         rustpython-pylib
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
- **The `ferrocad_shell_python` split** (host and CPython backend in separate crates) waits
  for a second *product*, not a second edition of this one — the CPython-free host has to
  host something. An FCStd viewer is interpreter-free but not kernel-free (`.brp` is OCCT's
  payload, so scripted documents are the lossy case); a different kernel is a crate behind
  `ferrocad_geom::GeometryBackend`, with its own document format to match (B7).
- **A desktop JS engine and a desktop wasm engine both wait** (B7). The addon engine is
  settled — `wasmi`, an interpreter with fuel metering and no executable memory — and the
  JS half needs a page trait before any engine can be plugged in. Neither is linked today,
  so the desktop binary carries neither.
- **`ScriptHost::overlays()`** returning stage-native scenes is the documented
  replacement for `inventor_scene`; until then the Coin compatibility path stays.
- **A JS workbench can draw, but not make tools or observe.** The browser's `?addons=1`
  bundle gets the document doorway, so a command creates and edits objects, and the
  display hook (`stageMeshes`) puts an object the document cannot mesh on screen. The
  same workbench runs as a wasm module (`?addons=wasm`, `ferrocad_addon_toy`) whose only
  import is that doorway. There is no JS `Tool`, no `apply_interaction` and no
  `mod-events` subscription yet, and a bundle cannot supply the grid/provider hooks. See
  [`js-addons.md`](../../project/research/js-addons.md) §7.
- **`ferrocad_shell_pywasm` has no tool or `pivy` path.** It is installed (the browser's
  `?pywasm=1`), so the console and the property rows are real, but a script cannot
  create an interactive tool, and the Python facade is read-only. `mod-events` (the
  fixed low-frequency event set a mod subscribes to) is still an open task.
- **The browser pays for RustPython in binary size** even when it runs
  `?runtime=plain`: the interpreter is compiled in, not fetched. Nothing boots it
  unless the page asks, but the module is bigger. A cargo feature that drops the half
  from a lean build is the obvious follow-up if the size matters.
