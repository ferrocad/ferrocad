# ferrocad_web_gpui

FerroCAD's **browser shell**: the real `ferrocad_shell_core` UI — leading bar,
viewport header, status line, model inspector, property editor and console — in a
GPUI window on WebGL2, with a Rust `AppBackend` and no CPython anywhere in the
dependency graph.

This crate used to be the step-2 probe (a `Part::Box` composited through
`gpui_interop`). The shell split (M0) and the wasm build (M1) made it a host; M2
replaced the probe's demo with the shell, and M3 returned the 3D viewport through the
shell's own `InventorViewport` element. The plan and the verified probe baseline are in
[`docs/project/tasks/wasm-shell.md`](../../docs/project/tasks/wasm-shell.md).

## What it proves

1. **The same shell crate as the desktop app.** `ferrocad_shell_core` compiles for
   `wasm32-unknown-unknown` and opens its window through `gpui`'s web backend. There
   is no second implementation of the chrome, panels or key handling.
2. **The seam, not a stub.** `SampleBackend` implements `ferrocad_shell_api`'s
   `AppBackend`. The shell asks it for the tree, properties, commands and stage exactly
   as it asks the Python backend natively: clicking `Box` in the tree (or on the shape
   in the viewport) fills the property pane and the status line;
   `Grid`/`Axes`/`Gizmo`/`Snap` toggle through a `ViewRequest` and report their new
   state on the next command refresh.
3. **The live 3D viewport.** `Shell::enable_viewport` builds the real
   `InventorViewport`; on the web its renderer is the zero-copy presenter, which takes
   the window's own wgpu device on the first paint and composites the frame with
   `paint_texture` (`SAMPLES = 1`, the surface's own colour format). Orbit, wheel zoom,
   the navigation cube, the axis cross and picking all run in the browser. There is no
   readback image path: it needs a blocking `poll`, which cannot complete here.
4. **One window, panels in-window.** The web backend rejects `Floating`/`Dialog`, so
   the inspector column (`panels_sidebar`) sits beside the canvas instead of in Dev
   Tools: Model, Properties and Console are stacked and all visible at once.
5. **Text works.** GPUI's web platform starts with an empty font database and panics
   when it cannot resolve a font. The shell registers a bundled DejaVu Sans
   (`ferrocad_shell_core::fonts`), whose family is already in GPUI's fallback stack,
   before opening the window.

The document is an in-memory sample (`Sample`: `Box`, `Box001`). Its stage is
tessellated in Rust — there is no OCCT kernel in this build — but it travels the same
`StageMesh` → `DisplaySync` → `Stage` path a `Part::Box` would, so picking, the entity
map and the gizmo behave as they do natively.

## Run it

This build needs no OCCT, so any toolchain with the wasm target works — no cross
image:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release -p ferrocad_web_gpui --target wasm32-unknown-unknown

# `--out-dir` is gitignored; the CLI version must match `wasm-bindgen` in Cargo.lock.
wasm-bindgen --target web --out-dir crates/ferrocad_web_gpui/www/pkg \
    target/wasm32-unknown-unknown/release/ferrocad_web_gpui.wasm

cd crates/ferrocad_web_gpui/www && python3 -m http.server 8081
```

Then open <http://localhost:8081>. The module is ~17 MB (GPUI + wgpu, no kernel);
the first load takes a moment.

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
[WARN] gpui_wgpu::wgpu_renderer: Failed to poll device during resize: Timeout
```

- The window shows the leading bar (`☰`), the viewport header
  (`Grid / Axes / WP / Gizmo / Select: Body / Snap`), the live 3D viewport (two boxes,
  the axis cross and the navigation cube), the in-window `Model` / `Properties` /
  `Console` column, the loadout bar and the status line (`2 object(s) · Sample`,
  `FreeCAD 0.1.5 · rust`).
- The console's first line is `FerroCAD browser shell · Rust backend (no CPython)`:
  the Rust backend really is the one answering.
- `?click=1004,100` selects `Box` in the tree: the row highlights, `Properties` shows
  `Label`, `Length`, `Width`, `Height`, and the status line reads `selected Box`.
- `?click=430,380` clicks the box **in the viewport**: same selection and properties,
  plus the transform gizmo and its move HUD — the CPU pick ran against the stage the
  presenter drew.
- `?drag=1` orbits (75 434 pixels differ from the base capture) and `?wheel=1` zooms
  (42 132) — the camera changes prove the presenter re-composites each frame.
- `?click=40,58` runs `App_ToggleGrid`: the header dot turns accent, the console logs
  `grid on`, and the status line shows `grid on` (the view request round-trips
  through the shell and back through `set_view_state`).
- No panic. Startup logs two
  `Scissor Rect … is not contained in the render target` validation errors from
  `gpui_wgpu` while the DOM canvas is still being sized (and one resize poll timeout);
  gpui retries and the frame after they clear is correct. They are the window's
  bootstrap, not the viewport: the surface is briefly smaller than the first frame.

## What it does not have (yet)

- **A real document.** M4: `ferrocad_core::Document` + `ferrocad_part` +
  `OcctBackend` behind the same `AppBackend` (and the cross image with the prebuilt
  wasm kernel).
- **Editing a document.** M5: property writes recompute and refresh the stage.
- **Python.** No interpreter, no workbench scripts, no `FreeCAD` facade, no `.ui`
  task panels. `evaluate` reports that honestly.
- **File I/O and multiple windows.** Browser file input / one window only.
- **Hardware (G-buffer) picking.** It reads a texel back with a blocking `poll`, which
  cannot complete on the web, so the browser uses the viewport's CPU ray cast instead
  (same queue and camera, no GPU readback).
