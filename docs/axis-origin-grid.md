# Axis, origin and grids: the spatial reference model

Status: design + G0–G5 built (2026-10-20). Companion to [`gizmo-manipulators.md`](gizmo-manipulators.md),
[`navicube-spec.md`](navicube-spec.md), [`stage.md`](stage.md) and
[`architecture.md`](architecture.md) §10.

This note defines **one** model for the things that tell a user where they are: the
world triad, the ground grid, local workplanes, face grids, and the snap feedback that
ties them together. It records the visual language, the data model, the render
mapping, and the slice path. **G0–G5 are built** (§1, §8).

---

## 1. What exists today

| Piece | State |
| --- | --- |
| `Topology::Points` / `Topology::Lines` | ✅ in the draw contract (`ferrocad_graphics`), rendered and picked by `ferrocad_wgpu` as **1 px** primitives |
| 1.5 px strokes | ✅ as **ribbons** (triangles) via `screen_ribbon`/`push_ribbon`; WebGPU has no line width, so the gizmo and the dress-up arc already do this |
| Grid snapping | ✅ workplane-aware `snap_to_geometry(event, camera, context: Option<&SnapContext>)` — unprojects onto the workplane (the world `z = 0` plane by default), rounds to the minor step and reports `Snap.local` + `Snap.major` |
| Feature snapping | ✅ **G4** — vertex, edge midpoint, face centre and workplane origin, resolved in screen space from the display tessellation |
| Snap feedback | ✅ **G4** — the aurora, the dashed rays and the `ΔU`/`ΔV` pill, plus the origin node's proximity bloom |
| Gizmo linear snap | ✅ `set_linear_snap(step)` snaps world components (T3) |
| Numeric pill | ✅ `readout_pill` in the viewport: 11 px monospace, `rgba(15,18,24,0.8)`, 1 px `#FFFFFF14` border |
| Depth-bias overlays | ✅ `StateSnapshot.depth_bias`; `WIRE_DEPTH_BIAS` for a coplanar wire over a face |
| Navigation cube | ✅ world-oriented; six faces + corners/edges, a compass ring, labels |
| Grid state | ✅ an in-memory flag (`ViewState.grid`, A5), toggled by `App_ToggleGrid`, mirrored to `FreeCADGui.ViewState`, and applied to the viewport (`set_grid`); **G5** adds step/extent preferences |
| Grid rendering | ✅ **G2** — `ferrocad_stage::grid` emits dots, major lines and the triad; the viewport draws them in the scene's depth pass |
| Workplane / UCS | ✅ **G1/G3** — the vocabulary, and a face resolved from the display tessellation (`FaceFrame`) with hover preview, activation and `Esc` |
| Per-vertex colour/alpha | ✅ **G0** — `Vertex.color` multiplies the packet colour; the grid carries its radial fade in it |

G0–G5 closed the first three gaps. The model is fully built; what is left is tuning the
visual tokens as the theme lands.

---

## 2. The coordinate model

Three frame kinds, one hierarchy. A **workplane** is the presentation-side UCS: it is
what the user draws on and what the local grid lies in.

```rust
/// A right-handed planar frame: `n = u × v`.
pub struct Workplane {
    pub origin: Vector3,
    pub u: Vector3,
    pub v: Vector3,
    pub n: Vector3,
    /// Where the frame came from (for the UI and the grid's extent rule).
    pub kind: WorkplaneKind,
}

pub enum WorkplaneKind {
    /// The world XY plane at the origin.
    World,
    /// A named datum plane from the document (a `Placement`).
    Datum { name: String },
    /// A planar face of a displayed object.
    Face { entity: u32, element: String },
}
```

The world is **Z-up**: the ground is the `XY` plane and height is `+Z`. That is what
`snap_to_geometry`'s `z = 0`, the toy's `Height`, and OCCT all use, and the grid model
follows it. The navicube's own naming convention is separate and unchanged (§6).

- **World.** `origin = 0`, `u = +X`, `v = +Y`, `n = +Z`. The navicube always reflects
  this frame; it is the immutable camera compass.
- **Workplane (UCS).** Any datum or face frame. It adds local coordinates
  `(a, b, h)` with `p = origin + u·a + v·b + n·h`. Sketch tools work in `(a, b)`.
- **Face frame.** A `WorkplaneKind::Face` resolved from the display tessellation:
  the face's triangles give the centroid and the averaged normal; `u` is the
  projection of world `+X` (or `+Y` if degenerate), `v = n × u`. Only **planar**
  faces produce a face workplane.

### Hierarchy and activity

```
World origin / XY plane
        │  always present (dimmed while a local plane is active)
        ▼
Datum or face workplane   ← the active UCS (sticky until Esc or a new plane)
        │
        └─ local grid, local U/V origin, local snap field
```

Exactly one workplane is **active** for snapping. Lower levels stay visible but
recede: when a workplane is active, the world triad drops to 30 % and the ground grid
to 25 % of its alpha, so the local frame reads first without the scene losing its
global anchor.

---

## 3. Visual language

All colours below are **sRGB hex + alpha**; the renderer consumes **linear** RGBA, so a
token is stored linear at definition time (the existing `gizmo::ACTIVE_COLOR`
(`#38BDF8`) and `gizmo::INACTIVE_COLOR` (`#94A3B8`) already match two of these). The
full token set, type scale, stroke rules and state model are in
[`visual-language.md`](visual-language.md).

### 3.1 World origin triad

| Token | Value | Notes |
| --- | --- | --- |
| X stem | `#F87171` @ 0.65 | linear ≈ `[0.939, 0.165, 0.165]` |
| Y stem | `#4ADE80` @ 0.65 | linear ≈ `[0.069, 0.730, 0.216]` |
| Z stem | `#38BDF8` @ 0.65 | linear ≈ `[0.040, 0.509, 0.939]` |
| Stem width | 1.5 px | **ribbons**, not GL lines |
| Stem length | screen-invariant, ~80 px (the gizmo's `DEFAULT_PIXELS`) | scales with the camera |
| Center node | hollow diamond, 3 px | blooms to full `#38BDF8` within a **5 px** cursor radius |
| Axis labels | 10 px monospace, `X`/`Y`/`Z` | pinned to the positive tips, gpui text layer |

The center node's 5 px is a **proximity highlight**, distinct from the 12 px grab slop
(`HIT_SLOP`): the node is never a pick target that steals a click from geometry.

### 3.2 Ground grid (the turntable field)

| Property | World | Workplane |
| --- | --- | --- |
| Minor step | 10 mm dots | 1 mm dots |
| Major step | 100 mm lines | 10 mm lines |
| Colour | `#94A3B8` | `#94A3B8` |
| Alpha at the origin | 0.15 | minor 0.15, major 0.35 |
| Extent | disc of radius `R_max` around the camera pivot | the parent face's extent, or `R_max` for a datum |
| Depth | normal depth test | coplanar + `WIRE_DEPTH_BIAS` |

**Radial falloff.** Every grid vertex carries

```
α(d) = max(0, α0 · (1 − d / R_max))
```

so lines fade to nothing rather than ending on a hard rectangle. `R_max` is
`2.5 × max(|eye − pivot|, scene radius)`, clamped to 0.5–5 m; the grid is regenerated
only when the pivot moves by more than 10 % of `R_max` (not every frame).

**Screen-space culling.** Below ~4 px of projected spacing a level is dropped, and the
surviving step is the next decade (`10 → 100`). This is what keeps a top-down
orthographic view crisp and a low-angle horizon view from turning into moiré: at a low
pitch the ground grid naturally compresses into a thin band, and the falloff closes it
off.

### 3.3 Active workplane grid

- **Local origin.** The U/V axes cross at the workplane origin: 1.5 px (ribbons),
  in the same X/Y hues but at the **local** frame's orientation, plus a 2 px glowing
  dot at `(0, 0)`.
- **Boundary.** Bounded to the parent face (plus a 10 % margin) or to `R_max` for a
  datum. The boundary is a fade, never a drawn rectangle.
- **Hierarchy.** Minor dots sit under major lines; the local axes sit over both.

### 3.4 Snap feedback

| Element | Spec |
| --- | --- |
| Snap aurora | diamond, `#38BDF8` @ 0.85, ~7 px, flashes for ~120 ms on acquiring a new snap point |
| Projection rays | 1 px dashed (`4 px` dash / `4 px` gap) from the snap point to the U and V axes |
| Dimension pill | `ΔU: 12.50 mm` / `ΔV: -4.00 mm`, existing `readout_pill` tokens (currently `rgba(15,18,24,0.8)`; the design intent is 0.75) |
| Origin proximity | the center node blooms within 5 px (see 3.1) |

WebGPU lines are 1 px solid, so a dashed ray is emitted as a run of short segments —
the same trick the ring polylines use.

---

## 4. Snapping

Today's signature is plane-blind: `snap_to_geometry(event, camera, grid: Option<f64>)`
and a hardcoded `z = 0` unprojection. The model replaces it with a context:

```rust
pub struct SnapContext {
    pub workplane: Workplane,
    pub minor: f64,
    pub major: f64,
}

pub fn snap_to_geometry(
    event: &PointerEvent,
    camera: &Camera,
    context: Option<&SnapContext>,
) -> Option<Snap>;
```

Priority, highest first:

1. **Feature snaps** — a picked vertex, edge midpoint, face centre, or the workplane
   origin. These extend `SnapKind` (`Vertex`, `EdgeMid`, `Centre`, `Origin`) and arrive
   with the richer pick (the existing `SnapKind::Point` is the fallback).
2. **Major grid intersections** on the workplane.
3. **Minor grid intersections** on the workplane, when the level survives the
   screen-space cull.
4. **Free point on the workplane** (the unprojected ray/plane intersection).

The result gains its local coordinates, so the readout can say `ΔU`/`ΔV` without
re-projecting:

```rust
pub struct Snap {
    pub point: [f32; 3],       // world
    pub kind: SnapKind,
    pub local: Option<[f64; 2]>, // (u, v) on the active workplane
}
```

**Gizmo snapping** stays world-component based for translation (a world grid is what a
translate wants) until sketch-constrained transforms need a local step; the doc records
that as deferred, not as a silent behaviour change.

---

## 5. Rendering, mapped to the contract

A grid is a **stage producer**, like any other item: it emits `DrawPacket`s into the
same queue, in the same pass, against the same depth buffer. It is not a second
renderer or a screen-space effect.

| Element | Topology | Notes |
| --- | --- | --- |
| Ground / workplane dots | `Points` | 1 px; per-dot alpha via the vertex colour (G0) |
| Major lines | `Lines` | 1 px; two vertices per segment, alpha interpolated |
| Local axes | triangles | 1.5 px ribbons (`screen_ribbon`) |
| Triad stems | triangles | 1.5 px ribbons |
| Labels, pill | gpui text layer | `readout_pill` / the navicube label path |
| Snap aurora | triangles | a small screen-facing diamond |

- **Not pickable.** The grid is emitted into its **own non-overlay queue**
  (`InventorViewport::grid_queue`) that is drawn in the same pass — so it shares the
  depth buffer — but is never handed to a pick path. Clicks fall through to geometry.
  (The earlier plan — the untagged `PickId(0)` plus a filter in `pick()` — was rejected
  once built: untagged scene packets also compile to `PickId(0)`, so the filter
discarded real hits.)
- **Depth.** The ground grid uses normal depth so geometry occludes it. A **face** grid
  is coplanar with the face, so it uses a positive `depth_bias` (the wire-over-surface
  mechanism) rather than being drawn as an overlay on top of everything.
- **Order.** One producer, ordered: dots → major lines → local axes → triad. It is not
  an overlay (`overlay: false`), so it participates in occlusion; the aurora and pill
  are overlays and are not.

### The one contract addition: per-vertex colour

The radial falloff needs alpha **per vertex**; `StateSnapshot.color` is per packet and
`Vertex` has no colour. **As built (G0)**, `Vertex` gained

```rust
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub part_idx: u32,
    pub color: [f32; 4],   // multiplied by the packet colour; [1,1,1,1] = no-op
}
```

`GpuVertex` carries it as attribute location 4 and `scene.wgsl` computes
`albedo = globals.color * input.color * textureSample(...)`. The pick shader keeps the
attribute in the layout (one vertex layout for both passes) and ignores it.
Producers that do not care emit white. **Rejected alternative:** concentric ring
batches with one alpha each — quantised, more packets, and it still misses the fade
along a line.

One consequence: the non-overlay **line** and **point** pipelines switched from
`REPLACE` to alpha blending (depth test/write unchanged) so the fade actually blends;
the triangle pipeline stays opaque.

---

## 6. Navicube and orientation

The navicube remains the **world camera compass**: its faces always mean world
`Front/Top/Right/…`, and a click snaps the camera to a world direction, exactly as
specified in [`navicube-spec.md`](navicube-spec.md). A local workplane never re-labels
the cube.

The compass ring is the one place a local frame may show through: when a workplane is
active, the ring's cardinal heading is rotated by the workplane's `u` projected onto
the world XY plane, so "heading left" on screen matches the sketch's `−U`. The ring is
a heading aid, not a second coordinate system. **Decision flag** (§10.5): whether the
ring rotates or gains a secondary inner tick set that shows both headings.

When the user clicks `TOP` while a workplane is active, the camera snaps to the
**world** top; a separate command (`View → Workplane`) is the way to look down the
active `n`. Keeping the cube world-only is deliberate: mixing the two would make
"which way is up" ambiguous.

---

## 7. Ownership

| Layer | Owns |
| --- | --- |
| `ferrocad_graphics` | `Vertex.color` (G0); `Topology::{Points, Lines}` already |
| `ferrocad_stage` | `Workplane`, `FaceFrame` (face → frame from the tessellation), `GridSpec`, `GridStyle`, `SnapContext`, the grid producer, the triad builder, `SnapKind` extensions. Pure math: no kernel, no gpui, no Python |
| `ferrocad_viewport` | building the frames from the display tessellation (`SelectionRef` → `WorkplaneKind::Face`), drawing the producer and the local layers, routing hover/clicks, the aurora and pill, `set_workplane`/`set_grid` |
| `ferrocad_gpui` / the edition | the **policy**: grid on/off, steps, units, sketch mode, the command that activates a datum plane. Preferences come from `App::ParamGet` |
| `ferrocad_part` and others | nothing — a workplane is a frame, not a shape |

Resolving a face frame uses only the tessellation already on the stage: triangle
centroids and normals. No kernel call, and no `ferrocad_part` dependency.

---

## 8. Slice plan

Each slice is independently testable. G0 is the only contract change; it goes first so
the visual work can use it.

| Slice | Deliverable | Acceptance |
| --- | --- | --- |
| **G0 — per-vertex colour** ✅ | `Vertex.color`; shader multiply; producers emit white; line/point blending | existing suites green; `per_vertex_colour_multiplies_the_packet_colour` reads back red/blue halves under a white packet colour |
| **G1 — vocabulary** ✅ | `Workplane` (+ kind), `GridSpec`/`GridStyle`, `SnapContext`; `snap_to_geometry` workplane-aware; `Snap.local`/`Snap.major` | unit tests: a tilted plane's unprojection and rounding, major-before-minor priority, `z = 0` world behaviour unchanged, local `(u, v)` of a snap |
| **G2 — world grid + triad** ✅ | `ferrocad_stage::grid` producer and triad builder; the viewport draws them from their own queue; pick-through | viewport: `grid_queue` gains `Points`/`Lines`/`Triangles`; a click over a dot resolves to nothing; the ground cache survives an orbit while the triad rebuilds; `set_grid` toggles it; the shell forwards `ViewState.grid` |
| **G3 — face workplane** ✅ | `SelectionRef` → `WorkplaneKind::Face`; hover blooms the face grid; activation/dimming; bounded extent; `depth_bias` | hovering a box face emits a local grid packet whose basis matches the face normal; activating it changes the snap plane; `Esc` clears |
| **G4 — snap feedback** ✅ | aurora, projection rays, `ΔU`/`ΔV` pill; origin proximity; feature snaps | dragging across the workplane shows an aurora at major nodes, rays to U/V, and the pill's numbers match `Snap.local` |
| **G5 — chrome + policy** ✅ | step/radius preferences, the workplane view command, ring-heading sync, origin labels | a preference change re-emits the grid without a restart; `View → Workplane` looks down `n`; the ring follows the active heading |

### G5 as built (2026-10-20) — policy and chrome

- **Preferences** — `ViewState` gained `grid_minor`, `grid_major` (mm) and `grid_radius`
  (mm; `0` means auto). `ViewRequest` carries them; `ViewState::apply` validates
  (positive, major ≥ minor) before mutating, and summarises (`grid minor 2, grid major
  20, grid radius 750`). The shell forwards them to `set_grid_steps` / `set_grid_radius`,
  which recompile the grid, so a change lands without a restart. `App_ReloadGridSettings`
  reads `User parameter:BaseApp/Preferences/View`, so a preference is a parameter write
  plus one command.
- **Workplane commands** — `App_ActivateWorkplane`, `App_ClearWorkplane`,
  `App_ViewWorkplane` return `{"view": {"workplane": …}}`. The shell performs the
  action on the viewport (`activate_hovered_workplane` / `clear_workplane` /
  `view_workplane`) and reports the outcome; without a viewport it reports
  `view: no viewport`. `view_workplane` snaps the camera to the plane's frame
  (`U` right, `V` up, `N` toward the eye) with the usual 220 ms animation.
- **Compass heading** — `NaviCube::set_heading_offset(Option<f64>)` rotates the
  cardinal letters (the ring geometry stays fixed, §10.5). The viewport derives the
  offset from the active plane's `U` in the ground plane, so a local frame turns the
  lettering; clearing the plane restores the world heading.
- **Axis labels** — `X`/`Y`/`Z` (or `U`/`V`/`N` on a local frame) as 10 px monospace text
  at the projected triad-tip points, in the axis hues, drawn in the same gpui text layer
  as the navicube labels.

### G4 as built (2026-10-20) — snap feedback

- **Feature snaps** — `SnapKind` gained `Vertex`, `EdgeMid`, `Centre`, `Origin`.
  `nearest_feature(camera, device, candidates, FEATURE_PIXELS)` projects each candidate and
  keeps the strictly nearest within 10 px; ties go to the earlier candidate, so the
  viewport orders them most-specific-first. `snap_to_geometry_with` tries features, then
  the picked point, then the grid. The candidates come from the **current hit** (not the
  stored hover — a tool is dispatched before the hover is published): the item's
  topological vertices, its tessellated edge midpoints (when the mesh is not dense:
  `MAX_EDGE_CANDIDATES`), its face centre via `FaceFrame::resolve`, and the workplane
  origin.
- **Aurora** — a screen-facing diamond (`AURORA_PIXELS = 7`) at the snapped point. On
  acquiring a **new** snap point it starts at `#38BDF8 @ 0.85` and decays to
  `AURORA_REST_ALPHA = 0.55` over `FLASH = 120 ms`; the render loop requests frames while
  the flash is pending.
- **Rays** — for a `Grid` snap only (the only kind that is on the plane by construction),
  dashed 4 px segments from the point to the `U` and `V` axes. A run shorter than a dash
  is drawn whole. They are emitted as overlay geometry (depth off), like the aurora.
- **Pill** — `snap_pill` in the viewport: two monospace rows (`ΔU: … mm`, `ΔV: … mm`) in
  the readout tokens, anchored on the projected snap point (not the cursor, so it does
  not jitter inside one cell). `snap_local()` exposes the value to the host.
- **Origin node** — a hollow, screen-facing diamond at the active workplane's origin (the
  world origin when none is active). It rests at `ORIGIN_ALPHA = 0.65` and blooms to
  `1.0` when the pointer is within `ORIGIN_PROXIMITY_PIXELS = 5` of the projected point.
  The marker rides in the grid queue (behind geometry), depth-biased when it is on a face.
- **Lifecycle** — the feedback lives while a tool is active; it is cleared by
  `set_tool(None)` and by the pointer leaving the viewport. A new snap point restarts the
  flash; moving within an unchanged snap does not.

**Deferred:** the feedback is tied to the tool session (the gizmo and manipulators have
 their own readouts); a standalone hover-snap readout would come with G5's policy.

### G3 as built (2026-10-20) — the face workplane

- **`ferrocad_stage::FaceFrame`** — `resolve(entity, element, positions, triangles, parts,
  part)` reads only the display tessellation: the part's triangles give an area-weighted
  centroid and normal; any facet more than ~2.6° off the mean (cos `< PLANAR_TOLERANCE`)
  rejects the part as curved; `u` is world `+X` projected into the plane (falling back to
  `+Y` when the face is normal to `X`), `v = n × u`; the grid radius is the widest
  in-plane vertex distance, grown by `FACE_MARGIN` (10 %). No kernel call.
- **Hover preview** — the viewport resolves the hovered `(entity, part)` into a frame on
  every pointer move (`refresh_hover_face`), keyed on the item's geometry generation so a
  reshaped item re-resolves without resetting the dwell. The grid **blooms after
  `FACE_DWELL` (120 ms)**; the immediate feedback is the hover report, as decided in §10.3.
  The render loop requests animation frames while the dwell is pending, so the bloom
  arrives without further pointer motion.
- **Layers** — `GridLayer` gained a `depth_bias`; `GridMesh` now owns the world layer plus
  an **active** and a **hover** layer. A local layer uses 1 mm dots / 10 mm lines
  (`GridStyle::workplane`), its face's radius, and `WIRE_DEPTH_BIAS` so it sits on the
  surface instead of z-fighting with it. While a local layer exists the world ground drops
  to `WORLD_DIM` (25 %) and the world triad to `WORLD_TRIAD_DIM` (30 %).
- **Activation** — `set_workplane(Some(FaceFrame))` / `activate_hovered_workplane()` /
  `clear_workplane()`. The plane is **sticky** (§10.4): it survives pointer moves and
  empty-space clicks. `Esc` clears it (after it has had the chance to cancel a manipulator
  or gizmo drag). Snapping switches to `SnapContext { minor: 1, major: 10 }` on the active
  plane; the world grid snapping step applies when none is active.
- **Caching** — the ground and triad keys now include the ink. A dimmed grid bakes
  different per-vertex alpha, so it is different geometry, not a different packet colour;
  without this the world level never receded.

**Deferred:** the command that activates a face and the ring-heading sync belong to G5;
until then a host calls `activate_hovered_workplane()` (a click handler or a command).
The active face itself is never re-previewed.

### G0–G2 as built (2026-10-20)

- **`ferrocad_graphics`** — `Vertex.color: [f32; 4]` (linear), default white.
- **`ferrocad_stage::workplane`** — `Workplane { origin, u, v, n, kind }` with
  `world()`, `new()` (normalises, forces a right-handed basis, rejects degenerate
  axes), `point(a, b)`, `project(world) -> [u, v]` and `ray_local(origin, direction)`.
- **`ferrocad_stage::snap`** — `SnapContext { workplane, minor, major }`, whose
  `world(minor)` is the `z = 0` case; `Snap { point, kind, local, major }`.
- **`ferrocad_stage::grid`** — pure arithmetic, no kernel/gpui/Python:
  - `MINOR_STEP = 10`, `MAJOR_STEP = 100`, `TRIAD_PIXELS = 80`,
    `RADIUS_RANGE = (500, 5_000)`, `MINOR_PIXELS = 4`, `MAX_DOTS = 40_000`.
  - `ground(spec, style)` — dots (skipping major lines) plus major lines subdivided at
    the minor step, alpha `α₀ · max(0, 1 − d / R)` per vertex.
  - `triad(workplane, camera, dim)` — three screen-facing ribbons via `screen_ribbon`,
    axis colours at 65 %, scaled by `dim`.
  - `effective_steps` — drops the minor level below 4 projected px, promoting a decade.
  - **Unlit by construction:** every vertex's normal is the shader's light direction,
    so the diffuse term is exactly 1 and the per-vertex colour reaches the screen.
- **`ferrocad_viewport`** — `GridMesh` (now three `GridLayer`s since G3) owns nine mesh
  ids (allocated from a private counter) and two cache keys per layer: the ground is
  keyed on the `GridSpec` and its ink (camera-independent, so an orbit does not rebuild
  it), the triad on the camera's view/projection, device height and alpha. `set_grid` /
  `grid_enabled` toggle it; `sync_grid` runs on every `recompile` and every paint.
- **`ferrocad_gpui`** — `apply_view` forwards `ViewState.grid` to the viewport, and
  `enable_viewport` applies the current state at creation.

---

## 9. Non-goals

- **No infinite shader grid.** The producer generates a disc around the pivot; "infinite"
  is an appearance, achieved by regeneration and the radial fade.
- **No curved-face grids.** A cylindrical face gets no planar UV field in this design;
  only planar faces and datums.
- **No second renderer.** Grids are packets in the existing pass, not a screen-space
  compositing layer.
- **No navicube relabelling.** The cube stays world; only the ring's heading may track a
  local frame.
- **No kernel dependency.** A workplane is an origin and a basis; resolving a face frame
  reads the tessellation.

---

## 10. Decisions to settle

**Settled by G0–G5:** 0 (Z-up), 1 (per-vertex colour), 2 (1 px points, with the 4 px
culling rule), 3 (the 120 ms hover dwell, with an immediate hover report), 4 (sticky
until `Esc` or another activation), 5 (rotate the compass letters, keep the ring fixed),
6 (`2.5 ×` eye distance, clamped 500–5 000 mm, overridable by the `grid_radius`
preference; the cache key is the exact spec, so a zoom rebuilds and an orbit does not),
7 (the world grid drops to 25 % under a local plane; its triad to 30 %). The rest remain
open.

0. **World up-axis.** The grid model assumes **Z-up** (`XY` ground), consistent with
   `snap_to_geometry`'s `z = 0`, the toy's `Height`, and OCCT. `navicube-spec.md` §2
   describes the default *view* as “+Z toward the viewer, +Y up”; that must be read as
   the initial camera orientation, not the world frame. If the cube's piece naming truly
   assumes a Y-up world, the two notes must be reconciled before G1 — the grid must not
   silently introduce a second convention.
1. **Per-vertex colour vs ring batches.** Recommend per-vertex (G0): exact fade, one
   attribute, useful elsewhere. Cost: a vertex-layout change in `ferrocad_wgpu` and the
   pick shader.
2. **Point rendering.** 1 px points meet "single-pixel dots"; if the design wants a 2 px
   dot, use screen-facing quads and accept the extra triangles. Recommend 1 px + the
   4 px culling rule.
3. **Hover latency for the face grid.** Instant feels responsive but flickers while
   crossing edges; 120 ms matches the aurora's flash. Recommend a 120 ms dwell with an
   immediate *highlight*.
4. **Workplane stickiness.** Recommend sticky until `Esc` or a new face/datum click;
   never auto-clear on empty space (an accidental click must not drop the sketch).
5. **Compass ring with a local plane.** Rotate the ring, or add a secondary tick set?
   Recommend rotating the cardinal labels, keeping the ring geometry fixed.
6. **`R_max` and regeneration.** Recommend `2.5 × camera distance`, clamped to 0.5–5 m,
   regenerated at 10 % pivot movement.
7. **World grid under a local plane.** Recommend visible at 25 % alpha; a fully hidden
   world grid loses the global anchor.

---

## 11. Verification

Once built, the note's guarantees are checked at three levels, matching the existing
pattern (G0–G2 are built; the entries for them are live tests):

- **`ferrocad_stage` (pure math)** — workplane construction (right-handedness), snap
  unprojection on a tilted plane, major/minor priority, screen-space culling, the grid
  generator's segment counts and per-vertex alpha at `d = 0`, `d = R_max`.
- **`ferrocad_viewport` (headless)** — the grid's packets (topology, ids, separate
  queue, pick-through, the two cache keys), `set_grid`, and the face workplane: hover
  resolution from a named slab, the dwell, the bloom's plane and depth bias, the world
  dimming, activation changing the snap context, and `Esc`. G4 adds the aurora's flash and
  settle, a vertex snap, a grid snap's rays and local coordinates, and the origin bloom.
- **`ferrocad_wgpu`** — the per-vertex colour multiply (a readback of a two-colour
  triangle), and the grid's line/point primitives in a real pass.
- **`ferrocad_gpui`** — `ViewState::apply` validation and summaries, `App_ToggleGrid` /
  `App_ReloadGridSettings` reaching the viewport (steps, radius), and a workplane action
  reporting cleanly without a viewport.

The visual language is not unit-testable beyond colour tokens, spacing and ordering;
those are asserted, and the rest stays a spec.
