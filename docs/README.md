# FerroCAD docs

Planning notes and reference material for the FerroCAD rewrite. These are working
notes, some of them snapshots in time, not user documentation. The product surface
is the `FreeCAD` Python API, documented by convention and by upstream.

## Direction

- [rewrite-strategy.md](rewrite-strategy.md) — the authoritative direction: why a
  Rust reimplementation, the risk register, and the shape of the work.
- [coin-bridge-reuse-assessment.md](coin-bridge-reuse-assessment.md) — the evidence
  behind the Coin3D and scene-graph decisions (a snapshot).
- [documentation.md](documentation.md) — how the project is documented.

## The record

- [milestones.md](milestones.md) — the full milestone record (M0 through the current
  slices), the decisions the spike unlocked, and the M3 plan.
- [mvp-path.md](mvp-path.md) — the headless engine MVP: definition, acceptance
  script, and the slice plan (tracks A/B/C/D).
- [app-shell-vision.md](app-shell-vision.md) — the interactive window: the five
  pillars, architecture, and slices S1–S6.

## Reference

- [architecture.md](architecture.md) — crates, the call paths, and the dependency
  rules.
- [inventor-renderer.md](inventor-renderer.md) — the Inventor scene core, the wgpu
  backend, and the gpui viewport (the render milestone).
- [gpu-interop-assessment.md](gpu-interop-assessment.md) — zero-copy GPU presentation
  through `gpui_interop`: the dma-buf hand-off, the same-device fallback, the platform
  matrix, and the implemented first slice.
- [gizmo-manipulators.md](gizmo-manipulators.md) — the context-driven manipulator suite:
  the Spatial Transactor math (T1 built), the analytic hit-test/readout/visual slices
  T2–T4, and the topology prerequisite F0 for fillet/chamfer/draft.
- [navicube.md](navicube.md) — the navigation cube: what it is upstream, the slice
  plan, and the as-built map (implemented, N1–N4b).
- [navicube-gpui-design.md](navicube-gpui-design.md) — the GPUI-native redesign: the
  wireframe gyro-cube, hover/proximity states, compass ring, projection toggle, and
  the D1–D5 slice plan.
- [navicube-spec.md](navicube-spec.md) — the navigation cube **specification**: the
  coordinate conventions, piece taxonomy and ids, geometry, appearance **roles** (styling
  is a theme), overlay camera, placement, interaction, and the producer build contract.
- [navicube-plan.md](navicube-plan.md) — making the navigation cube **producer-
  independent**: the model/producer/controller layering and the V1–V4 slice path.
- [directives.md](directives.md) — the neutral **directives** channel: semantic scene data
  the renderer ignores and the viewport consumes (the `SoNaviCube` / `SoEventCallback`
  bridge).
- [inventor-plan.md](inventor-plan.md) — the Inventor/Coin emulation roadmap (four
  phases), adapted to the code as it stands (plan, with per-item status).
- [stage.md](stage.md) — the modern declarative stack (`ferrocad_stage`): the slice
  path (T0–T6, T6 planned), the T0–T5 build, and the deferred decisions.
- [geometry-and-topology.md](geometry-and-topology.md) — topological sorting vs
  topological naming, and the seam for a future OCCT geometry engine.
- [topology-selection.md](topology-selection.md) — the F0 prerequisite for the
  manipulators: the topology/selection seam, the tessellation contract, the
  `opencascade-sys` binding findings, and the F0.0–F0.6 slice plan.
- [occt-history-spike.md](occt-history-spike.md) — the first geometry spike: what
  OCCT history FreeCAD depends on, and what it implies for the trait and the B slices.
- [occt-integration.md](occt-integration.md) — the crate layout for OCCT: the
  `ferrocad_geom` seam, the `ferrocad_occt` backend, and where Part/its Python
  bindings sit.
- [occt-bundling.md](occt-bundling.md) — shipping OCCT the way we ship CPython
  (closure, payload layout, launcher, licensing).
- [python-bindings.md](python-bindings.md) — the PyO3 binding design.
- [property-types.md](property-types.md) — how FreeCAD exposes property types (C++
  `Base::Type`, addressed by name) and what that demands of the document-object SPI.
- [property-value-extension.md](property-value-extension.md) — how a module holds its own
  property value (`Property::Extension`) with `clone`/`eq`/`save`/`restore`.
- [distribution.md](distribution.md) — how the app runs and is packaged (payload,
  bundled runtime, artifacts).
- [releasing.md](releasing.md) — how a release is cut (crates.io, the tag, the
  artifacts).
- [repackaging.md](repackaging.md) — editions and the repackaging vision.
- [edition-publishing.md](edition-publishing.md) — how the first edition
  (`FerroCAD: Parametric`) is published: a linked `Part`/OCCT binary and its
  bundled `lib/`, the slices, and the open decisions.
- [input-components.md](input-components.md) — the input widget design.
- [python-ui-research.md](python-ui-research.md) — Python-declarative UI research.
