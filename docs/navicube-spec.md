# Navigation cube (NaviCube) — specification

Status: **specification** (2026-10-12), producer-neutral. Companions:
[`navicube.md`](navicube.md) (the as-built record, N1–N4b), [`navicube-plan.md`](navicube-plan.md)
(the slice plan that makes this spec producer-independent), and
[`inventor-renderer.md`](inventor-renderer.md) (the render stack it rides).

This note states the NaviCube **normatively**: the coordinate conventions, the piece
taxonomy and ids, the exact geometry and colours, the overlay camera, placement,
interaction, and the **build contract** a producer must satisfy. It is written so the
cube can be produced by either the legacy `pivy.coin` stack (`ferrocad_inventor`) or the
modern declarative stack (`ferrocad_stage`) and hosted by one controller
(`ferrocad_viewport`) — see [`navicube-plan.md`](navicube-plan.md) for that layering.

## 1. Scope

A **NaviCube** is a screen-space orientation gizmo in a viewport corner. It:

- mirrors the main camera's **orientation** (not its zoom or pan);
- is drawn as a depth-disabled **overlay** confined to a corner rectangle;
- responds to hover (highlight), click (re-orient the view), and drag (orbit the view);
- carries 6 faces, 12 edges, 8 corners, and a **Home** button.

**Normative vs upstream.** The behaviour, the 26-piece taxonomy and the interaction
model follow FreeCAD's `Gui::NaviCube` + `Gui::SoNaviCube`. The **axis naming** and the
compact **pick-id numbering** are FerroCAD's own (upstream is Z-up with ~35 ids spanning
faces, edges, corners, the four arrows, `Backside`, `Home`, `ViewMenu`; see §13). The
arrow buttons, the context menu, `Backside`/`ViewMenu`, `stepByTurn` and the rotate-to-
nearest rounding are **out of scope** here (§14).

Everything a producer emits is expressed in the shared draw vocabulary
(`ferrocad_graphics`): a `DrawPacket` names a mesh (`mesh_id` + `generation`), a
`Topology`, an optional `Geometry` (on a cache miss), and a `StateSnapshot`
(`world_transform`, `color`, `depth_bias`, `texture`, `id`). A NaviCube adds **no** new
node kinds, node classes or draw-contract fields.

## 2. Conventions

**Axes.** FerroCAD's engine default view: **+Z toward the viewer**, **+Y up**,
**+X right**. A `Piece` is a *signed axis direction* in this frame (each component
−1, 0 or +1). Piece **names** therefore differ from upstream's Z-up naming; the geometry
and behaviour do not.

**Pixels.** Every position below is a **device (physical) pixel** unless it is called a
*logical* pixel. The renderer, the pick buffer, the overlay `Rect`, and the facade all
speak device pixels (see [`inventor-renderer.md`](inventor-renderer.md) §4). The host
converts window → element → device at the gpui boundary; the controller converts a
viewport-local logical point to device through `Camera.viewport`.
Device positions and `Rect`s use the **bottom-left origin** (Coin); logical positions use
the **top-left origin** (Qt/gpui).

**Overlay invariant.** The overlay camera is a **fixed orthographic camera looking down
−Z** at the cube at the origin; the cube is **rotated** to mirror the main camera by
writing the view rotation into a subtree transform (§6). The overlay camera is *not*
rebuilt per frame, and the cube is never moved — only rotated.

## 3. Anatomy and pick taxonomy

```
         6 faces     12 edges     8 corners          1 button
         (1 axis)    (2 axes)     (3 axes)           Home
```

A `Piece` is `[i8; 3]` (each component −1/0/+1); its `kind` is the count of non-zero
components (`Face` 1, `Edge` 2, `Corner` 3). The **pick id** is the 1-based index in
`Piece::ALL`; the Home button is a fixed id.

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

## 4. Geometry (normative constants)

All values in **cube space** (the unit the overlay camera sees). Constants:

| Constant | Value | Meaning |
| --- | --- | --- |
| `CUBE_HALF` | `0.5` | each face sits at ±0.5 on its axis |
| `FACE_INSET` | `0.42` | half-extent of a face quad (inset so gaps read as edges) |
| `EDGE_END_INSET` | `0.13` | how far an edge segment stops short of a corner |
| `CORNER_HALF` | `0.12` | half-size of the small cube at each corner |
| `ORTHO_EXTENT` | `1.1` | orthographic camera half-extent (how much cube space fills the rect) |
| `CAMERA_DISTANCE` | `3.0` | overlay camera distance along +Z |
| `HOME_CENTER` | `(0.0, −0.82)` | Home button centre, in cube space (below the cube) |
| `HOME_HALF` | `(0.24, 0.18)` | Home button half-width / half-height |

**Face quad.** `orientation = Rotation::from_vectors(direction, +Z)` (the rotation that
maps the piece direction onto +Z — the same rotation the main camera takes when it snaps
onto the face, §8). With `inverse = orientation⁻¹` and
`u = inverse·(1,0,0)`, `v = inverse·(0,1,0)`, `center = direction · CUBE_HALF`, the four
corners are

```
center + su·FACE_INSET·u + sv·FACE_INSET·v     for (su, sv) in
(-1,-1), (1,-1), (1,1), (-1,1)
```

wound so the triangle normal points **outward** (`+direction`). Indices `[0,1,2, 0,2,3]`.
Texture coordinates: `FACE_UV = [[0,1],[1,1],[1,0],[0,0]]` (bottom-left, bottom-right,
top-right, top-left), matching the winding. Deriving `u`/`v` from `orientation` makes a
face's baked label read **upright exactly when that face is turned to the viewer**.

**Edge segment.** Let `along` be the piece's single zero axis and `end = CUBE_HALF −
EDGE_END_INSET = 0.37`. Both endpoints take the two non-zero axes at
`axes[i] · CUBE_HALF`; the `along` component is `∓end` (one endpoint `−end`, the other
`+end`). Topology `Lines`, indices `[0,1]`.

**Corner cube.** `center = direction · (CUBE_HALF · √3)`; eight vertices at
`center + (±CORNER_HALF, ±CORNER_HALF, ±CORNER_HALF)`; twelve triangles (a unit cube's six
quads), wound outward. Topology `Triangles`.

**Home button.** A fixed quad (a sibling of the rotating subtree, **not** rotated):
corners `(±HOME_HALF.x, ±HOME_HALF.y, 0) + (HOME_CENTER.x, HOME_CENTER.y, 0)`, wound
counter-clockwise toward the viewer; indices `[0,1,2, 0,2,3]`; UV `FACE_UV`; a baked `H`
label.

## 5. Visual state and colours

| Constant | RGBA |
| --- | --- |
| `BASE_COLOR` | `[0.55, 0.6, 0.66, 1.0]` |
| `HILITE_COLOR` | `[0.2, 0.6, 0.95, 1.0]` |
| `INACTIVE_COLOR` | `[0.38, 0.42, 0.48, 0.5]` |
| `HOME_COLOR` | `[0.5, 0.55, 0.62, 1.0]` |

**Colour rule** (a pure function of the requested state), applied per piece:

```
color(piece) =
    HILITE_COLOR    if piece == hilite
    INACTIVE_COLOR  if piece ∈ away
    BASE_COLOR      otherwise
```

`away` is the set of **face** pieces pointing away from the viewer given the cube's
current orientation; they are drawn **first** and translucent, so the cube reads as a
solid but see-through box. The Home quad is always `HOME_COLOR`.

**Labels.** Faces and the Home button carry a baked label texture (`SoTexture2`-style),
multiplied into the face colour: a light background (`#FFFFFF`) with a dark glyph
(`#282828`), rasterized into a `LABEL_SIZE = 64` square RGBA texture (256 bytes/row, the
upload alignment). Glyphs are 5×7 block letters in an 8-pixel cell, centred. Face
letters: Front `F`, Back `B`, Top `T`, Bottom `D`, Right `R`, Left `L`; Home `H`.
Untextured pieces sample white (so the colour is unaffected).

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
by rotating the **cube**, not the camera: the controller writes
`Rotation::from_matrix(main.view)` into the rotating subtree's transform (§9).

The overlay is drawn with the overlay pipeline (depth write off, depth compare
`Always`, alpha blending) confined to `rect` via `set_viewport` + `set_scissor_rect`; the
main scene is the first view in the same pass and the depth buffer is cleared once.

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
(Upstream's `getNaviCubeSize` default is 132 logical px; the value is config, not
behaviour.) `rect` and the camera's viewport are refreshed whenever the main viewport
size or scale changes.

## 8. Interaction

State: `hilite: Option<Piece>` (hover) and the cube's synced orientation. All thresholds
are **logical** pixels unless noted.

| Gesture | Condition | Effect |
| --- | --- | --- |
| **Hover** | pointer inside `rect`, over a piece / Home | set `hilite` to it (recolour), clear otherwise |
| **Click** | press+release inside `rect`, movement ≤ `drag_threshold` | snap the **main camera** onto the target: a `Piece` → `piece.orientation()`; `Home` → `home_orientation()`; **animated** |
| **Drag** | movement > `drag_threshold` while over the cube | orbit the main camera (`drag_view`) |

**Drag threshold** = `3 logical px × dpr` (upstream's minimum). A movement under it is a
click (snap), not an orbit.

**Snap** is a **quaternion slerp** from the current view rotation to the target rotation,
**smoothstep-eased**, over **220 ms**, preserving the camera distance (the view matrix's
translation). `home_orientation()` = the `(1,1,1)` corner orientation (front-top-right
toward the viewer).

**Drag orbit** (`drag_view`): from a logical delta `(dx, dy)`,
`RADIANS_PER_PIXEL = 0.012`; yaw about world +Y by `dx·RADIANS_PER_PIXEL`, pitch about
world +X by `dy·RADIANS_PER_PIXEL`; compose as `yaw · pitch · currentViewRotation`,
preserving the distance.

**Config** (`NaviCubeConfig`; §12 lists what is deferred):

| Field | Default | Meaning |
| --- | --- | --- |
| `corner` | `TopRight` | which corner |
| `size` | `96.0` | logical px square |
| `margin` | `8.0` | logical px from the corner |
| `base_color` / `hilite_color` / `inactive_color` | §5 | piece colours |
| `inactive_opacity` | `0.5` | away-face alpha (folded into `INACTIVE_COLOR`) |
| `draggable` | `false` | drag moves the *cube* instead of orbiting |
| `rotate_to_nearest` | `false` | a click rounds to the nearest face |
| `step_by_turn` | `0` | quantize rotation into N steps per turn |

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
sits on top of `ferrocad_inventor` and `ferrocad_stage` (see [`navicube-plan.md`](navicube-plan.md)
§2).

## 11. Conformance

A conforming producer, driven with the same `NavicubeVisual`, must emit a cube that
matches the reference producer in:

1. **Piece set and ids** — exactly 26 pieces + the Home quad, ids `1..=26` and `27`; each
   piece's `StateSnapshot.id` is its `pick_id()`.
2. **Geometry** — each piece's vertices/topology per §4 (faces `Triangles`, edges
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

- The four **arrow buttons** (`ArrowNorth/South/East/West`), `ArrowRight`/`ArrowLeft`,
  `Backside`, and `ViewMenu` (§13 ids upstream).
- The **context menu** and the per-view `NaviCubeCommands`.
- `rotate_to_nearest` rounding and `step_by_turn` quantization (config is reserved, §8).
- `draggable` **cube** movement (drag orbits the view today).
- Per-viewer colour/label overrides beyond `NaviCubeConfig`.
- A **text/glyph-run** node (labels exist only as baked textures).

## 13. Upstream mapping

| Upstream (`SoNaviCube::PickId`) | FerroCAD |
| --- | --- |
| `Front/Top/Right/Rear/Bottom/Left` | `FRONT/BACK/TOP/BOTTOM/RIGHT/LEFT` (engine axes, §2) |
| 12 edges (named by two faces) | 12 edges (two signed axes, ids 7–18) |
| 8 corners | 8 corners (three signed axes, ids 19–26) |
| `Home` | Home (id 27) |
| `ArrowNorth…ArrowLeft`, `Backside`, `ViewMenu` | deferred (§12) |

- `src/Gui/NaviCube.{h,cpp}` — controller, config, interaction, animation, and the
  min-drag threshold.
- `src/Gui/Inventor/SoNaviCube.{h,cpp}` — the Coin node, `pickAt`, `viewportRect`,
  `setLabelImage`, the `StyleState`.
- `src/Gui/View3DInventorViewer.cpp` — `naviCubeAnnotation`/`decorationroot`, event
  forwarding, `getNaviCube`.
- `src/Mod/Test/TestCoinNodeSnapshots.py` — curates `SoNaviCube`,
  `SoNaviCubeTranslucent`, `SoNaviCubeHiliteFront`.

## 14. As-built notes (non-normative)

The current build (`docs/navicube.md` §7) already realises §2–§9 and §11 for the
`ferrocad_inventor` producer; make it producer-neutral via
[`navicube-plan.md`](navicube-plan.md). A deviation to fold in:

- The **min-drag threshold** (§8) is specified at `3 logical px × dpr`; the build
  accumulates drag movement (`NavicubeDrag::moved`) but does **not** yet gate the
  click/snap on it — a release over the cube always snaps. Wire the threshold in.
