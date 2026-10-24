# Milestones — high-level history

Status: record (compact, 2026-10-24). This page is the timeline and the current
assessment. The per-slice detail — every slice's design, measurements and follow-ups —
is archived in
[`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md).

## Timeline

| When | Phase | What it established |
| --- | --- | --- |
| 2026-09/10 | **M0–M2** | A Rust core behind the `FreeCAD` API: the C ABI replaced by PyO3, then `fc-core`; a headless Python-declared UI spike. |
| 2026-10 | **M3a–M3d** | The `.pyi` inventory, hand-written bindings, generated skeletons, and the conformance harness. Parity became *measurable*. |
| 2026-10 | **M4 slices 1–16** | The App/Base surface: units, geometry values, documents and persistence, extensions and groups, links, observers, expressions, matrix numerics. |
| 2026-10 | **Restructure** | Workspace crates, edition 2024, packaging, and the published `0.1.2` crate set. |
| 2026-10 | **MVP A1–D2** | Object state, label semantics, enumerations, undo/redo, property round-trip, `DocumentBasicCases`, expressions v2. The headless MVP was met. |
| 2026-10 | **App shell S1–S1++++** | The gpui window, console, inspector, property editing, and the widgets extraction. |
| 2026-10 | **Inventor/Coin S1–S4, NaviCube N1–N4** | The renderer replacement, the `pivy.coin` facade, and the navigation cube. |
| 2026-10-21 | **Grid overlay, Draft probe** | The external-mod track (G0–G6), the Draft import-ladder assessment, and the upstream dma-buf issue. |
| 2026-10-22 | **Part conformance push** | The headless `ViewObject`, the `.FCStd` reader, link arrays, the Ctrl+K palette, the Dev Tools window, and the first Part measurements. |
| 2026-10-23 | **Serializers, lineage, Qt** | Native `.ferrocad` + real `.FCStd` ZIP, the lineage layer, Links L0–L5, the `.ui` reader, and the `PySideUic` proxies. |
| 2026-10-24 | **Part completion** | The full Part surface, live `Shape.Placement`, exact 20/47 compound-cut maps, and **529 upstream tests passing with zero errors**. |

## Current state (2026-10-24)

| Surface | Then | Now |
| --- | --- | --- |
| Upstream Part suite | 2 passed, 1 failed, 89 errored, 4 files failed to load | **531 passed, 0 failed, 0 errored, 0 load failures** |
| Upstream Draft suite | no group | 10 passed, 1 failed, 1 load error (upstream Draft, not a shim) |
| Python facade | 377 passed | **444 passed, 17 skipped** (134 subtests) |
| Part module | `makeBox` and a shape handle | primitives, booleans, dress-ups, links and arrays, conics, splines, NURBS conversion, `ShapeFix`, `ChFi2d`, `Sketcher`, feature objects |
| Document format | one JSON blob | native `.ferrocad` + real `.FCStd` ZIP, with an additive lineage sidecar |
| Naming | positional strings | lineage layer with stored maps; upstream's **20/47** compound maps, and the version enabled for reachable shapes |

### Three lessons

1. **The conformance harness is the design tool, not the scoreboard.** Every semantic
   correction in the 2026-10-23/24 round was found by an upstream test that had been
   skipped or unloaded a day earlier. Loading the tests was worth more than adding API.
2. **Separating naming from geometry survived contact.** `ferrocad_lineage` owns the
   vocabulary and policy, the kernel reports provenance, and Part stores and applies it.
   That is why the 20/47 result was a naming change, not a geometry rewrite.
3. **Honest approximations beat silent ones.** Helices are interpolating B-splines,
   threads are helical pipes, and tubes accept only analytic spines — each says so at
   the call site and in the record.

### Two asymmetries that define the next round

1. **Six guarded map asserts still skip.** Four consumer shapes (`makeShell`,
   `makeSolid`, `makeLoft`, `makeFilledFace`) do not yet attach a lineage although their
   element counts already match, and two counts (`FaceMakerCheese` 10,
   `makeShellFromWires` 24) are composed aliases rather than element names.
2. **The Python feature-proxy layer does not exist.** Part works as a library; Draft and
   SimpleCAD need objects whose `execute` is a Python callback. That is the gate between
   the two halves of the project.

### What is deliberately not claimed

- **Drop-in FreeCAD compatibility.** 529 upstream Part tests are a measured subset, not
  the whole workbench: PartDesign, Sketcher's solver, FEM, CAM and the GUI workbenches
  are out of scope here.
- **A finished `.FCStd` writer.** It round-trips our documents; an upstream-FreeCAD open
  check is still a follow-up.
- **Element-map parity.** The two compound cuts match exactly; the other guarded shapes
  do not yet.
- **Any performance claim.** The suite measures behaviour, not speed.

## Where the detail lives

- [`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md) — every
  M0–M4 slice, the restructure, the MVP and app-shell slices, the reactor decisions, and
  the M3 plan, with their measurements.
- [`TODO-21-10-2026.md`](TODO-21-10-2026.md), [`TODO-22-10-2026.md`](TODO-22-10-2026.md),
  [`TODO-23-10-2026.md`](TODO-23-10-2026.md) — the compact closed backlogs; their full
  text is in `archive/`.
- [`incident-2026-10-21-session-crash.md`](incident-2026-10-21-session-crash.md) — the
  `systemd-oomd` post-mortem.
- [`../ROADMAP.md`](../ROADMAP.md) — where the project is going next.
