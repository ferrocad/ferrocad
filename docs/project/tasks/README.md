# Active task specs

Status: index (2026-10-30). Seven active specs and one recorded decision:

- [transform-session.md](transform-session.md) — **the transform overhaul, planned not built**:
  explicit uncommitted state (`PendingEdit` + `EditView` in `ferrocad_core`), one cumulative
  `TransformSession`, loadout slots with a topology filter, hover-to-arm, the in-place pill
  and the 3-D measuring tape. Stages **A0–A3** (invisible) then **U1–U6** (the UX), each with
  an acceptance and the spec sections it rewrites.

- [simplecad-bringup.md](simplecad-bringup.md) — the **SC0–SC8** cut of the SimpleCAD
  bring-up: triage the 64 failures, exact `ShapeType`, `makeWireString` on `skrifa`/
  `harfrust`, the GUI classification, the S4.4–S4.6 view half, and the workflow acceptance.
- [text-and-3d-text.md](text-and-3d-text.md) — what is kernel-free about text, why the toy
  can only preview it, why a wire extrudes to a shell but a face to a solid, and why there
  is no DPI or point size in the geometry path. The demo is `tests/test_text_workflow.py`.
- [ux-research.md](ux-research.md) — the interaction questions **U1–U7** (draw path, active
  action, snap feedback, plane affordances, wheel verbs, numeric entry, walkthroughs), each
  with a method and an acceptance, and a decision log to fill.
- [creation-tools-ownership.md](creation-tools-ownership.md) — should a polyline be a core
  default that mods can override, or an edition/mod choice? A decision task (**X1–X4**).
- [polyline-vertex-editing-and-active-tool-hud.md](polyline-vertex-editing-and-active-tool-hud.md)
  — edit a placed vertex while the tool is still open, and replace the split
  tool-strip/floating-form presentation with one dielectric active-action surface.
- [workplanes.md](workplanes.md) — make the world plane a real state, bind a face
  ephemerally during a creation tool, and replace the `WP` plumbing menu with plane
  choices.
- [stroke-units.md](stroke-units.md) — **decided, not open work**: viewport strokes are
  device pixels (gpui interop); a logical-pixel/UI-scale option was considered and
  deferred, with the reasons and the reopening plan.

Every other spec written so far has landed and moved to
[`../record/tasks/`](../record/tasks/README.md); the open
work is tracked in [`../TODO-31-10-2026.md`](../TODO-31-10-2026.md) and the standing
design notes under `architecture/`, `interaction/`, `viewport/` and `workbenches/`.

A new spec belongs here when work is large enough to need its own design, checklist and
acceptance, and is still in progress. When the last checklist item lands, move the file
to `record/tasks/`, add it to that index, and leave only its open tail in the backlog.
The label convention is in [`../labels.md`](../labels.md).
