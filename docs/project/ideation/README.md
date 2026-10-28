# Ideation

Status: ideation (2026-10-28). Exploratory product ideas. Nothing here is a commitment,
a roadmap, or an accepted design.

This folder is for hypotheses about what FerroCAD could become. An idea here is allowed
to be ambitious and incomplete; it becomes real only when it is accepted into the
[backlog](../TODO-27-10-2026.md) for work, or written up as a stable design in
[developer/](../../developer/README.md).

- [Intent, lineage and safe automation](intent-and-automation.md) — the product thesis:
  make design intent, dependencies, failures and automation first-class while keeping
  the FreeCAD-compatible Python API and `.FCStd` interoperability.
- [FreeCAD pain points and the FerroCAD response](freecad-pain-points.md) — a
  consolidated review of recurring community pain points, their upstream root causes,
  and the crate or layer that could carry each response.

## The rule

- **Ideation is not truth.** Estimates, competitor claims and proposed APIs here are
  hypotheses, not measured facts.
- **Accepted ideas move out.** A stable design goes to `developer/`; sequenced work goes
  to the backlog and its results to `record/`.
- **Keep the familiar layer excellent.** None of these ideas replaces the
  FreeCAD-compatible workflow or the `.FCStd` boundary; they are additions on top of it.
