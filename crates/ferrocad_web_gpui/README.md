# ferrocad_web_gpui

FerroCAD's **browser shell**: the real `ferrocad_shell_core` UI — leading bar,
viewport header, status line, model inspector, property editor and console — in a
GPUI window on WebGL2, over a real `ferrocad_core::Document` whose geometry is built
by the OCCT kernel compiled to wasm, with no CPython anywhere in the dependency
graph. The document backend itself is `ferrocad_shell_rust`'s `RustAppBackend` —
the same crate a desktop edition installs — so the browser host owns only the
window, the page bridge and the file picker.

This crate used to be the step-2 probe (a `Part::Box` composited through
`gpui_interop`). The shell split (M0) and the wasm build (M1) made it a host; M2
replaced the probe's demo with the shell, M3 returned the 3D viewport through the
shell's own `InventorViewport` element, and M4 replaced the hand-built stage with a
document. The plan and the verified baselines are in
[`docs/project/tasks/wasm-shell.md`](../../docs/project/tasks/wasm-shell.md).

## What it proves

1. **The same shell crate as the desktop app.** `ferrocad_shell_core` compiles for
   `wasm32-unknown-unknown` and opens its window through `gpui`'s web backend. There
   is no second implementation of the chrome, panels or key handling.
2. **The seam, not a stub.** `ferrocad_shell_rust::RustAppBackend` implements
   `ferrocad_shell_api`'s `AppBackend` over a real document. The shell asks it for
   the tree, the property rows, the commands and the stage exactly as it asks the
   Python backend natively: clicking a shape (or a tree row) fills the property pane
   and the status line; `Grid`/`Axes`/`Gizmo`/`Snap` toggle through a `ViewRequest`
   and report their new state on the next command refresh.
3. **A real document in the browser.** `Sample` is a `Part::Box` with a `Part::Fillet`
   on `Edge1`, recomputed by the wasm OCCT kernel: the fillet's `Base` is a
   `LinkSub` to the box, and its rebuild produces 40 triangles where the box has 12.
   The tree, the rows (`Base` `Box: Edge1`, `Radius 3.0 mm`, `Shape <shape> [Output]`)
   and the viewport all read that one document. Editing a property recomputes it.
4. **The live 3D viewport.** `Shell::enable_viewport` builds the real
   `InventorViewport`; on the web its renderer is the zero-copy presenter, which takes
   the window's own wgpu device on the first paint and composites the frame with
   `paint_texture` (`SAMPLES = 1`, the surface's own colour format). Orbit, wheel zoom,
   the navigation cube, the axis cross and picking all run in the browser. There is no
   readback image path: it needs a blocking `poll`, which cannot complete here.
5. **One window, panels in-window.** The web backend rejects `Floating`/`Dialog`, so
   the inspector column (`panels_sidebar`) sits beside the canvas instead of in Dev
   Tools: Model, Properties and Console are stacked and all visible at once.
6. **A real interpreter, when asked.** `ferrocad_shell_web` composes the same
   document backend with one of the browser's script halves from the query string —
   RustPython in the module (`?pywasm=1`), the page's ES module (`?js=1`), or both
   by default; `?runtime=plain` is the document-only session. The startup log names
   the chain that booted (`ferrocad: scripting runtime pywasm,js` by default).
   With RustPython the Console evaluates real Python against the same
   `FreeCAD.ActiveDocument` the model tree shows. That is RustPython linked into the
   module, so it costs size and build time; the plain runtime pays only the binary.
   Under `?addons=1` a JS bundle calls the document through the same document API the
   Python facade reads (`ferrocad_doc_api`) and draws its own object through the display
   hook: one document, no second registry, and no kernel in the bundle's graph.
   `?addons=wasm` runs the same workbench as a wasm module whose only import is that
   document API — nothing but text crosses the boundary.
7. **Text works.** GPUI's web platform starts with an empty font database and panics
   when it cannot resolve a font. The shell registers a bundled DejaVu Sans
   (`ferrocad_shell_core::fonts`), whose family is already in GPUI's fallback stack,
   before opening the window.

## Run it

```sh
crates/ferrocad_web_gpui/run.sh
```

Then open <http://localhost:8081> (the script opens it for you unless `--no-open`).
The module is tens of MB (GPUI + wgpu + OCCT, plus RustPython for `?pywasm`); the
first load takes a moment.

The document links the prebuilt wasm OCCT kernel, so the build happens inside the
digest-pinned `ghcr.io/lzpel/cross-wasm32-unknown-unknown` image (the C++ bindings and
the kernel must agree on the wasm-EH encoding). On the first run the script:

1. downloads cadrum's prebuilt **OCCT 8.0.1** for `wasm32-unknown-unknown` into
   `.occt-wasm/` and builds the one toolkit that tarball prunes (**`TKFeat`**) — both
   prepared by `tools/wasm-prepare.sh`;
2. installs the **wasm-bindgen CLI version pinned by `Cargo.lock`** into
   `target/wasm-bindgen/`;
3. builds `ferrocad_web_gpui` in the cross image, with the build output and Cargo's
   home kept on the host (`target/wasm-cargo/`, `target/wasm-cargo-home/`) so later
   runs are incremental;
4. generates `www/pkg/` with wasm-bindgen, packs `mods/` into `www/mods/`, and serves
   `www/` with `python3 -m http.server`.

Options:

```sh
PORT=9000 crates/ferrocad_web_gpui/run.sh      # another port
JOBS=4 crates/ferrocad_web_gpui/run.sh         # more build parallelism (default 2)
crates/ferrocad_web_gpui/run.sh --no-open      # don't open a browser
crates/ferrocad_web_gpui/run.sh --build-only   # build www/pkg and stop
WASM_OFFLINE=1 crates/ferrocad_web_gpui/run.sh # no network; seed the cargo home first
```

Requirements: Docker (usable without `sudo`), `curl`, `tar`, `git` and `python3`.
A host Rust toolchain is not needed.

`Ctrl+O` opens a `.FCStd` file from the browser's file picker, `Ctrl+S` downloads
the active document, and dropping a `.FCStd` on the window opens it.

`www/index.html` also drives the shell for headless captures:

| Query | Effect |
| --- | --- |
| `?click=x,y` | one pointer press/release at a client coordinate (select a tree row, run a header command, click a shape) |
| `?select=x,y` | the same press, then a short settle (an object's property rows are built from it) |
| `?keys=step;…` | `;`-separated steps: `press:x,y`, `move:x,y`, `drag:x1,y1,x2,y2`, `wheel:x,y`, `wait:ms`, a named key (`space`, `tab`, `enter`, `escape`, `backspace`, `delete`) or one character; modifiers prefix with `ctrl-`/`shift-`/`alt` |
| `?type=x,y,text` | hover, click a text field, clear it, type `text`, `Enter` |
| `?drag=1` | one pointer drag across the canvas (orbits the camera) |
| `?wheel=1` | one scroll notch (zooms) |
| `?dblclick=1` | a drag then two quick clicks (the viewport's double-click) |
| `?save=1` | run `App_Save` (`Ctrl+S`); the page logs the bytes it was asked to download |
| `?reopen=1` | save, capture those bytes instead of downloading, then open them again |
| `?probe=1` | once the synthetic input has settled, log the document the document API returns (`ferrocad harness: document {…}`), so a capture can assert on the document rather than on the canvas |
| `?manipulator=id:action:object:element:values` | send one workbench commit before the first frame, through the same call a released handle uses (`apply_interaction`): `?manipulator=dress_up:fillet:Box:Edge1:2`, `?manipulator=push_pull:extrude:Box:Face1:3`. The element may be empty and the values are comma-separated |
| `?gesture=slot:object:element:pixels` | drive a manipulator for real, with no pointer: equip the hand (`2` push/pull, `3` dress-up), select the sub-element as a click would, press on the point the manipulator drew and pull it `pixels` outward: `?gesture=3:Fillet:Edge1:60`. The value comes from the manipulator's own scrub |
| `?settings=<node>` | log the rows the settings card would render for that node (`?settings=Settings/Grid`), through the same `app::properties` call the pane makes. The tree and the card are painted into the canvas, so this is the only way a capture can read them |
| `?settings=<node>.<property>=<value>` | commit one settings row, as the pane does (`app::set_property`), and log the presentation request it returns: `?settings=Settings/Grid.minor=20` |
| `?settings_file=<url>` | load a whole settings file before the module boots. The desktop parameter format (`{version, values:[{root, group, name, kind, value}]}`) and the browser group map (`{group: {name: {Kind: value}}}`) are both accepted; existing `ferrocad.settings.*` keys are cleared first. The capture tools pass the desktop's `parameters.json` here |
| `?command=<id>` | run one command through the backend before the first frame and log its outcome, including a `select` directive: `?command=View_GridSettings`. The palette needs synthetic keystrokes, so this is how a row a runtime only lists is exercised |
| `?providers=1` | log which provider each feature has, its popover switch list (`switch=N`) and the Dev Tools registry (`registry=grid=1,…`). The page ships no `mods/` and has no filesystem to scan, so one native provider per feature and an empty switch list is the correct answer here |
| (no query) | boots the `pywasm,js` chain: RustPython plus the page's JS module |
| `?runtime=plain\|pywasm\|js` | which script half to install; `?js=1` and `?pywasm=1` are the short forms, `?runtime=` wins when both are present |
| `?addons=1` | load `www/addons/toy.js` before the module boots: a JS bundle that registers `Toy_Slab`/`Toy_Taller`, creates a `Toy` object through the document API and draws it through the display hook |
| `?addons=wasm` | the same workbench as a wasm bundle (`www/addons/toy.wasm`, built from `crates/ferrocad_addon_toy`): `loader.js` instantiates it and provides its one import |
| `?mods=<url>` | the mod-pack index to fetch, resolved against the page (default `mods/index.json`); `?mods=0` disables the fetch. Packs load **after the first frame**, and only when the composition has a Python half |

## Mods

The desktop scans `mods/`; the browser has no filesystem, so it fetches. `tools/mods_pack.py`
turns each workbench into `www/mods/<Name>.json` plus `www/mods/index.json` (both
generated and ignored — `run.sh` and `smoke.sh` run the packer). After the window's
first frame `ferrocad_web_gpui` fetches the index, then each pack in index order, and
hands the pack document to `load_workbench_pack` on the script half — the same loader
rule a scanned tree uses (`Init.py` then `InitGui.py`; a script that raises is recorded,
not fatal). The status line shows `Loading mods 3/7 · Fasteners` and then a summary;
the About card (`Settings/About`) lists what loaded and what did not.

Delivery and capability are separate. A pack the current facade cannot run (the
Coin/shader skins read a resource file and need the overlay bridge; a task panel needs
`.ui`) is reported and skipped, and the window is unaffected; `?mods=0` turns the fetch
off. The provider settings clients (`GridOverlay`, `AxisOverlay`, `GizmoOverlay`) run:
since B18b the App and Gui facades are the shared `python/` source (with `bootstrap.py`
supplying only the host gaps), so each registers its `grid`/`axis`/`gizmo` provider.

## Settings

User settings go through `ferrocad_shell_api::settings::SettingsBackend`, so the grid
schema and the card are shared while the storage stays host-specific:

| Host | Backend |
| --- | --- |
| browser | `WebSettings`: one JSON object per group under `localStorage` key `ferrocad.settings.<group>`, with an in-memory fallback when storage is unavailable |
| desktop | `PythonSettings`: `FreeCAD.ParamGet` under `User parameter:BaseApp/Preferences/<group>` |
| headless / tests | `MemorySettings` |

The native appearance groups are `Mods/grid/ferrocad`, `Mods/axis/ferrocad` and
`Mods/gizmo/ferrocad`; their keys are the settings cards' rows (`enabled`, `color`,
`minor`, `major`, `length`, `size`, …). An empty store means the built-in `ViewState`
defaults — the grid is off, the axis cross is on, spacing is 10/100 mm, and the grid
ink is `#94a3b8`. `RustAppBackend::appearance_view` applies stored values at boot and
`set_view_state` writes every applied change back to the host store. `?settings_file=`
imports the same groups into a fresh browser profile.

`?type=` still depends on the field taking pointer focus, and synthetic pointer events
race the frame GPUI takes its hit test from; treat it as best-effort in headless
captures. The palette (`Ctrl+K`) and the Move HUD focus programmatically and are
reliable there. The bottom action dock is also unreliable under synthetic input; run
a dock-only command through the palette instead (e.g. `ctrl-k;f;i;t;enter` for `Fit`).

`crates/ferrocad_web_gpui/smoke.sh` runs the baseline capture and asserts it, and
`crates/ferrocad_web_gpui/run.sh --check` builds and runs it in one step (it skips
cleanly when Chrome or `www/pkg` is missing).

## Verified

Headless Chrome, WebGL2 through ANGLE/SwiftShader
(`--use-angle=swiftshader --enable-unsafe-swiftshader`), 1280×800:

```
[INFO] gpui_wgpu::wgpu_context: Browser graphics initialized: requested=WebGl, selected=Gl, adapter="ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device ...))"
[INFO] ferrocad_shell_core::fonts: ferrocad: bundled UI font (DejaVu Sans) registered
[INFO] ferrocad_web_gpui::document: ferrocad: Box · 12 triangles, 12 edges
[INFO] ferrocad_web_gpui::document: ferrocad: Fillet · 40 triangles, 21 edges
[INFO] ferrocad_web_gpui::document: ferrocad: stage draws Fillet
[WARN] gpui_wgpu::wgpu_renderer: Failed to poll device during resize: Timeout
```

- **Both shapes tessellate; only one is drawn.** The two triangle lines are
  diagnostics over the document (they prove the kernel rebuilt both shapes).
  `stage draws Fillet` is the set the viewport actually has: the fillet consumes the
  box (`Base` is a `LinkSub` to it), so the box is not drawn beside it. That rule
  lives in `ferrocad_core::document::drawable`, and the native host asks the same
  function, so a feature cannot be drawn over its own input on either side.
- The window shows the leading bar (`☰`), the viewport header
  (`Grid / Axes / WP / Gizmo / Select: Body / Snap`), the live 3D viewport (the
  filleted shape, the axis cross and the navigation cube), the in-window `Model` /
  `Properties` / `Console` column, the loadout bar and the status line
  (`2 object(s) · Sample`, `FreeCAD 0.1.5 · rust`).
- The console greets with `FerroCAD shell · Rust document backend · OCCT · no
  CPython`: the Rust document really is the one answering. Under `?pywasm=1` it greets
  with `FerroCAD shell · RustPython · wasm Python scripting` and the Console evaluates
  Python against the same document.
- The tree lists `Box` (`Part::Box`) and `Fillet` (`Part::Fillet`). `?click=950,131`
  selects the fillet and the pane shows its real rows: `Label`, `Base`
  (`Box: Edge1`), `Edges`, `Propagate` (a toggle), `Radius` (`3.0 mm`, editable) and
  `Shape` (`<shape>`, `Output`).
- `?click=430,380` clicks the shape **in the viewport**: it is the fillet (the box's
  shape after the edge round, and the only mesh drawn), so the pane shows the fillet's
  rows and the transform gizmo and its move HUD appear — the CPU pick ran against the
  stage the presenter drew.
- `?drag=1` orbits and `?wheel=1` zooms the camera; the presenter re-composites each
  frame. `?click=40,58` runs `App_ToggleGrid` (dot, status and console agree).
- **Editing writes the document.** `?select=1004,100&keys=ctrl-shift-m;tab;1;5;enter`
  runs `App_Move`, accepts `rust-move-2 {"dx":15.0,…}`, prints `moved 1 object(s)`
  and the box's `Placement` row becomes `(10.0, -5.0, -5.0)` with the stage following.
  `?select=1004,100&keys=space;move:600,384;space` opens the object wheel, hovers
  `Hide` and commits it: `hid 1 object(s)`, the base box leaves the stage (the fillet
  stays), and a `Visibility App::PropertyBool` row appears switched off — the hide is
  a document property, not a display flag. The wheel's `Rotate` verb plus `Enter`
  commits through `apply_interaction` (`rotated`) and the box turns. Typing `25` into
  the property pane's `Length` field commits `set Box.Length` and the box lengthens.
- **Undo, redo and fit.** Appending `ctrl-z` to the Move run prints `undo: Move` and
  puts `Placement` back to `(-5.0, -5.0, -5.0)`; `ctrl-shift-z` prints `redo: Move` and
  the box moves again. Every backend edit opens one named document transaction, so an
  edit is one step. Zooming out three notches and running `Fit` through the palette
  (`ctrl-k;f;i;t;enter`) reframes the stage and prints `fit`; `Fit` is a view request,
  so it changes no stored presentation state.
- **Open and save.** `?reopen=1` runs `App_Save` (`saved Sample (3788 bytes)`), captures
  the bytes the page would have downloaded, and opens them through the file seam:
  `opened Sample (2 object(s))`. The tree, the stage, the title and the status line all
  come from the decoded document, and the selection is cleared. `?keys=ctrl-o` reaches
  the page bridge (`open requested`). The shell owns both verbs (S6) and asks this
  host's `WebFilePicker`, which delegates to the page: the `<input>`, the download and
  drag-and-drop are the page's half.
- **The wasm workbench edits the document.** `?runtime=js&addons=wasm` installs the
  page's script half and instantiates `www/addons/toy.wasm`, whose only import is the
  document API. The console shows `ferrocad: scripting runtime js`,
  `ferrocad toy (wasm): registered Toy_Slab, Toy_Taller`, and — after the palette run
  (`ctrl-k;t;o;y;enter`) — `ferrocad toy (wasm): run Toy_Slab`. `?probe=1` then reads
  the document back through `window.ferrocad.call('{"call":"document"}')` (trimmed):

  ```
  ferrocad harness: document {"ok":true,"version":1,"result":{"name":"Sample","objects":[…,
  {"name":"Toy","label":"Toy","type_id":"App::FeaturePython","entity":3,…,
  "properties":[…,{"name":"Height","type_id":"App::PropertyLength","kind":"quantity","value":"20 mm","choices":[]}],…}]}}
  ```

  `Toy` is in the same document as `Box` and `Fillet`, its type is
  `App::FeaturePython` (so only the bundle's display hook can draw it), and its
  `Height` is `20 mm`: the command reached the document through the module's one
  import, not through a second registry. `smoke.sh` asserts the console lines and that
  JSON.
- **Both Part manipulators build their feature.** A capture cannot move a mouse, so two
  probes stand in for it. `?manipulator=` sends the commit a handle *would* release;
  `?gesture=` goes further and drives the handle: it equips the hand, selects the
  sub-element through the click handler, presses on the point the manipulator drew and
  pulls it 60 px outward, so the value is the scrub's own. The backend's stage line is
  the assertion:

  ```
  ferrocad probe: dress_up fillet Box -> ok=true
  ferrocad: stage draws Fillet, Box_Fillet
  ferrocad probe: gesture slot 3 Fillet Edge1 pulled 60 from 265,362
  ferrocad: stage draws Fillet_Fillet          Radius 3.888 mm   (scrubbed from 3)
  ferrocad probe: gesture slot 2 Fillet Face1 pulled 40 from 205,326
  ferrocad: stage draws Fillet_Fuse            LengthFwd 3.621 mm
  ```

  Each feature replaced the body it consumed, the hidden extrusion is not drawn, and no
  interpreter was installed: the commits are the document half's own arms. What no
  capture can supply is the *device* — a real mouse delivering those events.
- **The grid has a settings card without an interpreter.** `?runtime=plain` installs no
  script half, so the `Settings` namespace used to be absent — the grid could be toggled
  and never configured. The document half answers it now, from the presentation state it
  is already mirrored (`AppBackend::set_view_state` at boot and after every applied
  request):

  ```
  ferrocad probe: settings Settings/Grid -> 10 row(s): enabled, color, opacity, adaptive,
    minor_style, major_width, minor_width, minor, major, radius
  ferrocad probe: settings Settings/Grid.minor=20 -> ok=true
  ferrocad probe: settings view ViewRequest { grid: None, …, grid_minor: Some(20.0),
    grid_major: Some(100.0), … }
  ```

  A write returns the `ViewRequest` the shell applies and mirrors back, so the rows and
  what the viewport draws cannot disagree, and the healing the script's card applies (a
  minor step at or above the major one raises the major; equal steps lower the minor)
  happens in the same place. All three browser runtimes get the same card — `?runtime=js`
  because the page's script half does not implement `Settings/*`, and `?runtime=pywasm`
  because `PywasmScriptHost` does not either; the card lives in the document half, so no
  configuration depends on an interpreter to configure the grid.
- **The header row is a catalogue built-in.** `View_GridSettings` used to be registered
  by the app script, so `plain` and `js` had no way to reach the card from the header.
  It is in `catalog.rs` now (surface `grid`, parent `App_ToggleGrid`), so every runtime
  lists it, and a host with no interpreter answers it by naming the node:

  ```
  ferrocad probe: command View_GridSettings -> ok=true status="grid settings"
    select=Some("Settings/grid")
  ```

  With a script half the id stays the Gui app's (`Executor::Script`), which is what
  keeps the desktop's headless-modal decision in `_GridSettingsCommand.Activated`.
- **The provider registry is the loaded packs'.** The page fetches the mod packs (see
  [Mods](#mods)), and the shipped settings clients (`GridOverlay`, `AxisOverlay`,
  `GizmoOverlay`) register their `grid`/`axis`/`gizmo` providers once the pack lands: the
  wasm facade is the shared `python/` source since B18b (`python/FreeCAD` included, so
  `ParamGet` is the facade's own), with `bootstrap.py` supplying only the host gaps. A
  registered pack reaches the About card, Dev Tools, the header popovers and its own
  `Settings/<Feature>` card; written values live in the in-memory `ParamGet`, so they
  last for the session and a reload starts from the declared defaults. Before the fetch — and with no pack loaded — the facade's
  `grid_summary`/`axis_summary` are the closed empty summaries, which the document half
  treats as "not mine", so the header names the native provider it draws. `?providers=1`
  is the measurement, and it runs at boot, before the fetch:

  ```
  ferrocad probe: providers grid=FerroCAD (native) switch=0 axis=FerroCAD (native) switch=0 registry=grid=1,axis=1
  ```

  `switch=0` is the popover's list — nothing to switch *to* with no provider mod loaded —
  while `registry` is what Dev Tools lists, one provider per feature, and it is the
  provider that actually draws the grid and the axis cross. `?runtime=pywasm,js` composes
  the chain, with both halves installed.
- **The document announces its edits to the script half.** The same capture shows the
  host's `document.edited`/`document.recomputed` events arriving in the page, once per
  edit and with the object it wrote:

  ```
  ferrocad toy (wasm): document.edited {"transaction":"Create Toy","objects":["Toy"]}
  ferrocad toy (wasm): document.recomputed {"objects":["Toy"]}
  ```

  They come from the *document API* (`DocApi::with_events`), not from the document half: an
  addon's own `create`/`set` never passes through `RustAppBackend`. A shell-driven edit
  (Move, undo, a property write) emits through the backend instead, so each edit is
  announced once, by whichever half performed it. `smoke.sh` asserts both lines.
- **The fetched mod packs reach the interpreter.** `?mods=1` walks `www/mods/index.json`
  after the first frame. The index is the packer's; each pack is handed to the script
  half; and the status line ends with a summary. Which scripts *succeed* depends on the
  facade's contract, so the smoke check asserts delivery (the index parsed, a named pack
  loaded, a per-pack report line), not a pass count:

  ```
  [INFO] ferrocad_web_gpui::mods: ferrocad: mods: 6 pack(s) from http://127.0.0.1:8099/mods/index.json
  [INFO] ferrocad_web_gpui::mods: ferrocad: mod GridOverlay: 1 script(s) loaded, 1 failed
  [WARN] ferrocad_shell_core::shell: ferrocad: workbench GridOverlay InitGui.py failed:
  ImportError: no os specific module found
  [INFO] ferrocad_web_gpui::mods: ferrocad: mods: 6 loaded, 6 failed
  ```
- No panic. Startup logs two
  `Scissor Rect … is not contained in the render target` validation errors from
  `gpui_wgpu` while the DOM canvas is still being sized (and the resize poll timeout
  above); gpui retries and the frame after they clear is correct. They are the
  window's bootstrap, not the viewport: the surface is briefly smaller than the first
  frame.

## What it does not have (yet)

- **Expressions in the UI.** The document supports them; the browser has no editor for
  them. `evaluate` reports that there is no interpreter rather than pretending.
- **STEP/IGES/STL.** The prebuilt wasm kernel omits the exchange toolkits. `.FCStd`
  itself round-trips (see above).
- **A gizmo *drag* that writes back.** Typed Move, Rotate, Hide/Show, undo/redo and
  Fit are wired; a pointer drag on the transform gizmo reaches the same
  `apply_interaction` hook but the headless capture drives it with zero deltas, so it
  is unverified there.
- **Interactive Push/Pull and edge dress-up.** Wired. The web host links
  `ferrocad_part_gui` and installs its two factories beside
  `ferrocad_shell_core::base_contributions()` — the same handles the Parametric edition
  contributes — and the document half applies the commits itself
  (`RustAppBackend::apply_interaction`), so no interpreter is involved on either side.
  Both halves of the interaction are checked in this host: `?manipulator=` sends the
  commit a handle would release, and `?gesture=` drives the handle itself — equip the
  hand, select the edge or face, press where the manipulator drew and scrub — through
  the same `on_selection`/`drag_begin`/`drag_update`/`drag_end`/`on_interaction` path a
  pointer takes. Only the device events are synthetic.
- **`WP`.** The workplane command reports that none is wired (see
  [workplanes.md](../../docs/project/tasks/workplanes.md)); `Fit` is wired.
- **Python workbenches.** `?pywasm=1` installs RustPython, so the Console evaluates
  Python against the real `FreeCAD.ActiveDocument` and the shell script's property
  rows are real. The shipped packs are fetched and run (`?mods=1`, the default): each
  `Init.py` loads, and the provider settings clients' `InitGui.py` runs — they are the
  mods the facade is complete enough for, and `tests/mods.rs` pins them natively. The
  Coin/shader skins read a resource file and need the overlay bridge, so those still
  record an error, shown in `Settings/About` and the status line and never fatal. The
  facade is read-only, though (no `setattr`, no recompute, no `Shape`), and there are no
  `.ui` task panels and no `pivy`.
  RustPython's compiler recurses, so `build.rs` sets the module's stack to 16 MiB
  (`wasm-ld`'s default is 1 MiB); if `?pywasm=1` traps on boot, that is the knob.
- **Multiple windows.** The browser backend has one window.
- **Hardware (G-buffer) picking.** It reads a texel back with a blocking `poll`, which
  cannot complete on the web, so the browser uses the viewport's CPU ray cast instead
  (same queue and camera, no GPU readback).
