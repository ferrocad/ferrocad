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
- an appearance **theme** — a viewport-level policy that assigns colour/width/opacity to
  the draw primitives **through the producer API** (`SoMaterial`/`SoDrawStyle`, or
  `ShapeItem` colours/`edge_width`), not the widget's ([`navicube-spec.md`](navicube-spec.md) §5);
- an **inventor producer** (as today, moved in) and a **stage producer** (new);
- the whole widget in **`ferrocad_viewport::navicube`**, with **no** dependency on
  `ferrocad_wgpu` or `pyo3`.

**Non-goals.** No new node kinds, no draw-contract fields, no new crate, no behaviour
change to the existing cube (V1 is a pure refactor). The arrow buttons, context menu,
`Backside`/`ViewMenu`, `stepByTurn`, rotate-to-nearest rounding and draggable-cube
movement stay deferred ([`navicube-spec.md`](navicube-spec.md) §12). Upstream-fidelity
gaps the current build has (chamfer geometry, animation timing, orbit, placement) are
catalogued in [`navicube-spec.md`](navicube-spec.md) §14 and are **not** part of V1–V4;
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
  ├── view      NavicubeVisual + the NavicubeView seam (theme passed in)              [pure]
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
5. **Appearance is themed at the viewport level.** The primitives already carry colour
   (and geometry a width); a **theme** is the higher-level policy that assigns them. It
   lives at the viewport level and flows **through the producer API** — the inventor
   producer sets `SoMaterial`/`SoDrawStyle`, the stage producer sets `ShapeItem`
   colours/`edge_width` ([`navicube-spec.md`](navicube-spec.md) §5). Producers tag each
   piece with a role (base / highlighted / away) and the theme resolves it. The first theme
   reproduces the current palette, so V1 stays a no-behaviour-change refactor.

## 4. Slices

### V1 — move the model in, add the seam, genericize the controller (no behaviour change) ✅ built 2026-10-12

Deliverable: the cube behaves exactly as today, but the model, the producers and the
controller are one viewport module tree and the controller drives a trait object.

- Turn `ferrocad_viewport/src/navicube.rs` into a module root beside a
  `ferrocad_viewport/src/navicube/` submodule directory:
  - `model`: `Piece`, `PieceKind`, `NaviTarget`, `Piece::ALL`/constants, the geometry
    generators (`quad`/`segment`/`corner_cube`), `camera(rect)`, `Corner`,
    `home_orientation` — moved in from `ferrocad_inventor::navicube`.
  - `view`: `NavicubeVisual { orientation, hilite, away }` and the `NavicubeView` trait.
    (The appearance roles and the viewport theme land in V3; V1 keeps the producer's
    colours.)
  - `inventor`: `InventorProducer` — `scene_with`/`set_orientation` compiled to a
    `RenderQueue`, now implementing `NavicubeView`.
  - `mod` (the `navicube.rs` root): the `NaviCube` controller (placement, camera sync,
    hit-test, hilite, snap, orbit) holding a `Box<dyn NavicubeView>` defaulting to
    `InventorProducer`.
- Delete `ferrocad_inventor/src/navicube.rs` and its `lib.rs` module; move its unit
  tests with it. `ferrocad_viewport/src/inventor_view.rs` keeps its imports via
  `crate::navicube::…`.
- `InventorViewport` is unchanged externally; the shell/example
  (`ferrocad_gpui`) `NaviCube::new(viewport)` call is unchanged.

**Gate:** every existing test passes unchanged (behaviour identical); new model tests
(ids round-trip, orientation, geometry constants) and a trait-object controller test; the
navicube test count moves from `ferrocad_inventor` to `ferrocad_viewport`.

**Built.** `model.rs`/`inventor.rs`/`view.rs` live under the `navicube.rs` root; the
controller drives `Box<dyn NavicubeView>`. Tests moved with the code: `ferrocad_inventor`
**14 → 6**, `ferrocad_viewport` **19 → 27**, no behaviour change. `NavicubeVisual.hilite`
is an `Option<NaviTarget>` (the producer can highlight the Home button, though the
controller only hovers pieces today, as `inventor_view` filters to `Piece`).

### V2 — the stage producer + conformance ✅ built 2026-10-12

Deliverable: a NaviCube the modern stack can build, driven by the same controller.

- `navicube::stage`: `StageProducer` implements `NavicubeView` by building the 26 pieces
  + Home as `ShapeItem`s **once** (`ShapeItem::new(piece.pick_id().0)`, `DrawMode::Shaded`
  for faces/corners, `DrawMode::Wireframe` for edges), then on each rebuild recolouring
  them per its [`navicube-spec.md`](navicube-spec.md) §5 role and baking the orientation
  into each piece's transform (no `Transform` node in the stage; Home stays fixed). The
  items are built once so their mesh ids are stable — a rebuild never re-uploads.
  `has_labels() == false`.
- The controller picks its producer: `NaviCube::with_producer(Box<dyn NavicubeView>)`, with
  `NaviCube::new` (inventor) and `NaviCube::stage` (stage) as the two conveniences.
- A **conformance test** (`navicube::conformance`) drives both producers with the same
  `NavicubeVisual` and asserts the spec's §11 items: 26 pieces + Home with the right ids
  and draw order, identical geometry (positions/normals/indices) and transforms, the §5
  per-piece roles, and stable `(mesh_id, generation)` across a rebuild.

**Gate:** the stage cube renders headlessly in a viewport; conformance passes; existing
tests pass.

**Built.** `navicube/stage.rs` holds `StageProducer`; `NaviCube::{with_producer, stage}`
wire it in. The conformance suite (`navicube/conformance.rs`) passes for both producers.
`ferrocad_viewport` is now **32** tests (was 27). No theme yet — V2 assigns the model's
role colours directly; V3 threads the viewport theme through.

### V3 — config and the viewport theme ✅ built 2026-10-12

Deliverable: the reserved config becomes real, and the specified threshold is enforced.

- `navicube::theme`: `NavicubeRole` (the semantic role a piece takes) + `NavicubeTheme`
  (the role→appearance mapping; default = the built palette; `edge_width` recorded).
  `NavicubeView::rebuild` takes `&NavicubeTheme`, and both producers resolve roles through
  it (inventor `Material`, stage `ShapeItem` colours) — styling leaves the producers; the
  Rust core owns it ([`directives.md`](directives.md) §5).
- `NaviCubeConfig { corner, size, margin }` + `NaviCube::with_config` / `with_theme`.
- The **min-drag threshold** (`max(3, round(3·dpr))` logical px) gates the click-vs-drag
  decision: a shorter movement is a snap on release, and an orbit no longer also snaps
  ([`navicube-spec.md`](navicube-spec.md) §8, §14).

`draggable`, `rotate_to_nearest` / `step_by_turn`, a `borderWidth`/opacity appearance
override, and the `showCS` axes stay deferred ([`navicube-spec.md`](navicube-spec.md) §12).

**Built.** `theme.rs` + `NaviCubeConfig` in the controller; the min-drag gate in
`inventor_view::{navicube_drag_move, handle_button}`. `ferrocad_viewport` is now **37**
tests (was 32): a custom theme recolours both producers, and a config repositions the cube.

### V4 — the directives channel: `SoNaviCube` → neutral model → producers

Deliverable: a `SoNaviCube` scene node drives the neutral navicube on **either** producer,
through the neutral directives channel ([`directives.md`](directives.md)). Split into three
sub-slices because it spans four crates.

**V4.1 — the channel ✅ built 2026-10-12**

- `ferrocad_graphics`: `Directives` + `Directive { NaviCube(NaviCubeDirective) }` and
  `NaviCubeDirective { rect, orientation: Option<Rotation>, hilite: Option<PickId>,
  orthographic }`; `compile`/`compile_with` return `(RenderQueue, Directives)`.
- `ferrocad_inventor`: `NodeKind::NaviCube(NaviCubeDirective)`; the compiler lifts it into
  the directives and emits **no** packet. (`Directive::EventCallback` joins with V4.2.)
  A test asserts a navicube node compiles to zero packets and one directive.

**V4.2 — the viewport consumes it.** The viewport hosts an overlay from each
`Directive::NaviCube` (`NaviCubeDirective` + theme → `NavicubeVisual` + `Rect` → the
producer → overlay `ViewSpec`), and the `SoEventCallback` path migrates from
`event_callbacks(scene)` into `Directive::EventCallback` (threading the directives through
the `EventListener` API).

**V4.3 — the `pivy.coin` surface.** `ferrocad_inventor_py::extract_scene` recognises a
Python `SoNaviCube` and captures its `SoSF*` fields; the node's colours are ignored
([`directives.md`](directives.md) §5).

**Gate:** a `SoNaviCube` in the scene drives the stage producer (structural); the
`TestCoinNodeSnapshots` NaviCube fixtures *render*, with pixel-conformance gated on the
§14 fidelity backlog.

Deferred: `Gizmo`/`Hud` variants, a per-directive appearance override, and `showCS`.

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
- **Edge width is not honoured yet.** A theme can assign a line/edge *width*, but the
  renderer draws fixed 1-px lines: `ShapeItem::edge_width` is stored-but-unconsumed and
  `SoDrawStyle` line width is dropped by `ferrocad_wgpu`'s line pipeline. Theming colour
  works today; theming `edge_width` needs a separate `ferrocad_wgpu` slice (a wide-line
  pipeline or quad-expansion) before it has any visible effect.

## 6. Verification

| Slice | Checks |
| --- | --- |
| **V1** | the moved `navicube` tests pass in `ferrocad_viewport` (behaviour unchanged); new model tests (ids, orientation, geometry); a trait-object controller test; `ferrocad_inventor` compiles without the module. |
| **V2** | the conformance test (both producers, spec §11); a headless render of the stage cube; `StageProducer` unit tests. |
| **V3** | config tests (corner/size, thresholds), a theme test, the min-drag threshold test, a warning test. |

Runs: `cargo test -p ferrocad_viewport -p ferrocad_inventor -p ferrocad_stage`
(plus `-p ferrocad_wgpu` headless). After V1/V2/V3 the navicube tests live in
`ferrocad_viewport` (**19 → 37**); `ferrocad_inventor` drops to **6**. The other counts
(stage 27, wgpu 7) must not regress.

## 7. Slice path at a glance

| Slice | Deliverable | Where | Status |
| --- | --- | --- | --- |
| **V1** | Move the model + inventor producer into `ferrocad_viewport::navicube`; add the `NavicubeView` seam and `NavicubeVisual`; the controller drives `Box<dyn NavicubeView>`. No behaviour change. | `ferrocad_viewport`, `ferrocad_inventor` | ✅ |
| **V2** | `navicube::stage::StageProducer` + the producer-conformance test. | `ferrocad_viewport` | ✅ |
| **V3** | `NaviCubeConfig`, the viewport `NavicubeTheme`, the min-drag threshold, `draggable`/`rotate_to_nearest`, docs. | `ferrocad_viewport`, docs | ✅ |
| **V4** | The neutral `Directives` channel in `ferrocad_graphics`; `SoNaviCube → NaviCubeDirective → producers` (see [`directives.md`](directives.md)). | `ferrocad_graphics`, `ferrocad_inventor`, `ferrocad_viewport` | ◐ (V4.1) |
