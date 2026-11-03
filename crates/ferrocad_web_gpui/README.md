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

The document links the prebuilt wasm OCCT kernel, so this build needs the cross image
(the same one `crates/ferrocad_web` uses: the C++ bindings and the kernel must agree on
the wasm-EH encoding). The kernel tarball is `occt-8_0_1_rev3-wasm32_unknown_unknown`;
`crates/ferrocad_web/run.sh` fetches it, builds the missing `TKFeat` archive, and is a
good reference for the whole recipe.

```sh
IMAGE=ghcr.io/lzpel/cross-wasm32-unknown-unknown@sha256:4dc88df287da141e67588ba80ebf1d14d5cb24f5b05302c57b11b2a1e9bed1df

docker run --rm -v "$PWD":/work -w /work \
    -v "$HOME/.cargo/registry":/cargo-home/registry:ro \
    -v "$HOME/.cargo/git":/cargo-home/git:ro \
    -v /tmp/ferrocad-wasm-target:/target \
    -e OCCT_WASM_ROOT=/work/.occt-wasm/occt-8_0_1_rev3-wasm32_unknown_unknown \
    -e CARGO_HOME=/cargo-home -e CARGO_TARGET_DIR=/target \
    "$IMAGE" \
    cargo build --release -p ferrocad_web_gpui --target wasm32-unknown-unknown --locked

# `--out-dir` is gitignored; the CLI version must match `wasm-bindgen` in Cargo.lock.
wasm-bindgen --target web --out-dir crates/ferrocad_web_gpui/www/pkg \
    /tmp/ferrocad-wasm-target/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm

cd crates/ferrocad_web_gpui/www && python3 -m http.server 8081
```

The two read-only cargo mounts are only there so the container needs no network; any
cargo home with the lock's crates works. Then open <http://localhost:8081>. The module
is ~37 MB (GPUI + wgpu + OCCT); the first load takes a moment.

`www/index.html` also drives the shell for headless captures:

| Query | Effect |
| --- | --- |
| `?click=x,y` | one pointer press/release at a client coordinate (select a tree row, run a header command, click a shape) |
| `?drag=1` | one pointer drag across the canvas (orbits the camera) |
| `?wheel=1` | one scroll notch (zooms) |
| `?dblclick=1` | a drag then two quick clicks (the viewport's double-click) |

## Verified

Headless Chrome, WebGL2 through ANGLE/SwiftShader
(`--use-angle=swiftshader --enable-unsafe-swiftshader`), 1280×800:

```
[INFO] gpui_wgpu::wgpu_context: Browser graphics initialized: requested=WebGl, selected=Gl, adapter="ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device ...))"
[INFO] ferrocad_shell_core::fonts: ferrocad: bundled UI font (DejaVu Sans) registered
[INFO] ferrocad_web_gpui::document: ferrocad: Box · 12 triangles, 12 edges
[INFO] ferrocad_web_gpui::document: ferrocad: Fillet · 40 triangles, 21 edges
[WARN] gpui_wgpu::wgpu_renderer: Failed to poll device during resize: Timeout
```

- The window shows the leading bar (`☰`), the viewport header
  (`Grid / Axes / WP / Gizmo / Select: Body / Snap`), the live 3D viewport (the box
  and its fillet, the axis cross and the navigation cube), the in-window `Model` /
  `Properties` / `Console` column, the loadout bar and the status line
  (`2 object(s) · Sample`, `FreeCAD 0.1.5 · rust`).
- The console greets with `FerroCAD browser shell · Rust document backend · OCCT wasm
  · no CPython`: the Rust document really is the one answering.
- The tree lists `Box` (`Part::Box`) and `Fillet` (`Part::Fillet`). `?click=950,131`
  selects the fillet and the pane shows its real rows: `Label`, `Base`
  (`Box: Edge1`), `Edges`, `Propagate` (a toggle), `Radius` (`3.0 mm`, editable) and
  `Shape` (`<shape>`, `Output`).
- `?click=430,380` clicks the box **in the viewport**: same selection and properties,
  plus the transform gizmo and its move HUD — the CPU pick ran against the stage the
  presenter drew.
- `?drag=1` orbits and `?wheel=1` zooms the camera; the presenter re-composites each
  frame. `?click=40,58` runs `App_ToggleGrid` (dot, status and console agree).
- No panic. Startup logs two
  `Scissor Rect … is not contained in the render target` validation errors from
  `gpui_wgpu` while the DOM canvas is still being sized (and the resize poll timeout
  above); gpui retries and the frame after they clear is correct. They are the
  window's bootstrap, not the viewport: the surface is briefly smaller than the first
  frame.

## What it does not have (yet)

- **Undo/redo, expressions and file I/O in the UI.** The document supports them; the
  browser has no menu for them and no file input. `evaluate` reports that there is no
  interpreter rather than pretending.
- **Python.** No interpreter, no workbench scripts, no `FreeCAD` facade, no `.ui`
  task panels.
- **Gizmo edits.** The transform gizmo draws and picks, but a drag does not yet write
  `Placement` back to the document (M5).
- **Multiple windows.** The browser backend has one window.
- **Hardware (G-buffer) picking.** It reads a texel back with a blocking `poll`, which
  cannot complete on the web, so the browser uses the viewport's CPU ray cast instead
  (same queue and camera, no GPU readback).
