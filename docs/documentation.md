# FerroCAD — documentation approach

Status: decision (2026-10-02). Companion to [`mvp-path.md`](direction/mvp-path.md) and
[`milestones.md`](record/milestones.md).

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
| **How-to** (tasks) | [`user/`](user/README.md) — generated from executable workflows in `tools/workflows/`; `ferrocad_core` crate page `# How-to guides`; README build/run/test/packaging sections |
| **Explanation** (understanding) | `ferrocad_core` crate page `# Explanation`; [`direction/rewrite-strategy.md`](direction/rewrite-strategy.md), [`direction/mvp-path.md`](direction/mvp-path.md), [`interaction/python-ui-research.md`](interaction/python-ui-research.md) |
| **Reference** (information) | [`python-api/`](python-api/README.md) — generated from the live modules; rustdoc item/module pages; the behaviour of the `FreeCAD` surface = upstream FreeCAD's reference |

## Slice documentation

The per-slice record lives in [`milestones.md`](record/milestones.md) (M0–M4 slices 1–16, the
FerroCAD restructure, and MVP slices A1/A2/B1). The `ferrocad_core` crate page carries
a short **Implemented capabilities** summary so the same history is visible on docs.rs.

## Repository layout

The `docs/` tree is indexed by [`README.md`](README.md) and organized by **subject**,
not by document kind: each note already declares its kind (`decision`, `reference`,
`plan`, `record`) on its own status line, so a second kind-based axis would only
duplicate it. The folders, and the rule for filing a new note:

| Folder | Subject |
| --- | --- |
| `direction/` | why we are doing this, and what the product is |
| `architecture/` | crate graph, seams, data model, packaging |
| `viewport/` | the scene/render/present stack |
| `interaction/` | spatial reference, selection, gizmos, navigation, visual language |
| `workbenches/` | the Python module contract and upstream bring-up |
| `user/` | generated workflow guides (`tools/userdocs.py`) |
| `python-api/` | generated signature reference (`tools/api_docs.py`) |
| `record/` | closed history: milestones, closed TODOs, incidents |
| `tasks/` | actionable specs with a checklist |

Three kinds of file stay at the root: [`README.md`](README.md) (the index),
[`architecture.md`](architecture.md) (the cross-cutting crate map that every section
links back to), and the active `TODO-<date>.md` backlog.

Conventions this imposes on a new note:

- **One file, one subject.** If a note covers two, split it rather than filing it in
  two folders.
- **Generated, not written.** A `user/` page is rendered from a workflow script that
  runs against the real facade; a `python-api/` page is read from the live module.
  Both generators have a `--check` mode, and CI runs it, so a page cannot describe
  behaviour the code no longer has. Never hand-edit either folder.
- **Relative links only.** Cross-folder links are `../<folder>/<name>.md`; there is no
  link macro and no site generator to resolve absolutes.
- **Status line first.** `Status: <kind> (<date>)` plus its companion notes, so a
  reader knows whether the note is a decision, a plan, or history before reading it.
- **Closed work moves to `record/`.** A TODO becomes historical when a successor
  carries only the open items; it is never deleted, because it holds the measurements.

## Verification hooks

- `cargo doc --no-deps -p ferrocad_core -p ferrocad_py` — builds without warnings.
- `cargo test --doc -p ferrocad_core` — the tutorial is a real, compiled doc test.
- `python3 tools/userdocs.py --check` — every user page matches a fresh run of its
  workflow.
- `python3 tools/api_docs.py --check` — every API page matches the live modules.
- A relative-link sweep over `docs/**/*.md` — cheap to run, and it is what catches a
  note that moved without its inbound links.

Both are cheap enough to add to CI when it is next touched.
