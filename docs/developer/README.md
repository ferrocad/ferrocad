# Developer documentation

Stable engineering notes for people changing FerroCAD. These pages are repository-only;
the published mdBook is the user and addon surface.

Start with [architecture.md](architecture.md), the crate map and call paths every
section links back to. If you are new to 3D CAD, read
[A gentle introduction to 3D CAD architecture](../general/cad-architecture.md) first;
its last chapter maps NURBS/B-Rep, tessellation and naming onto the crates described
here. The research notes that shaped these designs live in
[`../project/research/`](../project/research/README.md); once a result is stable, the
decision is written here and the research is linked.

## The sections

- [architecture.md](architecture.md) — the crate graph, the call paths, the dependency
  rules and the C-ABI rationale.
- [architecture/](architecture/) — the seams and data model: namespaces, bindings,
  properties, lineage, Part/Draft layering, packaging and releases.
- [viewport/](viewport/) — the scene/render/present stack.
- [interaction/](interaction/) — spatial reference, selection, gizmos, navigation and
  visual language.
- [workbenches/](workbenches/) — the workbench command model, upstream bring-up,
  conformance and the screenshot tooling.
- [testing.md](testing.md) — the two test images, the edition dependency-closure
  contract and the CI order.

## How to use it

- **New to the code?** Read [architecture.md](architecture.md), then the section you
  are changing. The crate-level rustdoc is the reference; the notes here are the
  reasoning and the invariants.
- **Changing a boundary?** The note that owns it states the rule. If you change the
  rule, update the note and the code in the same change.
- **Looking for a specific file?** [`../INDEX.md`](../INDEX.md) lists every note once,
  with a one-line description.
- **Looking for the plan, a milestone or a closed task?** Those are in
  [`../project/`](../project/README.md); this folder is the stable state, not the
  history.

The complete folder list is in [`../README.md`](../README.md).
