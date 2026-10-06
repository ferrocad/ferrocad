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
- [geometry-and-topology.md](geometry-and-topology.md) — topological sorting vs
  topological naming, and the seam for a future OCCT geometry engine.
- [occt-history-spike.md](occt-history-spike.md) — the first geometry spike: what
  OCCT history FreeCAD depends on, and what it implies for the trait and the B slices.
- [occt-integration.md](occt-integration.md) — the crate layout for OCCT: the
  `ferrocad_geom` seam, the `ferrocad_occt` backend, and where Part/its Python
  bindings sit.
- [occt-bundling.md](occt-bundling.md) — shipping OCCT the way we ship CPython
  (closure, payload layout, launcher, licensing).
- [python-bindings.md](python-bindings.md) — the PyO3 binding design.
- [property-types.md](property-types.md) — how FreeCAD exposes property types (C++
  `Base::Type`, addressed by name) and what that demands of the document-object SPI.
- [property-value-extension.md](property-value-extension.md) — how a module holds its own
  property value (`Property::Extension`) with `clone`/`eq`/`save`/`restore`.
- [distribution.md](distribution.md) — how the app runs and is packaged (payload,
  bundled runtime, artifacts).
- [releasing.md](releasing.md) — how a release is cut (crates.io, the tag, the
  artifacts).
- [repackaging.md](repackaging.md) — editions and the repackaging vision.
- [input-components.md](input-components.md) — the input widget design.
- [python-ui-research.md](python-ui-research.md) — Python-declarative UI research.
