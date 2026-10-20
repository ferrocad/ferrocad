# The modern stack: `ferrocad_stage` — plan and status

Status: **T0–T5 built (2026-10-12); F0.4 adds the topology view (2026-10-20).** T4's
four sub-slices are done (`ferrocad_graphics` extracted, surface depth bias, mesh-cache
generational eviction, and the offscreen G-buffer pick pass with vertex `part_idx`), and
T5 exposes the whole modern stack to Python as `ferrocad.stage` (`ferrocad_stage_py`).
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
([`DrawPacket`](../crates/ferrocad_graphics/src/batch.rs)), which is the whole point: a
renderer draws them in one pass against one depth buffer, so there are no layering
artifacts and one pick path serves both.

```text
legacy:  Scene ──compile──────────▶ RenderQueue ─┐
                                                 ├──▶ WgpuRenderer (one pass, one depth buffer)
modern:  Stage ──emit_batches─────▶ ─────────────┘
```

The stage does **not** depend on Open Inventor at all: it compiles to
[`ferrocad_graphics`](../crates/ferrocad_graphics), the neutral draw contract (the batch
vocabulary, camera and picking) shared with the legacy stack (T4, sub-slice 1).

## 3. Slice path

| Slice | Deliverable | Status |
| --- | --- | --- |
| **T0 — crate + declarative spine** | `ferrocad_stage`: a `Stage` of items and a `ShapeItem` (positions/normals/triangles/edges + `DrawMode`/colours/`entity_id`), with `Stage::emit_batches(&mut RenderQueue)`. No pyo3, no wgpu, no scene graph. | ✅ |
| **T1 — render + coexistence** | Draw a stage beside a compiled inventor queue in one pass; headless readback asserts both rasterize and that frame 2 reuses the upload cache. | ✅ |
| **T1.5 — viewport bridge** | `InventorViewport` holds a `Stage` (`stage_mut`/`set_stage`); its queue is the inventor compile plus `stage.emit_batches`, so the shell renders both producers. | ✅ |
| **T2 — tools** | `Tool`/`PointerEvent`/`MouseButton`, snapping (`snap_to_geometry`), transient items (`PolylineTool`), and host dispatch from the viewport (drawn as a transient overlay, never picked). | ✅ |
| **T3 — `TransformGizmo`** | Draggers with drag/commit delta events; transient transform override. | ✅ |
| **T4 — G-buffer pick pass** | *(1/4)* `ferrocad_graphics` extracted, `ferrocad_stage` decoupled from inventor. *(2/4)* surface slope/constant depth bias (coplanar shaded-with-edges). *(3/4)* mesh-cache generational eviction. *(4/4)* vertex `part_idx` + the offscreen `Rg32Uint` pass + 1×1 readback + world-point unprojection, wired into the viewport. | ✅ |
| **T5 — `ferrocad.stage` bindings** | `ferrocad_stage_py`: the PyO3 surface of the modern stack (`ShapeItem`/`Stage`/`Tool`/`TransformGizmo`, hits/events, the enums), surfaced as `ferrocad.stage`. | ✅ |
| **T6 — navigation-cube producer** | A second `NavicubeView` producer (in `ferrocad_viewport::navicube`) that builds the NaviCube from `ShapeItem`s, so the modern stack draws it beside the legacy one (see [`navicube-plan.md`](navicube-plan.md), V2). | ✅ |

## 4. As built (T0–T5)

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

### T2 — tools & pointer events

`ferrocad_stage` gains the interactive layer:

- `input` — `PointerKind` (`Move`/`Down`/`Up`), `MouseButton`, and `PointerEvent`
  (device-pixel position + the CPU `Hit` under the pointer): the structured replacement
  for `SoEventCallback`.
- `snap` — `snap_to_geometry(event, camera, grid)`: snaps to the picked world point over
  geometry (`SnapKind::Point`), else to a `z = 0` grid node (`SnapKind::Grid`). Feature
  snaps (vertex/edge/centre) wait on the richer pick in T4.
- `tool` — the `Tool` trait (`on_event`, `items`) and a reference `PolylineTool` (the
  `DraftWireTool` analogue): left-click places vertices, with a rubber band and a snap
  marker following the cursor. The three transient visuals are **fixed slots** with
  stable mesh identities — only their geometry is rewritten — so the renderer's upload
  cache is not flooded with new meshes. (Each rewrite bumps the generation to force a
  re-upload; reclaiming old generations landed in T4, sub-slice 3.)

`InventorViewport` hosts a tool (`with_tool`/`set_tool`): `handle_pointer`/`handle_button`
build a `PointerEvent`, snap it, dispatch to the tool, and rebuild the transient queue.
The transient visuals are drawn as a **separate overlay view** (depth off, on top) and
picked against the scene-and-stage queue only — so a rubber band at the cursor can never
shadow the geometry it is snapping to, and needs no depth-bias fix.

**Verification:** `ferrocad_stage` tests cover point/grid snapping, snap-over-hit,
left-click placement, band/marker emission, and right-click ignoring;
`ferrocad_viewport`'s `a_tool_receives_pointer_events_and_its_items_are_drawn` drops a
`PolylineTool` into the viewport, dispatches a click, and asserts the transient
(wireframe) overlay was populated.

### T3 — the transform gizmo

`ferrocad_stage::gizmo` implements the manipulator (the `SoDragger` replacement):

- **Handles** — `GizmoHandle` gives 9 sub-handles (3 axes, 3 planes, 3 rings), each with
  a distinct `PickId` (`GIZMO_BASE_ID + 0..8`); `GizmoMode` selects which are built
  (`Translate` → axes + planes, `Rotate` → rings, `Universal` → all). An axis is a shaft
  line plus a cone arrowhead, a plane is a translucent quad, a ring is a circle.
- **Screen-space invariance** — `scale_for(camera, origin, pixels)` keeps the triad a
  fixed device-pixel radius (`∝ distance / (m5 · height)`), recomputed each frame by
  `TransformGizmo::update`, so zooming and orbiting never shrink or explode it.
- **Constraint projection** — `begin_drag`/`drag_to`/`end_drag` implement the ray math:
  `closest_axis_parameter` (1D skew-line closest point), `ray_plane` (2D), and
  `ring_angle` (arc, via `atan2`), emitting a `GizmoDragEvent` (`delta_transform` +
  `total_transform`) per tick and a `GizmoCommitEvent` on release.
- **Solver backbone (T1, `docs/gizmo-manipulators.md`)** — `ManipulatorFrame` gives the
  triad an orthonormal origin/basis (world by default, object-local via
  `from_rotation`); a near view-aligned axis switches to a screen-space projection
  (`ScreenAxis`) and is disabled below 1 px; edge-on planes/rings fall back to a
  view-facing plane; rotation rings use relative 15° detents, with `Ctrl` free via
  `drag_to_with_modifiers`/`GizmoModifiers`. The existing `begin_drag`/`drag_to`
  signatures are unchanged, so the Python binding and viewport need no changes.
- **Analytic hit test (T2)** — `TransformGizmo::hit` projects the axis stems, plane quads
  and rings and applies a 2 px stroke + 10 px grab padding. Priority is axis > plane >
  ring, then nearest/front-most; the triad interior is not in the pick set, so it passes
  through to the scene. `InventorViewport::gizmo_handle_at` delegates to it; the
  `gizmo_queue` is now draw-only.
- **Typed values and readout (T3)** — the drag/commit events carry `GizmoAxis`,
  `GizmoConstraint` and `GizmoValue` (`Linear`/`Planar`/`Angular`); `readout()` formats
  `ΔX +24.5 mm` / `ΔθZ +15.0°`. `begin_drag_with_modifiers` resolves Shift into a plane,
  `drag_to_with_modifiers` applies Ctrl to the ring detents, and `set_linear_snap` uses
  the viewport grid for axis/plane drags. `cancel_drag` aborts without a commit. The
  viewport threads gpui modifiers, focuses on mouse-down (Esc cancels) and paints the
  projected tag as a text layer.
- **Visual language (T4)** — active cyan (`#38BDF8`) / inactive slate (`#94A3B8`) tokens
  (linear), a 1.5 px shaft/ring ribbon via the shared
  `ferrocad_graphics::screen_ribbon`, V-shaped micro-chevrons instead of cones, and
  translucent glass planes with a 1 px rim. The gizmo rebuilds when the camera view
  changes because the ribbons depend on the view direction.
- **Transient override** — `ShapeItem` gained `temporary_transform` (`world = placement ·
  temporary`), used as a live preview: the viewport sets it on the target item during a
  drag and clears it on commit, when the document applies the committed transform.

The gizmo is drawn as an overlay and picked **before** the tool and the scene, so a drag
on a handle wins.

**Verification:** `ferrocad_stage` tests cover the distinct pick ids, the distance-
invariant scale, the per-mode handle set, axis/plane/ring drags, the commit, hover, the
`ManipulatorFrame` basis/local-frame drag, the view-aligned axis fallback, the point-at-eye
disable rule, the edge-on plane fallback, relative 15° detents/`Ctrl` free rotation,
frame-change cancellation, the analytic hit test (padding, priority, mode filtering,
interior pass-through), the typed `GizmoValue`/readout, Shift plane drags, linear grid
snapping, readout anchors, the ribbon draw modes and the 1.5 px stroke;
`ferrocad_viewport`'s `a_gizmo_drag_previews_a_transform_on_the_target_item` presses an
axis handle, drags, asserts the target item's preview, then releases and asserts the
commit and the cleared preview, `the_gizmo_interior_falls_through_to_the_scene` presses
the empty interior and asserts a scene orbit instead, and the T3 tests cover Esc cancel,
the readout lifecycle, Shift planarity and the gpui modifier mapping.

### T4 — the G-buffer pick pass

T4 is four independent sub-slices:

1. **`ferrocad_graphics` extracted** — the neutral draw contract (batch vocabulary,
camera/viewport, CPU picking) moved out of `ferrocad_inventor` into its own leaf crate
(§5.1).
2. **Surface depth bias** — the triangle (surface) pipeline carries a slope/constant depth
bias (`constant: 1, slope_scale: 1.0`), so a coplanar edge wireframe wins the depth test
(§5.4).
3. **Mesh-cache generational eviction** — the renderer's mesh cache is keyed by `MeshId`
alone; a newer `Generation` **replaces** (and drops) the old GPU buffers, so transient
rewrites (tool cursors, gizmo drags) never leak.
4. **The offscreen pick pass** — the selection path, replacing the CPU ray cast on a GPU.

The pick pass (`ferrocad_wgpu::pick`, `crates/ferrocad_wgpu/src/pick.wgsl`) draws the
main scene view once more into its own **single-sampled** targets (the scene pass is
multisampled, so it cannot share the MSAA attachments):

- target 0 `Rg32Uint` — `[entity_id, part_idx]` (the packet's `PickId`, and the hit
  vertex's topological sub-element index);
- target 1 `Rgba32Float` — `[normal.xyz, topology]`;
- target 2 `R32Float` — the depth, written from the fragment's `@builtin(position).z`.
  A depth *attachment* cannot be partially copied on every backend, so the pass binds a
  depth buffer for the test and mirrors its value into a colour target the host can copy
  a texel at a time.

`WgpuRenderer::pick_at(queue, camera, device)` re-uploads any fresh geometry, re-encodes
the pass, copies the 1×1 texel at `device` (bottom-left origin) from each target with
256-byte-aligned staging buffers, and reconstructs the world point by unprojecting the
pixel-centre NDC (`ndc = 2·(p + 0.5)/size − 1`) through the inverse view-projection. A
zero G-buffer id means empty space: the shader writes `pick + 1`, so an object that
carries `PickId(0)` (the compiler's default) is still a valid hit.

It is exposed through the **`InventorView` seam** (in `ferrocad_inventor`): the trait
gains `supports_pick()` (default `false`) and `pick_at(...)` (default `None`).
`InventorViewport::pick` prefers the presenter's G-buffer (when it supports picking),
then the backend's pick when `supports_pick()` is true, and the CPU ray cast
(`ferrocad_graphics::pick_at_hit`) otherwise, so the CPU path stays the fallback for
headless/CPU hosts and every listener/tool/gizmo keeps working unchanged.
The vertex `part_idx` (`Vertex { pos, norm, uv, part_idx }`, a `u32`) is the green
channel; `Hit::part` carries it, and `Hit::element` is the primitive ordinal when the
CPU ray cast resolves one (the G-buffer resolves a pixel, so it reports `None`).
`ShapeItem::set_mesh` tags each position with its part id, and `SoFaceDetail`/
`SoLineDetail` now receive both (`part_index` = `part`, `face_index`/`line_index` =
`element`).

**Topology view (F0.4).** `ShapeItem::from_tessellation` accepts a
`ferrocad_geom::Tessellation`, derives per-position part ids from the per-triangle and
per-segment ids, and keeps the part names (`Face6`, `Edge3`) for `Hit::part`.
`set_highlight` emits a separate flat-shaded surface (and wire) for the selected parts,
in `HIGHLIGHT_COLOR` with `HIGHLIGHT_DEPTH_BIAS`; `with_vertices(true)` adds a
`Topology::Points` packet. The highlight has its own generation, so changing the
selection does not re-upload the base mesh. `StateSnapshot.depth_bias` is now applied
in the scene and pick shaders (`clip.z -= bias * clip.w`).

**Selection bridge (F0.5).** `InventorViewport` turns a click into a
`SelectionEvent` (`Picked(SelectionRef)` or `Cleared`). It resolves `Hit::part` through
the stage item's name table, so a click on a kernel mesh reports `Face6`; a mesh with no
names reports `SelectionRef::whole(entity)`. The gesture rules are explicit: a click is a
left press+release that stayed under the orbit threshold and was not consumed by a gizmo
handle, the navigation cube, the chrome or an active tool. `PickId(0)` (the compiler's
untagged default, and what `with_selectable(false)` emits) never selects. The host owns
the object-id → document mapping; the viewport only gained `ferrocad_geom` for the
vocabulary.

**Verification:** `ferrocad_wgpu`'s `pick_at_resolves_the_object_and_its_world_point`
(two clip-space triangles, distinct ids — a pixel picks the right id and the world
point/normal of the hit) and `pick_at_reports_the_sub_element_index_and_topology` (the
sub-element index and `Topology` come back for a triangle and a line);
`ferrocad_viewport`'s `the_viewport_prefers_a_hardware_pick_when_the_backend_has_one`
proves the viewport prefers a backend's `pick_at` over the CPU ray cast, and
`a_ready_presenters_g_buffer_pick_is_preferred` proves a ready presenter's pick wins for
both hover and button events. `ferrocad_stage`'s `a_tessellation_becomes_a_pickable_stage_item`,
`a_highlight_draws_only_the_selected_part` and `clearing_the_highlight_removes_the_packet`
cover the F0.4 bridge; `ferrocad_wgpu`'s
`a_depth_biased_overlay_draws_over_a_coplanar_surface` proves the bias reaches the GPU.
The F0.5 bridge is covered by `ferrocad_viewport`'s
`a_click_selects_the_named_sub_element_under_the_pointer`,
`hover_mirrors_the_selection_without_consuming_it`,
`an_untagged_mesh_selects_the_whole_object`, `clicking_empty_space_clears_the_selection`,
`an_orbit_drag_is_not_a_selection` and `a_non_selectable_item_is_not_selected`.

**Manipulators (F1).** `ferrocad_stage` owns the *seam*, not the feature:
`Manipulator` + `InteractionCommit` (see `architecture.md` §10). A workbench's
draggable overlay implements it in its own presentation crate;
`ferrocad_part_gui::DressUpScrubber` is the Part edge fillet/chamfer arc. It is built
from the selected edge's world polyline (`ShapeItem::edge_polyline("Edge3")` resolves
a click's element name to ordered points; the anchor is the middle segment's midpoint)
and draws a camera-facing arc whose **world radius is the feature's radius**, plus the
radius tick to its apex. The hit test is analytic and screen-space (the arc and the
tick, within the shared 2 + 10 px slop); the interior between the edge and the arc
passes through. A drag changes the value by the pointer's radial travel, snapped and
clamped, and `drag_end` commits a neutral `InteractionCommit`
(`manipulator: "dress_up"`, `action: "fillet"|"chamfer"`, `values: [radius]` or
`[first, second]`, with the `R 3.50 mm`-style readout);
`DressUpChange::from_commit` is the typed inverse. `InventorViewport` hosts any
manipulator (`with_manipulator`, `add_manipulator`/`set_manipulators`,
`take_interaction_commit`, `cancel_manipulator_drag`), picks manipulators **before**
the gizmo, draws their items as depth-off overlays, and renders the readout pill the
manipulator supplies. The edition contributes the manipulator
(`Contributions::with_manipulator(ferrocad_part_gui::fillet_manipulator)`);
`ferrocad_gpui` maps a commit's entity to the object and calls the app script's
`interaction` hook, which Part implements with `Part::Fillet`/`Part::Chamfer`.
Neither the viewport nor the shell links a kernel, and the base app links nothing at
all. Covered by `ferrocad_part_gui`'s 15 tests (11 arc cases plus the factory and the
commit round-trip), `ferrocad_stage`'s `Manipulator`/`InteractionCommit` test and 2
`edge_polyline` tests, `ferrocad_viewport`'s `a_manipulator_drag_commits_neutral_data`,
`a_click_away_from_a_manipulator_falls_through_to_the_scene`,
`the_manipulator_overlay_is_drawn_and_hovers`,
`cancelling_a_manipulator_drag_ends_it_without_a_commit` and
`a_manipulator_takes_priority_over_the_gizmo` (all driven by a **fake** manipulator,
so no Part code is linked), `ferrocad_gpui`'s `Contributions`/`DisplaySync` tests, and
`ferrocad_part_py`'s `the_shell_helpers_tessellate_and_dress_up_a_document_shape`.

### T5 — the `ferrocad.stage` Python bindings

`crates/ferrocad_stage_py` is the modern-stack counterpart of `ferrocad_inventor_py`: the
PyO3 bindings over `ferrocad_stage`/`ferrocad_graphics`, surfaced as **`ferrocad.stage`**
(and importable standalone as the `ferrocad_stage` extension). It is **pure binding
glue** — every class wraps its Rust counterpart, so no behaviour lives only in Python.

- **`ShapeItem`** — `ShapeItem(entity_id)` with `set_shape(positions, normals=None,
  triangles=None, edges=None)`. Ingestion goes through the **buffer protocol**: a
  contiguous `float32`/`uint32` buffer (a NumPy array, `array.array`, `memoryview`) is
  read directly — validated by element type and contiguity, no per-element Python
  objects — with a sequence fallback (a list of `(x, y, z)` triples). Display attributes
  are properties: `draw_mode`, `surface_color`, `edge_color`, `edge_width`,
  `selectable`, `temporary_transform`; `positions`/`normals`/`triangles`/`edges` read the
  tessellation back.
- **`Stage`** — `add`, `clear`, `len`, `entities`, and `batch_count()` (compile to draw
  packets) so a script can assert what it will draw. `extract_stage` is the host-facing
  analogue of `ferrocad_inventor_py::extract_scene`.
- **`Hit`/`PointerEvent`** and the `MouseButton`/`PointerKind` enums — the typed input
  surface.
- **`Tool`** — a Python base class (`on_event`, `add_item`/`set_items`/`clear_items`);
  `PyTool` drives an instance through the native `ferrocad_stage::Tool` interface, so a
  scripted tool drops into the viewport exactly like `PolylineTool`.
- **`TransformGizmo`** — with `Camera` (`perspective`/`identity`), the
  `GizmoMode`/`GizmoHandle` enums, and `GizmoDragEvent`/`GizmoCommitEvent` (row-major
  16-float matrices). `begin_drag`/`drag_to`/`end_drag` mirror the Rust API; `handle_event`
  dispatches a `PointerEvent` end to end.

`ferrocad_stage_py::install(py)` registers the module as both `ferrocad_stage` and
`ferrocad.stage`, and attaches `stage` to the `ferrocad` extension when it is loaded (so
the host calls it after booting `ferrocad`). Where `ferrocad_inventor_py::install`
fabricates a synthetic **`pivy`** package (so `from pivy import coin` resolves) and the
App-side facades own the **`FreeCAD`** namespace, this one fabricates nothing: `ferrocad`
is a real extension module, and creating a synthetic one would shadow it.

**Verification:** the acceptance suite lives in
[`tests/test_stage_py.py`](../tests/test_stage_py.py) (9 cases: buffer + sequence
ingestion, the misaligned-buffer error, attribute round-trip, `Stage` emission, a
scripted `Tool` receiving events, and two gizmo drags asserting the emitted events). It is
the single source of truth: `crates/ferrocad_stage_py/tests/stage_py.rs` runs it against an
**embedded** CPython, and `PYTHONPATH=python python3 tests/test_stage_py.py` runs it
against the **built** `ferrocad_stage` extension. A second Rust test
(`the_native_adapter_drives_a_python_tool`) drives a Python `Tool` through the native
`PyTool` adapter.

## 5. Deferred decisions

1. **Batch vocabulary home.** *Done (T4, sub-slice 1):* the vocabulary now lives in the
   neutral `ferrocad_graphics` crate. `ferrocad_inventor` re-exports it, `ferrocad_stage`
   is fully decoupled from Inventor, and `ferrocad_wgpu`/`ferrocad_viewport` depend on it
   directly.
2. **Contract extension.** *Done (T4, sub-slice 4):* `Vertex` gained `part_idx` (the
   G-buffer's sub-element index) and the per-draw uniform gained `pick` + `topology`, so
   the pick pass has a consumer. `pipeline_kind` remains folded into
   `Topology` + the view's `overlay` flag — a separate enum would carry no new information
   yet.
3. **Math crates.** Stay on `ferrocad_types` (CAD-grade f64 `Matrix4`/`Placement`/
   `Rotation`); do **not** introduce `glam` alongside it. The renderer converts f64→f32
   at the WGSL boundary.
4. **Coplanar CAD edges.** *Resolved (T4, sub-slice 2):* the surface (triangle) pipeline
   carries a slope/constant depth bias (`constant: 1, slope_scale: 1.0`), so a coplanar
   edge wireframe wins the depth test; `ferrocad_wgpu` verifies it headlessly
   (`coplanar_edges_render_over_the_shaded_surface`).
5. **Picking path.** *Resolved (T4, sub-slice 4):* the G-buffer pick is the primary path
   where a GPU exists; the CPU ray cast is retained as the fallback (it needs geometry in
   the queue, which a fresh compile always carries). The two agree on the object id and
   the sub-element index, so a switch is invisible to a listener.
6. **Python floor.** Raised 3.10 → 3.11 with T5: FreeCAD 1.0 targets Python 3.11, and
   the stable-ABI `PyBuffer` (the buffer-protocol ingestion `ShapeItem.set_shape` uses)
   only enters the limited API at 3.11. Every facade now names `abi3-py311` — PyO3
   resolves to the *lowest* enabled `abi3-py3NN`, so a mix silently lowers the floor.

## 6. Naming reconciliation

The architecture sketch named some types that this build expresses with the existing
vocabulary:

| Sketch | As built |
| --- | --- |
| `DrawBatchDescriptor` | `ferrocad_inventor::DrawPacket` (+ the `DrawBatch` trait) |
| `MeshCacheKey` | `(MeshId, Generation)` |
| `BatchPipelineKind` | the pipeline is chosen from `Topology` + the view's `overlay` flag (extended at T4) |
| `entity_id` | `PickId` (a `u32`) |
| interleaved `[pos, norm, part_idx]` | `Geometry { Vec<Vertex>, Vec<u32> }`, `Vertex { pos, norm, uv, part_idx }` |
| `glam::Mat4`/`Vec4` | `ferrocad_types::Matrix4` / `[f32; 4]` |
