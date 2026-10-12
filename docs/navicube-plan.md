# Making the navigation cube producer-independent — plan

Status: **plan** (2026-10-12), revised. Companions: [`navicube-spec.md`](navicube-spec.md)
(the normative spec this delivers), [`navicube.md`](navicube.md) (the as-built record,
N1–N4b), [`stage.md`](stage.md) (the modern stack the second producer targets), and
[`inventor-renderer.md`](inventor-renderer.md) (the render stack).

The cube exists and works (`ferrocad_viewport::navicube` + `ferrocad_inventor::navicube`),
but it is welded to the legacy producer: the controller owns a `Scene`, compiles it, and
takes its geometry, ids and colours from `ferrocad_inventor`. This plan makes the *model*
producer-neutral, the *controller* agnostic to its producer, and adds a second producer
for the modern stack — so a NaviCube can be built from either `pivy.coin` nodes or
`ferrocad_stage` items.

## 1. Goal and non-goals

**Goal.** One NaviCube behaviour, two producers, one controller, per
[`navicube-spec.md`](navicube-spec.md):

- a **model** (taxonomy, ids, geometry, appearance **roles**, camera, config);
- a **`NavicubeView`** seam and a producer-neutral **visual state**;
- an appearance **theme** seam so styling is a theming concern over the draw-batch
  primitives (`ferrocad_graphics`), not the widget's ([`navicube-spec.md`](navicube-spec.md) §5);
- an **inventor producer** (as today, moved in) and a **stage producer** (new);
- the whole widget in **`ferrocad_viewport::navicube`**, with **no** dependency on
  `ferrocad_wgpu` or `pyo3`.

**Non-goals.** No new node kinds, no draw-contract fields, no new crate, no behaviour
change to the existing cube (V1 is a pure refactor). The arrow buttons, context menu,
`Backside`/`ViewMenu`, `stepByTurn`, rotate-to-nearest rounding and draggable-cube
movement stay deferred ([`navicube-spec.md`](navicube-spec.md) §12). Upstream-fidelity
gaps the current build has (chamfer geometry, animation timing, orbit, placement) are
catalogued in [`navicube-spec.md`](navicube-spec.md) §14 and are **not** part of V1–V3;
palette and label *rendering* are theme choices, not fidelity gaps (§13).

## 2. Where it sits

A NaviCube is a **viewer-level widget**: it is a corner overlay that *consumes* a scene
stack (it builds a cube out of `SoSeparator`s or out of `ShapeItem`s). It is therefore
**not** a low-level primitive and does not belong beneath the scene stacks — it belongs
**on top of** them, and `ferrocad_viewport` already depends on both `ferrocad_inventor`
and `ferrocad_stage`. So the whole widget lives in one module tree in the viewport:

```
ferrocad_viewport::navicube   ── the NaviCube widget: sits ON TOP of both scene stacks
  ├── model     Piece / NaviTarget / geometry / roles / camera / config               [pure]
  ├── view      NavicubeVisual + the NavicubeView seam + the NavicubeTheme mapping      [pure]
  ├── inventor  builds a `Scene` → compile_with        ─┐  two producers for one model;
  ├── stage     builds `ShapeItem`s → emit into a queue ─┘  both are viewport-local
  └── NaviCube  the controller: Box<dyn NavicubeView> (placement, sync, hit, snap, orbit)
        │
        └─ uses ferrocad_inventor + ferrocad_stage + ferrocad_graphics
                    │
              ferrocad_wgpu  (one overlay pass, both producers)
```

`ferrocad_inventor` and `ferrocad_stage` stay **unaware** of the NaviCube; the viewport
(which already depends on them) is the only crate that assembles it. Nothing outside
`ferrocad_viewport` consumes `ferrocad_inventor::navicube` today, so that module moves to
the viewport rather than staying behind a new crate.

## 3. Decisions to ratify

1. **Home: `ferrocad_viewport::navicube` (one module tree).** Because the widget *sits on
   top of* the scene stacks and both producers are viewport-local, the shared model does
   **not** need to live below the producers — so no `ferrocad_navicube` leaf crate, and no
   inverted dependency. `ferrocad_inventor::navicube` moves into the viewport
   (`ferrocad_inventor` loses the module; `ferrocad_stage` gains nothing). *If a
   non-gpui consumer of the model ever appears (e.g. a headless `SoNaviCube` for
   `ferrocad_inventor_py`), extract the `model`/`view` modules into a leaf crate then.*
2. **The controller holds a `Box<dyn NavicubeView>`.** The seam is object-safe, so the
   viewport stays non-generic (`InventorViewport` keeps `Option<NaviCube>`) rather than
   threading a type parameter through the host. *Alternative given both producers are
   local: an `enum Producer { Inventor(…), Stage(…), Custom(Box<dyn …>) }`; the trait
   object is preferred for a clean capability seam (`has_labels`) and test doubles.*
3. **Labels are producer-gated.** `has_labels()` returns `false` for the stage producer
   in V1/V2 (no per-item texture path in `ShapeItem`); conformance compares geometry, ids
   and **roles**, not labels or resolved colours ([`navicube-spec.md`](navicube-spec.md) §11).
4. **Both producers are viewport-local.** `NavicubeView` is the internal seam between the
   controller and the two producers; it is not a public extension point.
5. **Appearance is themed, not hard-coded.** Producers tag each piece with a
   appearance **role** (base / highlighted / away) and resolve it through a
   `NavicubeTheme`; the widget owns no colours ([`navicube-spec.md`](navicube-spec.md) §5).
   The first theme reproduces the current palette, so V1 stays a no-behaviour-change
   refactor.

## 4. Slices

### V1 — move the model in, add the seam, genericize the controller (no behaviour change)

Deliverable: the cube behaves exactly as today, but the model, the producers and the
controller are one viewport module tree and the controller drives a trait object.

- Turn `ferrocad_viewport/src/navicube.rs` into a directory module
  `ferrocad_viewport/src/navicube/`:
  - `model`: `Piece`, `PieceKind`, `NaviTarget`, `Piece::ALL`/constants, the geometry
    generators (`quad`/`segment`/`corner_cube`/`home_quad`), the **`NavicubeRole` rule**,
    `camera(rect)`, `Corner`, `NaviCubeConfig`, `home_orientation` — moved in from
    `ferrocad_inventor::navicube`.
  - `view`: `NavicubeVisual { orientation, hilite, away }`, `NavicubeRole`, the
    `NavicubeTheme` mapping, and the `NavicubeView` trait (whose `rebuild` takes a theme).
  - `inventor`: `InventorProducer` — the existing `Scene` + `RenderQueue` + `synced` cache
    (`scene`/`scene_with`/`set_orientation`), now implementing `NavicubeView`.
  - `mod`: the `NaviCube` controller (placement, camera sync, hit-test, hilite, snap,
    orbit) holding a `Box<dyn NavicubeView>` defaulting to `InventorProducer`.
- Delete `ferrocad_inventor/src/navicube.rs` and its `lib.rs` module/re-export; move its
  unit tests with it. `ferrocad_viewport/src/inventor_view.rs` keeps its imports via
  `crate::navicube::…`.
- `InventorViewport` is unchanged externally; the shell/example
  (`ferrocad_gpui`) `NaviCube::new(viewport)` call is unchanged.

**Gate:** every existing test passes unchanged (behaviour identical); new model tests
(ids round-trip, orientation, geometry constants) and a trait-object controller test; the
navicube test count moves from `ferrocad_inventor` to `ferrocad_viewport`.

### V2 — the stage producer + conformance

Deliverable: a NaviCube the modern stack can build, driven by the same controller.

- `navicube::stage`: `StageProducer` implements `NavicubeView` by building `ShapeItem`s
  from the model geometry (`ShapeItem::new(entity_id = piece.pick_id().0)`,
  `DrawMode::Shaded` for faces/corners, `DrawMode::Wireframe` for edges), assigning each
  piece its [`navicube-spec.md`](navicube-spec.md) §5 role and resolving it through the
  theme, and baking the orientation into each item's placement (no `Transform` node in the
  stage). `has_labels() == false`.
- The controller gains a way to pick the producer (`NaviCube::with_backend(Box<dyn
  NavicubeView>)`, or a `NaviCube::stage(…)` / `NaviCube::inventor(…)` pair).
- A **conformance test** in `ferrocad_viewport` drives both producers with the same
  `NavicubeVisual` and asserts the spec's §11 items: 26 pieces + Home with the right ids,
  identical geometry, per-piece **roles** (highlighted/away/base), and reuse of
  `(mesh_id, generation)` on an unchanged rebuild.

**Gate:** the stage cube renders headlessly in a viewport; conformance passes; existing
tests pass.

### V3 — config and polish

Deliverable: the reserved config becomes real, and the specified thresholds are enforced.

- Apply `NaviCubeConfig` (corner, size, margin, `chamfer`, thresholds) to the controller
  and producers; add a `NaviCube::with_config`.
- Formalise `NavicubeTheme` (role→appearance) with the current palette as the default
  theme, in the theming layer over `ferrocad_graphics`; add a `NaviCube::with_theme`.
- Enforce the **min-drag threshold** (`3 logical px × dpr`) in the click-vs-drag decision
  ([`navicube-spec.md`](navicube-spec.md) §8, §14) — the build currently accumulates
  movement but never gates on it.
- Add `draggable` and `rotate_to_nearest`; warn (once) when no producer is available.
- Docs: cross-link `stage.md` (T6) and `inventor-renderer.md` §7.

## 5. Risks and pitfalls

- **Stage mesh churn.** `ShapeItem::set_shape` bumps `Generation`; the stage producer must
  set each piece's geometry **once** and realise hover/orientation/away by mutating only
  `placement`/`temporary_transform` and `set_surface_color` (both generation-free), or the
  cache re-uploads every hover. Warm the items once and re-emit.
- **No `Transform` in the stage.** The rotating subtree must become "compose the view
  rotation into every piece's placement" — mirroring the inventor producer's single
  `Transform` node; the Home quad stays fixed (a sibling).
- **Mesh-id collisions.** The inventor compiler numbers meshes `0..n` per compile; the
  stage producer's `ShapeItem`s start at `1 << 40`, so the shared upload cache stays
  disjoint (already true — keep it).
- **Object-safety.** `NavicubeView` must not gain generics or `Self`-typed returns, or the
  `Box<dyn …>` controller stops compiling.
- **Recompile discipline.** The controller already rebuilds only when the visual changed;
  preserve that for both producers (a hover must not recompile the main scene).
- **The move touches `ferrocad_inventor`.** `ferrocad_inventor/src/navicube.rs` is deleted
  and its tests move to `ferrocad_viewport`; update the doc counts in
  [`inventor-renderer.md`](inventor-renderer.md) §6 and [`navicube.md`](navicube.md) §7.

## 6. Verification

| Slice | Checks |
| --- | --- |
| **V1** | the moved `navicube` tests pass in `ferrocad_viewport` (behaviour unchanged); new model tests (ids, orientation, geometry); a trait-object controller test; `ferrocad_inventor` compiles without the module. |
| **V2** | the conformance test (both producers, spec §11); a headless render of the stage cube; `StageProducer` unit tests. |
| **V3** | config tests (corner/size, thresholds), a theme test, the min-drag threshold test, a warning test. |

Runs: `cargo test -p ferrocad_viewport -p ferrocad_inventor -p ferrocad_stage`
(plus `-p ferrocad_wgpu` headless). The navicube tests shift crate (out of the current
inventor 14, into the current viewport 19); the other counts (stage 27, wgpu 7) must not
regress.

## 7. Slice path at a glance

| Slice | Deliverable | Where | Status |
| --- | --- | --- | --- |
| **V1** | Move the model + inventor producer into `ferrocad_viewport::navicube`; add the `NavicubeView` seam and `NavicubeVisual`; the controller drives `Box<dyn NavicubeView>`. No behaviour change. | `ferrocad_viewport`, `ferrocad_inventor` | ▢ |
| **V2** | `navicube::stage::StageProducer` + the producer-conformance test. | `ferrocad_viewport` | ▢ |
| **V3** | `NaviCubeConfig`, `NavicubeTheme`, the min-drag threshold, `draggable`/`rotate_to_nearest`, docs. | `ferrocad_viewport`, docs | ▢ |
