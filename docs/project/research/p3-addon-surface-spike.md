# P3 host spike: the Curves and SubD call surface

Status: research (2026-11-16). The evidence behind P3's first deliverable: whether
FerroCAD can host the freeform workbenches through the script facade, and what a
bundle actually calls. The design argument is
[`../ideation/surface-and-freeform-modelling.md`](../ideation/surface-and-freeform-modelling.md)
§7; the implementation notes are in
[`../../developer/workbenches/part-shape-edits.md`](../../developer/workbenches/part-shape-edits.md).

## What was missing

The ideation note identified one document type as the blocker for the closest
worked answer to a freeform skin (SubD Modeler, MIT, unindexed): it stores its
cage on a `Mesh::FeaturePython`. FerroCAD registered `Mesh::Feature` but not the
Python-feature variant, so `doc.addObject("Mesh::FeaturePython", …)` raised.

That type now exists (`crates/ferrocad_mesh/src/mesh_property.rs`):

- the same `Mesh` + `Placement` pair as `Mesh::Feature`, including the
  `MeshFeature::onChanged` mirror between them;
- `execute` records a request (`Document::mark_python_pending`), and the binding
  runs `proxy.execute(obj)` with the document lock released — the Part
  Python-feature path, unchanged;
- no `Shape`: a cage is geometry the proxy owns.

## The synthesized workload

Neither add-on is in this checkout, so the spike **synthesizes** the calls their
object model documents and runs them headlessly against one `ferrocad` + `Part` +
`Mesh` image (`tests/test_p3_addon_surface.py`). Every call is upstream API, so a
failure is a named gap rather than a missing feature of the test.

| Step | Calls | Result |
| --- | --- | --- |
| Cage | `doc.addObject("Mesh::FeaturePython", …)`, `addProperty("App::PropertyVectorList", "CagePoints")`, `addProperty("App::PropertyIntegerList", "CageFaces")`, `Proxy.execute` assigns `obj.Mesh = Mesh.Mesh((points, facets))` | **answered**; the preview follows a cage edit |
| Patch | `Part.BSplineSurface()`, `buildFromPolesMultsKnots(poles, [4,4], [4,4], udegree=3, vdegree=3)`, `toShape()` | **answered**; a valid `Face` with a `BSplineSurface`, area exact |
| Skin | `Part.Shell(patches)`, `shell.sewShape(1e-3)`, `Part.Solid(shell)` | **answered**; 6 patches → 12 edges, 0 free, a closed valid solid of volume 1.0 |
| Wrapper | `Part::FeaturePython` + `App::PropertyLink`, `Samples`, `Tolerance`; `execute` rebuilds `Shape` and follows the cage's `Placement` | **answered** |
| Curves | `Part.BSplineCurve()`, `interpolate(points, PeriodicFlag=True)`, `toShape()`, `Part.Wire`, `Part.makeFilledFace(edges)` | **answered** |

The geometry calls were already present (`Part.BSplineSurface.buildFromPolesMultsKnots`,
`Part.Shell`, `Part.Solid`, `Shape.sewShape`, `Part.makeFilledFace`), which is why
the ideation note called the cost "a document type, not geometry". The measurement
confirms it: **the only missing piece was the document type**.

## Findings worth carrying forward

- **A mesh write resets `Placement`.** `Mesh::FeaturePython` mirrors like
  `Mesh::Feature` (upstream `MeshFeature::onChanged`), so assigning a fresh
  `Mesh.Mesh` adopts *its* transform into `Placement`. A cage proxy that rebuilds
  the preview must copy the object's `Placement` onto the new mesh first, or the
  cage jumps to the origin on every edit. The spike does that and asserts it.
- **`Mesh.Mesh(points, facets)` is not accepted; `Mesh.Mesh((points, facets))` is.**
  SimpleCAD uses the tuple form, so this has not mattered before. The two-argument
  form is a candidate API addition, not a blocker for these workbenches.
- **`Shape.connectEdgesToWires(shared=False, tolerance=1e-9)`** was missing and is
  now implemented (`ShapeAnalysis_FreeBounds::ConnectEdgesToWires`, returning a
  compound of wires). SimpleCAD's coplanar-face merge falls back to it; it is also
  the natural boundary-rebuild call for a bundle that sews patches.
- **`Part.makeSolid`/`Part.Solid` add shells as given.** Upstream's
  `makeElementSolid` is `BRepBuilderAPI_MakeSolid::Add` per shell plus
  `BRepLib::OrientClosedSolid`; it does not reverse cavity shells, because a shell
  taken from a cut already carries that orientation. FerroCAD reversed them, which
  turned a cavity into a second skin. The spike and the two SimpleCAD errors below
  are why this is now upstream-faithful.

## What this leaves for P3

- **Curves and SubD sources are still absent**, so their proprietary call lists
  remain unverified. The spike covers the document object, the patch/skin assembly
  and the wrapper; a pinned checkout of either add-on is the next step, not more
  synthesis.
- **`PartGui` is not published** (a GUI-session gap, not a headless one).
- **Native cage editing** (section 5 of the ideation note) is still viewport work,
  and the parametric surface-to-solid feature in the ideation note's section 6 is
  now proven as a *scripted* object; promoting it to a native feature is optional.

## Measured alongside

Two SimpleCAD errors closed while touching `ferrocad_mesh` and the document
wrapper:

- `test_forming`'s stretched cavity: the shells of a cut now rebuild the cavity
  (`make_hollow_solid` as given), so the 17-test module is green.
- `test_subdivision`'s stale operation: `DocumentObject.isValid()` on a removed
  object is `False` (upstream's answer), so SimpleCAD's guard short-circuits before
  reading `.Name`.

The integrated suite is **1038 passed / 6 failed / 3 errored / 7 skipped over
88/88 modules**. The three errors left are `Mesh.decimate` (needs a decimator),
`test_loop_faces`'s twisted-quad merge (the reference's merge accepts a candidate
on area and normal without checking validity, so it returns an invalid face; the
kernel calls are identical), and the chamfered face-order assumption.
