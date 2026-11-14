# The sub-shape edit seam

Status: design note (2026-11-14, revised 2026-11-16).

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

## A circle's frame reaches its edge

`Part.makeCircle(radius, center, normal)` and `Circle.toShape()` build the edge from the
analytic circle. The edge is a `gp_Circ` in a `gp_Ax2`, and `edge_from_circle` used to pass
only the **normal**, letting OCCT choose the X direction. For a `-Z` normal that choice is
half a turn from the circle's own `XAxis`/`AngularXU`, so the edge's seam vertex sat at
parameter π while the same circle reported its `value(0)` on the other side.

That showed up as geometry, not as a curve reading: sewing a rim built with `-Z` to the
cylinder's own `+Z` rim split the shared edge at both seam points, so one circular rim
became **two arcs** (`SimpleCAD`'s boss rebuild counted three circles instead of two). The
edge now takes the analytic frame (`gp_Ax2_new3(center, normal, x_axis)`), which is where
upstream's `Geom_Circle` parameterises from, and `Circle.XAxis = …` moves the edge's start
point with it. Pinned by `a_circles_x_axis_is_its_edges_parameter_zero`.

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

## A degenerate edge has no curve

A sphere pole and a cone apex are real edges whose 3D curve collapses to a point.
`Edge.Curve` raises `TypeError` for them, as upstream does, rather than returning `None` or
a zero-length line: a caller that means "this edge has geometry" should be able to check by
asking. `Edge.Degenerated` is the question to ask instead, and upstream's heal uses exactly
that pair (`None if edge.Degenerated else edge.Curve`).

## An elementary surface is a value object

`Part.Cylinder` (and its siblings) are filled in after they exist:
`surface = Part.Cylinder(); surface.Radius = …; surface.Center = …; surface.Axis = …`, and
`surface.toShape(u1, u2, v1, v2)` builds the face over that parameter band with upstream's
`BRepBuilderAPI_MakeFace(Geom_Surface, u1, u2, v1, v2)` (a new
`GeometryBackend::make_face_on_band`). A full turn is one wall with a seam, which is how a
caller rebuilds a hole wall square to its aligned rims. An axis write keeps the axis point
and projects the frame's `u = 0` direction onto the new plane.

`parameter(point)` belongs to the same value-object contract (`GeometrySurfacePy::parameter`):
it is the inverse of `value(u, v)` and answers `(u, v)` in the surface's own range. Every
surface class here had it except `Part.Toroid`, which SimpleCAD reads through
`Face.Surface.parameter` for a click's normal. The torus's `u` is the angle around the axis
and `v` the angle around the tube, both normalised to `[0, 2π)`.

`Part.BSplineSurface()` starts empty and `interpolate(grid)` **replaces** it in place —
upstream's `setHandle(sur)` — so `value`/`parameter`/`NbUPoles` work after the call. Keeping
only the interpolation grid and knots is not enough: every one of those queries reads the
explicit geometry, and the surface must keep it.

This matters more than it looks: SimpleCAD's heal rebuilds every aligned face, and its
per-face `except Exception` turns a missing setter into "keep the face as it was". A
read-only surface therefore left the tilted face tilted *and* pinned its neighbours, which
reads as a geometry bug rather than a binding gap.

## `Face.cutHoles` is `BRepBuilderAPI_MakeFace::Add` per wire

Upstream's `TopoShapeFacePy::cutHoles` builds `BRepBuilderAPI_MakeFace(face)` and adds
each item with `Add(wire)` — again no reversal and no containment test, and the items are
wires, not necessarily `Part.Wire` instances (`Wire.reversed()` returns a plain `Shape`).
The seam therefore accepts any `Part.Shape` whose shape type is a wire, and a null face
raises `ValueError` before the list is parsed. Pinned by
`a_face_cuts_hole_wires`; it is what closed `test_mesh_surfaces`' fitted-face pair.

## `Shape.tessellate` takes upstream's default angular deflection

Upstream's `defaultAngularDeflection(linear)` is `min(0.1, linear * 5 + 0.005)`: a coarse
linear request still bounds the angular error and a fine one does not explode the triangle
count. The one-argument `Shape.tessellate(deflection)` form uses it (OCCT's own 0.5 rad is
much coarser, and `test_mesh_glue` measured the difference), and the STL path shares the
same `ferrocad_geom::default_angular_deflection`. Passing `angular` explicitly is a house
extension that pins the angle instead.

A finer default is not free: it is what exposed the `flatmesh` unfold's vertex tear (see
[`../../project/record/2026-11-14-p2-boolean-seam.md`](../../project/record/2026-11-14-p2-boolean-seam.md)
§8.4), so a mesh-density-sensitive helper needs a fine-mesh case in its tests.

## `App::Part`'s extensions are type-static

Upstream instantiates `App::GeoFeatureGroupExtension` (which derives from
`App::GroupExtension`) in the constructor of `App::Part` and `PartDesign::Body`. A caller
that asks `hasExtension("App::GeoFeatureGroupExtension")` — SimpleCAD's `editable`, which
keeps a transformed `App::Part` from being moved as one shape — must get `True`. The answer
is computed from the type in `Document::has_extension` and is **not** written into the
object's extension set, so a file keeps carrying only the extensions a caller actually
added.

## The boolean tolerance contract

`fuse`, `cut`, `common` and `multiFuse` declare upstream's signature from
`TopoShape.pyi`:

```python
fuse(other, tolerance=0.0, *, noElementMap=False)
```

The **second positional argument is the tolerance**. It used to be a house `glue` flag,
with `refine` beside it, so a script that passed a fuzzy value positionally
(`a.fuse(b, 0.001)`) set a truthiness flag and the kernel never saw the number — that is
what left `test_union`'s rod a hair off its bore. The pair is gone; every internal call
site passes the tolerance in the second position.

The value reaches the kernel the way upstream's `TopoShape::makeElementBoolean` does:

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

## A refused boolean raises `ValueError`

OCCT refuses some boolean operands outright: a solid cut by an open shell returns
`BOPAlgo_AlertBOPNotAllowed` and a null result — on 7.9.2 exactly as on 8.0.1. Upstream
does not check `IsDone()`; it hands the null to `makeShapeWithElementMap`, whose
`NullShapeException` derives from `Base::ValueError`, so Python sees a **`ValueError`**.
Only an OCCT *exception* leaves that path as a `Part.OCCError`.

`Shape.fuse`/`cut`/`common`/`multiFuse` split the two the same way (`boolean_error` in the
binding): a `NotDone` or null-input backend error is a `ValueError`, while an
`OcctError::Exception` stays a `Part.OCCError`. This is not cosmetic — SimpleCAD's
`preserves_volume` refuses an open result by catching `ValueError`, so an `OCCError` there
is an unhandled exception. Pinned by
`a_refused_boolean_raises_value_error_like_upstream`.

## A sub-shape result is typed

Every list of shapes a `Part.Shape` method returns goes through `wrap_shape`, not a bare
`Part.Shape`: `generalFuse`'s result and its per-input pieces, `childShapes`,
`SubShapes`, `Faces`/`Edges`/`Vertices`/`Solids`, `getElement`. Upstream does the same with
`shape2pyshape` (`TopoShapePy::generalFuse`, `getElements`), and callers depend on it: a
sliced-off vertex piece must answer `.Point`, which only `Part.Vertex` has.

`childShapes()` keeps its compound-aware walk (the coarsest kind present, depth-first) but
returns the typed classes now. `BOPTools.SplitAPI.slice` reads those pieces and needed no
change.

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

## `Shape.Placement` is a location over an un-moved base

Upstream keeps a standalone shape's placement as a `TopLoc_Location` on the shape
handle: a move changes where the shape *is*, not what its coordinates *are*. We baked the
placement in (`BRepBuilderAPI_Transform`), which is geometrically equivalent and invisible
until something asks about **identity** or **exactness**:

- `bake(P)` then `bake(P⁻¹)` left ~5e-15 on a 10 mm cube, so a perfect cube's
  longest-side tie-break chose Y instead of X.
- Every sub-shape handle changed, so `isPartner` stopped recognising a rigidly moved
  shape — and SimpleCAD's style mapping short-circuits on `current.isPartner(original)`,
  so a moved object fell back to the predominant style.

A `PartShape` now carries the geometry with the accumulated placement removed (its
**base**) beside the placement. A placement write transforms *from the base*, so
`Placement = identity` hands the base back untouched; `Shape.copy()` hands the base over
as a shared immutable handle rather than copying the kernel shape. A **rigid** write is
`GeometryBackend::relocate`, i.e. `set_location` with a `TopLoc_Location` built from the
transform, on the same TShape; `isSame` still reports false after a move, as upstream's
does, because the location is part of the shape. Pinned by
`a_placement_reset_is_bit_exact` and `a_rigid_move_keeps_the_shapes_identity`.

The rule for a new consumer: treat `Placement` as where the shape sits, never as a value
to fold into coordinates. A transformed shape reports `1000.000000` mm³ for a 10 mm cube,
not `1000.0000000000002`.

## A near-tangent boolean is not the seam's error

`test_precision.test_through_push_from_a_slightly_tilted_face_leaves_no_skin` used to be
filed as the last P2 failure. It is not one. The test's own gate fails, not the boolean
contract:

- `opposite_face` moves the profile to the exit plane and proves coverage with
  `fuzzy = 2 × gap + 1e-7`; it then runs its sweep test as `swept.common(shape)` with the
  default tolerance 0. With the same fuzzy the sweep is exact (81.714105065 mm³).
- At tolerance 0 OCCT sees a near-tangency: the prism cap is tilted by 2.35e-5 rad against
  the block's x = 0 face, so the exit cap comes back as two pieces totalling 7.19e-4 mm² too
  much, with a result vertex tolerance of 9 × tilt. The error is linear in the tilt.
- That is not ours: the two-shape `BRepAlgoAPI_Common` (upstream's C++ call) and this seam's
  `Cut` + `SetOperation(COMMON)` return the same volume to 12 digits, and an independent
  build against OCCT 7.9.2 returns it too. Every input tolerance is 1e-7; neither upstream nor
  this seam refines inside `common`; the prism is exact (`area × distance`).

The reference fix is one line in `../simplecad` (`swept.common(shape, fuzzy)`), which stays
unmodified here. The detail is in
[`../../project/record/2026-11-14-p2-boolean-seam.md`](../../project/record/2026-11-14-p2-boolean-seam.md)
§8.8.

## What is still open in this seam

- **A boolean result is a `Compound` around one `Solid`** (see
  [`part-containers.md`](part-containers.md#open-questions)).
- **The chamfered single-face analytic selection is a face-order assumption, not a seam
  defect.** The test takes the first face with a horizontal normal between z 2 and 18;
  after SimpleCAD's mesh reconstruction that is a box side, which `prepare` correctly
  rejects. All 26 actual wall facets are accepted (radius 6.0000, height 19.002) and
  rebuild a valid closed solid.
- **The `close_holes` cap policy** (`test_edge_deletion` and `test_rounding`) is filed
  above: it is SimpleCAD's documented behaviour, and the reference checkout stays
  unmodified.
- **The wrapped-drawing inverse picks another branch, not another geometry.**
  `WrapFrame.local` inverts `point(q)` by projecting the traced point onto the *target
triangle's plane* (`basis @ (trace(q) - p)`). On a closed section that residual is
  rank-deficient when the target face is parallel to the start face: at the mouse's
  previous q the moved point differs by a vector *along the target normal*, which the 2D
  projection drops, so the solver cannot walk from `(25, 3)` to `(35, 0)`. It falls back to
  the triangle-corridor unfold, whose Dijkstra tree is rooted in the start face's first
  triangle (the x<0 half on every OCCT tried), and the four symmetric routes around the box
  make the -Y route shorter in centre-to-centre length. The developed coordinate comes back
  as `-45` instead of `35` — the same surface point, the other way round the perimeter.
  Both choices are the reference's, not the seam's: the guide mesh is OCCT's own (7.9.2 and
  8.0.1 return the same two facets in the same order), every input tolerance is 1e-7, and
  the forward `point(35, 0)` is correct. Filed as a documented test assumption; the
  branch-consistent inverse would have to compare candidates against the previous q in the
  reference.
