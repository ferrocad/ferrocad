# FerroCAD docs

Planning notes and reference material for the FerroCAD rewrite. These are working
notes, some of them snapshots in time, not user documentation. The product surface
is the `FreeCAD` Python API, documented by convention and by upstream.

## Direction

- [rewrite-strategy.md](rewrite-strategy.md) — the authoritative direction: why a
  Rust reimplementation, the risk register, and the shape of the work.
- [coin-bridge-reuse-assessment.md](coin-bridge-reuse-assessment.md) — the evidence
  behind the Coin3D and scene-graph decisions (a snapshot).
- [documentation.md](documentation.md) — how the project is documented.

## The record

- [milestones.md](milestones.md) — the full milestone record (M0 through the current
  slices), the decisions the spike unlocked, and the M3 plan.
- [mvp-path.md](mvp-path.md) — the headless engine MVP: definition, acceptance
  script, and the slice plan (tracks A/B/C/D).
- [app-shell-vision.md](app-shell-vision.md) — the interactive window: the five
  pillars, architecture, and slices S1–S6.

## Reference

- [architecture.md](architecture.md) — crates, the call paths, and the dependency
  rules.
- [python-bindings.md](python-bindings.md) — the PyO3 binding design.
- [distribution.md](distribution.md) — how the app runs and is packaged (payload,
  bundled runtime, artifacts).
- [releasing.md](releasing.md) — how a release is cut (crates.io, the tag, the
  artifacts).
- [repackaging.md](repackaging.md) — editions and the repackaging vision.
- [input-components.md](input-components.md) — the input widget design.
- [python-ui-research.md](python-ui-research.md) — Python-declarative UI research.
