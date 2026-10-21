# Directives: the neutral host-facing channel

Status: **decision** (2026-10-12). Companion to
[`inventor-renderer.md`](inventor-renderer.md) (the render stack),
[`navicube-spec.md`](../interaction/navicube-spec.md) / [`navicube-plan.md`](../interaction/navicube-plan.md) (the
`SoNaviCube` bridge this enables), [`stage.md`](stage.md), and
[`architecture.md`](../architecture.md) (crate layering).

## 1. Why

Some of what a scene or a stack produces is **semantic, not drawable**: a renderer must
ignore it, but the viewport must consume it. `SoEventCallback` and `SoNaviCube` are both
such things, and the original `SoEventCallback` handling was an ad-hoc rescan of the AST
(`ferrocad_inventor::event_callbacks`, since removed) that the directive shape replaces:

- it did a **second traversal** of the scene;
- it **leaked scene node types** up to the host;
- it tied the mechanism to `ferrocad_inventor`, so a modern `ferrocad_stage` workbench
  could not drive the same overlays without depending on Inventor.

## 2. Decision

A **neutral `Directives` channel** in `ferrocad_graphics`, returned **alongside** the
`RenderQueue` as a bare tuple:

```rust
compile_with(scene, viewport) -> (RenderQueue, Directives)
```

- **Produced** by the compiler in the **same traversal** that emits packets — so
  `SoSwitch` state is honoured and no rescan happens.
- **Consumed** by the viewport, which translates each directive into an overlay (or event
  routing).
- **Invisible to the renderer**: `RenderQueue` + `Camera` remain its entire input. The
  invariant is structural — the directives are never *passed* to it — not a runtime
  "ignore".

Because the channel lives in `ferrocad_graphics`, **both** producers can emit it without
either depending on the other.

## 3. Shapes

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Directive {
    /// A host-rendered navicube overlay (from a `SoNaviCube` node).
    NaviCube(NaviCubeDirective),
    /// An active `SoEventCallback`: the host routes input to the callables keyed by
    /// this id. It draws nothing.
    EventCallback(u64),
}

/// A navicube overlay request — structural only; **styling is the Rust core's** (§5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NaviCubeDirective {
    pub rect: Rect,
    /// `None` mirrors the main camera (the modern default); `Some` is explicit (legacy).
    pub orientation: Option<Rotation>,
    pub hilite: Option<PickId>,
    pub orthographic: bool,
}
```

A **flat enum**, not `Overlay(kind)`: `EventCallback` is a directive but not an overlay,
so a shared `Overlay` supertype would be false. Generalising to gizmos / HUD /
annotations is **adding variants**. `Directives::{navicubes, event_callbacks}` filter it
by variant for the two consumers.

## 4. The invariant

```
compile_with(scene) ─▶ (RenderQueue, Directives)
                             │             │
                   ┌─────────┘             └────────────┐
                   ▼                                    ▼
            renderer (draw_frame)              viewport: translate directives
            sees only queue + camera           → overlay ViewSpec / event routing
```

## 5. Appearance is not a directive

Directives carry **what and where**, never **how it looks**. Styling — colours, opacity,
line width — is owned by the Rust core: the viewport **theme** over the draw primitives
(see [`navicube-spec.md`](../interaction/navicube-spec.md) §5). Consequence for the legacy `SoNaviCube`:
its `baseColor` / `emphaseColor` / `hiliteColor` (+ alphas) and `borderWidth` are
**accepted but ignored** for now — the theme wins. `showCoordinateSystem` (a feature flag,
not styling) and the axis colours are deferred with the axes themselves.

## 6. Layering

```
ferrocad_graphics   RenderQueue · Camera · ViewSpec · Directives · Directive · NaviCubeDirective
      ▲        ▲
      │        │
 inventor   stage          both emit (RenderQueue, Directives); neither depends on the other
      ▲        ▲
      └───┬────┘
      viewport             consumes Directives → producers → overlay ViewSpec
          │
      ferrocad_wgpu        renderer: RenderQueue only
```

## 7. Producers and consumers

- **`ferrocad_inventor`** — a `NodeKind::NaviCube(NaviCubeDirective)` node emits **no**
  packet; the compiler lifts it into `Directives`. A `NodeKind::EventCallback(id)` node
  does the same, so `SoEventCallback` is found in the **same traversal** that emits
  geometry (honouring `SoSwitch` state) rather than by a second walk
  (`event_callbacks(scene)` was removed). The `EventListener` API carries the
  `Directives` to the listener, so there is one channel, not two.
- **`ferrocad_stage`** — the channel is neutral, so a stage can emit directives (e.g. a
  stage-side navicube); that is why this lives in `ferrocad_graphics`.
- **`ferrocad_viewport`** — consumes `Directive::NaviCube` (`NaviCubeDirective` + the
  viewport theme → `NavicubeVisual` + `Rect` → the producer → overlay `ViewSpec`) and
  routes `Directive::EventCallback` to the Python callables (`dispatch_event` iterates
  `Directives::event_callbacks`).

## 8. Ratified decisions

1. **Bare tuple** `(RenderQueue, Directives)` — no wrapper type.
2. **Flat `Directive` enum** — not `Overlay(kind)`.
3. **No appearance** in directives — the Rust core owns styling.
4. **`orientation: Option<Rotation>`** — `None` mirrors the main camera.

## 9. Deferred

- `Gizmo` / `Hud` / `Annotation` variants (Rust-driven overlays — the gizmo and tool
  transients — stay directly built by the viewport for now).
- A per-directive appearance override (e.g. the legacy `SoNaviCube` colours).
- `showCoordinateSystem` and the axis section.
