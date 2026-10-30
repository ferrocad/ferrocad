# FerroCAD docs

The repository's documentation. The product surface is the `FreeCAD` Python API;
these notes explain how to use it, how to extend it, how it is built, and how the work
is planned.

## The six folders

The tree is split by **audience and stability**. The first four folders are the
stable public surface (and what the mdBook publishes); `developer/` is stable
engineering documentation; `project/` is the unstable record of the work.

| Folder | Surface / audience | Stability | Start here |
| --- | --- | --- | --- |
| [`general/`](general/README.md) | general readers: the concepts behind 3D CAD and FerroCAD, useful to users and developers | stable, published | [general/README.md](general/README.md) |
| [`user/`](user/README.md) | UI users, and the generated API workflows for console, macro and script users | stable, published | [user/README.md](user/README.md) |
| [`mods/`](mods/README.md) | addon authors: mods, workbenches, scripts, automated clients | stable, published | [mods/README.md](mods/README.md) |
| [`python-api/`](python-api/index.md) | API users: the exact signature reference | stable, published | [python-api/index.md](python-api/index.md) |
| [`developer/`](developer/README.md) | people changing FerroCAD | stable, repository only | [developer/README.md](developer/README.md) |
| [`project/`](project/README.md) | maintainers and contributors | unstable, repository only | [project/README.md](project/README.md) |

The complete file-by-file map is [INDEX.md](INDEX.md): every note appears once, under
its primary folder, with a one-line description. This page is the short tour.

## UI users and API users

The public surface has two front doors, and the distinction is deliberate:

- **UI users** work in the app window. The window is the guide; the generated pages
  are not click-throughs. The [repository README](../README.md) shows the window.
- **API users** — the Python console, macros and scripts — use the same document model
  through `FreeCAD`/`FreeCADGui`. Start with
  [UI users and API users](user/explanation/ui-and-api.md), then the generated
  [API workflows](user/README.md) and the [reference](python-api/index.md).
- **Addon authors** package the API for other people: [mods/README.md](mods/README.md).

If you want a task done once, use the window; if you want to repeat it, automate it or
build on it, use the API. A macro is not a second-class path — the console, a macro and
a workbench command call the same functions.

## Start here

- **New to 3D CAD?** — [A gentle introduction to 3D CAD architecture](general/README.md):
  NURBS/B-Rep vs meshes, the tessellation and GPU pipeline, topological naming, and
  how FerroCAD's crates map onto them.
- **Using the app (UI)** — [user/README.md](user/README.md) and the
  [repository README](../README.md) for the window.
- **Using the API (console, macros, scripts)** —
  [UI users and API users](user/explanation/ui-and-api.md), the generated
  [API workflows](user/README.md), and the [Python API reference](python-api/index.md).
- **Writing a mod or a workbench** — [mods/README.md](mods/README.md): the contract,
  the five contributions, and the JSON boundary; the signatures are in
  [python-api/index.md](python-api/index.md).
- **Changing FerroCAD** — [developer/README.md](developer/README.md): start with
  [developer/architecture.md](developer/architecture.md) for the crate graph.
- **Planning the next round** — [project/ROADMAP.md](project/ROADMAP.md) for the
  horizons and [project/TODO-31-10-2026.md](project/TODO-31-10-2026.md) for the active
  backlog.
- **Understanding a decision** — [project/documentation.md](project/documentation.md)
  for the documentation policy, [project/labels.md](project/labels.md) for the labels,
  and [project/direction/rewrite-strategy.md](project/direction/rewrite-strategy.md)
  for the argument behind the rewrite.

## Conventions

- **Generated, not written.** `user/` pages come from `tools/workflows/` via
  `tools/userdocs.py`; `python-api/` pages come from the live modules via
  `tools/api_docs.py`. Both have a `--check` mode in CI. Edit the source, never the
  page. The one authored exception is [`user/explanation/`](user/explanation/).
- **The index is exhaustive.** [`INDEX.md`](INDEX.md) lists every Markdown note once;
  `tools/test_docs_tooling.py` fails when a note is missing or duplicated.
- **Relative links only.** Cross-folder links are `../<folder>/<name>.md`.
  `tools/build_book.py --check` and the relative-link sweep catch a move that left an
  inbound link behind.
- **Stable here, unstable there.** Research notes, open plans, milestones and task
  specs live in `project/`; when a result becomes a stable design, the design note goes
  in `developer/` and the research or plan is linked, not duplicated.

The repository-root [`README.md`](../README.md) covers install, build, test and the
workspace layout. The mdBook edition at <https://ferrocad.github.io/docs/> is assembled
from `user/`, `mods/` and `python-api/` by `tools/build_book.py`.
