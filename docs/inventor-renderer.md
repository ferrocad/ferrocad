# The Inventor renderer: scene core, wgpu backend, and the gpui viewport

Status: milestone record (2026-10-12). Companion to
[`rewrite-strategy.md`](rewrite-strategy.md) (pillars 3–4), the
[`coin-bridge-reuse-assessment.md`](coin-bridge-reuse-assessment.md) evidence, and
[`architecture.md`](architecture.md) (dependency rules).

This note records the first working slice of the **Coin/Inventor replacement**:
a renderer-agnostic scene core, a wgpu backend that draws it, offscreen readback,
and a gpui viewport wired into the app shell — exercised end to end by an example.

## 1. Pieces and ownership

| Crate | Role | Depends on |
| --- | --- | --- |
| `ferrocad_inventor` | The **scene core**: node AST, traversal/state accumulator, mesh compiler, picking, events, viewport/DPI. Renderer-agnostic. A **leaf** above `ferrocad_types`. | `ferrocad_types` |
| `ferrocad_wgpu` | The **backend**: consumes compiled batches, WGSL pipelines, dynamic uniforms, MSAA, offscreen target, readback. | `ferrocad_inventor`, `ferrocad_types`, `wgpu`, `bytemuck` |
| `ferrocad_inventor_py` | The **`pivy.coin` facade** (PyO3): the class spine, type registry, geometry field objects, and the `install`/`extract_scene` bridge (slices S1–S3). | `ferrocad_inventor`, `ferrocad_types`, `pyo3` |
| `ferrocad_gpui` | The **host/glue**: `InventorViewport` (element + interaction loop), `scene` (the Rust sample), `python::inventor_scene` (the `pivy.coin` scene builder), the shell wiring. | `ferrocad_inventor`, `ferrocad_inventor_py`, `ferrocad_wgpu`, `ferrocad_types`, `bite-gpui`, `pyo3` |

The split follows the assessment's seam: the glossary of scene semantics stays in
`ferrocad_inventor`; the graphics API stays behind a trait; the host owns the
window, the scale factor and the event loop.

```mermaid
flowchart LR
    S["Scene"] --> C["compile"]
    C --> RQ["RenderQueue"]
    RQ -. "trait DrawBatch" .-> RW["WgpuRenderer"]
    RW --> IMG["ImageData / wgpu::Texture"]
    IMG --> VP["InventorViewport (gpui)"]
    VP --> W["window (window_frame)"]
```

## 2. The batch boundary

`ferrocad_inventor::DrawBatch` is the trait a backend consumes; `DrawPacket` is
the concrete packet the compiler emits. A packet carries:

- `mesh_id` + `generation` — the renderer keys its upload cache on the pair, so a
  stable scene uploads once and reuses;
- `topology` (`Triangles`/`Lines`/`Points`);
- `geometry: Option<Geometry>` — **present only on a cache miss**;
- a `StateSnapshot` — `world_transform` (a `Matrix4`), `color`, `depth_bias`,
  `texture` (an optional `Texture2`), `id` (the `PickId`).

Keeping the boundary a trait (not a struct) is what lets a future D3D/Metal
backend sit behind it without touching the node model or compiler.

## 3. Picking and events

- **Picking** is a CPU ray cast (`ferrocad_inventor::pick`): unproject a device
  position through the camera (`Camera::ray_through`, using a local 4×4 inverse)
  and intersect Möller–Trumbore against the world-space triangles. This is the
  `SoRayPickAction` analogue and needs no GPU. `pick_hit`/`pick_at_hit` return a
  [`Hit`](../crates/ferrocad_inventor/src/pick.rs) (object id, world point, surface
  normal, triangle index) — what `SoPickedPoint`/`SoFaceDetail` expose to Python. A
  g-buffer pick in `ferrocad_wgpu` remains the stub for later.
- **Events** are `ferrocad_inventor::Event` (`MouseMove`/`MouseButton`/`MouseLeave`)
  delivered to an `EventListener` (`on_event(event, hit, scene)`) — the
  `SoEventCallback` analogue. The listener may edit the scene and reports whether it
  did; the viewport recompiles when it did. Hover highlight is implemented exactly
  this way (recolour the object's `Material`). **Scripted** callbacks are the other
  half (S4): an `SoEventCallback` node sits in the AST, the host finds it via
  `event_callbacks`, and `ferrocad_inventor_py::dispatch_event` delivers the event and
  pick to the registered Python callables (`event_cb.getEvent()`/`getPickedPoint()`).

## 4. HiDPI: everything is physical pixels

The contract, matching FreeCAD (see the assessment and `InputDevice`/
`View3DInventorViewer::toQPoint`):

- **Device positions are bottom-left origin** (Coin); **logical positions are
  top-left origin** (Qt/gpui); conversions flip Y.
- `ferrocad_inventor::Viewport` holds the **device** size + `device_pixel_ratio`
  and provides `to_device`/`to_logical` (the FreeCAD `toDevicePixelPosition`/
  `toQPoint` pair). Screen-space nodes read `State.device_pixel_ratio` (the
  `SoDevicePixelRatioElement` analogue).
- **Conversion lives only at the host** (the gpui boundary). Everything below —
  the renderer, the camera, the pick buffer, and the `pivy.coin` facade — speaks
  device pixels. The facade performs **no** conversion; it exposes physical event
  positions to Python, exactly as pivy does under FreeCAD (verified: workbench
  code such as `Sketcher`'s `viewport_to_qpoint` divides by the ratio to reach a
  Qt logical point).
- The render target is sized in physical pixels; MSAA (4× by default) resolves
  into the single-sampled target that is read back or shared.

## 5. What is implemented vs not

**Implemented**

- Node AST (`Separator`/`Switch`/`Coordinate3`/`Normal`/`Material`/`Transform`/`Mesh`),
  Coin-style field scoping (fields affect following siblings; `Separator`
  isolates; `SoSwitch` is group-like and honours `whichChild`), flat-shaded mesh
  compilation to packets.
- `SoType`-style registry (`TypeRegistry`) and the `pivy.coin` facade: the PyO3
  class spine (`isinstance`), `SoType.fromName(...).createInstance()`, and the
  geometry field objects (`SoCoordinate3.point`, `SoMaterial.diffuseColor`, …).
- The **scene bridge** (S3): `ferrocad_inventor_py::install` registers `pivy.coin`
  in the embedded interpreter; a `from pivy import coin` script builds a tree that
  `extract_scene` copies into a `Scene` — the same `compile` path the Rust sample
  uses.
- The **event bridge** (S4): the `SoEvent`/`SoButtonEvent`/`SoMouseButtonEvent`/
  `SoLocation2Event` classes, `SoPickedPoint`/`SoDetail`/`SoFaceDetail`/`SoLineDetail`,
  and the `SoEventCallback` node; the host routes pointer moves and button down/up
  through `python::PythonEventCallbacks` → `dispatch_event`.
- wgpu renderer: triangle pipeline with depth, dynamic-offset uniforms, 4× MSAA,
  offscreen target, RGBA readback, mesh upload cache.
- CPU ray picking; event/listener dispatch; `Viewport`/DPI primitives.
- gpui `InventorViewport`: image path (readback → `RenderImage` → `img`), pointer
  events → pick → listener → recompile, `with_listener`.
- Wired into the app shell: `Shell::enable_viewport` (best-effort, needs a GPU)
  shows a scene built from a `pivy.coin` script (`python::inventor_scene`, with the
  Rust `scene::sample_scene` as fallback); the `inventor_roundtrip` example renders
  both windowed and headless (`--headless out.png`).
- **Overlays / multi-view** (the navigation cube, `docs/navicube.md`):
  `ferrocad_inventor::ViewSpec` + `InventorView::draw_frame`,
  `WgpuRenderer::render_views` (per-view `set_viewport`/`set_scissor_rect`, an overlay
  pipeline with depth off and alpha blending), `ferrocad_inventor::navicube` (the
  cube scene + ortho camera), and `ferrocad_gpui::navicube::NaviCube` +
  `InventorViewport::with_navicube` (placement, camera mirroring, hover, click-snap,
  drag-orbit, the animated snap). The cube carries baked face labels and a fixed
  `Home` button (N4b; see [`navicube.md`](navicube.md)).
- **Line and point pipelines**, routed by `Topology` (scene and overlay variants), so
  `Topology::Lines`/`Points` packets draw correctly; **line (segment) picking**
  (`Hit` carries its `Topology` + `element`, and the facade builds a `SoLineDetail`
  for a line hit). The navigation cube's edges are lines and its corners small cubes.
- A **texture path** (`SoTexture2` analogue): `NodeKind::Texture2` +
  `TextureCoordinate2`, a vertex `uv`, `State.texture` (an `Arc<Texture2>`, RGBA8 with
  a stable id), and a wgpu bind group (group 1) sampling the shape texture (a 1×1
  white default when absent). The navigation cube's face **labels** and **Home**
  button bake glyphs into `Texture2`s.

**Not implemented**

- The g-buffer pick pass (`ferrocad_wgpu::pick` is a stub); a **transparent** (order-
  independent) pipeline; a **text/glyph-run** node (labels exist, but as baked
  textures, not text nodes).
- The `pivy.coin` facade is deliberately narrow: no arena/object identity, no
  `SoPath`/`SoSearchAction`, no NumPy zero-copy or generational dirty counters, and
  no `-1` polygon delimiters (see [`inventor-plan.md`](inventor-plan.md) §8).
- Real document geometry: the MVP engine is geometry-free, so the viewport draws
  the sample scene until the kernel and workbench `ViewProvider`s produce scene
  nodes.
- Layout-driven sizing: the target is a fixed size stretched into the pane.

## 6. Verification

- `cargo test -p ferrocad_inventor` — compiler, scoping, registry, `Viewport`
  conversions, ray pick, event/listener.
- `cargo test -p ferrocad_wgpu` — offscreen render + readback (skips without a GPU).
- `cargo test -p ferrocad_inventor_py` — the facade spine, `SoType`, the field
  objects, the S3 scene-extraction bridge, and the S4 event/dispatch surface
  (`tests/coin_boot.rs`, `coin_fields.rs`, `coin_scene.rs`, `coin_events.rs`).
- `cargo test -p ferrocad_gpui --lib` — viewport wiring, `RenderImage` swizzle,
  pointer→pick→listener, the `pivy.coin` scene builder, the scripted
  `SoEventCallback` dispatch, and a headless shell paint that attaches the viewport
  when a GPU exists.
- `cargo run -p ferrocad_gpui --example inventor_roundtrip -- --headless out.png`
  — `pick at centre -> Some(PickId(0))`, writes the (highlighted) frame.

## 7. Next

The full Open-Inventor-emulation roadmap (four phases — AST/type spine, fields,
traversal/compile, picking/events — adapted to the code as it stands) is in
[`inventor-plan.md`](inventor-plan.md). In short:

1. **Layout-driven sizing**: an `Element` that reads its bounds ×
   `window.scale_factor()` to size the target and place the camera, making hover
   coordinates exact in a docked pane (and removing the fixed-size stretch).
2. **Navigation-cube polish** (N4b, remaining): a context menu and the arrow buttons
   (see [`navicube.md`](navicube.md)). The pieces, the overlay pass, drag-orbit, the
   animated snap, translucent away-facing faces, baked face labels and the `Home`
   button are built.
3. **The g-buffer pick** and line/point pipelines.
4. **Grow the `pivy.coin` facade** and the `TypeRegistry` toward the ~63
   registered node/kit classes and the `SoType.fromName` surface the workbenches
   use. S1–S4 cover the class spine, the geometry fields, the scene-extraction
   bridge, and the event/pick surface; the arena (object identity, detached fields),
   triangulation, field-driven generations, and the g-buffer pick are the next
   depth items.
