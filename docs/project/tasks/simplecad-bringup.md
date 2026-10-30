# SimpleCAD bring-up: the closing cut

Status: task spec (2026-10-30). Cut from [`../TODO-30-10-2026.md`](../TODO-30-10-2026.md)
§P1 after the [fan-out](../record/2026-10-30-fan-out-todo-30.md). The measurement it
starts from is [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7.3;
the round's closed backdrop is
[`../record/2026-10-29-workplane-visual-hierarchy-and-grid.md`](../record/2026-10-29-workplane-visual-hierarchy-and-grid.md).

The goal is one scripted session that **draws on a face, extrudes, push/pulls, cuts a hole
and exports** without hitting a missing member, plus a tree that is green enough for the
remaining failures to be named rather than counted. Current state: **985 passed / 49
failed / 23 errored / 2 skipped** over 88/88 modules (2026-10-30, after SC0 and SC1; the cut
started from 969 / 64 / 26 / 2), and SC2 has closed the last missing `Part.*` name.

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

Measured: Part extension set **743 / 3** at SC2 (was 738 / 3), now **750 / 3** with the 3-D
text workflow ([`tests/test_text_workflow.py`](../../../tests/test_text_workflow.py)); base
unchanged at **687 / 8**; SimpleCAD **985 / 49 / 23 / 2** (unchanged — the suite does not
exercise the text tool).

**Still open:** whether upstream's `/ 10` is a bug (the scale is reproduced as-is, so
SimpleCAD gets the same size it would from upstream); `.ttc` indices and Type 1 `.pfb`
(the fixture is a plain `.ttf`); and `Draft`'s own `ShapeString` rescale, which is not part
of this slice.

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
