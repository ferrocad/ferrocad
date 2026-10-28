# Testing and the edition contracts

Status: note (2026-10-28).

Two boundaries are easy to cross by accident and hard to spot in review: a Python
test can start needing the geometry kernel, and a lightweight Rust edition can
start linking it. Neither shows up as a behaviour change in the line that caused
it. Two cheap static checks pin both:

- `tools/run_tests.py` answers "which artifact is this test allowed to need?";
- `cargo xtask editions --check` answers "which crates may this edition link?".

They are separate because they guard different failures. Under plain
`unittest discover`, a test that imports `Part` still passes on a developer
checkout that happens to have `ferrocad_combined.abi3.so`, and fails only in CI's
App-only build. A single ordinary `[dependencies]` line can pull OCCT into
`ferrocad_toy` without changing any code the toy tests execute.

## 1. The two test images

A test run names an **image**: the set of modules that run must be able to import.
The runner installs a meta-path finder that makes everything else unimportable, so
the image is enforced by construction rather than inferred from whatever the
developer has built.

| Image | Importable | What it is |
| --- | --- | --- |
| `app` | `ferrocad` (the primary extension) | Exactly what CI's `headless` job builds. `ferrocad_combined`, `Part`, `Mesh` and `MeshPart` are unimportable. |
| `combined` | `ferrocad`, `Part`, `Mesh`, `MeshPart` in `ferrocad_combined` | App + Part + Mesh in one extension. Nothing is hidden; it needs OCCT. |

`app` is the important one for ordinary tests. It is not a reduced convenience
build: it is the artifact the primary CI path produces, so a test that only passes
when `Part` is present is a test that would pass locally and fail in CI. Making
the combined extension unimportable turns that into a local failure.

```sh
python3 tools/run_tests.py --check            # static contract check, no build (~0.06 s)
python3 tools/run_tests.py --image app        # 41 modules, 686 tests, 21 skipped
python3 tools/run_tests.py --image combined   # 44 modules, 704 tests, 3 skipped
```

Each image runs in a fresh subprocess, with `tools/test_images.install(image)`
called before discovery. Import caches and module-level registration therefore
cannot leak from one image to the next. `combined` also exports the local OCCT
library path when `.occt/prefix` exists, so it works from a plain shell as well as
from `build.sh`.

The invariant the runner enforces is **pass or skip, never error**. A test that
needs something the image does not have is a failure, not a silent skip. In
`app`, `import Part` raises, so an App module that acquired a kernel dependency
errors the run instead of disappearing from it.

## 2. The test-image contract

`contracts/test-images.json` classifies every `tests/test_*.py` module as either
`app` or `kernel`:

```json
{
  "images": ["app", "combined"],
  "suites": {
    "app": ["test_commands", "..."],
    "kernel": ["test_part", "..."]
  }
}
```

The runner reads this file; `--check` verifies that it names every discovered
module exactly once and that every named module exists. It imports nothing, so it
is the first check a contributor can run.

This is a contract rather than a decorator for a reason. If an App module silently
acquires a kernel dependency, it moves from the `app` suite to the `kernel` suite,
which changes this file, which shows up as a reviewed diff. The classification is
also materialised by the run: in `app` the module errors; in `combined` it
passes. `--write-contract` uses exactly that signal: a module that passes in
`combined` and passes nothing in `app` is `kernel`. A module that passes nothing
in either image stays `app`, because it skips for an unrelated optional artifact
(`fc_gen`, the SimpleCAD tree) rather than for the kernel.

### Adding a test

1. Add `tests/test_something.py` and run
   `python3 tools/run_tests.py --write-contract`; the module starts in `app`
   unless its tests only pass in `combined`.
2. Run `python3 tools/run_tests.py --image app`. The module must pass or skip,
   never error.
3. If the test needs `Part`, `Mesh`, `MeshPart`, or a module that imports one of
   them at module scope, it belongs in a module the contract classifies as
   `kernel`. Import it directly there; the kernel suite is only run against
   `combined`.

Do **not** add a `HAVE_PART` guard or a `skipUnless` decorator to keep a kernel
test in an App module. The guard converts the rule into a skip, so the dependency
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
| `ferrocad_py` | must not reach `ferrocad_part_py`, `ferrocad_mesh_py`, `ferrocad_occt` (the App-only image) |
| `ferrocad_parametric` | must reach `ferrocad_part_py` and `ferrocad_occt` |
| `ferrocad_combined_py` | must reach `ferrocad_part_py` and `ferrocad_mesh_py` |

The purpose is narrow and deliberate: a lightweight edition must not silently
start bundling OCCT. `ferrocad_toy` is documented as kernel-free, `ferrocad_py`
is the App-only image, and the shell/renderer layer sits below Part. The check
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
  `python3 tools/run_tests.py --image app`. This is where a test that needs the
  kernel shows up.
- **geometry** fetches OCCT 7.8.1, builds and tests the OCCT-backed crates, builds
  `ferrocad_combined_py`, and runs the combined image for the `kernel` suites.

The order is not only about speed. Each stage is allowed to need strictly more
than the one before, so a preflight failure means the later stages would have been
testing the wrong artifact.

## 5. Where the rules live

| File | What it owns |
| --- | --- |
| [`tools/test_images.py`](../../tools/test_images.py) | the image definitions and the import blocker |
| [`tools/run_tests.py`](../../tools/run_tests.py) | discovery, per-image subprocesses, `--check`, `--write-contract` |
| [`contracts/test-images.json`](../../contracts/test-images.json) | which suite each test module belongs to |
| [`crates/xtask/src/editions.rs`](../../crates/xtask/src/editions.rs) | the per-edition forbidden/required rules and the seam |
| [`contracts/editions.json`](../../contracts/editions.json) | the pinned transitive closure per edition |

Change the code and the contract in the same commit. For a dependency that is
intentional, `cargo xtask editions --write` regenerates the closure; for a new or
reclassified test module, `python3 tools/run_tests.py --write-contract` does the
same. If the change is a new boundary rather than a new edge, update this note,
[`architecture.md`](architecture.md) §6 and the architecture page that owns the
seam as well.
