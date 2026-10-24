# FerroCAD — documentation approach

Status: decision (2026-10-02). Companion to [`mvp-path.md`](direction/mvp-path.md) and
[`milestones.md`](record/milestones.md).

## Decision

Use **rustdoc** (Diátaxis-structured) as the primary, verifiable documentation, and
treat the Python surface as documented by convention + upstream.

### Audiences

Four audiences, four entry points. A reader should never have to guess which folder
is theirs; a writer should never have to duplicate a fact across them.

| # | Audience | What they need | Entry point | Tooling |
| --- | --- | --- | --- | --- |
| 1 | **Users of FerroCAD** | do a task in the app or a script | [`user/`](user/README.md), in Diátaxis quadrants | generated from executable workflows + mdBook |
| 2 | **Users of the API** — mod writers, script writers, AI agents, MCP servers | the contract, the exact surface, and the JSON boundary | [`mods/`](mods/README.md) and [`python-api/`](python-api/README.md) | authored contract guide + generated reference |
| 3 | **Developers of FerroCAD** | why it is shaped this way, and what to build next | `direction/`, `architecture/`, `viewport/`, `interaction/`, `workbenches/`, `record/`, `tasks/`; rustdoc | authored notes + rustdoc |
| 4 | **Developers of upstream FreeCAD and of mods used by FerroCAD** | compatibility, divergence, and how to run upstream code here | [`workbenches/`](workbenches/conformance.md) (conformance, Qt strategy, Part/Draft layering) and `architecture/` | authored notes + the executable conformance harness |

Audience chooses the entry point; the folder chooses the subject. The same fact may
appear for two audiences only if it is *generated* for both — for example the API
surface is generated from the modules, so `mods/` links to it rather than restating
it.

The published artifact is `tools/build_book.py`, which assembles audiences 1 and 2
into one mdBook (tutorials, how-to, reference, explanation, mod writers). The
internal notes for audiences 3 and 4 stay in the repository.

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
| **Tutorial** (learning) | [`user/tutorials/`](user/tutorials/first-model.md) — generated from an executable workflow; `ferrocad_core` crate page (`crates/ferrocad_core/src/lib.rs`, `# Tutorial`, a runnable doc test); `README.md` quickstart |
| **How-to** (tasks) | [`user/how-to/`](user/how-to/documents-and-properties.md) — generated from executable workflows in `tools/workflows/`; `ferrocad_core` crate page `# How-to guides`; README build/run/test/packaging sections |
| **Reference** (information) | [`python-api/`](python-api/README.md) — generated from the live modules; rustdoc item/module pages; the behaviour of the `FreeCAD` surface = upstream FreeCAD's reference |
| **Explanation** (understanding) | [`user/explanation/`](user/explanation/how-the-shell-talks-to-the-document.md) — hand-written, links into the design notes; `ferrocad_core` crate page `# Explanation`; [`direction/rewrite-strategy.md`](direction/rewrite-strategy.md), [`direction/mvp-path.md`](direction/mvp-path.md), [`interaction/python-ui-research.md`](interaction/python-ui-research.md) |

`docs/user/` is the Diátaxis slice of the docs: three of the four quadrants live
there (reference is the sibling `docs/python-api/`). The four together are what
`tools/build_book.py` assembles into the mdBook edition, in Diátaxis order.

## Slice documentation

The per-slice record lives in [`milestones.md`](record/milestones.md) (M0–M4 slices 1–16, the
FerroCAD restructure, MVP slices A1/A2/B1, and §5 for the 2026-10-23/24 Part, lineage
and Qt round). The `ferrocad_core` crate page carries
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
| `mods/` | the external contract for mod writers and API clients |
| `user/` | generated workflow guides, in Diátaxis sections (`tools/userdocs.py`) |
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
  behaviour the code no longer has. Never hand-edit either folder. The one
exception is `user/explanation/`, which is authored: it has no runnable subject.
- **Quadrants are declared, not inferred.** A workflow sets `QUADRANT`, and the
  runner writes the page to `user/<quadrant>/` and groups the index from it. A page
  that moves between quadrants is moved in one place.
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
- `python3 tools/build_book.py --check` — the mdBook tree assembles, every page is
  in `SUMMARY.md`, and every link resolves (external ones are rewritten to GitHub).
- A relative-link sweep over `docs/**/*.md` — cheap to run, and it is what catches a
  note that moved without its inbound links.

All of these run in CI; the docs checks are in the `headless` job.
