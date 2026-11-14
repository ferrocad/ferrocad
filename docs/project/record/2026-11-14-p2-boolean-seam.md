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

SimpleCAD's integrated run went **985 / 49 / 18 / 7 → 1019 / 15 / 13 / 7 over 88/88
modules**: **34 failures closed, none added, and the errors moved 18 → 13**. The printed
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

## 9. Measurements at close

Upstream `../freecad-upstream` @ `33adf61`; SimpleCAD `../simplecad` @ `9180c4b` (v0.38.0),
`git status` empty. Built with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=2`; the extension was rebuilt before the Python/SimpleCAD runs.

| Measure | Value | Command |
| --- | --- | --- |
| Part conformance | **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures** | `FERROCAD_UPSTREAM=… cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` |
| SimpleCAD integrated | **1019 passed, 15 failed, 13 errored, 7 skipped, 88/88 modules** | `FERROCAD_SIMPLECAD=$PWD/../simplecad cargo test -p ferrocad_part_py --test simplecad_conformance -- --nocapture` |
| Part API audit | **253/408 (62.0%)**, no contract re-pin needed | `python3 tools/part_api_audit.py --check` |
| Rust lib tests | `ferrocad_occt` **82**, `ferrocad_part` **59**, `ferrocad_core` **108**, `ferrocad_mesh` **63**, `part_boot` **3**, `part_shape_ops` **17**, all 0 failed | per-crate `cargo test` |
| Python extension sets | `base` **707 / 0 / 0 / 8**; `workbenches` **789 / 0 / 0 / 3** | `python3 tools/run_tests.py --extension base` / `workbenches` |
| Docs/tooling | `api_docs` 9 pages, `userdocs` 8, `build_book` 48 chapters, extension-set contract 50 modules, `test_docs_tooling` 49 | the `--check` gates |

Two runs of the full SimpleCAD suite before the edge-placement fix produced the same
1001/29/17/7, so that figure was stable rather than a lucky pass; the post-fix runs are
1003/27/17/7, 1005/25/17/7 and then 1006/24/17/7, with the rotate-edges, elongating and
boss records closed and the error set unchanged in size (one analytic error left, now
filed as a face-order assumption). The signature/tessellation/cutHoles batch then measured
**1013/20/14/7** and the unwrap fix **1014/20/13/7**, then the undo/label round
**1019/15/13/7**, all over 88/88 modules; no run moved a module from load-failure to loaded.

## 10. What the 15 failures and 13 errors are

The failed records are re-bucketed in [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P2;
in short: **11 → P2** (boolean/topology 1, query/tolerance/determinism 9, identity 1),
**1 → P3** (wrapped-drawing parameter mapping) and **3 documented test assumptions** (the
exact `BoundBox` precondition and the two `close_holes` cases). The 13 errors are 4
`test_surface_offset` draft shapes, 1 wrapped-drawing area measurement, 7 behavioural/API
results (forming, loop faces, measuring, mesh conversion, slicing, subdivision, the surface
snap) and **1 documented face-order assumption** (the chamfered single-face selection, which
is an error rather than a failure).

The undo/label bucket is empty: the transaction path is closed. What remains is the
query/tolerance group and one sub-tolerance push/pull, both of which need a reproduction
before they need code.
