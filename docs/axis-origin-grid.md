# Axis, origin and grids: the spatial reference model

Status: design (2026-10-20). Companion to [`gizmo-manipulators.md`](gizmo-manipulators.md),
[`navicube-spec.md`](navicube-spec.md), [`stage.md`](stage.md) and
[`architecture.md`](architecture.md) §10.

This note defines **one** model for the things that tell a user where they are: the
world triad, the ground grid, local workplanes, face grids, and the snap feedback that
ties them together. It records the visual language, the data model, the render
mapping, and the slice path. Nothing here is built yet except where marked.

---

## 1. What exists today

| Piece | State |
| --- | --- |
| `Topology::Points` / `Topology::Lines` | ✅ in the draw contract (`ferrocad_graphics`), rendered and picked by `ferrocad_wgpu` as **1 px** primitives |
| 1.5 px strokes | ✅ as **ribbons** (triangles) via `screen_ribbon`/`push_ribbon`; WebGPU has no line width, so the gizmo and the dress-up arc already do this |
| Grid snapping | ◐ `snap_to_geometry(event, camera, grid: Option<f64>)` — unprojects onto the **world `z = 0` plane**, rounds to the step. `SnapKind::{Point, Grid}` |
| Gizmo linear snap | ✅ `set_linear_snap(step)` snaps world components (T3) |
| Numeric pill | ✅ `readout_pill` in the viewport: 11 px monospace, `rgba(15,18,24,0.8)`, 1 px `#FFFFFF14` border |
| Depth-bias overlays | ✅ `StateSnapshot.depth_bias`; `WIRE_DEPTH_BIAS` for a coplanar wire over a face |
| Navigation cube | ✅ world-oriented; six faces + corners/edges, a compass ring, labels |
| Grid rendering | ▢ **none** |
| Workplane / UCS | ▢ **none** |
| Per-vertex colour/alpha | ▢ **none** — colour is per packet (`StateSnapshot.color`) |

Three gaps drive the plan: there is no grid producer, snapping knows only the world
ground plane, and a smooth radial fade needs a per-vertex alpha the vertex layout does
not carry.

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

- **Not pickable.** `PickId(0)` and `selectable(false)`; the GPU pick writes the
  untagged id, which resolves to `Cleared`. Clicks fall through to geometry.
- **Depth.** The ground grid uses normal depth so geometry occludes it. A **face** grid
  is coplanar with the face, so it uses a positive `depth_bias` (the wire-over-surface
  mechanism) rather than being drawn as an overlay on top of everything.
- **Order.** One producer, ordered: dots → major lines → local axes → triad. It is not
  an overlay (`overlay: false`), so it participates in occlusion; the aurora and pill
  are overlays and are not.

### The one contract addition: per-vertex colour

The radial falloff needs alpha **per vertex**; `StateSnapshot.color` is per packet and
`Vertex` has no colour. The plan adds

```rust
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub part_idx: u32,
    pub color: [f32; 4],   // NEW: multiplied by the packet colour; [1,1,1,1] = no-op
}
```

and multiplies it into the fragment colour in both render and pick shaders. This is
generic (per-face tint, gradients, the grid fade) and cheap: one `Float32x4` attribute.
Producers that do not care emit white. **Rejected alternative:** concentric ring
batches with one alpha each — quantised, more packets, and it still misses the fade
along a line.

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
| `ferrocad_stage` | `Workplane`, `GridSpec`, `GridStyle`, `SnapContext`, the grid producer, the triad builder, `SnapKind` extensions. Pure math: no kernel, no gpui, no Python |
| `ferrocad_viewport` | building the frames from the display tessellation (`SelectionRef` → `WorkplaneKind::Face`), drawing the producer, routing hover/clicks, the aurora and pill, `set_workplane`/`set_grid` |
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
| **G0 — per-vertex colour** | `Vertex.color`; shader multiply in render + pick; producers emit white | existing suites green; a two-colour triangle renders (readback test); the pick pass still returns the same ids |
| **G1 — vocabulary** | `Workplane` (+ kind), `GridSpec`/`GridStyle`, `SnapContext`; `snap_to_geometry` becomes workplane-aware; `Snap.local` | unit tests: a tilted plane's unprojection and rounding, major-before-minor priority, `z = 0` world behaviour unchanged, local `(u, v)` of a snap |
| **G2 — world grid + triad** | `ferrocad_stage::grid` producer and triad builder; the viewport draws them; labels; depth; pick-through | a viewport test: the queue gains `Points`/`Lines` packets with `PickId(0)`; a click over a dot still selects the object behind it |
| **G3 — face workplane** | `SelectionRef` → `WorkplaneKind::Face`; hover blooms the face grid; activation/dimming; bounded extent; `depth_bias` | hovering a box face emits a local grid packet whose basis matches the face normal; activating it changes the snap plane; `Esc` clears |
| **G4 — snap feedback** | aurora, projection rays, `ΔU`/`ΔV` pill; origin proximity; feature snaps | dragging across the workplane shows an aurora at major nodes, rays to U/V, and the pill's numbers match `Snap.local` |
| **G5 — chrome + policy** | grid on/off and step preferences, the workplane view command, ring-heading sync; the toolbar toggle from the actions slice | a preference change re-emits the grid without a restart; `View → Workplane` looks down `n`; the ring follows the active heading |

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
pattern:

- **`ferrocad_stage` (pure math)** — workplane construction (right-handedness), snap
  unprojection on a tilted plane, major/minor priority, screen-space culling, the grid
  generator's segment counts and per-vertex alpha at `d = 0`, `d = R_max`.
- **`ferrocad_viewport` (headless)** — the grid's packets (topology, ids, depth bias),
  pick-through, face-frame resolution from a tessellated box, the aurora/pill on a
  synthetic drag.
- **`ferrocad_wgpu`** — the per-vertex colour multiply (a readback of a two-colour
  triangle), and the grid's line/point primitives in a real pass.

The visual language is not unit-testable beyond colour tokens, spacing and ordering;
those are asserted, and the rest stays a spec.
