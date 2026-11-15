# The sketch model round — 2026-11-15

Status: round record. This is the measurement of the P4 SK1-lite/SK3 work that closed
`Draft.make_sketch`. The backlog is [`../TODO-15-11-2026.md`](../TODO-15-11-2026.md) §P4;
the stable contract is
[`../../developer/workbenches/sketch-constraints.md`](../../developer/workbenches/sketch-constraints.md);
the failing-test map it touches is
[`../../developer/workbenches/draft-tools.md`](../../developer/workbenches/draft-tools.md).

## 1. What the round established

| Step | Change | Measured |
| --- | --- | --- |
| **The constraint model** | `Sketcher::SketchObject` stores typed records in a hidden `ConstraintData` `StringList`; `python/Sketcher` ships `Constraint`/`PointPos`; `addConstraint`/`delConstraint`/`setDatum`/`getDatum`/`solve`/`GeometryCount`/`ConstraintCount`/`Constraints`/`RedundantConstraints` and the four autoconstraint calls are installed on `ferrocad.DocumentObject` | `part_sketcher` **13/13**; `tests/test_sketch_constraints.py` **22/22** |
| **`Shape.findPlane` on a wire** | a shape with no planar face takes its normal from the first pair of non-parallel edge tangents, as upstream's `findPlane` answers for wires and edges | `test_modification` moved from a load-blocked `make_sketch` to green |
| **`Part.LineSegment(curve, first, last)`** | the trimmed-piece constructor Draft's `orientEdge` calls, plus the one-argument copy form | same |
| **An angle as a `Quantity`** | `Quantity.__rmul__`/`__mul__` accept a plain factor; `Shape.rotate`/`rotated` convert an angle quantity through `getValueAs("deg")` | exposed the unit bug below |
| **`App::PropertyAngle` is degrees** | the property default was a **millimetre** quantity, so `obj.Rotation` read as `45 mm`; it is a degree quantity now, which is the signature the `.FCStd` writer already infers | `test_hatch` **green** again |
| **Draft gate** | `test_modification` joined `DRAFT_FILES` | `--draft` **93/0/0/1** (was 60/0/0/1) |

Part conformance stayed **531 / 0 / 0 / 12**, the `base` extension set **707 / 0 / 0 / 8**,
and `workbenches` went **789 → 845** cases with 3 skips and no failures.

## 2. Three root causes, all in the seam

`Draft.make_sketch` failed in three places, and each was fixed where the missing behaviour
belongs rather than in the Draft shim:

1. **`Shape.findPlane()` only knew planar faces.** Draft asks a *wire* for its plane,
   because that is what a Draft object's `Shape` is. `planar_face_normal` returns `None`
   without a surface, so `find_plane` fell through to `None` and `make_sketch` reported
   "All shapes must be planar". A wire or an edge now derives the normal from its edge
   tangents; a single straight edge still answers `None`, which is upstream's answer too.
2. **`Part.LineSegment` had only `(start, end)`.** `orientEdge` builds every oriented line
   as `Part.LineSegment(edge.Curve, edge.FirstParameter, edge.LastParameter)`, i.e. from
   the edge's basis line and its parameter range. The constructor now accepts that form
   (and the copy form) and trims through `EdgeCurve::trimmed`.
3. **An angle quantity could not be built or consumed.** `DraftVecUtils.angle(...) *
   App.Units.Radian` multiplies a float by a quantity, which raised `TypeError`; once
   multiplication worked, `Shape.rotate` had to read the quantity in degrees.

## 3. The unit bug the third cause exposed

`App::PropertyAngle` was seeded with a millimetre quantity — every angle-valued property in
a document read back as `<value> mm`. Nothing noticed, because `float(quantity)` returns the
numeric value in whatever unit the quantity carries, and 45 mm and 45° have the same
number. Converting through `getValueAs("deg")` made the mislabel observable:
`Draft.make_hatch` rotated its pattern by 2578° and its bounding box check failed.

The fix is at the registry: `App::PropertyAngle` seeds `Quantity::new(0.0, deg)`. That is
also the signature the `.FCStd` writer already uses to choose `App::PropertyAngle` on the
way out, so a document now round-trips an angle as an angle in both directions.

## 4. What the model does and does not do

It stores a constraint, reports it back as a `Sketcher.Constraint`, edits a datum, deletes
by index, detects the coincident/axis relations a wire already satisfies, and checks that
every constraint names a geometry or root that exists.

It does **not** solve. `solve()` returns `0` when the references resolve and `-1` when one
dangles; geometry never moves, so a datum that the geometry does not satisfy still reports
`0`. `RedundantConstraints` is empty because there is no rank analysis to report. Those are
the documented approximations at the call sites; P4 SK2 (the PlanGCS adapter) is what
replaces them.

## 5. Measurement at close

Upstream `../freecad-upstream` @ `33adf61`; `../simplecad` untouched at `9180c4b`. Built with
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`; the combined extension
was rebuilt before the Python and Draft runs.

| Measure | Value | Command |
| --- | --- | --- |
| Part conformance | **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures** | `FERROCAD_UPSTREAM=… cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` |
| Draft gate | **93 passed, 0 failed, 0 errored, 1 skipped, 0 load failures** | `python3 tools/conformance.py --root ../freecad-upstream --draft` |
| `test_modification` | **33 / 0 / 0 / 0** | `--files test_modification` |
| Sketch Rust tests | `part_sketcher` **13 / 13** | `cargo test -p ferrocad_part_py --test part_sketcher` |
| Sketch Python tests | **22 / 22** | `python3 tools/run_tests.py --extension workbenches` |
| Python extension sets | `base` **707 / 0 / 0 / 8**; `workbenches` **845 / 0 / 0 / 3** | `--extension base` / `workbenches` |
| Core/py unit tests | `ferrocad_core` + `ferrocad_py` all green (108 + 41 across suites) | `cargo test -p ferrocad_core -p ferrocad_py` |

The extension-set contract gained exactly one module (`test_sketch_constraints`, 42 base /
11 workbenches) via `python3 tools/run_tests.py --write-contract`.

## 6. The identity and construction round (same day, P4 SK1)

SK1's missing half was identity: a constraint addressed a curve by its index in `Geometry`,
so deleting an earlier curve rebound it to a different one, and construction geometry —
stored in a separate list — was not addressable at all.

**What landed.** Each geometry element now carries a stable id (`GeometryIds`, a
`StringList`) and a construction flag (`Construction`, a `BoolList`), both index-aligned
with `Geometry`. A constraint record stores `#g3`, not `3`; the Python surface still speaks
upstream's positional GeoId and translates at the boundary. `delGeometry(index)` removes the
element, drops the constraints that named its id, and leaves the rest resolving — their
positions shift, their references do not. `addGeometry(geo, construction=False)`,
`toggleConstruction(index)` and `getConstruction(index)` are the construction surface;
`Shape`/`InternalShape` build from the non-construction entries only, while `GeometryCount`
counts all of them. `-1` and `-2` are the root axes in `addConstraint`, `solve` and the
detection (an endpoint on the origin stages `Coincident(-1, start, …)`); external geometry
(`-3` and below) is refused by name.

**Two things this exposed.** `Sketcher.Constraint` had been defaulting unused references to
`0`, which is a real geometry index; they default to `GeoUndef` (`-2000`) now, as upstream's
`GeoEnum` does. And a document object's `__setattr__` turns an unknown name into a Python
instance attribute, so a write to a pre-identity sketch's missing `GeometryIds` would have
been silently lost — such a sketch gains the properties through `addProperty` on first use.

**Measured.** `part_sketcher` **13 → 16** cases, `tests/test_sketch_constraints.py`
**22 → 29** (including a `.FCStd` save/reload that checks the ids, the construction flags,
the constraints and a root reference), `workbenches` **845 → 852** cases, the Draft gate
unchanged at **93/0/0/1**, and Part conformance unchanged at **531/0/0/12**.

## 7. The solver round (P4 SK2)

SK2 was planned as a binding to upstream's PlanGCS. It became a **pure-Rust solver**
instead:

| | PlanGCS | `ferrocad_sketch_solver` |
| --- | --- | --- |
| Dependencies | C++ toolchain, Eigen headers, a Boost graph header | `ferrocad_curves` |
| `target/` | the Eigen/Boost closure | a single small `rlib` |
| Availability | not in the sparse upstream checkout this repo measures | in the workspace |
| Licence | LGPL-2.1+ (compatible, but another obligation to track) | the workspace's own |
| Contract | any | behaviour is the contract: `solve()`, DOF, redundancy, conflicts |

No contract gate required PlanGCS. The crate parameterizes points, line segments,
circles and arcs — the parameterization PlanGCS uses — builds one residual per constraint
from values that carry exact first derivatives, iterates Levenberg-Marquardt with a dense
Cholesky solve, and reports `DOF = parameters − rank(J)` from a Gram-Schmidt analysis in
constraint order.

**What changed for a caller.** `solve()` now moves geometry and returns the remaining
degrees of freedom (upstream's convention: `0` is fully constrained); `-1` still means
"not solved" and now also covers an unconverged system, and nothing is written in that
case. `RedundantConstraints` reports real 1-based indices, and `ConflictingConstraints`
joins it: a conflict is diagnosed by removing one constraint at a time, so a single bad
datum names itself instead of dragging a dozen rows off. `addConstraint` runs the solver's
own residual code, so a `Radius` on a line is refused at that point rather than ignored by
the next solve. `SketchObject.execute` solves too, which is what keeps a recomputed sketch
solved the way upstream's does.

**Where it stops.** A first-order method cannot start from a configuration whose Jacobian
is rank-deficient along the needed direction (a horizontal line given `Vertical` *and* a
length datum); that reports `-1` with both constraints named. Ellipses and splines are
stored, drawn and constrained-never: they are carried through the solve untouched. `Block`
pins at the position the solve started from, which makes it idempotent rather than a stored
reference point. All three are in the contract page.

**Islands, and the conflict diagnosis that needed them.** The solve is per connected
component: a constraint couples only the geometry it names, and a root-axis reference
couples nothing. Three tiers fall out — an unconstrained island is answered from its
parameter count, an already-satisfied one skips the iteration but keeps the rank analysis,
an active one solves and diagnoses. The split is not only a speed measure: leave-one-out
over a whole sketch finds nothing when two *independent* islands are broken, because
removing one constraint still leaves the other unconverged. Per island, the diagnosis also
filters candidates first (rank-dependent cycle, or violated at the best fit), so the
leave-one-out only runs on constraints that could possibly be responsible.

**The drag seam.** `SoftPrior` + `solve_with_priors` add weighted rows pulling a geometry
point toward a target. They are penalties, not constraints: they never enter the
convergence test or the rank analysis, so a prior steers the free degrees of freedom
instead of fighting a hard constraint — dragging a pinned point moves nothing and reports
nothing. One prior-free pass runs afterwards to restore hard feasibility, so the result is
a hard-feasible point near the prior's constrained optimum rather than an exact QP
solution. A prior on a root axis is refused. §8 gives it a Python consumer.

**Measured.** `ferrocad_sketch_solver` **14** unit cases (four for the island split, the
tier fast paths and the prior seam); `part_sketcher` **16** (the DOF assertions replaced
the old `solve() == 0`); `tests/test_sketch_constraints.py` **29 → 36** with the end-to-end
solve cases (rectangle, datums, circle radius, construction-driven profile, redundancy,
conflict refusal); `workbenches` **852 → 859**; the Draft gate still **93/0/0/1** (with
`make_sketch` now solving for real) and Part still **531/0/0/12**.

## 8. The interactive half (P4 SK6 drag, SK4 and SK5)

**The drag seam gets its Python consumer.** `SketchObject.drag(geometry, pos, target)`
solves with one `SoftPrior`, writes the moved geometry back, and returns the remaining
degrees of freedom. The contract is the prior's, not a constraint's:

- `dof` and `RedundantConstraints` are the same as `solve()` reports, because the prior
  rows never enter the rank analysis;
- a hard constraint wins — a pinned point reports `0` and does not move, where a hard
  constraint would have reported a conflict;
- a root axis raises `ValueError`, a missing index raises `IndexError`, and a system the
  solver cannot build or converge returns `-1` with `Geometry` untouched.

One shared write-back now skips the property write when the solve changed nothing, so a
drag that loses to the constraints does not dirty the document.

**Drawing on a workplane.** The parametric edition contributes the stage's
`PolylineTool` under the id `polyline`, and its app script registers `Draft_Wire` (whose
`Activated()` returns `{"tool": "polyline"}`). Four snapped clicks on the tilted plane
`x = 5` finish a **creating** commit; the edition's hook builds, in one named
transaction, a parametric `Part::FeaturePython` wire (`Points`, `Closed`, `Thickness`)
and the `Part::Extrusion` that links it. Every vertex survives the tool to f32
precision, and one undo removes both objects.

**The pad.** The extrusion's `Dir` is the profile's own plane normal (the first three
non-collinear points), so a profile on a tilted plane extrudes perpendicular to itself;
`Solid = True` and `DirMode = "Custom"`. Editing the wire's `Points`/`Closed` rebuilds
it on the next recompute and the linked extrusion follows — SK5's acceptance. A profile
the kernel refuses keeps the wire and drops only the pad.

**The second rail: a sketch instead of a wire.** The tool itself is generic (it collects
snapped points and commits), so intent has to travel in the commit. `TaggedPolyline`
wraps `PolylineTool` and rewrites `commit.manipulator` on `finish()`, and the edition
registers the same tool under two ids: `polyline` for `Part_Polyline` (the wire above)
and `sketch_polyline` for `Sketch_Polyline`. The sketch hook projects the world points
onto the plane their first three non-collinear members define, builds the sketch in that
local `(u, v)`, and puts the plane back where the drawing was with
`Placement(origin, Rotation(x, y, z, "ZXY"))` — so a profile drawn **on a face stays on
the face**, and "horizontal" in the autoconstraints means horizontal in the sketch.
The profile is closed, one `Part.LineSegment` per edge, and the coincidences and the
axis relations come from the sketcher's own two-phase detect/make pair. The linked
`Part::Extrusion` is created in the same named transaction, so undo takes the sketch and
the pad together; a pad the kernel refuses is removed and the sketch kept.

**The dimension, and with it the loop.** `Sketch_Dimension` reads the selected sketch
edge and adds one *driving* datum **at the value the edge already has** — `Radius` for a
`Part::GeomCircle`, otherwise the endpoint `Distance` — so adding it moves nothing. The
chain is then complete end to end: draw → constrain → dimension → `setDatum` → the solid
changes on the next recompute. The numbers a closed four-point profile pins down are
worth stating because they are not guesses: **10 constraints** (4 shared corners, **2
origin pairs** — the autodetector stages the origin coincidence for *both* segments
meeting at the first point — and 2 `Horizontal` + 2 `Vertical`), `solve() == 2`,
`RedundantConstraints == [6]`, the datum at index `10`, and `setDatum(10, 20.0)` moving
the pad's `YLength` from 10 to 20. The second origin pair is redundant *in rank*, not in
intent: that is reported rather than suppressed. `Part_Polyline` keeps its own id and
its own path, so the two creation commands cannot collide — and `Draft_Wire` still
belongs to the scripting facade, which is why the new ids live in a `Sketch_` namespace.

**Measured.** `part_sketcher` **18/18** (two drag cases);
`tests/test_sketch_constraints.py` **40/40** (four drag cases); the edition's
`polyline_round_trip` **1/1** through the real tool and `Part::Extrusion`; the sketch rail
`sketch_pad_round_trip` **1/1** (draw, dimension, `setDatum`, two undos); the bin's
registry test **1/1** (both tool ids present); the rest of the parametric suite unchanged
(9 + 11 + 1 + 3 + 2 + 1 + 1 + 1 + 2). The Python `workbenches` set is **863 / 0 / 0 / 3**
over 53 modules (11 workbench modules); it does not drive the new commands yet, which is
the next widening. The
Part API audit contract is re-pinned for the installed `drag` method (the audit tracks
`installed_as_methods`).

## 9. The workplane state contract (2026-11-15)

The state refactor that landed on 2026-10-29 was correct and left one ambiguous door open.
`ActiveWorkplane` has no absence — the world plane is the default and is present from the
first frame — but `Viewport::set_workplane(Option<FaceFrame>)` still accepted `None`,
which the implementation read as "the world plane" and every caller read as "no plane".
The workplane task spec had recorded exactly that as the state problem
([`../tasks/workplanes.md`](../tasks/workplanes.md) §1).

**What closed it.**

- **The `Option` entry point is deleted.** `set_active_workplane(ActiveWorkplane)` is the
  only setter. Tier 1's first press, `activate_hovered_workplane` and
  `set_workplane_from_element` each bind `ActiveWorkplane::of(&frame)`;
  `clear_workplane()` binds `ActiveWorkplane::world()`. So "no active workplane" is not
  expressible, and a consumer reads the plane without an `Option`. No other crate named the
  removed method: the shell uses the verb-level `activate_hovered_workplane` /
  `clear_workplane`.
- **`is_default()` compares the frame.** It was the offset, the label and
  `kind == World`; it is now also `plane == Workplane::world()`. The pill's highlight and
  Tier 1's bind-or-not decision both read this test, so a frame that merely *reads* `XY`
  (a shifted origin) must not count as the default.
- **The contract is written once.** §1.2 of the task spec is now a table of the five ways
  in (world standard, face, datum, custom/view, offset) with the `kind`, the label,
  `is_default()`, `extent` and the constructor for each, plus the five rules a consumer may
  rely on — including that the label is presentation and the kind is the truth, and that
  `extent` is the grid's rule rather than the plane's identity.

**Measured.** `ferrocad_viewport` **235 passed / 0 failed / 1 ignored** (the extended
`resetting_the_default_plane_is_a_no_op` asserts that a reset leaves a world *binding*, and
that a shifted-but-`XY` frame is not the default); `ferrocad_shell_core` **289/0**;
`ferrocad_toy` **18/0** plus its five round-trip suites; `cargo check -p
ferrocad_shell_core -p ferrocad_toy --all-targets` clean.

**What is still open, and is not a state question.** `SketchObject.drag` has no
manipulator in front of it, and the Python suite does not drive `Sketch_Polyline` /
`Sketch_Dimension` yet. Both are new surface on top of a contract that is now closed.

## 10. The vertex grips (P4 SK6, interactive)

The last piece the sketch rail was missing: `SketchObject.drag` behind a handle you can
actually grab. The design question was where the handle lives, because a sketch grip
needs two things a compiled Rust factory cannot see — the sketch's **plane** and its
**vertex list** — both of which are document state, not stage state.

**The answer is a declared plane handle.** `ferrocad_stage::DeclarativeKind` gained a
third kind beside `Axis` and `Ring`:

| Kind | Value | Drag |
| --- | --- | --- |
| `Axis` | one world unit along `axis` | projected axis travel |
| `Ring` | degrees about `axis` | ray/ring-plane angle |
| **`Plane`** | **two** world units, `[value, value_v]` | ray intersects the handle's own `u`/`v` plane; each coordinate moves by the frame-local travel since the press |

A plane handle's `axis` names its **normal**, its mesh sits at
`origin + u·value + v·value_v` (so the handle *is* the point it edits), and it publishes
no numeric row: two unlabelled millimetres would be a worse interface than the drag it
already has, so the readout carries both numbers instead. A pointer ray that cannot meet
the plane (an edge-on view) refuses the press rather than inventing a coordinate. The
transport accepts `"kind": "plane"` and `"value_v"`.

**The provider.** The edition's `manipulator_specs` returns one grip per **topological
vertex** of `sketch.Shape` — `Vertex1..VertexN`, so a corner shared by two segments is one
grip and the name is the one a selection reports. Each spec's frame is the sketch's own
(`Placement.Rotation` applied to local X/Y, `z` = the normal) with the origin on the
vertex. The commit is `sketch_drag` / `drag`, `element = "VertexN"`, `values = [u, v]`.

**The edit.** `VertexN` resolves to the nearest geometry endpoint (or a circle's centre)
through `Placement.inverse()`, and `sketch.drag(index, pos, start + (u, v))` runs in one
named transaction, so the constraints decide where the point lands and the linked
`Part::Extrusion` follows on the same recompute.

**A seam bug this exposed.** The declarative `manipulator_specs` bridge handed providers
the selection's *name*, while `manipulator_probe` and the `interaction` half hand the
**document object**. Any provider that read `obj.TypeId` was therefore silently inert —
including the existing applied-fillet round grip. The bridge now resolves the name, and a
selection with no live object is `None`. `tests/test_manipulator_specs.py` pins the
argument (`ferrocad_shell.manipulator_specs` → the object, not `"Thing"`).

**What the preview is, and is not.** The grip rides the pointer's target in the sketch
plane; the *solved* geometry appears on commit, because the solve is a document operation
and a manipulator never edits the document during a drag. The result is still
constraint-correct: dragging `Vertex2` of a horizontal edge by `(5, 3)` moves it 5 across,
leaves the height alone, and takes the pad's `YLength` from 10 to 15 with `ZLength` fixed
at 10. A fully constrained vertex still gets a grip; its drag reports the DOF it kept
(`0`) and moves nothing.

**A note on undo.** `doc.undo()` restores the *inputs* (a `Geometry` list, a datum) but a
derived `Shape` refreshes on the next recompute; the app's `App_Undo` command is the path
that does both. The Python test therefore undoes through `App_Undo`, which is also what a
user presses.

**Measured.** `ferrocad_stage` **263** (four new plane-handle cases: the drag and its
commit, an edge-on refusal, snap/clamp on both coordinates, and the typed two-value
path); `ferrocad_shell_core` **290** (the plane wire form and its `value_v`);
`ferrocad_parametric` `sketch_drag_round_trip` **1/1** through the real provider, transport,
grip, commit and Python edit; `tests/test_sketch_tools.py` **7** (commands, creating
commit, dimension, the provider's grips, the drag commit, and the two refusals). The Python
`workbenches` set is **871 / 0 / 0 / 3** over 54 modules, `base` is **708 / 0 / 0 / 8**, and
`tests/test_manipulator_specs` is **8/0** with the argument pinned.
