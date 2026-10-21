# Navigation cube (NaviCube) — specification

Status: **specification** (2026-10-12), producer-neutral, refined against FreeCAD
upstream. Companions: [`navicube.md`](navicube.md) (the as-built record, N1–N4b),
[`navicube-plan.md`](navicube-plan.md) (the slice plan that makes this spec
producer-independent), and [`inventor-renderer.md`](inventor-renderer.md) (the render
stack it rides).

This note states the NaviCube **normatively**: the coordinate conventions, the piece
taxonomy and ids, the geometry, the appearance **roles**, the overlay camera, placement,
interaction, and the **build contract** a producer must satisfy. Upstream (`Gui::NaviCube`
+ `Gui::SoNaviCube`) is the reference; where the current FerroCAD build deviates, §14
records it. It is written so the cube can be produced by either the legacy `pivy.coin`
stack (`ferrocad_inventor`) or the modern declarative stack (`ferrocad_stage`) and hosted
by one controller (`ferrocad_viewport`) — see [`navicube-plan.md`](navicube-plan.md) for
that layering.

## 1. Scope

A **NaviCube** is a screen-space orientation gizmo in a viewport corner. It:

- mirrors the main camera's **orientation** (not its zoom or pan);
- is drawn as a depth-disabled **overlay** confined to a corner rectangle;
- responds to hover (highlight), click (re-orient the view), and drag (orbit the view);
- carries 6 faces, 12 edges and 8 corners. **Home**, the projection toggle and the
  angle readout are gpui chrome beside the overlay (D4), not cube targets; upstream adds
  a coordinate system, six arrow buttons, `Backside` and a `ViewMenu` button.

**Behaviour, not styling.** This spec fixes *geometry, taxonomy, interaction and the
semantic appearance roles* the cube uses. It does **not** fix colours, opacity, fonts or
glyph bitmaps: assigning those to the draw primitives is a **theme**, a viewport-level
policy applied through the producer's API (§5). The concrete palettes upstream and the
current build use are recorded as *reference theme values* (§13), not spec constants.

**Normative vs upstream.** The behaviour, taxonomy and interaction model follow upstream.
The **axis naming** follows FreeCAD's Z-up frame (`TOP = +Z`, `FRONT = −Y`); the compact
**pick-id numbering** is FerroCAD's own (upstream uses 10 ordered `PickId`s per group; see
§13). The arrow buttons, the context menu,
`Backside`/`ViewMenu`, the coordinate-system axes, `stepByTurn` and `rotateToNearest`
rounding are **deferred** here (§12) — their upstream reference values are recorded (§13)
so they can be added later.

Everything a producer emits is expressed in the shared draw vocabulary
(`ferrocad_graphics`): a `DrawPacket` names a mesh (`mesh_id` + `generation`), a
`Topology`, an optional `Geometry` (on a cache miss), and a `StateSnapshot`
(`world_transform`, `color`, `depth_bias`, `texture`, `id`). A NaviCube adds **no** new
node kinds, node classes or draw-contract fields; appearance is resolved through a theme
into the existing `StateSnapshot` fields.

## 2. Conventions

**Axes.** The world is **Z-up** (`docs/axis-origin-grid.md` §2 is authoritative): the
ground is the `XY` plane and `+Z` is up. A `Piece` is a *signed world direction* in this
frame (each component −1, 0 or +1): `TOP = +Z`, `BOTTOM = −Z`, `FRONT = −Y` (FreeCAD's
front), `BACK = +Y`, `RIGHT = +X`, `LEFT = −X`.

**View space.** A camera stores a world→view rotation. View space is `+X` right,
`+Y` screen-up, `+Z` toward the viewer. The overlay camera is fixed in view space and the
cube is rotated by the main camera's view rotation, so the cube's local axes are world
axes. A **roll-free** view keeps world `+Z` in the screen's vertical plane; the identity
view is therefore the canonical **top** view (eye `+Z`, screen-up `+Y`).

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

**Relationship to grids and workplanes.** The cube is the **world** compass and is never
re-labelled by a local workplane; how a local frame shows through (the compass heading)
is specified in [`axis-origin-grid.md`](axis-origin-grid.md) §6. The world up-axis
question is **settled: Z-up** (`axis-origin-grid.md` §2, convention table).

## 3. Anatomy and pick taxonomy

```
         6 faces     12 edges     8 corners
         (1 axis)    (2 axes)     (3 axes)
```

A `Piece` is `[i8; 3]` (each component −1/0/+1); its `kind` is the count of non-zero
components. Upstream classifies the same shapes as `FaceType::{Main, Edge, Corner,
Button}`; a piece maps to `Main`/`Edge`/`Corner` by its `kind`.

The **pick id** is the 1-based index in `Piece::ALL`.

`Piece::ALL` order — **this order is normative**, because it defines the ids:

| Range | Kind | Count | Direction components |
| --- | --- | --- | --- |
| 1–6 | Face | 6 | one non-zero: `FRONT (0,−1,0)`, `BACK (0,1,0)`, `TOP (0,0,1)`, `BOTTOM (0,0,−1)`, `RIGHT (1,0,0)`, `LEFT (−1,0,0)` |
| 7–18 | Edge | 12 | two non-zero, in `Piece::ALL` order: the four XY-plane (`z=0`) pairs, then the four YZ (`x=0`), then the four XZ (`y=0`) |
| 19–26 | Corner | 8 | three non-zero sign triples |

A producer **must** stamp every piece's `StateSnapshot.id` with `piece.pick_id()`.
The old `NaviTarget` wrapper and `HOME_PICK_ID` were removed in D4 when Home moved to
the gpui chrome bar; `Piece::from_pick_id(id)` maps an id back to a piece.

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
| `CUBE_HALF` | `0.5` | each face plane sits at ±0.5 on its axis |
| `FACE_HALF` | `0.38` | half-size of each face **square** in its tangent plane |
| `ORTHO_EXTENT` | `1.28` | orthographic camera **half**-extent |
| `RING_INNER` | `1.15` | turntable rim band's inner radius |
| `RING_RADIUS` | `1.22` | turntable outer radius (disc edge and outer rim) |
| `RING_Z` | `−0.5` | the ground `XY` plane: coplanar with the cube's base |
| `RING_LABEL_RADIUS` | `0.96` | radius of the `N/E/S/W` anchors **inside** the rim band |
| `RING_BAND_IN` / `_OUT` | `0.12` / `0.12` | in-plane scrub band about `RING_RADIUS` |
| `RING_PICK_ID` | `27` | the ring packets' id (not a piece) |
| `GLYPH_SCALE` / `_STROKE` | `0.38` / `0.06` | filled face glyph height and stroke width, in cube units |
| `CAMERA_DISTANCE` | `3.0` | overlay camera distance along +Z |
| `CHROME_WIDTH` | `248` | logical width of the chrome column the cube is centred in |

Roll is no longer a view control: the chrome pill's `◀`/`▶` buttons now **yaw** the camera
±90° about the vertical axis (`YawDirection`), keeping the horizon level and the angle
readout live. The D5 hover **preview cone** was removed in WA7, and the old roll-view
helper was removed in WA9.

**Home, projection and angles are chrome, not geometry.** D4 moved Home out of the 3D
overlay: the projection toggle, Home and the `AZ/EL` readout are a gpui bar beside the
cube (see [`navicube-gpui-design.md`](navicube-gpui-design.md) §7.3). `home_orientation()`
remains the orientation Home snaps to.

**The 24-vertex chamfered cube (WA9).** The body is the watertight construction with
`H = CUBE_HALF = 0.5` and `a = FACE_HALF = 0.38`: 6 square faces, 12 bevel quads and 8
equilateral corner triangles, 24 vertices in total. Every quad/triangle shares its edges
exactly with its neighbours, so there are no holes and no hexagonal corners.

**Face square.** `orientation = Piece::orientation()` is the **roll-free snap** rotation
that maps the piece direction onto view `+Z` and keeps world `+Z` as screen up (the same
rotation the camera takes when it snaps onto the face, §8). With `inverse = orientation⁻¹`,
`u = inverse·(1,0,0)`, `v = inverse·(0,1,0)`, `center = direction · CUBE_HALF`, the four
vertices are
`center + su·a·u + sv·a·v` for `(su, sv)` in `(a,a), (−a,a), (−a,−a), (a,−a)`, wound so the
normal points outward (`+direction`). Fill indices are `[0,1,2, 0,2,3]`; the hover outline
is `[0,1, 1,2, 2,3, 3,0]`.

**Bevel quad (WA9).** Let `along` be the edge's single zero axis and `(i, j)` its two
non-zero axes. The four vertices connect the shared square corners on the two adjacent
faces:
```
(-a, si·0.5, sj·0.38)  (+a, si·0.5, sj·0.38)
(+a, si·0.38, sj·0.5)  (-a, si·0.38, sj·0.5)
```
The quad is planar (plane `si·x_i + sj·x_j = CUBE_HALF + FACE_HALF = 0.88`), wound outward,
and split into two triangles. Its **two long edges are the squares' twin in-plane lines**
and its short ends are the corner triangles' edges, so every cube edge shows the
double-line bevel. The outline is all four edges; the hover glow ribbon follows the ridge.

**Corner triangle (WA9).** One vertex from each adjacent face square:
```
(sx·0.5, sy·0.38, sz·0.38)
(sx·0.38, sy·0.5, sz·0.38)
(sx·0.38, sy·0.38, sz·0.5)
```
All three lie on `|x|+|y|+|z| = CUBE_HALF + 2·FACE_HALF = 1.26`; the triangle is
equilateral with side `(CUBE_HALF − FACE_HALF)·√2`, and its three edges are exactly the
three bevel quads' short ends. Wound outward (reversed when the sign product is negative).

**Visibility (WA9).** Back-facing pieces are **culled**, not drawn dimmed: a face is
emitted only when `n_view.z > 0`, a bevel when at least one adjacent face is visible, and a
corner when at least one adjacent face is visible. A convex polyhedron's visible pieces
then never overlap, so the overlay reads as solid without the old X-ray double exposure.
The turntable is the exception: its floor disc stays under the cube, while its rim band and
loops are split into **far** and **near** halves (below).

**Hover-only geometry.** A hovered bevel adds a screen-space ribbon of half-width `0.024`
in view units, built by projecting its ridge, offsetting perpendicular in view XY, then
mapping back through the cube's inverse rotation. A hovered bevel or corner fills cyan; a
hovered **corner** also lights its three incident bevels in a softer cyan
(`EdgeNeighbour`). A hovered **face** keeps its directional glass fill and only gains the
cyan square outline, so the glyph stays readable.

**Face glyphs (WA7).** Each face carries a **filled** 3D glyph in its own plane:
`face_strokes(letter)` gives `F/T/L/D/B/R` as segments in a `[-0.5, 0.5]²` box; each
segment becomes a two-triangle ribbon `GLYPH_STROKE = 0.06` wide at `GLYPH_SCALE = 0.38`,
mapped through the same in-plane axes as [`Piece::polygon`] and offset `CUBE_HALF + 0.002`
along the normal. The glyphs are `Triangles` packets under the **face's** pick id, emitted
only for front-facing faces. The compass `N/E/S/W` letters remain gpui text.

**Turntable (WA1, WA6, WA7).** A **ground-plane** assembly at `z = RING_Z` in the world
`XY` plane, inside the rotating subtree:
- a dark, translucent **floor disc** (64-triangle fan at `RING_RADIUS`, `z = RING_Z − 0.005`)
  that grounds the cube against the viewport;
- a **thick rim band** (quads between `RING_INNER` and `RING_RADIUS`, cyan-grey);
- inner and outer **rim loops** plus a radial tick every 30° as a line list;
- a filled in-plane diamond heading marker (half 0.06) at the camera's compass angle.

The view rotation projects it edge-on into the cube's base line when the eye is level, and
into an ellipse as the camera tilts. **E** = 0 (`+X`), **N** = 90 (`+Y`), **W** = 180
(`−X`), **S** = 270 (`−Y`); the marker angle is `compass_angle(rotation)`. North is `+Y` so
that the canonical top view (down `+Z`, `+Y` screen-up) reads N at the top. All packets
share `RING_PICK_ID` = 27.

The rim band and loops are split at build time by view-space depth into a **far** half
(drawn before the cube) and a **near** half (drawn after it). The far half fades to
`alpha × 0.2`, so it reads as passing behind the cube; the near half keeps the theme colour
and can pass in front of the body. The floor disc stays wholly behind the cube. The
heading marker joins whichever half contains its mean depth. The `N/E/S/W` letters are gpui
text at `RING_LABEL_RADIUS = 0.96` (inside the band, semibold 11px), projected by
`NaviCube::labels`. Labels on the far side whose projected radius is `< 0.9` are **culled**
(they would stamp over the front faces); any other far letter is faded to `alpha × 0.2`.

**Home button.** Removed from the overlay in D4; the gpui chrome's Home control snaps
the camera to `home_orientation()` with the same animated snap as a piece click.

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
  `(a,b,a)`… per axis, with `a = −1.1`, `b = −1.05`, `c = 0.5`, coloured from the theme
  (upstream default: red/green/blue).

## 5. Appearance: roles and theming

The role layer below is the cube's own; the **token values** it resolves to come from
[`visual-language.md`](visual-language.md), which is the single source for the UI palette.

A producer does not choose colours; it tags each piece with a **semantic role** and a
**theme** assigns the appearance. The draw primitives already carry colour, and geometry
carries line/edge width, so theming is a *higher-level assignment* of those attributes —
a **viewport-level** concern that flows through the producer's own API:

- the legacy producer sets `SoMaterial` (colour) and `SoDrawStyle` (line width);
- the modern producer sets `ShapeItem` surface/edge colours and `edge_width`.

The primitives stay generic; the theme supplies the policy, so no `ferrocad_graphics`
change is implied.

`NavicubeRole` is the closed vocabulary a NaviCube emits:

| Role | Meaning |
| --- | --- |
| `Face` / `FaceHighlighted` / `FaceAway` | a cube face: resting / hovered / pointing away |
| `Edge` / `EdgeHighlighted` | an edge segment: resting / hovered |
| `Corner` / `CornerHighlighted` | a corner bead: resting / hovered |

**Role rule** (a pure function of the requested state), per piece — priority highlight,
then away:

```
role(piece) =
    <kind>Highlighted    if piece == hilite          (kind ∈ Face | Edge | Corner)
    FaceAway             if piece is a Face and piece ∈ away
    <kind>               otherwise
```

`away` is the set of **face** pieces pointing away from the viewer given the cube's
current orientation. Away faces are emitted **first** (a draw-order/compositing
requirement, e.g. as translucent surfaces) so the cube reads as a solid but see-through
box; the theme decides their actual opacity. (Upstream has no "away" concept; it dims the
*whole* cube when not hovered — a theme choice.)

The overlay pass has depth testing off, so the current build sorts **all** pieces
back-to-front by their view-space centre depth (which puts the away faces first), with
the pick id as a stable tie-breaker. Drawing every face before the edges/corners let a
back edge or corner paint over a front face; the depth order makes them sit behind it.

**Labels (WA2).** Labels are **not overlay geometry**. `NaviCube::labels()` projects the
text anchors with the same view rotation and rect used by the overlay and returns
`NavicubeLabel { text, position, size, alpha }` in element-local logical pixels. The gpui
viewport paints them as ordinary text elements between the image and the chrome, so they
are crisp/anti-aliased and use the application font.

- **Face letters** (`F/B/T/D/L/R`): one per face whose normal satisfies `n·view > 0.08`,
  centred on the projected face centre, size 16 logical px. Alpha fades from 0.85 at
  `n·view = 1` to 0.30 at the silhouette.
- **Compass letters** (`N/E/S/W`): at `RING_LABEL_RADIUS` in the ground plane, size 10,
  faded as the ring flattens (edge-on the N/S anchors coincide).
- No label is a pick target of its own; the piece/ring pick targets own the area.

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
into the rotating subtree's transform (§9). The cube's local axes are world axes, so this
is a direct copy of the scene's world→view rotation. Upstream instead sets
`rootTransform.rotation = cameraOrientation⁻¹` and `translation = (0,0,OVERLAY_CUBE_Z)`.

**Upstream overlay reference:** near `0.1`, far `10.1`; ortho `height = 2·2.1 = 4.2`
(Y ∈ [−2.1, 2.1]); cube at `z = −5.1` (`focalDistance 5.1`), buttons at `z = −4.0`; a
**perspective fallback** with `heightAngle = 2·atan(tan(π/8)·1.1)`; `aspectRatio = 1`.

The overlay is drawn with the overlay pipeline (depth write off, depth compare `Always`,
alpha blending) confined to `rect` via `set_viewport` + `set_scissor_rect`; the main
scene is the first view in the same pass. Upstream clears **only the depth buffer** in the
overlay rect (`clearOverlayDepth`) — equivalent in effect to depth-off + `Always` here.

## 7. Placement

The cube occupies a **square device-pixel `Rect`** in one of four corners. The caller sets
**the cube's logical size**, and the overlay box around it is derived from the ring layout;
the box is then scaled by the device-pixel ratio. The cube is **centred** in a logical
`CHROME_WIDTH = 248` column whose outer edge carries the margin (so the gpui chrome bar,
which fills that column, is symmetric under the cube):

```
CUBE_SPAN   = √3                         // the cube's body diagonal, in cube units
dpr         = viewport.device_pixel_ratio()
box         = cube_size · 2·ORTHO_EXTENT / CUBE_SPAN   // logical overlay width
box_px      = round(box    · dpr)         // min 1
margin_px   = round(margin · dpr)
group_px    = round(248    · dpr)         // CHROME_WIDTH
inset       = (group_px − box_px) / 2
left        = margin_px + inset
right       = W − margin_px − group_px + inset
top         = H − margin_px − box_px      // H = device size, bottom-left origin
bottom      = margin_px

TopLeft     → (left,  top)
TopRight    → (right, top)
BottomLeft  → (left,  bottom)
BottomRight → (right, bottom)
```

Build defaults: corner `TopRight`, `cube_size = 100` logical px (`box ≈ 148`), `margin = 8`
logical px. `cube_size` is the cube's **largest projected extent** (its body diagonal), so
the body always fits inside a `cube_size × cube_size` square at any orientation, and DPR
scales only the final device rect. The box is wider than the cube because the turntable
(outer radius `1.22`, rim band `1.15..1.22`, labels inside at `0.96`) needs
`ORTHO_EXTENT = 1.28`; **changing the ring layout moves the box, never the cube**, which is
the invariant that keeps this a UI element rather than a zoomable 3D view.

**Back to logical for gpui chrome.** The rounded device rect maps back to a logical rect
whose width is `box_px / dpr`, which at fractional ratios differs by up to `1/dpr` from the
ideal `box`. `NaviCube::logical_rect()` returns it as `[left, top, right, bottom]`
(top-left origin), and the gpui chrome bar is placed from that rect — never from
`NaviCubeConfig` — so painting and pointer reservation agree (design doc §7.5).

**Upstream reference:** `cubeWidgetSize` default **132** logical px
(`getPhysicalCubeWidgetSize = size · dpr`); the cube's **centre** is placed `0.55·size`
from the chosen corner (`posAreaBase = min(posOffset + 0.55·size, view/2)`, then
`relPos ∈ {0,1}²` picks the corner within the remaining area), so the near edge is
`0.05·size` from the corner; `posOffset` (default `(0,0)`) shifts it. `rect` and the
camera viewport are refreshed whenever the main viewport size or scale changes.

## 8. Interaction

State: `hilite: Option<Piece>` (hover), the cube's synced orientation, and the viewport's
projection. All thresholds are **logical** pixels unless noted.

| Gesture | Condition | Effect |
| --- | --- | --- |
| **Hover** | pointer inside `rect`, over a piece or the ring | set `hilite` (piece) or `ring_active` (projected ring); clear otherwise |
| **Click** | press+release inside `rect`, movement ≤ `drag_threshold` | snap the **main camera** onto the piece |
| **Drag** | movement > `drag_threshold` while over a piece | orbit the main camera |
| **Yaw** | press the chrome pill's `◀`/`▶` | animate a ±90° yaw of the eye about **world +Z** |
| **Ring scrub** | press on the projected ground ring, then move | yaw the main camera about **world +Z** by the in-plane pointer angle delta |
| **Release** | after a drag, still moving | inertial glide with exponential damping (`τ = 0.18 s`) |

**Drag threshold** = `max(3, round(3 logical px × dpr))` ("device-independent 3 logical
px, at least 3 device px"). A movement under it is a click (snap), not an orbit.

**Click snap.** The target is the piece's snap rotation (§ below). The camera is animated
to it (see below), preserving the camera distance (the view matrix's translation).
The gpui chrome's **Home** control snaps to `home_orientation()` (the `(1,−1,1)` corner
orientation — front-top-right toward the viewer — with world `+Z` up) with the same
animation; `home_orientation()` is exported by the model.
Upstream's `Home` calls `viewer->viewHome()` and a **double click** (same pick within the
double-click interval) recenters the camera on the scene bounding box.

**Snap rotation.** The view rotation that turns a piece toward the viewer. It must map the
piece `direction()` onto +Z **and** leave that piece's label upright. Upstream fixes the
roll with an explicit in-plane `x` axis and (for corners/edges) an extra `rotZ` of `0`,
`π/2` or `π` via `getFaceRotation` — e.g. `Top → (x, z, 0)`, `Front → (x, −y, 0)`,
`FrontTopRight → (−x−y, x−y+z, π)`, `FrontTop → (x, z−y, 0)`.

**Nearest orientation** (`rotate_to_nearest`, upstream default **on**): snap to the
closest of the discrete in-plane orientations instead of the exact roll — **4** candidates
(0, π/2, π, 3π/2) for a main/edge face, **6** (multiples of π/3) for a corner, chosen by
the smallest rotation angle about the snapped axis.

**Drag orbit.** Current build (`drag_view`): from a logical delta `(dx, dy)`,
`RADIANS_PER_PIXEL = 0.012`; yaw the eye about world +Z by `dx·RADIANS_PER_PIXEL`, pitch
about the current screen-right (view row 0) by `dy·RADIANS_PER_PIXEL`; compose
`pitch · yaw · currentViewRotation`, preserving the distance. Both factors are world
rotations composed on the right; the order is equivalent to yawing first and pitching
about the post-yaw screen-right, which keeps the world vertical in the screen's vertical
plane (no roll) on a diagonal drag. `yaw · pitch` with a fixed world pitch axis rolled the
view (and made the navicube tumble). Upstream delegates to the viewer's navigation
**orbit drag** (`centerMode = SceneBoundingSphere`, `sensitivity = 0.45`,
`minDistanceFactor = 1.05`, `clippingRadiusFactor = 1.0`); when `draggable` it instead
**moves the cube** within the viewport area.

**Animation.** Current build: a **quaternion slerp** from the current to the target
rotation, **smoothstep**-eased, over **220 ms**, preserving the camera distance. Upstream:
a `FixedTimeAnimation` over **500 ms** (`View/AnimationDuration`) with an
**InOutCubic** ease (`View/NavigationAnimationEasingCurve`).

**Ring scrub (WA1, WA8).** The pointer is projected onto the **ground `XY` plane**
`z = RING_Z` in cube space; the in-plane radius must fall in `RING_RADIUS − RING_BAND_IN` …
`RING_RADIUS + RING_BAND_OUT`, and the in-plane angle is the scrub angle. Pieces under
the pointer always win. The projection rotation is **frozen at press time**
(`ring_angle_for`): the turntable is inside the rotating subtree, so re-measuring with the
live rotation after each applied yaw feeds the camera's own motion back into the pointer
angle, which made the same drag amplitude produce different yaws depending on frame timing
and made left/right drags fail to cancel. With the frozen projection the angle is a pure
function of the pointer position, so the scrub integrates exactly. A press starts a scrub;
each move applies `yaw_view(current, Δθ)`, which composes a **world-Z** rotation on the
right and moves the eye by `Δθ` in the ground plane, so the elevation and roll are
unchanged at any tilt. On release, a
heading within 10° of a multiple of 90° animates onto that detent over 180 ms; otherwise
the view stays. The scrub and the ring's diamond marker use the same in-plane angle. When
the ring is edge-on the ray misses the plane (no unique intersection), so scrubbing
requires a tilted view.

**Navicube view (WA9).** `NaviCube::sync` does **not** copy the camera's rotation. It
extracts the eye's azimuth/elevation and rebuilds a roll-free rotation
(`view_rotation(azimuth, elevation)`): azimuth is measured in the ground `XY` plane from
`+X`, elevation above it; screen up is world `+Z` projected onto the view plane, with
`+Y` at the top pole and `−Y` at the bottom pole. The cube therefore always shows `T`
upright and the turntable horizontal, even if the scene camera has a roll; the chrome
readout (which also shows AZ/EL, not roll) stays consistent with it.

At a pole the horizon has no direction: within ~2.5° of vertical, `sync` keeps the
camera's own in-plane up instead (`view_rotation_with_up`). A world-Z yaw at the pole is
a pure camera roll; without this exception the roll-free rebuild would produce an
identical rotation and the cube/compass would look frozen after a Top/Bottom snap. The
AZ readout likewise falls back to the screen-right bearing at a pole, so the yaw arrows
still change the heading there.

**Roll (WA6–WA9).** There is no roll control. The pill's `◀`/`▶` buttons call
`InventorViewport::yaw(direction)`, which applies `yaw_view(current, ±90°)` — a world-Z
yaw moving the eye by the requested azimuth, preserving the eye distance and elevation —
through the same 220 ms smoothstep animation as a snap. The target composes from any
**pending animation's target**, not the partially animated camera, so two quick opposite
clicks cancel exactly. A press inside the pill's logical rect is reserved and never picks,
orbit, or scrubs. Input order is **pieces → ring → cube orbit → scene**.

**Inertial orbit (D4).** A drag samples the pointer velocity (smoothed, logical px/s).
On release, if the pointer was still moving above `120 px/s` and was sampled within
150 ms, the camera keeps orbiting with the velocity decaying by `e^(−dt/0.18 s)`; the
glide stops below `12 px/s`. A new press stops it. The render loop requests frames while
it runs.

**Hover damping.** Hover sets the Highlighted roles immediately. Optionally a **pick
hysteresis** (upstream `FREECAD_NAVICUBE_PICK_STABLE_FRAMES`, default 0 = off) can require
a hover to persist over several frames before it takes.

**Config** (`NaviCubeConfig`) — structural only; appearance is the theme's (§5):

| Field | Current build | Upstream reference |
| --- | --- | --- |
| `corner` | `TopRight` | `TopRightCorner` |
| `cube_size` | `100.0` logical px (a 148px overlay box) | `132` logical px |
| `margin` | `8.0` logical px | `0.05·size` near-edge (centre at `0.55·size`) |
| `offset` | — | `posOffset (0,0)`, device px |
| `chamfer` | — (inset model) | `0.12`, clamp `[0.05, 0.18]` |
| `show_cs` | — | `true` |
| `draggable` | `false` | `false` |
| `rotate_to_nearest` | `false` | **`true`** |
| `step_by_turn` | `0` | `8` (`rotStepAngle = 2π/step`) |
| `border_width` | — | `1.1` (a line-style theme value) |

## 9. The build contract (visual state)

A producer renders the cube **on demand** from a producer-neutral state and a theme. It
resolves each piece's **role** (§5) through the viewport theme to concrete attributes
(colour, edge width, label texture) and applies them through its stack's API while
emitting the piece's geometry. Highlight and away-face appearance are therefore **not**
hard-coded — the producer tags a piece and the theme decides how it looks.

```rust
/// What to draw this frame — the cube's interaction state, independent of styling.
pub struct NavicubeVisual {
    pub orientation: Rotation,     // the main camera's view rotation, mirrored
    pub hilite: Option<Piece>,     // the hovered piece, if any
    pub away: Vec<Piece>,          // away-facing faces (emitted first)
    pub proximity: bool,           // the pointer is anywhere in the gizmo rect
}
```

Given `visual`, a piece's `NavicubeRole` is a pure function (§5); the theme maps the
role to appearance. The controller recomputes `away` from `orientation` and calls the
producer **only when the visual changed** (orientation, `hilite`, `away` or `proximity`).

## 10. The `NavicubeView` capability trait

A producer implements one trait and advertises its capabilities:

```rust
/// A producer's rendering of the navigation cube (a legacy `Scene`, or modern items).
pub trait NavicubeView {
    /// Rebuild the overlay packets for `visual`, resolving each piece's role (§5)
    /// through `theme`, compiled at `viewport` (its device-pixel ratio seeds any
    /// screen-space geometry). Called only when the visual state changed.
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual, theme: &Theme);

    /// The current overlay draw packets.
    fn queue(&self) -> &RenderQueue;
}
```

`Theme` is the viewport-level appearance policy (§5); it carries the attributes the cube
needs (base/highlighted/away colours, an edge width, ring tokens) and is threaded through
the producer API, not defined here. Labels are controller/gpui text, so no producer has a
texture path (WA2). The trait is **object-safe** (no generics, no `Self`-typed returns),
so a controller can hold a `Box<dyn NavicubeView>` and the viewport stays non-generic.
The model, the trait, the visual state and both producers live with the
widget in `ferrocad_viewport`, which sits on top of `ferrocad_inventor` and
`ferrocad_stage` (see [`navicube-plan.md`](navicube-plan.md) §2).

## 11. Conformance

A conforming producer, driven with the same `NavicubeVisual`, must emit a cube that
matches the reference producer in:

1. **Piece set and ids** — exactly 26 pieces, ids `1..=26`; each piece's
   `StateSnapshot.id` is its `pick_id()`.
2. **Geometry** — each piece's vertices/topology per §4.1 (faces `Triangles`, edges
   `Lines`, corners `Triangles`); identical positions across producers.
3. **Roles** — each piece carries the role the §5 rule assigns it (highlighted > away >
   base). Conformance compares **roles**, not resolved colours: appearance is the theme's,
   and a producer is compared under the same theme (or on the roles alone).
4. **Placement/camera** — the rect from §7 and the camera from §6, such that a
   rect-relative pick at the cube centre resolves `TOP` for the identity view.
5. **Stability** — a second `rebuild` with an unchanged `visual` reuses each
   `(mesh_id, generation)` (no cache growth).

Compass labels are **not** producer geometry (WA2): the controller computes them and the
gpui viewport paints them. **Face glyphs are** producer geometry (WA7): filled triangle
packets under the face id, emitted by both producers, so conformance checks them. The D2
edge ribbon and the turntable are **capability-gated**: the stage producer has no ribbon or
turntable packets, but both must still meet 1–5 (compared on the first packet per id). The
chamfered-cube geometry (square faces, bevel quads, corner triangles) and the filled face
glyphs are shared, so both producers emit them and conformance compares them as well. The
D5 hover preview cone was removed in WA7.

The reference is the inventor producer; the conformance test lives with the widget in
`ferrocad_viewport` so both producers are checked against the same expectations.

## 12. Deferred (out of scope)

- The six **arrow buttons** (`ArrowNorth/South/East/West`, `ArrowRight/Left`), `Backside`,
  and `ViewMenu`, plus the **context menu** they open (§13).
- The **coordinate-system axes** (`showCS`).
- `rotate_to_nearest` rounding and `step_by_turn` quantization (§13 values recorded).
- `draggable` **cube** movement (drag orbits the view today).
- Double-click **recenter** on the scene bounding box.
- A **text/glyph-run** node in the 3D overlay (compass labels are gpui text over the
  image; face letters are filled 3D triangle geometry).
- The **`SoNaviCube` Python node** — a scene node driving the cube through the neutral
  **directives** channel, on either producer (see [`directives.md`](directives.md) and
  [`navicube-plan.md`](navicube-plan.md) V4).

## 13. Upstream mapping and reference values

| Upstream (`SoNaviCube::PickId`) | FerroCAD |
| --- | --- |
| `Front/Top/Right/Rear/Bottom/Left` | `FRONT/BACK/TOP/BOTTOM/RIGHT/LEFT` (world axes, §2) |
| 12 edges (named by two faces) | 12 edges (two signed axes, ids 7–18) |
| 8 corners | 8 corners (three signed axes, ids 19–26) |
| `Home` | gpui chrome bar (D4): projection toggle, Home, `AZ/EL` |
| `ArrowNorth…ArrowLeft`, `Backside`, `ViewMenu` | deferred (§12) |

**Structural reference defaults** (`NaviCubeImplementation`): `showCS = true`,
`rotateToNearest = true`, `naviStepByTurn = 8` (`rotStepAngle = 2π/step`, `step` clamped
`[4, 36]`), `chamfer = 0.12`, `posOffset = (0,0)`, `cubeWidgetSize = 132`, `borderWidth =
1.1`. Buttons: a click rotates the camera by `rotStepAngle` about the button's axis,
`Backside` by `π`; `Home` calls `viewHome`; `ViewMenu` opens the context menu. Animation:
500 ms, InOutCubic.

**Reference theme values** (informative — styling is not normative, §1). Current build
(D1/D3): base `[0.055,0.09,0.125]` @ 0.05, hilite `[0.2,0.6,0.95]` @ 0.16, away @ 0.025,
edge `[0.54,0.58,0.64]` @ 0.35 (proximity 0.85, hover cyan @ 1.0), labels light-on-
transparent 64² glyphs at 0.55/0.9/1.0, ring `[0.2,0.24,0.3]` @ 0.45 (proximity 0.7,
active cyan @ 0.9). Upstream: `baseColor #E2E8EF`, `hiliteColor
#AAE2FF`, `emphaseColor #000000` (the **edge/border** colour), `inactiveOpacity 0.5`,
`fontZoom 0.3`, Arial; labels are a **192**² texture with **white** text on a transparent
background, vertically mirrored for the GL texture origin, for the six faces in the order
Front, Top, Right, Rear, Bottom, Left.

- `src/Gui/NaviCube.{h,cpp}` — controller, config defaults, interaction, animation
  (`NavigationStyle::setCameraOrientation`, `NavigationAnimator`).
- `src/Gui/Inventor/SoNaviCube.{h,cpp}` — the Coin node: `PickId`, `pickAt`,
  `viewportRect`, `getFaceRotation`, the chamfer geometry, the overlay cameras and
  `clearOverlayDepth`, the `StyleState`. It registers in Coin's type system, so it is
  instantiable from Python (`coin.SoType.fromName("SoNaviCube").createInstance()`).
- `src/Gui/View3DViewerPy.cpp` — the viewer's Python methods
  (`setEnabledNaviCube`/`isEnabledNaviCube`/`setNaviCubeCorner`) exposed on
  `FreeCADGui`'s `_View3DInventorViewer`.
- `src/Gui/View3DInventorViewer.cpp` — `naviCubeAnnotation`/`decorationroot`, event
  forwarding, `getNaviCube`, `viewHome`, `startAnimation`.
- `src/Mod/Test/TestCoinNodeSnapshots.py` — curates `SoNaviCube`,
  `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront`.

## 14. Current build and deviations from upstream

The current build (`docs/navicube.md` §7) realises §2–§9 and §11 for the
`ferrocad_inventor` producer; [`navicube-plan.md`](navicube-plan.md) makes it
producer-neutral. Structural deviations to reconcile (a fidelity backlog, not blockers) —
**appearance differences are theme choices, recorded in §13, not deviations**:

| Area | Current build | Upstream reference |
| --- | --- | --- |
| Geometry | watertight 24-vertex chamfered cube: square faces, bevel quads, corner triangles (§4.1) | one chamfered cube — octagon/quad/hexagon faces, no gaps (§4.2) |
| Snap animation | 220 ms smoothstep | 500 ms InOutCubic |
| Orbit drag | fixed `0.012` rad/px, pitch·yaw (horizon-preserving) | navigation-style orbit (sensitivity 0.45, scene-bound centre) |
| Placement | rect with an 8-px margin | centre at `0.55·size`, `posOffset` |
| Size | cube `100` logical px + turntable (148px box) | `132` logical px widget |
| `rotate_to_nearest` / `step_by_turn` | off / n/a | on / 8 |
| Coordinate system, arrows, menu | absent | present (§12–§13) |
