# JS and wasm addons over the document doorway

Status: **spike (2026-11-08).** `ferrocad_doc_api`, the JS half's `with_kernel` and the
JS display hook (`stageMeshes`) are in the tree and tested; the browser path is wired and
verified in a real page (§6). The wasm-addon import ABI (§7) is designed, not built.

Companions: [`../../developer/architecture/host-backends.md`](../../developer/architecture/host-backends.md)
(§4 the JS half, §8 dependency rules), [`../../mods/README.md`](../../mods/README.md)
(the Python mod contract this mirrors) and
[`mod-events.md`](../tasks/mod-events.md) (the observe direction: first slice built, §7a).

## 0. TL;DR

`JsScriptHost` could call *out* to the page — `evaluate`, `commands`, `runCommand`,
`onEvent` — but a JS workbench could not touch the **document**: the document is
`RustAppBackend`'s, and a JS page cannot link `ferrocad_core`. It now can, through one
JSON doorway:

```text
JS addon ──▶ window.ferrocad.call(request) ──▶ DocApi ──▶ the host's one document
```

The doorway is `ferrocad_doc_api`, which depends on `ferrocad_core` and **not** on
`ferrocad_part`: Part arrives at run time because the host called
`ferrocad_part::register`, so a created `Part::Box` recomputes to a real `Shape` with no
kernel in the script half's graph.

The tempting alternative — compiling a wrapper over core/part into a **second** wasm
module and loading it next to the host — cannot work for editing the open document. Two
wasm modules have two linear memories and therefore two registries: the addon would edit
a document nobody draws. §2 is the comparison.

## 1. What a JS workbench needs

The same things `mods/Toy` needs in Python: create an object, read and write properties,
recompute, and see the result in the tree and the viewport. The shell already does the
last part for any command (`run_command` ends with `refresh` + `sync_stage`), so the
addon only needs the document.

Four things it explicitly does **not** get, by design:

- **The selection.** It belongs to the shell; an addon observes it through
  `mod-events` when that lands.
- **An open transaction.** Each write is one named transaction, committed before the
  call returns, so an addon cannot leave the document mid-edit.
- **Frame-rate access.** This is command-speed work, like a workbench button.
- **A second registry.** Everything goes through the registry the host already owns.

## 2. The three shapes

| Shape | What it is | One document? | Runtime-loaded? |
| --- | --- | --- | --- |
| **A · static link** | the workbench crate (e.g. `ferrocad_toy`) is a dependency of the host | yes — same module, same memory | no |
| **B · doorway** | the addon calls functions the host exports/imports; it holds no document | yes — one instance answers | yes |
| **C · copied engine** | the addon bundle carries its own core (+part) wasm | **no** — two registries, two kernels | yes |

C is the shape the "bundle a wrapper over core/part" idea suggests, and it is the one
that fails: the host's document lives in the host's linear memory, so the addon's copy is
invisible to the pane, the viewport and the file verbs. It is only correct for an
application that *is* the addon.

A is the cheapest and stays: a first-party workbench compiled into the host is one
module, no serialization, full type safety. `ferrocad_toy` is exactly this today (as a
Python module plus a Rust manipulator); compiling the toy into the browser host would be
the "Toy as a wasm workbench" experiment, not an addon system.

B is what the spike builds, because it is the only shape that is both dynamic and
single-document.

## 3. The doorway

`ferrocad_shell_api::DocumentApi` is the trait (`call(request: &str) -> String`), so a
script half can hold one without depending on the engine. `ferrocad_doc_api::DocApi`
implements it over `ferrocad_core::application()`'s active document.

Every request is one JSON object with a `call` name; every response is
`{"ok": true, "version": 1, "result": …}` or `{"ok": false, "version": 1, "error": "…"}`.
A failure is a response, never a panic and never an empty string, so a script reports it
and keeps running.

| `call` | arguments | `result` |
| --- | --- | --- |
| `document` | — | `{name, objects: [{name, label, type_id, properties, …}]}`, or `null` |
| `properties` | `object` | that object's rows |
| `create` | `type`, optional `name`, optional `properties` | `{name, properties, recomputed}` |
| `set` | `object`, `property`, `value` | `{object, property, value, [recompute]}` |
| `recompute` | — | `{recomputed: [names]}` |

Values are **editor text**, coerced by `ferrocad_core::present` — the same rules the
property pane uses. `"25 mm"` is a length, `"25"` is a bare float, `"banana"` is refused,
and an enumeration must spell an entry. Rows carry `kind`, so a JS editor can pick the
same control the pane does.

A `create` property value is either that text (set a property the type has) or a
**declaration**, which is how a workbench defines its own object the way
`obj.addProperty(...)` does in Python:

```json
{ "Height": { "type": "App::PropertyLength", "value": "20 mm", "group": "Base" } }
```

The type must be one `ferrocad_core::property_types` registers. The payload also carries
`entity` per object, the stage's pick identity (`id + 1`), so a display hook echoes it and
a pick still resolves to the right name.

The payload is `ferrocad_doc_api::payload`, and the wasm-Python half uses the *same*
presenter for `FreeCAD.ActiveDocument`. That is one shape for a JS addon, a Python script
and the pane; a drift in any of them fails the pywasm tests.

## 4. The loader and a bundle

The spike's loader is the page, which is honest about how little a loader needs:

```html
<script type="module">
  window.__ferrocadInstallKernel = (call) => { window.ferrocad = { call }; };
  if (params.has("addons")) {
    const { install } = await import("./addons/toy.js");
    install();
  }
  await init();
</script>
```

`JsScriptHost::with_kernel(Arc<dyn DocumentApi>)` turns the doorway into a wasm-bindgen
closure and calls `window.__ferrocadInstallKernel` once, when the backend is composed.
The closure is owned by the `JsScriptHost`, so it lives as long as the host.

A bundle is a module that exposes `install()`. It runs **before** the wasm boots (so the
shell's first `commands()` read includes its ids) and is free to wrap a page surface that
was already installed (`?js=1`). A command runs after boot, when `window.ferrocad` exists.
The generalisation is a directory per bundle:

```text
addons/toy/
  addon.json     id, version, commands (the same fields as a catalogue entry), entry
  index.js       install() / the command bodies / the display hook
  addon.wasm     optional, §7
```

## 4a. The display hook

A command can put an object in the document, but an object with no `Shape` cannot be
meshed by the document half. `www/addons/toy.js` therefore supplies both channels:

- `runCommand("Toy_Slab")` creates (`App::FeaturePython`, declared `Height`) or edits it;
- `stageMeshes(have)` returns mesh JSON for it — six flat-shaded quads built from
  `Height`, with the `entity` the doorway reported and `display: "shaded_edges"`.

`JsScriptHost::stage_meshes` passes `have` (the object names the backend already meshed)
and parses the answer with `StageMesh::from_json_array`, so a hook never has to
re-tessellate a shape the document has. The distinction the Rust side makes: a page with
no `stageMeshes` is **not a hook** (`None`; the document's rows stand), while a hook that
worked once and then fails is an error the sync reports. The first answer is cached, so a
page that only registers commands is quiet.

## 4b. A wasm bundle

`crates/ferrocad_addon_toy` is the same workbench compiled to `wasm32-unknown-unknown`
(a 250 KB cdylib, no C toolchain, no wasm-bindgen). Shape B holds: the module imports the
doorway and holds no document.

```wat
(import "ferrocad" "call" (func $call (param i32 i32) (result i64)))
```

The memory contract, and it is the whole ABI:

| direction | form |
| --- | --- |
| host → bundle | `(ptr, len)` into the bundle's memory, written with the bundle's `ferrocad_alloc` |
| bundle → host | packed `len << 32 \| ptr`, freed by the host with `ferrocad_dealloc` |
| bundle → host (the doorway) | the same `(ptr, len)`, and the *loader* allocates the response **in the bundle's memory** with `ferrocad_alloc` |

One allocator serves both directions (`Layout::from_size_align(len, 1)`), so `dealloc`
needs only the length. `www/addons/loader.js` implements the host side and knows nothing
about the DOM, which is why it runs under Node.

The split inside the crate is the useful part: `toy.rs` is the workbench (commands, the
object, the mesh) with the doorway as a closure and five native tests against the real
`DocApi`; `abi.rs` is only the exports and the import. A bundle's *behaviour* is
therefore testable without a wasm runtime, and its *boundary* is testable without a
browser:

```sh
cargo build -p ferrocad_addon_toy --release --target wasm32-unknown-unknown
node --test crates/ferrocad_web_gpui/www/addons/loader.test.mjs
```

`?addons=wasm` serves this bundle; `?addons=1` serves the JS one. Both answer the same
three page functions (`commands`, `runCommand`, `stageMeshes`), so the host does not know
which it got.

The ABI is host-neutral by construction, not only in principle: `loader.js` is the
*browser's* implementation of the host side, and a native host implements the same
`ferrocad.call` import over `ferrocad_doc_api` — which is what a desktop addon engine
(`wasmi`, B7 in [`../tasks/backend-parity.md`](../tasks/backend-parity.md)) would be. The
bundle does not know which one it got either.

## 5. `catalog.json` is not the addon table

`python/ferrocad_shell/catalog.json` is **generated** from
`ferrocad_shell_api::catalog` and drift-tested from both sides; it is the *built-in*
command table, not a registry a bundle may append to. Addon commands arrive the way a
Python mod's do: a runtime registration, merged by `ScriptHost::commands()`.

A bundle may ship a catalogue fragment for discoverability (a palette entry before the
module loads, a settings list), but that is a cache: the host's `commands()` stays the
source of truth. Making `catalog.json` writable would be a second table to drift.

## 6. What the spike proves, and what it does not

Proven natively (`cargo test -p ferrocad_doc_api`):

- a `Part::Box` created over the doorway recomputes to a `Shape` (core + part wiring);
- a `set` through editor text round-trips and shows in the rows;
- a failed create rolls back and leaves no half-built object;
- with no document open, `document` is `null` and `properties` is a clear error;
- unknown calls, unknown objects and bad values are responses, not panics.

Proven natively at the seam (`cargo test -p ferrocad_shell_web --test shape_b`) — this is
the JavaScript chain without a browser, because the test's host is written exactly like
the page's hook:

- a script half that holds **no document** composes with
  `RustAppBackend::from_shared_registry()` and reports the sample it booted;
- an `App::FeaturePython` created through the doorway appears in
  `RustAppBackend::stage_meshes()` **from the hook**, beside the document's own `Fillet`;
- the hook is told `have = ["Fillet"]` and not its own object;
- `set Toy.Height = 40 mm` through the doorway changes the mesh the next sync returns.

Type-checked for `wasm32` (no browser): `ferrocad_doc_api`, `ferrocad_shell_api` and
`ferrocad_shell_js`, including the closure that installs the kernel and the
`stageMeshes` import.

Proven under Node (`node --test crates/ferrocad_web_gpui/www/addons/loader.test.mjs`),
against the real `ferrocad_addon_toy.wasm`:

- the bundle registers `Toy_Slab`/`Toy_Taller` (`ferrocad_commands`, a packed string);
- a command crosses the import and creates the object (the fake doorway records the
  `create` the bundle sent — the real protocol, checked by `ferrocad_doc_api`'s tests);
- the hook's mesh crosses back intact (24 positions, 36 indices, parts per triangle);
- a second command edits the same object and the hook's height follows;
- `have = ["Toy"]` answers `[]`.

The doorway is faked there on purpose: that test is about the *boundary*, not the
document.

Proven on the desktop too (`ferrocad_shell_wasm/tests/shipped.rs`, in CI): the same
`ferrocad_addon_toy.wasm` is instantiated by `wasmi` over the **real** `DocApi`, so the
commands are listed, `Toy_Slab` creates its object through the doorway, and the hook
returns the slab's mesh. Two hosts, one artifact — see
[`../tasks/backend-parity.md`](../tasks/backend-parity.md) B7e.

Verified in a real page (2026-11-09, `crates/ferrocad_web_gpui/smoke.sh`, headless Chrome
against the `www/pkg` build of 2026-11-08): `?runtime=js&addons=wasm&keys=ctrl-k;t;o;y;enter
&probe=1` logs `ferrocad: scripting runtime js`, the bundle's `registered Toy_Slab,
Toy_Taller`, `run Toy_Slab`, `document.edited {"transaction":"Create Toy","objects":
["Toy"]}` and `document.recomputed {"objects":["Toy"]}`, and the document probe returns
`Toy` as `App::FeaturePython` with `Height = 20 mm` beside the sample's `Box` and `Fillet`.
The same run also checks the JS bundle (`?js=1`), the palette round trip and the save/open
reopen. The pkg predates the desktop work of this session; `run.sh --check` rebuilds it and
re-runs this check when a current artifact is wanted.

## 7. Next steps, in order

1. ~~**Run the page in a browser** and assert the capture~~ — done (see §6): `smoke.sh`
   covers `?js=1`, `?runtime=js&addons=wasm` and the palette round trip.
2. **A JS `Tool`** (§7b below). The observe direction it depends on is now built:
   the two document events ship in this slice (§7a).
3. ~~**Capabilities.**~~ Done on both hosts (`backend-parity.md` B7e): a bundle is
   read-only unless its `<stem>.json` manifest says `{"document": "read-write"}`, and the
   refusal happens before the request reaches the document — in Rust on the desktop and
   inside `loader.js`'s doorway on the page. `loader.js`'s `WRITE_VERBS` mirrors
   `ferrocad_shell_api::document_api::is_write`, which is the drift to watch.

Neither half running on the desktop is deliberate, not a gap in this list: **B7** in
[`../tasks/backend-parity.md`](../tasks/backend-parity.md) treats a desktop JS engine and a
desktop wasm engine as two *separate* missing dependencies, with the addon one first,
because the browser is the shipping surface for both.

### 7a. Events: done, this slice

The fixed vocabulary is `ferrocad_shell_api::events::DocumentEvent`
(`crates/ferrocad_shell_api/src/events.rs:19`) with `document.edited`
(`events.rs:28`) and `document.recomputed` (`events.rs:31`).
`ScriptHost::on_document_event` has a default body
(`crates/ferrocad_shell_api/src/app.rs:613`), so no existing host changed.

`RustAppBackend` emits one pair per committed edit (`create`/`set` through the doorway
are the other emitter): `announce`
(`crates/ferrocad_shell_rust/src/backend.rs:229`) is called once by `edit`
(`backend.rs:213`), and the doorway emits from `create`/`set`/`recompute`
(`crates/ferrocad_doc_api/src/lib.rs:181`, `:278`, `:299`). A refused write emits
nothing. `JsScriptHost::on_document_event` (`crates/ferrocad_shell_js/src/lib.rs:205`)
forwards to `window.__ferrocadJs.onDocumentEvent(id, payloadJson)` (the import is
`lib.rs:76`), and both page bundles log one line per event. The payload is
`{transaction, objects}` for an edit and `{objects}` for a recompute.

### 7b. A JS `Tool`: designed, not built

`ScriptHost::tool(name) -> Box<dyn Tool>`
(`crates/ferrocad_shell_api/src/app.rs:441`) is already in the seam, and
`Tool::on_event`, `Tool::items` and `Tool::finish` are enough to draw and end a
session (`crates/ferrocad_stage/src/interaction/tool.rs:83`, `:86`, `:118`). Two
things stop a page from supplying one.

**(a) A script half cannot express the directive.** The shell starts a tool only from
`CommandOutcome.tool` (`crates/ferrocad_shell_api/src/commands.rs:297`) in
`Shell::run_command` (`crates/ferrocad_shell_core/src/shell/bridge/dispatch.rs:377`).
`JsScriptHost::run_command` (`crates/ferrocad_shell_js/src/lib.rs:239`) builds that
outcome from `js_run_command(id) -> Result<bool, JsValue>` (`lib.rs:63`), so the page
can only say `true`/`false` — never `tool` (or `gizmo`, `dialog`, `view`). And even if
it could, `Shell::tool_for` (`dispatch.rs:733`) sends only `python:`-prefixed ids
(`PYTHON_TOOL_PREFIX`, `dispatch.rs:13`) to `AppBackend::tool`
(`crates/ferrocad_shell_api/src/app.rs:235`) and so to `ScriptHost::tool`; the three
session sites repeat that prefix test (`cancel_tool` `dispatch.rs:749`, `finish_tool`
`dispatch.rs:788`, `finish_active_tool` `crates/ferrocad_shell_core/src/shell/mod.rs:966`).

**(b) `items()` returns owned `ShapeItem`s.** `Tool::items` is
`&[ShapeItem]` (`crates/ferrocad_stage/src/interaction/tool.rs:86`), and `ShapeItem`
is a private-field type built through `ShapeItem::new` and the `with_*`/`set_*`
mutators (`crates/ferrocad_stage/src/scene/shape.rs:129`, `:198`, `:245`). There is
no JSON→`ShapeItem` path: the only JSON mesh constructor is
`StageMesh::from_json_array` (`crates/ferrocad_shell_api/src/display.rs:74`), which
makes display rows, not stage items. A `JsTool` therefore has to own a
`Vec<ShapeItem>` and rebuild it inside `on_event` — reusing the items and calling
`ShapeItem::set_shape` (`shape.rs:245`) so the mesh ids stay stable, which the
`items` contract requires (`tool.rs:73-75`).

**Smallest safe slice.** (1) Let `runCommand` answer either a boolean (as now) or a
JSON object; `JsScriptHost::run_command` parses both into `CommandOutcome`, so
`{ok: true, tool: "js:toy_polyline"}` reaches the shell without touching
`ScriptHost::run_command` (`crates/ferrocad_shell_api/src/app.rs:684`). (2) Teach
`tool_for` and the cancel/finish prefix tests a `js:` prefix beside `python:`;
`JsScriptHost::tool` then returns a `JsTool`. (3) The `JsTool` forwards `on_event` to
a page call and caches its `Vec<ShapeItem>`; `finish()` keeps the default (`None`,
`tool.rs:118`), so the first tool only draws. A committing tool (one that returns
`InteractionCommit`) is the slice after that.

## 8. Known costs

- **JSON per call.** Fine for a command, and the display hook is one call per **sync**,
  not per frame. It is the first path where a large document's hook would matter: the
  hook sees the whole payload, so a bundle that rebuilds a mesh for every object would
  pay for every row. The `have` list is what keeps a well-written hook to its own
  objects.
- **A linear scan by name.** `Document::get_by_name` is fine at the sizes a command
  addresses; nothing here is a hot path.
- **The toggle is coarse.** A bundle can create objects the edition cannot edit, and the
  pane will show whatever rows the type has. That is the same freedom a Python mod has.
