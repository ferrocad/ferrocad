# The sub-shape edit seam

Status: design note (2026-11-14, revised 2026-11-15).

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

## `Edge.Placement` and the cached entry

The same divergence as `Orientation`, for the geometry: `Placement`, `translate`, `rotate`
and `replaceShape` re-bake the base `PartShape`, while the cached `EdgeEntry` keeps the
shape the edge was built from. `Part.Wire`, `Part.Face`, `ShapeFix`, `ChFi2d` and
`Edge.Curve` read the entry, so an edge moved by `edge.Placement = …` reached a new wire at
its **old** place:

```python
edge = shape.getElement("Edge11").copy()
edge.Placement = turn.multiply(edge.Placement)   # the base moves …
wire = Part.Wire([edge])                         # … the wire did not
```

`Part.Wire([edge.copy()])` was correct only because the copy is a plain `Part.Shape`, which
has no cached entry. SimpleCAD's rotate-edges path is exactly this idiom, and the erase had
already opened the body, so the crescent appeared 180° from where it was dropped.

The consumers now take a live edge's geometry from its base (`PyEdge::entry_from`,
`wire_shapes`, the two `edge_shape` helpers, `Edge.Curve`); the entry's analytic `curve`
remains the fallback for a shape the kernel cannot classify. Pinned by
`an_edge_placement_reaches_the_shapes_that_consume_it`.

## A wire wrapped from a shape counts its edges

`Part.Wire([...])` carries the analytic entries it was built from, but a wire reached
through a shape — `Face.OuterWire`, or the `Shape` a `Part::Feature` hands to Python — is
built by `PyWire::from_shape`, which has no entries. `Wire.__len__` and `__getitem__` read
only that list, so such a wire reported `len() == 0` while `Wire.Edges` returned all four:
Python derives truthiness from `__len__`, so **`bool(wire)` was False**.

That is not cosmetic. SimpleCAD's `heal_dangling_wires` selects its candidates with
`if … and obj.Shape and not obj.Shape.Faces …`, so a real slot wire was skipped and two
`test_elongating` heal cases never ran. Length and iteration now derive the edges from the
B-Rep when the cache is empty (the same fallback `Wire.Edges` already had), and the three
paths share one helper (`PyWire::edge_entries`). Pinned by
`a_wire_wrapped_from_a_shape_counts_its_edges`.

## `Face.addWire` is `BRep_Builder::Add`

Upstream's `TopoShapeFacePy::addWire` adds the wire to the face with
`BRep_Builder::Add(face, wire)`: no reversal and no containment test, so a caller that means
a hole reverses the wire first (upstream's doc example does exactly that). The seam exposes
it as `GeometryBackend::add_wire_to_face` → `policy::face_add_wire`, and the Python method
requires a `Part.Wire` as upstream's typed signature does.

This is how SimpleCAD rebuilds a face one boundary at a time for a concentric offset; the
empty-leftover case is the area assertion in `a_face_accepts_another_boundary_wire`.

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

## SimpleCAD's `close_holes` — an upstream policy, not a seam defect

`erase_elements` closes a hole's wall through SimpleCAD's own `close_holes`, which picks
the shape of the closure. Deleting a box's side face leaves the “opened” body closed,
because the boundary loop has **four** neighbours, so the one-neighbour fill branch never
runs, the coplanar guard is false, and the function falls through to its **cap** branch.
That is the function's documented policy: "any other flat loop … a new flat cap closes
it", and the sibling `test_hole_closing.py` asserts that a one-neighbour hole wall does
close.

The probes confirm the seam is not at fault: `Part.Face(wire)` builds the 300 mm² cap,
`Part.makeSolid(shell)` returns a valid closed 12 000 mm³ body, and both the forward and
the reversed wire are accepted. `test_edge_deletion.py` expects the opposite (5 faces,
`isClosed() == False`), so the two failures are an **upstream expectation mismatch**, filed
with the test assumptions.

A twelve-line rule in the **SimpleCAD checkout** (`simplecad/core/geometry.py`,
`close_holes`) made this loop decline the cap and took `test_edge_deletion` to **25/25**,
but the project keeps `../simplecad` at `9180c4b` and unmodified, so the experiment was
reverted. If upstream changes the policy, revisit; do not patch the reference checkout.

## What is still open in this seam

- **`test_analytic`'s in-place pair, `test_mesh_glue`,
  `test_union.test_rod_a_hair_off_its_bore`.** Each needs its own reproduction; none is the
  seam above.
- **`test_precision.test_through_push_from_a_slightly_tilted_face_leaves_no_skin`** — the
  last sub-tolerance case; it is a push/pull, not a plain boolean.
- **A boolean result is a `Compound` around one `Solid`** (see
  [`part-containers.md`](part-containers.md#open-questions)).
