# Sculpt: a cage, a skin and a body

**Sculpt** is a bundle, not a second kernel. It models a free-form body with three
ordinary document objects.

| Object | Type | What it is | Body role |
| --- | --- | --- | --- |
| **cage** | `Mesh::FeaturePython` | a control hull (`CagePoints`, `CageFaces`) and its subdivision preview | **support** |
| **skin** | `Part::FeaturePython` | the sewn patch solid built from the cage | **base** (`BaseFeature`) |
| **body** | `PartDesign::Body` | the container the later features build in | — |

The cage is a support, not the body's shape. `Mesh::FeaturePython` declares that
capability, so the cage can sit in the body group without taking the one base slot; the
skin is the B-rep the body builds on. The rules are in
[Part shape edits](../../developer/workbenches/part-shape-edits.md)
("`PartDesign::Body` membership is declared, not listed").

## The document recipe

```python
import FreeCAD, Sculpt

doc = FreeCAD.newDocument("SculptDemo")
points, faces = Sculpt.box_cage(40, 30, 20)
cage = Sculpt.makeCage(doc, "Cage", points, faces, levels=2)
skin = Sculpt.makeSkin(doc, "Skin", cage)

body = doc.addObject("PartDesign::Body", "Body")
roles = Sculpt.nest(body, cage, skin)   # ["support", "base"]
doc.recompute()
```

`box_cage` is the smallest closed cage and the starting point for a real one;
`makeCage` adds the control points, the faces and the preview; `makeSkin` links the
cage and builds the sewn solid. The skin's `Levels = 0` follows the cage's own
resolution, so the preview and the solid agree; a positive value gives the skin its own.
A cage edit re-runs the skin because the skin holds the cage as an `App::PropertyLink`,
and moving the cage's `Placement` moves both.

The edition's six commands are the same operations:

| Command | What it does |
| --- | --- |
| `Sculpt_MakeCage` | add a 40 × 30 × 20 box cage with its preview |
| `Sculpt_MakeSkin` | sew the selected cage (or the document's only one) into a solid |
| `Sculpt_NestInBody` | nest the selected skin and its cage, or all of them, in a body |
| `Sculpt_LevelUp` / `Sculpt_LevelDown` | one refinement step, levels 0…4 |
| `Sculpt_AttachProxies` | re-attach the proxies after a `.FCStd` load |

## The subdivision

`Sculpt.catmull_clark(points, faces)` is one Catmull-Clark refinement; `Sculpt.subdivide`
repeats it. The boundary rules matter because a cage is open while it is being built:

- a face point is the centroid of its face;
- an interior edge point averages its endpoints and the two adjacent face points; a
  boundary edge point is its midpoint;
- an interior vertex moves to `(F + 2R + (n − 3)P) / n`;
- a boundary vertex with one incident face (the corner of an open patch), or with a
  boundary fan that is not one chain, is a corner and stays put;
- any other boundary vertex uses the `(P_prev + 6P + P_next) / 8` curve rule.

The corner rule is what keeps the rim of an open cage on the original outline instead of
shrinking a single quad to three quarters of itself. An edge used by more than two faces
is treated as a boundary, so a non-manifold cage refines instead of failing.

The output point order is part of the contract because the preview and the patch builder
index it: the original points first (moved), then one point per face in face order, then
one per edge in first-seen order.

## The patches, and how close they are

`Sculpt.bicubic_patch(corners)` builds a quad's bilinear patch on a 4 × 4 pole grid. Two
neighbours share their boundary straight line exactly, so the sewn shell has no gaps and
is **G0 by construction**. The smoothness comes from the refinement level, not from the
stitch: this is not a Catmull-Clark limit patch, and the level knob is the accuracy dial.

For the 40 × 30 × 20 box cage:

| Levels | Preview mesh volume | Skin volume | Faces |
| --- | --- | --- | --- |
| 0 (cage) | 24 000 mm³ | — | — |
| 1 | 10 000.0 | 10 250.0 | 24 |
| 2 | 8 402.8 | 8 404.6 | 96 |
| 3 | 7 993.9 | 7 993.94 | 384 |

The two representations converge on the same limit surface; at level 3 they agree to
0.07 mm³ of 7 994. A level is one undo step, and the preview and the B-rep are rebuilt
together.

`Sculpt.patch_solid` raises `ValueError` when the sewn shell is not closed: an open cage
is a surface, and returning a shell would let a body adopt a shape it cannot use. The
object records the normal proxy failure, `Invalid: proxy.execute failed: the cage is not
closed: …`.

## Moving a control point

The cage's one interaction is a **control-point grip**:

- the display hook publishes the subdivision preview as the surface, the control hull as
  edges, and every control point as a stage vertex named `Point0`, `Point1`, …;
- `crates/ferrocad_sculpt` declares one octahedron handle per `PointN`; a drag moves the
  grabbed point in the plane through the press point that faces the camera, and its typed
  twin is X/Y/Z;
- the commit is `cage_point` / `move` with the world-space `[x, y, z]`;
  `python/Sculpt/interaction.py` inverts the cage's `Placement`, writes
  `CagePoints[index]`, recomputes and commits one named transaction. The skin is ordered
  after the cage by its link, so the preview and the sewn solid follow in the same
  recompute and one drag is one undo step.

A cage point has no `VertexN` name: it is not on the resulting shape, because the
subdivision moves it. The published `PointN` part is what gives the drag a stable name
across a rebuild.

The grip arms from a selection on the cage's item that publishes a `Point*` part — a face
pick is enough.

## Persistence

Native `.ferrocad` documents keep each proxy by module and class name, so reopening one
rebuilds the cage and the skin through `onDocumentRestored`.

A `.FCStd` keeps the properties and the shapes but not the proxy: upstream stores a binary
Python object there, which FerroCAD reports rather than writing an incompatible payload.
`Sculpt.attach(document)` installs the proxies on the objects that need them — the same
call an add-on makes after opening its own file — and returns the names it re-attached.

## What it does not do

- **No GUI beyond the six `Sculpt_*` commands**, and no workbench switcher entry: the
  shell's Search palette is the surface that lists them.
- **No cage edge or face handles.** The grip moves one control point; extruding a cage face
  or sliding a hull edge needs a different commit shape.
- **No limit-surface patches** (G1/G2 continuity), no n-gon output (any polygon is accepted
  as input and comes back as quads), and no vendored `Curves`/`SubD` add-ons.
- **No general surface modeler.** It is a cage → subdivision → sewn solid workflow, not a
  replacement for the Surface workbench.

## Where the detail lives

- [The Sculpt bundle](../../developer/workbenches/sculpt.md) is the stable design note:
  the body roles, the subdivision rules, the patch construction and the measurements.
- [Surface and free-form modelling](../../project/ideation/surface-and-freeform-modelling.md)
  is the argument for a bundle rather than a native workbench.
- [Edition publishing](../../developer/architecture/edition-publishing.md) and
  [Distribution](../../developer/architecture/distribution.md) cover the `ferrocad_sculpt`
  binary and the `--edition sculpt` payload.

## Evidence

`tests/test_sculpt_workflow.py` has **29** cases over the rules, the objects, the solid,
the body, the display row, the interaction verb and persistence. `crates/ferrocad_sculpt`
adds **6** unit tests for the grip and **3** composition tests that boot the edition's
script through the host. `cargo xtask editions --check` is green over **14 editions**,
with `ferrocad_sculpt` at 34 crates.
