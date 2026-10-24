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
| 2026-10-24 | **Part completion** | The full Part surface, live `Shape.Placement`, exact 20/47 compound-cut maps, and **531 upstream tests passing with zero errors**. |
| 2026-10-24 | **Draft M2 green** | Attachment, `PropertyLinkSubList`, the link extension, sub-shape `getSubObject` and the hatch shim: the two Draft M2 modules pass **27/27** with zero load failures. |
| 2026-10-24 | **Qt reduction Q4/Q5** | A native `FreeCADGui.Snapper`/`draftToolBar` over a camera-free snap API, async point sessions, the native line/wire tools and `PlaneGui`, the checkable snap bar, and the four remaining Draft forms; `pytest tests` reaches **538 passed** ([round log](2026-10-24-q4-q5-draft-tools.md)). |

## Current state (2026-10-24)

| Surface | Then | Now |
| --- | --- | --- |
| Upstream Part suite | 2 passed, 1 failed, 89 errored, 4 files failed to load | **531 passed, 0 failed, 0 errored, 0 load failures** |
| Upstream Draft suite | no group | 10 passed, 1 failed, 1 load error (upstream Draft, not a shim) |
| Draft M2 (`test_creation`, `test_array`) | 15 passed, 1 failed, 11 errored | **27 passed, 0 failed, 0 errored, 0 load failures** |
| Python facade | 377 passed | **538 passed, 8 skipped** (172 subtests; 480 passed / 66 skipped without the stage extension) |
| Part module | `makeBox` and a shape handle | primitives, booleans, dress-ups, links and arrays, conics, splines, NURBS conversion, `ShapeFix`, `ChFi2d`, `Sketcher`, feature objects |
| Document format | one JSON blob | native `.ferrocad` + real `.FCStd` ZIP, with an additive lineage sidecar |
| Naming | positional strings | lineage layer with stored maps; every reachable guarded size matches (52/58/26/24/9/6/60) plus the cuts' 20/47 |
| Workbench platform | no Python-defined objects | `Part::FeaturePython`/`Part2DObjectPython` execute proxies and attach; the Draft M2 modules run green (27/27); the Draft tool surface (Snapper, tool bar, point sessions, line/wire, forms) is native |

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

### The remaining Part asymmetry

**Two guarded map counts remain documented non-results.** `FaceMakerCheese` (10) needs
upstream's face-combo alias and `makeShellFromWires` (24) needs a different (ruled-shell)
topology. Every other guarded size is reproduced.

The attachment gate is crossed: both Draft M2 modules run green, and the Qt-reduction
plan's Q4/Q5 slices now supply the native spatial reference and the remaining forms.
The next round is SimpleCAD S1 rather than more Part surface.

### What is deliberately not claimed

- **Drop-in FreeCAD compatibility.** 531 upstream Part tests are a measured subset, not
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
- [`2026-10-24-q4-q5-draft-tools.md`](2026-10-24-q4-q5-draft-tools.md) — the closed
  Q4/Q5 round: what landed, the measurements, and the deviations it records.
- [`incident-2026-10-21-session-crash.md`](incident-2026-10-21-session-crash.md) — the
  `systemd-oomd` post-mortem.
- [`../ROADMAP.md`](../ROADMAP.md) — where the project is going next.
