# Project documentation

The unstable, working record: why the project is shaped this way, what is planned, what
has been measured, and the specs behind the current round. These pages are
repository-only and are expected to change or be superseded.

The stable engineering state lives in [`../developer/`](../developer/README.md); the
public surface is [`../user/`](../user/README.md), [`../mods/`](../mods/README.md) and
[`../python-api/`](../python-api/index.md).

## Where to start

- [ROADMAP.md](ROADMAP.md) — the three horizons and their exit criteria.
- [TODO-30-10-2026.md](TODO-30-10-2026.md) — the active backlog.
- [labels.md](labels.md) — what every `S1`, `P4`, `D0`, `T4` and `G1` means, and which
  program owns it.
- [direction/rewrite-strategy.md](direction/rewrite-strategy.md) — the authoritative
  argument for the rewrite, the risk register and the shape of the work.
- [ideation/intent-and-automation.md](ideation/intent-and-automation.md) — the product
  thesis for making intent, lineage and automation first-class (speculative).
- [record/milestones.md](record/milestones.md) — the compact timeline and the current
  assessment.

## The sections

- [direction/](direction/) — why we are doing this: the strategy, the MVP path and the
  app-shell vision.
- [ideation/](ideation/README.md) — exploratory product ideas. Nothing here is a
  commitment; accepted ideas move to `developer/` or the backlog.
- [research/](research/) — the spikes and assessments that shaped the design: OCCT
  history, geometry/topology, Coin3D reuse, GPU interop, the upstream dma-buf issue,
  Python UI research, the grid-overlay fixture and the SimpleCAD bring-up.
- [record/](record/) — closed history: compact milestones, closed backlogs, closed task
  specs, incidents, and `archive/` for the full detail.
- [tasks/](tasks/) — active task specs (the SimpleCAD bring-up cut, the UX research plan,
  the creation-tool ownership decision, the workplane hierarchy, the polyline/HUD spec and
  the stroke-unit decision); the backlog tracks the work, and finished specs move to
  [`record/tasks/`](record/tasks/README.md).
- [documentation.md](documentation.md) — the documentation decision: audiences,
  Diátaxis, rustdoc-first and the folder rules behind this layout.

## The rule

When work closes, the open fragment moves to the successor backlog; the measurements
and the design argument move to [record/](record/) and are never deleted. When a
research note becomes a stable design, the decision is written in
[`../developer/`](../developer/README.md) and the research note stays here, linked
rather than duplicated.

[Ideation](ideation/README.md) is deliberately different: it holds hypotheses, not
measurements. An idea is promoted only when it becomes a backlog item or a stable
design; until then it is labelled speculative.

The complete folder list is in [`../README.md`](../README.md); the file-by-file map is
[`../INDEX.md`](../INDEX.md).
