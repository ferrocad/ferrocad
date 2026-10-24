# Round log — 2026-10-24: Q4/Q5 native Draft tools

Status: record. Closes the **Q4/Q5 items** of P3 in
[`../TODO-24-10-2026.md`](../TODO-24-10-2026.md); the plan is
[`../tasks/qt-reduction.md`](../tasks/qt-reduction.md) Q4/Q5 and the surface reference is
[`../workbenches/draft-tools.md`](../workbenches/draft-tools.md).

## Goal

Q3 had landed the rendered form host, so the remaining Qt-reduction work was the
**Draft tool layer**: a native spatial reference (`Snapper`, the task-panel adapter and
`getPoint`), the line/wire commands, `PlaneGui`, the checkable snap bar, and the four
Draft forms that had no headless driver. The acceptance was behavioural: known-coordinate
snaps on a rotated workplane, two clicks create a line, Escape leaves the document
untouched, and every form accepts and rejects headlessly.

## Delivered

| Slice | What landed | Tests |
| --- | --- | --- |
| **F1 — camera-free snapping** | `SnapContext::snap_point` (feature candidates within a world-space tolerance, else the workplane grid, major/minor reported); `ferrocad.stage.Workplane`, `WorkplaneKind`, `SnapContext`, `Snap`, `SnapCandidate`, `SnapKind` | `ferrocad_stage` 149 (was 142); `tests/test_stage_py.py` 17 cases |
| **F2 — control surface** | `FreeCADGui.Snapper` (six native modes, constraints, workplane/grid, async point sessions), `draftToolBar`, `HintManager`, `InputHint`/`UserInput`, `getMainWindow` / `addModule` / `doCommand`; `draft.pick`/`escape`/`finish`; six checkable `Draft_Snap_*` commands | `tests/test_draft_snapper.py` 42 tests |
| **F3 — tools** | `FerroCADGui.tools.PlaneGui`; native `Draft_Line`/`Draft_Wire` (`Creator`-shaped session, commit through `doCommand`) | `tests/test_draft_tools.py` 23 tests, also run in-image with the real Part kernel |
| **F4 — forms** | `FerroCADGui.forms`: `Draft_SetStyle`, `Draft_Hatch`, `Draft_AnnotationStyleEditor`, `Draft_Scale` over the headless `PySideUic` proxies, plus `TaskPanel_Scale.ui` | `tests/test_draft_forms.py` 12 tests, 4 subtests |

## Measurements

| Suite | Before | After |
| --- | --- | --- |
| `pytest tests` (with `ferrocad_stage` built) | 444 passed / 17 skipped / 134 subtests | **538 passed / 8 skipped / 172 subtests** |
| `pytest tests` (without the stage extension) | — | 480 passed / 66 skipped (clean skips) |
| `ferrocad_stage` | 142 | **149** (+1 doctest) |
| `ferrocad_stage_py` acceptance suite | 10 cases | **17 cases** |
| `draft_tools` with the real Part kernel | — | **23 passed** |
| Part conformance | 531 / 0 / 0 | **531 / 0 / 0** (unchanged) |
| Draft M2 | 27 / 0 / 0 | **27 / 0 / 0** (unchanged) |

## Deviations recorded

1. **The upstream `gui_lines` route is blocked headlessly.** `Line.Activated` writes
   `ViewObject.ShowInTree`/`Selectable`, picking is driven by
   `view.addEventCallback("SoEvent", …)`, the preview path needs Part, and the commit
   path calls `Draft.make_line`/`make_wire` while `python/Draft` is the `make_clone`
   shim. The native `Creator` reproduces the observable contract;
   `tools.upstream_report()` records the blockers and the test asserts them.
2. **SetStyle's apply/annotation buttons are not wired.** The headless `ViewObject` has
   no `LineColor`/`LineWidth`/`PointColor`/`DrawStyle` or annotation-font properties.
   Presets round-trip through `ParamGet`; applying a partial style would be misleading.
3. **Annotation styles stay in `Doc.Meta` + `ParamGet`.** They are not pushed into live
   annotation view objects, for the same `ViewObject` reason.
4. **The nine Draft-only snap modes raise.** `Extension`, `Parallel`, `Perpendicular`,
   `Intersection`, `Angle`, `Special`, `Dimensions`, `Ortho` and `Lock` need a live
   cursor/extension solver; `Snapper.supported_modes()` and `unsupported_modes()` report
   the split.
5. **Real viewport clicks are not routed yet.** The session is complete and driven by
   `draft.pick`/`escape`/`finish`; wiring gpui pointer events into those three calls is
   the remaining shell integration.
6. `Snapper.setAngle` accepts degrees (`90`) and radians (`pi/2`); values above `2π`
   are treated as degrees.

## Commits

```
1c7a7a1 feat: add camera-free snapping and workplane bindings
38008a1 feat: add the native Draft Snapper and input sessions
4360c42 feat: add the Draft line/wire tools and PlaneGui
09c4db4 feat: add the remaining Draft form drivers
1e0baa5 docs: record the Qt-reduction Q4/Q5 completion
```

## Documents

New: [`../workbenches/draft-tools.md`](../workbenches/draft-tools.md) (the surface
reference) and this log. Updated: the [`docs/README.md`](../README.md) index, the
[roadmap](../ROADMAP.md) H2 status, [`qt-strategy.md`](../workbenches/qt-strategy.md)
(status and routing table), [`qt-reduction.md`](../tasks/qt-reduction.md) (Q4/Q5 status
and deviations), [`conformance.md`](../workbenches/conformance.md) (the native tool
suites), [`actions-and-tools.md`](../workbenches/actions-and-tools.md) §7.1 (the point
session), [`milestones.md`](milestones.md), [`stage.md`](../viewport/stage.md) (the
spatial reference bindings), [`documentation.md`](../documentation.md) (the slice-documentation
timeline link), and the generated `python-api/FreeCADGui.md`.

## Next

**P4 — SimpleCAD S1.** The Q4/Q5 layer removes the last Qt dependency on the Draft tool
path; the follow-ups above (shell click routing, the `ViewObject` view properties, the
advanced snap modes) are independent and can be scheduled with the SimpleCAD bring-up
rather than blocking it.
