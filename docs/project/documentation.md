# FerroCAD — documentation approach

Status: decision (2026-10-02; layout updated 2026-10-27). Companion to
[`mvp-path.md`](direction/mvp-path.md), [`milestones.md`](record/milestones.md) and the
[label index](labels.md).

## Decision

Use **rustdoc** (Diátaxis-structured) as the primary, verifiable documentation, and
treat the Python surface as documented by convention + upstream.

### Audiences

Six audiences, six folders. General readers start with `general/`; UI users and API
users share `user/` — the window is one entry point and the generated pages are the
API workflows. The exact API surface is `python-api/`, and the addon contract is
`mods/`. A reader should never have to guess which folder is theirs; a writer should
never have to duplicate a fact across them.

| # | Audience | What they need | Entry point | Tooling |
| --- | --- | --- | --- | --- |
| 1 | **General readers** | the concepts behind 3D CAD and FerroCAD, shared by users and developers | [`general/README.md`](../general/README.md) | authored explanations + mdBook |
| 2 | **UI users** | do a task in the application window | [`user/README.md`](../user/README.md) and the app | screenshots + the app |
| 3 | **API users** — Python console, macros, scripts | drive the same model from Python | [`user/explanation/ui-and-api.md`](../user/explanation/ui-and-api.md), [`user/`](../user/README.md) (generated API workflows) and [`python-api/`](../python-api/index.md) | generated from executable workflows + generated reference |
| 4 | **Addon authors** — mod and workbench writers, AI agents, MCP servers | the contract and the JSON boundary | [`mods/`](../mods/README.md) and [`python-api/`](../python-api/index.md) | authored contract guide + generated reference |
| 5 | **Developers of FerroCAD** — including upstream-compatibility work | why it is shaped this way, and what to build next | [`developer/`](../developer/README.md): [`architecture.md`](../developer/architecture.md), `architecture/`, `viewport/`, `interaction/`, `workbenches/`; rustdoc | authored notes + the executable conformance harness |
| 6 | **Process and research** — maintainers and contributors | the plan, the measurements, and the argument behind a decision | [`ROADMAP.md`](ROADMAP.md), [`TODO-29-10-2026.md`](TODO-29-10-2026.md), `direction/`, `record/`, `tasks/`, [`labels.md`](labels.md) | authored notes and records |

The complete file-by-file map is [`../INDEX.md`](../INDEX.md): every note appears once,
under its primary audience, with a one-line description. This table is the policy
(which folder is whose entry point); the index is the exhaustive list. The general,
user, API and addon material is what `tools/build_book.py` publishes; the developer
and project notes stay in the repository.

Audience chooses the entry point; the folder chooses the subject. A reader should
never have to guess which folder is theirs, and a writer should never have to
duplicate a fact across them. The same fact may appear for two audiences only if it
is *generated* for both — for example the API surface is generated from the
modules, so `mods/` links to it rather than restating it.

The published artifact is `tools/build_book.py`, which assembles the general, user, API
and addon material into one mdBook (concepts, tutorials, how-to, reference,
explanation, mod writers). It is served
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
for audiences 5 and 6 stay in the repository.

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
| **Tutorial** (learning) | [`user/tutorials/`](../user/tutorials/first-model.md) — generated from an executable workflow; `ferrocad_core` crate page (`crates/ferrocad_core/src/lib.rs`, `# Tutorial`, a runnable doc test); `README.md` quickstart |
| **How-to** (tasks) | [`user/how-to/`](../user/how-to/documents-and-properties.md) — generated from executable workflows in `tools/workflows/`; `ferrocad_core` crate page `# How-to guides`; README build/run/test/packaging sections |
| **Reference** (information) | [`python-api/`](../python-api/index.md) — generated from the live modules; rustdoc item/module pages; the behaviour of the `FreeCAD` surface = upstream FreeCAD's reference |
| **Explanation** (understanding) | [`user/explanation/`](../user/explanation/how-the-shell-talks-to-the-document.md) — hand-written, links into the design notes; `ferrocad_core` crate page `# Explanation`; [`direction/rewrite-strategy.md`](direction/rewrite-strategy.md), [`direction/mvp-path.md`](direction/mvp-path.md), [`research/python-ui-research.md`](research/python-ui-research.md) |

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

`docs/` has five top-level folders. The split is by **audience and stability**, not
by document kind: each note still declares its kind (`decision`, `reference`,
`plan`, `record`) on its status line.

| Folder | Audience | Stability | Holds |
| --- | --- | --- | --- |
| `general/` | general readers | stable, published | concepts shared by users and developers |
| `user/` | UI users, and the API workflows of console, macro and script users | stable, published | generated tutorial, how-to and explanation pages |
| `mods/` | addon authors | stable, published | the external contract for mod and script writers |
| `python-api/` | API users and addon authors | stable, published | generated signature reference |
| `developer/` | FerroCAD developers | stable, repository only | architecture, viewport, interaction, workbenches and tooling |
| `project/` | maintainers and contributors | unstable, repository only | direction, research, roadmap, backlogs, milestones, task specs and this decision |

The root has two files: [`../README.md`](../README.md) (the subject-based tour) and
[`../INDEX.md`](../INDEX.md) (the complete audience-grouped index). `developer/`
keeps the stable design notes; `project/` keeps the material that changes with the
work: `direction/`, `research/`, `record/`, `tasks/`, `ROADMAP.md`,
`TODO-<date>.md` and `labels.md`.

Conventions this imposes on a new note:

- **One file, one subject, one audience.** File the note under the folder of its
  primary audience. If it also serves another audience, link across; do not copy it.
- **Stable design goes in `developer/`; research and open plans go in `project/`.**
  A spike, assessment or research note belongs in `project/research/`; an open plan
  belongs in `project/`. Once its result is a stable design, the decision is written in
  `developer/` and the research or plan is linked, not duplicated.
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
- `python3 tools/generate_concept_diagrams.py --check` — the hand-authored concept
  SVGs are byte-identical to their generator.
- `python3 tools/generate_general_figures.py --check` — the general chapters' viewport
  PNGs match `docs/general/images/manifest.json`. When `Part` is importable it also
  rebuilds each render spec from real geometry and compares the digest; without a
  kernel it checks the manifest, the PNG bytes and the generator digest. `--capture`
  (local, needs a GPU adapter) regenerates the PNGs.
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

The general chapters have two image generators. The SVG diagrams are pure geometry
(`tools/generate_concept_diagrams.py`, no kernel, no GPU). The viewport figures are
rendered by `ferrocad_snapshot` from specs built out of `Part`/`Mesh`
(`tools/generate_general_figures.py`); the check is GPU- and kernel-free because the
manifest pins the spec and PNG digests, so the `headless` job can run it too.

The generator also pins the **modules**, not just the interpreter. It makes
`ferrocad_combined`, `Part`, `Mesh` and `MeshPart` unimportable while it runs, so a
developer checkout with `ferrocad_combined.abi3.so` in `python/` produces the same
pages as CI's base-only job. Importing the workbenches would add members that only
exist after `Part` loads (for example `SketchObject.addGeometry` on
`ferrocad.DocumentObject`) and would swap Part's `CADKernelError` docstring; the
workbench surfaces are measured by `tools/part_api_audit.py` instead.
