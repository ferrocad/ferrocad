# The navigation cube (NaviCube) — implementation plan

Status: **N1–N3, the N4 core and the GPUI design slices D1–D4 built (2026-10-20); the
context menu and arrow buttons remain deferred.** §8 records the upstream Python surface
and the snapshot test that exercises it.
Companion to [`inventor-renderer.md`](inventor-renderer.md) (the render stack this
builds on), [`inventor-plan.md`](inventor-plan.md) (the emulation phase roadmap), and
the [`coin-bridge-reuse-assessment.md`](coin-bridge-reuse-assessment.md) evidence.
The producer-neutral **specification** is [`navicube-spec.md`](navicube-spec.md), and the
plan to make the cube implementable by both the legacy and modern stacks is
[`navicube-plan.md`](navicube-plan.md); this note remains the as-built record.

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
| Controller: camera-mirroring transform, corner placement × DPR, hover highlight, click→orientation, drag | `ferrocad_viewport` | New module; reuses listener/events/pick. |
| Overlay routing in the viewport (corner events → NaviCube, else main scene) | `ferrocad_viewport` | `InventorViewport` grows an optional overlay view. |
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
| **N2 — cube + controller** ✅ | The cube model (6 faces, ids 1–6) and the `NaviCube` controller: mirror the main camera orientation into the cube's `Transform`, place the corner rect × DPR, base/hilite colours. | cube scene in `ferrocad_inventor::navicube`; controller in `ferrocad_viewport::navicube` | headless: the corner cube renders and (test) rotates with the camera. |
| **N3 — interaction** ✅ | Rect hit-test → hover highlight (recompile on change); click → snap the main camera onto the picked face. | `ferrocad_viewport::navicube` + `InventorViewport` | headless: a synthesized pointer maps to a face id; a click sets the camera orientation. |
| **N4a — edges, corners & drag** ✅ | Line/point render pipelines (routed by topology) + **line (segment) picking**; the 12 edges (lines) and 8 corners (small cubes) as pieces, each a `PickId`, with hover and click; **drag-rotate** (gpui `on_drag`/`on_drag_move`). | `ferrocad_wgpu` (pipelines), `ferrocad_inventor` (pick + `Piece`), `ferrocad_viewport` | unit: 26 pieces, segment pick, corner hit, orbit; example shows the faceted cube. |
| **N4b — polish** ◐ | **Built:** an **animated snap** (quaternion slerp, eased; `request_animation_frame`) on click; **translucent away-facing faces** (drawn first in `INACTIVE_COLOR`); **baked face labels** (a `Texture2` path + 5×7 glyphs, `SoTexture2`-style) and a fixed **Home** button that snaps to the default orientation. **Remaining:** a context menu and the arrow buttons. | `ferrocad_inventor` + `ferrocad_wgpu` + `ferrocad_viewport` | unit: `slerp`/`SnapAnimation`, away-face colour, labels and Home; headless: the labeled cube. |

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
3. **Controller home:** `ferrocad_viewport` (viewer glue); the cube *scene builder* lives
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
| Cube model (`Piece`, `camera(rect)`) + the producers | `ferrocad_viewport::navicube::{model, inventor, stage}` |
| Multi-view hook | `ferrocad_inventor::view::ViewSpec` + `InventorView::draw_frame` |
| Multi-view pass (overlay pipeline, `set_viewport`/`set_scissor_rect`) | `ferrocad_wgpu::WgpuRenderer::render_views` |
| Controller (placement × DPR, camera sync, hit-test, hover, snap, orbit) | `ferrocad_viewport::navicube::NaviCube` |
| Viewport integration (`with_navicube`, event routing) | `ferrocad_viewport::InventorViewport` |
| Texture path (`State.texture`, `Texture2`, wgpu bind group 1) | `ferrocad_inventor::{state,geometry,node}`, `ferrocad_wgpu::textures` |
| Filled face glyphs (`face_strokes`/`face_glyph`) | `ferrocad_viewport::navicube::model` |
| Animated snap (`SnapAnimation`, `navicube::slerp`), away-face translucency, inertial orbit | `ferrocad_viewport::navicube` + `InventorViewport` |
| gpui chrome: PERSP/ORTHO, Home, `AZ/EL` readout, `◀`/`▶` yaw | `InventorViewport::render` (`navicube_chrome`) |
| Compass ring, yaw scrub, detents | `navicube::{model, inventor}`, `InventorViewport` |

**Behaviour:** the cube mirrors the main camera's **view rotation** (its `Transform`
node); a hover recompiles the cube with the hovered **piece** (face, edge or corner)
highlighted; a click **animates** the main camera onto that piece
(`SnapAnimation` + `navicube::slerp`), a drag **orbits** it (`navicube::drag_view`),
keeping the camera distance. Away-facing faces are drawn first, translucent, so the
cube reads as a solid-but-see-through box. The pieces are sorted **back-to-front** by
their view-space depth (pick id breaks ties), because the overlay has depth off: back
edges and corner cubes then sit behind the front faces instead of painting over them.
Every face carries a **filled 3D glyph** (WA7) whose triangle quads lie in the face plane,
inset a hair along the normal, oriented from the face's *snap* rotation
([`Piece::orientation`]) so it reads upright when that face turns to the viewer and
foreshortens with the facet. The overlay is drawn in one pass. The orbit composes
`pitch · yaw` so a diagonal drag does not roll the horizon.

**Pieces:** `Piece` is the signed axis direction (1/2/3 non-zero components);
`Piece::ALL` orders 6 faces (ids 1–6), 12 edges (7–18) and 8 corners (19–26). The body
is the **watertight 24-vertex chamfered cube** (WA9): six square faces (`FACE_HALF 0.38`
in the ±0.5 planes), twelve bevel quads with twin lines, and eight equilateral corner
triangles. There is no Home packet any more (D4); the turntable uses id 27 (not a piece).
Yaw has no 3D packets at all; it is a chrome control (WA9). `Piece::from_pick_id` maps a
piece id back.

**D1 — GPUI-native rest state (2026-10-20).** The presentation was reset to match the
viewport's thin, vector-sharp language (design: [`navicube-gpui-design.md`](navicube-gpui-design.md)):
faces are low-alpha glass facets, the twelve edge segments are the only wireframe (a
resting face draws no inset rectangle), corners are small octahedral beads, and labels
are light-on-transparent glyph textures at 0.55 alpha. Pointer **proximity** anywhere in
the gizmo rect brightens edges and labels to 0.85/0.9; a hovered piece still takes the
cyan highlight and an inner facet outline. The negative space between pieces passes
through to the gizmo/tool/scene (tests: `navicube_negative_space_reaches_the_scene`).
D1 also uncovered two renderer cache bugs: the compiler now sets a geometry-content
`generation` (mesh ids are positional), and the upload cache is keyed per view so
independent queues cannot overwrite each other's mesh 0.

**D2 — hover targets (2026-10-20).** Hover is now legible on the small targets without a
wide-line pipeline: a hovered face keeps its cyan fill and gains an inner outline plus a
white label; a hovered edge goes cyan and gains a translucent screen-space ribbon; a
hovered corner bead goes cyan and grows a translucent halo. Every corner also carries an
invisible, larger octahedral pick target (radius 0.115 vs the 0.075 bead), so corners are
comfortable to acquire while the drawn bead stays light. Rest and proximity are
unchanged; the stage producer keeps the recolouring but not the ribbon/halo/pick target
(an inventor-producer appearance, like labels).

**D4 — gpui chrome, projection and inertia (2026-10-20).** Home left the 3D overlay: a
gpui bar pinned beside the gizmo rect now holds `[PERSP | ORTHO]`, Home and a live
`AZ/EL` readout (`InventorViewport::render` → `navicube_chrome`, anchored to the cube's
**painted logical rect** via `chrome_bar_rect` / `NaviCube::logical_rect`, so it follows
HiDPI device-pixel rounding instead of `NaviCubeConfig`). The toggle is
**framing-preserving**: `Camera::toggle_projection` sets the ortho `extent` to
`eye_distance·tan(fov/2)` (and back), and orthographic zoom scales the extent instead of
the eye distance. The readout derives the eye's bearing from the view matrix.
The bar's rect is reserved from the viewport's input (pointer/scroll/drag), so it cannot
orbit or pick the scene behind it. Releasing an orbit while the pointer is still moving
starts an **inertial glide** (smoothed velocity, `e^(−t/0.18s)` damping, stop at 12 px/s);
a new press or a zoom stops it. `NaviTarget`, `HOME_PICK_ID`, the Home geometry and
`NavicubeTheme::{Home, home}` were removed; the cube is 26 pieces and `home_orientation()`
remains for the chrome's Home.

**D3 — compass ring (2026-10-20).** The widget grew to 132×132 (ortho half-extent 1.3) and
a **screen-fixed compass ring** now surrounds the cube: a 64-segment circle, 30° ticks,
`N/E/S/W` labels and a diamond heading marker at the camera's azimuth. `compass_angle`
derives the marker from the view rotation with the Y-up convention (E = 0/+X, N = 90/+Z,
W = 180, S = 270); the controller keeps no extra state. `NaviCube::{ring_hit,
ring_angle}` test the annulus in device pixels (pieces win where their pick targets
overlap). A press starts a scrub; `drag_move` applies `navicube::yaw_view` by the pointer
angle delta, a view-space yaw that leaves the horizon level. Releasing within 10° of a
90° heading animates onto the detent (180 ms), otherwise the heading stays. The ring uses
id 27 (deliberately not a piece) and is an inventor-producer capability.

**D5 — hover preview; roll (2026-10-20).** *(The roll chevrons described here were removed
in WA6; the chrome pill's turn controls are now yaw (WA9). The preview cone was removed
in WA7.)* Two filled **roll chevrons** sat in
the ring's diagonal gaps (135° counter-clockwise, 45° clockwise, radius 1.12) and used the
ring tokens. `NaviCube::roll_hit` hit-tested them in cube space (chevrons won over the
ring's scrub band; pieces won over both) and a press animated a **quarter turn about the
screen normal** with `navicube::roll_view` (`RollDirection::angle()` = ±π/2, 220 ms).
Hovering a piece also added a wireframe **preview cone** pointing from the piece toward
the cube centre; it was removed in WA7 because it collided with the glyphs.

**WA1–WA5 — visual pass (2026-10-20).** Five rendering/geometry corrections. (WA1) The
compass ring is now a **ground-plane turntable** at `y = −0.5`, inside the rotating
subtree, so it projects as a flat line at EL 0 and an ellipse as the camera tilts; ring
hits project the pointer onto that plane, and `NaviCube::ring_point` exposes projected
anchors. The cube itself is sized explicitly in logical pixels: `NaviCubeConfig.cube_size`
is its largest projected extent (body diagonal, default **100** px), and the ≈148px
turntable box around it is derived from `ORTHO_EXTENT`; DPR scales the device rect. Ring
layout changes move the box, never the cube. (WA2) All labels became **gpui text**: face letters at projected face centres
(front-facing only, faded by `n·view`) and `N/E/S/W` at projected ring anchors; the baked
glyph atlas and `has_labels` are gone. *(WA6 later moved the face letters back into 3D,
first as stroke line geometry and then as filled triangle glyphs in WA7.)* (WA3) The body
is a **truncated cube** (chamfer
0.10): octagon faces, truncated edge segments and flush corner triangles; the octahedral
beads, pick beads and halos are removed and edges/corners own the resting wireframe.
(WA4) Depth is carried by opacity: front edges @ 0.62, back edges @ 0.10, front glass
@ 0.16 vs away @ 0.06. (WA5) The chrome is one compact row (11px,
`#0F1218` @ 0.6, 1px `#FFFFFF14`) instead of a heavy two-line card. Also fixed:
`yaw_view` now composes the yaw on the right (world Y), so a ring scrub no longer changes
elevation when the camera is tilted.

**WA6 — depth, scale and typography pass (2026-10-20).** (1) The ring's far arc
(`view z < 0`) is a separate packet emitted first at 20% alpha, so the back half passes
behind the cube; far-side `N/E/S/W` letters fade to 0.2 and are culled when their projected
radius is `< 0.9`. (2) The turntable widened: ortho half-extent 1.55, ring radius 1.22,
label radius 1.38, so the ellipse clears the cube's 0.707 corner radius. (3) Face letters
are **3D stroke glyphs** (`F/T/L/D/B/R` as line segments in the face plane, scale 0.22,
inset 0.002), emitted by both producers under the face pick id and drawn only on
front-facing faces. (4) Resting faces are **directionally lit** with
`alpha = 0.08 + 0.14·max(0, n̂·L̂)` for `L = normalize(0.3, 0.8, 0.5)`. (5) The cube is
**centred** in a `CHROME_WIDTH = 248` logical column at every corner, so the pill sits
symmetric beneath it; the pill gained `◀`/`▶` buttons (now yaw controls, WA9) and the
floating chevrons, `ROLL_*` constants and `roll_hover` state were removed.

**WA7 — machined-glass pass (2026-10-20).** (1) The cube became a chamfered body with
bevel quads and cut corners; *(the octagon/hexagonal form was corrected to the watertight
24-vertex square/triangle construction in WA9)*. Face letters are **filled triangle
glyphs** (scale 0.38, stroke 0.06, `#EBF2FC` @ 0.8) instead of 1px strokes. (2) The
turntable has a dark `#141820` **floor disc**, a thick `1.15..1.22` **rim band** and two
crisp loop lines; the `N/E/S/W` anchors moved **inside** the track to `0.96`, and
`ORTHO_EXTENT` tightened to `1.28` so the cube fills the frame. (3) Face shading uses an
upper-left light and a smoothstepped dark→silver ramp, giving a truly dark shade face
against the mid-grey viewport. (4) The hover preview cone is deleted. A hovered face keeps
its directional fill and adds only a cyan outline; a hovered corner lights its three
incident bevels (`EdgeNeighbour`), and bevels/corners fill vibrant cyan.

**Interaction:** a drag is tracked with gpui's `on_drag`/`on_drag_move` (this fork's
documented path for non-D&D drags "like resizing"); `on_drag_move` gives the pointer
position, the controller accumulates the delta into `drag_view`. A press/release
without movement snaps to the piece under the pointer.

**WA8 — input idempotency (2026-10-20).** A ring scrub now measures every pointer with
the **press-time** projection (`NaviCube::ring_angle_for`) instead of the live synced
rotation. The turntable rotates with the view, so the live measurement fed the applied
yaw back into the next angle; the resulting gain depended on render timing, so left/right
drags did not cancel and the angle readout seemed frozen. `handle_pointer` is also inert
while a drag is active, leaving the gesture to `drag_move`, and the `◀`/`▶` target
composes from a pending snap's target so rapid opposite clicks cancel. Tests:
`a_multi_step_ring_scrub_returns_to_its_start`,
`rapid_opposite_yaws_compose_to_the_start`.

**WA9 — watertight chamfer, culling and roll-free view (2026-10-20).** (1) The body is the
**24-vertex chamfered cube**: six **square** faces (`FACE_HALF = 0.38` in the ±0.5 planes),
twelve **bevel quads** connecting the shared square corners, and eight **equilateral
corner triangles** of the face vertices `(±0.5, ±0.38, ±0.38)`. The construction is
watertight — no corner gaps or hexagonal facets — and restores the twin bevel lines.
(2) **Back-facing pieces are culled** (faces with `n_view.z < 0`; bevels/corners only when
all adjacent faces are away), so the visible convex facets never overlap and the X-ray
back-face bleed is gone. (3) The turntable is split into a **back** node (floor disc, far
band/loops, far marker) and a **front** node (near band/loops), drawn as
back → visible cube → front, with the far half at 20% alpha. (4) `NaviCube::sync` rebuilds
a **roll-free** rotation from the camera's AZ/EL (`view_rotation`), so `T` stays upright and
the turntable stays horizontal even if the scene is rolled. Near a pole (within ~2.5° of
vertical) it keeps the camera's own in-plane up instead, because a world-Y yaw there is a
pure roll: without that exception the cube and compass would freeze after a Top/Bottom
snap. `camera_angles` reports the screen-right bearing at a pole for the same reason. (5) The chrome `◀`/`▶` arrows
are now **yaw** controls (`InventorViewport::yaw`, `YawDirection`) about world +Y, so the
AZ/EL readout changes and the ring never tilts; the roll-view helper is gone.

**Crate:** these viewport pieces (the element, the controller and the cube) live in
**`ferrocad_viewport`**, extracted from `ferrocad_gpui`; it depends only on
`ferrocad_inventor` (no `ferrocad_wgpu`, no Python). The `InventorView` capability
trait — implemented by `ferrocad_wgpu`, supplied by the shell — is the seam.

**Producer split (V1–V2).** The cube's model and its producers now live in
`ferrocad_viewport::navicube` (`model`/`inventor`/`stage`) behind a `NavicubeView` trait,
and the controller drives a `Box<dyn NavicubeView>`; `ferrocad_inventor::navicube` was
removed. V1 moved the model + inventor producer (behaviour unchanged); V2 added the
modern `StageProducer` and a cross-producer conformance test. V3 formalised the
appearance **theme** (`NavicubeRole`/`NavicubeTheme` — the Rust core owns styling, §5 of
[`navicube-spec.md`](navicube-spec.md)), `NaviCubeConfig`, and the min-drag threshold. See
[`navicube-plan.md`](navicube-plan.md).

**Directives channel (V4.1–V4.2).** A `SoNaviCube` scene node drives the neutral cube
through the neutral `Directives` channel ([`directives.md`](directives.md)) rather than a
bespoke overlay API: the compiler lifts it into a `Directive::NaviCube`, and the viewport
hosts one overlay per directive (a `NaviCube::from_directive` in explicit-placement mode).
The same channel carries `Directive::EventCallback`, so `SoEventCallback` routing and the
navicube overlay share one traversal and one `EventListener` argument — the old
`event_callbacks(scene)` rescan is gone. The `pivy.coin` `SoNaviCube` constructor (so a
Python script can build the node) is V4.3, still pending (§8).

**Verification:** `ferrocad_viewport::navicube` tests — the model/producer units (ids,
kinds, orientation, compilation, labels, ring geometry/compass angle) and the controller
(corner rect, centre→Front, corner hit and pick reach, mirroring, hilite, snap, orbit,
ring bands) — plus the cross-producer conformance suite; viewport tests for the chrome
(placement, projection round-trip, input reservation, angles), the compass scrub and
detents, and the inertial glide; `ferrocad_wgpu` `overlay_draws_into_its_rect`; the
`inventor_roundtrip` example shows the cube and ring (`--headless out.png`).

## 8. The Python surface and conformance (upstream)

The cube is **not** hidden from Python: it is reachable on two levels, and one upstream
test module exercises it.

**The Coin node (`pivy.coin`).** `Gui::SoNaviCube` registers itself in Coin's type system
(`SoNaviCube::initClass`), so a script can build one through pivy — exactly the form
`TestCoinNodeSnapshots.py` uses:

```python
cube = coin.SoType.fromName("SoNaviCube").createInstance()
```

Its fields: `size`, `opacity`, `borderWidth`, `showCoordinateSystem`, `hiliteId`
(`SoSFInt32`, a `PickId`), `cameraOrientation` (`SoSFRotation`), `cameraIsOrthographic`,
`viewportRect` (`SoSFVec4f`), `baseColor`/`baseAlpha`, `emphaseColor`/`emphaseAlpha`,
`hiliteColor`/`hiliteAlpha`, and `axisX/Y/ZColor`.

**The viewer (`FreeCADGui`).** The controller is not a Python class, but
`FreeCADGui.ActiveDocument.ActiveView.getViewer()` returns a `_View3DInventorViewer`
exposing `setEnabledNaviCube(bool)`, `isEnabledNaviCube()`, and `setNaviCubeCorner(int)`
(0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right — `View3DViewerPy.cpp`). So
consumer code can toggle and corner the cube; hover / click / drag stay controller-side.

**Tests.** `src/Mod/Test/TestCoinNodeSnapshots.py` (run `FreeCAD -t
TestCoinNodeSnapshots`) is a **visual snapshot** suite: it renders each curated node
through the live viewer and compares the PNG against a checked-in baseline (pixel
tolerance 8, max mismatch 20%), requiring GUI + pivy + a display. It carries three
NaviCube fixtures — `SoNaviCube`, `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront` —
built with `size`, `opacity` (`0.55` for the translucent one), `borderWidth`,
`showCoordinateSystem = true`, `cameraIsOrthographic = true`, and `hiliteId = 1` (Front)
for the hilite one. Other tests (`TestCornerAxisCrossVisual`, `TestSelectionVisual`,
`TestView3DFramebufferCapture`, `TestRubberbandSelection`) touch the cube only to
**disable** it for framebuffer determinism, and `src/Tools/typing/smoke/smoke.py`
type-checks `isEnabledNaviCube()`.

**Implication for FerroCAD.** The neutral scene node and its viewport host are in place
(V4.1–V4.2: a `NodeKind::NaviCube` compiles to a `Directive::NaviCube` and the viewport
hosts it), but the `pivy.coin` facade does not yet register `SoNaviCube` (or its fields)
— so a Python script cannot construct one, and the `FreeCADGui` facade does not yet
expose the viewer's NaviCube methods. Both are a **tracked gap** (V4.3), not a covered
surface.
