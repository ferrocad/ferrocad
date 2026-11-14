# Active task specs

Status: index (2026-11-13). These are the specs that still have open work or an open
decision. Completed specs moved to [`../record/tasks/`](../record/tasks/README.md).

- [simplecad-bringup.md](simplecad-bringup.md) — **the SC0–SC8 cut of P1.** Triage the
  failures, exact `ShapeType`, `makeWireString` on `skrifa`/`harfrust`, the GUI
  classification, the S4.4–S4.6 view half, the workflow acceptance and the final
  re-bucketing. **P1 is closed:** SC0–SC8 are done, the baseline is **985/49/18/7**, and
  all 49 failures are attributed (37 P2 / 11 P3 / 1 documented test assumption).

- [opening-a-large-model.md](opening-a-large-model.md) — **the open/sync budget.** Stages 1
  and 2 landed (the `.FCStd` payloads restore in parallel, `open` 5.9 s → 1.8 s, the 900a
  assembly 13.5 s end to end); what is left is `BRepMesh`, why a coarser first mesh only
  recovers a third, and the order to fix it. Stages 1–5.

- [operation-failure-modelling.md](operation-failure-modelling.md) — **typed, recoverable
  failures.** `OperationFailure::{InvalidGeometry, Kernel}` at the geometry seam, the
  `FilletRef`/`Recovery` affordances, the `SESSION_HOLD(failure)` payload and the
  never-report rule for domain errors. **F1 landed 2026-11-01** (`FailureKind::classify`);
  F2–F5 open.

- [mod-events.md](mod-events.md) — **the mod boundary.** A fixed low-frequency event set
  the host advertises and explicit `FerroCAD.subscribe`. The two document events are
  emitted; `selection.changed` and `transform.committed` are unbuilt, and a mod-to-mod bus
  (capability graph, ordering, teardown) is deferred with a named trigger. Stage **E1**.

- [ux-research.md](ux-research.md) — the interaction questions **U1–U7** (draw path, active
  action, snap feedback, plane affordances, wheel verbs, numeric entry, walkthroughs), each
  with a method and an acceptance, and a decision log to fill.

- [creation-tools-ownership.md](creation-tools-ownership.md) — should a polyline be a core
  default that mods can override, or an edition/mod choice? A decision task (**X1–X4**).

- [polyline-vertex-editing-and-active-tool-hud.md](polyline-vertex-editing-and-active-tool-hud.md)
  — edit a placed vertex while the tool is still open, and one dielectric active-action
  surface. Built on 2026-10-29; the numeric-entry tail is carried in P4.

- [workplanes.md](workplanes.md) — the plane state model and the visual hierarchy's four
  slices. Built; the remaining gaps are named in §8 and carried in P4 SK4.

- [text-and-3d-text.md](text-and-3d-text.md) — what is kernel-free about text, why a wire
  extrudes to a shell but a face to a solid, and why there is no DPI or point size in the
  geometry path. A recorded design, not open work.

- [stroke-units.md](stroke-units.md) — **decided, not open work.** Viewport strokes are
  device pixels (gpui interop); a logical-pixel/UI-scale option was considered and deferred,
  with the reasons and the reopening plan.

Completed specs — `backend-parity`, `host-backends`, `wasm-shell`, `transform-session`,
`tool-loadout` and the earlier work — are indexed in
[`../record/tasks/`](../record/tasks/README.md). The open work is tracked in
[`../TODO-14-11-2026.md`](../TODO-14-11-2026.md) and the standing design notes under
`architecture/`, `interaction/`, `viewport/` and `workbenches/`.

A new spec belongs here when work is large enough to need its own design, checklist and
acceptance, and is still in progress. When the last checklist item lands, move the file
to `record/tasks/`, add it to that index, and leave only its open tail in the backlog.
The label convention is in [`../labels.md`](../labels.md).
