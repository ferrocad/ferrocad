# ferrocad_web_gpui

FerroCAD's **browser shell**: the real `ferrocad_shell_core` UI — leading bar,
viewport header, status line, model inspector, property editor and console — in a
GPUI window on WebGL2, over a real `ferrocad_core::Document` whose geometry is built
by the OCCT kernel compiled to wasm, with no CPython anywhere in the dependency
graph.

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
2. **The seam, not a stub.** `DocumentBackend` implements `ferrocad_shell_api`'s
   `AppBackend` over a real document. The shell asks it for the tree, the property
   rows, the commands and the stage exactly as it asks the Python backend natively:
   clicking a shape (or a tree row) fills the property pane and the status line;
   `Grid`/`Axes`/`Gizmo`/`Snap` toggle through a `ViewRequest` and report their new
   state on the next command refresh.
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
6. **Text works.** GPUI's web platform starts with an empty font database and panics
   when it cannot resolve a font. The shell registers a bundled DejaVu Sans
   (`ferrocad_shell_core::fonts`), whose family is already in GPUI's fallback stack,
   before opening the window.

## Run it

```sh
crates/ferrocad_web_gpui/run.sh
```

Then open <http://localhost:8081> (the script opens it for you unless `--no-open`).
The module is ~37 MB (GPUI + wgpu + OCCT); the first load takes a moment.

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
4. generates `www/pkg/` and serves `www/` with `python3 -m http.server`.

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
- The console greets with `FerroCAD browser shell · Rust document backend · OCCT wasm
  · no CPython`: the Rust document really is the one answering.
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
  runs `App_Move`, accepts `browser-move-2 {"dx":15.0,…}`, prints `moved 1 object(s)`
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
  the page bridge (`open requested`) — the picker, the download and drag-and-drop are
  the page's half.
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
- **`WP`.** The workplane command reports that none is wired (see
  [workplanes.md](../../docs/project/tasks/workplanes.md)); `Fit` is wired.
- **Python.** No interpreter, no workbench scripts, no `FreeCAD` facade, no `.ui`
  task panels.
- **Multiple windows.** The browser backend has one window.
- **Hardware (G-buffer) picking.** It reads a texel back with a blocking `poll`, which
  cannot complete on the web, so the browser uses the viewport's CPU ray cast instead
  (same queue and camera, no GPU readback).
