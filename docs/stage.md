# The modern stack: `ferrocad_stage` — plan and status

Status: **T0 (crate + declarative spine), T1 (render + coexistence) and T1.5 (the
viewport bridge) built (2026-10-12); T2–T5 planned.**
Companion to [`inventor-renderer.md`](inventor-renderer.md) (the legacy `pivy.coin`
stack this runs beside), [`inventor-plan.md`](inventor-plan.md) (the emulation roadmap),
and [`architecture.md`](architecture.md) (crate layering).

## 1. Why

FreeCAD's workbench visuals reach the screen through `pivy.coin`: a workbench builds an
imperative `SoSeparator`/`SoCoordinate3`/`SoMaterial` tree and Coin draws it. FerroCAD
keeps that path working (`ferrocad_inventor_py` + `ferrocad_inventor`), but it is a poor
fit for *new* workbenches: imperative scene-graph assembly, `SoEventCallback`-style event
handling, and `SoDragger`-style gizmos, all mediated by a retained node tree.

`ferrocad_stage` is the modern alternative: a **declarative** CAD presentation layer. A
workbench declares high-level items (a shape with tessellation and display attributes, and
— later — tools and gizmos); the stage compiles them **directly** to draw batches, with
no intermediate scene graph, no traversal, and no node arena.

## 2. Two producers, one batch contract

Both stacks emit the **same** batch vocabulary
([`DrawPacket`](../crates/ferrocad_inventor/src/batch.rs)), which is the whole point: a
renderer draws them in one pass against one depth buffer, so there are no layering
artifacts and one pick path serves both.

```text
legacy:  Scene ──compile──────────▶ RenderQueue ─┐
                                                 ├──▶ WgpuRenderer (one pass, one depth buffer)
modern:  Stage ──emit_batches─────▶ ─────────────┘
```

The stage does **not** depend on the Inventor *semantics* (no `Scene`, no traversal); it
depends on `ferrocad_inventor` only for the shared batch types, which are already the
neutral, renderer-facing seam. If a second consumer ever justifies it, the vocabulary
moves to a neutral `ferrocad_batch` crate (see §5).

## 3. Slice path

| Slice | Deliverable | Status |
| --- | --- | --- |
| **T0 — crate + declarative spine** | `ferrocad_stage`: a `Stage` of items and a `ShapeItem` (positions/normals/triangles/edges + `DrawMode`/colours/`entity_id`), with `Stage::emit_batches(&mut RenderQueue)`. No pyo3, no wgpu, no scene graph. | ✅ |
| **T1 — render + coexistence** | Draw a stage beside a compiled inventor queue in one pass; headless readback asserts both rasterize and that frame 2 reuses the upload cache. | ✅ |
| **T1.5 — viewport bridge** | `InventorViewport` holds a `Stage` (`stage_mut`/`set_stage`); its queue is the inventor compile plus `stage.emit_batches`, so the shell renders both producers. | ✅ |
| **T2 — tools** | `Tool`/`PointerEvent`, transient items (rubber-band, snap marker), `snap_to_geometry`, host dispatch. | ▢ |
| **T3 — `TransformGizmo`** | Draggers with drag/commit delta events; transient transform override. | ▢ |
| **T4 — G-buffer pick pass** | The offscreen `Rg32Uint`/normal/depth pass + 1×1 readback + `part_index` unprojection — the shared enabler for O(1) picking in both stacks. **This is where the batch contract is extended and (likely) extracted.** | ▢ |
| **T5 — `ferrocad.stage` bindings** | PyO3 module exposing `ShapeItem`/`Tool`/`TransformGizmo`. | ▢ |

## 4. As built (T0–T1)

### T0 — declarative spine

`crates/ferrocad_stage` (pure Rust; depends on `ferrocad_inventor` + `ferrocad_types`):

- **`ShapeItem`** — a tessellated CAD shape: `positions`, optional per-vertex `normals`
  (indexed when supplied, flat-shaded per triangle otherwise), `triangles`, `edges`, plus
  the display attributes `DrawMode` (`Shaded` / `Wireframe` / `ShadedWithEdges`),
  `surface_color`, `edge_color`, `edge_width`, `selectable`, and a `Placement`
  (the world transform). Constructed with `ShapeItem::new(entity_id)` + `with_*`
  builders; geometry replaced with `set_shape(...)`.
- **`Stage`** — an ordered list of items; `emit_batches(&mut RenderQueue)` appends, so a
  caller can merge a stage with a compiled inventor queue in one pass (`to_queue()` makes
  a fresh queue).
- **Emission** — a `ShadedWithEdges` item decomposes into a `Topology::Triangles` packet
  (surface colour) and a `Topology::Lines` packet (edge colour). Each packet binds the
  item's `Placement` and its `PickId` (the `entity_id`, or `PickId(0)` when not
  selectable). `depth_bias` is `0` and `texture` is `None` for now.
- **Mesh identity** — each item allocates two stable `MeshId`s at construction (surface
  and wire) from a process-global counter based at `1 << 40`, so stage ids never collide
  with the inventor compiler's per-frame `0..n` ids in the renderer's shared
  `(MeshId, Generation)` upload cache. `set_shape` bumps the `Generation` to force a
  re-upload; an unchanged item keeps its key and the upload is reused.

**Verification:** `ferrocad_stage` unit tests (12) cover the three draw modes, indexed vs.
flat-shaded surfaces, stable/unique/high mesh ids, generation bumps, non-selectable pick
ids, empty geometry, placement → world transform, and `emit_batches` appending.

### T1 — render + coexistence

Both producers emit the same vocabulary, so they draw together with no new machinery:
`let mut queue = compile_with(&scene, &viewport); stage.emit_batches(&mut queue);`, then
one `draw_frame` (the renderer's `InventorView` impl — what `ferrocad_viewport` calls).

`ferrocad_wgpu`'s `a_stage_and_an_inventor_scene_coexist_in_one_pass` (dev-dependency on
`ferrocad_stage`, the renderer's own test module) proves it end to end on a real GPU
(skipping without an adapter):

- a legacy inventor red surface, a modern blue `Shaded` surface, and a modern green
  `Wireframe` triangle are merged into one queue and drawn in one pass;
- the readback shows each producer's object rasterized in its own region (red-dominant
  left, blue-dominant right) — no layering artifacts;
- **frame 2 rebuilds the same queue and adds nothing to the upload cache** (the mesh ids
  are stable across compiles), i.e. every unchanged item is a cache hit.

The two producers are spatially separated; the coplanar-edge case is deliberately left
to T4 (see §5.4).

### T1.5 — viewport bridge

`InventorViewport` now holds a `Stage` (empty by default) and compiles it into the same
queue as the inventor scene: `recompile` is `compile_with(scene)`, then
`stage.emit_batches(&mut queue)`. The accessors are `stage()`, `stage_mut()` and
`set_stage()`; a mutation marks the queue stale and the next draw (`refresh`) rebuilds
it — so an event handler that edits the stage and calls `cx.notify()` gets it rendered
on the next frame. `stage_mut`/`set_stage` are additive: existing callers are unchanged.

This closes the app-level loop: the shell's 3D pane can now draw legacy `pivy.coin`
shapes and modern stage items together, and T2's transient tool items (rubber-bands, snap
markers) have a place to live without a combined render+dispatch refactor.

**Verification:** `ferrocad_viewport`'s `a_stage_item_is_merged_into_the_viewport_queue`
adds a `ShapeItem` via `stage_mut()`, asserts the rebuilt queue gained exactly one packet
carrying its `PickId`, and that `set_stage(Stage::new())` drops it again.

## 5. Deferred decisions

1. **Batch vocabulary home.** Kept in `ferrocad_inventor` for T0/T1 (`DrawPacket` is
   already the neutral seam); extract a neutral `ferrocad_batch` crate at T4, the moment
   the contract must change for the G-buffer pass. Doing it then is one motivated
   refactor instead of two.
2. **Contract extension.** `pipeline_kind`, `entity_id`, and interleaved `part_idx` are
   deliberately **not** added yet: `ShadedWithEdges` already decomposes into existing
   `Topology` packets, and surface/wireframe/overlay is already expressible. Add them at
   T4, where the G-buffer pass gives them a consumer.
3. **Math crates.** Stay on `ferrocad_types` (CAD-grade f64 `Matrix4`/`Placement`/
   `Rotation`); do **not** introduce `glam` alongside it. The renderer converts f64→f32
   at the WGSL boundary.
4. **Coplanar CAD edges.** The current scene pipelines use `DepthBiasState::default()`
   (zero bias), so an edge coplanar with its shaded surface will z-fight. T1 therefore
   verified **spatially separated** `Shaded`/`Wireframe` items (see §4); the slope/constant
   depth bias is T4 work.

## 6. Naming reconciliation

The architecture sketch named some types that this build expresses with the existing
vocabulary:

| Sketch | As built |
| --- | --- |
| `DrawBatchDescriptor` | `ferrocad_inventor::DrawPacket` (+ the `DrawBatch` trait) |
| `MeshCacheKey` | `(MeshId, Generation)` |
| `BatchPipelineKind` | the pipeline is chosen from `Topology` + the view's `overlay` flag (extended at T4) |
| `entity_id` | `PickId` (a `u32`) |
| interleaved `[pos, norm, part_idx]` | `Geometry { Vec<Vertex>, Vec<u32> }`, `Vertex { pos, norm, uv }` |
| `glam::Mat4`/`Vec4` | `ferrocad_types::Matrix4` / `[f32; 4]` |
