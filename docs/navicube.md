# The navigation cube (NaviCube) — implementation plan

Status: **N1–N3, the N4 core (edges/corners, drag) and N4b (animated snap,
translucent faces, baked labels and the Home button) built (2026-10-12); the context
menu and arrow buttons remain deferred.**
Companion to [`inventor-renderer.md`](inventor-renderer.md) (the render stack this
builds on), [`inventor-plan.md`](inventor-plan.md) (the emulation phase roadmap), and
the [`coin-bridge-reuse-assessment.md`](coin-bridge-reuse-assessment.md) evidence.

This note records how FreeCAD's navigation cube works and what it took to
reimplement it on the FerroCAD render stack (§7 is the as-built map).

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
| Face labels | `ferrocad_wgpu` + `ferrocad_inventor` | Built (N4b): a texture path (`SoTexture2` analogue) + glyph baking into a `Texture2`. |
| Context menu / `Home` / arrows / opacity | — | Later polish. |

**Already reusable** from [`inventor-renderer.md`](inventor-renderer.md): DPR +
`Viewport` conversions, the `Event`/`EventListener` → recompile → highlight loop,
the CPU `pick`, MSAA, and the shell wiring.

## 4. The chosen slice path

Order: **N1 → N2 → N3**, with **N4 deferred**. N1 splits so the pure-Rust change
lands (and is tested) before the renderer change.

| Slice | Deliverable | Where | Verification |
| --- | --- | --- | --- |
| **N1a — sub-rect viewport** ✅ | A sub-rectangle (origin + size, device px) on the view, and a **rect-relative** pick (NDC computed within the rect). | `ferrocad_inventor` | unit: a pick inside a rect resolves the same object as the equivalent full-frame pick. |
| **N1b — overlay render** ✅ | Multi-view `render_views(target, &[ViewSpec])`: main view (full, depth clear) then each overlay (`set_viewport` + `set_scissor_rect`, load), an overlay pipeline (depth write off, compare `Always`, alpha blend). | `ferrocad_wgpu` | headless: the corner region differs from the clear colour (`overlay_draws_into_its_rect`). |
| **N2 — cube + controller** ✅ | The cube model (6 faces, ids 1–6) and the `NaviCube` controller: mirror the main camera orientation into the cube's `Transform`, place the corner rect × DPR, base/hilite colours. | cube scene in `ferrocad_inventor::navicube`; controller in `ferrocad_gpui::navicube` | headless: the corner cube renders and (test) rotates with the camera. |
| **N3 — interaction** ✅ | Rect hit-test → hover highlight (recompile on change); click → snap the main camera onto the picked face. | `ferrocad_gpui::navicube` + `InventorViewport` | headless: a synthesized pointer maps to a face id; a click sets the camera orientation. |
| **N4a — edges, corners & drag** ✅ | Line/point render pipelines (routed by topology) + **line (segment) picking**; the 12 edges (lines) and 8 corners (small cubes) as pieces, each a `PickId`, with hover and click; **drag-rotate** (gpui `on_drag`/`on_drag_move`). | `ferrocad_wgpu` (pipelines), `ferrocad_inventor` (pick + `Piece`), `ferrocad_gpui` | unit: 26 pieces, segment pick, corner hit, orbit; example shows the faceted cube. |
| **N4b — polish** ◐ | **Built:** an **animated snap** (quaternion slerp, eased; `request_animation_frame`) on click; **translucent away-facing faces** (drawn first in `INACTIVE_COLOR`); **baked face labels** (a `Texture2` path + 5×7 glyphs, `SoTexture2`-style) and a fixed **Home** button that snaps to the default orientation. **Remaining:** a context menu and the arrow buttons. | `ferrocad_inventor` + `ferrocad_wgpu` + `ferrocad_gpui` | unit: `slerp`/`SnapAnimation`, away-face colour, labels and Home; headless: the labeled cube. |

A bare corner cube after N1a–N3 is already a genuine NaviCube; N4 adds fidelity
(edges, corners, drag, labels, the Home button). **N1b is the real work**
(multi-view/scissor in the renderer); N2/N3 reuse the built pick/event/highlight path.

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
5. **Labels:** a **`SoTexture2`**-style texture path on the state (`State.texture`,
   `Texture2`) with a baked-glyph texture per face — **not** text nodes (built in N4b).
6. **Config:** corner, size, offset, base/hilite colour, `draggable`,
   `rotateToNearest`, `stepByTurn` (opacity later).
7. **Events:** the corner rect's pointer events route to the overlay first, then the
   main scene — `InventorViewport` gains an optional overlay view.

## 6. Upstream references

- `src/Gui/NaviCube.{h,cpp}` — controller, interaction, animation.
- `src/Gui/Inventor/SoNaviCube.{h,cpp}` — the node, `PickId`, `pickAt`, `viewportRect`.
- `src/Gui/View3DInventorViewer.cpp` — `naviCubeAnnotation`/`decorationroot`, event
  forwarding (`naviCube->processSoEvent(ev)`), `getNaviCube`.
- The NaviCube is one of the node classes `src/Mod/Test/TestCoinNodeSnapshots.py`
  curates (`SoNaviCube`, `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront`).

## 7. As built (N1–N4b)

| Piece | Module |
| --- | --- |
| Sub-rect + rect-relative pick | `ferrocad_inventor::pixels::Rect`; `Camera::ray_through_rect`; `pick_at_rect`/`pick_at_rect_hit` |
| Selection id (per-face pick ids) | `ferrocad_inventor::node::NodeKind::PickId(u32)` |
| Cube scene (`Piece`, `scene(hilite)`, `camera(rect)`, `snap_view`) | `ferrocad_inventor::navicube` |
| Multi-view hook | `ferrocad_inventor::view::ViewSpec` + `InventorView::draw_frame` |
| Multi-view pass (overlay pipeline, `set_viewport`/`set_scissor_rect`) | `ferrocad_wgpu::WgpuRenderer::render_views` |
| Controller (placement × DPR, camera sync, hit-test, hover, snap, orbit) | `ferrocad_gpui::navicube::NaviCube` |
| Viewport integration (`with_navicube`, event routing) | `ferrocad_gpui::InventorViewport` |
| Texture path (`State.texture`, `Texture2`, wgpu bind group 1) | `ferrocad_inventor::{state,geometry,node}`, `ferrocad_wgpu::textures` |
| Baked face labels + fixed Home button + `NaviTarget` | `ferrocad_inventor::navicube` |
| Animated snap (`SnapAnimation`, `navicube::slerp`) + away-face translucency | `ferrocad_gpui::navicube` |

**Behaviour:** the cube mirrors the main camera's **view rotation** (its `Transform`
node); a hover recompiles the cube with the hovered **piece** (face, edge or corner)
in `HILITE_COLOR`; a click **animates** the main camera onto that piece
(`SnapAnimation` + `navicube::slerp`), a drag **orbits** it (`navicube::drag_view`),
keeping the camera distance. Away-facing faces are drawn first, translucent, so the
cube reads as a solid-but-see-through box. Every face carries a baked **label** (a
letter texture, `SoTexture2`-style) and a fixed **Home** button sits below the cube
(outside the rotating subtree, so it does not turn) and snaps the view to the default
axonometric orientation. The overlay is drawn in one pass.

**Pieces:** `Piece` is the signed axis direction (1/2/3 non-zero components);
`Piece::ALL` orders 6 faces (ids 1–6), 12 edges (7–18) and 8 corners (19–26). Faces
are inset quads, edges are line segments (line pipeline), corners are small cubes. The
Home button is id 27 (`HOME_PICK_ID`); `NaviTarget` unifies "a piece" and "Home" for
hit-testing and click handling.

**Interaction:** a drag is tracked with gpui's `on_drag`/`on_drag_move` (this fork's
documented path for non-D&D drags "like resizing"); `on_drag_move` gives the pointer
position, the controller accumulates the delta into `drag_view`. A press/release
without movement snaps to the piece under the pointer.

**Verification:** `ferrocad_inventor::navicube` unit tests (ids, kinds, orientation,
compilation, labels, Home) + `pick` segment tests; `ferrocad_wgpu`
`overlay_draws_into_its_rect`; `ferrocad_gpui::navicube` tests (corner rect,
centre→Front, corner hit, Home hit, mirroring, hilite, snap, orbit); the
`inventor_roundtrip` example shows the labeled cube with the Home button
(`--headless out.png`).
