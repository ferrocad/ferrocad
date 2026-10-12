# The navigation cube (NaviCube) — implementation plan

Status: **plan (2026-10-12), not implemented.** Companion to
[`inventor-renderer.md`](inventor-renderer.md) (the render stack this builds on),
[`inventor-plan.md`](inventor-plan.md) (the emulation phase roadmap), and the
[`coin-bridge-reuse-assessment.md`](coin-bridge-reuse-assessment.md) evidence.

This note records how FreeCAD's navigation cube works and what it would take to
reimplement it on the FerroCAD render stack. It is a plan, not a commitment.

## 1. What it is

A screen-space **overlay in the viewer's corner** that mirrors the camera
orientation and can be clicked/dragged to reorient the view. It is **not** part of
Quarter and **not** pivy: it is a FreeCAD `Gui` controller over a Coin node.

- `Gui::NaviCube` (`src/Gui/NaviCube.h/.cpp`) — the controller: config, camera
  sync, input.
- `Gui::SoNaviCube` (`src/Gui/Inventor/SoNaviCube.h/.cpp`) — the Coin node
  (`SoShape`) with `viewportRect`, `cameraOrientation`, `hiliteId`, colours, …
- Placed by `View3DInventorViewer`: an `SoAnnotation` (`naviCubeAnnotation`) under
  `decorationroot` holds the cube node (a depth-disabled overlay); the viewer
  forwards each event to `NaviCube::processSoEvent`.

## 2. Anatomy (as built upstream)

**Structure.** `coinRoot` (`SoSeparator`) → `SoCallback` (which calls
`syncNodeState` every traversal, copying the viewer camera's orientation and
orthographic flag into the node) → `SoNaviCube`.

**Pick taxonomy** (`SoNaviCube::PickId`, ~35 ids): 6 faces (`Front`, `Top`,
`Right`, `Rear`, `Bottom`, `Left`), 12 edges (`FrontTop`, …), 8 corners
(`FrontTopRight`, …), arrows (`ArrowNorth`…`ArrowRight`), plus `Backside`, `Home`,
`ViewMenu`. `FaceType` (`None`/`Main`/`Edge`/`Corner`/`Button`) decides the drag
behaviour.

**Placement.** A corner rectangle sized `logicalSize × DPR` (default size **132**
logical px), positioned by `relPos`/`posAreaBase` from the chosen corner; the node
carries it as `viewportRect`.

**Hit-test.** `pickAt(point)` — a **screen-space** hit-test inside the node, not a
ray cast.

**Camera sync.** The cube's transform mirrors the viewer camera's orientation, so
it rotates with the view.

**Interaction.** hover → `hiliteId`; click → `getFaceRotation`/
`getNearestOrientation` then `viewer->setCameraOrientation(orientation,
moveToCenter)` (animated); drag → `RotateCamera` (or `MoveNaviCube` when
draggable), with a minimum threshold of 3 logical px × DPR. `FaceType` selects the
mode.

**Labels** are baked into **textures** (`createCubeFaceTextures` →
`setLabelImage` → `SoTexture2`), not drawn as text nodes.

## 3. What it needs in our stack

| Piece | Home | Gap |
| --- | --- | --- |
| **Overlay / multi-view render** (corner rect + scissor + on-top, depth off, its own camera) | `ferrocad_wgpu` | The bulk. Today: one pass, full target, one camera. |
| Ortho camera + viewport **rect**; rect-aware pick | `ferrocad_inventor` | `Camera` already carries matrices (ortho is another projection); `Viewport` needs a sub-rect; `pick_at` needs NDC within the rect. |
| The cube model (6 faces, 12 edges, 8 corners, ids) | `ferrocad_inventor` scene | Just `Coordinate3`+`Mesh`+`Material`; ids via `PickId`. No new node kinds. |
| Controller: camera-mirroring transform, corner placement × DPR, hover highlight, click→orientation, drag | host/glue (`ferrocad_gpui`) | New module; reuses listener/events/pick. |
| Overlay routing in the viewport (corner events → NaviCube, else main scene) | `ferrocad_gpui` | `InventorViewport` grows an optional overlay view. |
| Face labels | `ferrocad_wgpu` + `ferrocad_inventor` | Extra: a texture path (`SoTexture2` analogue) + text baking. |
| Context menu / `Home` / arrows / opacity | — | Later polish. |

**Already reusable** from [`inventor-renderer.md`](inventor-renderer.md): DPR +
`Viewport` conversions, the `Event`/`EventListener` → recompile → highlight loop,
the CPU `pick`, MSAA, and the shell wiring.

## 4. The chosen slice path

Order: **N1 → N2 → N3**, with **N4 deferred**. N1 splits so the pure-Rust change
lands (and is tested) before the renderer change.

| Slice | Deliverable | Where | Verification |
| --- | --- | --- | --- |
| **N1a — sub-rect viewport** | A sub-rectangle (origin + size, device px) on the view, and a **rect-relative** pick (NDC computed within the rect). | `ferrocad_inventor` | unit: a pick inside a rect resolves the same object as the equivalent full-frame pick. |
| **N1b — overlay render** | Multi-view `render(target, &[View])`: main view (full, depth clear) then each overlay (`set_viewport` + `set_scissor_rect`, load), an overlay pipeline (depth write off, compare `Always`, alpha blend); a static ortho cube drawn in the corner. | `ferrocad_wgpu` | headless: the corner region differs from the clear colour; present with and without the overlay. |
| **N2 — cube + controller** | The cube model (6 faces + 12 edges + 8 corners, each a `PickId`) and the `NaviCube` controller: mirror the main camera orientation into the cube's `Transform`, place the corner rect × DPR, base/hilite colours. | cube scene in `ferrocad_inventor`; controller in `ferrocad_gpui` | headless: the corner cube renders and rotates with the camera. |
| **N3 — interaction** | Rect hit-test → hover highlight (reuse the listener); click → snap the main camera to the picked face/edge/corner orientation; drag threshold + drag-rotate. | `ferrocad_gpui` (+ `ferrocad_inventor` pick) | headless: a synthesized pointer maps to a face id; a click sets the camera orientation. |
| **N4 — polish (deferred)** | Face labels (baked textures → needs a texture path), inactive opacity/translucency, context menu, `Home`/arrows/buttons. | `ferrocad_wgpu` + `ferrocad_inventor` + `ferrocad_gpui` | — |

An unlabeled corner cube after N1a–N3 is already a genuine NaviCube; N4 adds
fidelity. **N1b is the real work** (multi-view/scissor in the renderer); N2/N3 reuse
the built pick/event/highlight path.

**Independent of the pivy roadmap.** The NaviCube needs none of
[`inventor-plan.md`](inventor-plan.md)'s Phase 1–2 (arena, fields, PyO3); it uses the
built vertical slice (state accumulator, packets, CPU pick, events, DPR, viewport).

## 5. Decisions (settled)

1. **Hit-test:** reuse the **ray `pick`** with the overlay's ortho camera and the
   corner rect (one pick path), not a bespoke screen-space `pickAt`.
2. **Overlay pass:** the **same MSAA pass**; `set_viewport` + `set_scissor_rect` per
   view; the overlay pipeline matches the pass depth format but writes no depth and
   compares `Always`; the depth buffer is cleared once for the whole pass.
3. **Controller home:** `ferrocad_gpui` (viewer glue); the cube *scene builder* lives
   in `ferrocad_inventor` (pure). Not `ferrocad_widgets`.
4. **Camera sync:** the overlay camera is a **fixed orthographic** camera; the cube's
   `Transform` mirrors the main camera's orientation (no camera juggling).
5. **Labels:** deferred to N4 (needs a texture path).
6. **Config:** corner, size, offset, base/hilite colour, `draggable`,
   `rotateToNearest`, `stepByTurn` (opacity/labels later).
7. **Events:** the corner rect's pointer events route to the overlay first, then the
   main scene — `InventorViewport` gains an optional overlay view.

## 6. Upstream references

- `src/Gui/NaviCube.{h,cpp}` — controller, interaction, animation.
- `src/Gui/Inventor/SoNaviCube.{h,cpp}` — the node, `PickId`, `pickAt`, `viewportRect`.
- `src/Gui/View3DInventorViewer.cpp` — `naviCubeAnnotation`/`decorationroot`, event
  forwarding (`naviCube->processSoEvent(ev)`), `getNaviCube`.
- The NaviCube is one of the node classes `src/Mod/Test/TestCoinNodeSnapshots.py`
  curates (`SoNaviCube`, `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront`).
