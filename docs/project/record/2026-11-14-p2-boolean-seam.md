# The P2 boolean/seam round — 2026-11-14/15

Status: round record (written 2026-11-15, extended the same evening). This is the compact
measurement of the P2 work that followed the manipulator day. The backlog that held it is
[`TODO-14-11-2026.md`](TODO-14-11-2026.md); its successor is
[`../TODO-15-11-2026.md`](../TODO-15-11-2026.md). The stable contracts are
[`../../developer/workbenches/part-containers.md`](../../developer/workbenches/part-containers.md)
and [`../../developer/workbenches/part-shape-edits.md`](../../developer/workbenches/part-shape-edits.md);
the failing-test map is [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md)
§7.3.

## 1. What the round established

| Step | Change | Measured |
| --- | --- | --- |
| **Containers** | `Refine` is declared, read and defaulted to upstream's `true`; `MultiCommon.Behavior`; a lone compound is flattened for both multi-booleans; `MultiFuse` arity; `Part.generalFuse(fuzzy)`; the four `Part_Fuse`/`Cut`/`Common`/`Compound` commands | Part **531/0/0/12**; SimpleCAD **985/49/18/7** (unchanged — fidelity, not the counted failures) |
| **Sub-shape seam** | One boolean build answers every `Shape.generalFuse` input; a replaced vertex reports its result image; `Edge.Orientation` reaches the cached entry; `App::Part`/`PartDesign::Body` answer the geo-feature-group extension | SimpleCAD **997/32/18/7** |
| **Boolean tolerance** | `Shape.fuse`/`cut`/`common` hand their `tolerance` to the kernel with upstream's signed contract (`0`, `> 0`, `AUTO_FUZZY`), through one helper | pinned by `part_shape_ops`; totals unchanged by the signature alone |
| **Edge placement** | a live `Part.Edge` is read from its baked base, not its stale cached entry, in `Part.Wire`/`Part.Face`/`ShapeFix`/`ChFi2d` and `Edge.Curve` | SimpleCAD **1003/27/17/7** |
| **Wire length and `Face.addWire`** | a wire wrapped from a shape counts its B-Rep edges, so `bool(wire)` is true; `Face.addWire` attaches a boundary as `BRep_Builder::Add` does | SimpleCAD **1005/25/17/7** |
| **Circle frame** | `Part.Circle`'s analytic `XAxis` is the edge's parameter 0 (`gp_Ax2` with the frame, not the normal alone) | SimpleCAD **1006/24/17/7** |
| **Clean-checkout re-measure** | the `close_holes` experiment was reverted; `../simplecad` stays at `9180c4b` | SimpleCAD **1006/24/17/7** |
| **Boolean signatures** | `Shape.fuse`/`cut`/`common`/`multiFuse` take upstream's `(other, tolerance=0.0, *, noElementMap)`: the second positional argument is the fuzzy tolerance, and the house-only `glue`/`refine` pair is gone | SimpleCAD **1013/20/14/7**; both `test_union` geometry records close |
| **Tessellation default** | `Shape.tessellate`'s omitted `angular` is upstream's `defaultAngularDeflection` (`min(0.1, linear * 5 + 0.005)`), one `ferrocad_geom` helper shared with the STL path | `test_mesh_glue` **6/6** (was 5/6) |
| **`Face.cutHoles` and a circle-based `Cylinder`** | `cutHoles(list)` is `BRepBuilderAPI_MakeFace::Add` per wire on any wire-shaped `Part.Shape`; `Part.Cylinder(circle)` reads radius, centre, axis and X direction from the circle | `test_mesh_surfaces` **7/7** (was 3 pass, 2 fail, 2 error) — P3's SimpleCAD share empties |
| **The unwrap tear** | `flatmesh` lays each vertex once: a fan-closing triangle keeps the first position instead of moving it | SimpleCAD **1014/20/13/7**; `test_surface_offset`'s across-the-seam error closes |
| **The undo transaction** | `set_label` records `Change::Label`, and a type's derived mirror (`Placement` → `Shape` on a `Part::Feature`, → the mesh transform on a `Mesh::Feature`, a shape assignment's adopted `Placement`) goes through `Document::set_property_derived` | SimpleCAD **1019/15/13/7** — the undo/label bucket empties; `test_union` **16/16** |
| **A degenerate edge and a writable surface** | `Edge.Curve` raises `TypeError` on a pole/apex edge; `Part.Cylinder`'s `Radius`/`Center`/`Axis` are writable and `toShape(u1, u2, v1, v2)` builds the parameter band | SimpleCAD **1026/8/13/7** — `test_healing` 14/14 and `test_degenerate_drawing` 5/5 |
| **The placement representation** | a `PartShape` keeps its un-moved geometry beside the placement, so a reset is bit-exact; a rigid `Placement` write is a `TopLoc_Location` (`relocate`), so a moved shape is still a partner of its source | SimpleCAD **1028/6/13/7** — the principal-axis tie and the style-mapping failures close |
| **The refused boolean** | a boolean OCCT cannot run (a solid cut by an open shell) yields a null result; upstream turns that into `NullShapeException`, a `ValueError`, where our binding raised `Part.OCCError` | SimpleCAD **1029/6/12/7** — `test_surface_wrap`'s area/volume comparison stops erroring |
| **Typed sub-shapes and surface queries** | `generalFuse`/`childShapes`/`SubShapes` answer the typed `Part.*` classes (`shape2pyshape`), `Part.Toroid` gains `parameter(point)`, and `BSplineSurface.interpolate` keeps the geometry it builds | SimpleCAD **1032/6/9/7** — the slicing vertex, the torus click and the fitted-boundary snap stop erroring |

SimpleCAD's integrated run went **985 / 49 / 18 / 7 → 1032 / 6 / 9 / 7 over 88/88
modules**: **43 failures closed, none added, and the errors moved 18 → 9**. The printed
failure count falls by one less than the records closed in the last two steps because a
fully-passing module counts once. No module failed to load.

## 2. The sub-shape seam

Three root causes, all in the seam:

1. **`Shape.generalFuse` rebuilt the boolean once per input.** Every input's images then
   came from a different build and did not share sub-shapes; mixing them into a face gave
   coincident, unshared vertices, so a local face split could not rebuild the faces around
   its line and fell back to splitting the whole solid. One build now answers every input.
2. **The kernel reports no image for a vertex it replaced.** The rebuilt faces kept a stale
   vertex at one end. A replaced vertex now reports the single result vertex at its point; a
   surviving vertex reports nothing.
3. **`Edge.Orientation = "Internal"` never reached the edge's cached entry**, so a spur read
   `Internal` and arrived in a new wire as `Forward`.

The 20 failures this seam closed:

| Group | Tests |
| --- | --- |
| Local split | `test_dense_drawing.LocalSplitTests.test_a_line_across_two_faces_splits_only_them`, `…test_a_line_ending_inside_an_edge_rebuilds_its_neighbour` |
| Nearly coplanar | `test_dense_drawing.NearlyCoplanarTests.test_a_line_across_leaning_triangles_divides_both` |
| Internal edges | `test_geometry.DocumentTests.test_clean_solid_faces_removes_internal_edges`, `…test_deleting_a_line_beside_a_step_sliver_merges_the_faces`, `…test_erase_internal_edge_on_solid_face` |
| Delete and redraw | `test_dense_deletion.DenseDeletionTests.test_mid_edge_divider_reconnects_neighboring_shell_faces`, `…test_open_shell_merge_and_redraw_keep_hole_and_other_divisions` |
| Change records | `test_dense_drawing.ChangeRecordTests.test_seams_through_a_derived_index_match_a_full_pass`, `…test_snaps_after_a_line_are_patched_without_listing_the_part`, `…test_the_result_index_matches_a_fresh_listing` |
| Surface line (P3) | the six `test_surface_line.FreeEndTests` subtests, plus `…SurfaceLineDocumentTests.test_moved_rotated_object_keeps_placement_and_survives_save_reopen` and `…test_open_segments_skip_the_split_and_the_closing_one_still_divides` |
| Query strictness | `test_geometry.DocumentTests.test_transformed_container_rejected` |

The eight `test_surface_line` failures were the same defect, so **P3 dropped from 11 to 3
without any surface work**.

## 3. The boolean tolerance

`Shape.fuse`/`cut`/`common` accepted a `tolerance` argument and dropped it, with `cut`
auto-fuzzing unconditionally while `fuse`/`common` set no fuzzy value at all — three
behaviours behind one signature. The seam now carries upstream's contract
(`TopoShape::makeElementBoolean`):

| `tolerance` | Kernel request |
| --- | --- |
| `0` (the `Shape.fuse` default) | leave the confusion tolerance alone |
| `> 0` | `SetFuzzyValue(tolerance)` |
| `AUTO_FUZZY` (`-1`) | the operands' bounding-box diagonal × `Precision::Confusion()` |

All three run through one helper on `BRepAlgoAPI_Cut`, the class the bridge exposes both
`SetOperation` and `SetFuzzyValue` on — the shape of upstream's
`BRepAlgoAPI_BooleanOperation::myOperation`. `Cut` keeps its compound-tool rewriting;
`Fuse` and `Common` are one plain build. The document booleans (`Part::Fuse`/`Cut`/
`Common`, and the extrusion's inner cut) pass `AUTO_FUZZY`, because upstream builds them
through the two-shape `FCBRepAlgoAPI_*` constructors, whose constructor computes it.

## 4. The edge placement, and the rotate cluster

`test_rotate_edges` failed with the crescent on the wrong axis. The probe showed the
rotation itself was right — `e.Placement = turn.multiply(e.Placement)` moved the half-ellipse
edges to the bottom — but `Part.Wire(rot_edges)` rebuilt them **unrotated**, and
`Part.Wire([e.copy() for e in rot_edges])` built them correctly.

The cause is the same shape the `Orientation` fix addressed: a `Part.Edge` carries the shape
it was built from in its cached `EdgeEntry`, beside the baked `PartShape`. `Placement`,
`translate` and `rotate` re-bake the **base**, so after a placement write the entry still
holds the pre-transform geometry, and every consumer that reads the entry (`Part.Wire`,
`Part.Face`, `ShapeFix`, `ChFi2d`, `Curve`) consumed it. SimpleCAD's idiom is exactly
`e = el.copy(); e.Placement = …; Part.Wire(rot_edges)`, so the whole rotate-edges path used
the old edges while the erase had already opened the body — the crescent appeared 180° from
where it was dropped.

The fix reads a live edge from its base in `PyEdge::entry_from`, `wire_shapes`, the two
`edge_shape` helpers and `Edge.Curve`; the entry's analytic curve remains the fallback for
geometry the kernel cannot classify. `part_shape_ops`'s
`an_edge_placement_reaches_the_shapes_that_consume_it` pins the edge, the wire and
`Curve.Center` after a placement write. `test_rotate_edges` is **5/5**.

## 5. The wire wrapper, and the elongating cluster

`test_elongating`'s two remaining failures both go through `heal_dangling_wires`, which
starts with `if … and obj.Shape and not obj.Shape.Faces …`. A probe showed the standalone
slot **wire** was skipped even though it had four edges: `bool(obj.Shape)` was **False**.

A `Part.Wire` carries the edges it was built from, but a wire reached through a shape
(`Face.OuterWire`, or the `Shape` a `Part::Feature` hands to Python) is made by
`PyWire::from_shape`, which leaves that list empty. `Wire.__len__` read only that list, so
it returned 0 — and Python derives truthiness from `__len__`, so a real four-edge wire was
falsy. `len(wire)` and iteration now derive the edges from the B-Rep when the cache is
empty (the same fallback `Wire.Edges` already had).

That exposed a second gap: once SimpleCAD's `partition_concentric_circle` accepted the
wire, it called `rem_f.addWire(...)` and raised
`AttributeError: 'Part.Shape' object has no attribute 'addWire'`. Upstream's `Face.addWire`
is a raw `BRep_Builder::Add(face, wire)` — no reversal, no containment test — so it is
implemented at the seam (`GeometryBackend::add_wire_to_face` → `policy::face_add_wire`),
and `Part.Edge` is rejected as upstream's typed signature does. Both are pinned by
`a_wire_wrapped_from_a_shape_counts_its_edges` and
`a_face_accepts_another_boundary_wire`. `test_elongating` is **19/0/0/1**. The `addWire`
work is what keeps `test_offsetting` at **17/17**: once the wire was truthy, its
`partition_concentric_circle` reached `addWire` for the first time.

## 6. The circle frame, and the analytic in-place pair

`test_analytic`'s boss case failed on `len(circles) == 2` with three circular edges of
radius 4. The rebuild is valid and has its one cylinder, but the base rim was **two
half-circles**: `Part.makeCircle(radius, center, -Z)` built its edge with the seam at
parameter π while the cylinder's own bottom rim had it at 0, so sewing split the shared
rim at both points.

The circle itself was consistent — `Circle.XAxis` was `(1, 0, 0)` and `value(0)` the same
point for both normals — but `edge_from_circle` built `gp_Ax2` from the **normal alone**, so
OCCT chose its own X direction for `-Z`. The edge now gets the analytic frame
(`gp_Ax2_new3(center, normal, x_axis)`), which is where upstream's `Geom_Circle`
parameterises from. Pinned by `a_circles_x_axis_is_its_edges_parameter_zero`; the boss
rebuild (and its fallback) now produce two single circle edges, and
`test_boss_wall_is_rebuilt_in_place_too` passes.

The sibling `test_single_face_on_chamfered_hole_replaces_in_place` is a different case: its
selection is the first face with a horizontal normal between z 2 and 18, which in our
reconstructed face order is a **box side** (`Face2`, area 600), not a facet of the hole.
`prepare` correctly rejects it. Every one of the 26 actual wall facets is accepted
(radius 6.0000, height 19.002, `can_cut` true) and the rebuild gives a valid closed solid
(volume 21 719.17 against the source's 21 718.16), so the fit and the seam support the
operation; the test's face-index precondition is what does not hold. It is filed with the
test assumptions, not with the P2 defects.

## 7. The `close_holes` experiment, and why it was reverted

The round wanted the two `test_edge_deletion` failures closed. A twelve-line rule in the
SimpleCAD checkout (`simplecad/core/geometry.py`, `close_holes`) made a loop that is the
outline of a removed planar face decline the cap; the body opened and the file was
**25/25**.

The project does not edit the reference checkout, so the patch was reverted and the totals
were re-measured on the bare `9180c4b`. The probes that justified the experiment also
settled the attribution:

- On a 40×30×10 box with `Face1` deleted, the boundary loop is one closed wire of four
  edges, each owned by exactly two faces, so the fill branch (one neighbour) never runs and
  the coplanar guard is false; the function falls through to its **cap** branch by design.
  `Part.Face(wire)` yields a 300 mm² cap and `Part.makeSolid(shell)` returns a valid closed
  12 000 mm³ body.
- The sibling `test_hole_closing.py` asserts that a hole wall (a one-neighbour loop) **does**
  close — the same function, the intended behaviour.

So the two failures are an **upstream expectation mismatch**: the shipped policy caps a
removed planar side face; the test asserts that the body opens. They are filed with the
test assumptions, not with the P2 defects, and `../simplecad` stays clean.

## 8. The 2026-11-15 rounds: signatures, tessellation, the unwrap tear and the undo log

### 8.1 The boolean signatures match upstream's declaration

`TopoShape.pyi` declares `fuse(self, other: Shape, tolerance: float = 0.0, *,
noElementMap: bool = False)` (and the same shape for `cut`, `common` and `multiFuse`). Our
declarations put a house-only `glue`/`refine` pair in front, so a script that passed a
fuzzy tolerance positionally set `glue` instead and the kernel never saw the number;
`test_union`'s rod stayed a hair off its bore, and a second record took the face-contact
path. The pair is gone, the second positional argument is the tolerance, and every internal
call site passes it in that position. `part_shape_ops` pins the new form; the two
`test_union` geometry records pass (`test_union` is **14/2**, the two remaining being the
document label `'Union' != 'First'` in §10).

### 8.2 `Shape.tessellate` defaults to upstream's angular deflection

Upstream's `defaultAngularDeflection` is `min(0.1, linear * 5 + 0.005)`: a coarse linear
request still bounds the angular error and a fine one does not explode the triangle count.
Our one-argument form used OCCT's own 0.5 rad, which is far coarser than FreeCAD's meshes —
`test_mesh_glue` measured a display mesh that upstream would not have produced and failed
its `thin_wall_triangle_profile_and_existing_hole` case. The helper now lives in
`ferrocad_geom::default_angular_deflection`, is used by `Shape.tessellate` **and** the STL
path (which had a copy of the formula), and the one-argument form meshes like FreeCAD.
`test_mesh_glue` is **6/6** and `part_shape_ops`'s
`tessellate_defaults_to_upstreams_angular_deflection` pins the value.

The finer default is also what exposed §8.4, the only regression this step caused.

### 8.3 `Face.cutHoles`, a circle-based `Cylinder`, and the mesh modules

`Face.cutHoles(list)` was an unimplemented stub. Upstream is a raw
`BRepBuilderAPI_MakeFace(face).Add(wire)` per item: no reversal, no containment test, and
only a wire-shaped `Part.Shape` accepted (`Wire.reversed()` is a plain `Shape`). It is
implemented that way, and a null face raises `ValueError` before the list is parsed.
`Part.Cylinder` gained upstream's circle-base constructor (radius, centre, axis and X
direction from the circle) and a cylinder copy, beside the house `radius/center/axis`
form. Together the two closed `test_mesh_surfaces` **7/7** (was 3 pass, 2 fail, 2 error),
so P3's SimpleCAD share is empty.

### 8.4 The unwrap tear

The finer tessellation broke `test_flatmesh_part`'s cone case: the cone wall's flat area
came back **1155.2 against its 1025.2** (12.7 % over), while the same face at a 0.5
deflection unwrapped to 1024.79. The mesh was not at fault — the sum of its 3-D triangle
areas is 1025.16, and every interior vertex has a zero angle defect except the two 2:1
boundary-transition rings (1.3e-5 rad).

`_unfold` was. It walks the dual graph breadth-first and, for each triangle it reaches,
writes its third vertex position **unconditionally**. In a triangle fan that vertex has
already been laid by an earlier fan member, so the write *moves* it away from the
triangles already using it — a tear. On a coarse mesh the fan closes exactly and the
rewrite is the same number; on a 170-column strip the residual compounds
multiplicatively, the layout spirals into itself, and 2 357 of 6 120 triangles ended up
between 1 % and 95× off their true area (the worst a 94× stretch). The relaxation pass
recovered only part of that: the measured flat area was 1 155 mm² (12.7 % over) where the
raw seed was 1 379 mm² (34 % over). A first-write-wins layout reproduces 1025.160 for the
same mesh — the sheet stays isometric and every closing fan keeps the first position. On a
developable face the two placements agree, so nothing is lost; on a curved one the residual
stays in that triangle and `_relax` spreads it, which is what the module always claimed to
do.

`test_a_finely_meshed_cone_wall_still_unwraps_to_its_area` pins a 0.006-deflection cone
(> 4 000 triangles, so the test only bites on a fine mesh). It also closed SimpleCAD's
`test_surface_offset.DraftTests` across-the-seam error, which unwraps a drawing that spans
the cut.

### 8.5 The undo log: a rename, and a placement's baked shape

The last five failures in the list were one defect seen from three modules: **half of an
edit was written outside the undo log**, so undoing or aborting the edit left the other
half applied.

1. **`Label` was never recorded.** It is a field of the document object rather than a
   property-container entry, and `set_label` assigned it directly. `ops.unite` renames its
   target to `Union` inside its transaction; `ops.commit_slicing` names the pieces
   `Block · 1`, `Block · 2`. Undo restored the shape and re-inserted the removed object, but
   the surviving object kept the new name, so `test_union`'s two document tests and
   `test_slicing`'s read `'Union' != 'First'` and `'Block · 1' != 'Block'`. A *refused*
   transaction had the same hole: `test_late_write_failure_aborts_all_changes` saw the
   rollback work and the name stay.
2. **The placement mirror was raw.** `Part::Feature` keeps its geometry stored placed, so
   `Placement = p` re-bakes `Shape`. That mirror went through `set_property_raw` — invisible
   to the undo log — while the placement itself was recorded. Committing looked right
   because both properties held the new value; *undo* reverted the placement and re-ran the
   hook, which re-baked the already-moved shape, so the body ended up elsewhere.
   `test_multiple_changes_are_atomic_even_if_commit_fails` caught the abort direction
   (`Shape.isSame(original)` false after `abortTransaction`) and
   `test_preview_commit_undo_redo_and_native_topology` the undo direction. The face/edge
   rotation path hit the same code from the other side: assigning a local `Shape` to a
   placed object adopts an identity `Placement`, and that adoption was raw too.

The seam now says every write a transaction makes is recorded:

| Call | Records | Touches the object | Fires the hook |
| --- | --- | --- | --- |
| `set_property` | yes | yes | yes |
| `set_property_derived` | yes | no | no |
| `set_property_raw` | no | no | no |

`set_property_derived` is for the mirror `on_property_changed` makes; `set_property_raw`
stays right for **computed output** (an `execute` writing the shape it just built is the
result of a recompute, not an edit). The rule, the restore path and the limits are
[`../../developer/architecture/transactions.md`](../../developer/architecture/transactions.md);
`abort_and_undo_restore_a_label`, `a_derived_property_write_is_reverted_with_its_primary`
and `a_placement_move_undoes_its_shape_and_a_label_undoes_with_it` pin it.

### 8.6 The query/tolerance round: a degenerate edge and a writable surface

Seven of the nine failures in this bucket were two missing contracts, both small and both
about what the kernel is allowed to say.

1. **`Edge.Curve` returned `None` for a degenerate edge.** The pole of a sphere (and a
   cone's apex) is a real `Edge` whose 3D curve collapses to a point, and upstream raises
   `TypeError` for it. We returned `None`, so `test_degenerate_drawing`'s shared `setUp`
   never raised and all five of its tests failed there — before reaching the drawing code
   they exist to exercise. `Curve` now asks `is_degenerate` first and raises; a copied pole
   refuses too, and an ordinary edge still answers.
2. **`Part.Cylinder` was read-only, and had no `toShape`.** Upstream's geometry classes are
   value objects: `surface = Part.Cylinder(); surface.Radius = …; surface.Center = …;
   surface.Axis = …` and then `surface.toShape(u1, u2, v1, v2)`. SimpleCAD's heal rebuilds
   a hole wall exactly that way, so the `AttributeError` was swallowed by its per-face
   `except Exception`, the wall was **kept** instead of rebuilt, and one kept face pinned
   its neighbours: the tilted face stayed tilted and the hole never became a circle
   (`aligned_holes: 0`). `test_healing`'s two failures were the two assertions that read
   that report. The properties are writable now (an axis change keeps the axis point and
   projects `u = 0` onto the new plane), and `toShape` builds the band with upstream's
   `BRepBuilderAPI_MakeFace(Geom_Surface, u1, u2, v1, v2)` through a new
   `make_face_on_band` seam. The healed block reports `aligned_faces: 6`,
   `aligned_holes: 1`, every planar axis exactly unit, and its volume unchanged to 1e-15.

The two that remain are attribution, not code:

- **`test_rounding`'s `type='Solid'` sample** is `erase_elements(box, ['Face1'])`, and it
  expects `selected_edges` to refuse the result. With the `close_holes` policy recorded in
  §7 the erase **caps the opening**, so the result is a valid closed solid and the refusal
  never happens. This is the same upstream-policy mismatch as the two `test_edge_deletion`
  failures, filed with them.
- **`test_symmetric_main_axis_is_deterministic`** asks a *perfect cube* for a principal
  axis. All three moments are equal, so `main_axis` falls back to the longest local
  bounding dimension; for a cube those are equal too and the **first** one must win. Ours
  differ by 5e-15 because clearing a shape's placement re-transforms its coordinates
  (`bake(P)`, then `bake(P⁻¹)`) while upstream stores the placement as a
  `TopLoc_Location` and clearing it is exact. The tie-break then picks Y instead of X. The
  fix direction is a base-cached placement (`Shape.Placement` writes transform from the
  unplaced base, so identity is exact); it is recorded in §P2 of
  [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) rather than guessed at here.

### 8.7 The placement representation, and the last two P2 failures

Three failures in the last two rounds were the same underlying question: **what is a
placement?** Upstream stores a standalone shape's placement as a `TopLoc_Location` on the
shape handle, so the coordinates are untouched by a move. We baked them
(`BRepBuilderAPI_Transform`), which is geometrically equivalent and observable nowhere —
until something asks about *identity* or *exactness*.

1. **Clearing a placement was not exact.** `Placement` writes composed a delta onto the
already-moved coordinates, so `bake(P)` then `bake(P⁻¹)` left 5e-15 behind on a 10 mm
cube. `main_axis`'s tie-break for a symmetric shape picks the longest local bounding
dimension, and for a perfect cube those are equal — so `test_symmetric_main_axis_is_deterministic`
saw Y instead of X. `PartShape` now keeps the geometry with the accumulated placement
removed and transforms *from it*, so reaching the identity hands the base back untouched;
`Shape.copy()` hands its base over too (an immutable handle, so no second kernel copy).
2. **A move replaced every sub-shape handle.** `isPartner` stopped recognising a moved
shape, and SimpleCAD's `mapped()` short-circuits on `current.isPartner(original)` — so a
rigidly moved object lost its per-face materials and fell back to the predominant style
(`test_rigid_placement_and_uniform_materials_keep_styles`). A rigid `Placement` write is
now `GeometryBackend::relocate`: the same TShape with a new `TopLoc_Location`. `isSame`
still reports false after a move, as upstream's does, because the location is part of the
shape.

The change is visible in the numbers: a moved box's volume is now `1000.000000` rather than
`1000.0000000000002`, because a move no longer rewrites a coordinate. Both are pinned by
`a_placement_reset_is_bit_exact` and `a_rigid_move_keeps_the_shapes_identity`.

### 8.8 `test_precision`'s tilted face: the seam is exonerated, the check drops its own fuzzy

The last "P2" failure is `test_through_push_from_a_slightly_tilted_face_leaves_no_skin`.
Re-traced 2026-11-16. It is **not** a seam gap: our boolean call is provably upstream's, the
kernel's error is the same on two OCCT releases, and the one line that decides the test has
already computed the tolerance it would need.

The geometry: a 9×12×8 block whose entry face leans by `tilt = 2.35e-5` rad. The test cuts a
Ø 3.4 mm disk into that face, then asks `opposite_face` for the exit through the material.
`opposite_face` moves the profile to the candidate exit plane, and its **coverage** check
passes with `fuzzy = 2 × gap + 1e-7` (`geometry.py:1811`). It then runs the **sweep** check
with `swept.common(shape)` — the default `tolerance = 0` (`geometry.py:1815`):

| Quantity | Value |
| --- | --- |
| profile | Ø 3.4 mm disk, area **9.079202769 mm²** |
| `gap` (the moved profile's tilt off the exit plane) | **3.9955e-5 mm** |
| allowance | **3.636e-4 mm³** |
| `swept.Volume` | 81.714105065 mm³ (= `area × distance` to the last digit) |
| `swept ∩ divided` at tolerance 0 | **81.714675305 mm³ — +5.702e-4, 1.57× the allowance** |
| the same `common` at `fuzzy = 2 × gap + 1e-7` | **81.714105065 mm³ — exact** |

So the function's own tolerance makes its own check exact, and the failure is that the second
call does not receive it.

The boolean is measuring a real near-tangency, not a dirty input: the prism's far cap is
parallel to the tilted entry face, hence **tilted by 2.35e-5 rad against the block's x = 0
face**. The two planes cross at y ≈ 6; OCCT intersects them and returns an exit cap split into
two pieces (4.540320 mm² on the prism's cap plane + 4.539601 mm² on x = 0) whose total
**9.079922 mm² is 7.19e-4 larger than the disk**, with a result vertex tolerance of
**2.1165e-4 = 9 × tilt**. The volume error scales exactly with the tilt, so no
confusion-scale fuzzy can help:

| `tilt` (rad) | 5e-6 | 1e-5 | 2.35e-5 | 5e-5 | 1e-4 |
| --- | --- | --- | --- | --- | --- |
| volume error (mm³) | +1.213e-4 | +2.426e-4 | +5.702e-4 | +1.213e-3 | +2.427e-3 |
| result vertex tolerance | 4.5e-5 | 9.0e-5 | 2.12e-4 | 4.5e-4 | 9.0e-4 |

**Four checks, four non-findings.**

1. **The boolean call is upstream's.** Upstream's Python `Shape.common` goes through
   `makeElementBoolean` (`TopoShapeExpansion.cpp:6082`): `SetRunParallel(true)`,
   `SetNonDestructive(true)`, `SetFuzzyValue` only for `tolerance > 0`, auto-fuzzy only for a
   negative one — exactly ours. `BRepAlgoAPI_Common` only sets `myOperation`; it does not
   override `Build`. A Rust probe ran the two-shape `BRepAlgoAPI_Common` (upstream's C++
   path) and our `Cut` + `SetOperation(COMMON)` path on the same operands:
   **81.714675305229 for both**, and **81.712824919871 for both** on the exact prism. The seam
   adds no error.
2. **It is not an OCCT 8 regression.** The host also has OCCT **7.9.2**; a standalone C++
   probe against it returns the same **81.714675305229**, the same 4 faces and the same
   **2.1165e-4** vertex tolerance.
3. **No tolerance was inflated on the way in.** Every vertex, edge and face of the block, the
   divided solid, the profile and the prism reports **1e-7**. The inflation appears *inside*
   the boolean, on the result, identically on both kernels.
4. **The prism is exact and nothing else touches it.** `swept.Volume = area × distance` to the
   last digit, its cap plane is parallel to the entry plane, and neither upstream nor this
   seam runs `ShapeFix`/`UnifySameDomain` inside `common` (upstream does not refine a boolean
   unless asked). Our `extrude` passes `Copy/Canonize = true` where upstream's `makePrism`
   passes `false/true`; the copy is bit-identical geometry, and the exact prism result proves
   it.

And the rest of the test is fine. With the stop supplied, `push_pull`'s `through` path extends
the tool 0.1 mm past the exit and the overshoot removes the near-tangent skin: the exit comes
back as **one** face, **86.920797229 mm² (expected 86.920797231) with 2 wires**. Without the
stop it leaves two coplanar exit faces (91.459912 + 4.540320) and `len(exit_faces) == 1`
fails next.

**Classification.** A kernel near-tangency artifact plus a reference-check gap: the sweep
check omits the fuzzy the coverage check has already computed. The fix is one line in
`../simplecad` (`swept.common(shape, fuzzy)`), which this project cannot make — the reference
checkout stays at `9180c4b`. Whether FreeCAD 1.1.3's own kernel (OCCT 7.8.x) is accurate
enough to pass without it is untested here; 7.9.2 is not. Filed as a documented test
assumption, so **P2 has no SimpleCAD failures left**.

### 8.9 Test-run visibility

A full SimpleCAD run is four minutes of silence — the in-process test captures `sys.stdout`
into a `StringIO` and prints the report at the end — so `tools/conformance.py` now writes
two lines per module to **stderr**: one before it starts, one with its totals and elapsed
time after. The stdout report, the summary assertion and `tools/test_conformance.py` are
unchanged. The timings immediately paid for themselves: `test_mesh_glue` is 56 s of the
283 s run (one of its six tests 36 s), and `test_analytic`'s 25 s is 13 separate
converted-mesh rebuilds. Neither is a performance gate; the suite measures correctness.

### 8.10 The 36-second glue test: where the time goes

`test_mesh_glue.test_thin_wall_triangle_profile_and_existing_hole` is **35.7 s**, and
essentially all of it is the *second* half — the hollow-tube split, not the thin-wall box:

| Stage | Time | What it is |
| --- | --- | --- |
| thin box: split + assertions | 1.7 s | 12-triangle box, 8 refine steps, 5 542 facets per piece |
| tube: mesh build | 0.07 s | `makeCylinder(8).cut(makeCylinder(6))` tessellated at 0.15 → **1 008 facets** |
| tube: `Roof.refine` | **12.5 s** | 5 steps of uniform 4× splitting → a **61 488-facet** cap; 504 boundary edges; `height()` is 10.2 s of it |
| tube: `getSelfIntersections` ×2 | **15.3 s** | 7.6 s per **67 032-facet** result, in the debug-built extension |
| tube: `stitch_precision` + `clean_precision` | 2.8 s | conforming the body fans to the refined boundary |
| tube: `Mesh` queries + rebuild | ~1.9 s | `isSolid`/`hasNonManifolds`/`hasNonUniformOrientedFacets` ×2, plus one 67k `Mesh.Mesh(...)` |
| `assert_closed_stl` (8 pieces) | 0.8 s | write + reload + three checks each |

**The seam is 66× the input.** SimpleCAD fixes the roof-profile tolerance at
`min(0.025, depth/16)` = 0.025 mm regardless of the input tessellation (0.15 mm), and the
tube's seam is a **2 mm-wide annulus**: every cap point is within `depth` of a boundary, so
the whole annulus is refined to 0.025 mm — four 4× splits and then `Roof.stitch` fans every
boundary-adjacent clipped triangle along the subdivided chain. That is the reference
algorithm's cost, not a FerroCAD defect, and it is mostly **build-independent**:
`Roof.refine` is pure Python + numpy (12.5 s), and only its `Vector`/`Rotation` calls touch
the extension.

The part that *is* ours is the other half: `getSelfIntersections()` over two 67k-facet
meshes is 15.3 s in a **debug** extension whose grid scan and triangle-pair tests are
exactly what optimisation affects most. A release build would shrink that leg
substantially (the debug build is the only artefact this machine can afford right now),
but it cannot shrink the 67k facets. So: a real algorithmic cost with a debug-multiplied
kernel leg — not a test artefact, and not something to "fix" by loosening an assertion.

The probe is `target/probe_thin.py` / `target/probe_tube.py` (throwaway, not committed).
The candidate improvements, if this ever matters: a `self_intersections` variant that
reports only pairs above a facet index (the caller in `glue.py` filters to the new cap
anyway), and a release-built extension for Python timings.

### 8.11 `test_surface_wrap`: a real error fixed, a reference branch filed

This was the last P3 failure ("wrapped-drawing parameter mapping"). Its module carries one
failure and one error, and they turned out to be different things.

**The error was ours.** `test_changed_faces_measure_the_whole_change_of_area_and_volume`
calls `preserves_volume(box, Part.makeShell(box.Faces[:-1]))`, which cuts a solid by an
**open shell**. OCCT refuses that operand outright — `BOPAlgo_AlertBOPNotAllowed`, null
result — on 7.9.2 as on 8.0.1. Upstream does not check `IsDone()`: `makeElementBoolean`
feeds the null to `makeShapeWithElementMap`, whose `NullShapeException` derives from
`Base::ValueError`, so Python sees a `ValueError`. Our seam raised `Part.OCCError` (not a
`ValueError`), so SimpleCAD's `except ValueError` never caught it and the test errored.
`Shape.fuse`/`cut`/`common`/`multiFuse` now map a not-done or null-input backend error to
`ValueError` and keep `OcctError::Exception` as `Part.OCCError`, which is upstream's split.
Pinned by `a_refused_boolean_raises_value_error_like_upstream`; the module's error count
drops 1 → 0.

**The failure is a reference branch choice.** `test_mouse_coordinates_reach_adjacent_and_opposite_faces`
round-trips `q = (15, 3), (25, 3), (35, 0)` through `frame.point(q)` and
`frame.local(point, face)`. The forward map is right — `point(35, 0)` is `(-10, 5, 0)`, the
far side of the box. The inverse comes back as `(-45, 0)`, exactly 80 mm — one perimeter —
from the expected `(35, 0)`: the same surface point, developed the other way round. Three
measurements locate the choice in the reference, not the seam:

1. The residual `basis @ (trace(q) - p)` projects onto the **target triangle's plane**. On
the opposite face that plane is parallel to the start face, so the last 5 mm of the
correction is a vector *along the target normal* and the projection drops it: the
finite-difference Jacobian at the mouse's previous q has rank **1** (`[[0, 0.05], [0, -0.05]]`),
and `trf`/`dogbox`/`lm` all fail to leave `(25, 3)` for `(35, 0)`.
2. The fallback is the triangle-corridor unfold, whose Dijkstra tree is rooted in the start
face's **first** triangle (`locate` ties both halves of the square at score 0 and takes the
first). That triangle is the y < 0 half, so the -Y route around the box is 29.76 mm of
centre-to-centre steps against 36.4 mm via +Y — the -Y branch wins outright, not by a tie.
3. The guide mesh is the kernel's own: OCCT **7.9.2** and 8.0.1 both mesh the +X face as
nodes `(10,-10,-10), (10,-10,10), (10,10,-10), (10,10,10)` with triangles `(2,1,3)` then
`(2,3,4)` — the y < 0 half first — so `locate` roots the corridor the same way on both.
Every input tolerance is 1e-7; nothing in the boolean or tessellation seam differs.

A branch-consistent inverse would have to prefer, among the verified candidates, the one
nearest the previous q — reference code this project does not modify. Filed as a documented
test assumption (the module's failure count stays 1), so **no SimpleCAD failure is a
FerroCAD defect any more**: 0 P2, 0 P3, 6 documented assumptions.

### 8.12 The error bucket's three seam gaps

Reviewing the twelve errors of §8.11 for API gaps found three that were ours, all
upstream-verified:

1. **`generalFuse`, `childShapes` and `SubShapes` answered bare `Part.Shape`s.** Upstream
   wraps every sub-shape result with `shape2pyshape` (`TopoShapePy::generalFuse`'s per-input
   `modifies` lists, and `getElements` behind `childShapes`/`SubShapes`/`Faces`/…). So a lone
   vertex piece is a `Part.Vertex` and answers `.Point`; ours was a plain `Shape`, and
   SimpleCAD's `slicing.leaves()` walks `childShapes()` to find a sliced-off vertex. All
   three now go through `wrap_shape`; `BOPTools.SplitAPI.slice` needed no change. Pinned by
   `general_fuse_and_child_shapes_return_typed_shapes`.
2. **`Part.Toroid` had no `parameter`.** That is `GeometrySurfacePy::parameter`, which every
   other surface class here already had; `GeomSurface.parameter` is how SimpleCAD's
   `normal_at` reads a torus click point, so `test_measuring`'s saddle-face row errored with
   `AttributeError`. Added as the exact inverse of `value(u, v)` (u around the axis, v around
   the tube, both normalised to `[0, 2π)`). Pinned by
   `a_toroids_parameter_inverts_its_value`.
3. **`BSplineSurface.interpolate` left the surface empty.** Upstream replaces the
   `Geom_BSplineSurface` handle
   (`this->getGeomBSplineSurfacePtr()->setHandle(sur)`), so `value`, `parameter` and
   `UDegree`/`NbUPoles` work afterwards; ours kept only the grid and the knot sequences and
   every query raised *this BSplineSurface has no control points yet*. The interpolated
   geometry is kept now, so `test_surface_line`'s fitted-boundary snap (which calls
   `surface.parameter`) runs. Pinned by `an_interpolated_bspline_surface_answers_queries`.

With those three, SimpleCAD measures **1032 / 6 / 9 / 7** over 88/88 modules. The nine
errors left are the documented face-order assumption, two "would create invalid geometry"
refusals (`test_forming`, `test_loop_faces`), `Mesh.decimate`, a removed object's `Name`
(`test_subdivision`), and the four `PartDesign`-module draft shapes.

### 8.13 The Draft round: a TechDraw page, a green wire module, and the DXF boundary

The Draft remainder was measured rather than assumed, and it is smaller than the backlog
said:

1. **`test_draftgeomutils` was already green (10/0/0/0).** The backlog's `6/4/0` predated
the seam rounds: `Part.Wire(list)` ordering, `Shape.copy` keeping its subclass and
`Edge.firstVertex`/`Wire.OrderedVertexes` following `WireExplorer` closed the
`get_extended_wire3/4/7/8` failures. No new code; the module joined `DRAFT_FILES`, so it
is now a gate instead of a neighbour. The `--draft` group is **60/0/0/1 over eleven
files**.
2. **`TechDraw::DrawPage` is a document stub, and that is all the test reads.**
`crates/ferrocad_part_py/src/techdraw.rs` registers `TechDraw::DrawPage`
(`Template` link, `Views` link list), `TechDraw::DrawSVGTemplate` (`Template` string) and
`TechDraw::DrawViewDraft` (`Source` link, `Direction` vector) from the Part kernel
bootstrap, beside `Sketcher::SketchObject` and `PartDesign::Body`. `ferrocad_py` gained
the generic `DocumentObject.addView` (append to a `Views` link list, `AttributeError` for
a type without one) and `OutList` is now **every outbound link**, not just a group's
`Group` — which is what `view in page.OutList` asserts. Nothing draws, projects or writes
SVG: FerroCAD still has no TechDraw workbench. `test_modification` is **32/0/1 of 33**
(was 31/0/2); the only error left is `test_draft_to_sketch`'s missing `Sketcher` module,
which P4 SK3 owns. Pinned by `crates/ferrocad_part_py/tests/part_techdraw_page.rs`
(page/`OutList`/`InList`/`addView`, plus the rejected non-page type).
3. **`Import.readDXF` is recorded as out of surface, not stubbed.** The draft test no
longer fails on `getHomePath`: it reaches `import Import` at `importDXF.py:2915`. The
blocker is not the ASCII DXF grammar — one `LINE` in the fixture — but the **object model
the importer is contracted to produce**. `importDXF._import_dxf_file` snapshots
`doc.Objects`, lets `Import.readDXF` create objects, then runs `DxfDraftPostProcessor`
over the difference. That post-processor reads `Part::Line`/`Part::Circle`/`Part::Vertex`
intermediates, `App::FeaturePython` placeholders carrying `DxfEntityType`, block
`Part::Compound`s under `_BlockDefinitions`, and layer groups whose proxy answers
`utils.get_type(...) == "Layer"`; the test's `len(doc.Objects) == 3` (Layers container,
layer `0`, one line) is only meaningful if that model is real. FerroCAD registers none of
those intermediate types. A Python module that emitted three objects would measure the
stub, not DXF support, so `test_dxf` stays out of `DRAFT_FILES` and the gap is named here
and in the backlog.

The contract re-pin that follows is mechanical: `contracts/part-api.json` gains the three
TechDraw types (object types 40 → 43), and the method coverage is unchanged at
**253/408 (62.0%)**.

Fresh measurements for the round, same image throughout (upstream @ `33adf61`):

| Measure | Value |
| --- | --- |
| `--draft` group (eleven files) | **60 passed, 0 failed, 0 errored, 1 skipped** |
| `test_modification` | **32/0/1** of 33 |
| `test_draftgeomutils` | **10/0/0/0** |
| `test_dxf` | **1/0/1** (`Import.readDXF`, out of surface) |
| Part conformance | **531/0/0/12** |
| Python `base` | **707 / 0 / 0 / 8** |
| Part API audit | **253/408 (62.0%)**, contract re-pinned for the TechDraw types |

### 8.14 The four PartDesign errors: a draft, not a boolean

The error bucket's note said "4 `PartDesign`-module draft shapes". Reading the records was
worth it: all four were `ModuleNotFoundError: No module named 'PartDesign'`, raised at
`import PartDesign` (`simplecad/surfaces/offset.py:248`). `PartDesign::Boolean` is not
touched by any of them. The draft tool's private document builds a `PartDesign::Body`, a
`PartDesign::Plane` hinge and a `PartDesign::Draft`, and reads the feature's `Shape`.

Upstream's `PartDesign::Draft` is a general dress-up (`BRepOffsetAPI_DraftAngle`): tilt each
selected face about its intersection with the neutral plane, heal the neighbours. The pinned
`ferrocad_occt_cxx` does not bind that class, and adding it would mean patching an external
crate. So the feature was implemented for the class the tool actually asks for, and refuses
the rest by name:

- one solid that is a straight prism along the neutral plane's normal;
- every side wall selected (flat walls, or one cylindrical wall), all parallel to the pull;
- exactly two end caps square to the pull, with the neutral plane through one of them.

The drafted body is then the hinge cap swept to the far cap with a taper — the existing
tapered-extrusion construction (`BRepOffsetAPI_MakeOffset` + loft, now `extrusion::
draft_section`). It is exact for that class: the box case's far cap measures
333.070775677 mm² against `(20 − 2·10·tan 5°)²`, the cylinder wall comes back as a real
`Part.Cone` with the frustum volume, and `Reversed` is SimpleCAD's `narrower = 'bottom'`.
`PartDesign::Plane` is a square marker derived from its `Placement` (the hinge the draft
reads), and `python/PartDesign.py` is the import shim.

Pinned by `crates/ferrocad_part_py/tests/partdesign_draft.rs` (narrowing, widening, the cone,
the datum plane, a cap selected for drafting, and the `PartDesign` import in a linked host).

A build note that cost a run: the SimpleCAD harness registers `Part` as a **built-in**, and
the three `test_surface_offset` cases call `offset.draft` in the harness process itself (only
the `surface_ops` path goes through the shape worker). Rebuilding `ferrocad_combined.abi3.so`
is therefore not enough to move those three; the harness test binary has to be rebuilt too.

The measurements, with the harness rebuilt after the change:

| Measure | Before | After |
| --- | --- | --- |
| `test_surface_offset` | 19/0/3/1 | **22/0/0/1** |
| `test_surface_ops` | 15/0/1/0 | **16/0/0/0** |
| SimpleCAD integrated | 1032/6/9/7 | **1036/6/5/7** over 88/88 |
| Part conformance | 531/0/0/12 | **531/0/0/12** |
| Python `base` | 707/0/0/8 | **707/0/0/8** |
| Rust lib tests | `ferrocad_part` 59, `part_py` 360 | **`ferrocad_part` 59, `part_py` 366**, 0 failed |
| Part API audit | 43 object types | **45 object types**, 253/408 (62.0%) |

The five errors left are `Mesh.decimate`, a removed object's `Name`, the two
"would create invalid geometry" refusals and the chamfered face-order assumption. None needs
a PartDesign workbench.

### 8.15 P3 opens: `Mesh::FeaturePython`, the measured add-on surface, and two errors

P3's first slice was the freeform host. The ideation note had already identified the one
missing piece for the closest worked answer to a freeform skin: the SubD workbench stores
its cage on a **`Mesh::FeaturePython`**, which we did not register. That type now exists
with the same `Mesh`/`Placement` pair and `MeshFeature::onChanged` mirror as `Mesh::Feature`,
and `execute` records a request for the binding (`Document::mark_python_pending`), so a
proxy runs exactly as it does for `Part::FeaturePython`.

**The call surface was synthesized and measured, not assumed.** Neither add-on is in this
checkout, so `tests/test_p3_addon_surface.py` runs the calls their object model documents
against one `ferrocad` + `Part` + `Mesh` image: a cage on the new type (`CagePoints` +
`CageFaces`, preview assigned in `execute`), each quad as a bicubic
`Part.BSplineSurface.buildFromPolesMultsKnots(...).toShape()`, a six-patch `Part.Shell` +
`sewShape` + `Part.Solid` skin (12 edges, 0 free, volume 1.0), a `Part::FeaturePython`
wrapper that follows the cage's `Placement`, and Curves' `interpolate` + `Part.Wire` +
`Part.makeFilledFace`. Every call is answered. The measurement and its caveats are in
[`research/p3-addon-surface-spike.md`](../research/p3-addon-surface-spike.md).

The one call that was missing is **`Shape.connectEdgesToWires`**
(`ShapeAnalysis_FreeBounds::ConnectEdgesToWires`, returning a compound of wires); SimpleCAD
reaches for it when two coplanar faces' sides are only geometrically coincident. It is now
implemented and pinned by `connect_edges_to_wires_links_a_boundary`.

**Two SimpleCAD errors closed on the way, and both were seam rules rather than surface work:**

1. **A cavity solid rebuilt from its own shells.** Upstream's `makeElementSolid` adds every
   shell with `BRepBuilderAPI_MakeSolid::Add` and then calls `BRepLib::OrientClosedSolid`;
   it never reverses them, because a cavity shell taken from a cut already carries the
   reversed orientation. We reversed everything after the first shell, so `test_forming`'s
   stretched box-with-a-sphere-cavity rebuilt a solid of 1000 + 33.5 mm³ that `BRepCheck`
   rejected. `make_hollow_solid` now adds as given; the caller reverses an independently
   built inner shell, which `Part.Solid.add` (our convenience, upstream has none) documents.
   Pinned by `hollow_solids_keep_the_cavity_the_shells_carry` and
   `a_part_solid_accepts_cavity_shells`.
2. **A removed object was still valid.** `DocumentObject.isValid()` read `Shape` first, and
   a deleted wrapper's `Shape` raised, whose `else` branch returned `true`. Upstream reports
   `false` for a removed object, and SimpleCAD's rotate guard tests `not obj.isValid()`
   before reading `.Name`; our `true` sent it on to the `ReferenceError`. The attachment is
   checked first now, so `test_subdivision`'s stale-operation guard short-circuits.

The measurements, with the extension and the SimpleCAD harness rebuilt after the change:

| Measure | Before | After |
| --- | --- | --- |
| SimpleCAD integrated | 1036/6/5/7 | **1038/6/3/7** over 88/88 |
| `test_forming` | 16/0/1/0 | **17/0/0/0** |
| `test_subdivision` | 14/0/1/0 | **15/0/0/0** |
| Part conformance | 531/0/0/12 | **531/0/0/12** |
| Rust lib tests | `ferrocad_occt` 63, `ferrocad_mesh` 82, `part_py` 366 | **`ferrocad_occt` 63, `ferrocad_mesh` 82, `part_py` 368**, 0 failed |
| Part API audit | 253/408 (62.0%) | **254/408 (62.3%)**: `connectEdgesToWires` |
| Python sets | `base` 707/0/0/8, `workbenches` 789/0/0/3 | **`base` 707/0/0/8, `workbenches` 794/0/0/3** |

The three errors left are `Mesh.decimate` (a decimator is a new algorithm, not a seam fix),
`test_loop_faces`'s twisted-quad merge (the reference's merge accepts a candidate on area
and normal without checking validity, so it returns a face `BRepCheck` rejects; every kernel
call is identical), and the chamfered face-order assumption.

## 9. Measurements at close

Upstream `../freecad-upstream` @ `33adf61`; SimpleCAD `../simplecad` @ `9180c4b` (v0.38.0),
`git status` empty. Built with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=2`; the extension was rebuilt before the Python/SimpleCAD runs.

| Measure | Value | Command |
| --- | --- | --- |
| Part conformance | **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures** | `FERROCAD_UPSTREAM=… cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` |
| SimpleCAD integrated | **1032 passed, 6 failed, 9 errored, 7 skipped, 88/88 modules** | `FERROCAD_SIMPLECAD=$PWD/../simplecad cargo test -p ferrocad_part_py --test simplecad_conformance -- --nocapture` |
| Part API audit | **253/408 (62.0%)**, contract re-pinned for `Cylinder.toShape` | `python3 tools/part_api_audit.py --check` |
| Rust lib tests | `ferrocad_occt` **82**, `ferrocad_part` **59**, `ferrocad_core` **137**, `ferrocad_mesh` **64**, `part_py` **355**, `part_shape_ops` **21**, all 0 failed | per-crate `cargo test` |
| Python extension sets | `base` **707 / 0 / 0 / 8**; `workbenches` **789 / 0 / 0 / 3** | `python3 tools/run_tests.py --extension base` / `workbenches` |
| Docs/tooling | `api_docs` 9 pages, `userdocs` 8, `build_book` 48 chapters, extension-set contract 50 modules, `test_docs_tooling` 49 | the `--check` gates |

Two runs of the full SimpleCAD suite before the edge-placement fix produced the same
1001/29/17/7, so that figure was stable rather than a lucky pass; the post-fix runs are
1003/27/17/7, 1005/25/17/7 and then 1006/24/17/7, with the rotate-edges, elongating and
boss records closed and the error set unchanged in size (one analytic error left, now
filed as a face-order assumption). The signature/tessellation/cutHoles batch then measured
**1013/20/14/7**, the unwrap fix **1014/20/13/7**, the undo/label round **1019/15/13/7**,
the query/tolerance round **1026/8/13/7**, the placement round **1028/6/13/7**, the
refused-boolean round **1029/6/12/7**, and the typed-shape/surface round **1032/6/9/7**, all
over 88/88 modules; no run moved a module from load-failure to loaded.

## 10. What the 6 failures and 9 errors are

The failed records are re-bucketed in [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P2;
in short: **no P2 and no P3 failure is left**, and all **6 are documented test assumptions**
(the exact `BoundBox` precondition, the two `close_holes` cases and `test_rounding`'s
erase-then-refuse sample — that same policy — plus `test_precision`'s tilted push and
`test_surface_wrap`'s inverse branch, which are reference-side checks, §8.8 and §8.11). The
**9 errors** are 4 `PartDesign`-module draft shapes (3 in `test_surface_offset`, 1 in
`test_surface_ops`), `Mesh.decimate`, a removed object's `Name` (`test_subdivision`), two
`Shape.isValid()` refusals (`test_forming`, `test_loop_faces`) and **1 documented face-order
assumption** (the chamfered single-face selection, which is an error rather than a failure).
The `test_measuring`, `test_slicing` and `test_surface_line` errors were the three seam gaps
of §8.12 and are fixed.

The P2 seam list is empty, and so is P3's; the identity, undo/label and query/tolerance
buckets are empty too. Both remaining "kernel" cases were traced to the reference, not the
seam (§8.8, §8.11).
