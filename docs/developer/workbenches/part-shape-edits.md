# The sub-shape edit seam

Status: design note (2026-11-14).

`Shape.replaceShape`, `Shape.removeShape`, `Shape.generalFuse`, `Shape.fuse`/`cut`/`common`
and the erase/split code built on them (`ferrocad_part_py`, and SimpleCAD's
`geometry.erase_elements` / `divide_face_with_edges`) all hand the kernel a set of
**sub-shapes** and ask for a new shape. That works only if the sub-shapes you hand back are
the ones the shape is built from, with the tolerance the caller asked for. This note
records the seams where that was not true, because each of them looked like a modelling
bug and was not.

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

## The boolean tolerance contract

`fuse`, `cut` and `common` take a `tolerance` and hand it to the kernel the way upstream's
`TopoShape::makeElementBoolean` does:

| `tolerance` | Kernel |
| --- | --- |
| `0` | nothing is set; the kernel's own confusion tolerance applies |
| `> 0` | `SetFuzzyValue(tolerance)` |
| `AUTO_FUZZY` (`-1`) | the operands' bounding-box diagonal × `Precision::Confusion()` |

The **document** booleans and `Shape.fuse`/`cut`/`common` therefore ask for different
things, because upstream builds them differently: `Part::Fuse`/`Cut`/`Common` go through
the two-shape `FCBRepAlgoAPI_*` constructors, whose constructor computes the automatic
value, so `containers.rs` and the extrusion's inner cut pass `AUTO_FUZZY`; `Shape.fuse`
(`tol = 0` by default) sets nothing. Before this, `Shape.fuse`/`cut`/`common` dropped the
argument entirely, and `cut` auto-fuzzed unconditionally while `fuse`/`common` did not —
three different behaviours behind one signature.

All three now run through one helper (`backend::boolean`) on `BRepAlgoAPI_Cut`, the class
the bridge exposes both the operation code and the fuzzy value on — the same thing
upstream does with `BRepAlgoAPI_BooleanOperation::myOperation`. `Cut` keeps its
compound-tool rewriting; `Fuse` and `Common` are one plain build.

## SimpleCAD's `close_holes` (a patch to the checkout)

`erase_elements` closes a hole's wall through SimpleCAD's own `close_holes`, which picks
the shape of the closure. It used to cap **any** loop it could not fill from a single
neighbour, so deleting a box's side face handed the face straight back and the “opened”
body stayed a solid. A loop that *is* the outline of a removed planar face is now read as
“that face was the opening” and declines, which opens the body.

This is a 10-line change to the **SimpleCAD checkout**
(`simplecad/core/geometry.py`, `close_holes`), not to this repository: the rule is theirs,
the seam it leans on (`Part.Face(wire)`, `replaceShape`, `removeShape`, `makeShell`) is
ours. It is pending upstream, so the conformance numbers quote
`../simplecad` @ `9180c4b` **plus this patch**.

## What is still open in this seam

- **`test_elongating` ×2, `test_mesh_glue`, `test_rotate_edges` ×2,
  `test_union.test_rod_a_hair_off_its_bore`.** Each needs its own reproduction; none is
  the seam above.
- **A boolean result is a `Compound` around one `Solid`** (see
  [`part-containers.md`](part-containers.md#open-questions)).
