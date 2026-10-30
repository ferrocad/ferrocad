# Label index — every `S1`, `P4`, `D0`, …

Status: reference (2026-10-27). This is the index for the slice and work-item labels
used across the notes, the backlogs and the code comments. If a label is not here, it is
not a project label (it may be an upstream name, an OCCT class, a file name or a
version).

The labels grew per program, so several letters were reused for unrelated work. When a
label appears in running text, **qualify it with its owner**: `SimpleCAD S4`,
`stage T4`, `lineage S6c`, not just `S4`. The tables below are the authority for which
owner a label belongs to.

## Read this first

| You saw | It can mean | The owner is decided by |
| --- | --- | --- |
| `S1` | the app-shell skeleton · the SimpleCAD headless baseline · the lineage type extraction · the settings schema | the note's subject; `S1` alone is ambiguous |
| `S4` | SimpleCAD interaction parity · app-shell property editor · Coin events + pick · lineage provenance | the note's subject |
| `P4` | the constraints/sketches section of the current backlog · the SimpleCAD section of the 2026-10-27 backlog | always cite the backlog date: `TODO-30 P4` is not `TODO-27 P4` |
| `P2` | the solids/booleans/Draft section of the current backlog · the workbench-platform section of the 2026-10-27 backlog · the Python feature-proxy slices (`P2a`–`P2e`) · the architecture slices (`P2.1`–`P2.5`) | the suffix or the backlog date; bare `P2` is the current backlog section |
| `P1` | the minimum-SimpleCAD-API section of the current backlog · the OCCT seam tail in the 2026-10-27 backlog | cite the backlog date |
| `SK1`–`SK6` | the free-sketcher slices in the current backlog ([`TODO-30-10-2026.md`](TODO-30-10-2026.md) §P4) | `SK` is sketch-only; not SimpleCAD's `S1`–`S5` |
| `D0` | the headless Draft import task | there is only one `D0` |
| `D1` | Dev Tools window slice · NaviCube GPUI slice · MVP demo slice | the note's subject |
| `T4` | the stage G-buffer pick pass · the toolbar-chrome slice · the spatial transactor | the note's subject |
| `M2` | the Part/Draft layering slice (“Draft objects are real”) · the project phase `M2` · Draft M2 acceptance | the note's subject |
| `F1` | the edge dress-up arc · the serializer dispatch split | the note's subject |
| `G1` | the grid-overlay node surface · the workspace gizmo style | the note's subject |
| `A4` | the actions-registry shortcuts slice · (not an MVP slice; the MVP has `A1`–`A3`) | the note's subject |

## Status key

| Mark | Meaning |
| --- | --- |
| ✅ | landed and verified by the named check or record |
| ◐ | partly landed; the open remainder is in the active backlog |
| ▢ | not started or explicitly parked |
| ⊘ | retired namespace; history only, do not add new items |

## Direction, horizons and the backlog

| Label | Owner | Meaning | Status |
| --- | --- | --- | --- |
| `H1` | [`ROADMAP.md`](ROADMAP.md) | Finish Part/OCCT honestly: zero Part failures and no “not implemented” in the advertised seam. | ✅ (optional alias counts only) |
| `H2` | [`ROADMAP.md`](ROADMAP.md) | The workbench platform: Python feature proxies, Draft M2–M5, the Qt-free tool layer. | ◐ |
| `H3` | [`ROADMAP.md`](ROADMAP.md) | Proof by use: SimpleCAD, a published edition, a BIM subset. | ◐ |
| `P0`–`P6` | the active backlog ([`TODO-30-10-2026.md`](TODO-30-10-2026.md)) | Section priorities, reused on every rotation. `P0` is the element-map/naming priority in old backlogs; the current sections start at `P1`. Always cite the backlog date. | the current backlog is open; older `P*` are ⊘ history |
| `P0.5`, `P0.6`, `P0.7`, `P0.8` | [`record/TODO-22-10-2026.md`](record/TODO-22-10-2026.md) | Numbered items in the 2026-10-22 backlog (Dev Tools, chrome, settings, event routing). Not the same as a `P0` section. | ⊘ |
| `M0`–`M4` | [`record/milestones.md`](record/milestones.md) | Project phases: M0 hello-world, M1/M2 the core and Python surface, M3 the binding inventory and conformance, M4 the App/Base surface (16 slices). | ✅ |

### MVP slices (historic)

The headless MVP track lives in [`direction/mvp-path.md`](direction/mvp-path.md). These
are `A`/`B`/`C`/`D` **slices**, not the actions registry or the Qt options.

| Label | Meaning |
| --- | --- |
| `A1`–`A3` | Model: object state, `PropertyEnumeration`/type checking, link completeness. |
| `B1`–`B5` | Workflow: label semantics, undo/redo, containers, expressions v2, extension lifecycle. |
| `C1`–`C3` | Persistence: property round-trip, `Content`/XML validation, file-included apply. |
| `D1`–`D3` | MVP surface: the demo/CI, the remaining `DocumentBasicCases`, the `FreeCADInitTests` stretch. |

The namespace is ⊘: the headless MVP was met on the `A1`–`D2` subset and later work
absorbed the rest. Do not reuse `A`–`D` for new slices without the owner prefix.

## The Part/Draft/BIM stack

| Label | Owner | Meaning | Status |
| --- | --- | --- | --- |
| `M0`–`M5` | [`../developer/architecture/part-draft-layering.md`](../developer/architecture/part-draft-layering.md) | The layering slices: M0 headless `import Draft`, M1 analytic curves, M2 real Draft document objects (the proxies), M3 the modification layer, M4 import/export, M5 BIM on top. **Distinct from the project phases `M0`–`M4`.** | M0–M2 ✅; M3 ◐ (31/0/2, two hand-offs); M4 ◐ (import surface started, `test_svg`/`test_draftgeomutils` remain); M5 ▢ |
| `S1`–`S6c` | [`../developer/architecture/lineage-and-naming.md`](../developer/architecture/lineage-and-naming.md) | The lineage layer: S1 generic types, S2 trait/policy, S3 Part stores a map, S4 provenance, S5 map-first resolution, S6 both formats, S6a the exposed reverse map, S6b the `.FCStd` sidecar, S6c the compound-cut counts. | ✅ |
| `F1`–`F8` | [`record/tasks/format-serializers.md`](record/tasks/format-serializers.md) | The serializer split: F1 dispatch, F2 native envelope, F3 migrate artefacts, F4 reader gaps, F5 ZIP writer, F6 writer breadth, F7 `GuiDocument.xml`, F8 the switch and the lineage sidecar. **Distinct from `F0`/`F1` in interaction.** | ✅ |
| `L0`–`L5` | [`record/tasks/links.md`](record/tasks/links.md) | `App::Link`, link arrays, sub-element paths and the lineage/format interlocks. | ✅ |
| `P2a`–`P2e` | [`record/tasks/part-feature-proxies.md`](record/tasks/part-feature-proxies.md) | The Python feature-proxy slices: registration/execute, `addObject`/`onChanged`, persistence, `Part2DObjectPython`, Draft M2 acceptance. | ✅ |
| `P2.1`–`P2.5` | [`record/tasks/p2-architecture.md`](record/tasks/p2-architecture.md) | The architecture decisions: `ferrocad_curves`, the Part/Draft seam, the gated Qt split plane, the SimpleCAD order, `QtSvg`. | P2.1/P2.2 ✅, P2.3 ✅ (via Q0–Q5), P2.4 ✅, P2.5 optional |
| `D0` | [`record/tasks/d0-draft-import.md`](record/tasks/d0-draft-import.md) | The headless Draft import task and its six fixes (`D0.1`–`D0.6`). **Distinct from every other `D`.** | ✅ |
| `M1`, `M1b` | [`record/tasks/mesh-module.md`](record/tasks/mesh-module.md) | The Mesh module: M1 the `Mesh`/`MeshPart`/`Mesh::Feature` contract, M1b facet groups, `.bms` and the `.FCStd` mesh member. | ✅ |
| `E1`–`E4` | [`../developer/architecture/edition-publishing.md`](../developer/architecture/edition-publishing.md) | Publishing `FerroCAD: Parametric`: E1 the OCCT bundle, E2 the release matrix, E3 the macOS/Windows closures, E4 an optional wheel. | E1/E2 ✅, E3 ◐, E4 ▢ |
| `K0`–`K4` | [`../developer/architecture/kernel-state-sharing.md`](../developer/architecture/kernel-state-sharing.md) | The one-core invariant: K0 identity, K1 mixed-image guard, K2 interpreter-scoped Python state (`K2a` tables, `K2b` nothing to do, `K2c` reclamation), K3 the `Kernel` facade, K4 the modular-wheel capsule. | K0/K1/K2a/K3 ✅; K2c/K4 ▢ (parked) |

## The desktop app

| Label | Owner | Meaning | Status |
| --- | --- | --- | --- |
| `S1`–`S6` | [`direction/app-shell-vision.md`](direction/app-shell-vision.md) | The app shell: S1 skeleton, S1.5 viewport wiring, S2 document lifecycle/commands, S3 inspector, S4 property editor, S5 console, S6 polish. **Distinct from SimpleCAD, lineage and settings `S`.** | S1–S5 ✅, S6 ◐ |
| `A0`–`A5` | [`../developer/workbenches/actions-and-tools.md`](../developer/workbenches/actions-and-tools.md) | The command registry: A0 registration, A1 shell bridge, A2 selection/built-ins, A3 dialogs, A4 shortcuts/status, A5 view/tool requests. **Distinct from the MVP `A` slices.** | ✅ |
| `T0`–`T7` | [`record/tasks/toolbar-chrome.md`](record/tasks/toolbar-chrome.md) | The viewport chrome: header controls, popovers, the action dock, the title-bar menu, grid settings. **Distinct from the stage and transactor `T`.** | ✅ |
| `D1`–`D6` | [`record/tasks/dev-tools-window.md`](record/tasks/dev-tools-window.md) | Dev Tools: D1 extract panels, D2 the second window, D3 slim the main window, D4 toggle/lifecycle, D5 tests, D6 docs. **Distinct from Draft `D0` and NaviCube `D1`.** | ✅ |
| `S0`–`S4c` | [`record/tasks/settings-model.md`](record/tasks/settings-model.md) | Provider-advertised settings: S0 schema/store, S1 `Settings` nodes, S2 pane rendering, S3 file-backed persistence, S4 the grid migration, S4b the axis cross, S4c the gizmo capability, S5 document overrides. **Distinct from other `S`.** | S0–S4c ✅, S5 optional |
| `Q0`–`Q5` | [`../developer/workbenches/qt-strategy.md`](../developer/workbenches/qt-strategy.md) | Qt reduction: Q0 the headless contract, Q1 the `.ui` reader, Q2 the proxy/micro-shim, Q3 the core forms, Q4 the native spatial reference, Q5 the remaining forms. | ✅ |

## Interaction, viewport and rendering

| Label | Owner | Meaning | Status |
| --- | --- | --- | --- |
| `F0`, `F0.0`–`F0.6` | [`../developer/interaction/topology-selection.md`](../developer/interaction/topology-selection.md) | The topology/selection prerequisite for the manipulators. **Distinct from the serializer `F1`–`F8`.** | ✅ |
| `F1` (interaction) | [`../developer/interaction/gizmo-manipulators.md`](../developer/interaction/gizmo-manipulators.md) | The edge dress-up arc on the `F0` seam. Always write `interaction F1`. | ✅ |
| `T1`–`T4` | [`../developer/interaction/gizmo-manipulators.md`](../developer/interaction/gizmo-manipulators.md) | The spatial transactor: T1 numeric entry, T2 axis constraint, T3 plane/ring constraint, T4 the readout/edge cases. **Distinct from stage `T` and chrome `T`.** | ✅ |
| `T0`–`T6` | [`../developer/viewport/stage.md`](../developer/viewport/stage.md) | The declarative stage: T0 spine, T1 coexistence, T1.5 viewport bridge, T2 tools, T3 `TransformGizmo`, T4 the G-buffer pick pass, T5 PyO3 bindings, T6 the NaviCube producer. **Distinct from chrome `T0`–`T7` and transactor `T1`–`T4`.** | ✅ |
| `V0`–`V4` | [`../developer/interaction/visual-language.md`](../developer/interaction/visual-language.md) | The visual language: V0 tokens, V1 occlusion, V2 canvas/surfaces, V3 states, V4 type/bevels (with V0b normalisation and V2c translucency deferred). **Distinct from the NaviCube producer `V1`–`V4`.** | V0 ◐, V1/V2 ✅, V3 ◐, V4 ◐ |
| `N1a`–`N4b` | [`../developer/interaction/navicube.md`](../developer/interaction/navicube.md) | The NaviCube: N1a sub-rect viewport, N1b overlay, N2 cube/controller, N3 interaction, N4a edges/corners/drag, N4b polish. | N1–N4a ✅, N4b ◐ |
| `V1`–`V4` (NaviCube) | [`../developer/interaction/navicube-plan.md`](../developer/interaction/navicube-plan.md) | The producer-independent NaviCube: V1 move the model in, V2 the stage producer, V3 config/theme, V4 the directives channel (`V4.1`–`V4.2` built, `V4.3` open). Always write `navicube V…`. | V1–V3 ✅, V4.1–V4.2 ✅, V4.3 ▢ |
| `D1`–`D5` (NaviCube) | [`../developer/interaction/navicube-gpui-design.md`](../developer/interaction/navicube-gpui-design.md) | The GPUI-native NaviCube redesign: the wireframe gyro-cube, hover/proximity, compass ring, projection toggle. Always write `navicube-gpui D…`. | ✅ |
| `WA1`–`WA9` | [`../developer/interaction/navicube-gpui-design.md`](../developer/interaction/navicube-gpui-design.md) | The NaviCube visual/input passes (labels, compass, roll chevrons, yaw, etc.). | ✅ |
| `S1`–`S4` (Coin) | [`../developer/viewport/inventor-plan.md`](../developer/viewport/inventor-plan.md) | The selective Open Inventor slice path: S1 facade spine, S2 geometry fields, S3 scene extraction, S4 events/pick. Always write `Coin S…`. | ✅ |
| `G0`–`G6` (grid) | [`../developer/workbenches/grid-overlay-plan.md`](../developer/workbenches/grid-overlay-plan.md) | The grid-overlay program: G0 Coin compatibility, G1 node/compiler/state, G2 the semantic grid API, G3 shader nodes, G4 the wgpu program path, G5 the A1 fork, G6 the shipping addon. **Distinct from the gizmo `G1`–`G3b`.** | G0–G4 ✅, G5 ▢, G6 ✅ |
| `G1`–`G3b` (gizmo) | [`../developer/interaction/gizmo-manipulators.md`](../developer/interaction/gizmo-manipulators.md) | The workspace gizmo style (G1), the shader skin (G2), mod-declared handles (G3a), and the remaining plane handles/shape-API input (G3b). Always write `gizmo G…`. | G1–G3a ✅, G3b ▢ |
| `O0`–`O4` | [`research/grid-overlay-bringup.md`](research/grid-overlay-bringup.md) | The first grid-addon import ladder: measure/extract (O0–O2), author WGSL (O3), manual run (O4). | ⊘ superseded by `G0`–`G6` |

## SimpleCAD

| Label | Owner | Meaning | Status |
| --- | --- | --- | --- |
| `S0` | [`research/simplecad-bringup.md`](research/simplecad-bringup.md) | The audit and the `--simplecad` conformance hook. | ✅ |
| `S1` | same | The headless unit baseline: all 88 modules attempted and classified. | ✅ |
| `S2` | same | The `Part` planar/topology slice (construction, queries, booleans, extrude, persistence). | ◐ |
| `S3` | same | The curved/dress-up slice (cylinders/spheres/cones/tori, curve/surface types, offset/thickness, fillet/chamfer, loft/sweep); `S3.4` is the mesh decision. | ◐ |
| `S4` | same | Interaction parity: `S4.1` view facade, `S4.2` ranked pick, `S4.3` `SelectionObject`, `S4.4` snapping, `S4.5` push/pull, `S4.6` previews. | ◐ (`S4.1`–`S4.3` in the tree, `S4.4` partial, `S4.5` via the FerroCAD push/pull, `S4.6` open) |
| `S5` | same | The GUI workbench: the Qt split plane, commands/toolbars, previews on stage, GUI suites under Xvfb. | ▢ |

## Historic and non-label names

| Name | What it is |
| --- | --- |
| `A`, `B`, `C`, `D` in an assessment | Option letters in a design comparison (for example the grid bring-up's A–D), not slice labels. |
| `O0`–`O4` | Retired grid import ladder, replaced by `G0`–`G6`. |
| `U1`, `U2` | Upstream `ElementMap` postfixes (`;U1`, `;U2`), not project labels. |
| `FaceMakerCheese` (10), `makeShellFromWires` (24) | The two guarded element-map sizes that remain documented non-results. |
| `P0.5`, `P0.6`, `P0.7`, `P0.8` | Items in the closed 2026-10-22 backlog, not current priorities. |

## The rule for new labels

The single-letter space is exhausted and several letters already mean two things. New
work uses an **area prefix**:

| Prefix | Area |
| --- | --- |
| `PART-` | the Part/OCCT kernel and its Python surface |
| `DRAFT-` | the Draft workbench |
| `SIMPLE-` | SimpleCAD |
| `MESH-` | Mesh/MeshPart |
| `GIZMO-` | the interaction gizmo and manipulators |
| `STAGE-` | `ferrocad_stage`, viewport and rendering |
| `SHELL-` | the gpui shell, chrome and Dev Tools |
| `INTERACT-` | selection, snapping, pointer routing |
| `QT-` | the Qt-replacement program |
| `KERNEL-` | kernel identity and state sharing |
| `LINEAGE-` | topological naming |
| `LINK-` | `App::Link` and sub-element references |
| `DOC-` | documentation, the book and the generated pages |
| `REL-` | packaging, editions and releases |
| `BUILD-` | build, CI and the sandbox |

Rules:

1. A new label is `AREA-<n>` (for example `PART-12`), and it is registered in the table
   for that area **before** it is used.
2. Existing single-letter labels are frozen. Do not renumber or reuse them; qualify
   them in prose when the owner is not obvious.
3. A retired namespace is marked ⊘ and is never revived. If a new idea fits an old
   letter, give it an area prefix instead.
4. A backlog section number (`P4`) is a position in one dated backlog, not a permanent
   identity. Cite it as `TODO-27 P4`; the durable name is the area label.
