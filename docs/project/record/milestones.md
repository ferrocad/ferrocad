# Milestones — high-level history

Status: record (compact, 2026-11-16). This page is the timeline and the current
assessment. The per-slice detail — every slice's design, measurements and follow-ups —
is archived in
[`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md). The
November round is [`2026-11-13-november-round.md`](2026-11-13-november-round.md). The
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
| 2026-10-29/30 | **Workplanes, datum planes and the grid** | The world plane as a real state and `WP` as a selector, three-point pick, `Part::DatumPlane` and `Save as datum plane`, the offset stem, the workplane carried in the program frame, adaptive grid spacing with a minor style and stroke widths, in-flight polyline vertex editing with one active-action HUD and a creating `finish`; four grid defects fixed with regression tests. SimpleCAD **969/64/26/2** over 88/88; Rust lib tests **1169** ([round log](2026-10-29-workplane-visual-hierarchy-and-grid.md)). |
| 2026-10-30 | **Fan-out over the backlog** | Six read-only streams checked the new backlog against the tree and corrected seven claims: `Shape.ShapeType`'s inference is the SimpleCAD `NoneType` cause, `makeWireString` is a FreeType port, `Part.Wire` is not the Draft failure, booleans ignore `Refine`, the P3 addons are absent, SK4's creation half does not exist, and macOS `app.sh` already exists. PlanGCS was verified bindable (not templated, no OCCT) ([fan-out record](2026-10-30-fan-out-todo-30.md)). SC1 then made `Shape.ShapeType` exact (SimpleCAD 969/64/26 → **985/49/23**) and SC2 added `Part.makeWireString` on `skrifa`/`read-fonts`, closing the last `Part.*` name. A 3-D text workflow followed (wires → Bullseye faces → extrude/etch), then upstream's `Draft.make_shape_string` as a scripted ShapeString object. |
| 2026-11-01 | **OCCT in Rust** | `ferrocad_part_cxx` is gone: `ShapeFix`, `ChFi2d`, topology, boolean history and the curve/geom2d helpers ported to Rust over `ferrocad_occt_cxx`'s sys bindings. The kernel seam is one code path. |
| 2026-11-02/04 | **The browser host** | GPUI + wgpu + WebGL2 in the browser, the wasm OCCT kernel, the shell crate split and the M0–M6 browser shell: a real `Part::Box` + `Part::Fillet` document, editing, `.FCStd` open/save and a headless smoke check. |
| 2026-11-04/05 | **Hosts and backends** | `ScriptHost`, the host `FilePicker`, `ferrocad_shell_rust`, one display path, shared property presentation, the command catalogue as data, and both browser script halves over `RustAppBackend` ([host-backends](tasks/host-backends.md)). |
| 2026-11-08/12 | **Backend parity and the mod format** | One `AppBackend`; the legacy `PythonAppBackend` deleted; every shown command a `FreeCADGui` command; `rquickjs` and RustPython halves; the `ferrocad_addon` ABI; fetched packs carrying their `Resources/`; one Coin scene core with CPython and RustPython adapters (B0–B21, [close-out](tasks/backend-parity.md)). |
| 2026-11-13 | **The rotation fix** | `RootNode` builds through the imported `pivy.coin` (Part back to **531/0/0/12**), the SimpleCAD in-process harness exports the worker's `PYTHONPATH`/OCCT library path (back to **985/49/23/2**), the Part API contract is re-pinned and the generated API/user pages are regenerated. SC3 then classifies the 5 Qt groups as skips and SC7 pins the draw → push/pull → cut → STL acceptance (**985/49/18/7**). |
| 2026-11-13 | **The SimpleCAD façade cut closes (P1)** | SC4 publishes the viewport's active workplane and resolves exact face frames (`FreeCADGui.getWorkplane`/`getFacePlane`); SC5 proves the selection-armed push/pull stem edits the document in one transaction; SC6 keeps `FreeCADGui.Preview` as the shape-preview channel, makes 3-D text a non-goal, and completes the `SoGroup` lifecycle (`getChild`/`insertChild`/`removeChild`/`removeAllChildren`/`findChild`, `isOfType`). SC8 then re-bucketed all 49 failures (37 P2, 11 P3, 1 documented test assumption), so **P1 is closed** with the baseline unchanged at **985/49/18/7**. |
| 2026-11-14/15 | **The P2 boolean/seam round** | The container objects read `Refine`, honour `MultiCommon.Behavior`, flatten a lone compound and enforce `MultiFuse` arity (fidelity: totals unchanged). Then `Shape.generalFuse` answers every input from one build, a replaced vertex reports its result image, `Edge.Orientation` and an edge's `Placement` reach the consumers, a shape-wrapped wire counts its edges, `Face.addWire` exists, a circle's frame reaches its edge's parameter 0, `Shape.fuse`/`cut`/`common` take upstream's `(other, tolerance, *, noElementMap)` signature and honour the tolerance, and `Shape.tessellate` defaults to upstream's `defaultAngularDeflection`. `Face.cutHoles` and a circle-based `Cylinder` close the mesh-surface pair, a `flatmesh` unfold that moved a vertex its own fan had laid is fixed, and the transaction log gains `Change::Label` plus `set_property_derived`, so a rename and a placement's baked shape undo with the edit that made them. SimpleCAD **985/49/18/7 → 1026/8/13/7**, P3 11 → 1; Part stays **531/0/0/12**. The `close_holes` experiment closed `test_edge_deletion` but edited the reference checkout, so it was reverted and the two failures were attributed to an upstream policy ([round log](2026-11-14-p2-boolean-seam.md)). |
| 2026-11-16 | **The placement representation** | A `PartShape` keeps its un-moved geometry beside the accumulated placement and transforms from it, so clearing a `Placement` is bit-exact (the principal-axis tie resolves to X again) and `Shape.copy()` hands the base over; a rigid placement write becomes `GeometryBackend::relocate`, a `TopLoc_Location` on the same TShape, so `isPartner` survives a move and the per-face style mapping holds. A moved box reads `1000.000000`, not `1000.0000000000002`. SimpleCAD **1026/8/13/7 → 1028/6/13/7**: the last case that looked like P2 (`test_precision`'s tilted push) was then traced to the reference check's own omitted fuzzy — our boolean matches upstream's call and OCCT 7.9.2 to 12 digits — so no P2 failure remains and only P3 keeps one ([round log](2026-11-14-p2-boolean-seam.md)). |
| 2026-11-16 | **The refused boolean and three API gaps** | A boolean OCCT cannot run (a solid cut by an open shell) returns a null result, and upstream turns that into `NullShapeException`, a `ValueError`; the binding raised `Part.OCCError`, so SimpleCAD's `preserves_volume` errored instead of refusing. `Shape.fuse`/`cut`/`common`/`multiFuse` now split a not-done null result (`ValueError`) from an OCCT exception (`Part.OCCError`), pinned by `a_refused_boolean_raises_value_error_like_upstream`. `test_surface_wrap`'s last failure — the inverse returning `-45` for `35` on the opposite face — was traced to the reference's rank-deficient residual and corridor tie, not the seam. Reviewing the error bucket then closed three gaps: `generalFuse`/`childShapes`/`SubShapes` return typed shapes (a sliced-off vertex answers `.Point`), `Part.Toroid.parameter(point)` exists, and `BSplineSurface.interpolate` keeps the interpolated geometry. SimpleCAD **1032/6/9/7**: 6 failures, all documented assumptions and no P2 or P3 failure; 9 errors, 4 of which need a PartDesign workbench ([round log](2026-11-14-p2-boolean-seam.md)). |
| 2026-11-16 | **The Draft remainder** | `test_draftgeomutils` turned out already green (**10/0/0/0**) from the wire/vertex-order seam rounds, so it joined the `--draft` gate (**60/0/0/1** over eleven files). `TechDraw::DrawPage`, `DrawSVGTemplate` and `DrawViewDraft` are registered document types with `DocumentObject.addView`, and `OutList` is every outbound link, so `test_modification` reaches **32/0/1** with only `Sketcher` left. `Import.readDXF` is recorded as out of surface: the blocker is the intermediate object model the post-import processor consumes, not the DXF grammar. Part stays **531/0/0/12**; the API contract re-pins to 43 object types at **253/408**. |
| 2026-11-16 | **The PartDesign dress-up** | The four remaining SimpleCAD errors were a missing `import PartDesign`, not a Boolean feature: the draft tool builds a `PartDesign::Body`, a `PartDesign::Plane` hinge and a `PartDesign::Draft` in a private document. Upstream's draft is a general dress-up needing `BRepOffsetAPI_DraftAngle`, which the pinned sys crate does not bind, so the feature implements the straight-prism class (one solid, every side wall selected, two end caps, the neutral plane through one) with the tapered-extrusion construction and refuses the rest by name. A drafted box is an exact frustum (far cap 333.070775677 mm²) and a drafted cylinder wall is a real `Part.Cone`. SimpleCAD **1032/6/9/7 → 1036/6/5/7**; Part stays **531/0/0/12**; the contract re-pins to 45 object types ([round log](2026-11-14-p2-boolean-seam.md)). |

## Current state (2026-11-16 baseline)

Measured on the current tree, upstream `33adf61`, SimpleCAD `9180c4b` (clean). The Part and
SimpleCAD rows are in-process runs; the Rust rows are lib tests only. The evidence is in
the [November round log](2026-11-13-november-round.md) and the
[P2 seam round](2026-11-14-p2-boolean-seam.md).

| Surface | Then (2026-10-27/30) | 2026-11-13 | 2026-11-16 |
| --- | --- | --- | --- |
| Upstream Part suite | 531 passed, 0 failed, 0 errored, 12 skipped | 531/0/0/12 (restored) | **531/0/0/12** |
| Upstream Draft suite | `--draft` 44/0/0/0; `test_modification` 31/0/2 | `--draft` 50/0/0/1; 31/0/2 | **`--draft` 60/0/0/1; `test_modification` 32/0/1** |
| Draft M2 | 27/0/0/0 | 27/0/0/0 | **not re-run** (carried) |
| Python facade | 631/3/4 (worktree) | not re-measured; the in-process runs are the gate | **not re-run** (carried) |
| Part module | 248/408 names in the worktree | 252/408 (61.8%), 21 shared classes | **253/408 (62.0%)**, `--check` clean |
| SimpleCAD | 702/44/368/2 | 985/49/18/7 over 88/88; **P1 closed** | **1036/6/5/7 over 88/88**; no P2 or P3 failure / 6 test assumptions (+1 error assumption) |
| Rust lib tests | 1 186 across 15 crates | 1 454 passed, 0 failed | **`ferrocad_occt` 82, `ferrocad_part` 59, `ferrocad_core` 137, `ferrocad_mesh` 64, `part_py` 366, `part_shape_ops` 21**, 0 failed (not a full aggregate) |
| Book | 36 chapters | 48 chapters | **48 chapters** |
| Hosts | desktop + a browser spike | one document, four runtimes; the browser shell runs the same backend | **unchanged** |

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

**The kernel contract is complete and the suite is green.** `FaceMakerCheese` (10) and
`makeShellFromWires` (24) remain documented non-results, and `makeCone`'s partial
revolution plus STEP/IGES import remain advertised limits. Nothing else is outstanding.

The workbench gate is crossed: both Draft M2 modules run green, the Qt-reduction
Q0–Q5 slices supply the native spatial reference and the remaining forms, and SimpleCAD
runs through S3. The next round is the SimpleCAD S4 remainder and the Draft M3–M5
breadth, not more Part surface for its own sake.

### What is deliberately not claimed

- **Drop-in FreeCAD compatibility.** The upstream Part tests are a measured subset, not
  the whole workbench: PartDesign, Sketcher's solver, FEM, CAM and the GUI workbenches
  are out of scope here.
- **A finished `.FCStd` writer.** It round-trips our documents; an upstream-FreeCAD open
  check is still a follow-up, and some module payloads remain lossy.
- **Element-map parity everywhere.** Every reachable guarded size matches; the two
  composed-alias non-results do not, and the alias policy is optional.
- **Any performance claim.** The suite measures behaviour, not speed.

## Where the detail lives

- [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) — the active backlog and the
  measurement caveats at this rotation.
- [`2026-11-14-p2-boolean-seam.md`](2026-11-14-p2-boolean-seam.md) — the P2 boolean/seam
  round: one general-fuse build, vertex images, `Edge.Orientation`/`Placement`, the wire
  wrapper, `Face.addWire` and the circle frame, the signed boolean tolerance, the 25 SimpleCAD failures and
  one error it closed, and the reverted `close_holes` experiment.
- [`2026-11-13-november-round.md`](2026-11-13-november-round.md) — the November round:
  the platform work, the one real regression and the harness gap, the commands that
  reproduce them, and (from §5) the manipulator editor round.
- [`TODO-31-10-2026.md`](TODO-31-10-2026.md) — the closed 2026-10-31 backlog.
- [`2026-10-29-workplane-visual-hierarchy-and-grid.md`](2026-10-29-workplane-visual-hierarchy-and-grid.md)
  — the workplane/grid round: what landed, the four grid defects and their mechanisms,
  the deferred stroke-unit decision, and the measurements at close.
- [`TODO-27-10-2026.md`](TODO-27-10-2026.md) — the closed 2026-10-27 backlog and the fifth
  to eighth rounds' state.
- [`TODO-26-10-2026.md`](TODO-26-10-2026.md) — the compact closed 2026-10-25/27 record;
  the full round detail (interaction, Draft, SimpleCAD, Mesh and kernel state) is in
  [`archive/TODO-26-detail-2026-10-26.md`](archive/TODO-26-detail-2026-10-26.md).
- [`archive/milestones-detail-2026-10.md`](archive/milestones-detail-2026-10.md) — every
  M0–M4 slice, the restructure, the MVP and app-shell slices, the reactor decisions, and
  the M3 plan, with their measurements.
- [`tasks/README.md`](tasks/README.md) — the closed task specs, indexed by label and
  closing backlog; the new November entries are `backend-parity`, `wasm-shell`,
  `transform-session` and `tool-loadout`.
- [`../ROADMAP.md`](../ROADMAP.md) — where the project is going next; [`../labels.md`](../labels.md)
  — what the letters mean.
