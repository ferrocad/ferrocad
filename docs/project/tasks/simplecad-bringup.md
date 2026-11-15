# SimpleCAD bring-up: the closing cut

Status: task spec (2026-10-30); **closed 2026-11-13** (SC0–SC8 done). Cut from
[`../record/TODO-31-10-2026.md`](../record/TODO-31-10-2026.md) §P1 after the
[fan-out](../record/2026-10-30-fan-out-todo-30.md); the slices are re-filed in
[`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P1 (closed) as P2/P3 backlog work. The
baseline at close was **985 passed / 49 failed / 18 errored / 7 skipped** over 88/88
modules (2026-11-13, after SC0–SC3; the cut started from 969 / 64 / 26 / 2); the later P2
rounds took it to **1039 / 6 / 2 / 7** on the clean checkout. The measurement it
starts from is [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7.3;
the round's closed backdrop is
[`../record/2026-10-29-workplane-visual-hierarchy-and-grid.md`](../record/2026-10-29-workplane-visual-hierarchy-and-grid.md).

The goal is one scripted session that **draws on a face, extrudes, push/pulls, cuts a hole
and exports** without hitting a missing member, plus a tree that is green enough for the
remaining failures to be named rather than counted. At SC0/SC1 the tree was **985 passed /
49 failed / 23 errored / 2 skipped** over 88/88 modules (2026-10-30; the cut
started from 969 / 64 / 26 / 2), and SC2 closed the last missing `Part.*` name.

Slices are numbered `SC*`. They are ordered so each one is independently shippable and
independently testable; `SC1` and `SC2` are the only ones that change the Python surface.

## 0. What is already known (verified 2026-10-30)

- The **64 failures are bucketed** (SC0, 63 our defect / 1 test expectation) and the 26
errors already were. After SC1 the tree is **985 / 49 / 23 / 2**.
- The 3 `'NoneType' … 'ShapeType'` errors were **not** a boolean: `Shape.ShapeType` inferred
  the kind from sub-shape counts, so a compound of one solid plus a loose edge reported
  `Solid`; SimpleCAD's `loose_edges` then early-returned `[]` and `assemble` returned `None`.
  SC1 fixed it with `ferrocad_occt::part_ops::shape_kind`.
- `Part.makeWireString` is the only missing `Part.*` name, and it is a **FreeType** port
  upstream. We will not take FreeType: the outlines we need are already available in Rust
  (§SC2).
- The 5 `QApplication()` errors are GUI tests behind the shim (§SC3).

## SC0 — triage the 64 failures

- [x] Re-run `tools/conformance.py --simplecad` and capture the 64 failure lines.
- [x] Bucket them by the harness's own cause text (the same shape as the §7.3 error table):
      cause → count → file:line of the assertion.
- [x] Split the buckets into *our defect*, *deliberate absence* and *test expectation*, and
      record the split in [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md)
      §7.3 beside the error table. Result: **63 our defect / 1 test expectation / 0
      deliberate absence**.
- [ ] Re-file the *our defect* buckets into SC4–SC7 or into P2/P4 if they belong there.
      Still open, but now tractable: `_short` (`tools/conformance.py:190`) was taught to skip
      stdlib `unittest` frames, so the 49 failures and 23 errors cite their own
      `test_*.py:NNN` (and the two biggest roots are a path-closure predicate in surface-line
      splitting and a missing degenerate-input rejection — see the note's re-bucket table).

**Acceptance:** the bring-up note has a 64-failure table with every bucket named, and no
failure is left as "other". **Size:** S (measurement only).

## SC1 — exact `Shape.ShapeType` — done 2026-10-30

- [x] Use `ferrocad_occt::part_ops::shape_kind` (already implemented, already documented as
      “a compound holding one solid is still a compound”) in the Python shape getter instead
      of the sub-shape-count inference.
- [x] Decide the null-shape answer: `Compound`, as before.
- [x] Run `cargo test -p ferrocad_part_py` and fix the fallout. **No expectation changes were
      needed**: 25 lib + 139 integration tests pass unchanged.
- [x] Regression: a compound of one solid plus a loose edge reports `Compound`, keeps its
      `Edges`, and `loose_edges` finds the loose edge. Assert in
      `crates/ferrocad_part_py/tests/part_containers.rs`.

**Acceptance:** the three SimpleCAD `NoneType` errors clear, no conformance test regresses,
and the new test fails on the old getter. **Size:** S (~35 lines + fallout).

## SC2 — `Part.makeWireString` on the Rust text stack — done 2026-10-30

**Landed:** `crates/ferrocad_text` (9 unit tests + a doctest) and
`crates/ferrocad_part_py/src/text.rs` (6 integration tests + 5 Python tests). All 42 of
SimpleCAD's `Part.*` names now resolve. What follows is the decision and the design as
written before the code, with the outcome recorded at the end ([§Outcome](#outcome-2026-10-30)).

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

### Design (2026-10-30)

**1. Crate shape — `ferrocad_text`.** A new leaf `crates/ferrocad_text`, a workspace member
and a `default-members` entry (`Cargo.toml:7-15, 56-59`), shaped like
`ferrocad_curves/Cargo.toml:15-19`: `font.rs` (parse, upem, line height, advance, kern),
`outline.rs` (`PathCommand`, `Contour`, the `OutlinePen` impl) and `lib.rs`:

```rust
pub fn outline_text(font: &[u8], text: &str, height: f64, tracking: f64) -> Result<TextOutline, TextError>;
pub struct TextOutline { pub glyphs: Vec<GlyphOutline>, pub advance: f64 }
pub struct GlyphOutline { pub glyph_id: u32, pub origin: Point2, pub advance: f64, pub contours: Vec<Contour> }
pub struct Contour { pub start: Point2, pub commands: Vec<PathCommand> }  // closes on start
pub enum PathCommand { LineTo(Point2), QuadTo { control: Point2, to: Point2 }, CubicTo { c1: Point2, c2: Point2, to: Point2 } }
```

`Point2` is mm: baseline `y = 0`, pen origin `x = 0`. Dependencies are `skrifa = "0.40"`
and `read-fonts = "0.37"` (the `kern` table only) — no `ferrocad_*`, no OCCT, no PyO3:
the leaf rule at `part-draft-layering.md:453-457` (a leaf must not name `Shape`). A glyph
with no outline returns empty `contours` and a non-zero advance (`FT2FC.cpp:201-208`).

**2. skrifa 0.40 calls.** The registry copy is unreadable in this sandbox, so these names
are **to-verify** on `docs.rs/skrifa/0.40.0`; versions are pinned at `Cargo.lock:5625-5639`
(0.40.0 → read-fonts 0.37.0). Depend on `0.40` to reuse `cosmic-text`'s copy.

```rust
let font = FontRef::new(bytes)?;                                 // parse       [tv]
let gid = font.charmap().map('A').unwrap_or(GlyphId::new(0));
let size = Size::unhinted(font.upem() as f32, ..);               // MetadataProvider, font units [tv]
let advance = font.glyph_metrics(size, ..).advance_width(gid);
font.outline_glyphs().get(gid)?.draw(DrawSettings::unhinted(size, ..), &mut pen)?;  // [tv]
```

The pen trait (`OutlinePen` [tv]) is `move_to(p)`, `line_to(p)`, `quad_to(ctrl, to)`,
`curve_to(c1, c2, to)`, `close()`; the size is the upem, so coordinates stay in font units.

**3. Units and placement.** In `FT2FC.cpp`: em = 480 pt (`:175-186`, the `*10` its comment
explains); `scale = height / FTFace->height / 10` (`:188-189`); the pen adds `kern(prev,
cur)` then the advance (`:198-211`, 26.6 px); the glyph is placed at `PenPos * scale +
charNum * tracking` (`:382-387`). So the scale is `height / face->height`, FreeType's
**line height** (hhea ascender − descender + lineGap; OS/2 sTypo\* with USE_TYPO_METRICS),
not upem and not cap height; kerning is the legacy `kern` table only (`:425-441`), exactly
what `read-fonts` exposes, and `tracking` is per character *index*, in output mm, unscaled.
Ours mirrors it: `mm = u * height / line_height`, plus `i * tracking`.

**A deliberate divergence.** The `/10` at `:189` is not offset by the 10× em at `:178`: they
cancel once, not twice, so upstream's raw output is 10× under `height`. Draft hides it by
rescaling `Size / YMax("M")` (`ScaleToSize` defaults to `True`, `shapestring.py:101-104,
224-228`); SimpleCAD never rescales (`edit.py:239-246`), so its 5 mm letter arrives at 0.5 mm.
**Implement upstream's formula exactly, including the `/10`** — the project's contract is that
an upstream-shaped script gets the same shape, and a 10× diverge would be a silent
incompatibility. Open question 1 settles only whether the `/10` is upstream's bug; if it is,
fixing it is a separate, deliberate change with SimpleCAD in the loop.

**4. OCCT mapping: exact beziers, no seam change.** Quads and cubics become
`ferrocad_curves::BezierCurve` → `make_edge` (`ferrocad_geom/src/lib.rs:736`) →
`edge_from_bezier` (`ferrocad_occt/src/backend.rs:1014, 4277`) → `fc_bezier_edge`
(`bridge.rs:301`); three and four poles both fit, so nothing is added. Upstream's
`ConvertToBSpline` (`FT2FC.cpp:288-293`) exists only because it builds 2-D segments on a
`Geom_Plane` (`:357`); 3-D beziers at `z = 0` are the same geometry. Lines use `LineSegment`
→ `make_edge`, a contour `make_wire` (`ferrocad_geom/src/lib.rs:744`). Exact, not polyline:
a polyline would quantise curves that push/pull then extrudes.

*Correction to work item 2 (verified).* Upstream returns `list[list[Wire]]`
(`AppPartPy.cpp:2399`, `FT2FC.cpp:202-203`), not a compound: SimpleCAD flattens it
(`edit.py:240-243`), Draft extends it (`ArchPanel.py:835`) — return the same nest. Upstream
also reverses every contour wire unconditionally (`:394-405`); keep font direction instead,
so bullseye fill works.

**5. Font fixture.** No font exists in the tree (`**/*.ttf|otf|ttc` finds none); the conda
copy under `.occt/` is not a fixture. Vendor, unmodified,
`crates/ferrocad_text/tests/fixtures/fonts/SpaceMono-Regular.ttf` (OFL-1.1, ~93 KB) with
`OFL.txt` and a `README.md` (source URL, sha256) — unmodified, because OFL's Modified-Version
clause would force a rename away from any Reserved Font Name. If size matters, `pyftsubset`
to `U+0020,0041,0042,0049,004D` (~15 KB), rename (`FerroCADTest`), keep the notice. Monospace
is deliberate: a constant advance makes the bounding-box assertion non-tautological.

**6. Test plan (offline).** Fixture paths from `concat!(env!("CARGO_MANIFEST_DIR"),
"/tests/fixtures/fonts/…")` and `Path(__file__).resolve().parents[1] / "crates/ferrocad_text/tests/fixtures/fonts"`.

- `ferrocad_text`: pin upem, line height and advance from the fixture; `"I"` is one glyph
  with only closed contours; `"AB"` advance is `2 ×` the single-glyph advance with origins
  `i × advance`; `tracking = 1.0` shifts glyph 1 by 1 mm.
- `ferrocad_part_py/tests/part_text.rs` (boots like `part_containers.rs`): 2 characters,
  every wire `isClosed()`, `Compound(wires).BoundBox.XLength` equals the value pinned from
  the fixture (advance minus both side bearings), the 4-arg form agrees, a missing file
  raises `Part.OCCError` (`AppPartPy.cpp:2395-2397`).
- `ferrocad/tests/test_part.py`: the same call through `Part`, plus empty text and bad font.

**7. Open questions.**

1. *The `/10`*: read upstream's `BoundBox.YMax` for `"M"` at height 10 — ≈6 mm means the
   division is a bug, ≈0.6 mm means it is the contract and SimpleCAD ships 10× small text.
2. *An OFL font already cached*: `ls ~/.cargo/registry/src/*/*/ | grep -iE '\.(ttf|otf)$'` first.
3. *The skrifa names*: `grep -n "fn draw\|fn unhinted\|advance_width"
   ~/.cargo/registry/src/*/skrifa-0.40.0/src/`.
4. *FreeType's height source*: compare the fixture's hhea with OS/2 sTypo\*; a
   USE_TYPO_METRICS font would make our line height differ from upstream's.
5. *Containers*: `.ttc` needs an index and ShapeString also accepts `.pfb`
   (`shapestring.py:177`); decide `.ttf`/`.otf` + `.ttc` index 0, `OCCError` for Type 1.

### Work items

- [x] Put the outline extraction in a kernel-free crate (`ferrocad_text`, modelled on
      `ferrocad_curves`: no OCCT, no Python). `outline_text(font_bytes, text, height,
      tracking) -> TextOutline`.
- [x] `ferrocad_part_py` maps contours to OCCT: lines → `LineSegment`/`MakeEdge`,
      quad/cubic → `BezierCurve` → `make_edge` (exact, no seam change), then `make_wire`;
      one wire per closed contour. **The return shape is upstream's `list[list[Wire]]`.**
- [x] Advance/kerning: `skrifa` metrics plus the legacy `kern` table (what
      `FT_Get_Kerning` reads). No shaping: `harfrust` is unused, and the fixture confirms
      the naive path.
- [x] Font source: the caller's `(dir, fontfile)`, joined; a missing file raises
      `Part.OCCError`. No `osifont` is added.
- [x] Test fixture: **Liberation Mono** (OFL-1.1, 319 KB) vendored unmodified under
      `crates/ferrocad_text/tests/fixtures/fonts/` with `OFL.txt` and a provenance README.
- [x] Acceptance: `makeWireString("AB", dir, file, 10.0)` returns two inner lists of
      closed `Wire`s; tracking shifts the second glyph by exactly its value; the name is in
      the `Part.*` audit; the text tool no longer raises `AttributeError`.

### Outcome (2026-10-30)

| Where | What |
| --- | --- |
| `crates/ferrocad_text` | `font.rs` (metrics, charmap, `kern`, outlines) and `outline.rs` (pen, public types, `outline_text`); **9 unit tests + 1 doctest** |
| `crates/ferrocad_part_py/src/text.rs` | `makeWireString`, contour → edges → wire, `list[list[Wire]]`; **6 integration tests** |
| `tests/test_part.py` | 5 Python cases (shape, space, tracking, missing font, zero height) |
| `contracts/part-api.json` | re-pinned: 39 functions (was 38), 252/408 methods unchanged |

Measured: Part extension set **743 / 3** at SC2 (was 738 / 3), now **756 / 3** with the 3-D
text workflow and `Draft.make_shape_string`
([`tests/test_text_workflow.py`](../../../tests/test_text_workflow.py)); base unchanged at
**687 / 8**; SimpleCAD **985 / 49 / 23 / 2** (unchanged — the suite does not exercise the
text tool).

**Still open:** whether upstream's `/ 10` is a bug (the scale is reproduced as-is, so
SimpleCAD gets the same size it would from upstream); `.ttc` indices and Type 1 `.pfb`
(the fixture is a plain `.ttf`); and `Draft`'s own `ShapeString` rescale, which is not part
of this slice.

**Acceptance:** the 42nd name resolves with a real implementation, and the test uses a
vendored font. **Size:** M (~300–450 lines across a new crate + binding, plus a fixture).

## SC3 — classify the `QApplication()` group — done 2026-11-13

- [x] Record the 16 tests behind the four failing `setUpClass` hooks (plus the inline
      constructor in `test_elongating.py`, 17 total) as **GUI** in the bring-up note, with
      the widget each one needs.
- [x] Make the harness count them as a classified skip rather than an error, so the error
      budget reflects the API surface.

**Acceptance met:** the 5 `TypeError` errors move to the named
“GUI (out of surface)” skip bucket and the error count drops by 5 without hiding a
behavioural failure. The table is in
[`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7; the harness rule
is `Collector(classify_gui=…)` in `tools/conformance.py`. **Size:** S.

## SC4 — S4.4: the plane and the normal through the facade — done 2026-11-13

SimpleCAD's `gui/snapping.py` (`_refine`, `axis_point`) needs the exact face plane/normal
and the active workplane, not tessellation.

- [x] Expose the active workplane's origin/axes/normal and the picked face's plane through
      `FreeCADGui`, with the same state the viewport already holds.
- [x] Write down what a *stale* plane means (a face deleted after binding) and return an
      explicit error rather than a previous frame.

**What landed.** `ferrocad_stage::view` publishes a `PublishedWorkplane` (the plane, its
label, the offset and its extent) beside the camera; the shell republishes it from
`ActiveWorkplane::into` on every camera publication, so a script sees the frame the grid
is drawn on. `published_face_frame` resolves a named planar face from the **published
stage** with the same `FaceFrame::resolve` rule the viewport's snap context uses (the
queue's world transform applied), and `PublishedStage::entity_named` inverts the
object-name table. The bindings are `ferrocad_stage.set_view_workplane`,
`view_workplane`, `entity_of` and `face_plane`; `FreeCADGui.ActiveView.getWorkplane()`
and `getFacePlane(obj, element)` read them. A face-bound plane whose entity no longer
resolves raises `RuntimeError` (it is not replayed), and a missing or curved element
raises `ValueError`.

**Acceptance met:** `tests/test_gui_view_facade.py::ViewWorkplaneTest` publishes a quad in
the plane `z = x` and reads back `n = (-√½, 0, √½)`, `u = (√½, 0, √½)`, `v = (0, 1, 0)`
and the centroid `(0.5, 0.5, 0.5)`; the Rust tests
(`ferrocad_stage::view::tests`) prove those numbers equal the resolver's own and that a
deleted face stops resolving. **Size:** M.

## SC5 — S4.5: arm the push/pull manipulator from a face selection — done 2026-11-13

- [x] Arm the existing stem (`ferrocad_part_gui::push_pull`) from a face selection, so the
      session starts without a separate command.
- [x] Carry the session's value into `document/operations.extrude` (the path the edition
      already uses) rather than a second code path.

**What landed.** The path already existed end to end — a scripted
`FreeCADGui.Selection.addSelection(obj, "Face6")` is drained into the shell's own
selection, `rearm_selection` asks the edition's factory for the stem, and the drag's
`InteractionCommit` reaches `python/ferrocad_parametric/__init__.py::_push_pull` (one
`Part::Extrusion` + `Part::Fuse`/`Cut` transaction). This slice closes it with an
end-to-end test instead of a new route: `the_push_pull_stem_drives_the_document_in_one_
transaction` in `crates/ferrocad_parametric/tests/push_pull_round_trip.rs` selects a
planar `Face1`, grabs the projected stem tip, drags along the normal, applies the
commit through the edition's hook, asserts the fused feature is displayed, and undoes
once to prove the edit is one transaction.

**Acceptance met:** the stem's commit produces the same document edit the edition's
push/pull route always produced, in one undo step. **Size:** M (verification + test).

## SC6 — S4.6: SimpleCAD's own previews — decision 2026-11-13

The FerroCAD side already exists (`FreeCADGui.Preview` → `set_preview_items` → the shell
merges them), so this slice is SimpleCAD-side plus one decision.

- [x] Decide whether labels/3-D text get a preview channel at all; if not, record it as a
      non-goal.
- [x] Keep the shape-preview path SimpleCAD-compatible (it is the only consumer).

**Decision.** SimpleCAD's Coin previews are **not** ported. The supported preview channel
is `FreeCADGui.Preview` → transient `ferrocad_stage.ShapeItem`s (unpickable by
construction, merged by the shell beside the document stage). 3-D text labels are a
**non-goal** for that channel: a snap label belongs on the status bar / the tool HUD,
where it is crisp, selectable and independent of camera distance; drawing `SoText2` would
mean a font rasterisation path the preview system does not need. What *did* land is the
imperative-coin lifecycle an overlay consumer needs: `SoGroup.getChild`,
`insertChild`, `removeChild` (node or index too), `removeAllChildren`,
`findChild`, plus `SoNode.isOfType` and inherited `getClassTypeId()`. A registered overlay
is pulled by the shared registry and compiles to line geometry with the depth state it
asked for (`an_overlay_subtree_is_pulled_and_compiles_as_lines` in
`crates/ferrocad_inventor_py/tests/coin_scene.rs`).

**Acceptance met:** the decision is recorded; the only shape-preview channel is the
existing `FreeCADGui.Preview`, and an imperative Coin overlay reaches the viewport through
the shared registry. **Size:** S (decision) + the small facade-completeness tail.

## SC7 — the workflow acceptance — done 2026-11-13

- [x] One test file (`tests/test_simplecad_workflow.py` or an entry added to the registered
      `tests/test_part.py`) that runs the session end to end:
      `makeBox` → `generalFuse` with a line to split the top → `extrude` + `fuse` →
      `cut` a hole → `exportStl` → read back with `Mesh.Mesh` and check the volume.
- [x] Assert volumes at each step (the fan-out verified every API exists; the expected
      numbers need a first run to pin).
- [x] Add the file to the extension-set contract if it needs `Part`.

**Acceptance met:** `tests/test_simplecad_workflow.py` passes and pins **12 000**, **3 000**,
**15 000**, **960** and **14 040** mm³, with the STL read-back equal to the B-Rep volume;
`contracts/extension-sets.json` lists it under `workbenches`. **Size:** S/M.

## SC8 — re-measure and record — done 2026-11-13

- [x] Re-run the integrated tree; record passed/failed/errored/skipped and the new buckets
      in the bring-up note and in the successor backlog's §0.
- [x] Delete the buckets this spec closed from §1.1 of the backlog.

**Final baseline (at close, 2026-11-13).** The in-process run (upstream `9180c4b`, worker
paths exported) was **985 passed, 49 failed, 18 errored, 7 skipped over 88/88 modules**,
with the SC7 workflow gate green. No pass, failure, error or skip moved during SC4–SC6;
those slices are façade work the unit suite does not exercise. The 49 failures were
attributed to five named buckets plus one documented test assumption (full table in
[`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7):

| Bucket | n | Owner |
| --- | --- | --- |
| Boolean / topology partition | 16 | P2 |
| Document undo, transaction and labels | 5 | P2 |
| Part query strictness, tolerance and determinism | 10 | P2 |
| Shape identity, ownership and BRep export | 6 | P2 |
| Surface / freeform | 11 | P3 |
| Test assumption: exact `BoundBox` | 1 | out of surface |

**37 → P2, 11 → P3, 1 documented out-of-surface, 0 unattributed** at SC8 (the later P2
seam rounds closed 43 failures; see [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P2 for
the current buckets). No failure needed a missing name or a GUI; the 7 skips are the Qt
groups plus the font and text skips. SC0's last item (re-filing the failures) is closed
with this table, so **P1 is closed**. **Size:** S.

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

---

# The UI bring-up (UI-1..UI-5)

Status: open (opened 2026-11-15). The geometry and document surface is measured —
**1039 / 6 / 2 / 7** over 88/88 modules on the clean checkout. This cut is the **UI**
half: what SimpleCAD's own interface asks of a host, what this repository already answers,
and the order to close the rest.

## UI-0. What SimpleCAD's UI is

`../simplecad` is one FreeCAD **workbench**, and almost all of it is Qt:

| Piece | File(s) | Needs |
| --- | --- | --- |
| the workbench | `InitGui.py` (5 lines), `simplecad/gui/workbench.py` (36) | `Gui.Workbench`, `Gui.addWorkbench`, `appendToolbar`/`appendMenu`/`appendContextMenu`, `GetClassName` |
| the controller | `simplecad/gui/controller.py` (1670) | a `QMainWindow` (`Gui.getMainWindow()`), `statusBar()`, a `QApplication` event filter, `QTimer`s, dock widgets, dialogs |
| the tools' state machines | `simplecad/gui/tools/` (39 modules) | the controller + widgets for the numeric inputs |
| the 3-D drawing | `display.py` (318), `surface_display.py` (254), `panels.py`, `face_browser.py` (619) | `pivy.coin` nodes under a Coin/SoQt scene graph |
| the look | `viewport_style.py` (1084), `toolbar.py`, `status_bar.py`, `settings_dialog.py` | `QtGui` palettes/icons, a Coin scene |
| the commands | `commands.py` (150), `command_state.py` (351) | `Gui.addCommand`, `Gui.runCommand`, `Gui.activeDocument`, `Gui.activateWorkbench` |

`InitGui.py` itself is trivial and portable:

```python
import FreeCADGui as Gui
from simplecad.gui.workbench import SimpleCADWorkbench
Gui.addWorkbench(SimpleCADWorkbench())
```

Everything expensive happens in `Activated()`: it builds a Qt control dock on the real main
window, installs an application event filter, starts three timers and starts the Coin
display layers.

## UI-1. The symbol surface, measured

Counted over `simplecad/`, `tests/gui/` and `tests/unit/`.

### `FreeCADGui`

| Symbol | Uses | This repository |
| --- | ---: | --- |
| `Gui.Selection` | 387 | **have** |
| `Gui.updateGui` | 142 | **have** |
| `Gui.activeDocument` | 114 | **have** |
| `Gui.activateWorkbench` | 89 | **missing** |
| `Gui.getMainWindow` | 66 | returns the GPUI window handle, not a `QMainWindow` |
| `Gui.runCommand` | 31 | **have** |
| `Gui.listCommands` | 13 | **have** |
| `Gui.addCommand` | 2 | **have** |
| `Gui.Workbench`, `Gui.addWorkbench` | 1 each | **missing** |
| `Gui.activeWorkbench` | 1 | **missing** |

### Qt

`QtCore.Qt` (290), `QtWidgets.QApplication` (169), `QtCore.QEvent` (149), then the widget
vocabulary a modelling workbench needs: `QDialog`/`QLabel`/`QComboBox`/`QPushButton`/
`QCheckBox`/`QVBoxLayout`/`QFormLayout`/`QDoubleSpinBox`/`QSlider`/`QTreeWidget`/
`QDockWidget`/`QProgressBar`/`QMdiArea`/`QProcess`, and `QtGui.QKeyEvent`/`QIcon`/`QColor`/
`QPalette`/`QImage`/`QGuiApplication`/`QPainter`/`QPixmap`.

`python/PySide` ships a **name allowlist** (enough for upstream scripts to *import*), a
`QApplication` that deliberately raises, and an opt-in
`FERROCAD_PYSIDE_BACKEND=PySide6|PySide2` that aliases a real installation. Real **PySide6
6.10.2 is present in the development image**.

### `pivy.coin`

38 modules `from pivy import coin`. Nodes used: `SoSeparator` (49), `SoCoordinate3` (24),
`SoPickStyle`/`SoLightModel`/`SoDrawStyle`/`SoBaseColor`/`SoMaterialBinding`/`SoMaterial`
(13–18 each), `SoLineSet`, `SoText2`, `SoPointSet`, `SoIndexedFaceSet`, `SoTranslation`,
`SoShapeHints`, `SoSwitch`, `SoSearchAction`, the event classes (`SoMouseButtonEvent`,
`SoButtonEvent`), `SoPerspectiveCamera`/`SoOrthographicCamera`, `SoFieldSensor`, `SbColor`,
`SbVec2s`/`SbVec3f`. `python/` ships a `pivy.coin` facade that the Part tests already drive.

## UI-2. Two routes, and what each can carry

**(a) Host SimpleCAD's own UI through a `FreeCADGui`/Qt/Coin shim.** Portable: the
workbench declaration (menus, toolbars, command ids), the command classes'
`GetResources`/`Activated`, and every tool state machine that is pure Python over the
document. **Not portable:** `controller.activate()` and everything under it — it needs a
real `QMainWindow` (dock widgets, `statusBar()`), a `QApplication` event filter, `QTimer`s
and a Coin/SoQt scene graph to draw into, and FerroCAD's shell is GPUI over wgpu. So route
(a) is a **headless** route: exactly right for measurement, not for the shell.

**(b) A native edition that re-exposes SimpleCAD's tools on our viewport.** The modelling
layer (`simplecad/core`, `modeling`, `surfaces`, `mesh`, `repair`) is Qt-free and green.
`ferrocad_simplecad` would be a Python workbench module in the Parametric/Sculpt shape that
registers **our** command ids and tools, implementing each by calling that layer, with
`ferrocad_stage` tools and the existing manipulators for the interaction. This is the
product path and the only one that gives an interactive viewport; it is per-tool work.

## UI-3. The slices

| # | Slice | State |
| --- | --- | --- |
| **UI-1** | **Measure the Qt-blocked unit modules headlessly.** `--simplecad-gui` runs the five modules the main group skips (they build a `QApplication` in `setUpClass`) against real PySide6 with the offscreen platform, and self-skips when no distribution is installed. | **landed 2026-11-15**: **78 / 1 / 0 / 0** over five modules |
| UI-2 | **The workbench host.** `Gui.Workbench`, `addWorkbench`, `listWorkbenches`, `activateWorkbench`, `activeWorkbench`, `appendToolbar`/`appendMenu`/`appendContextMenu` → the shell's command catalogue. `../simplecad/InitGui.py` imports and declares its five toolbars and menus; `--check-init` proves it headlessly. | **landed 2026-11-15**: five toolbars, six menus, 49 ids; see UI-5 |
| UI-3 | **The edition skeleton.** `ferrocad_simplecad` (crate, app script, `--check-init`), the `SimpleCAD_Rectangle` id and the two-click rectangle on our stage, and the Parametric tool/manipulator ids re-registered beside it so the inherited command set resolves. Pinned by a round-trip test in the Parametric mould. | **landed 2026-11-15**: edition + first native tool; see UI-6 |
| UI-4 | **The rest of the tool surface, in use order.** The three round drawing tools — Circle, Polygon, Arc — landed 2026-11-15 (see UI-7). Next: Round (the dress-up scrubber, already generic), Offset/Elongate, Move/Rotate, then the surface tools (imprint/shell/fill-hole/unwrap), each a command over `simplecad.modeling`. The 63 `tests/gui/gui_*.py` scripts are the behavioural reference: each is a state machine to translate into a stage-tool test. | **in progress**: Circle/Polygon/Arc landed |
| UI-5 | **The two windows that are not the viewport**: the face browser (a dock panel) and the numeric-input rows. The shell already has a properties panel and a manipulator numeric row; these are the same seams. | after UI-3 |

## UI-4. UI-1, as measured

```sh
python3 tools/conformance.py --simplecad-gui
```

| Module | Tests | Result |
| --- | ---: | --- |
| `test_dense_drawing` | 27 | 26 pass, **1 fail** (`test_a_line_between_holes_reaches_them_where_their_display_mesh_falls_short`) |
| `test_elongating` | 20 | 20 pass |
| `test_measuring` | 13 | 13 pass |
| `test_panel_placement` | 5 | 5 pass |
| `test_push_pull_ui` | 14 | 14 pass |
| **total** | **79** | **78 pass, 1 fail, 0 errored, 0 skipped** |

The five "GUI" skips were never a widget-tree problem: four modules are green the moment a
real `QApplication` exists, and the fifth is a `BoundBox` premise. The suite's Qt
dependency is **five modules and one failing assertion**, not a surface.

**The one failure, classified.** The case calls `plate.tessellate(1)` and then asserts each
hole rim's `BoundBox` *stops short* of its facing point by more than
`2 × BOUNDS_MARGIN` (`0.002` mm) — its comment says "a shape's BoundBox follows its display
mesh and can be smaller than its curves". On this seam the bound is **exact**
(`lower.BoundBox.YMax == 25.0`, so `25 − YMax == 0.0`). That is the same rule the upstream
source states (`TopoShape::getBoundBox` is `BRepBndLib::Add` with `SetGap(0.0)`), so the
premise cannot be derived from the checkout this repository measures. It is the same
out-of-surface bucket SC8 already recorded ("Test assumption: exact `BoundBox`", 1). What
the case is *about* — the new line closes the region through both rims — **works**: the
operation after the failing assert gives 9 faces of 9, `isValid()`, `isClosed()`, and the
volume unchanged (13 108.4073 mm³). Recorded, not "fixed": matching a mesh-derived bound
would make `BoundBox` wrong for every other caller.

## UI-5. UI-2, as measured

```sh
python3 tools/run_tests.py --extension base        # test_workbench_host: 10 cases
```

`FreeCADGui` gained the workbench host (`Workbench`, `addWorkbench`, `removeWorkbench`,
`listWorkbenches`, `getWorkbench`, `activateWorkbench`, `activeWorkbench`,
`_workbench_catalogue`, `_set_gui_host`) and `ferrocad_shell` the two bridge entries
(`workbenches()`, `activate_workbench(json)`); the contract is
[`../../developer/workbenches/qt-strategy.md`](../../developer/workbenches/qt-strategy.md) §9.

Two decisions the port needed:

- **The lifecycle splits where the halves differ.** `Initialize` is Qt-free (commands plus
  `append*`) and runs once; `Activated` puts a dock on a main window, so it runs only when
  a host has adopted one — and `Deactivated` mirrors it, so a headless activation has
  nothing to undo. Without that split, SimpleCAD's `Activated` (a Qt controller, three
  timers and a Coin display) would fail on every load.
- **`addCommand` takes groups.** SimpleCAD registers `SmartSelectGroup`, which implements
  `GetCommands` and no `Activated`; upstream renders it as a toolbar dropdown. Our
  validation required `Activated`, so the whole workbench registration aborted. A group is
  now accepted, listed (`is_group`, `commands`) and refused by name if run.

**The checkout loads.** With `../simplecad` on the path, `import InitGui` registers
`SimpleCADWorkbench`, and activating it runs `commands.register()` (53 ids) then
`Initialize`'s appends:

| Declared | Count |
| --- | ---: |
| toolbars | **5** (`SimpleCAD Draw` 8 slots, `Modify` 20, `Surface` 8, `Inspect` 4, `Mesh` 4) |
| menus | **6** (one per group plus the `Separator`/`Settings`/`MCP` root row) |
| command ids the workbench lists | **49** (deduped across rows; `Separator` excluded) |
| command ids registered in total | **101** (the 49 plus the shell's own) |

Every row's ids are registered and in the `SimpleCAD_` namespace, and the `Inspect`
toolbar's `SimpleCAD_SmartSelect` is reported as a group.

This is the **declaration** half of route (a) — it is the part that is genuinely portable.
The next slice (UI-3) is the first native tool: `ferrocad_simplecad` registering our own
command ids and a two-click Rectangle on the GPUI stage.

## UI-6. UI-3, as measured

```sh
cargo test -p ferrocad_simplecad            # 3: the rectangle round trip
cargo run  -p ferrocad_simplecad -- --check-init   # ferrocad: init OK
```

The edition exists as a crate in the Sculpt/Parametric mould: `ferrocad_combined_py`'s
built-ins registered before the interpreter starts, `ferrocad::edition_args()` for the
flags, `--check-init` / `--render` / the window each on their own branch, and
`run_edition_with_script("FerroCAD: SimpleCAD", "ferrocad_simplecad", …)`. Three pieces
are worth naming:

**The contributions are Parametric's plus one tool.** `contributions()` is
`ferrocad::base_contributions()` + the two Part manipulators (`fillet_manipulator`,
`push_pull_manipulator`) + the two polyline tools (`polyline`, `sketch_polyline`) +
`rectangle`. The first four are not decoration: the app script re-exports
`ferrocad_parametric`, so the edition's command set **is** the Part one, and a command
whose `{"tool": id}` the host cannot resolve reports `no such tool <id>` the moment it is
clicked. Registering the ids is what makes the inherited surface honest, and
`cargo xtask editions --check` now *requires* `ferrocad_part_gui` in this edition's
closure so the push/pull stem cannot silently disappear. `TaggedPolyline` (the polyline
tool under a workbench-specific manipulator id) moved from
`ferrocad_parametric/src/main.rs` into `ferrocad_stage`, so the two editions share one
definition of it instead of two copies.

**`Tool::completed()` is the new lifecycle seam.** A two-click rectangle needs no
`Enter`; the host should finish it on the press that completed it. `Tool` gained a
defaulted `completed() -> bool` (default `false`, so a polyline is unchanged), and the
viewport's `dispatch_tool` calls `finish_tool()` when it turns true — which is the same
path `Enter` takes, so there is one commit route rather than a second one. The pointer-up
that follows the press emits the commit.

**The script names the object, the tool collects the geometry.** The hook turns the
committed numbers into one `Part::Feature` in one named transaction, through
`simplecad.core.geometry.rectangle` when the package is importable and
`Part.Face(Part.makePolygon(…))` when it is not — the round trip asserts both build the
same face (area 40, bbox 10 × 4). The provider is registered as `_interaction`, not
`interaction`: `interaction` is the *shell* entry point the Python host calls by name,
and shadowing it hands the hook a JSON string where the host expects a list.

| Check | Result |
| --- | --- |
| `ferrocad_stage --lib` | **268** (4 rectangle cases + the tag case) |
| `ferrocad_viewport` | **235 / 0 / 1 ignored** (the auto-finish is on the existing dispatch path) |
| `ferrocad_simplecad` | **3**: the two-click chain through the real registry, a degenerate click, a vertical drag |
| `ferrocad_parametric` | all suites green (the `TaggedPolyline` move) |
| `run_tests.py --extension base` | **718 / 0 / 0 / 8** |
| `run_tests.py --extension workbenches` | **881 / 0 / 0 / 3** |
| `conformance.py --simplecad-gui` | **78 / 1 / 0 / 0** (unchanged; the 1 is UI-4's classified `BoundBox` case) |
| `cargo xtask editions --check` | green over **14 editions** (`ferrocad_simplecad`: 35 crates) |

**Not done, deliberately.** Packaging is not wired (`xtask bundle` still knows
`parametric` and `sculpt`; no `packaging/editions/simplecad.json`, no release-matrix
row), and the rest of the tool surface — Round, Offset, Move/Rotate, the surface
tools — is UI-4. Line and Push/Pull already work here because they are the Parametric
edition's tools, not new ones.

## UI-7. UI-4's first slice, as measured

The three round drawing tools are in, with upstream's click sequences (`controller.py`'s
own hints: "Click circle center · Tab for radius", "Click center, then start, then arc
end"):

| Tool | Clicks | Commit (numbers) | Object |
| --- | --- | --- | --- |
| `SimpleCAD_Circle` | centre, rim | centre + rim + normal (9) | a `Part::Feature` face |
| `SimpleCAD_Polygon` | centre, vertex | + side count (10) | a face |
| `SimpleCAD_Arc` | centre, start, end | start + halfway + end (9) | a `Part::Feature` **edge** |

The 9/10 numbers are points and a normal rather than a radius, because the script must not
re-derive the plane from a rule: the tool saw the plane, so the tool states it.

**One shared rule for the plane.** `interaction/frame.rs` now owns what the rectangle used
to inline: `drag_axes` (the axis least aligned with the drag is the normal) and
`DragFrame` (centre, radius, `u` along the drag, `v` square to it, plus `ring()`,
`polygon(sides)` and `arc(sweep)` sampling). `rectangle_corners` calls `drag_axes`, so
"which plane did the user draw in" has one definition. The frame keeps the normal it was
built from, so a ground-plane circle commits exactly `(0, 0, 1)` instead of
`1.0000000000000002`.

**The arc's sweep accumulates.** `atan2` alone folds a drag back at ±180°, so a user who
swept three quarters of a turn would get the quarter they did not draw. The tool wraps each
step to `(-π, π]` and adds it, clamped just under a full turn (upstream refuses exactly
360°). The commit is **three points on the arc**, which is exactly what upstream's
`Arc.edge()` hands the kernel (`Part.Arc(p0, p1, p2).toShape()`), so the script needs no
plane arithmetic at all.

**The side count is the user's, within what a tool can offer.** Six by default (upstream
`polygon_sides = 6`); a digit `3`–`9` while the tool is armed changes it and rebuilds the
preview. Upstream uses a spinbox — that is UI-5's numeric rows, not this slice. The count
travels in the commit, so two polygons in one document can differ.

**The script is a table, not four branches.** `_BUILDERS` maps a tool id to `(object name,
builder, arity)`, `_commit` owns the transaction/`_invalid`/appearance shape, and
`_interaction` is now `action != create → None; no entry → None; else commit`. The circle
and the polygon use `simplecad.core.geometry` when importable and the equivalent `Part`
construction when not — the round trip checks both paths build the same area.

| Check | Result |
| --- | --- |
| `ferrocad_stage --lib` | **287** (+19: 5 `DragFrame`, 4 circle, 5 polygon, 5 arc) |
| `ferrocad_simplecad` | **6** round trips: rectangle, circle, polygon (6 and 4 sides), arc, degenerate clicks, vertical rectangle |
| `--check-init` | draws all four and asserts them: rectangle 40 mm², circle 25π, hexagon 300·sin 60°, arc `5π` long and a `Part.Circle` |
| `ferrocad_viewport`, `ferrocad_part_gui` | **235 / 0 / 1 ignored**, **41 / 0** (unchanged) |
| `cargo xtask editions --check` | green over 14 editions (no dependency change) |

**Still open in UI-4.** Round (the dress-up scrubber is generic already, so it is a
command with the right `selection` filter), Offset/Elongate, Move/Rotate, and the surface
tools. Face **partitioning** is also untouched: upstream's `draw_profile`/`draw_edge` divide
a host face or imprint an edge onto a solid, while this edition always writes a standalone
`Part::Feature` — the same simplification UI-3 made for the rectangle, and the reason a
drawn circle on a box's face is a new object rather than a hole in it.
