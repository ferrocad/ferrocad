# The Sculpt bundle: a cage, a surface and a body

Status: design note (2026-11-16). The implementation is `python/Sculpt/`
(`subdivision.py`, `patches.py`, `objects.py`); the workflow test is
`tests/test_sculpt_workflow.py`; the argument for a bundle rather than a native
workbench is
[`../../project/ideation/surface-and-freeform-modelling.md`](../../project/ideation/surface-and-freeform-modelling.md)
§7.

Sculpt is the **bundle** tier in that note: FerroCAD owns the document model, the
body rules and the interaction seam, while the geometry is the ordinary `Part`
API. It is not an add-on host and not a new kernel — a cage, a subdivision and a
patch shell, all in Python over the existing modules.

## The three pieces

| Object | Type | Properties | Body role |
| --- | --- | --- | --- |
| **cage** | `Mesh::FeaturePython` | `CagePoints` (`App::PropertyVectorList`), `CageFaces` (`App::PropertyIntegerList`, four indices per quad), `Levels` | **support** |
| **skin** | `Part::FeaturePython` | `Cage` (`App::PropertyLink`), `Levels` (`0` = follow the cage), `Tolerance` (sew, mm) | **base** (`BaseFeature`) |
| body | `PartDesign::Body` | `Group`, `Tip`, `BaseFeature` | — |

`Sculpt.makeCage` / `makeSkin` add the properties and the proxies;
`Sculpt.nest(body, *objects)` adds them and answers the role each one took
(`"support"`, `"base"`, `"tip"`), which is the one thing a workbench must get
right: **the cage must not become the body's shape** and **the skin must be the
shape the features build on**.

Why the cage is a support and not the base is the `BodyMembership` declaration,
not a special case in the container: `Mesh::FeaturePython` declares `Support`
(`crates/ferrocad_mesh/src/mesh_property.rs`). Upstream's `Body::isAllowed`
refuses every mesh, so an add-on doing this reaches a body only through a
binder; the declared capability is what lets a cage sit in the group without
taking the one base slot. The rules are in
[`part-shape-edits.md`](part-shape-edits.md) ("`PartDesign::Body` membership is
declared, not listed").

## The subdivision

`Sculpt.catmull_clark(points, faces)` is one refinement and returns
`(points, quads)`; `Sculpt.subdivide(..., levels)` repeats it. The rules are the
standard ones, and the boundary rules matter because a cage is open while it is
being built:

- a face point is the centroid of its face;
- an interior edge point averages its two endpoints and the two adjacent face
  points, a boundary edge point is its midpoint;
- an interior vertex with valence `n` moves to `(F + 2R + (n - 3)P) / n`;
- a boundary vertex **with one incident face** (the corner of an open patch), or
  with a boundary fan that is not one chain, is a corner and stays put;
- any other boundary vertex (two boundary edges, two or more faces) uses the
  `(P_prev + 6P + P_next) / 8` curve rule.

The corner rule is the one to keep: without it a single-quad cage shrinks to
three quarters of itself at the rim and a user dragging a corner sees the surface
pull away from their point. With it, a refined open cage's rim stays on the
original outline (split at the new edge points), which is what
`test_a_single_quad_refines_into_four_and_keeps_its_rim` pins.

An edge used by more than two faces is treated as a boundary, so a non-manifold
cage refines instead of failing.

The output point order is part of the contract, because the preview and the patch
builder index it: the original points first (moved), then one point per face in
face order, then one per edge in first-seen order.

## The patches — a house approximation, and it says so

`Sculpt.bicubic_patch(corners)` is the quad's bilinear patch sampled on a 4×4
pole grid (degree 3 both ways). Two neighbours share their boundary straight line
exactly — the boundary control points are collinear with the shared corners — so
the sewn shell has no gaps and the surface is **G0 by construction**. The
smoothness comes from the refinement level, not from the stitch.

That is deliberately *not* a Catmull-Clark limit patch: a true limit patch needs
the one-ring stencils of all four corners (OpenSubdiv's approach) and is a
separate piece of work. The level knob is the accuracy dial, and the drop between
levels measures what is left:

| Levels | Preview mesh volume | Skin volume | Faces |
| --- | --- | --- | --- |
| 0 (cage) | 24 000 mm³ (the box) | — | — |
| 1 | 10 000.0 | 10 250.0 | 24 |
| 2 | 8 402.8 | 8 404.6 | 96 |
| 3 | 7 993.9 | 7 993.94 | 384 |

(40 × 30 × 20 mm box cage, in-process debug extension.) The two representations
converge on the same limit surface; at level 3 they agree to 0.07 mm³ of 7 994,
and the level-to-level step has fallen from 1 845 to 411 to about 100 mm³. The
skin's volume is slightly *above* the faceted preview at low levels because a
bilinear patch through four corners bulges outside the planar quads.

`Sculpt.patch_solid` raises `ValueError` when the sewn shell is not closed: an
open cage is a surface, and returning a shell would let a body adopt a shape it
cannot use. The proxy's exception reaches the object as
`Invalid: proxy.execute failed: the cage is not closed: …`, which is the binding's
normal proxy-failure path.

## The document flow

The cage and the skin are one model with two representations, and the document's
link graph does the ordering — the skin holds `Cage` as an `App::PropertyLink`,
so a cage edit re-runs the skin. Moving the cage's `Placement` moves both (the
skin transforms the cage's points, it does not read the mesh). A body placement
then moves the assembled body and leaves the pieces in model space.

Measured on the cube cage: `makeCage` + recompute is 5 ms at level 2, the skin's
first rebuild is about 140 ms (level 2, 96 patches) and 520 ms at level 3, in the
debug extension. Sewing dominates; the subdivision itself is milliseconds.

## The cage grip

Dragging a control point is the one interaction the bundle owns. It is split across
two halves that never name each other's layer:

**The display hook** (`python/Sculpt/display.py`). For a cage it answers one stage row
in world space: the subdivision preview as the surface (part 0, no element name), the
**control hull** as edges, and every control point as a stage *vertex* whose part is
named `Point0`, `Point1`, … A Part object, a plain mesh or the toy slab produces no
such row, so nothing else can accidentally arm the grip.

**The manipulator** (`crates/ferrocad_sculpt/src/cage.rs`). A `ManipulatorFactory`
is handed only the selection and the stage, so the stage is the only place the grip
can learn where the control points are — hence the row above. It arms when any
selection on the cage's item publishes `Point*` parts (a face pick is enough, as for
the toy grip), draws a screen-invariant octahedron at every control point, and a drag
moves the grabbed one in the plane through its press position that faces the camera
(the right behaviour for a free vertex: no axis to choose). Its typed twin is X/Y/Z,
so a coordinate can be typed or nudged instead of dragged.

**The commit** is `cage_point` / `move`, element `Point3`, values `[x, y, z]` in
world space. The release is data; whoever registered the verb applies it.
`python/Sculpt/interaction.py` does: it inverts the cage's `Placement` to get the
local coordinate, writes `CagePoints[3]`, recomputes and commits **one named
transaction**. The skin is ordered after the cage by its `Cage` link, so that
recompute rebuilds the preview mesh and the sewn solid together — one drag is one
undo step, and the B-rep follows live.

Returning `None` for every other verb is part of the contract: the facade asks its
interaction providers in order and the first non-`None` answer owns the commit, so the
Sculpt hook stays out of the way of Part's dress-up, push/pull and transform.

Why not select a kernel vertex instead? A cage point is a *control* point: it has no
`VertexN` name because it is not on the resulting shape (the subdivision moves it).
The published `PointN` part is what gives the drag a stable name across a rebuild.

## Persistence

Native `.ferrocad` documents keep each proxy by module and class name and restore
it, so a reopened document rebuilds both objects through `onDocumentRestored`
(`test_a_native_round_trip_restores_both_proxies`).

A `.FCStd` keeps the properties and the shapes but **not** the proxy: FerroCAD's
FCStd writer reports upstream's binary Python object rather than writing an
incompatible payload
([`fcstd_write.rs`](../../../crates/ferrocad_core/src/document/fcstd_write.rs)).
`Sculpt.attach(document)` installs the proxies on the objects that need them —
the same call an add-on makes after opening its own file — and returns the names
it re-attached.

## What this slice does not do

- **No GUI commands beyond the six `Sculpt_*` ones**, and no workbench switcher
  entry: the commands are registered and listed, but the shell's palette is the
  only surface that shows them.
- **No cage edge/face handles.** The grip moves one control point; extruding a cage
  face or sliding a hull edge is the next interaction, and it needs a different
  commit shape (a face needs two points, not one).
- **No vendored add-ons.** `Curves`/`SubD` are not bundled; the payload records them
  as `planned` and Silk as excluded, and the synthetic call surface they document is
  measured in
  [`../../project/research/p3-addon-surface-spike.md`](../../project/research/p3-addon-surface-spike.md).
  The **edition and packaging** are done: `ferrocad_sculpt` builds, `--edition sculpt`
  stages a curated payload, and `release.yml` has the matrix legs
  ([`../architecture/edition-publishing.md`](../architecture/edition-publishing.md)).
- **No limit-surface patches** (G1/G2 continuity), no n-gon *output* (any polygon
  is accepted as input and comes back as quads), and no cage undo grouping beyond
  what the document already does per property write.

## Testing

`tests/test_sculpt_workflow.py` (29 cases, `workbenches` extension set) covers the
rules (a quad's corner stays, a crease vertex follows the curve rule, a closed cage
refines closed and quad-for-quad), the objects (preview counts and volume, an edit
re-running the proxy, placement), the solid (valid, closed, one solid, one face per
refined quad, convergence over levels, an open cage reported), the body (support/base
roles, the body shape being the skin, the body placement, a second base refused, a
non-cage feature refused, a PartDesign feature taking the tip), the display row (the
`Point*` parts, the hull edges, world space) and the interaction verb (one undo, the
placement inverted, other verbs left alone, a bad element refused) and persistence
(native round trip, `.FCStd` + `attach`).

`crates/ferrocad_sculpt` adds six unit tests for the grip (arming, the marker overlay,
the screen-parallel drag, the X/Y/Z twin, a miss) and three composition tests that
boot the edition's own app script through the host: the startup sample, the command
flow, and the grip's contract end to end (`FerroCAD.display_mesh` →
`ferrocad_shell.interaction` → one undo).

The body half is pinned twice: this module through the public API and
`crates/ferrocad_part_py/tests/body_membership.rs`
(`a_mesh_cage_is_a_support_member`).
