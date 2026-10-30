# Document index

Status: index (2026-10-29). Every Markdown note under `docs/`, grouped by the folder
and audience it serves. The folders are split by **audience and stability** (see
[project/documentation.md](project/documentation.md)); this page is the complete map,
and [README.md](README.md) is the short tour.

| Folder | Audience | Stability | Published |
| --- | --- | --- | --- |
| [General concepts](#general-concepts) | general readers: the concepts behind 3D CAD and FerroCAD | stable | yes, in the mdBook |
| [User documentation](#user-documentation) | UI users, and the generated API workflows for console, macro and script users | stable | yes, in the mdBook |
| [Mods documentation](#mods-documentation) | addon authors: mods, workbenches, scripts | stable | yes, in the mdBook |
| [Python API reference](#python-api-reference) | API users: the exact signature reference | stable | yes, in the mdBook |
| [Developer documentation](#developer-documentation) | people changing FerroCAD | stable | no, repository only |
| [Project documentation](#project-documentation) | maintainers and contributors | unstable | no, repository only |

Each note appears **once**, under its primary folder. When a note serves a second
audience, its opening links across rather than duplicating the text. The first four
folders are what `tools/build_book.py` publishes at
<https://ferrocad.github.io/docs/>; `developer/` and `project/` stay in the repository.

## Where do I start?

| I want to… | Start here |
| --- | --- |
| understand 3D CAD concepts | [general/README.md](general/README.md) |
| use the app's window (UI) | [user/README.md](user/README.md) and the [repository README](../README.md) |
| use the Python console, a macro, or a script (API) | [UI users and API users](user/explanation/ui-and-api.md), then [user/README.md](user/README.md) and [python-api/index.md](python-api/index.md) |
| write a mod or a workbench | [mods/README.md](mods/README.md) |
| drive `FreeCAD` from another program | [mods/README.md#the-json-boundary-for-automated-clients](mods/README.md#the-json-boundary-for-automated-clients) and [python-api/ferrocad_shell.md](python-api/ferrocad_shell.md) |
| change FerroCAD | [developer/README.md](developer/README.md) and [developer/architecture.md](developer/architecture.md) |
| understand why, or what is next | [project/direction/rewrite-strategy.md](project/direction/rewrite-strategy.md), [project/ROADMAP.md](project/ROADMAP.md) and [project/TODO-31-10-2026.md](project/TODO-31-10-2026.md) |
| find what a label means | [project/labels.md](project/labels.md) |

## General concepts

Explanations of the ideas behind 3D CAD and FerroCAD. They serve a general audience —
UI users, API users and developers — and avoid task steps and crate internals except
where the mapping needs them.

- [general/README.md](general/README.md) — the overview and reading order for the five chapters.
- [general/01-the-dna-of-3d-geometry.md](general/01-the-dna-of-3d-geometry.md) — analytic surfaces, NURBS, B-Rep and meshes.
- [general/02-the-great-translation.md](general/02-the-great-translation.md) — tessellation, Coin3D and the GPU pipeline.
- [general/03-topological-naming.md](general/03-topological-naming.md) — parametric history, splits and merges, and the naming problem.
- [general/04-how-freecad-organizes-geometry.md](general/04-how-freecad-organizes-geometry.md) — the `App`/`Part` split and why kernels cannot simply be swapped.
- [general/05-how-ferrocad-organizes-this.md](general/05-how-ferrocad-organizes-this.md) — how FerroCAD's crates map onto each layer.

## User documentation

The pages a user reads. They are **generated** by `tools/userdocs.py` from the
executable workflows in `tools/workflows/`, so a code block is the code that ran and a
table is what it read back. Never hand-edit them; edit the workflow. The mdBook edition
is assembled from these pages and the addon pages below.

- [user/README.md](user/README.md) — the generated index and progress list; the workflows implemented today.
- [user/tutorials/first-model.md](user/tutorials/first-model.md) — build your first model, one pass end to end.
- [user/how-to/documents-and-properties.md](user/how-to/documents-and-properties.md) — create a document and edit its properties.
- [user/how-to/grid-visibility.md](user/how-to/grid-visibility.md) — turn the reference grid on and off.
- [user/how-to/model-a-nurbs-surface.md](user/how-to/model-a-nurbs-surface.md) — build a clamped B-spline patch with `Part.BSplineSurface`.
- [user/how-to/parametric-recompute.md](user/how-to/parametric-recompute.md) — wire two objects together with an expression.
- [user/how-to/selection-and-commands.md](user/how-to/selection-and-commands.md) — drive a command from the selection.
- [user/how-to/solid-from-a-nurbs-surface.md](user/how-to/solid-from-a-nurbs-surface.md) — extrude a B-spline face or cap a closed surface to make a solid.
- [user/how-to/text-to-3d.md](user/how-to/text-to-3d.md) — turn a font into 3-D text with `Draft.make_shape_string`, then extrude, raise or etch it.
- [user/how-to/toy-workbench.md](user/how-to/toy-workbench.md) — create, display and manipulate an object in the toy edition.
- [user/how-to/undo-and-persistence.md](user/how-to/undo-and-persistence.md) — undo an edit and reload the document.
- [user/explanation/how-the-shell-talks-to-the-document.md](user/explanation/how-the-shell-talks-to-the-document.md) — why the shell is a client of the facade, and why dialogs are data.
- [user/explanation/ui-and-api.md](user/explanation/ui-and-api.md) — a gentle introduction to UI users, API users (console, macros, scripts), and the FreeCAD/Ondsel history behind the split.

## Mods documentation

The authored external contract for mods, workbenches and automated clients. The
background notes they lean on are in development: the command model in
[developer/workbenches/actions-and-tools.md](developer/workbenches/actions-and-tools.md),
the provider design in [developer/workbenches/mod-providers.md](developer/workbenches/mod-providers.md),
a complete workbench in [developer/workbenches/toy-workbench.md](developer/workbenches/toy-workbench.md),
and the naming boundary in
[developer/architecture/api-namespaces.md](developer/architecture/api-namespaces.md).

- [mods/README.md](mods/README.md) — the external contract: what a mod is, the five contributions, the JSON boundary and the reading order.
- [mods/minimal-mod.md](mods/minimal-mod.md) — a complete mod you can load today.
- [mods/mod-anatomy.md](mods/mod-anatomy.md) — the directory, the startup scripts, the lifecycle and the isolation rules.
- [mods/commands.md](mods/commands.md) — commands, resources, dialogs and the `view`/`tool` requests.
- [mods/display-and-interaction.md](mods/display-and-interaction.md) — display meshes, interaction edits and the manipulator commit path.
- [mods/providers.md](mods/providers.md) — one active backend per feature, and who owns the switch.
- [mods/settings-and-parameters.md](mods/settings-and-parameters.md) — `ParamGet`, the settings tree and `FerroCAD.Grid`.
- [mods/testing-a-mod.md](mods/testing-a-mod.md) — headless tests for a mod, with no GPU.

## Python API reference

The generated signature reference, read from the live modules by `tools/api_docs.py`.
Do not hand-edit these pages; the source is the module itself.

- [python-api/index.md](python-api/index.md) — the reference landing page, module by module.
- [python-api/FreeCAD.md](python-api/FreeCAD.md) — the `FreeCAD` module surface.
- [python-api/FreeCAD.Base.md](python-api/FreeCAD.Base.md) — `FreeCAD.Base`: values, vectors, placements and exceptions.
- [python-api/FreeCAD.Units.md](python-api/FreeCAD.Units.md) — `FreeCAD.Units`: quantities, unit strings and conversion.
- [python-api/FreeCAD.Console.md](python-api/FreeCAD.Console.md) — `FreeCAD.Console`: the print/log surface.
- [python-api/FreeCADGui.md](python-api/FreeCADGui.md) — `FreeCADGui`: commands, selection, providers and view state.
- [python-api/FerroCAD.md](python-api/FerroCAD.md) — the FerroCAD-only `FerroCAD` module.
- [python-api/FerroCADGui.md](python-api/FerroCADGui.md) — the FerroCAD-only `FerroCADGui` module.
- [python-api/ferrocad_shell.md](python-api/ferrocad_shell.md) — the JSON bridge the shell calls.

## Developer documentation

Stable design and reference notes for people changing FerroCAD. The folder landing page
is the place to start; the research notes that shaped these designs are under
[project/research/](project/research/README.md), and the code-adjacent decisions stay
here.

- [developer/README.md](developer/README.md) — the folder landing page and reading order.
- [developer/architecture.md](developer/architecture.md) — the crate graph, the call paths, the dependency rules and the C-ABI rationale.
- [developer/testing.md](developer/testing.md) — extension sets, the edition dependency-closure contract and the CI order.

### Architecture and the crate map

- [developer/architecture/api-namespaces.md](developer/architecture/api-namespaces.md) — the namespace rule: which names stay FreeCAD-compatible and which are FerroCAD-only.
- [developer/architecture/distribution.md](developer/architecture/distribution.md) — how the app runs and is packaged: payload, bundled runtime and artifacts.
- [developer/architecture/edition-publishing.md](developer/architecture/edition-publishing.md) — publishing `FerroCAD: Parametric` with its linked `Part`/OCCT binary.
- [developer/architecture/kernel-state-sharing.md](developer/architecture/kernel-state-sharing.md) — the one-process-wide-core invariant, the state scopes, the linking options and decision, the supported compositions, and the deferred `PyCapsule` C-ABI.
- [developer/architecture/lineage-and-naming.md](developer/architecture/lineage-and-naming.md) — topological naming as a lineage layer rather than a kernel concern.
- [developer/architecture/macos.md](developer/architecture/macos.md) — why macOS is blocked upstream and the re-enable checklist.
- [developer/architecture/occt-bundling.md](developer/architecture/occt-bundling.md) — shipping OCCT like CPython: closure, payload layout, launcher and licensing.
- [developer/architecture/occt-integration.md](developer/architecture/occt-integration.md) — the `ferrocad_geom` seam, the `ferrocad_occt` backend and where Part sits.
- [developer/architecture/part-draft-layering.md](developer/architecture/part-draft-layering.md) — the Part/Draft/BIM layers and the analytic/B-Rep split.
- [developer/architecture/property-types.md](developer/architecture/property-types.md) — the C++ property registry and what it demands of the document-object SPI.
- [developer/architecture/property-value-extension.md](developer/architecture/property-value-extension.md) — how a module holds its own property value.
- [developer/architecture/python-bindings.md](developer/architecture/python-bindings.md) — the PyO3 binding design.
- [developer/architecture/releasing.md](developer/architecture/releasing.md) — how a release is cut: crates.io, the tag and the artifacts.
- [developer/architecture/repackaging.md](developer/architecture/repackaging.md) — the one-shell/many-editions repackaging strategy.

### Viewport

- [developer/viewport/inventor-renderer.md](developer/viewport/inventor-renderer.md) — the Inventor scene core, the wgpu backend and the gpui viewport.
- [developer/viewport/inventor-plan.md](developer/viewport/inventor-plan.md) — the Inventor/Coin emulation roadmap and its per-item status.
- [developer/viewport/stage.md](developer/viewport/stage.md) — `ferrocad_stage`: the declarative stack, T0–T6 and the as-built sections.
- [developer/viewport/directives.md](developer/viewport/directives.md) — the neutral directives channel the renderer ignores and the viewport consumes.
- [developer/viewport/render-diagnostics.md](developer/viewport/render-diagnostics.md) — the `FERROCAD_TRACE`/`--trace` knobs and the black-frame investigation.

### Interaction

- [developer/interaction/axis-origin-grid.md](developer/interaction/axis-origin-grid.md) — the world triad, the ground/workplane grids, face frames and snapping.
- [developer/interaction/gizmo-manipulators.md](developer/interaction/gizmo-manipulators.md) — the context-driven manipulator suite: the Spatial Transactor, F0 and edge dress-up.
- [developer/interaction/input-components.md](developer/interaction/input-components.md) — the input widget architecture.
- [developer/interaction/navicube.md](developer/interaction/navicube.md) — the navigation cube: what it is upstream, the slice plan and the as-built map.
- [developer/interaction/navicube-spec.md](developer/interaction/navicube-spec.md) — the producer-neutral NaviCube specification.
- [developer/interaction/navicube-plan.md](developer/interaction/navicube-plan.md) — making the navigation cube producer-independent: the layering and V1–V4.
- [developer/interaction/navicube-gpui-design.md](developer/interaction/navicube-gpui-design.md) — the GPUI-native gyro-cube design and the D1–D5 plan.
- [developer/interaction/selection-gizmos-and-forms.md](developer/interaction/selection-gizmos-and-forms.md) — the interaction state machine: selection, the wheel, gizmos and the floating form.
- [developer/interaction/topology-selection.md](developer/interaction/topology-selection.md) — the topology/selection seam, the tessellation contract and the F0 plan.
- [developer/interaction/transform-ux.md](developer/interaction/transform-ux.md) — transform modes, pivots, re-pivoting and push/pull.
- [developer/interaction/visual-language.md](developer/interaction/visual-language.md) — the tokens, the type scale, strokes and the idle → hover → drag state model.
- [developer/interaction/workplanes.md](developer/interaction/workplanes.md) — the workplane as a first-class state, the `WP` selector (no dot, label carries the state) and the three ways to bind a plane.

### Workbenches and tooling

- [developer/workbenches/actions-and-tools.md](developer/workbenches/actions-and-tools.md) — the workbench command model: registration, enablement, dialogs and the actions bar.
- [developer/workbenches/conformance.md](developer/workbenches/conformance.md) — running the upstream FreeCAD suite against the facade.
- [developer/workbenches/draft-tools.md](developer/workbenches/draft-tools.md) — the native Draft tool layer (Q4/Q5): Snapper, point sessions, line/wire and the snap bar.
- [developer/workbenches/grid-overlay-plan.md](developer/workbenches/grid-overlay-plan.md) — the grid-overlay decisions, the `FerroCAD.Grid` surface and G0–G6.
- [developer/workbenches/mod-providers.md](developer/workbenches/mod-providers.md) — how a mod declares its providers and how the active backend is chosen.
- [developer/workbenches/part-api-audit.md](developer/workbenches/part-api-audit.md) — the repeatable Part/OCCT surface audit and its `--check` contract.
- [developer/workbenches/qt-strategy.md](developer/workbenches/qt-strategy.md) — running upstream workbench GUIs without Qt: the measured surface and Q0–Q5.
- [developer/workbenches/screenshot-tool.md](developer/workbenches/screenshot-tool.md) — the three screenshot scopes, the snapshot spec and the GPU-free image manifest gate.
- [developer/workbenches/toy-workbench.md](developer/workbenches/toy-workbench.md) — the kernel-free reference edition and the template for a new workbench.

## Project documentation

Why the work is shaped this way, how it is tracked, and the measured record. This folder
is deliberately unstable: it changes with the work, and closed material is kept rather
than deleted.

- [project/README.md](project/README.md) — the folder landing page, reading order and the record rule.
- [project/ROADMAP.md](project/ROADMAP.md) — the three horizons and their exit criteria.
- [project/TODO-31-10-2026.md](project/TODO-31-10-2026.md) — the active backlog: the minimum SimpleCAD surface, solids and booleans, surface/subdivision, and constraints with a bound solver.
- [project/labels.md](project/labels.md) — what every `S1`, `P4`, `D0`, `T4` and `G1` means, and which program owns it.

### Documentation system

- [README.md](README.md) — the short tour of the five folders and the reading paths.
- [project/documentation.md](project/documentation.md) — the documentation decision: audiences, Diátaxis, rustdoc-first and the folder rules.

### Direction and strategy

- [project/direction/rewrite-strategy.md](project/direction/rewrite-strategy.md) — the authoritative direction: why a Rust reimplementation, the risk register and the shape of the work.
- [project/direction/mvp-path.md](project/direction/mvp-path.md) — the headless engine MVP: definition, acceptance script and the A/B/C/D slice plan.
- [project/direction/app-shell-vision.md](project/direction/app-shell-vision.md) — the interactive window: the five pillars, the architecture and S1–S6.

### Ideation

Exploratory product ideas. They are hypotheses, not commitments or accepted designs.

- [project/ideation/README.md](project/ideation/README.md) — the ideation landing page and the rule for promoting an idea.
- [project/ideation/intent-and-automation.md](project/ideation/intent-and-automation.md) — the product thesis: design intent as a document object, lineage as explanations, constraint proposals, live context, and a local-first agent surface.
- [project/ideation/freecad-pain-points.md](project/ideation/freecad-pain-points.md) — a consolidated review of 13 recurring FreeCAD pain points, their upstream root causes, the FerroCAD response, and a validation plan.
- [project/ideation/surface-and-freeform-modelling.md](project/ideation/surface-and-freeform-modelling.md) — the built-in Surface workbench at the pinned upstream commit, the `Body` allow-list that keeps surfaces out of `PartDesign`, the fragmentation of surfacing across modules, upstream's draft FEP-0014 (Forms) cage workbench, and host/bundle/native options for shipping a freeform edition.

### Research notes

The folder landing page is the reading order for the spikes and assessments.

- [project/research/README.md](project/research/README.md) — the research landing page and reading order.
- [project/research/geometry-and-topology.md](project/research/geometry-and-topology.md) — topological sorting vs topological naming, and the seam for a future geometry engine.
- [project/research/tns-decoupling.md](project/research/tns-decoupling.md) — whether naming can be decoupled from the kernel: what is kernel-neutral today, what is unproven, and the criteria to promote it.
- [project/research/occt-history-spike.md](project/research/occt-history-spike.md) — the first OCCT history spike: what FreeCAD depends on and what it implies for the trait.
- [project/research/coin-bridge-reuse-assessment.md](project/research/coin-bridge-reuse-assessment.md) — the evidence behind the Coin3D and scene-graph decisions.
- [project/research/gpu-interop-assessment.md](project/research/gpu-interop-assessment.md) — zero-copy GPU presentation: the dma-buf hand-off, the fallback and the platform matrix.
- [project/research/upstream-dmabuf-issue.md](project/research/upstream-dmabuf-issue.md) — the single-dma-buf race filed as `bite-gpui/gpui_interop#1` and the residual symptom.
- [project/research/python-ui-research.md](project/research/python-ui-research.md) — Python scripting and a Python-built UI: the research behind the shell.
- [project/research/grid-overlay-bringup.md](project/research/grid-overlay-bringup.md) — a third-party Coin/GLSL addon as a shader/extraction fixture.
- [project/research/simplecad-bringup.md](project/research/simplecad-bringup.md) — SimpleCAD as the canvas-first proving ground: the measured tree and S0–S5.

### Closed history

- [project/record/milestones.md](project/record/milestones.md) — the compact timeline and the current assessment.
- [project/record/TODO-29-10-2026.md](project/record/TODO-29-10-2026.md) — the closed 2026-10-29 backlog: the workplane hierarchy, datum planes, the grid and the SimpleCAD P1 closure.
- [project/record/2026-10-29-workplane-visual-hierarchy-and-grid.md](project/record/2026-10-29-workplane-visual-hierarchy-and-grid.md) — the round log for the workplane and grid work, including the four grid defects and their mechanisms.
- [project/record/2026-10-30-fan-out-todo-30.md](project/record/2026-10-30-fan-out-todo-30.md) — the six-stream fan-out over the 2026-10-30 backlog: seven corrected claims, the evidence-sized slices and the open unknowns.
- [project/record/2026-10-30-viewport-pick-and-fixed-pitch-grid.md](project/record/2026-10-30-viewport-pick-and-fixed-pitch-grid.md) — the 2026-10-30 code round: the unrenderable pick format, the image-path cost, the fixed-pitch grid fade and the Move session.
- [project/record/TODO-30-10-2026.md](project/record/TODO-30-10-2026.md) — the closed 2026-10-30 backlog: the SimpleCAD bring-up slices, the 3-D text workflow, the fan-out's re-filed sections and the platform remainder.
- [project/record/TODO-27-10-2026.md](project/record/TODO-27-10-2026.md) — the closed 2026-10-27 backlog: the kernel regimes, the SimpleCAD reconstruction fixes, the Draft import closure and the extension-set/CI split.
- [project/record/TODO-26-10-2026.md](project/record/TODO-26-10-2026.md) — the closed 2026-10-26 backlog: the interaction/gizmo layer, Draft, SimpleCAD and Mesh.
- [project/record/TODO-24-10-2026.md](project/record/TODO-24-10-2026.md) — the closed 2026-10-24 backlog: the element map, the OCCT seam, feature proxies and Qt Q0–Q5.
- [project/record/TODO-23-10-2026.md](project/record/TODO-23-10-2026.md) — the closed 2026-10-23 backlog: serializers, lineage, Links and Qt Q0–Q3b.
- [project/record/TODO-22-10-2026.md](project/record/TODO-22-10-2026.md) — the closed 2026-10-22 backlog: the Part conformance push, the headless `ViewObject` and the `.FCStd` reader.
- [project/record/TODO-21-10-2026.md](project/record/TODO-21-10-2026.md) — the closed 2026-10-21 backlog: the grid overlay, the Draft probe and the external mod repository.
- [project/record/2026-10-24-q4-q5-draft-tools.md](project/record/2026-10-24-q4-q5-draft-tools.md) — the closed Q4/Q5 round log for the native Draft tools.
- [project/record/incident-2026-10-21-session-crash.md](project/record/incident-2026-10-21-session-crash.md) — the post-mortem of the lost GNOME session: `systemd-oomd`, not the GPU or Wayland.

### Closed task specs

- [project/record/tasks/README.md](project/record/tasks/README.md) — the index of closed specs, by label and closing backlog.
- [project/record/tasks/d0-draft-import.md](project/record/tasks/d0-draft-import.md) — D0: import the headless Draft core.
- [project/record/tasks/dev-tools-window.md](project/record/tasks/dev-tools-window.md) — move the old tooling out of the main window into Dev Tools.
- [project/record/tasks/element-map-policy.md](project/record/tasks/element-map-policy.md) — the compound-cut element-map size gap (S6c).
- [project/record/tasks/format-serializers.md](project/record/tasks/format-serializers.md) — split document persistence into two independent serializers.
- [project/record/tasks/grid-visibility.md](project/record/tasks/grid-visibility.md) — the grid toggle only reached the native grid, not the Coin backend.
- [project/record/tasks/kernel-state-sharing.md](project/record/tasks/kernel-state-sharing.md) — the kernel-state-sharing task and its K0–K2a slices.
- [project/record/tasks/links.md](project/record/tasks/links.md) — `App::Link`, link arrays and their interlocks.
- [project/record/tasks/mesh-module.md](project/record/tasks/mesh-module.md) — the `Mesh`/`MeshPart`/`Mesh::Feature` contract.
- [project/record/tasks/p2-architecture.md](project/record/tasks/p2-architecture.md) — P2: curves, Part/Draft layering, Qt and the SimpleCAD order.
- [project/record/tasks/part-feature-proxies.md](project/record/tasks/part-feature-proxies.md) — Python feature proxies (`Part::FeaturePython` and friends).
- [project/record/tasks/qt-reduction.md](project/record/tasks/qt-reduction.md) — taking Qt out of the paths that do not need it (Q0a–Q5).
- [project/record/tasks/settings-model.md](project/record/tasks/settings-model.md) — provider-advertised settings and the `Settings` tree.
- [project/record/tasks/toolbar-chrome.md](project/record/tasks/toolbar-chrome.md) — the viewport toolbar system and one home for grid settings.
- [project/record/tasks/viewport-event-routing.md](project/record/tasks/viewport-event-routing.md) — viewport event routing: occlusion and the settings card.
- [project/record/tasks/viewport-feedback-and-window-bounds.md](project/record/tasks/viewport-feedback-and-window-bounds.md) — viewport feedback and window-bounds correctness.

### Archive

- [project/record/archive/milestones-detail-2026-10.md](project/record/archive/milestones-detail-2026-10.md) — the M0–M4 slices, the restructure, the MVP and app-shell slices, and the M3 plan.
- [project/record/archive/TODO-26-detail-2026-10-26.md](project/record/archive/TODO-26-detail-2026-10-26.md) — the full 2026-10-26 round-by-round detail.
- [project/record/archive/TODO-23-detail-2026-10-23.md](project/record/archive/TODO-23-detail-2026-10-23.md) — the full 2026-10-23 round-by-round detail.
- [project/record/archive/TODO-22-detail-2026-10-22.md](project/record/archive/TODO-22-detail-2026-10-22.md) — the full 2026-10-22 round-by-round detail.
- [project/record/archive/TODO-21-detail-2026-10-21.md](project/record/archive/TODO-21-detail-2026-10-21.md) — the full 2026-10-21 round-by-round detail.

### Active task specs

- [project/tasks/README.md](project/tasks/README.md) — the active-spec policy and the current ones.
- [project/tasks/simplecad-bringup.md](project/tasks/simplecad-bringup.md) — the SC0–SC8 cut of the SimpleCAD bring-up, including `makeWireString` on the Rust text stack.
- [project/tasks/text-and-3d-text.md](project/tasks/text-and-3d-text.md) — text boundaries: kernel-free outlines vs Part wires/faces, the wire→shell vs face→solid rule, and why there is no DPI or point size.
- [project/tasks/transform-session.md](project/tasks/transform-session.md) — the transform session: explicit uncommitted state (`PendingEdit`/`EditView`), one cumulative move session, loadout slots, hover-to-arm, the 3-D tape and the staged plan A0–U6.
- [project/tasks/ux-research.md](project/tasks/ux-research.md) — the interaction questions U1–U7, their evidence sources and the decision log.
- [project/tasks/creation-tools-ownership.md](project/tasks/creation-tools-ownership.md) — whether creation tools are a core default or an edition/mod choice, and how override precedence works.
- [project/tasks/polyline-vertex-editing-and-active-tool-hud.md](project/tasks/polyline-vertex-editing-and-active-tool-hud.md) — in-flight vertex editing inside the polyline tool, and one dielectric surface for every active action.
- [project/tasks/workplanes.md](project/tasks/workplanes.md) — one workplane state (the world plane is a real plane), ephemeral face binding while drawing, and the `WP` menu as plane choices rather than plumbing.
- [project/tasks/stroke-units.md](project/tasks/stroke-units.md) — the recorded decision that viewport strokes are device pixels, the rejected logical-pixel option, and the plan for reopening it.
