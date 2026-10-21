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
- [TODO-21-10-2026.md](TODO-21-10-2026.md) — closed record: the Draft assessment,
  the measured import ladder, the five D0 fixes, and what landed on 2026-10-21.
- [TODO-22-10-2026.md](TODO-22-10-2026.md) — active backlog: residual black frames,
  external-mod ingestion, D0 + `--draft`, the `Part` kernel, and the open decisions.
- [incident-2026-10-21-session-crash.md](incident-2026-10-21-session-crash.md) — post-mortem
  of the lost GNOME session: `systemd-oomd` killed `gnome-shell`, not the GPU or Wayland;
  the evidence, the timeline, and the prevention notes.

## Reference

- [architecture.md](architecture.md) — crates, the call paths, and the dependency
  rules.
- [inventor-renderer.md](inventor-renderer.md) — the Inventor scene core, the wgpu
  backend, and the gpui viewport (the render milestone).
- [gpu-interop-assessment.md](gpu-interop-assessment.md) — zero-copy GPU presentation
  through `gpui_interop`: the dma-buf hand-off, the same-device fallback, the platform
  matrix, and the implemented first slice.
- [gizmo-manipulators.md](gizmo-manipulators.md) — the context-driven manipulator suite:
  the Spatial Transactor (T1–T4 built), the topology prerequisite F0, and the edge
  dress-up arc F1.
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
- [conformance.md](conformance.md) — running the upstream FreeCAD test suite against
  the facade: the sparse checkout, the extension build, the commands and the baseline.
- [qt-strategy.md](qt-strategy.md) — running upstream workbench GUIs without Qt: the
  measured Qt surface, the `.ui` loader and micro-shim (option D), and the Q0–Q5 plan.
- [part-draft-layering.md](part-draft-layering.md) — the Part/Draft/BIM layers: why the
  cycle is a test fixture, the analytic/B-Rep split, `ferrocad_curves`, and M0–M5.
- [simplecad-bringup.md](simplecad-bringup.md) — SimpleCAD as the canvas-first proving
  ground: the measured tree, the Part gap table, and the S0–S5 plan.
- [grid-overlay-bringup.md](grid-overlay-bringup.md) — a third-party Coin/GLSL addon as
  a **shader/extraction fixture**: why it is not a workbench, why its GLSL 1.20 cannot
  run on wgpu, and the D→C plan (headless fixture, then WGSL authoring).
- [grid-overlay-plan.md](grid-overlay-plan.md) — the decisions that followed (fork/WGSL,
  `FreeCAD.Grid` semantic API, Coin completeness), the `FreeCAD.Grid` surface and
  semantics, and the G0–G6 staging.
- [mod-providers.md](mod-providers.md) — how a mod declares what it provides, how one
  provider per feature is chosen and persisted, and how `Mods_Settings` switches them.
- [render-diagnostics.md](render-diagnostics.md) — the `FERROCAD_TRACE` / `--trace`
  knobs, the frame/pipeline/presenter trace lines, and the black-frame investigation.
- [upstream-dmabuf-issue.md](upstream-dmabuf-issue.md) — the single-dma-buf race filed
  as `bite-gpui/gpui_interop#1`, the residual black-frame symptom, and the usage rules.
- [repackaging.md](repackaging.md) — editions and the repackaging vision.
- [axis-origin-grid.md](axis-origin-grid.md) — the spatial reference model: the world
  triad, the ground and workplane grids, face frames, snapping, and how the navicube
  relates to them.
- [visual-language.md](visual-language.md) — the token set (canvas, accents, glass,
  edges, text), the type scale, stroke/occlusion rules and the idle → hover → drag
  state model.
- [actions-and-tools.md](actions-and-tools.md) — the workbench command model: `Gui`
  command registration, enablement, dialogs, the actions bar, and the command/tool
  split.
- [toy-workbench.md](toy-workbench.md) — the kernel-free edition that exercises the
  generic interaction registry end to end (and the template for a new workbench).
- [edition-publishing.md](edition-publishing.md) — how the first edition
  (`FerroCAD: Parametric`) is published: a linked `Part`/OCCT binary and its
  bundled `lib/`, the slices, and the open decisions.
- [input-components.md](input-components.md) — the input widget design.
- [python-ui-research.md](python-ui-research.md) — Python-declarative UI research.
