# Sculpt

A quad cage in, an editable solid out, nested in a `PartDesign::Body`.

```python
import FreeCAD, PartDesign, Sculpt

doc = FreeCAD.newDocument("SculptDemo")
points, faces = Sculpt.box_cage(40, 30, 20)

cage = Sculpt.makeCage(doc, "Cage", points, faces, levels=2)   # mesh preview
skin = Sculpt.makeSkin(doc, "Skin", cage)                      # B-rep solid

body = doc.addObject("PartDesign::Body", "Body")
Sculpt.nest(body, cage, skin)      # -> ["support", "base"]
doc.recompute()
```

## The objects

| Object | Type | What it holds |
| --- | --- | --- |
| `cage` | `Mesh::FeaturePython` | `CagePoints` + `CageFaces` (four indices per quad), `Levels`; the Catmull-Clark preview is its `Mesh` |
| `skin` | `Part::FeaturePython` | `Cage` link, `Levels` (`0` follows the cage), `Tolerance`; the sewn patch solid is its `Shape` |

Editing `cage.CagePoints` re-runs both objects on the next `doc.recompute()`: the
preview rebuilds as a mesh and the solid is re-sewn.

In a body the cage is a **support** member (it is not the body's shape) and the
skin becomes the **BaseFeature**. A later PartDesign feature takes the `Tip`.

## The knobs

- `Levels` — Catmull-Clark refinements. Each level moves the surface toward the
  limit surface and quadruples the patch count; 2 is a good editing default and
  3 is close to converged on a box cage.
- `Tolerance` — the sewing tolerance of the patch shell (mm).

## Faces and persistence

`CageFaces` is a flat integer list, four indices per face, and any polygon is
accepted (a refinement always returns quads). An open cage previews fine but the
skin refuses to build a solid from it — the object reports
`Invalid: … the cage is not closed`.

A document saved as `.ferrocad` restores both proxies. A `.FCStd` keeps the
properties and the shapes but not the Python proxy, so call
`Sculpt.attach(doc)` after opening one; it returns the names it re-attached.

The design note is
[`docs/developer/workbenches/sculpt.md`](../../docs/developer/workbenches/sculpt.md).
