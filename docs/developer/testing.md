# Testing and the edition contracts

Status: note (2026-10-28).

Two boundaries are easy to cross by accident and hard to spot in review: a Python
test can start needing the geometry kernel, and a lightweight Rust edition can
start linking it. Neither shows up as a behaviour change in the line that caused
it. Two cheap static checks pin both:

- `tools/run_tests.py` answers "which modules may this test need?";
- `cargo xtask editions --check` answers "which crates may this edition link?".

They are separate because they guard different failures. Under plain
`unittest discover`, a test that imports `Part` still passes on a developer
checkout that happens to have `ferrocad_combined.abi3.so`, and fails only in CI's
App-only build. A single ordinary `[dependencies]` line can pull OCCT into
`ferrocad_toy` without changing any code the toy tests execute.

## 1. Extension sets: what a test run may import

A test run states which workbench modules it is **provided**: an **extension
set**. The runner installs a meta-path finder that makes every other workbench
module unimportable, so the set is enforced by construction rather than inferred
from whatever the developer has built. `--extension` names what is provided; the
runner hides the rest.

An **edition** is the shipped bundle: binary, app script, defaults, branding and
workbench set. An **extension set** is the test-side view of its workbench half:
a set of names, not a file. It is also not a packaging: `ferrocad_combined` is the
packaging `ferrocad_parametric` also uses, so it is not a name an extension set
takes.

| Extension set | Provides | What it is |
| --- | --- | --- |
| `base` | no workbench module: the `ferrocad` facade alone | Exactly what CI's `headless` job builds. Every workbench module — `ferrocad_combined`, `Part`, `Mesh` and `MeshPart` — is unimportable. |
| `workbenches` | `Part`, `Mesh` and `MeshPart` (over the `ferrocad_combined` packaging) | App + Part + Mesh in one extension. Nothing is hidden; it needs OCCT. |

`base` is the important one for ordinary tests. It is not a reduced convenience
build: it is the artifact the primary CI path produces, so a test that only passes
when `Part` is present is a test that would pass locally and fail in CI. Making
the workbench modules unimportable turns that into a local failure.

`--extension` is repeatable, so a run can name modules individually instead of
using a preset: `--extension Part --extension Mesh --extension MeshPart`. Naming
any of `Part`, `Mesh` or `MeshPart` also provides `ferrocad_combined`: those
modules are shims that import it. `ferrocad_combined` is not itself an
`--extension` value — it is packaging, not a workbench — and passing it is an
unknown-extension error.

```sh
python3 tools/run_tests.py --check                  # static contract check, no build (~0.06 s)
python3 tools/run_tests.py --extension base         # 41 modules, 673 tests, 8 skipped
python3 tools/run_tests.py --extension workbenches  # 45 modules, 704 tests, 3 skipped
```

Each extension set runs in a fresh subprocess, with `tools/extensions.install`
called before discovery. Import caches and module-level registration therefore
cannot leak from one set to the next. A set that provides workbench modules also
exports the local OCCT library path when `.occt/prefix` exists, so it works from
a plain shell as well as from `build.sh`.

The invariant the runner enforces is **pass or skip, never error**. A test that
needs something the set does not provide is a failure, not a silent skip. In
`base`, `import Part` raises, so a base module that acquired a workbench
dependency errors the run instead of disappearing from it.

## 2. The extension-set contract

`contracts/extension-sets.json` classifies every `tests/test_*.py` module as
either `base` or `workbenches`:

```json
{
  "schema": 2,
  "presets": {
    "base": [],
    "workbenches": ["Mesh", "MeshPart", "Part"]
  },
  "suites": {
    "base": ["test_commands", "..."],
    "workbenches": ["test_part", "..."]
  }
}
```

The runner reads this file; `--check` verifies that it names every discovered
module exactly once and that every named module exists. It also pins `presets`
against `tools/extensions.py`, so adding a workbench module is a reviewed diff.
It imports nothing, so it is the first check a contributor can run.

This is a contract rather than a decorator for a reason. If a base module silently
acquires a workbench dependency, it moves from the `base` suite to the
`workbenches` suite, which changes this file, which shows up as a reviewed diff.
The classification is also materialised by the run: in `base` the module errors;
with the workbench modules provided it passes. `--write-contract` uses exactly
that signal: a module that passes with the workbench modules and passes nothing
in `base` is `workbenches`. A module that passes nothing in either set stays
`base`, because it skips for an unrelated optional artifact (`fc_gen`, the
SimpleCAD tree) rather than for the kernel.

### Adding a test

1. Add `tests/test_something.py` and run
   `python3 tools/run_tests.py --write-contract`; the module starts in `base`
   unless its tests only pass with the workbench modules.
2. Run `python3 tools/run_tests.py --extension base`. The module must pass or
   skip, never error.
3. If the test needs `Part`, `Mesh`, `MeshPart`, or a module that imports one of
   them at module scope, it belongs in a module the contract classifies as
   `workbenches`. Import it directly there; the workbenches suite is only run
   when those modules are provided.

Do **not** add a `HAVE_PART` guard or a `skipUnless` decorator to keep a kernel
test in a base module. The guard converts the rule into a skip, so the dependency
disappears from the contract and the test can rot without anyone noticing. It is
also the reason the old `unittest discover` workflow was unreliable: the same
guard skipped on an App-only checkout and passed on a developer machine.

## 3. The edition closure check

`cargo xtask editions --check` reads `cargo metadata --format-version 1 --no-deps`
and walks the **normal** dependency edges from each edition. It needs no compiler,
no OCCT and no network, and runs in tens of milliseconds.

The walk counts only `kind: normal` edges. Dev- and build-dependencies are
dropped, so `ferrocad_part`'s dev-dependency on `ferrocad_occt` does not trip the
`ferrocad` rule. Optional dependencies count when a default-reachable feature
turns them on, which matches what `cargo build -p <edition>` links. The resulting
set is pinned in `contracts/editions.json`; adding a dependency is then a reviewed
diff instead of silent drift.

The rules themselves live in `crates/xtask/src/editions.rs`:

| Crate | Rule |
| --- | --- |
| `ferrocad` (base), `ferrocad_toy` | must not reach `ferrocad_occt`, `ferrocad_part`, `ferrocad_part_py`, `ferrocad_part_gui`, `ferrocad_mesh`, `ferrocad_mesh_py`, `opencascade-sys` |
| `ferrocad_gpui`, `ferrocad_stage`, `ferrocad_viewport`, `ferrocad_widgets`, `ferrocad_inventor`, `ferrocad_wgpu`, `ferrocad_ui` | the same list, for the same reason: they sit below Part |
| `ferrocad_py` | must not reach `ferrocad_part_py`, `ferrocad_mesh_py`, `ferrocad_occt` (the App-only extension) |
| `ferrocad_parametric` | must reach `ferrocad_part_py` and `ferrocad_occt` |
| `ferrocad_combined_py` | must reach `ferrocad_part_py` and `ferrocad_mesh_py` |

The purpose is narrow and deliberate: a lightweight edition must not silently
start bundling OCCT. `ferrocad_toy` is documented as kernel-free, `ferrocad_py`
is the App-only extension, and the shell/renderer layer sits below Part. The check
walks transitive edges, not just direct ones, because "the crate I added does not
mention OCCT" is not the same as "OCCT is not reachable from my edition".

### The geometry seam carve-out

`ferrocad_geom` depends on `ferrocad_curves`: the geometry seam described in
[`architecture/part-draft-layering.md`](architecture/part-draft-layering.md) §5.2.
`ferrocad_curves` is pure Rust math with no OCCT dependency, but every light crate
already depends on `ferrocad_geom` for `Shape`. The rules therefore allow
`ferrocad_curves` to be reached from a light edition **through `ferrocad_geom`
only**. A direct edge, or a route through a heavier crate, still fails and reports
the offending path. Removing that edge would move curve construction back into
every composition root; the check models it instead of pretending it does not
exist.

## 4. The CI order

Both checks are cheap, so they run before anything that compiles or downloads.
The order is:

```text
preflight -> headless -> geometry
```

- **preflight** runs `python3 tools/run_tests.py --check` and
  `cargo xtask editions --check`. Neither builds; contract drift is caught before
  a compiler or OCCT is involved.
- **headless** builds the primary App extension and runs
  `python3 tools/run_tests.py --extension base`. This is where a test that needs
  the kernel shows up.
- **geometry** fetches OCCT 7.8.1, builds and tests the OCCT-backed crates, builds
  `ferrocad_combined_py`, and runs the `workbenches` suite with the modules that
  build provides (`--extension Part --extension Mesh --extension MeshPart`).

The order is not only about speed. Each stage is allowed to need strictly more
than the one before, so a preflight failure means the later stages would have been
testing the wrong artifact.

## 5. Where the rules live

| File | What it owns |
| --- | --- |
| [`tools/extensions.py`](../../tools/extensions.py) | the extension sets and the import blocker |
| [`tools/run_tests.py`](../../tools/run_tests.py) | discovery, per-set subprocesses, `--check`, `--write-contract` |
| [`contracts/extension-sets.json`](../../contracts/extension-sets.json) | which suite each test module belongs to |
| [`crates/xtask/src/editions.rs`](../../crates/xtask/src/editions.rs) | the per-edition forbidden/required rules and the seam |
| [`contracts/editions.json`](../../contracts/editions.json) | the pinned transitive closure per edition |

Change the code and the contract in the same commit. For a dependency that is
intentional, `cargo xtask editions --write` regenerates the closure; for a new or
reclassified test module, `python3 tools/run_tests.py --write-contract` does the
same. If the change is a new boundary rather than a new edge, update this note,
[`architecture.md`](architecture.md) §6 and the architecture page that owns the
seam as well.
