# Spike: OCCT shape history (topological naming)

A throwaway probe. It answers one question before we design a `GeometryEngine`
trait or a Lineage Engine:

> Does OCCT expose enough **shape history** for us to build a stable element map?

If the answer is yes, the lineage plan in
[`../../docs/geometry-and-topology.md`](../../docs/geometry-and-topology.md) §4–§6
stands. If the `Modified`/`Generated`/`Deleted` lists come back empty for real
operations, the plan changes: stable naming would have to be built some other way.

## The experiment

`probe.cpp` builds two overlapping boxes, fuses them, and fillets one edge. It then
prints, per input face and edge, what each operation reports through
`BRepBuilderAPI_MakeShape::Modified` / `Generated` / `IsDeleted`, plus the
`BRepTools_History` the boolean keeps, plus output element counts.

## Build and run

Requires OCCT **7.8 or newer** dev headers and libraries (FreeCAD's element mapper
gates on `OCC_VERSION_HEX >= 0x070800`; see the spike report).

```sh
# Debian/Ubuntu:
#   sudo apt-get install -y libocct-*-dev occt-misc   # 7.9.x on recent Ubuntu
#   (or conda: `conda install -c conda-forge occt`)

cmake -S . -B build
cmake --build build -j
./build/occ_history_probe
```

The probe is **untested in the authoring sandbox**: that environment has no OCCT,
no network, and `sudo` requires a password, so nothing could be compiled there.
Method names are written against OCCT 7.8/7.9; adjust if a signature differs.

## What to look for

- **Fuse**: coincident faces should be `Modified` (two inputs map to one output),
  the faces created at the intersection should be `Generated`, and a simple overlap
  should delete nothing. If all three are empty, the fuse is not reporting lineage.
- **Fillet**: the filleted edge should be `Deleted` or `Modified`, and the round
  should appear as a `Generated` face. This is the operation most likely to expose
  weak history.
- **`BRepTools_History`**: should agree with the binary accessors and additionally
  answer `IsRemoved`. This is what FreeCAD wraps as `MapperHistory`.

## Why this matters for the B slices

The probe is not only about geometry. `App::Link` (the links B-slice) carries a
placement, a scale, a transform flag, and can target sub-elements by name, and
containers aggregate placements recursively. So a "links/containers" slice that has
no notion of shape references is a partial `Link`. The history probe tells us how
much of that reference machinery must exist before B3/B5 are honest. Details in the
report: [`../../docs/occt-history-spike.md`](../../docs/occt-history-spike.md).
