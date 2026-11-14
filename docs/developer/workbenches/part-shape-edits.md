# The sub-shape edit seam

Status: design note (2026-11-14).

`Shape.replaceShape`, `Shape.removeShape`, `Shape.generalFuse` and the erase/split code
built on them (`ferrocad_part_py`, and SimpleCAD's `geometry.erase_elements` /
`divide_face_with_edges`) all hand the kernel a set of **sub-shapes** and ask for a new
shape. That works only if the sub-shapes you hand back are the ones the shape is built
from. This note records the three seams where that was not true, because each of them
looked like a modelling bug and was not.

## `Shape.generalFuse` answers every input from one build

`Shape.generalFuse(shapes, tolerance)` returns `(result, parts)` where `parts` has one
entry per input — the pieces that input was split into. Upstream gets them from
`BRepAlgoAPI_BuilderAlgo::Modified(input)` on the builder that produced the result.

The seam used to rebuild the whole general fuse **once per input**, so each entry came
from a different boolean. Every input therefore got its own independent copy of the
result's sub-shapes: an edge image and the vertex image at its end were two different
`TopoDS_Vertex`es at the same point. A caller that swaps those images into a face — the
local face split — then built a face with four vertices for three edges, `BRepCheck`
called it invalid, and every local split fell back to the whole-solid boolean.

Now one build answers every input (`shape_ops::general_fuse_parts`), and the images are
sub-shapes of the result that is returned. A regression test
(`a_general_fuse_reports_images_of_its_own_result`) asserts exactly that: every reported
piece is a sub-shape of the returned result.

Two consequences worth knowing:

- **Cost.** One boolean per call instead of `n + 1`.
- **The kernel omits vertex images.** `BRepTools_History` supports vertices, but the
  data structure records no `Modified` relation for a vertex the fuse replaced: the
  arguments' vertices are unified with (or duplicated beside) their neighbours without
  one. A vertex that is *gone* from the result is reported as the single result vertex at
  its point; a vertex that survived reports nothing, and so does an ambiguous match. That
  is measured, not invented — the same rule the refine history uses.

## `replaceShape`/`removeShape` are `BRepTools_ReShape`

Both are `ReShape::Replace`/`Remove` + `Apply(shape, TopAbs_SHAPE)`, exactly upstream's
`TopoShape::replaceShape`/`removeShape`. `ReShape` does **not** sew: it substitutes
sub-shapes and rebuilds the parents. A face rebuilt from a mix of images that do not share
vertices gets a coincident duplicate at every such seam, and the face is invalid. There is
nothing to fix in `replaceShape` — the fix belongs in whoever produced the images (above).

## `Edge.Orientation` and the cached entry

A `Part.Edge` carries two things: the base `PartShape` (`Part.Shape`'s storage) and the
`EdgeEntry` it was built from. `Part.Wire`, `Part.Face`, `replaceShape` and every rebuild
read the **entry**, so a flag written only to the base never reaches a new wire.

`Edge.Orientation = "Internal"` used to do exactly that: the edge *reported* `Internal`
and arrived in a new wire as `Forward`. An internal spur could then never be built, found
or erased, which is what `clean_solid_faces`/`erase_elements` need. `PyEdge` now overrides
`Orientation` and keeps the entry in step with the base
(`an_edge_orientation_reaches_the_shapes_that_consume_it`).

## `App::Part`'s extensions are type-static

Upstream instantiates `App::GeoFeatureGroupExtension` (which derives from
`App::GroupExtension`) in the constructor of `App::Part` and `PartDesign::Body`. A caller
that asks `hasExtension("App::GeoFeatureGroupExtension")` — SimpleCAD's `editable`, which
keeps a transformed `App::Part` from being moved as one shape — must get `True`. The answer
is computed from the type in `Document::has_extension` and is **not** written into the
object's extension set, so a file keeps carrying only the extensions a caller actually
added.

## What is still open in this seam

- **`erase_elements` closing a box that should open.** Deleting one side face of a box
  returns the closed solid: `close_holes` caps the loop (four neighbours, so not the
  "single neighbour" path) instead of declining. The expected behaviour is a five-face
  open shell.
- **A sub-tolerance step.** Pushing a face down `10 − 1.9e-6` mm is expected to leave a
  `1.9e-6`-thick step (a face with area `< 1e-3`); the cut drops it. The boolean's
  automatic fuzzy value (`bounds diagonal × Precision::Confusion()`, ≈ `7e-6` here) is
  larger than the step. Upstream computes the same fuzzy value, so this may be an OCCT
  version difference rather than a seam bug — check before "fixing" it.
- **`Shape.fuse`/`cut`/`common` ignore their `tolerance` argument.** The seam has no
  parameter for it; `cut` always applies the automatic fuzzy value and `fuse`/`common`
  apply none. Upstream passes `tol` through (`> 0` explicit, `< 0` automatic, `0` the
  kernel's own confusion tolerance).
