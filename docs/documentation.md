# FerroCAD — documentation approach

Status: decision (2026-10-02; layout updated 2026-10-27). Companion to
[`mvp-path.md`](direction/mvp-path.md), [`milestones.md`](record/milestones.md) and the
[label index](labels.md).

## Decision

Use **rustdoc** (Diátaxis-structured) as the primary, verifiable documentation, and
treat the Python surface as documented by convention + upstream.

### Audiences

Four audiences, four entry points. A reader should never have to guess which folder
is theirs; a writer should never have to duplicate a fact across them.

| # | Audience | What they need | Entry point | Tooling |
| --- | --- | --- | --- | --- |
| 1 | **Users of FerroCAD** | do a task in the app or a script | [`user/`](user/README.md), in Diátaxis quadrants | generated from executable workflows + mdBook |
| 2 | **Users of the API** — mod writers, script writers, AI agents, MCP servers | the contract, the exact surface, and the JSON boundary | [`mods/`](mods/README.md) and [`python-api/`](python-api/index.md) | authored contract guide + generated reference |
| 3 | **Developers of FerroCAD** | why it is shaped this way, and what to build next | `direction/`, `architecture/`, `viewport/`, `interaction/`, `workbenches/`, `record/`, `tasks/`; rustdoc | authored notes + rustdoc |
| 4 | **Developers of upstream FreeCAD and of mods used by FerroCAD** | compatibility, divergence, and how to run upstream code here | [`workbenches/`](workbenches/conformance.md) (conformance, Qt strategy, Part/Draft layering) and `architecture/` | authored notes + the executable conformance harness |

Audience chooses the entry point; the folder chooses the subject. The same fact may
appear for two audiences only if it is *generated* for both — for example the API
surface is generated from the modules, so `mods/` links to it rather than restating
it.

The published artifact is `tools/build_book.py`, which assembles audiences 1 and 2
into one mdBook (tutorials, how-to, reference, explanation, mod writers). It is served
at <https://ferrocad.github.io/docs/> from the separate
[`ferrocad/docs`](https://github.com/ferrocad/docs) repository. That repository holds
only the Pages workflow: it checks out a **shallow, sparse copy** of this repository
(`docs/` and `tools/`), runs
`tools/build_book.py --build --site-url https://ferrocad.github.io/docs/`, and deploys
the result. Assembly drops the `user/` level, so the published paths are
`/tutorials/…`, `/how-to/…` and `/explanation/…`; `--site-url` sets the `<base>` of
mdBook's `404.html` so a mistyped URL still loads its assets from `/docs/`. The
content, the generators and the freshness checks stay here; `ferrocad/docs` only
renders and deploys, so there is no second copy to fall out of date. The internal notes
for audiences 3 and 4 stay in the repository.

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
| **Reference** (information) | [`python-api/`](python-api/index.md) — generated from the live modules; rustdoc item/module pages; the behaviour of the `FreeCAD` surface = upstream FreeCAD's reference |
| **Explanation** (understanding) | [`user/explanation/`](user/explanation/how-the-shell-talks-to-the-document.md) — hand-written, links into the design notes; `ferrocad_core` crate page `# Explanation`; [`direction/rewrite-strategy.md`](direction/rewrite-strategy.md), [`direction/mvp-path.md`](direction/mvp-path.md), [`interaction/python-ui-research.md`](interaction/python-ui-research.md) |

`docs/user/` is the Diátaxis slice of the docs: three of the four quadrants live
there (reference is the sibling `docs/python-api/`). The four together are what
`tools/build_book.py` assembles into the mdBook edition, in Diátaxis order.

## Slice documentation

The per-slice record lives in [`milestones.md`](record/milestones.md) (M0–M4 slices 1–16, the
FerroCAD restructure, MVP slices A1/A2/B1, and the 2026-10-23/24 Part, lineage and Qt
round). The 2026-10-24 Q4/Q5 round has its own closed record:
[`record/2026-10-24-q4-q5-draft-tools.md`](record/2026-10-24-q4-q5-draft-tools.md); the
2026-10-25/27 rounds are in [`record/TODO-26-10-2026.md`](record/TODO-26-10-2026.md),
with the full detail in
[`record/archive/TODO-26-detail-2026-10-26.md`](record/archive/TODO-26-detail-2026-10-26.md).
Closed task specs are in [`record/tasks/`](record/tasks/README.md). The `ferrocad_core`
crate page carries a short **Implemented capabilities** summary so the same history is
visible on docs.rs.

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
| `record/` | closed history: compact milestones and backlogs, closed task specs, incidents, and `archive/` for the full detail |
| `ROADMAP.md` (root) | the three horizons and their exit criteria |
| `labels.md` (root) | the index for every slice/work-item label and the area-prefix rule |
| `tasks/` | active task specs (currently none; closed specs are in `record/tasks/`) |

Six files stay at the root: [`README.md`](README.md) (the index),
[`architecture.md`](architecture.md) (the cross-cutting crate map that every section
links back to), [`documentation.md`](documentation.md) (this decision),
[`ROADMAP.md`](ROADMAP.md) (the high-level horizons), [`labels.md`](labels.md) (the label
index), and the active `TODO-<date>.md` backlog.

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
- **Labels are indexed and prefixed.** A slice or work-item label is registered in
  [`labels.md`](labels.md) before it is used, and a new label uses an area prefix
  (`PART-`, `DRAFT-`, `SIMPLE-`, …) rather than a single letter. Qualify an old label
  with its owner in prose (`SimpleCAD S4`, `stage T4`).
- **Closed work moves to `record/`.** A TODO becomes historical when a successor
  carries only the open items; it is never deleted, because it holds the measurements.
  A task spec moves to `record/tasks/` when its last checklist item lands, and is added
  to that folder's index; only its open tail stays in the backlog.
- **The record stays compact.** `record/` keeps the timeline, the current assessment,
  one-page closed backlogs and one file per closed task spec. The slice-by-slice detail
  moves to `record/archive/` and is linked, not restated. `ROADMAP.md` is the only
  forward-looking summary; the backlog is where the work is tracked, and `labels.md` is
  where the letters are defined.

## Verification hooks

- `cargo doc --no-deps -p ferrocad_core -p ferrocad_py` — builds without warnings.
- `cargo test --doc -p ferrocad_core` — the tutorial is a real, compiled doc test.
- `python3 tools/userdocs.py --check` — every user page matches a fresh run of its
  workflow.
- `python3 tools/api_docs.py --check` — every API page matches the live modules.
- `python3 tools/part_api_audit.py --check` — the Part/OCCT surface, the registered object
  types and the honest gap markers still match the pinned contract (self-skips without
  the upstream checkout).
- `python3 tools/build_book.py --check` — the mdBook tree assembles, every page is
  in `SUMMARY.md`, and every link resolves (external ones are rewritten to GitHub).
- A relative-link sweep over `docs/**/*.md` — cheap to run, and it is what catches a
  note that moved without its inbound links.

All of these run in CI; the docs checks are in the `headless` job.

### Reproducibility of the generated pages

The docs checks and the published build use **CPython 3.12** (pinned with
`actions/setup-python` in `ci.yml` here and in the docs repository's Pages workflow). The interpreter must be fixed
because the API reference is read from the live modules: an interpreter upgrade can
change what `inspect` reports for *inherited* builtins (`int.bit_count`,
`Exception.add_note`, …). `tools/api_docs.py` therefore documents only the members a
class **declares** — it walks the MRO, keeps every project base, and stops at
`builtins`/`enum` — so an `IntEnum` page lists its own values rather than all of
`int`'s surface. With that rule the output is byte-identical on 3.12 and 3.14 (the
drift that failed CI on 2026-10-25).

The docs repository's Pages workflow deliberately runs no generator that imports
Python modules: `build_book.py` only assembles the committed `docs/user/`,
`docs/python-api/` and `docs/mods/` pages, so publishing cannot invent content. It
builds from a shallow copy of `main`, and the freshness guarantee comes from this
repository's `headless` job, which checks the generated pages on every push.

The generator also pins the **modules**, not just the interpreter. It makes
`ferrocad_combined`, `Part`, `Mesh` and `MeshPart` unimportable while it runs, so a
developer checkout with `ferrocad_combined.abi3.so` in `python/` produces the same
pages as CI's base-only job. Importing the workbenches would add members that only
exist after `Part` loads (for example `SketchObject.addGeometry` on
`ferrocad.DocumentObject`) and would swap Part's `CADKernelError` docstring; the
workbench surfaces are measured by `tools/part_api_audit.py` instead.
