# Milestones — high-level history

Status: record (compact, 2026-10-27). This page is the timeline and the current
assessment. The per-slice detail — every slice's design, measurements and follow-ups —
is archived in
[`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md). The
labels are disambiguated in [`../labels.md`](../labels.md).

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
| 2026-10-25/26 | **Interaction layer, v0.1.4** | The wheel and floating Move form, re-pivot, select mode, the gizmo kinds/registry, the feathered ribbon and hover casing, edition appearance presets, the `opencascade-sys` MSVC fix and the Windows edition leg. v0.1.3 and v0.1.4 were tagged. |
| 2026-10-26 | **Mesh and SimpleCAD S1** | `ferrocad_mesh`, `ferrocad_mesh_py`, `Part.Shape.tessellate`, `BOPTools.SplitAPI`; the first 88-module SimpleCAD baseline and the one-extension packaging fix. |
| 2026-10-27 | **Part breadth, Draft M3, gizmo set** | The Part shape-member batch (`isInside`, `hashCode`, tolerance family, `Shape.read`, `exportStl`, …), Draft M3's first slice, the gizmo **set** host with session re-narrowing, the `.FCStd` mesh member, and the S2–S3 SimpleCAD baseline (**702/44/368/2**, 88/88). |
| 2026-10-27 (in the tree) | **Fifth fan-out** | SimpleCAD S4.1 view facade, S4.2 ranked pick, S4.3 `SelectionObject`; the Draft M4 import round (`test_modification` **31/0/2**); the Part `part_ops` batch (248/408); the offscreen snapshot path, the `ferrocad_snapshot` renderer and the `tools/screenshot.py` viewport/chrome/desktop scopes, and the first user-doc images with a spec/PNG manifest; the Draft conformance harness. Uncommitted; the full suite is **631/3/4**, and the three failures are test expectations the round has not updated. |

## Current state (2026-10-27)

| Surface | Then | Now |
| --- | --- | --- |
| Upstream Part suite | 2 passed, 1 failed, 89 errored, 4 files failed to load | **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures** |
| Upstream Draft suite | no group | The `--draft` group (ten files) is **44 passed, 0 failed, 0 errored, 0 load errors**; `test_modification` is **31/0/2** after the fifth round's M4 import work (22/1/10 before it, 17/0/16 at M3's start) |
| Draft M2 (`test_creation`, `test_array`) | 15 passed, 1 failed, 11 errored | **27 passed, 0 failed, 0 errored, 0 load failures** |
| Python facade | 377 passed | **588 passed, 3 skipped** (225 subtests) at the last recorded baseline; the worktree adds 47 passing facade/selection tests and a full run of **631/3/4** whose three failures are test expectations for the new wrapper/`Part::Prism` behaviour |
| Part module | `makeBox` and a shape handle | Primitives, booleans, dress-ups, links and arrays, conics, splines, NURBS conversion, `ShapeFix`, `ChFi2d`, `Sketcher`, feature objects; **220/408** upstream method names committed (**248/408** in the worktree) |
| Document format | one JSON blob | Native `.ferrocad` + real `.FCStd` ZIP, with an additive lineage sidecar and a Mesh member |
| Naming | positional strings | Lineage layer with stored maps; every reachable guarded size matches (52/58/26/24/9/6/60) plus the cuts' 20/47; two composed-alias counts remain documented non-results |
| Workbench platform | no Python-defined objects | `Part::FeaturePython`/`Part2DObjectPython` execute proxies and attach; Draft M2 is green; Draft M3's first slice is in; the Draft tool surface is native |
| SimpleCAD | not run | **88/88 modules load, 702 passed / 44 failed / 368 errored / 2 skipped, no aborts** (integrated tree, before the fifth round) |
| Interaction | no wheel/gizmos | The wheel and Move HUD, re-pivot, select mode, the gizmo registry/set, rotate-ring policy; the S4.1–S4.3 view facade is in the worktree |

### Three lessons

1. **The conformance harness is the design tool, not the scoreboard.** Every semantic
   correction in the 2026-10-23/24 round was found by an upstream test that had been
   skipped or unloaded a day earlier. Loading the tests was worth more than adding API.
2. **Separating naming from geometry survived contact.** `ferrocad_lineage` owns the
   vocabulary and policy, the kernel reports provenance, and Part stores and applies it.
   That is why the 20/47 result was a naming change, not a geometry rewrite.
3. **Honest approximations beat silent ones.** A helix has no polynomial 3D OCCT curve,
   so its 3D representative is an interpolating B-spline built from the exact pcurve;
   the seam says so at the call site and in the record. The same rule now applies to
   measurements: a stale extension is labelled stale, not reported as a regression.

### The remaining Part asymmetry

**Two guarded map counts remain documented non-results.** `FaceMakerCheese` (10) needs
upstream's face-combo alias and `makeShellFromWires` (24) needs a different (ruled-shell)
topology. Every other guarded size is reproduced. The alias work is optional; no
advertised API raises “not implemented”.

The workbench gate is crossed: both Draft M2 modules run green, the Qt-reduction
Q0–Q5 slices supply the native spatial reference and the remaining forms, and SimpleCAD
runs through S3. The next round is the SimpleCAD S4 remainder and the Draft M3–M5
breadth, not more Part surface for its own sake.

### What is deliberately not claimed

- **Drop-in FreeCAD compatibility.** 531 upstream Part tests are a measured subset, not
  the whole workbench: PartDesign, Sketcher's solver, FEM, CAM and the GUI workbenches
  are out of scope here.
- **A finished `.FCStd` writer.** It round-trips our documents; an upstream-FreeCAD open
  check is still a follow-up, and some module payloads remain lossy.
- **Element-map parity everywhere.** Every reachable guarded size matches; the two
  composed-alias non-results do not, and the alias policy is optional.
- **Any performance claim.** The suite measures behaviour, not speed.

## Where the detail lives

- [`TODO-27-10-2026.md`](TODO-27-10-2026.md) — the closed 2026-10-27 backlog and the fifth
  to eighth rounds' state.
- [`TODO-26-10-2026.md`](TODO-26-10-2026.md) — the compact closed 2026-10-25/27 record;
  the full round detail (interaction, Draft, SimpleCAD, Mesh and kernel state) is in
  [`archive/TODO-26-detail-2026-10-26.md`](archive/TODO-26-detail-2026-10-26.md).
- [`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md) — every
  M0–M4 slice, the restructure, the MVP and app-shell slices, the reactor decisions, and
  the M3 plan, with their measurements.
- [`tasks/README.md`](tasks/README.md) — the closed task specs, indexed by label and
  closing backlog.
- [`TODO-21-10-2026.md`](TODO-21-10-2026.md), [`TODO-22-10-2026.md`](TODO-22-10-2026.md),
  [`TODO-23-10-2026.md`](TODO-23-10-2026.md), [`TODO-24-10-2026.md`](TODO-24-10-2026.md)
  — the compact closed backlogs; their full text is in `archive/` or, for the 24 record,
  in the file itself.
- [`2026-10-24-q4-q5-draft-tools.md`](2026-10-24-q4-q5-draft-tools.md) — the closed
  Q4/Q5 round: what landed, the measurements, and the deviations it records.
- [`incident-2026-10-21-session-crash.md`](incident-2026-10-21-session-crash.md) — the
  `systemd-oomd` post-mortem.
- [`../ROADMAP.md`](../ROADMAP.md) — where the project is going next; [`../labels.md`](../labels.md)
  — what the letters mean.
