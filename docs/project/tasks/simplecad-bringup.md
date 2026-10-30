# SimpleCAD bring-up: the closing cut

Status: task spec (2026-10-30). Cut from [`../TODO-30-10-2026.md`](../TODO-30-10-2026.md)
§P1 after the [fan-out](../record/2026-10-30-fan-out-todo-30.md). The measurement it
starts from is [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7.3;
the round's closed backdrop is
[`../record/2026-10-29-workplane-visual-hierarchy-and-grid.md`](../record/2026-10-29-workplane-visual-hierarchy-and-grid.md).

The goal is one scripted session that **draws on a face, extrudes, push/pulls, cuts a hole
and exports** without hitting a missing member, plus a tree that is green enough for the
remaining failures to be named rather than counted. Current state: **969 passed / 64
failed / 26 errored / 2 skipped** over 88/88 modules.

Slices are numbered `SC*`. They are ordered so each one is independently shippable and
independently testable; `SC1` and `SC2` are the only ones that change the Python surface.

## 0. What is already known (verified 2026-10-30)

- The **26 errors** are bucketed in the bring-up note. The **64 failures are not** — that is
  the first slice, because a bring-up cannot be scheduled against an unclassified count.
- The 3 `'NoneType' … 'ShapeType'` errors are **not** a boolean: `Shape.ShapeType` infers
  the kind from sub-shape counts, so a compound of one solid plus a loose edge reports
  `Solid`; SimpleCAD's `loose_edges` then early-returns `[]` and `assemble` returns `None`.
  `ferrocad_occt::part_ops::shape_kind` already exposes the exact `TopoDS_Shape` kind and is
  unused by the getter.
- `Part.makeWireString` is the only missing `Part.*` name, and it is a **FreeType** port
  upstream. We will not take FreeType: the outlines we need are already available in Rust
  (§SC2).
- The 5 `QApplication()` errors are GUI tests behind the shim (§SC3).

## SC0 — triage the 64 failures

- [ ] Re-run `tools/conformance.py --simplecad` and capture the 64 failure lines.
- [ ] Bucket them by the harness's own cause text (the same shape as the §7.3 error table):
      cause → count → file:line of the assertion.
- [ ] Split the buckets into *our defect*, *deliberate absence* and *test expectation*, and
      record the split in [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md)
      §7.3 beside the error table.
- [ ] Re-file the *our defect* buckets into SC4–SC7 or into P2/P4 if they belong there.

**Acceptance:** the bring-up note has a 64-failure table with every bucket named, and no
failure is left as "other". **Size:** S (measurement only).

## SC1 — exact `Shape.ShapeType`

- [ ] Use `ferrocad_occt::part_ops::shape_kind` (already implemented, already documented as
      “a compound holding one solid is still a compound”) in the Python shape getter instead
      of the sub-shape-count inference.
- [ ] Decide the null-shape answer: upstream's `ShapeType` for a null shape is `Compound`
      (the getter's current fallback); keep that.
- [ ] Run `cargo test -p ferrocad_part_py` and `tools/conformance.py --part`; fix the fallout.
      Expect places where our booleans return a compound upstream returns as a solid — those
      are *real* differences and must be fixed or recorded, not papered over by keeping the
      inference.
- [ ] Regression: a compound of one solid plus a loose edge reports `Compound`, keeps its
      `Edges`, and `loose_edges` finds the loose edge. Assert in
      `crates/ferrocad_part_py/tests/part_containers.rs`.

**Acceptance:** the three SimpleCAD `NoneType` errors clear, no conformance test regresses,
and the new test fails on the old getter. **Size:** S (~35 lines + fallout).

## SC2 — `Part.makeWireString` on the Rust text stack

**Decision (2026-10-30): no FreeType, no new C/C++ dependency.** The outlines are a font
problem, not a kernel one, and the workspace already resolves the stack:

| Crate | Version in `Cargo.lock` | Role |
| --- | --- | --- |
| `skrifa` | **0.40.0** (via `cosmic-text` ← `bite-gp-wgpu`) | font parsing, metadata, glyph outlines, advances |
| `read-fonts` | 0.37.0 (via `skrifa` 0.40) | tables behind `skrifa` |
| `harfrust` | **0.5.2** (via `cosmic-text`) | shaping (glyph ids + advances + kerning), if we want it |
| `fontdb` | 0.23.0 (via `cosmic-text`, `usvg`) | font discovery, only if we ever need it |

`parley` is **not** in the lock — do not add it for this. Note there are two `skrifa`
copies (0.40 and 0.44); depend on **0.40** so we reuse the copy `cosmic-text` already
builds instead of adding a third. Offline builds work because these crates are already in
the registry cache.

Upstream's signature is `makeWireString(text, dir, fontfile, height[, tracking])` — a
directory and a file name, not a path — so the shim must join them and reject a missing
file with `Part.OCCError`.

- [ ] Put the outline extraction in a kernel-free crate (recommended: `ferrocad_text`,
      modelled on `ferrocad_curves`: no OCCT, no Python). It takes `(text, font bytes,
      height, tracking)` and returns per-glyph contours of move/line/quad/cubic commands plus
      the advance sum.
- [ ] `ferrocad_part_py` maps those contours to OCCT: lines → `MakeEdge`, quadratic/cubic →
      `Geom_BezierCurve`/`Geom_BSplineCurve` via the existing seam, then `MakeWire`; one wire
      per closed contour, a compound of all of them (upstream's shape).
- [ ] Advance/kerning: use `skrifa` metrics first (upstream applies per-char advance plus a
      kerning pair, no complex shaping). Use `harfrust` only if the fixture shows the naive
      path is wrong; record which was chosen.
- [ ] Font source: the caller's path. SimpleCAD's default (`osifont-lgpl3fe.ttf`) is **not in
      this tree**, so decide whether we ship a font (licence: OFL only) or let the text tool
      raise a clear "choose a font" error. Do **not** add `osifont` without a licence review.
- [ ] Test fixture: there is no hermetic font in the repo. Vendor one small OFL font (or a
      subset) under a test fixture directory with its licence, and pin the expected advance
      width; a system-font path is not acceptable in a test.
- [ ] Acceptance: `Part.makeWireString("AB", dir, file, 10.0)` returns a compound of closed
      wires whose bounding box matches the expected advance; the name appears in the
      `Part.*` audit; SimpleCAD's text tool stops raising `AttributeError`.

**Acceptance:** the 42nd name resolves with a real implementation, and the test uses a
vendored font. **Size:** M (~300–450 lines across a new crate + binding, plus a fixture).

## SC3 — classify the `QApplication()` group

- [ ] Record the 16 tests behind the four failing `setUpClass` hooks (plus the inline
      constructor in `test_elongating.py`, 17 total) as **GUI** in the bring-up note, with
      the widget each one needs.
- [ ] Make the harness count them as a classified skip rather than an error, so the error
      budget reflects the API surface.

**Acceptance:** the 5 `TypeError` errors move to a named "GUI (out of surface)" bucket and
the error count drops by 5 without hiding a behavioural failure. **Size:** S (text + one
harness rule).

## SC4 — S4.4: the plane and the normal through the facade

SimpleCAD's `gui/snapping.py` (`_refine`, `axis_point`) needs the exact face plane/normal
and the active workplane, not tessellation.

- [ ] Expose the active workplane's origin/axes/normal and the picked face's plane through
      `FreeCADGui`, with the same state the viewport already holds.
- [ ] Write down what a *stale* plane means (a face deleted after binding) and return an
      explicit error rather than a previous frame.

**Acceptance:** a script reads the plane of a tilted face and gets numbers that agree with
the viewport's own snap context. **Size:** M (~150 lines).

## SC5 — S4.5: arm the push/pull manipulator from a face selection

- [ ] Arm the existing stem (`ferrocad_part_gui::push_pull`) from a face selection, so the
      session starts without a separate command.
- [ ] Carry the session's value into `document/operations.extrude` (the path the edition
      already uses) rather than a second code path.

**Acceptance:** selecting a face and dragging the stem produces the same document edit as
the current push/pull route, in one transaction. **Size:** M (~200 lines).

## SC6 — S4.6: SimpleCAD's own previews

The FerroCAD side already exists (`FreeCADGui.Preview` → `set_preview_items` → the shell
merges them), so this slice is SimpleCAD-side plus one decision.

- [ ] Decide whether labels/3-D text get a preview channel at all; if not, record it as a
      non-goal.
- [ ] Keep the shape-preview path SimpleCAD-compatible (it is the only consumer).

**Acceptance:** a decision recorded; any SimpleCAD overlay the project keeps uses the
existing channel. **Size:** S (decision) / M if the overlay ports.

## SC7 — the workflow acceptance

- [ ] One test file (`tests/test_simplecad_workflow.py` or an entry added to the registered
      `tests/test_part.py`) that runs the session end to end:
      `makeBox` → `generalFuse` with a line to split the top → `extrude` + `fuse` →
      `cut` a hole → `exportStl` → read back with `Mesh.Mesh` and check the volume.
- [ ] Assert volumes at each step (the fan-out verified every API exists; the expected
      numbers need a first run to pin).
- [ ] Add the file to the extension-set contract if it needs `Part`.

**Acceptance:** the test passes and its assertion numbers are pinned in the file. **Size:**
S/M (~120 lines).

## SC8 — re-measure and record

- [ ] Re-run the integrated tree; record passed/failed/errored/skipped and the new buckets
      in the bring-up note and in the successor backlog's §0.
- [ ] Delete the buckets this spec closed from §1.1 of the backlog.

**Acceptance:** the numbers move and the note says why. **Size:** S.

## Decisions needed before starting

1. **Where the text crate lives** (`ferrocad_text` vs inside `ferrocad_part_py`) — SC2
   recommends a kernel-free crate, matching `ferrocad_curves`.
2. **Whether we ship a font** — an OFL font is the only licence-clean option; otherwise the
   text tool keeps failing with a clear message.
3. **Shaping depth** — skrifa metrics + kerning is upstream-equivalent; harfrust is only
   worth it if a fixture proves otherwise.
4. **How aggressive SC1's `ShapeType` change is** — exact kind is upstream-faithful, but it
   will expose boolean results that are compounds where upstream produces solids. That is a
   bug report, not a reason to keep the inference.

## Non-goals

- A Qt-free widget layer (`SC3` classifies instead).
- `osifont` or any GPL font.
- FreeType, harfbuzz-rs, or any new C/C++ dependency for text.
- A general text/markup engine: `makeWireString` is one name, producing wires.
