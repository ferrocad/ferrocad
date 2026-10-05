# FerroCAD — documentation approach

Status: decision (2026-10-02). Companion to [`mvp-path.md`](mvp-path.md) and
[`milestones.md`](milestones.md).

## Decision

Use **rustdoc** (Diátaxis-structured) as the primary, verifiable documentation, and
treat the Python surface as documented by convention + upstream.

| Audience | Where their docs live | Tooling |
| --- | --- | --- |
| Rust users/contributors (`ferrocad_core`, `ferrocad_py`) | crate + module pages | **rustdoc** (`cargo doc`, docs.rs) |
| Python workbench authors (`import FreeCAD`) | facade docstrings + this repo's `docs/` | Markdown now; **Sphinx/mkdocstrings** later |
| Upstream-compat reference | FreeCAD's own docs | link, don't duplicate |

## Why rustdoc first

- It is **built into the toolchain** and works offline — we can build and verify it
  here (`cargo doc --no-deps`, `cargo test --doc`). Sphinx/mkdocs/pdoc are **not
  installed** in the sandbox and cannot be fetched, so a Python doc build would be
  unverifiable.
- The docs.rs rendering of a crate page is a natural home for a tutorial + explanation
  (the `//!` block), which is exactly the Diátaxis "learning/understanding" material a
  dull API reference lacks.

## Why not Sphinx *yet*

- The **product surface is the `FreeCAD` namespace, which deliberately mirrors
  upstream FreeCAD.** A generated API reference would largely duplicate FreeCAD's own
  documentation; the value we add is *guidance and explanation* (what FerroCAD
  supports, what is out of scope, how to run it), not a second API catalogue.
- We cannot build Sphinx here, so adding it now would ship unverifiable config.
- When a Python doc build can run in CI, adopt **Sphinx (`autodoc`) or
  `mkdocstrings`** over `python/FreeCAD`, and keep the Rust side on rustdoc. The
  repository `docs/` stays the source for narrative/explanation.

## Diátaxis mapping (current)

| Quadrant | Location |
| --- | --- |
| **Tutorial** (learning) | `ferrocad_core` crate page (`crates/ferrocad_core/src/lib.rs`, `# Tutorial`, a runnable doc test); `README.md` quickstart |
| **How-to** (tasks) | `ferrocad_core` crate page `# How-to guides`; README build/run/test/packaging sections |
| **Explanation** (understanding) | `ferrocad_core` crate page `# Explanation`; `docs/rewrite-strategy.md`, `docs/mvp-path.md`, `docs/python-ui-research.md` |
| **Reference** (information) | rustdoc item/module pages; the Python surface = upstream FreeCAD's reference |

## Slice documentation

The per-slice record lives in [`milestones.md`](milestones.md) (M0–M4 slices 1–16, the
FerroCAD restructure, and MVP slices A1/A2/B1). The `ferrocad_core` crate page carries
a short **Implemented capabilities** summary so the same history is visible on docs.rs.

## Verification hooks

- `cargo doc --no-deps -p ferrocad_core -p ferrocad_py` — builds without warnings.
- `cargo test --doc -p ferrocad_core` — the tutorial is a real, compiled doc test.

Both are cheap enough to add to CI when it is next touched.
