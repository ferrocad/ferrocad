# Text and 3D text: what is kernel, what is Part, and how it is sized

Status: design note / decision (2026-10-30). Answers the questions raised after SC2
(`Part.makeWireString`) landed: can the toy edition have a text tool, is text OCCT, is a
wire or an extrusion Part, and what happened to DPI and points. The executable demo is
[`tests/test_text_workflow.py`](../../../tests/test_text_workflow.py).

Related: [`simplecad-bringup.md`](simplecad-bringup.md) SC2 (the font pipeline),
[`creation-tools-ownership.md`](creation-tools-ownership.md) (who owns a creation tool),
[`../TODO-30-10-2026.md`](../TODO-30-10-2026.md) P4 SK4.

## 1. What is kernel, and what is not

| Layer | What it is | Where | Needs OCCT? |
| --- | --- | --- | --- |
| Font parsing and outlines | glyph ids, advances, kerning, contours of move/line/quad/cubic | `ferrocad_text` | **no** |
| Wires | closed loops of edges | `Part.Wire`, `backend.make_wire` | yes |
| Faces | planar faces with counters (holes) | `Part.Face(wires, "Part::FaceMakerBullseye")` | yes |
| Extrusion | `Shape.extrude(Vector)` / `Part::Extrusion` | `ferrocad_part_py`, `ferrocad_occt` | yes |
| **ShapeString object** | a `Part::Part2DObjectPython` whose proxy rebuilds the text on every recompute | `python/Draft/__init__.py` | yes |

**Text is not an OCCT concept.** A string is a set of 2-D contours, which is a font problem;
`ferrocad_text` produces them with `skrifa`/`read-fonts` and no kernel. The moment a contour
must become geometry, it crosses into Part — and only the *binding*
(`crates/ferrocad_part_py/src/text.rs`) does that, by mapping contours to edges and wires.

## 2. Can the toy edition have a text tool? Should we add `ferrocad_parametric_toy`?

**The toy can preview text, and cannot make 3D text.** `crates/ferrocad_toy/Cargo.toml`
links `ferrocad`, `ferrocad_gpui`, `ferrocad_stage`, `ferrocad_geom`, `ferrocad_graphics`
and `ferrocad_types` — no `ferrocad_part`, no OCCT. Its objects are `App::FeaturePython`
with float lists and its slab is a mesh, so `Part.makeWireString`, `Part.Wire`,
`Part.Face` and `extrude` do not exist there. A toy text tool could compute contours with
`ferrocad_text` and draw them as 2-D polylines; it could not produce a wire, a face or a
solid.

**No `ferrocad_parametric_toy`.** `ferrocad_parametric` *is* the Part edition; an edition is
a payload plus a `HostConfig` over a shared binary
([`../developer/architecture/edition-publishing.md`](../../developer/architecture/edition-publishing.md)),
not a new Rust composition. A kernel-free text *preview* is a tool in the toy; 3D text is a
command in the Parametric edition (or a script). A third crate would duplicate the shell for
no new capability.

## 3. A wire is not enough: wire → shell, face → solid

This is the one thing the demo exists to show. Measured on the fixture:

| Input | `extrude(Vector(0, 0, 2))` | Result |
| --- | --- | --- |
| the outer wire of `o` | `Shell` | open, `isClosed() == False`, `Solids == 0` |
| the counter (hole) wire of `o` | `Shell` | open, **negative** volume |
| `Part.Face([outer, counter], "Part::FaceMakerBullseye")` | `Solid` | closed, volume `area × 2` |

So the workflow is `makeWireString → Face(Bullseye) → extrude → fuse/cut`, never
`makeWireString → extrude`. Bullseye nests the contours: a wire inside a face is a hole, one
inside a hole starts a new face, and disconnected regions come back as a compound.

## 4. Units, DPI and points

**There is no DPI, and no point size, anywhere in the geometry path.** DPI is a raster
concept: it converts a length to pixels. B-Rep text has no pixels.

- `Part.makeWireString(text, dir, file, height)` takes `height` as a **length in the
  document's units** (millimetres here).
- The scale is upstream's: `mm = font_unit × height / (10 × line_height)`, where
  `line_height` is the font's ascender − descender + line gap in font units. FreeType's
  fixed 480 pt em and the units-per-em cancel, which is why the result has no
  pixels-per-inch term at all.
- **`height` is ten times the drawn line height.** A glyph's line box maps to `height / 10`,
  so a 100 mm `height` draws a 10 mm line (≈ 5.8 mm cap height in the fixture). That `/ 10`
  is upstream's and is reproduced deliberately; whether it is upstream's bug is SC2's open
  question.
- A point is `1/72 in` only once a physical device exists. If a UI wants to say "12 pt", the
  UI converts (`1 pt = 0.3528 mm`) and passes the millimetres. **Do not bake 72 or 96 into
  the crate.**

## 5. Shaping and paragraphs

- **Shaping is not implemented, deliberately.** We map characters through the font's cmap
  and place them with advances plus the legacy `kern` table — exactly what `FT_Get_Kerning`
  does. That is enough for Latin text. It is *not* enough for ligatures, GPOS kerning,
  bidi or complex scripts.
- `harfrust` 0.5.2 is already in the lock (via `cosmic-text`). Replacing the advance/kerning
  step with a `harfrust` shaping pass would not change `ferrocad_text`'s public types
  (`GlyphOutline` already carries a glyph id, an origin and an advance), so it is a later,
  contained change.
- **Paragraph layout is a separate layer.** Line breaking, wrapping, alignment, bidi runs and
  multi-line placement belong above `ferrocad_text` and would emit one `TextOutline` per
  line. `parley` is not in the lock and is not added for this.

## 6. The minimal demo

**The object form is `Draft.make_shape_string`.** Upstream's Draft ShapeString was
checked before writing anything, and it was not implemented here: the tree had the
`TaskShapeString.ui` **form** (loaded and tested by the Qt-reduction work) but no
`make_shape_string`, no object type and no command. It now exists as a scripted
`Part::Part2DObjectPython` proxy in `python/Draft/__init__.py` (the Draft shim):

```python
doc = App.newDocument()
obj = Draft.make_shape_string("ferrocad", "/path/to/font.ttf", 20, 1.0)
obj.Shape                       # compound of 8 faces (MakeFace defaults to True)
obj.Shape.Faces[0].extrude(App.Vector(0, 0, 2)).ShapeType   # "Solid"
obj.String = "AB"; doc.recompute()                          # rebuilds
```

Properties implemented: `String`, `FontFile`, `Size`, `Tracking`, `MakeFace`,
`ScaleToSize`, `Justification`, `JustificationReference`, `KeepLeftMargin`,
`ObliqueAngle`. `make_shapestring`/`makeShapeString` are the deprecated aliases.
The proxy survives a native `.ferrocad` save/reopen (the restore path finds
`Draft.ShapeString` and re-runs `execute` on the next edit).

The low-level workflow is still worth knowing, and `tests/test_text_workflow.py` runs
both. On the word **`ferrocad`**:

1. `Part.makeWireString("ferrocad", dir, "LiberationMono-Regular.ttf", 100.0)` → 8
   lists of wires; contour counts are `[1, 2, 1, 1, 2, 1, 2, 2]` (`e`, `o`, `a`, `d`
   have counters).
2. `Part.Face(wires, "Part::FaceMakerBullseye")` → one planar face per glyph.
3. `face.extrude(Vector(0, 0, 2))` → 8 closed, valid solids.
4. **Raised:** `plate.fuse(text)` adds exactly `text.Volume`.
5. **Etched:** `plate.cut(text)` placed 1 mm into the plate removes exactly half the
   text volume (2 mm deep, 1 mm overlapping).

### 6.1 From a `ferrocad_parametric` script

The object form works in the edition's `--script` path unchanged: the script runs
after bootstrap with `python/` on `sys.path`, `import Draft` resolves, and the
proxy's `execute` runs through the ordinary recompute.

```sh
cargo run -p ferrocad_parametric -- \
  --script examples/headless_text.py --stl target/demo/text.stl --size 1200x900
# ferrocad: 1 object(s) [Text3D] from Text3D
# ferrocad: wrote STL target/demo/text.stl (2146 triangles)
```

[`examples/headless_text.py`](../../../examples/headless_text.py) is the minimal
script: `Draft.make_shape_string`, extrude the faces, fuse, raise on a plate, keep
one `Part::Feature`. It is the same wire→face→solid rule as above; a bare wire
would extrude to a shell.

One host fix came out of running it: the headless report used to name the
*bootstrap* document, so a script that called `newDocument` printed
"from Shell" while writing to its own document. The report now asks for the
active document (`ferrocad_shell.active_document`), which is also the document
`stage_meshes` reads.

### 6.2 Two traps the demo pins

Two traps the test pins:

- `Shape.translate` **mutates in place** (upstream semantics), so the two placements must
  `copy()` first; otherwise the second boolean operates on text already moved to z = 3.
- Round letters overshoot below the baseline (`o`, `e`, `c` start at y ≈ −0.11 at this
  size), so the plate placement shifts by the bounding box, not by the baseline.

## 7. Open questions

1. **`/ 10`**: confirm upstream's own `YMax("M")` at a nominal height; if it is 10× off, fix
   it as its own change with SimpleCAD in the loop. (`Draft.make_shape_string` sidesteps it
   with `ScaleToSize`, which is why SimpleCAD — which does not rescale — is the place to
   see it.)
2. **`Part.makeWireString` has two call forms**: `(text, dir, fontfile, height[, tracking])`
   and `(text, fontspec, height[, tracking])`. Both are implemented; Draft uses the second,
   and a script that passes a full path in the first form would have failed before this was
   added.
3. **ShapeString gaps**, deliberate for now: `Fuse` (needs Draft's
   `faces.concatenate`) and the sticky-font probe that disables `MakeFace` for fonts whose
   contours cannot be filled. Neither is declared as a property, so a script that sets them
   fails loudly rather than silently doing nothing.
4. **A GUI command**: `TaskShapeString.ui` is already loaded by the shell's form host; the
   command that would drive it (`Draft_ShapeString`) is not registered.
5. **`.FCStd` persistence**: the object's proxy survives a native `.ferrocad` save, but the
   `.FCStd` writer drops proxy state (a known limitation), so a reopened `.FCStd` keeps the
   last `Shape` but will not rebuild on edit.
6. **Shaping**, if a non-Latin or ligature requirement appears.
7. **Paragraph layout**, only when a real multi-line use case exists.
8. **`ferrocad_parametric_toy`**: rejected here; reopen only if a kernel-free edition needs
   editable 2-D text, which would be a toy tool, not a new edition.
