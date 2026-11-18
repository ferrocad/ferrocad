# SimpleCAD: the canvas-first proving ground

Status: reference (2026-11-16). The research note is
[`../../project/research/simplecad-bringup.md`](../../project/research/simplecad-bringup.md);
the closed bring-up spec is [`../../project/tasks/simplecad-bringup.md`](../../project/tasks/simplecad-bringup.md);
the harness is [`conformance.md`](conformance.md).

SimpleCAD is a **separate application**, not a FerroCAD edition. It is a PySide,
canvas-first CAD app whose workflows are ordinary `FreeCAD` Python: a box, a profile drawn
on a face, a push/pull, a cut, an STL export. FerroCAD runs its tests and workflow against
the `FreeCAD` facade to find the API and seam gaps that a script written for FreeCAD
would hit.

That makes SimpleCAD the project's **proof by use**: it is a real consumer of the API,
not another copy of our own tests. It is also a moving reference — the checkout is pinned
for a measurement, and the harness records the revision with every number.

## What it measures

The unit group is SimpleCAD's `tests/unit` tree: **88/88 modules load** in one image. The
workflow group is `tests/test_simplecad_workflow.py`, which draws → push/pulls → cuts →
exports → reads the mesh back, asserting at every step.

The current baseline (2026-11-16, SimpleCAD `9180c4b`, v0.38.0):

| Measure | Value |
| --- | --- |
| Unit modules | **88 / 88** |
| Unit cases | **1039 passed, 6 failed, 2 errored, 7 skipped** |
| Workflow gate | green; **12 000 / 3 000 / 15 000 / 960 / 14 040 mm³**, STL read-back 44 facets |

The 7 skips are classified `QApplication()` groups plus the font/text skips; no test is
lost. The 6 failures and 2 errors are in the documented buckets in
[`../../project/TODO-15-11-2026.md`](../../project/TODO-15-11-2026.md) §0 — the mesh/topology
bucket and the documented face-order case are the two errors. No failure is an
unattributed missing name.

For comparison, the 2026-11-13 close of P1 was **985 / 49 / 18 / 7**. The P2 seam rounds
closed **47 failures** and **16 errors** without adding one; the P3 round closed the
remaining four `PartDesign` errors. The trajectory, not the absolute number, is the useful
part: each closed bucket named a seam rule in Part rather than a SimpleCAD workaround.

## What the workflow pins

The scripted acceptance is deliberately exact, because every face is planar:

| Step | Volume |
| --- | --- |
| `Part.makeBox(40, 30, 10)` | 12 000 mm³ |
| `generalFuse` with a line that splits the top | still 12 000, two profiles of 600 mm² each |
| extrude the left profile by 5 mm and fuse | 15 000 mm³ |
| cut an 8 × 8 × 15 hole | 14 040 mm³ |
| `exportStl`, read back with `Mesh.Mesh` | 14 040 mm³, 44 facets |

The equal B-Rep and mesh volumes are the point: the STL path and the kernel agree on this
all-planar scenario, so the gate detects a topology or report regression, not a
tessellation tolerance change.

## How to run it

The checkout sits **outside version control**, by default at `<repo>/../simplecad`.
`SIMPLECAD_DIR`, `FERROCAD_SIMPLECAD` or `--simplecad-root` can point elsewhere.

```sh
# The unit group (reads ../simplecad unless the variable is set)
python3 tools/conformance.py --simplecad

# The same group through the in-process extension test
export FERROCAD_SIMPLECAD=$PWD/../simplecad
cargo test -p ferrocad_part_py --test simplecad_conformance -- --nocapture

# The scripted workflow, as part of the workbenches extension set
python3 tools/run_tests.py --extension workbenches
```

`FERROCAD_SIMPLECAD_FILES=test_union,test_surface_ops` narrows the in-process run to
named file stems; `--list` lists the SimpleCAD unit stems. The harness requires the
combined extension (`Part`, `Mesh` and `FreeCAD` sharing one core), and a build from the
current tree — the packaged `.so` measures the packaged tree, not your working copy.

## What it is not

- **Not a FerroCAD edition.** It is not packaged, not shipped and has no FerroCAD UI.
- **Not a parity claim.** The numbers say how much of *this* app's tests pass against the
  facade; they do not promise every FreeCAD script runs.
- **Not a second test suite to maintain.** The source is the pinned SimpleCAD checkout;
  the harness only loads it and classifies the result. A GUI test it cannot run is a
  reported skip, not a silently dropped case.
- **Not the owner of a failure.** A red line is attributed to a Part/OCCT seam, a Draft
  seam or a documented test assumption; the reference checkout is not patched to make a
  number move.
