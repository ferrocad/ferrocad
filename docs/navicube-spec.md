# Navigation cube (NaviCube) — specification

Status: **specification** (2026-10-12), producer-neutral, refined against FreeCAD
upstream. Companions: [`navicube.md`](navicube.md) (the as-built record, N1–N4b),
[`navicube-plan.md`](navicube-plan.md) (the slice plan that makes this spec
producer-independent), and [`inventor-renderer.md`](inventor-renderer.md) (the render
stack it rides).

This note states the NaviCube **normatively**: the coordinate conventions, the piece
taxonomy and ids, the geometry, colours, overlay camera, placement, interaction, and the
**build contract** a producer must satisfy. Upstream (`Gui::NaviCube` +
`Gui::SoNaviCube`) is the reference; where the current FerroCAD build deviates, §14 records
it. It is written so the cube can be produced by either the legacy `pivy.coin` stack
(`ferrocad_inventor`) or the modern declarative stack (`ferrocad_stage`) and hosted by one
controller (`ferrocad_viewport`) — see [`navicube-plan.md`](navicube-plan.md) for that
layering.

## 1. Scope

A **NaviCube** is a screen-space orientation gizmo in a viewport corner. It:

- mirrors the main camera's **orientation** (not its zoom or pan);
- is drawn as a depth-disabled **overlay** confined to a corner rectangle;
- responds to hover (highlight), click (re-orient the view), and drag (orbit the view);
- carries 6 faces, 12 edges, 8 corners, a **Home** button, and (upstream) a coordinate
  system, six arrow buttons, `Backside` and a `ViewMenu` button.

**Normative vs upstream.** The behaviour, taxonomy and interaction model follow upstream.
The **axis naming** and the compact **pick-id numbering** are FerroCAD's own (upstream is
Z-up with 10 ordered `PickId`s per group; see §13). The arrow buttons, the context menu,
`Backside`/`ViewMenu`, the coordinate-system axes, `stepByTurn` and `rotateToNearest`
rounding are **deferred** here (§12) — their upstream reference values are recorded (§13)
so they can be added later.

Everything a producer emits is expressed in the shared draw vocabulary
(`ferrocad_graphics`): a `DrawPacket` names a mesh (`mesh_id` + `generation`), a
`Topology`, an optional `Geometry` (on a cache miss), and a `StateSnapshot`
(`world_transform`, `color`, `depth_bias`, `texture`, `id`). A NaviCube adds **no** new
node kinds, node classes or draw-contract fields.

## 2. Conventions

**Axes.** FerroCAD's engine default view: **+Z toward the viewer**, **+Y up**,
**+X right**. A `Piece` is a *signed axis direction* in this frame (each component
−1, 0 or +1). Upstream is Z-up, so piece **names** differ; the geometry and behaviour do
not.

**Pixels.** Every position below is a **device (physical) pixel** unless it is called a
*logical* pixel. The renderer, the pick buffer, the overlay `Rect`, and the facade all
speak device pixels (see [`inventor-renderer.md`](inventor-renderer.md) §4). The host
converts window → element → device at the gpui boundary; the controller converts a
viewport-local logical point to device through `Camera.viewport`. Device positions and
`Rect`s use the **bottom-left origin** (Coin); logical positions use the **top-left
origin** (Qt/gpui).

**Overlay invariant.** The overlay camera is a **fixed, square** projection looking down
−Z at the cube; the cube is **rotated** to mirror the main camera by writing the view
rotation into a subtree transform (§6). Upstream forces `aspectRatio = 1` and
`viewportMapping = LEAVE_ALONE`, so a non-square rect stretches the overlay; the same
holds here (and the rect is square by construction, §7).

## 3. Anatomy and pick taxonomy

```
         6 faces     12 edges     8 corners          1 button
         (1 axis)    (2 axes)     (3 axes)           Home (+6 arrows, Backside, ViewMenu — upstream, deferred)
```

A `Piece` is `[i8; 3]` (each component −1/0/+1); its `kind` is the count of non-zero
components. Upstream classifies the same shapes as `FaceType::{Main, Edge, Corner,
Button}`; a piece maps to `Main`/`Edge`/`Corner` by its `kind`.

The **pick id** is the 1-based index in `Piece::ALL`; the Home button is a fixed id.

`Piece::ALL` order — **this order is normative**, because it defines the ids:

| Range | Kind | Count | Direction components |
| --- | --- | --- | --- |
| 1–6 | Face | 6 | one non-zero: `FRONT (0,0,1)`, `BACK (0,0,−1)`, `TOP (0,1,0)`, `BOTTOM (0,−1,0)`, `RIGHT (1,0,0)`, `LEFT (−1,0,0)` |
| 7–18 | Edge | 12 | two non-zero, in `Piece::ALL` order: the four XY-plane (`z=0`) pairs, then the four YZ (`x=0`), then the four XZ (`y=0`) |
| 19–26 | Corner | 8 | three non-zero sign triples |
| 27 | Home | 1 | `HOME_PICK_ID` |

`NaviTarget` unifies the two clickable kinds:

```rust
pub enum NaviTarget {
    Piece(Piece),
    Home,
}
```

`NaviTarget::pick_id()` maps to the id; `NaviTarget::from_pick_id(id)` maps back
(Home first, then `Piece::from_pick_id`). A producer **must** stamp every piece's
`StateSnapshot.id` with `piece.pick_id()` and the Home quad's with `HOME_PICK_ID`.

> *Upstream ids.* Upstream's `Enum` is `None, Front, Top, Right, Rear, Bottom, Left`, the
> 12 edges, the 8 corners, `ArrowNorth/South/East/West`, `ArrowRight/Left`, `Backside`,
> `Home`, `ViewMenu`. Its face **geometry order** (`kCubeFacePickOrder`) is
> `Top, Front, Left, Rear, Right, Bottom,` then the 8 corners, then the 12 edges;
> FerroCAD's id order (faces→edges→corners) is its own — only the *names* correspond.

## 4. Geometry

All values in **cube space**. Two models are recorded: the **current build** (inset
primitives, §4.1 — what the producers implement today) and the **upstream reference**
(one chamfered cube, §4.2). Reconciling them is a deviation (§14).

### 4.1 Current build (normative for the producers)

| Constant | Value | Meaning |
| --- | --- | --- |
| `CUBE_HALF` | `0.5` | each face sits at ±0.5 on its axis |
| `FACE_INSET` | `0.42` | half-extent of a face quad (inset so gaps read as edges) |
| `EDGE_END_INSET` | `0.13` | how far an edge segment stops short of a corner |
| `CORNER_HALF` | `0.12` | half-size of the small cube at each corner |
| `ORTHO_EXTENT` | `1.1` | orthographic camera **half**-extent (cube ≈ 45% of the rect) |
| `CAMERA_DISTANCE` | `3.0` | overlay camera distance along +Z |
| `HOME_CENTER` | `(0.0, −0.82)` | Home button centre (below the cube) |
| `HOME_HALF` | `(0.24, 0.18)` | Home button half-width / half-height |

**Face quad.** `orientation = Rotation::from_vectors(direction, +Z)` (maps the piece
direction onto +Z — the same rotation the camera takes when it snaps onto the face, §8).
With `inverse = orientation⁻¹`, `u = inverse·(1,0,0)`, `v = inverse·(0,1,0)`,
`center = direction · CUBE_HALF`, the four corners are
`center + su·FACE_INSET·u + sv·FACE_INSET·v` for `(su, sv)` in
`(−1,−1), (1,−1), (1,1), (−1,1)`, wound so the normal points outward (`+direction`).
Indices `[0,1,2, 0,2,3]`. `FACE_UV = [[0,1],[1,1],[1,0],[0,0]]`, matching the winding.

**Edge segment.** Let `along` be the piece's single zero axis and
`end = CUBE_HALF − EDGE_END_INSET = 0.37`. Both endpoints take the two non-zero axes at
`axes[i] · CUBE_HALF`; the `along` component is `∓end`. Topology `Lines`, indices `[0,1]`.

**Corner cube.** `center = direction · (CUBE_HALF · √3)` (a cube vertex); eight vertices
at `center + (±CORNER_HALF, ±CORNER_HALF, ±CORNER_HALF)`; twelve outward-wound triangles.

**Home button.** A fixed quad (a sibling of the rotating subtree, **not** rotated):
corners `(HOME_CENTER.x ± HOME_HALF.x, HOME_CENTER.y ± HOME_HALF.y, 0)`, wound
counter-clockwise toward the viewer; indices `[0,1,2, 0,2,3]`; UV `FACE_UV`; a baked `H`
label.

### 4.2 Upstream reference (single chamfered cube)

Upstream builds **one** cube (local half-extent 1) with a single `chamfer` `c` (default
`0.12`, clamped to `[0.05, 0.18]`) that separates faces, edges and corners without gaps.
Each region is placed from two unit axes `x` (in-plane) and `z` (outward normal), with
`y = x × (−z)`:

- **Main face** — an octagon, 8 verts (inset `x2 = 1−2c`, `x4 = 1−4c`, likewise `y2`,`y4`):
  ```
  (z ± x2) ± y4  and  (z ± x4) ± y2   (the 8 corners, wound CCW about +z)
  ```
- **Edge face** — a quad, 4 verts (`yE = c`, `zE = 1−c`, `x4 = 1−4c`):
  ```
  (zE ± x4) ± yE
  ```
- **Corner face** — a hexagon, 6 verts (`xC = yC = c`, `zC = 1−2c`):
  ```
  zC − 2xC,  (zC − xC) − yC,  (zC + xC) − yC,  zC + 2xC,  (zC + xC) + yC,  (zC − xC) + yC
  ```
- **Coordinate system** (`showCS`, upstream default **on**) — three line segments from
  `(a,b,a)`… per axis, with `a = −1.1`, `b = −1.05`, `c = 0.5`, coloured
  `axisX/Y/ZColor` (default red/green/blue).

## 5. Visual state and colours

**Current build palette** (FerroCAD):

| Constant | RGBA |
| --- | --- |
| `BASE_COLOR` | `[0.55, 0.6, 0.66, 1.0]` |
| `HILITE_COLOR` | `[0.2, 0.6, 0.95, 1.0]` |
| `INACTIVE_COLOR` | `[0.38, 0.42, 0.48, 0.5]` |
| `HOME_COLOR` | `[0.5, 0.55, 0.62, 1.0]` |

**Upstream defaults** (reference): `baseColor #E2E8EF`, `hiliteColor #AAE2FF`,
`emphaseColor #000000` — where **`emphaseColor` colours the cube's edges/border**, not a
highlighted face; a hovered face swaps to `hiliteColor` via a per-face material index;
per-colour alpha (`baseAlpha`/`hiliteAlpha`/`emphaseAlpha`) multiplies a **whole-cube
opacity** that is `1.0` while hovering and `inactiveOpacity` (**0.5**) otherwise.

**Colour rule** (current build — a pure function of the requested state), per piece:

```
color(piece) =
    HILITE_COLOR    if piece == hilite
    INACTIVE_COLOR  if piece ∈ away
    BASE_COLOR      otherwise
```

`away` is the set of **face** pieces pointing away from the viewer given the cube's
current orientation; they are drawn **first** and translucent, so the cube reads as a
solid but see-through box. The Home quad is always `HOME_COLOR`. (Upstream has no
"away" concept; it makes the *whole* cube translucent when not hovered.)

**Labels.** Faces and the Home button carry a baked label texture (`SoTexture2`-style)
multiplied into the face colour. Current build: a light background (`#FFFFFF`) with a
dark glyph (`#282828`) in a `LABEL_SIZE = 64` square (256 bytes/row, the upload
alignment); 5×7 block letters in an 8-pixel cell, centred. Face letters: Front `F`,
Back `B`, Top `T`, Bottom `D`, Right `R`, Left `L`; Home `H`. Untextured pieces sample
white. Upstream: a **192**×192 texture, **Arial**, **white** text on a transparent
background, `fontZoom` (0.3) sizing, vertically mirrored for the GL texture origin, for
the six faces in the order Front, Top, Right, Rear, Bottom, Left.

> Labels are a **producer capability**, not a model requirement (§10): the legacy
> producer bakes textures; a producer without a texture path draws untextured faces and
> still conforms (§11).

## 6. Overlay camera

`camera(rect)` is a **fixed orthographic camera**:

```rust
Camera {
    view: Placement::new(Vector3::new(0.0, 0.0, -CAMERA_DISTANCE), Rotation::identity()).to_matrix(),
    projection: Projection::Orthographic { extent: ORTHO_EXTENT, near: 0.1, far: 100.0 },
    viewport: Viewport::new([rect.width.max(1), rect.height.max(1)], 1.0),
}
```

The camera looks down −Z from `(0,0,−3)` at the origin. Mirroring the main camera is done
by rotating the **cube**, not the camera: the controller writes the main view rotation
into the rotating subtree's transform (§9). Upstream instead sets
`rootTransform.rotation = cameraOrientation⁻¹` and `translation = (0,0,OVERLAY_CUBE_Z)`.

**Upstream overlay reference:** near `0.1`, far `10.1`; ortho `height = 2·2.1 = 4.2`
(Y ∈ [−2.1, 2.1]); cube at `z = −5.1` (`focalDistance 5.1`), buttons at `z = −4.0`; a
**perspective fallback** with `heightAngle = 2·atan(tan(π/8)·1.1)`; `aspectRatio = 1`.

The overlay is drawn with the overlay pipeline (depth write off, depth compare `Always`,
alpha blending) confined to `rect` via `set_viewport` + `set_scissor_rect`; the main
scene is the first view in the same pass. Upstream clears **only the depth buffer** in the
overlay rect (`clearOverlayDepth`) — equivalent in effect to depth-off + `Always` here.

## 7. Placement

The cube occupies a **square device-pixel `Rect`** in one of four corners. The rect is
computed from a **logical** size and margin, scaled by the device-pixel ratio:

```
dpr        = viewport.device_pixel_ratio()
size_px    = round(size  · dpr)     // min 1
margin_px  = round(margin · dpr)
right      = W − margin_px − size_px      // W, H = device size
top        = H − margin_px − size_px      // bottom-left origin

TopLeft     → (margin_px, top)
TopRight    → (right,     top)
BottomLeft  → (margin_px, margin_px)
BottomRight → (right,     margin_px)
```

Build defaults: corner `TopRight`, `size = 96` logical px, `margin = 8` logical px.

**Upstream reference:** `cubeWidgetSize` default **132** logical px
(`getPhysicalCubeWidgetSize = size · dpr`); the cube's **centre** is placed `0.55·size`
from the chosen corner (`posAreaBase = min(posOffset + 0.55·size, view/2)`, then
`relPos ∈ {0,1}²` picks the corner within the remaining area), so the near edge is
`0.05·size` from the corner; `posOffset` (default `(0,0)`) shifts it. `rect` and the
camera viewport are refreshed whenever the main viewport size or scale changes.

## 8. Interaction

State: `hilite: Option<Piece>` (hover) and the cube's synced orientation. All thresholds
are **logical** pixels unless noted.

| Gesture | Condition | Effect |
| --- | --- | --- |
| **Hover** | pointer inside `rect`, over a piece / Home | set `hilite` to it (recolour), clear otherwise |
| **Click** | press+release inside `rect`, movement ≤ `drag_threshold` | snap the **main camera** onto the target (§ below) |
| **Drag** | movement > `drag_threshold` while over a piece | orbit the main camera |

**Drag threshold** = `max(3, round(3 logical px × dpr))` ("device-independent 3 logical
px, at least 3 device px"). A movement under it is a click (snap), not an orbit.

**Click snap.** For a `Piece` the target is the piece's snap rotation (§ below); for
`Home` it is `home_orientation()`. The camera is animated to the target (see below),
preserving the camera distance (the view matrix's translation). `home_orientation()` =
the `(1,1,1)` corner orientation (front-top-right toward the viewer).
Upstream's `Home` calls `viewer->viewHome()` and a **double click** (same pick within the
double-click interval) recenters the camera on the scene bounding box.

**Snap rotation.** The view rotation that turns a piece toward the viewer. It must map the
piece `direction()` onto +Z **and** leave that piece's label upright. Upstream fixes the
roll with an explicit in-plane `x` axis and (for corners/edges) an extra `rotZ` of `0`,
`π/2` or `π` via `getFaceRotation` — e.g. `Top → (x, z, 0)`, `Front → (x, −y, 0)`,
`FrontTopRight → (−x−y, x−y+z, π)`, `FrontTop → (x, z−y, 0)`.

**Nearest orientation** (`rotateToNearest`, upstream default **on**): snap to the closest
of the discrete in-plane orientations instead of the exact roll — **4** candidates
(0, π/2, π, 3π/2) for a main/edge face, **6** (multiples of π/3) for a corner, chosen by
the smallest rotation angle about the snapped axis.

**Drag orbit.** Current build (`drag_view`): from a logical delta `(dx, dy)`,
`RADIANS_PER_PIXEL = 0.012`; yaw about world +Y by `dx·RADIANS_PER_PIXEL`, pitch about
world +X by `dy·RADIANS_PER_PIXEL`; compose `yaw · pitch · currentViewRotation`,
preserving the distance. Upstream delegates to the viewer's navigation **orbit drag**
(`centerMode = SceneBoundingSphere`, `sensitivity = 0.45`, `minDistanceFactor = 1.05`,
`clippingRadiusFactor = 1.0`); when `draggable` it instead **moves the cube** within the
viewport area.

**Animation.** Current build: a **quaternion slerp** from the current to the target
rotation, **smoothstep**-eased, over **220 ms**, preserving the camera distance. Upstream:
a `FixedTimeAnimation` over **500 ms** (`View/AnimationDuration`) with an
**InOutCubic** ease (`View/NavigationAnimationEasingCurve`).

**Hover damping.** Current build: hilite is immediate; the whole cube is fully opaque
only while a piece is hovered, otherwise `INACTIVE_COLOR` faces are translucent.
Upstream: whole-cube `opacity` is `1.0` while hovering else `inactiveOpacity` (0.5), and
an optional **pick hysteresis** (`FREECAD_NAVICUBE_PICK_STABLE_FRAMES`, default 0 =
off) can require a hover to persist over several frames.

**Config** (`NaviCubeConfig`; §12 lists what is deferred):

| Field | Current build | Upstream reference |
| --- | --- | --- |
| `corner` | `TopRight` | `TopRightCorner` |
| `size` | `96.0` logical px | `132` logical px |
| `margin` | `8.0` logical px | `0.05·size` near-edge (centre at `0.55·size`) |
| `offset` | — | `posOffset (0,0)`, device px |
| `chamfer` | — (inset model) | `0.12`, clamp `[0.05, 0.18]` |
| `base_color` / `hilite_color` | §5 | `#E2E8EF` / `#AAE2FF` |
| `emphase_color` | — | `#000000` (edge/border colour) |
| `inactive_opacity` | `0.5` (baked into `INACTIVE_COLOR`) | `0.5` |
| `border_width` | — | `1.1` |
| `show_cs` | — | `true` |
| `draggable` | `false` | `false` |
| `rotate_to_nearest` | `false` | **`true`** |
| `step_by_turn` | `0` | `8` (`rotStepAngle = 2π/step`) |

## 9. The build contract (visual state)

A producer renders the cube **on demand** from a producer-neutral state. Highlight and
away-face translucency are *not* draw-contract features — the contract carries only
`color` and `world_transform` per packet — so a producer realises them by **recolouring
pieces** (and drawing away faces first), exactly as the model's colour rule (§5) says.

```rust
/// What to draw this frame: the cube's current orientation, the highlighted target, and
/// the away-facing faces (drawn first, translucent).
pub struct NavicubeVisual {
    pub orientation: Rotation,       // the main camera's view rotation, mirrored
    pub hilite: Option<NaviTarget>,  // the hovered piece / Home, if any
    pub away: Vec<Piece>,            // away-facing faces (recoloured, drawn first)
}
```

The controller recomputes `away` (§5) from `orientation` and calls the producer **only
when the visual changed** (orientation, `hilite`, or `away`). The Home button is a
sibling of the rotating subtree and is unaffected by `orientation`.

## 10. The `NavicubeView` capability trait

A producer implements one trait and advertises its capabilities:

```rust
/// A producer's rendering of the navigation cube (a legacy `Scene`, or modern items).
pub trait NavicubeView {
    /// Rebuild the overlay packets for `visual`, compiled at `viewport`
    /// (its device-pixel ratio seeds any screen-space geometry). Called only when the
    /// visual state changed.
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual);

    /// The current overlay draw packets.
    fn queue(&self) -> &RenderQueue;

    /// Whether this producer can draw face/Home **labels**. A producer without a
    /// texture path returns `false` and draws untextured pieces (conforming by §11).
    fn has_labels(&self) -> bool {
        false
    }
}
```

The trait is **object-safe** (no generics, no `Self`-typed returns), so a controller can
hold a `Box<dyn NavicubeView>` and the viewport stays non-generic. The model, the trait,
the visual state and both producers live with the widget in `ferrocad_viewport`, which
sits on top of `ferrocad_inventor` and `ferrocad_stage` (see
[`navicube-plan.md`](navicube-plan.md) §2).

## 11. Conformance

A conforming producer, driven with the same `NavicubeVisual`, must emit a cube that
matches the reference producer in:

1. **Piece set and ids** — exactly 26 pieces + the Home quad, ids `1..=26` and `27`; each
   piece's `StateSnapshot.id` is its `pick_id()`.
2. **Geometry** — each piece's vertices/topology per §4.1 (faces `Triangles`, edges
   `Lines`, corners `Triangles`); identical positions across producers.
3. **Colours** — per the §5 rule, including the translucent away faces and the highlighted
   target.
4. **Placement/camera** — the rect from §7 and the camera from §6, such that a
   rect-relative pick at the cube centre resolves `FRONT`.
5. **Stability** — a second `rebuild` with an unchanged `visual` reuses each
   `(mesh_id, generation)` (no cache growth).

Labels (§5) are **capability-gated**: a producer with `has_labels() == false` is exempt
from label comparisons but must still meet 1–5.

The reference is the inventor producer; the conformance test lives with the widget in
`ferrocad_viewport` so both producers are checked against the same expectations.

## 12. Deferred (out of scope)

- The six **arrow buttons** (`ArrowNorth/South/East/West`, `ArrowRight/Left`), `Backside`,
  and `ViewMenu`, plus the **context menu** they open (§13).
- The **coordinate-system axes** (`showCS`).
- `rotate_to_nearest` rounding and `step_by_turn` quantization (§13 values recorded).
- `draggable` **cube** movement (drag orbits the view today).
- Double-click **recenter** on the scene bounding box.
- Per-viewer colour/label overrides beyond `NaviCubeConfig`.
- A **text/glyph-run** node (labels exist only as baked textures).

## 13. Upstream mapping and reference values

| Upstream (`SoNaviCube::PickId`) | FerroCAD |
| --- | --- |
| `Front/Top/Right/Rear/Bottom/Left` | `FRONT/BACK/TOP/BOTTOM/RIGHT/LEFT` (engine axes, §2) |
| 12 edges (named by two faces) | 12 edges (two signed axes, ids 7–18) |
| 8 corners | 8 corners (three signed axes, ids 19–26) |
| `Home` | Home (id 27) |
| `ArrowNorth…ArrowLeft`, `Backside`, `ViewMenu` | deferred (§12) |

Reference defaults (`NaviCubeImplementation`): `showCS = true`, `borderWidth = 1.1`,
`rotateToNearest = true`, `naviStepByTurn = 8` (`rotStepAngle = 2π/step`, `step` clamped
`[4, 36]`), `chamfer = 0.12`, `fontZoom = 0.3`, Arial, `inactiveOpacity = 0.5`,
`posOffset = (0,0)`, `cubeWidgetSize = 132`, axis colours red/green/blue. Buttons: a
click rotates the camera by `rotStepAngle` about the button's axis, `Backside` by `π`;
`Home` calls `viewHome`; `ViewMenu` opens the context menu. Animation: 500 ms, InOutCubic.

- `src/Gui/NaviCube.{h,cpp}` — controller, config defaults, interaction, animation
  (`NavigationStyle::setCameraOrientation`, `NavigationAnimator`).
- `src/Gui/Inventor/SoNaviCube.{h,cpp}` — the Coin node: `PickId`, `pickAt`,
  `viewportRect`, `getFaceRotation`, the chamfer geometry, the overlay cameras and
  `clearOverlayDepth`, the `StyleState`.
- `src/Gui/View3DInventorViewer.cpp` — `naviCubeAnnotation`/`decorationroot`, event
  forwarding, `getNaviCube`, `viewHome`, `startAnimation`.
- `src/Mod/Test/TestCoinNodeSnapshots.py` — curates `SoNaviCube`,
  `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront`.

## 14. Current build and deviations from upstream

The current build (`docs/navicube.md` §7) realises §2–§9 and §11 for the
`ferrocad_inventor` producer; [`navicube-plan.md`](navicube-plan.md) makes it
producer-neutral. Deviations to reconcile (a fidelity backlog, not blockers):

| Area | Current build | Upstream reference |
| --- | --- | --- |
| Geometry | inset quads + line segments + small corner cubes, with gaps (§4.1) | one chamfered cube — octagon/quad/hexagon faces, no gaps (§4.2) |
| Palette | base `[0.55,0.6,0.66]`, hilite `[0.2,0.6,0.95]` | base `#E2E8EF`, hilite `#AAE2FF`, black edges |
| Away faces | translucent `INACTIVE_COLOR` faces | whole cube at `inactiveOpacity` when not hovered |
| Labels | 64² dark glyph on light bg | 192² Arial white on transparent, mirrored |
| Snap animation | 220 ms smoothstep | 500 ms InOutCubic |
| Orbit drag | fixed `0.012` rad/px yaw·pitch | navigation-style orbit (sensitivity 0.45, scene-bound centre) |
| Placement | rect with an 8-px margin | centre at `0.55·size`, `posOffset` |
| Size | 96 logical px | 132 logical px |
| Min-drag threshold | movement tracked, not gated (§8) | `max(3, round(3·dpr))` gates click vs drag |
| Coordinate system, arrows, menu | absent | present (§12–§13) |
