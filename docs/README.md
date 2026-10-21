# FerroCAD docs

Planning notes and reference material for the FerroCAD rewrite. These are working
notes, some of them snapshots in time, not user documentation. The product surface
is the `FreeCAD` Python API, documented by convention and by upstream.

## Layout

The tree is organized by **what a note is about**, not by what kind of note it is —
each file already states its own status (`decision`, `reference`, `plan`, `record`)
on the first line, so the folder only has to answer "where do I look?".

| Folder | Holds | Read it when |
| --- | --- | --- |
| `direction/` | why we are doing this, and what the product is | you want the argument, not the design |
| `architecture/` | the crate graph, seams, data model, packaging | you are changing a boundary |
| `viewport/` | the scene/render/present stack | you are working on what reaches the screen |
| `interaction/` | spatial reference, selection, gizmos, navigation, visual language | you are working on how the user acts on the scene |
| `workbenches/` | the Python module contract and upstream bring-up | you are wiring a mod or an upstream workbench |
| `record/` | closed history: milestones, the closed TODO, incidents | you need to know how we got here |
| `tasks/` | actionable specs with a checklist | you are picking up work |

Four files stay at the root: this index, the [crate map](architecture.md) (the
cross-cutting reference every section links back to), the
[documentation decision](documentation.md), and the active backlog
([`TODO-22-10-2026.md`](TODO-22-10-2026.md)).

## Start here

- [architecture.md](architecture.md) — crates, the call paths, the dependency rules
  between them, and the C-ABI rationale.
- [TODO-22-10-2026.md](TODO-22-10-2026.md) — the active backlog: residual black
  frames, external-mod ingestion, D0 + `--draft`, the `Part` kernel, and the open
  decisions.
- [documentation.md](documentation.md) — how the project is documented (Diátaxis,
  rustdoc-first, and this layout).

## Direction

- [direction/rewrite-strategy.md](direction/rewrite-strategy.md) — the authoritative
  direction: why a Rust reimplementation, the risk register, and the shape of the work.
- [direction/mvp-path.md](direction/mvp-path.md) — the headless engine MVP: definition,
  acceptance script, and the slice plan (tracks A/B/C/D).
- [direction/app-shell-vision.md](direction/app-shell-vision.md) — the interactive
  window: the five pillars, architecture, and slices S1–S6.

## Architecture

- [architecture/geometry-and-topology.md](architecture/geometry-and-topology.md) —
  topological sorting vs topological naming, and the seam for a future OCCT geometry
  engine.
- [architecture/part-draft-layering.md](architecture/part-draft-layering.md) — the
  Part/Draft/BIM layers: why the cycle is a test fixture, the analytic/B-Rep split,
  `ferrocad_curves`, and M0–M5.
- [architecture/occt-history-spike.md](architecture/occt-history-spike.md) — the first
  geometry spike: what OCCT history FreeCAD depends on, and what it implies for the
  trait and the B slices.
- [architecture/occt-integration.md](architecture/occt-integration.md) — the crate
  layout for OCCT: the `ferrocad_geom` seam, the `ferrocad_occt` backend, and where
  Part/its Python bindings sit.
- [architecture/occt-bundling.md](architecture/occt-bundling.md) — shipping OCCT the
  way we ship CPython (closure, payload layout, launcher, licensing).
- [architecture/property-types.md](architecture/property-types.md) — how FreeCAD
  exposes property types (C++ `Base::Type`, addressed by name) and what that demands
  of the document-object SPI.
- [architecture/property-value-extension.md](architecture/property-value-extension.md) —
  how a module holds its own property value (`Property::Extension`) with
  `clone`/`eq`/`save`/`restore`.
- [architecture/python-bindings.md](architecture/python-bindings.md) — the PyO3 binding
  design.
- [architecture/distribution.md](architecture/distribution.md) — how the app runs and is
  packaged (payload, bundled runtime, artifacts).
- [architecture/repackaging.md](architecture/repackaging.md) — editions and the
  repackaging vision.
- [architecture/edition-publishing.md](architecture/edition-publishing.md) — how the
  first edition (`FerroCAD: Parametric`) is published: a linked `Part`/OCCT binary and
  its bundled `lib/`, the slices, and the open decisions.
- [architecture/releasing.md](architecture/releasing.md) — how a release is cut
  (crates.io, the tag, the artifacts).

## Viewport

- [viewport/inventor-renderer.md](viewport/inventor-renderer.md) — the Inventor scene
  core, the wgpu backend, and the gpui viewport (the render milestone).
- [viewport/inventor-plan.md](viewport/inventor-plan.md) — the Inventor/Coin emulation
  roadmap (four phases), adapted to the code as it stands (plan, with per-item status).
- [viewport/coin-bridge-reuse-assessment.md](viewport/coin-bridge-reuse-assessment.md) —
  the evidence behind the Coin3D and scene-graph decisions (a snapshot).
- [viewport/stage.md](viewport/stage.md) — the modern declarative stack
  (`ferrocad_stage`): the slice path (T0–T6, T6 planned), the T0–T5 build, and the
  deferred decisions.
- [viewport/directives.md](viewport/directives.md) — the neutral **directives** channel:
  semantic scene data the renderer ignores and the viewport consumes (the `SoNaviCube` /
  `SoEventCallback` bridge).
- [viewport/gpu-interop-assessment.md](viewport/gpu-interop-assessment.md) — zero-copy
  GPU presentation through `gpui_interop`: the dma-buf hand-off, the same-device
  fallback, the platform matrix, and the implemented first slice.
- [viewport/render-diagnostics.md](viewport/render-diagnostics.md) — the `FERROCAD_TRACE`
  / `--trace` knobs, the frame/pipeline/presenter trace lines, and the black-frame
  investigation.
- [viewport/upstream-dmabuf-issue.md](viewport/upstream-dmabuf-issue.md) — the
  single-dma-buf race filed as `bite-gpui/gpui_interop#1`, the residual black-frame
  symptom, and the usage rules.

## Interaction

- [interaction/axis-origin-grid.md](interaction/axis-origin-grid.md) — the spatial
  reference model: the world triad, the ground and workplane grids, face frames,
  snapping, and how the navicube relates to them.
- [interaction/visual-language.md](interaction/visual-language.md) — the token set
  (canvas, accents, glass, edges, text), the type scale, stroke/occlusion rules and the
  idle → hover → drag state model.
- [interaction/gizmo-manipulators.md](interaction/gizmo-manipulators.md) — the
  context-driven manipulator suite: the Spatial Transactor (T1–T4 built), the topology
  prerequisite F0, and the edge dress-up arc F1.
- [interaction/topology-selection.md](interaction/topology-selection.md) — the F0
  prerequisite for the manipulators: the topology/selection seam, the tessellation
  contract, the `opencascade-sys` binding findings, and the F0.0–F0.6 slice plan.
- [interaction/navicube.md](interaction/navicube.md) — the navigation cube: what it is
  upstream, the slice plan, and the as-built map (implemented, N1–N4b).
- [interaction/navicube-spec.md](interaction/navicube-spec.md) — the navigation cube
  **specification**: the coordinate conventions, piece taxonomy and ids, geometry,
  appearance **roles** (styling is a theme), overlay camera, placement, interaction, and
  the producer build contract.
- [interaction/navicube-plan.md](interaction/navicube-plan.md) — making the navigation
  cube **producer-independent**: the model/producer/controller layering and the V1–V4
  slice path.
- [interaction/navicube-gpui-design.md](interaction/navicube-gpui-design.md) — the
  GPUI-native redesign: the wireframe gyro-cube, hover/proximity states, compass ring,
  projection toggle, and the D1–D5 slice plan.
- [interaction/input-components.md](interaction/input-components.md) — the input widget
  design.
- [interaction/python-ui-research.md](interaction/python-ui-research.md) —
  Python-declarative UI research.

## Workbenches

- [workbenches/actions-and-tools.md](workbenches/actions-and-tools.md) — the workbench
  command model: `Gui` command registration, enablement, dialogs, the actions bar, and
  the command/tool split.
- [workbenches/mod-providers.md](workbenches/mod-providers.md) — how a mod declares what
  it provides, how one provider per feature is chosen and persisted, and how
  `Mods_Settings` switches them.
- [workbenches/toy-workbench.md](workbenches/toy-workbench.md) — the kernel-free edition
  that exercises the generic interaction registry end to end (and the template for a new
  workbench).
- [workbenches/conformance.md](workbenches/conformance.md) — running the upstream
  FreeCAD test suite against the facade: the sparse checkout, the extension build, the
  commands and the baseline.
- [workbenches/qt-strategy.md](workbenches/qt-strategy.md) — running upstream workbench
  GUIs without Qt: the measured Qt surface, the `.ui` loader and micro-shim (option D),
  and the Q0–Q5 plan.
- [workbenches/simplecad-bringup.md](workbenches/simplecad-bringup.md) — SimpleCAD as the
  canvas-first proving ground: the measured tree, the Part gap table, and the S0–S5 plan.
- [workbenches/grid-overlay-bringup.md](workbenches/grid-overlay-bringup.md) — a
  third-party Coin/GLSL addon as a **shader/extraction fixture**: why it is not a
  workbench, why its GLSL 1.20 cannot run on wgpu, and the D→C plan (headless fixture,
  then WGSL authoring).
- [workbenches/grid-overlay-plan.md](workbenches/grid-overlay-plan.md) — the decisions
  that followed (fork/WGSL, `FreeCAD.Grid` semantic API, Coin completeness), the
  `FreeCAD.Grid` surface and semantics, and the G0–G6 staging.

## The record

- [record/milestones.md](record/milestones.md) — the full milestone record (M0 through
  the current slices), the decisions the spike unlocked, and the M3 plan.
- [record/TODO-21-10-2026.md](record/TODO-21-10-2026.md) — closed record: the Draft
  assessment, the measured import ladder, the five D0 fixes, and what landed on
  2026-10-21.
- [record/incident-2026-10-21-session-crash.md](record/incident-2026-10-21-session-crash.md)
  — post-mortem of the lost GNOME session: `systemd-oomd` killed `gnome-shell`, not the
  GPU or Wayland; the evidence, the timeline, and the prevention notes.

## Tasks

- [tasks/dev-tools-window.md](tasks/dev-tools-window.md) — move the actions bar,
  inspector, property editor and console into a separate Dev Tools window; the main
  window becomes a clean viewport.
- [tasks/d0-draft-import.md](tasks/d0-draft-import.md) — the five headless fixes and the
  `--draft` conformance group.
- [tasks/p2-architecture.md](tasks/p2-architecture.md) — `ferrocad_curves`, the
  Part/Draft seam, the Qt split plane, and the SimpleCAD order.
