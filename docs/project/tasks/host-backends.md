# Host backends: one shell, one contract, several runtimes

Status: **task (2026-11-04). S1–S8 done; the backend ladder is complete.**
Companions: [`wasm-shell.md`](wasm-shell.md) (M0–M6, done), [`../research/web-wasm-host.md`](../research/web-wasm-host.md)
(the browser findings), [`../../developer/architecture/repackaging.md`](../../developer/architecture/repackaging.md)
(the shell-library/edition shape), [`../../developer/architecture.md`](../../developer/architecture.md)
(the crate map), [`../../developer/architecture/property-types.md`](../../developer/architecture/property-types.md)
(the property registry) and [`mod-events.md`](mod-events.md) (the mod contract, E1).

## 0. Decisions

Every claim below is backed by a `file:line` in §1–§6. This section is the short
version.

| # | Decision | Why | Lands in |
| --- | --- | --- | --- |
| **D1** | `AppBackend` stays the **only** shell↔engine contract. Backends are not shell forks. | The shell already installs one `Box<dyn AppBackend>` and needs no other knowledge of the engine. | — |
| **D2** | Backends are named by the **runtime they embed**, not by platform: `RustAppBackend` (none), `PythonAppBackend` (CPython), `JsAppBackend` (JS/WASM mods), and later a wasm-Python backend. | Platform is a property of the *host*; the document API is identical. | S5 ✓ |
| **D3** | The Rust backend becomes its own crate, `ferrocad_shell_rust`, used by **both** the browser host and a desktop edition. | Today the only Rust backend lives inside the web host (`document.rs`), so "Rust document" reads as "browser-only". It is not. | S5 ✓ |
| **D4** | Property **presentation** (editor kind, formatting, text→value) moves into `ferrocad_core` and is exposed to Python by `ferrocad_py`. | It is a property of the `Property` enum, not of a GUI. `ferrocad_py` stays a pure core binding. | S3 ✓ |
| **D5** | Part primitives store lengths as `Property::Quantity` (mm), not `Property::Float`. | That is what a FreeCAD `Part::Box.Length` is on disk, and the current mismatch loses data (§3.3). | S3 ✓ |
| **D6** | **One** display path: document → `StageMesh`, with the "which object is drawable, how is it coloured" logic in **`ferrocad_core`** (`drawable`, `effective_color`, `origin` — a document question, so it lives with the document). `ferrocad_shell_api::display` keeps only `StageMesh`, the payload that crosses the seam. | Two copies exist today; one of them draws a fillet on top of the box it consumed (§3.2). | S2 ✓ |
| **D7** | `ferrocad_inventor` becomes a **producer-side compatibility adapter** (pivy scenes → `Stage`), not a second render path. The legacy methods leave `AppBackend` for a `ScriptHost` trait. | Keeps FreeCAD compatibility for legacy workbenches without making every backend implement a scenegraph. | S6 (split ✓; overlay replacement later) |
| **D8** | File verbs (**Open/Save**) are a **host** concern: the shell picks a path and moves bytes; the backend only decodes/encodes them. | `open_document(name, bytes)`/`save_document() -> Vec<u8>` are already byte-based, and the browser proved that shape works. | S1 ✓ |
| **D9** | Native dialogs use GPUI's `App::prompt_for_paths` / `prompt_for_new_path` — xdg-desktop-portal on Linux. | Already in the framework; no dialog crate needed. | S1 ✓ |
| **D10** | The built-in **command catalogue** becomes data in `ferrocad_shell_api::catalog`, generated into the Python package and diffed in CI. | One list of ids/labels/accels; Python still owns dynamic `addCommand`. | S4 ✓ |
| **D11** | `ferrocad_web` (the old canvas demo) is retired once its smoke coverage moves to the shell host. | It duplicates nothing renderable but owned the only TKFeat/fillet-refusal runtime check. | S2 ✓ |

### 0.1 The four questions this document answers

| Question | Answer | Evidence |
| --- | --- | --- |
| **A.** Does GPUI already have a file dialog using xdg portals? | **Yes.** `App::prompt_for_paths(PathPromptOptions)` and `App::prompt_for_new_path(dir, name)`; the Linux backend is `ashpd` (xdg-desktop-portal `OpenFileRequest`/`SaveFileRequest`). No extension filters exist in `PathPromptOptions`, so the shell must validate the suffix after the pick. The **web** backend answers `Unsupported`. | `bite-gp-authoring-1.25.203/src/app.rs:1793,1806`; `bite-gp-platform-1.25.203/src/prompt.rs:7`; `bite-gp-linux-1.25.203/src/linux/platform.rs:844,915`; `bite-gp-web-1.25.203/src/platform.rs:474,486` |
| **B.** Should `ferrocad_py` carry the property/GUI abstraction? What is `ferrocad_ui` for? | **No, and neither.** `ferrocad_py` is the PyO3 surface of `ferrocad_core` and must stay GUI-free. `ferrocad_ui` is the **Qt Designer `.ui` reader** (a typed form tree; no Qt, no rendering) — it is the *form* layer, not the property layer. The property schema belongs next to `Property` in `ferrocad_core`, exposed by `ferrocad_py` as plain functions. | `ferrocad_ui/Cargo.toml:1-8`; `ferrocad_ui/src/{model,parse,error}.rs`; `ferrocad_shell_core/Cargo.toml:22` (renders forms) |
| **C.** Is the display path split-brained? | **Yes.** The Python backend reaches the viewport through `inventor_scene`/`EventListener`/`Tool` (a scenegraph), the Rust backend through `stage_meshes`. Both are on `AppBackend`, so *every* backend is asked for both. The fix is to keep the FreeCAD-compatible shape on the **producer** side and make `StageMesh` the single path to the screen. | `shell_api/src/app.rs:153,262,267`; `shell_core/src/shell/mod.rs:1354,1359` |
| **D.** Three backends? | **Three scripting surfaces, four backends** — see §2. Rust (none), CPython, JS/WASM, and (later) Python-in-wasm. The JS and wasm-Python ones both need the same *mod* contract, so they are two runtimes over one `mod-events` surface, not two new architectures. | §2, §7 |

## 1. What exists today (verified 2026-11-04, commit `17ae6aa`)

### 1.1 Four crates, one seam

*(As it was at `17ae6aa`, before S5; today the Rust backend is its own crate — see
§4.1.)*

| Crate | Role | Python | wasm |
| --- | --- | --- | --- |
| `ferrocad_shell_api` | The contract: `AppBackend`, `StageMesh`, `CommandInfo`, `PropRow`, dialogs, view requests, selection. Depends only on pure crates (`ferrocad_stage`, `ferrocad_inventor`, `ferrocad_geom`, `ferrocad_types`). | no | yes |
| `ferrocad_shell_core` | The GPUI shell: `Shell`, chrome, panels, viewport element, `DisplaySync`, `Contributions`. | no | yes |
| `ferrocad_shell_gpui` | Native assembly: owns `pyo3`, `ferrocad_inventor_py`, `ferrocad_stage_py` and `PythonAppBackend`; exports `run_with`/`run_edition_with`. | yes | no |
| `ferrocad_web_gpui` | Browser host: the `wasm-bindgen` entry point and the page bridge, over the shared Rust backend. | no | yes |

### 1.2 Two backends, same trait

`AppBackend` (`shell_api/src/app.rs:42`) has 33 methods. The two implementations
agree on the *document* half and differ on the *script* half:

| Group | Methods | Python backend | Rust backend |
| --- | --- | --- | --- |
| Boot & document | `bootstrap`, `open_startup_document`, `open_document`, `save_document`, `model_tree`, `properties`, `stage_meshes` | forwards to `python::*` JSON calls | real, over `ferrocad_core` + OCCT |
| Editing | `set_property`, `set_visibility`, `add_datum_plane`, `run_command`, `dialog_accept/cancel` | forwards to the `FreeCADGui` registry | real, over `Document` |
| View & selection | `appearance_view`, `set_view_state`, `set_selection`, `take_selection_requests`, `set_provider`, `refresh_providers`, `grid/axis/provider_*`, `gpu_adapter` | Python registry / providers | stored state, no providers |
| Manipulators | `manipulator_specs`, `apply_interaction` | workbench Python | `ferrocad_stage` |
| **Scripting** | `evaluate`, `load_workbenches`, `hello`, `tool`, `tool_command`, `event_listener`, `inventor_scene` | CPython | stub (`Err`/no-op) |

The last row is the structural split: a backend with no interpreter must still
answer `inventor_scene` and `event_listener`, and a backend with an interpreter
answers them with a Coffee-style scene graph while its geometry also travels as
`StageMesh`.

### 1.3 The desktop still works

Checked on `17ae6aa` (no edits, warm target):

| Check | Result |
| --- | --- |
| `cargo build -p ferrocad -p ferrocad_parametric` | exit 0 |
| `ferrocad_parametric --check-init` | `ferrocad: init OK` (four-module kernel-id equality, `Part::Feature` + `makeBox`, `ShapeType == "Solid"`) |
| `ferrocad --check-init` | init OK |
| `cargo test -p ferrocad_parametric` | 3 pass, headless |
| `--render /tmp/x.png --size 320x240` | PNG written, `1 object(s) [Box] from Shell` |

The only warnings are the pre-existing dead `parse_*`/`three` helpers in
`ferrocad_occt` (from `bebdf12`). Nothing in the shell split or M0–M6 regressed the
native edition.

### 1.4 What the contract already gets right

Worth stating, because the plan below must not break it:

- **Byte-based file I/O.** `open_document(&mut self, name, bytes)` and
  `save_document() -> Vec<u8>` (`app.rs:67,74`) are already platform-neutral; the
  browser's `<input type=file>` and a native portal both produce bytes.
- **Serialization is shared.** `ferrocad_core::fcstd::decode_fcstd` /
  `fcstd_write::encode_fcstd` are the only encoders; both hosts use
  `SavedDocument` (`web_gpui/src/document.rs:172,189`;
  `ferrocad_py` for the native path).
- **Picking is shared.** `InventorViewport::pick` plus the CPU ray cast serves both
  hosts, so a pick id means the same thing everywhere.
- **The presenter is shared.** `zero_copy_presenter()` is installed by both.
- **`PropRow`/`CommandInfo`/`StageMesh` are already the right shapes.** The
  duplication is in *producing* them, not in the types.

## 2. The model

![The four layers: the platform hosts (native desktop, browser wasm), the seam
(`ferrocad_shell_core` over `AppBackend` + optional `ScriptHost`), the pluggable
backends named by runtime (CPython, no-runtime Rust, planned JS and wasm Python), and
the shared engine](../../developer/images/host-backends.svg)

### 2.1 Runtimes, not platforms

A **host** owns a window and a platform. A **backend** owns a document and,
optionally, an interpreter. The shell knows only the backend.

```
             ┌──────────────────────────────────────────────┐
   native ──▶│ ferrocad_shell_core (GPUI window, chrome)     │
   wasm   ──▶│   Shell + Box<dyn AppBackend>                 │
             │   + optional ScriptHost (script_host())       │
             └───────────────┬──────────────────────────────┘
                             │ AppBackend
        ┌────────────────────┼────────────────────┬───────────────┐
        ▼                    ▼                    ▼               ▼
  RustAppBackend      PythonAppBackend      JsAppBackend    PywasmBackend
  (no runtime)        (CPython, native)     (wasm + JS)     (wasm + Python)
  native + wasm       native                wasm            wasm (research)
  ScriptHost: none    ScriptHost: yes       ScriptHost: yes ScriptHost: yes
```

The two browsers-only backends differ in **who runs the user's code**, not in what
the shell sees. Both must answer the same document methods, which is why the
Rust document backend (D3) must be a crate, not a module in one host.

### 2.2 Three scripting surfaces

| Surface | User writes | Runs in | Reaches the document through | Contract |
| --- | --- | --- | --- | --- |
| **Python / CPython** | `python/`, workbench scripts, `FreeCAD` + `FreeCADGui` | embedded interpreter, native | `ferrocad_py` (+ `_inventor_py`, `_stage_py`) | today's JSON bridge (`shell_gpui/src/python.rs`) |
| **Rust** | workbenches compiled into the edition (`ferrocad_part_gui`, editions) | in-process | `ferrocad_core` directly | `AppBackend` + `Contributions` |
| **JS / WASM** | ES modules loaded by the page | browser JS, wasm exports | a `wasm-bindgen` module API over the Rust document | **new** — must be the `mod-events` contract (E1) |
| **Python / wasm** | the same Python as today | Pyodide or RustPython | a *new* binding layer (pyo3 does not target wasm) | research spike |

The important consequence: **the JS and wasm-Python backends are the same product
decision** — "run a user module in a browser" — with two runtimes. They should share
one module API and one event contract, so building the JS one first is not throwaway
work.

### 2.3 Names

| Thing | Name | Task | Note |
| --- | --- | --- | --- |
| The contract | `ferrocad_shell_api` | — | unchanged |
| The shell | `ferrocad_shell_core` | — | unchanged |
| Native CPython assembly | `ferrocad_shell_gpui` | S1–S5 | unchanged (it is the *native* assembly, hence "gpui"); it is host **and** backend — see §4.3 |
| Rust document backend | **`ferrocad_shell_rust`** | S5 ✓ | primary; `ferrocad_shell_doc` is the alternative |
| Browser host | `ferrocad_web_gpui` | S5 ✓ | unchanged; a pure host since S5 |
| JS backend | `ferrocad_shell_js` | S7 ✓ | wasm32 only; the spike is `publish = false` |
| Future wasm Python backend | `ferrocad_shell_pywasm` | S8 ✓ | RustPython; wasm32 at the host, `publish = false` (spike) |
| Future CPython backend crate | `ferrocad_shell_python` | (with a Rust desktop edition, §4.3) | only if that edition needs `ferrocad_shell_gpui` to stop linking `pyo3` |

`ferrocad_shell_rust` is named for the runtime it embeds (none — the document *is*
Rust), not for a scripting language. That is consistent with `ferrocad_shell_gpui`
being named for its host framework.

**Why `_python`, not `_cpython` — and why nothing is renamed today.** `_cpython`
cannot distinguish the two Python backends: Pyodide *is* CPython compiled to wasm
(§6.3). The split therefore uses `_python` (native embedding) against `_pywasm` (a
wasm runtime), matching `_rust` and `_js` one-for-one. `ferrocad_shell_gpui` keeps
its name until that split: it is a host *and* a backend today, D2 names backends, and
a rename now would be a breaking release for a crate that is about to be divided
anyway. The host names (`_gpui`, `_web_gpui`) are a separate question and are not on
this plan's critical path.

Each stage that adds a backend creates exactly one crate from this table, and it
implements the two traits from §5.1; §8 states which stage depends on which.

## 3. Duplication and drift

### 3.1 Inventory

| # | Duplicated | Copy A (browser) | Copy B (desktop) | Effort |
| --- | --- | --- | --- | --- |
| 1 | **Property presentation**: editor kind, formatting, text→value | `web_gpui/src/document.rs:1142-1312` | `python/ferrocad_shell/__init__.py:33-47,440-514` | M |
| 2 | **Visibility as one operation** | `document.rs:977-1106` | `shell_gpui/src/python.rs:600` + an embedded script | S |
| 3 | **Stage dressing**: drawable set, colour, origin | `document.rs:281-324,1110-1135` | `ferrocad_shell/__init__.py:646-678`, `ferrocad_parametric/__init__.py:60-133` | S |
| 4 | **Command catalogue**: ids, labels, accels, surface, wheel | `document.rs:43-56,326-410` | `python/ferrocad_shell` + `FreeCADGui` registry | M |
| 5 | **Fit** | `InventorViewport::frame_stage` | `shell_gpui/src/headless.rs:210-276` (`framed_camera`) | S |
| 6 | **Undo/redo, open/save** | present (`document.rs:171-191,559-590`) | absent | M |

Drift visible to a user, same object, same build:

| Case | Browser | Desktop |
| --- | --- | --- |
| `Part::Box.Length` row | `10.0 mm`, kind `quantity` (name list `Length/Width/Height/Radius/Distance`, `document.rs:1148`) | `10.0`, kind `float` (`_EDITABLE_KINDS` has no rule, `__init__.py:33`) |
| Colour property | row is editable, then `coerce` refuses (`document.rs:1298`) | row is read-only |
| Constraint property | editable, but `coerce` refuses | read-only |
| `"3.7"` into an integer row | refused | accepted (Python `int()` truncation) |

### 3.2 Bugs this duplication has already produced

1. **The fillet is drawn on top of its box (browser).** The desktop's
   `stage_meshes` builds a `consumed` set from each feature's `Base` link and skips
   those objects (`ferrocad_shell/__init__.py:663-668`); the browser's
   (`document.rs:281-324`) has no such set, so `Part::Box` and its
   `Part::Fillet` are both tessellated and drawn as overlapping solids. The browser
   README baseline logs both (`Box · 12 triangles`, `Fillet · 40 triangles`).
2. **Visibility is not the same operation.** The browser falls back to a
   `ViewObject.visibility` value when `Visibility` is absent
   (`document.rs:1091-1106`); the desktop's display hook does not, so a script-set
   `ViewObject.Visibility` is honoured in one host and ignored in the other.

### 3.3 A data-loss bug behind the property mismatch

This one is more serious than a formatting difference and is the main argument for
D5:

- A FreeCAD-authored document stores `Part::Box.Length` as `App::PropertyLength`
  (`App::PropertyLength` in the file's `Properties.xml`).
- `fcstd::decode_fcstd` maps that to `Property::Quantity`
  (`ferrocad_core/src/document/fcstd.rs:644`).
- `Document::from_saved` keeps the value as decoded and marks every registered type
  `must_execute` (`ferrocad_core/src/document/mod.rs:1102`).
- The browser's `open_document` recomputes (`web_gpui/src/document.rs:174`), so
  `Part::Box::execute` runs.
- `Part::Box::execute` reads its dimensions with `float_property`, which accepts
  only `Property::Float` and `Property::Integer`
  (`ferrocad_part/src/lib.rs:309-318`).

So a box opened from a real FreeCAD file recomputes from the **default 10 mm**;
and because the browser's `coerce` handles the `Quantity` branch
(`document.rs:1288-1290`), editing `Length` writes a `Quantity` that the next
recompute ignores. The native path is unaffected only because
`ferrocad_py::openDocument` does not recompute
(`ferrocad_py/src/lib.rs:7381-7404`).

**Fixed in S3.** Part dimension properties are `Property::Quantity` now, so a
FreeCAD-authored box recomputes from its real size; `float_property` accepts both
storages, so files written before S3 still load; and `Document::set_property` keeps
a quantity's unit when a bare number is assigned (`obj.Length = 25` stays 25 mm).

*Confidence:* high — five small, specific code paths. *Not verified by execution*:
no `.FCStd` fixture from upstream FreeCAD exists in the repo. S3 added the 15-line
test that settles it (a saved document with `App::PropertyLength`, then
`from_saved` + `recompute` + assert the dimension survived), so the claim rests on
that test rather than on an imported fixture.

## 4. Target crate graph

### 4.1 Crates

Only the new/moved ones are listed; everything else keeps its place.

| Crate | Role | Edges |
| --- | --- | --- |
| `ferrocad_shell_api` | contract + `display` (`StageMesh` only) + `catalog` (command data) | unchanged, plus nothing new |
| `ferrocad_shell_core` | shell + `files` (picker/file verbs) | unchanged |
| `ferrocad_shell_rust` **(new)** | `RustAppBackend`: the current `web_gpui/src/document.rs`, plus the sample document | `ferrocad_core`, `ferrocad_part`, `ferrocad_occt`, `ferrocad_stage`, `ferrocad_shell_api`, `serde_json` |
| `ferrocad_shell_gpui` | native assembly: CPython + `run_with` | unchanged (host *and* CPython backend — §4.3; may depend on `ferrocad_shell_rust` for a Rust-desktop edition) |
| `ferrocad_web_gpui` | browser host: entry point, JS bridge, harness | `ferrocad_shell_core`, `ferrocad_shell_rust`, `wasm-bindgen` |
| `ferrocad_core` | + `present` module (D4) and `view` dressing (D6: `drawable`/`effective_color`/`origin`) | unchanged |
| `ferrocad_py` | + thin `ferrocad.property` functions over `present` | still **only** `ferrocad_core` + `pyo3` |
| `ferrocad_shell_js` (S7) | the JS script half: `JsScriptHost` over the page's ES module, merged into `RustAppBackend` | `ferrocad_shell_api`, `ferrocad_inventor`, `ferrocad_stage`, `wasm-bindgen` |
| `ferrocad_shell_pywasm` (S8) | `PywasmScriptHost`: RustPython over the *same* `python/ferrocad_shell` source, composed by `RustAppBackend::with_script_host` | `ferrocad_shell_api`, `ferrocad_core`, `rustpython-vm`/`-stdlib`/`-pylib`; **not** `*_py` (CPython C-ABI modules) |

### 4.2 Dependency rules to add

Add these to [`architecture.md`](../../developer/architecture.md) §6 when S5 lands:

```
ferrocad_shell_rust -> ferrocad_core, ferrocad_part, ferrocad_occt,
                       ferrocad_stage, ferrocad_shell_api, serde_json
                       must NOT -> gpui, pyo3, wasm-bindgen
ferrocad_shell_gpui -> ... (unchanged), may -> ferrocad_shell_rust
ferrocad_web_gpui   -> ferrocad_shell_core, ferrocad_shell_rust, wasm-bindgen
                       must NOT -> pyo3, ferrocad_py, ferrocad_inventor_py
ferrocad_py         -> ferrocad_core, pyo3         [unchanged, and now enforced]
ferrocad_shell_api  -> ferrocad_stage, ferrocad_geom, ferrocad_types
                       must NOT -> ferrocad_occt, gpui, pyo3
```

`ferrocad_shell_rust` must not name `gpui`: it is the same backend on desktop and in
the browser, and a gpui edge would make that impossible.

### 4.3 Hosts vs backends

D2 is a *naming* rule with a structural consequence: a **host** owns the process
entry, the window and the platform APIs; a **backend** is a runtime behind
`AppBackend` (+ optional `ScriptHost`) and is otherwise platform-free. After S5 the
split is visible in the crate graph:

| Layer | Role | Crate | What it is |
| --- | --- | --- | --- |
| Contract | trait + DTOs | `ferrocad_shell_api` | `AppBackend`, `ScriptHost`, the view models, `catalog` |
| UI shell | window, chrome, panels | `ferrocad_shell_core` | platform-neutral GPUI `Shell` |
| **Backends** (D2, by runtime) | none | `ferrocad_shell_rust` | `RustAppBackend` — desktop *or* wasm |
| | CPython | *inside `ferrocad_shell_gpui`* | `PythonAppBackend` (`backend.rs`, `python.rs`) |
| | JS / wasm modules | `ferrocad_shell_js` (S7 ✓) | `JsScriptHost`, over `ferrocad_shell_rust` |
| | wasm Python | `ferrocad_shell_pywasm` (S8 ✓) | `PywasmScriptHost`, over `ferrocad_shell_rust` |
| **Hosts** | browser entry | `ferrocad_web_gpui` | wasm `cdylib`, page bridge, installs a backend |
| | native entry | `ferrocad_shell_gpui` | `run_with` + headless, **and** the CPython backend |
| | editions | `ferrocad`, `ferrocad_parametric` | the binaries an edition ships |

`ferrocad_web_gpui` is now a **pure host** (S5 moved the backend out), while
`ferrocad_shell_gpui` still straddles both: it is the native host *and* the CPython
backend. That is deliberate for now — the shipped edition needs both, and a separate
crate would not remove a dependency from any current build.

It becomes worth splitting only when a **pure-Rust desktop edition** exists (Open
Question 1). That edition must not link `pyo3`/CPython at all, at which point
`PythonAppBackend` moves to `ferrocad_shell_python` and `ferrocad_shell_gpui` is left
as a thin native host runner. The desktop then mirrors the browser exactly:
*host crate + `ferrocad_shell_rust`*, with the runtime as a separate, replaceable
crate. No rename is needed to reach that: the boundary is already `script_host()`.

## 5. Seams

### 5.1 `AppBackend` → `DocumentHost` + `ScriptHost` — **done (2026-11-05)**

The split is a **disjoint** one: a backend implements the document half, and
*optionally* exposes a script half.

```rust
pub trait AppBackend {
    // boot, document, properties, stage, view, selection, commands, dialogs,
    // manipulators  — the 26 methods both hosts already implement
    fn script_host(&self) -> Option<&dyn ScriptHost> { None }
}

/// The scripting half. `None` for a backend with no interpreter.
pub trait ScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String>;
    fn load_workbenches(&self) -> Result<WorkbenchReport, String>;
    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String>;
    fn tool_command(&self, name: &str, verb: &str) -> bool;
    fn inventor_scene(&self) -> Result<Scene, String>;
    fn event_listener(&self) -> Box<dyn EventListener>;
}
```

- The scripting methods stay on `AppBackend` **defaulted and forwarding**, so every
  existing call site (`ferrocad_shell_core::app::evaluate`, `tool`, …, and the shell's
  console and dispatch) is unchanged. `ferrocad_shell_rust` inherits the defaults and
  therefore reports "scripting is not available in this build";
  `PythonAppBackend` returns `Some(self)`.
- `ScriptHost` is all `&self`: none of it touches the document model, and the
  interpreter guards its own state.

**`inventor_scene` moved rather than disappeared.** It stays on `ScriptHost` (it is a
scripting capability: the host runs a `pivy.coin` script and extracts the tree). It is
the compatibility path for workbenches that build their own Coin scene —
`ActiveView.getSceneGraph().addChild(...)`, which the facade registers through
`FerroCADGui.addOverlay` — and it is what the base app's fallback scene uses. Its
documented eventual replacement is `ScriptHost::overlays()` returning stage-native
scenes (D7), which is a later stage; deleting it now would remove the demo scene and
nothing else.

`event_listener` also moved onto `ScriptHost`, so `ActiveView.addEventCallbackPivy`
(`SoEventCallback` dispatch) keeps working exactly as before: the shell asks the
script half for the listener, and a document-only backend gets the no-op.

Compatibility is checked by the Draft conformance group: `test_pivy` (2/2) adds a
node via `getSceneGraph()` and passes on the split build, the group's summary is
unchanged, and `ferrocad_shell_api`'s new tests pin the forwarding and the
no-script-host defaults.

### 5.2 One display path (D6)

```
Document ──(backend)──▶ Vec<StageMesh> ──(DisplaySync)──▶ Stage ──▶ draw
                          ▲
   ferrocad_core::{drawable, effective_color, origin}(document)  ← the shared rules
```

The rules are **document** questions, so they live in `ferrocad_core`
(`ferrocad_core/src/document/view.rs`), and both hosts call them. They own the three
decisions that were duplicated:

```rust
// ferrocad_core::document::view (re-exported at the crate root)
/// Objects a feature consumes (`Base` links) are not drawn on their own; the
/// feature's shape replaces them. This is the rule the browser was missing.
pub fn drawable(document: &Document) -> Vec<ObjectId>;
/// The object's effective RGBA (its view colour, else the module default).
pub fn effective_color(object: &DocumentObject) -> Option<[f32; 4]>;
/// The placement origin a `StageMesh` carries.
pub fn origin(object: &DocumentObject) -> Option<[f32; 3]>;
```

`ferrocad_shell_api::display` keeps only `StageMesh` — the mesh payload that crosses
the seam. It cannot hold `drawable`: it does not depend on `ferrocad_core` (§4.2).

The backend keeps `tessellate` (it owns the kernel) and the `StageMesh` assembly;
it stops deciding *which* objects exist in the picture.

### 5.3 Property presentation (D4) — **done**

```rust
// ferrocad_core/src/present.rs — pure, no window, no Python, no kernel
pub enum EditorKind { Text, Bool, Int, Float, Quantity, Enum, Color }
pub fn kind(value: &Property) -> Option<EditorKind>;
pub fn kind_for_type(type_id: &str) -> Option<EditorKind>;   // via the type registry's default
pub fn format(value: &Property) -> String;
pub fn coerce(current: &Property, text: &str) -> Result<Property, String>;
```

Rules that the old duplicate tables got wrong:

- the kind comes from the **value**, never its name (a `Quantity` carries a unit), so
  a Part length and a module float cannot diverge;
- one spelling of each value (`True`, `80.0`, `10 mm`, `#428cd9`, `Box: Edge1`);
- a colour edit keeps the current alpha, and a multi-colour list **refuses** a
  single-colour edit instead of truncating it;
- a constraint clamps, an integer refuses `3.7`, and a bare float still accepts a
  typed `25 mm`.

`ferrocad_py` exposes exactly these to Python — `propertyKindOf(obj, prop)`,
`formatProperty(obj, prop)`, `coercePropertyText(obj, prop, text)` — and
`ferrocad_shell.properties`/`set_property` call them; the shell keeps only the
FreeCAD-specific rows (`Settings/<Feature>`, enum choices).

D5 (lengths as `Quantity`) is what makes the kind rule correct rather than a
heuristic. `float_property` grows a `Quantity` arm so both storage shapes execute;
the `.FCStd` writer already encodes `Quantity` as `App::PropertyLength`
(`fcstd_write.rs:688-696`), so files stay FreeCAD-shaped.

Migration note: an existing Rust-written `.FCStd` stores `App::PropertyFloat` for
`Part::Box.Length`. `float_property` must keep accepting `Float` so those files keep
working; only newly created objects use `Quantity`.

### 5.4 Command catalogue as data (D10) — done

```rust
// ferrocad_shell_api/src/catalog.rs
pub struct CommandDef { /* id, menu, tooltip, accel, group, cmd_type, surface,
                          short, tone, parent, wheel, checkable, dev_only */ }
pub const TOGGLE_GRID: &str = "App_ToggleGrid";   // …and the other ids, named once
pub fn builtin() -> &'static [CommandDef];
pub fn find(id: &str) -> Option<&'static CommandDef>;
impl CommandDef { pub fn info(&self) -> CommandInfo; }
pub fn move_dialog() -> DialogSpec;
pub fn to_json() -> serde_json::Value;
pub fn json_text() -> String;
```

- The Rust backend uses `builtin()` directly: `ferrocad_shell_rust`'s
  `commands()` is the catalogue plus live state (`checked`, `enabled`), with
  browser-only verbs (`App_Fit`, undo/redo, `Draft_Snap_Lock`, hide/show) as
  overrides beside it, not in the table.
- `cargo run -p ferrocad_shell_api --example catalog >
  python/ferrocad_shell/catalog.json` **generates** the file; it is committed.
  Object keys are sorted, so the file does not depend on whether another crate in
  the build turned on `serde_json`'s `preserve_order` (the shell builds do).
  Every built-in's `GetResources()` in `python/ferrocad_shell` reads it
  (`_catalog_resources`), so Python owns behaviour only.
- Drift fails in two places, not one: the crate's own test
  (`the_checked_in_catalogue_is_current`) fails when the JSON falls behind the
  table, and `tests/test_commands.py::CatalogTests` fails when the live
  `FreeCADGui` registry and the JSON disagree in either direction (a built-in on
  one side without the other). `cargo test -p ferrocad_shell_api` and the base
  Python suites are both in CI.
- Python mods keep `FreeCADGui.addCommand` — the catalogue is only the built-in set.
- `App_Move`'s dialog spec exists once (in Rust) and is read by both hosts: the
  browser calls `catalog::move_dialog()`, Python's `_MoveCommand` reads
  `catalog.json["dialogs"]["App_Move"]`.

### 5.5 File verbs (D8, D9) — **done (2026-11-05)**

`App_Open` / `App_Save` are **shell** ids on every platform, intercepted in
`Shell::run_command` like `SAVE_DATUM_PLANE`, `select:` and `ferrocad-provider:`
already are (`shell/bridge/dispatch.rs`). The host supplies one thing the shell cannot
do itself: a [`FilePicker`](../../../crates/ferrocad_shell_core/src/files.rs).

```rust
pub trait FilePicker: 'static {
    fn open(&mut self, cx: &App) -> FileTask<Result<OpenOutcome, String>>;
    fn save(&mut self, cx: &App, suggested: &str) -> FileTask<Result<SaveOutcome, String>>;
    fn deliver(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
}
```

The outcome is an enum because a picker can be in one of three positions:

| Outcome | Meaning | Who finishes |
| --- | --- | --- |
| `OpenOutcome::Path(p)` / `SaveOutcome::Path(p)` | the host has paths | the shell: `AppBackend::open_path`/`save_path`, so the backend keeps the real file name |
| `OpenOutcome::Bytes { name, bytes }` | the host read the file but has no path | the shell: `Shell::open_document` |
| `Delegated` | the host owns the picker (a browser page) | the host: it calls `Shell::open_document`, or the shell serializes and calls `deliver` |
| `Cancelled` | the user cancelled or the prompt was dropped | nobody |

Delegation is what makes one trait work on both platforms: a user gesture cannot be
awaited by a Rust future without a channel back through the wasm boundary, so the
browser's picker asks the page (`window.__ferrocad.requestOpen`) and returns
`Delegated`; the page calls `ferrocad_open` → `Shell::open_document` when it has the
bytes. Its save is `Delegated` too: the shell serializes the document and the picker
hands the bytes to the page's download.

- **Native** (`NativeFilePicker`, installed by `Shell::from_model`): GPUI's platform
  prompt plus `std::fs` — a path goes to the backend, so the interpreter-backed
  backend opens the file itself and records the name for the next save.
- **Browser** (`WebFilePicker`, installed by `ferrocad_web_gpui` at boot): the page's
  `<input type=file>` and download.
- The shell suggests the save name from `ShellModel::document` (set at boot and on
every open), not from the first model-tree node — that node is the synthetic
  `Settings` root when a provider is registered, which used to suggest
  `Settings.FCStd`.

Both hosts now replace the document through the one `Shell::replace_document`, which
is also the byte path's — one place does the rollback, the refresh and the fit.
Nothing in `AppBackend` changes, and the backend no longer carries file hooks:
**bytes cross the seam, paths do not.**

Testing is cheap because the GPUI test platform has `select_paths`/
`simulate_path_prompt_response` hooks
(`bite-gp-authoring-1.25.203/src/platform/test/platform.rs:247`), and the test suite
plugs fake pickers in for the byte and delegated shapes.

### 5.6 JS scripting surface (S7) — **done (2026-11-05)**

The JS backend is a **composition**, not a second document implementation:

```text
ferrocad_shell_rust::RustAppBackend   (document half)
        + ferrocad_shell_js::JsScriptHost   (script half)
        = the JS backend
```

`RustAppBackend` gained `with_script_host(Box<dyn ScriptHost>)`; the shell already asks
`AppBackend::script_host()`, so `evaluate`, the event listener and the script's
commands work with no new seam. `ScriptHost` gained two defaulted methods for the
last one:

```rust
fn commands(&self) -> Vec<CommandInfo> { Vec::new() }
fn run_command(&self, id: &str) -> Option<CommandOutcome> { None }
```

`RustAppBackend::commands()` appends them (a script command reaches the palette like
a built-in) and its `run_command` asks the script only for ids it does not own.

**The page contract.** `www/js-demo.js` is an ES module the page imports with
`?js=1`; it installs `window.__ferrocadJs` with `active`, `commands` (JSON),
`evaluate`, `runCommand` and `onEvent`. `JsScriptHost` maps those onto `ScriptHost`:
commands are read once at boot, `evaluate` returns the page's string, and the
`EventListener` forwards every pointer event with the picked stage entity (`0` for a
miss).

**Spike scope:** static command list, no JS tool session and no real `mod-events`
contract yet (the next step, per [`mod-events.md`](mod-events.md)). The smoke check
runs the whole path its two `?js=1` captures: the module registers `Demo_JS`, the
palette runs it, and a viewport click reaches `onEvent` with entity 2 (the fillet).

### 5.7 wasm Python surface (S8) — **done (2026-11-05)**

`ferrocad_shell_pywasm` is S7's composition with a real interpreter as the script
half:

```text
ferrocad_shell_rust::RustAppBackend          (document half)
      + ferrocad_shell_pywasm::PywasmScriptHost   (script half)
      = the wasm-Python backend
```

**The runtime is RustPython** (§6.3's second row). It is a Python VM written in Rust,
so it needs no C ABI and no Pyodide download; with `host_env` off it compiles for
`wasm32-unknown-unknown` — the `os`/`_io` file layer it drops needs a POSIX module
that target does not have. `getrandom` needs both `rustpython-vm`'s `wasmbind` feature
and `--cfg getrandom_backend="wasm_js"` (`.cargo/config.toml`).

**What is shared is the script, not the bindings.** `src/lib.rs` includes
`python/ferrocad_shell/__init__.py` and its `catalog.json` as source, so the module the
interpreter runs *is* the one the native assembly imports. The property rows still come
from `ferrocad_core::present`: one native function (`_ferrocad.document()`) hands the
interpreter a JSON snapshot whose `kind` and `value` the engine decided, and
`bootstrap.py` only presents it. The `*_py` PyO3 modules are **not** reachable — they
are CPython extension modules — which is the trade-off §6.3 predicted.

**Host gaps the spike fills.** `wasm32-unknown-unknown` has no POSIX module, so
`contextlib`, `io`, `pathlib` and `traceback` cannot be imported from the frozen
stdlib. `ferrocad_shell` imports all four at module scope, so `bootstrap.py` supplies
the names it touches (the console's `StringIO` capture included), and serves the
catalogue through a module-local `open`.

**The stack matters.** RustPython's compiler recurses; a host must raise the stack
before `PywasmScriptHost::new` — the tests run it on a 16 MiB worker thread for exactly
that reason.

Unlike `ferrocad_shell_js`, this crate is **not** `cfg(wasm)`: RustPython has no
`window` dependency, so the same code builds natively and its test runs
`properties()` on an ordinary `cargo test`. The wasm target is proven separately.

## 6. Platform facts the plan relies on

### 6.1 Dialogs (answer to A)

| Fact | Evidence |
| --- | --- |
| `App::prompt_for_paths(PathPromptOptions) -> oneshot<Result<Option<Vec<PathBuf>>>>` | `bite-gp-authoring-1.25.203/src/app.rs:1793` |
| `App::prompt_for_new_path(&Path, Option<&str>) -> oneshot<Result<Option<PathBuf>>>` | `:1806` |
| Options: `files`, `directories`, `multiple`, `prompt` — **no extension filter** | `bite-gp-platform-1.25.203/src/prompt.rs:7-17` |
| Linux: `ashpd::desktop::file_chooser::{OpenFileRequest, SaveFileRequest}`, `modal(true)`, window identifier; `PortalNotFound` is a distinct error | `bite-gp-linux-1.25.203/src/linux/platform.rs:844-945` |
| Web: `Unsupported`, "not supported on the web" | `bite-gp-web-1.25.203/src/platform.rs:474,486` |
| Test platform: `select_paths` | `bite-gp-authoring-1.25.203/src/platform/test/platform.rs:247` |

Filed upstream as [bite-gpui/bite-gpui#23](https://github.com/bite-gpui/bite-gpui/issues/23):
a bytes-level `prompt_for_files`/`save_file` pair (with `allowed_extensions`), so
native and web share one contract. Until it lands, S1 uses the native path only.

So: **yes, the portal dialog exists and is the right mechanism.** Two consequences:
there is no file-type filter, so the shell validates the suffix; and headless Linux
without a portal returns `Err(PortalNotFound)`, which must surface as a status line,
not a panic.

### 6.2 Web limits (unchanged from M0–M6)

- One window; `Floating`/`Dialog` are rejected (`bite-gp-web-1.25.203/src/platform.rs`),
  so Dev Tools stays in-window.
- No readback; the zero-copy presenter is the only display path.
- No adapter enumeration.
- File I/O is the page's (`<input type=file>` / download), never a path.

### 6.3 The wasm-Python fork (research, not a decision)

The **intent is one binding layer**: the wasm-Python host should reuse
`ferrocad_py`/`ferrocad_part_py`, not fork bindings (that is what the diagram's
shared bindings box shows). Today it cannot: `pyo3` has no
`wasm32-unknown-unknown` story — that is the premise of the M0 split. The options are
about how to reach that intent:

| Option | What runs | Binding layer | Risk |
| --- | --- | --- | --- |
| **Pyodide** | CPython compiled to wasm | a new C-API or JS-mediated layer; `ferrocad_py` would need an emscripten build we do not have | large runtime download, no pyo3, threading needs COOP/COEP |
| **RustPython** | a Rust Python VM | a native Rust module registry (`rustpython-vm`), no C ABI | incomplete stdlib/C extensions (`numpy`-style workbenches will not run) |
| **MicroPython** | a small Python | a C module ABI port | least compatible with the existing `python/` tree |

None is a drop-in for the existing `python/` package. That is why §2 calls this a
*runtime* choice, not a port: the JS backend and any wasm-Python backend share the
`mod-events` contract so the ecosystem work is done once.

**Result (S8).** RustPython reaches the intent most cheaply. The
`python/ferrocad_shell` source runs **unchanged** in it and the property rules still
come from `ferrocad_core`, but the `*_py` bindings do not carry over — they are
CPython C-ABI modules — so the engine surface they expose is re-exposed as a native
module (the spike does it with one function plus a Python facade). Pyodide would keep
CPython, and therefore the bindings' semantics, but needs a JS-mediated layer, a large
download and COOP/COEP; MicroPython stays the least compatible. None of the three
reuses the bindings as-is, which settles the "where possible" in this section: it is
not possible today, and the layer that *is* shared is the script plus
`ferrocad_core`.

## 7. Backends, in detail

| Method group | Rust (`ferrocad_shell_rust`) | CPython (`ferrocad_shell_gpui`) | JS (`ferrocad_shell_js`, S7) | wasm Python (`ferrocad_shell_pywasm`, S8 ✓) |
| --- | --- | --- | --- | --- |
| document/tree/properties/stage | Rust document + OCCT | `ferrocad_py` | Rust document + JS callbacks | Rust document + Python |
| commands | `catalog::builtin` | `FreeCADGui` registry | `catalog` + JS `registerCommand` | `FreeCADGui` registry |
| `evaluate` | unsupported | CPython | `eval` in a sandboxed realm | Python |
| workbenches | compiled-in `Contributions` | `python/` scripts | ES modules | `python/` scripts |
| overlays | stage-native | pivy → `Stage` adapter | stage-native | pivy → `Stage` adapter |

The rule for a new backend: implement the **document half** first, and it is a
read-only viewer in a day; add the script half and it is a workbench host.

The table is the target for each backend. The S8 spike reaches it for the document,
`commands` and `evaluate`; `load_workbenches` reports none (there is no filesystem to
scan), `inventor_scene` is empty and the event listener forwards nothing (no `pivy`) —
those three are the mod-loading and overlay work §6.3 leaves open.

## 8. Stages

Effort is engineer-days for one person who knows the tree. Each stage is demoable.

| # | Stage | Delivers | Effort |
| --- | --- | --- | --- |
| **S1** | **Desktop file verbs** (D8/D9) — **done (2026-11-04)** | `Ctrl+O`/`Ctrl+S` in the native app through the portal; `App_Open`/`App_Save` in the Python registry; works for every backend | 3–4 d |
| **S2** | **One display path** (D6/D11) — **done (2026-11-04)** | `ferrocad_core::drawable` (the fillet is drawn instead of the box it consumed); both hosts ask it; `ferrocad_web` and `ferrocad_web_viewport` retired with their checks ported | 2–3 d |
| **S3** | **Shared property presentation** (D4/D5) — **done (2026-11-04)** | `ferrocad_core::present` (`kind`/`format`/`coerce`); `ferrocad_py` exposes `propertyKindOf`/`formatProperty`/`coercePropertyText`; Part lengths are `Quantity` (datum planes already were); `set_property` keeps a quantity's unit when given a bare number | 4–6 d |
| **S4** | **Catalogue as data** (D10) — **done (2026-11-04)** | `ferrocad_shell_api::catalog` (the table, the named ids, the Move form), generated `python/ferrocad_shell/catalog.json`, the browser backend reading `builtin()`, Python's `GetResources()` reading the JSON, and a drift test on each side | 4–5 d |
| **S5** | **`ferrocad_shell_rust`** (D2/D3) — **done (2026-11-05)** | `RustAppBackend` moved out of `ferrocad_web_gpui` into its own platform-neutral crate (no gpui, no pyo3, no wasm-bindgen); the browser installs it with two file-picker hooks and keeps only its window and page bridge; `cargo run -p ferrocad_shell_rust --example headless` boots it with no window and prints the same document | 2–3 d |
| **S6** | **Seam split** (D7) — **done (2026-11-05)** | `ScriptHost` (`evaluate`, `load_workbenches`, `tool`, `tool_command`, `inventor_scene`, `event_listener`) with `AppBackend::script_host()`, the document half unchanged; `FilePicker` as the host's half of `App_Open`/`App_Save` (native prompt, browser delegation), so the shell owns both ids on every platform and the backend carries no file hooks | 3–4 d |
| **S7** | **JS backend spike** (D2) — **done (2026-11-05)** | `ferrocad_shell_js`: `JsScriptHost` implements `ScriptHost` over `window.__ferrocadJs` (an ES module the page loads), and `RustAppBackend::with_script_host` composes it with the document half. `ScriptHost::commands`/`run_command` were added so the page's command reaches the palette; the event listener forwards picks. The page is `www/js-demo.js`, loaded with `?js=1`. | 5–8 d |
| **S8** | **wasm Python spike** (D2) — **done (2026-11-05)** | `ferrocad_shell_pywasm`: `PywasmScriptHost` runs RustPython over the *same* `python/ferrocad_shell/__init__.py` (included as source, with its `catalog.json`), and `RustAppBackend::with_script_host` composes it with the document half. The property rows come from `ferrocad_core::present` through one native function; `bootstrap.py` supplies the `FreeCAD` surface and the four stdlib modules `wasm32-unknown-unknown` lacks. §6.3 records the verdict. | research |

### Acceptance

| Stage | The check |
| --- | --- |
| S1 | `#[gpui::test]`s drive the prompt hooks: the picked path reaches the backend and the title/status update; Save hands its path over; a cancelled prompt changes nothing; without a portal the status line shows the error. |
| S2 | The browser smoke log shows `stage draws Fillet` and **not** `stage draws Box` (the feature replaced its base), while both shapes still tessellate; desktop output unchanged; `cargo test -p ferrocad_shell_core` green. |
| S3 | `present` unit tests (kind/format/coerce, colour alpha, constraints, multi-colour refusal); `ferrocad_core` and `ferrocad_part` suites green; the base Python suites green (687); the browser smoke shows the same rows; doc checks pass. *Follow-up:* a `.FCStd` fixture from upstream FreeCAD (none in the repo) to pin the open+recompute path end to end. |
| S4 | The crate's `the_checked_in_catalogue_is_current` fails when `python/ferrocad_shell/catalog.json` is stale, and `CatalogTests` fails when a built-in exists on only one side (Python registry vs. JSON); `App_Move` exists once, in `catalog::move_dialog()`. |
| S5 | `cargo check -p ferrocad_shell_rust` (native) passes, `cargo tree -p ferrocad_shell_rust` has no gpui/pyo3/wasm-bindgen, and the browser still builds and passes its smoke check on the extracted crate (`run.sh --check`). The native example prints `document Sample (2 object(s))` and `Fillet · 40 triangles`, the same document the browser draws. |
| S6 | `AppBackend`'s scripting methods are behind `script_host()`; the browser still builds with no scripting backend; `inventor_scene` returns from the trait with the same behaviour; the Draft conformance group is unchanged (`test_pivy` 2/2). `FilePicker` is installed by both hosts: `cargo test -p ferrocad_shell_core` covers the path, bytes, delegated and error outcomes, and the browser smoke round trip (`?reopen=1`) plus `?keys=ctrl-o` still pass with the shell owning the ids. |
| S7 | The smoke check's `?js=1` captures prove the composition end to end: the page's ES module logs `ferrocad js: registered Demo_JS`, the palette run (`ctrl-k;d;e;m;o;enter`) logs `ferrocad js: run Demo_JS`, and the viewport click logs `ferrocad js: event press entity 2 at 430 310`. The crate compiles only for wasm and names no native-only edge. |
| S8 | `cargo check -p ferrocad_shell_pywasm --target wasm32-unknown-unknown` passes, and `cargo test -p ferrocad_shell_pywasm` runs the real `properties("Box")` through the interpreter: `Length` is a `quantity`/`10 mm`, the fillet's `Radius` a `float`/`3.0`, `Placement` read-only, and an unknown object reports no rows. |

S1–S4 are the "dedup" half of this document and are independent. S5–S6 are the
"one architecture" half and should land before S7/S8, because both new backends
would otherwise inherit the split seam.

The backend tasks tie together as follows — each one adds exactly the crate named in
§2.3 and reuses the half that already exists:

- **S5 is S7's document half.** `ferrocad_shell_js` does not reimplement a document;
  `JsAppBackend` delegates to `RustAppBackend` (§4.1) and adds only the JS runtime.
- **S6 is both spikes' scripting half.** `ScriptHost` is the seam S7 and S8 implement;
  `FilePicker` (§5.5) is the shared last mile for open/save, so neither spike grows
  a second file story.
- **S8 is §6.3's decision made concrete.** It picked RustPython and produced
  `ferrocad_shell_pywasm`. The bindings did *not* carry over — they are CPython
  C-ABI modules — so the bridge re-exposes the rows the shell asks for rather than
  forking `python/`; the script itself is still the shared artifact.
- **S7 and S8 share the `mod-events` contract** ([`mod-events.md`](mod-events.md)), so
  a module written for one is not thrown away for the other.

## 9. File-level changes for S1–S5

| File | Change |
| --- | --- |
| `crates/ferrocad_shell_api/src/app.rs` | `open_path`/`save_path` with byte-level defaults, overridden by a path-aware backend |
| `crates/ferrocad_shell_core/src/app.rs` | the two shell-facing helpers (`open_path`, `save_path`) |
| `crates/ferrocad_shell_core/src/shell/files.rs` **(new)** | the verbs: prompt, spawn, `open_path`, status |
| `crates/ferrocad_shell_core/src/shell/bridge/document.rs` | one `replace_document` behind both `open_document` and `open_path` |
| `crates/ferrocad_shell_core/src/shell/bridge/dispatch.rs` | intercept `App_Open`/`App_Save` next to the existing shell ids |
| `crates/ferrocad_shell_gpui/src/backend.rs`, `src/python.rs` | `open_path` → `FreeCAD.open`, `save_path` → `Document.saveAs` |
| `python/ferrocad_shell/__init__.py` | `save_document(path)` plus the `App_Open`/`App_Save` registry rows (labels `Open…`, `Save As…`, accels `Ctrl+O`, `Ctrl+S`, surface `hidden`) |
| `crates/ferrocad_shell_core/src/shell/tests/files.rs` **(new)** | the prompt → path → backend plumbing, driven by `select_paths` |
| `crates/ferrocad_core/src/document/view.rs` | `drawable`, `effective_color`, `origin`, `is_visible` (the shared dressing rules; re-exported at the crate root) |
| `crates/ferrocad_shell_api/src/display.rs` | `StageMesh` only — the mesh payload that crosses the seam |
| `crates/ferrocad_web_gpui/src/document.rs` | use `ferrocad_core::{drawable, effective_color, origin, is_visible}`; delete the local colour/visibility duplicates (S5 moved the file to `crates/ferrocad_shell_rust/src/backend.rs`) |
| `python/ferrocad_shell/__init__.py`, `python/ferrocad_parametric/__init__.py` | use the shared dressing rules where a row's presence depends on it |
| `crates/ferrocad_core/src/present.rs` **(new)** | `EditorKind`, `kind`, `kind_for_type`, `format`, `coerce`, `parse_color` |
| `crates/ferrocad_core/src/lib.rs` | `pub mod present` |
| `crates/ferrocad_part/src/lib.rs`, `src/dress_up.rs` | dimensions default to `Quantity`; `float_property` accepts both |
| `crates/ferrocad_py/src/lib.rs` | `propertyKindOf`, `formatProperty`, `coercePropertyText` |
| `python/ferrocad_shell/__init__.py` | `properties`/`set_property` call the shared functions; `_EDITABLE_KINDS`, `_coerce` and the colour parsing are gone |
| `crates/ferrocad_shell_api/src/catalog.rs` **(new)** | `CommandDef`, the named ids, `builtin()`, `find()`, `info()`, `move_dialog()`, `to_json`/`json_text` |
| `crates/ferrocad_shell_api/examples/catalog.rs` **(new)** | writes `python/ferrocad_shell/catalog.json` |
| `crates/ferrocad_web_gpui/src/document.rs` | `commands()` becomes `catalog::builtin()` + live state; the local id constants and `move_dialog()` go |
| `python/ferrocad_shell/catalog.json` **(generated)** | built-ins for the Python registry |
| `python/ferrocad_shell/__init__.py` | `_catalog_resources()`; the built-ins' `GetResources()` read the JSON; `_MoveCommand` builds its form from it; `_BUILTIN_COMMANDS` names the set |
| `tests/test_commands.py` | `CatalogTests`: registry vs. catalogue, both directions |
| `crates/ferrocad_shell_rust/` **(new crate)** | `RustAppBackend` (the former `web_gpui/src/document.rs`), its sample document, and `examples/headless.rs` |
| `crates/ferrocad_web_gpui/src/lib.rs` | installs `RustAppBackend::new().with_file_hooks(..)`; `document.rs` is gone |
| `docs/INDEX.md`, `docs/project/tasks/README.md` | this note |

## 10. Risks, open questions and doc debt

| Risk | Why it matters | Mitigation |
| --- | --- | --- |
| **`Quantity` migration changes file output** | A box created now writes `App::PropertyFloat`; after S3 it writes `App::PropertyLength`. Old files must keep loading. | `float_property` accepts both; a round-trip test with a pre-S3 file; no schema bump. |
| **S3 reaches into Python-visible behaviour** | `properties()` output changes (units appear), and `set_property` coercion changes. | Run `tools/api_docs.py`, the Python suites and the doc checks; call the change out in the release notes. |
| **Catalogue generation adds a build step** | A generated file in `python/` can drift if nothing checks it. | Two tests do: the crate's own (stale JSON) and `CatalogTests` (registry vs. JSON, both directions). Regenerate with `cargo run -p ferrocad_shell_api --example catalog > python/ferrocad_shell/catalog.json`. |
| **S1 without a portal** | Headless CI and minimal desktops have no xdg portal. | Treat `PortalNotFound` as a status line; keep `--open` for tests; drive S1's test through `select_paths`. |
| **The wasm-Python choice is unmade** | S8 could invalidate the module API chosen in S7. | Do S7 against the `mod-events` contract (E1), not against JS specifics; treat `ferrocad_py` as native-only until S8 reports. |
| **Doc debt: `architecture.md` §1/§6 predate the shell split** | It still describes `ferrocad_shell_gpui` as owning `Shell`, and listed neither `ferrocad_shell_api`/`_core` nor `ferrocad_web_gpui`. | Fixed in S5: the component table, the module tree and the dependency rules name `ferrocad_shell_api`, `_core`, `_rust` and `_web_gpui`, and record the edges this plan depends on. |
| **The action dock overlaps the Inspect bar when it wraps** (found in S1) | The dock is absolutely positioned at the bottom and `flex_wrap`s, so adding two pills made it wrap upward over the loadout/inspect strip and swallow a click. | S1 keeps file verbs off the dock (`FerroCAD_Surface: hidden`; palette + accelerators reach them). Proper fix: anchor the dock's height against the footer, or give the app menu a File section. |

Open questions:

1. Does `ferrocad_shell_rust` gain a desktop edition of its own (a "Rust" edition
   beside `ferrocad`/`ferrocad_parametric`), or is it only linked by the browser
   until a mod ships? S5's acceptance only needs a headless check. If such an edition
   ships, split `PythonAppBackend` out of `ferrocad_shell_gpui` into
   `ferrocad_shell_python` first, so the pure-Rust binary links no CPython at all
   (§4.3).
2. **Answered in S6:** the shell owns `App_Open`/`App_Save` on both platforms and
   asks the installed `FilePicker`. The browser's picker delegates to the page, so
   the page still owns the `<input>` and the download; the difference is that the
   shell, not the backend, runs the verb. The browser smoke conventions
   (`?reopen=1`, `?keys=ctrl-o`) are unchanged.
3. Should `overlays()` return `ferrocad_stage` items directly (S6) or keep
   `ferrocad_inventor::Scene` and convert in the viewport? The former removes the
   last `ferrocad_inventor` dependency from `AppBackend`; the latter is a smaller diff.

## 11. Non-goals

- **No new UI framework.** `ferrocad_shell_core` stays the only shell.
- **No Python in the browser in this plan.** §6.3 is a research fork, not a stage
  with an estimate.
- **No portable `.FCStd` byte path beyond what exists.** The serializer is shared
  already.
- **No second command registry.** Python keeps `addCommand`; the catalogue is only
  the built-ins.
- **No `ferrocad_ui` change.** It is the `.ui` reader; the form *renderer* lives in
  `ferrocad_shell_core/ui/forms.rs`. Moving property presentation there would put a
  document type in a crate that deliberately has none.

## 12. References

- [`wasm-shell.md`](wasm-shell.md) — the M0–M6 record, the crate split, the
  `AppBackend` rationale, the browser acceptance baselines.
- [`../research/web-wasm-host.md`](../research/web-wasm-host.md) — the probe, the
  presenter negotiation, and the wasm limits (§6.2).
- [`../../developer/architecture/repackaging.md`](../../developer/architecture/repackaging.md)
  — why the shell is a library and the app an edition.
- [`../../developer/architecture/property-types.md`](../../developer/architecture/property-types.md)
  — the name-keyed property registry that D4/D5 extend.
- [`mod-events.md`](mod-events.md) — the event contract S7/S8 must use.
- GPUI: `bite-gp-authoring` `App::prompt_for_paths`/`prompt_for_new_path`;
  `bite-gp-platform::prompt::PathPromptOptions`; `bite-gp-linux` `ashpd`;
  `bite-gp-web` `Unsupported`.
