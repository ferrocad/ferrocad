# Conformance: running the upstream suite

Status: reference (2026-10-20; baselines updated 2026-10-24). The harness is milestone **M3d**
([`milestones.md`](../record/milestones.md)); this note is the setup and runbook for it.

The conformance harness answers one question: **how much of FreeCAD's own headless
Python test suite does our `FreeCAD` pass today?** It loads the upstream test files
*unchanged* against our facade, runs their `unittest` cases, and classifies every
failure — so a red line names the missing API rather than hiding it.

---

## 1. What it runs

| Group | Source | Requirements |
| --- | --- | --- |
| **App** (default) | `src/Mod/Test/{Document,StringHasher,UnitTests,UnicodeTests,TestApp,BaseTests,TestIntPairList,FreeCADInitTests}.py` | our extension module on `sys.path` |
| **Part** (`--part`) | `src/Mod/Part/parttests/*.py` | an image that links **both** `Part` and `FreeCAD` (one core) |
| **Draft** (`--draft`) | `src/Mod/Draft/drafttests/{test_base,test_grid_settings,test_pivy,test_airfoildat,test_import}.py` | the Qt-free half of Draft: `python/lazy_loader`, the `PySide` alias, `Draft_rc`, and `src/Ext` on the path (the harness adds them); `Part` is still required by two of them |

Each file reports `pass / fail / error / skip`, and files that cannot import are
reported as **load errors** with the missing name (e.g. `missing 'FreeCAD.Base'`). The
report is a *measurement*: `crates/ferrocad_part_py/tests/part_conformance.rs` asserts
only that the harness ran, not a pass count.

The `--draft` group is in the same spirit and reports the upstream revision with the
summary. Baseline at upstream `33adf61` (2026-10-22, after the GUI document API):
**10 passed, 1 failed, 0 errored, 0 skipped, 1 load error**. `test_grid_settings` is
**5/5**, and `test_pivy` is **2/2**: `FreeCADGui.getDocument(name)` now returns a
`GuiDocument` whose `ActiveView.getSceneGraph()` is a mutable `pivy.coin.SoSeparator`.
The remaining failure is the shared `Part` kernel rather than a Draft bug:
`test_airfoildat` is a load error and `test_import.DraftImport.test_import_draft`
fails on `import Part`. See
[`../tasks/d0-draft-import.md`](../tasks/d0-draft-import.md).

The **Draft M2 modules** are measured separately from that fixed list, because they need
the Python feature proxies: after P2a–P2c (2026-10-24), `drafttests/test_creation.py`
loads and runs 26 cases (15 passed, 1 failed, 10 errored), `test_array.py` loads
(1 errored), and **both have zero load errors**. The residual errors are attachment
(`AttachmentSupport`/`MapMode`) and a few link/property coercions; see
[`../tasks/part-feature-proxies.md`](../tasks/part-feature-proxies.md).

---

## 2. One-time setup

### 2.1 The upstream checkout

The tools expect it at `<repo>/../freecad-upstream`, **outside version control**:

```sh
cd /home/john/projects/ferrocad          # the project root, not the git repo
git clone --depth 1 --filter=blob:none --sparse \
    https://github.com/FreeCAD/FreeCAD.git freecad-upstream
cd freecad-upstream
git sparse-checkout set src/Mod/Test src/Mod/Part
```

This is the documented sparse/blobless method (see
[`coin-bridge-reuse-assessment.md`](../viewport/coin-bridge-reuse-assessment.md), appendix). The
two test trees are ~34 MB. Widen the checkout when you need more:

```sh
git sparse-checkout add src/Mod                     # all workbenches
git sparse-checkout set src                          # the full reference tree (~409 MB)
```

`tools/inventory.py` and `tools/codegen.py` scan generated stubs across `src/Mod`, so
they need the wider set; the conformance harness does not.

The clone is `HEAD` and therefore a moving target. Record the revision with any number
you report:

```sh
git -C ../freecad-upstream rev-parse --short HEAD
```

`--depth 1` cannot check out an older commit directly; to pin, `git fetch --depth 1
origin <sha>` then `git checkout <sha>` (or re-clone with `--branch <tag>`).

### 2.2 Our extension on `sys.path`

The harness imports `FreeCAD` from `python/`, so the extension must exist there:

```sh
cargo build -p ferrocad_py --features extension-module
cp target/debug/libferrocad_py.so python/ferrocad.abi3.so   # macOS: .dylib -> .so
```

`python/*.so` is gitignored. This is the same step CI's headless job performs.

### 2.3 OCCT (Part group only)

The Part tests need the kernel, so use the CI-matching prefix:

```sh
export CMAKE_POLICY_VERSION_MINIMUM=3.5
export OpenCASCADE_DIR=$HOME/.cache/ferrocad-occt/prefix/lib/cmake/opencascade
export OCCT_INCLUDE_DIR=$HOME/.cache/ferrocad-occt/prefix/include/opencascade
export CMAKE_PREFIX_PATH=$HOME/.cache/ferrocad-occt/prefix
export LD_LIBRARY_PATH=$HOME/.cache/ferrocad-occt/prefix/lib
```

See [`occt-bundling.md`](../architecture/occt-bundling.md) for obtaining the prefix.

---

## 3. Running it

```sh
# The App group (default curated files)
python3 tools/conformance.py --root ../freecad-upstream

# A subset
python3 tools/conformance.py --root ../freecad-upstream --files Document StringHasher

# What can be run
python3 tools/conformance.py --root ../freecad-upstream --list

# The Part group: through the one image that links Part + FreeCAD
FERROCAD_UPSTREAM=$PWD/../freecad-upstream \
  cargo test -p ferrocad_part_py --test part_conformance -- --nocapture
```

`tools/conformance.py --part` also exists, but only an image that links both modules
can run it; the cargo test supplies that image and calls the harness in-process. The
test is **skipped** when `FERROCAD_UPSTREAM` is unset, so ordinary `cargo test` stays
hermetic.

---

## 4. Baseline (2026-10-20, upstream `33adf61`, sparse `src/Mod/Test` + `src/Mod/Part`)

| Group | Result |
| --- | --- |
| App | **169 passed**, 41 failed, 5 errored, **0 files failed to load** |
| Part | **2 passed**, 1 failed, 95 errored, **3 files failed to load** (`TestPartApp` → missing `Draft`; the rest are missing `Part.Wire`/`BSplineCurve`, and `ViewObject`-style attributes) |

The App number matches the figure recorded in [`milestones.md`](../record/milestones.md) (169
passing across eight `Mod/Test` files). The Part group is the newer, much larger gap;
its load errors are the useful signal.

### Current (2026-10-24, upstream `33adf61`, same sparse checkout)

| Group | Result |
| --- | --- |
| Part | **531 passed**, 0 failed, 0 errored, 12 skipped, **0 files failed to load** |
| Draft | **10 passed**, 1 failed, 0 errored, 0 skipped, 1 load error |
| Draft M2 (`test_creation`, `test_array`) | **27 passed**, 0 failed, 0 errored, **0 load failures** |

The 2026-10-20 Part row is the pre-shim baseline: `TestPartApp.py` and
`TestPartMirror.py` failed to load (missing `Draft`), so TestPartApp's ~300 tests never
ran. A minimal `python/Draft` shim (`make_clone` over `App::Link`) removed both load
errors; the Part-surface slices then added the constructors, surfaces and group
semantics the loaded tests exercise. The harness puts the group roots ahead of `python/`,
so the **Draft** group still runs upstream `src/Mod/Draft/Draft.py`, not the shim; its
baseline is unchanged.

**No failures remain.** The sweep test now takes its correct-bounds branch because the
element-map version is enabled for shapes that carry a lineage, and the compound, fuse,
cut/common, general-fuse and sweep maps match upstream exactly. Six guarded asserts
still skip honestly: four consumer shapes in `solids.rs` store an empty lineage, and two
(`FaceMakerCheese` 10, `makeShellFromWires` 24) are composed-alias counts. See
[`../TODO-24-10-2026.md`](../TODO-24-10-2026.md) §P0.

### Draft M2 (Python feature proxies)

The two modules that exercise Python-defined objects are measured separately from the
fixed `--draft` list:

| Module | Result |
| --- | --- |
| `drafttests/test_creation.py` | **26 passed**, 0 failed, 0 errored |
| `drafttests/test_array.py` | **1 passed**, 0 failed, 0 errored |

`cargo test -p ferrocad_part_py --test draft_proxies -- --nocapture` runs them. The last
gaps were the attachment extension (`Part::AttachExtensionPython`), `PropertyLinkSubList`
with `(object, subname)` entries, the link extension, `Placement.translate`/`.Matrix`, and
`getSubObject` returning the named sub-shape (`Part::FeaturePy::getSubObject`) rather than
its owner.

`test_hatch` also imports `TechDraw` for `makeGeomHatch`. FerroCAD supplies that one
function as a small pure-Python shim (`python/TechDraw`) that reads the standard PAT file
from `<getResourceDir()>/Mod/TechDraw/PAT/`; `App.getResourceDir()` ends with the
separator upstream callers assume (`... + "Mod/TechDraw/PAT/FCPAT.pat"`).

### Reading the report

- `[LOAD ERROR]` — the module could not import; the message names the missing module or
  attribute (often a whole subsystem).
- `[ERROR]` — it imported, but a test raised (a missing runtime API).
- `[FAIL]` — it ran and the behaviour differs (a real parity bug).
- Files are ordered as the harness lists them; fix the first wall in a file before
  chasing later errors.

---

## 5. CI and the reference tree

- The `headless` CI job builds `python/ferrocad.abi3.so` and runs the Python tests, but
  it does **not** clone upstream: conformance is a local measurement, because the
  numbers move with upstream and are not yet a gate.
- `part_conformance` self-skips without `FERROCAD_UPSTREAM`.
- If we later want a tracked number, a scheduled job can clone the sparse tree at a
  pinned tag and publish the report. That is not in place.

The checkout is outside the git tree; never commit it. The `python/*.so` it exercises
is ignored too.

---

## 6. Related tools

| Tool | Uses the checkout | Needs |
| --- | --- | --- |
| `tools/conformance.py` | `src/Mod/Test`, `src/Mod/Part` | our extension |
| `tools/inventory.py` | generated `.pyi` / `*Py.xml` across `src/Mod` | a wider sparse set |
| `tools/codegen.py` | the same | a wider sparse set |
| `tools/test_conformance.py` | no (pure helpers) | nothing |
| `tools/test_inventory.py`, `tests/test_codegen.py` | guard on the tree existing | — |
