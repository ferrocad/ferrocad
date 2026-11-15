# Sketch constraints — the model and the solver

Status: stable contract for the sketch constraint surface (`Sketcher::SketchObject`,
`python/Sketcher` and `ferrocad_sketch_solver`). It records what FerroCAD stores, how the
solve works, what it diagnoses, and where it stops.

## The scope decision

P4's scope: **FerroCAD owns the constraint model, its storage, its selectors, its
diagnostics and the solve.** The original plan was to bind upstream's C++ solver
(PlanGCS for 2D, OndselSolver for 3D); SK2 replaced the 2D half with a pure-Rust solver
instead, because PlanGCS would add an Eigen/Boost C++ closure to a workspace whose kernel
is Rust, grow `target/` for a 30 MB sketch vocabulary, and is not even in the sparse
upstream checkout this repository measures against. No contract gate required it. The
stable API is behavioural — `solve()` returns degrees of freedom, `RedundantConstraints`
and `ConflictingConstraints` name indices — so a later PlanGCS binding could replace the
implementation behind the same contract. `TODO-15-11-2026.md` §P4 holds the decision.

`SketchObject.solve()` **moves geometry** and returns the remaining degrees of freedom
(upstream's convention: `0` means fully constrained). `-1` means it did not solve — a
dangling reference, a constraint the solver cannot apply, or a system that does not
converge. Nothing is written in that case, so a failed solve never corrupts a sketch.

## Where it lives

| Piece | File |
| --- | --- |
| The solver: parameterization, residuals, Levenberg-Marquardt, rank analysis | `crates/ferrocad_sketch_solver/` (depends only on `ferrocad_curves`) |
| The document object, the model, the encoding and the Python descriptor | `crates/ferrocad_part_py/src/solids.rs` (the `Sketcher` section) |
| The bridge between stored shapes/ids and the solver | `crates/ferrocad_part_py/src/sketch_solver.rs` |
| The Python record type | `python/Sketcher/__init__.py` |
| The `Part` seam `findPlane`/rotate/`LineSegment(curve, first, last)` support `Draft.make_sketch` needs | `crates/ferrocad_part_py/src/lib.rs`, `crates/ferrocad_part_py/src/curves.rs` |
| Tests | `crates/ferrocad_sketch_solver/src/lib.rs`, `crates/ferrocad_part_py/tests/part_sketcher.rs`, `tests/test_sketch_constraints.py` |

## Storage and identity

Three hidden properties stay index-aligned — entry `i` of each describes the same
geometrical element:

| Property | Type | Holds |
| --- | --- | --- |
| `Geometry` | `Part::PropertyTopoShapeList` | the edge each geometry object's `toShape()` returns |
| `GeometryIds` | `App::PropertyStringList` | the stable id, `g1`, `g2`, … |
| `Construction` | `App::PropertyBoolList` | whether the entry is construction geometry |

**The id is what a constraint stores.** Upstream's Python surface is positional (a 0-based
GeoId), and FerroCAD still speaks it at the boundary, but the record itself holds `#g3`.
That is what makes `delGeometry` safe: the surviving elements' positions shift down, every
surviving constraint still names the curve it was built on, and only the constraints that
named the removed element are dropped.

Ids are minted as `g<max + 1>` per sketch. They are **sketch-local**, not document-scope:
a constraint belongs to one sketch, so a sketch-local id is enough; a document-scope UUID
becomes necessary only when external geometry (`-3` and below) arrives. They are opaque
rather than derived from the curve, because a content hash would change exactly when the
geometry is edited — which is when the constraint must survive.

The three lists are written geometry-first, and a reader truncates a longer metadata list
or backfills a shorter one, never reorders. A torn undo therefore cannot pair a shape with
the wrong id. A sketch written before the identity existed (in practice: before
2026-11-15) gains the two properties through `addProperty` on first use, so its writes are
real document properties rather than Python instance attributes.

A constraint is a record in the sketch's `ConstraintData` property, an
`App::PropertyStringList`. A string list keeps it persistable through both serializers
without adding a value type, and it makes a corrupted record visible rather than silently
dropped.

The property is **not** called `Constraints`: that name is the Python getter which decodes
these records into `Sketcher.Constraint` objects, and a class attribute of that name would
shadow a document property of the same name (that recursion was a real defect in the first
cut). Upstream has no `Constraints` *property* either — the list is computed from the
solver state.

### The encoding

One record per entry, `;`-separated, first field the upstream type name:

```text
Radius;first=#g1;value=5
Equal;first=#g1;second=#g3
Coincident;first=#g2;firstPos=2;second=#g1;secondPos=1
Distance;first=#g1;firstPos=2;value=12.5;name=overall
Coincident;first=-1;firstPos=1;second=#g1;secondPos=1
```

A reference is `#<id>` for a geometry element, or a bare negative int for the root:
`-1` is the X axis (its start point is the root point) and `-2` the Y axis, upstream's
`GeoEnum`. External geometry (`-3` and below) is out of surface. A bare **non-negative**
int (`first=0`) is a record written before the ids existed: it is read as a position, so
it cannot survive a deletion — which is why nothing writes it any more.

Keys: `first`, `firstPos`, `second`, `secondPos`, `third`, `thirdPos`, `value`,
`driving`, `active`, and a trailing `name=…`. `second`/`third` are written only when the
record uses them. `PointPos` uses upstream's numbering — `0` none, `1` start, `2` end,
`3` mid. A key equal to its default is omitted, so decoding an entry without it yields the
same record: positions are `none`, `Value` is `0`, `Driving`/`IsActive` are `true`.

`name` is emitted **last** so a label may contain `;` or `=`: the decoder splits at the
first `;name=` and takes the rest of the string as the name.

## The Python surface

`SketchObject` gains these attributes (they are installed as descriptors on
`ferrocad.DocumentObject` by `install_sketch_methods`, which raises `AttributeError` for
every other object type, exactly as if the attribute did not exist):

| Name | Kind | Meaning |
| --- | --- | --- |
| `addGeometry(geometry, construction=False)` | method | Append one geometry object (or a sequence); returns the last index |
| `delGeometry(index)` | method | Remove one element and every constraint that named it |
| `toggleConstruction(index)` | method | Flip one element's construction flag |
| `getConstruction(index)` | method | Whether one element is construction geometry |
| `addConstraint(constraint)` | method | Append one record or a sequence; returns the last index, `-1` for an empty sequence |
| `delConstraint(index)` | method | Remove one record by zero-based index |
| `setDatum(index, value)` | method | Write a dimensional constraint's datum |
| `getDatum(index)` | method | Read it; a non-dimensional kind is a `ValueError` |
| `solve()` | method | Solve; returns the remaining degrees of freedom (`0` = fully constrained) or `-1` |
| `drag(geometry, pos, target)` | method | Solve with one soft pull on a point toward `target`; returns the same DOF count (never `0` or `-1` because of a prior) |
| `GeometryCount` | attribute | `len(Geometry)` — construction geometry is included |
| `ConstraintCount` | attribute | number of stored records |
| `Constraints` | attribute | the records as `Sketcher.Constraint` objects, in positional GeoIds |
| `RedundantConstraints` | attribute | 1-based indices of the constraints that add no rank (upstream's convention) |
| `ConflictingConstraints` | attribute | 1-based indices of the constraints that cannot all hold |
| `detectMissingPointOnPointConstraints(tol)` | method | Stage coincidences (geometry pairs and the root point) that carry no `Coincident` |
| `makeMissingPointOnPointCoincident(includeSecond=True)` | method | Commit the staged ones; returns how many were added |
| `detectMissingVerticalHorizontalConstraints(tol)` | method | Stage straight edges already aligned with an axis |
| `makeMissingVerticalHorizontal(includeSecond=True)` | method | Commit the staged ones; returns how many were added |

The four detection methods keep their staged records in the hidden
`MissingPointOnPointConstraints` / `MissingVerticalHorizontalConstraints` properties, so
the two-phase upstream call shape works without per-object Rust state. Detection reads
the sketch's **local** geometry coordinates, which is the space upstream's constraints
live in. It skips closed edges (one vertex, no endpoints) and only classifies an edge as
vertical/horizontal when it is a straight segment. Construction geometry is included: it
is real geometry for a constraint.

An endpoint on the sketch origin stages `Coincident(first=-1, firstPos=start, second=…,
secondPos=…)` — upstream's coincidence with the root point.

`includeSecond` is accepted for signature compatibility. Detection already emits each
`(point, point)` pair explicitly, so `includeSecond=False` commits one constraint per
first reference rather than skipping the second half of a pair — which is the property the
flag names.

### `Sketcher.Constraint`

A data holder: `Constraint(type, *args, **kwargs)`. The positional shapes are upstream's:

| Shape | Example |
| --- | --- |
| `(type, geometry)` | `Constraint("Vertical", 0)` |
| `(type, first, second)` | `Constraint("Equal", 0, 1)` |
| `(type, first, firstPos, second, secondPos)` | `Constraint("Coincident", 0, 2, 1, 1)` |
| `(type, geometry, value)` | `Constraint("Radius", 0, 5.0)` |
| `(type, geometry, pointPos, value)` | `Constraint("Distance", 0, 2, 5.0)` |
| `(type, first, second, value)` | `Constraint("Angle", 0, 1, 45.0)` |

The keywords `First`, `FirstPos`, `Second`, `SecondPos`, `Third`, `ThirdPos`, `Value`,
`Name`, `Driving` and `IsActive` override the positional form. `Sketcher.PointPos` carries
the numbering as class attributes, and `Sketcher.GeoUndef` (`-2000`) is the value of a
reference the constraint does not use — not `0`, because geometry `0` is a real curve.

## The creation path and the dimension command

The workbench route into this model is the parametric edition's `Sketch_Polyline` and
`Sketch_Dimension`, whose tool plumbing is in
[`draft-tools.md`](draft-tools.md#the-sketch-backed-profile-and-its-dimension-p4-sk6).
What matters for *this* contract is the order the calls land in and the numbers they
produce:

| Step | Calls | Why this order |
| --- | --- | --- |
| the profile | `addGeometry(segments, False)` | one `Part.LineSegment` per drawn edge, already in sketch-local `(u, v)` |
| the vertices | `detectMissingPointOnPointConstraints(1e-6)` → `makeMissingPointOnPointCoincident(True)` | detection stages, `make` commits — upstream's two-phase shape |
| the axes | `detectMissingVerticalHorizontalConstraints(1e-6)` → `makeMissingVerticalHorizontal(True)` | same pair, for the edges already aligned with a sketch axis |
| the solve | `solve()` | returns the remaining DOF, which the linked pad then lives with |

A closed four-point profile comes out at **10 constraints, `solve() == 2`,
`RedundantConstraints == [6]`**. The extra constraint is the origin pair: the detector
stages a root coincidence for *both* segments that meet at the first point, and the second
one adds no rank. That is the detector being faithful, not a defect — the redundancy is
reported honestly rather than suppressed.

`Sketch_Dimension` then adds **one** driving datum at the value the edge already has:
`Radius` for a `Part::GeomCircle`, otherwise the endpoint-to-endpoint `Distance`. Adding it
is geometrically a no-op and drops the DOF to `1`; from there `setDatum(index, value)` is
the whole editing interface, and the extrusion that names the sketch follows on the next
recompute.

## The vertex grips

The interactive half of `drag()` is a **declared plane handle** per sketch vertex
(`ferrocad_stage::DeclarativeKind::Plane`). Nothing about the sketch is compiled into the
shell: the edition's `manipulator_specs` provider reads the document and declares the
grips in data.

| Step | What happens |
| --- | --- |
| arming | a `Sketcher::SketchObject` selection publishes one grip per **topological vertex** of `sketch.Shape` (`Vertex1..VertexN`); a `VertexN` selection narrows to that one |
| the frame | the origin is the vertex, `u`/`v` are the sketch's own axes (`Placement.Rotation` applied to local X/Y), `z` is its normal |
| the drag | the pointer ray is intersected with that plane, so the two committed coordinates are a motion **inside the sketch** |
| the commit | `sketch_drag` / `drag`, `element = "VertexN"`, `values = [u, v]` |
| the edit | `VertexN` resolves to the nearest geometry endpoint (or a circle's centre) through `Placement.inverse()`, and `sketch.drag(index, pos, start + (u, v))` runs in one named transaction |

The vertices are the **shape's**, so a corner shared by two segments is one grip and
`VertexN` is the name a selection reports. A coincident corner matches two geometry
endpoints — segment *i*'s end and segment *i+1*'s start — and the lowest index wins; the
`Coincident` constraint is what drags the other half, so the choice does not change the
result.

**What the preview shows, and what it does not.** The grip rides the pointer's target in
the sketch plane; the *solved* geometry appears when the drag is committed, because the
solve is a document operation and a manipulator never edits the document during a drag.
The result is still the constraint-correct one: dragging `Vertex2` of a horizontal edge
by `(5, 3)` moves it 5 across and leaves the height alone, and the pad's `YLength` grows
while its `ZLength` does not.

A **fully constrained** vertex still gets a grip. The drag then reports the degrees of
freedom it kept (`0`) and moves nothing, which is the honest refusal — a grip that
disappeared would look like a broken handle.

**When they arm.** Declared handles are requested only for the loadout slots that want a
manipulator — `Push/Pull` and `Fillet` today (`Loadout::wants_manipulator`) — so the
vertex grips appear with one of those hands equipped, exactly like the applied-fillet
round grip. A dedicated sketch hand is a **UI** decision (U4's hand list, not U4's
mechanism) and is deliberately not taken here: widening the gate would arm every declared
handle in the `Move` hand as well, which is a different change.

## The solve

The solver is `ferrocad_sketch_solver`; the sketch object calls it from `solve()` and from
`execute()` (a recompute keeps a sketch solved, as upstream's does).

### Geometry and parameters

| Stored geometry | Parameters |
| --- | --- |
| a point (`Part.Vertex`) | `(x, y)` |
| a line segment | `(x1, y1, x2, y2)` |
| a circle | `(cx, cy, r)` |
| an arc | `(cx, cy, r, first, last)` |

An edge reports its *basis* curve, so the edge's own parameter range is what turns a line
into a segment and a circular edge into a circle (a whole turn) or an arc (anything
shorter). Everything is solved in the sketch's local XY plane, which is the plane
`Placement` moves afterwards. An ellipse, a spline or any other curve the solver does not
parameterize is `Curve2d::Other`: it is carried through untouched, contributes no degrees
of freedom, and is refused as a constraint reference by name.

### Constraint equations

Each constraint contributes one or more residual rows, built from values that carry their
exact first derivatives (forward-mode accumulation, not a finite difference), so the
Jacobian is analytic and two identical constraints give bit-identical rows — which is what
makes the redundancy analysis trustworthy.

| Kind | Row(s) |
| --- | --- |
| `Coincident` | `p − q` (two rows) |
| `PointOnObject` | signed distance from the line, or `\|p − c\|² − r²` on a circle |
| `Horizontal` / `Vertical` | `Δy` / `Δx` of a line, or of two points |
| `Parallel` / `Perpendicular` | `cross(u, v) / (\|u\|\|v\|)` / `dot(u, v) / (\|u\|\|v\|)` |
| `Tangent` | line·circle: `d² − r²`; circle·circle: `\|c₁ − c₂\|² − (r₁ + r₂)²` |
| `Equal` | `\|u\| − \|v\|` for lines, `r₁ − r₂` for circles |
| `Symmetric` | `p₁ + p₂ − 2p₃` (two rows) |
| `Block` | one row per parameter: `xᵢ − xᵢ⁰` |
| `Distance` | `\|p − q\|² − d²` (squared, so the row is smooth when the points meet) |
| `DistanceX` / `DistanceY` | `Δx − d` / `Δy − d` |
| `Radius` / `Diameter` | `r − d` / `2r − d` |
| `Angle` | `cos θ(u, v) − cos(d)` |

The iteration is Levenberg-Marquardt on the dense normal equations with a Cholesky solve
and adaptive damping. It stops at `max\|r\| ≤ 1e-9`, at a vanishing step, or after 200
iterations, whichever comes first.

### Islands

A constraint couples only the geometry it names, and a root-axis reference couples
nothing (the axes are fixed), so the sketch is split into connected components before
anything is evaluated. Each island is then handled at the cheapest tier that applies:

| Tier | Condition | Work |
| --- | --- | --- |
| **1** | the island has no constraint and no prior | none — the island's parameter count is added to DOF |
| **2** | every residual is already within tolerance and there is no prior | no iteration; the rank analysis still runs, so DOF and redundancy are reported |
| **3** | otherwise | the Levenberg-Marquardt iteration, the rank analysis, and — if it fails — the conflict diagnosis |

The split is not only a speed measure: it is what makes a multi-conflict sketch
diagnosable. Leave-one-out over a whole sketch finds nothing when two *independent*
islands are each broken, because removing one constraint still leaves the other
unconverged. Per island, each diagnosis is local and its indices are mapped back to the
global constraint list.

### Degrees of freedom, redundancy and conflicts

* **DOF** = parameters − rank(J), summed over the islands. The rank comes from
  Gram-Schmidt over the Jacobian rows *in constraint order*, so a constraint that says
  nothing its predecessors did not say is the one named.
* **`RedundantConstraints`** is 1-based (Draft's `make_sketch` deletes with `idx - 1`). A
  multi-row constraint is redundant only when *every* row is dependent; calling a partly
  dependent constraint redundant would make deleting it destructive. The analysis runs
  whether or not an island converges — redundancy is a statement about the equations.
* **`ConflictingConstraints`** is also 1-based, and it is a diagnosis, not a residual
  dump. Two filters pick the candidates first: a constraint has to be part of a
  **rank-dependent cycle** (its row was a combination of earlier ones, or it owns a
  direction a dependent row consumed) *or* still be **violated** at the best fit. Then the
  solver removes one candidate at a time and names those whose removal lets the rest of
  the island solve — the constraint a user has to change, not the dozen rows a single bad
  datum drags off. If no single removal helps, it falls back to the constraints still
  violated at the best fit.

`addConstraint` runs the solver's own residual code on the new record before storing it,
so a constraint the solver could never apply is a `ValueError` at that point rather than a
silent no-op in the next solve.

### Soft priors — the drag seam

`ferrocad_sketch_solver::SoftPrior { reference, target, weight }` plus
`solve_with_priors` is the seam an interactive drag uses: each prior adds two weighted
rows pulling a geometry point toward `target`, and only those rows.

* Priors are **penalties, not constraints**. They never enter the convergence test or the
  rank analysis, so they steer the degrees of freedom the constraints leave free instead
  of fighting them: dragging a pinned point moves nothing and reports nothing, where a
  hard constraint would report a conflict. `dof` and `redundant` are exactly what they
  would be without the prior.
* The penalty optimum trades a small hard-constraint violation (`O(weight²)`) for the
  pull, so the solver runs one prior-free pass afterwards to restore hard feasibility —
  the dragged free parameter stays where the prior put it. The return is therefore a
  hard-feasible point near the prior's constrained optimum, not the exact QP solution.
* A prior on a root axis is refused (`InapplicablePrior`): the origin cannot be dragged,
  and ignoring it would look like a drag that does nothing.

**`SketchObject.drag(geometry, pos, target)` is the Python consumer.** `geometry` is the
same positional GeoId `addConstraint` takes (a negative root axis raises `ValueError`),
`pos` is upstream's `PointPos` (`1` start, `2` end, `3` mid, `0` the curve itself), and
`target` is an `(x, y)` pair or anything with `x`/`y`, in the sketch's local plane. It
solves with one prior, writes the moved geometry back, and returns the remaining DOF —
the same number `solve()` returns, because the prior never enters the rank analysis. A
system that cannot be built or will not converge returns `-1` and leaves `Geometry`
untouched, so a drag can never corrupt a sketch.

What a drag therefore does, precisely: it moves the free degrees of freedom toward the
pointer and loses to every hard constraint. A pinned point does not move (and reports
`0`, not a conflict); on a line whose two ends share a `Horizontal` row, dragging one end
vertically moves both, because that is what the row means. The sketch drag tool and the
3-D manipulators are the remaining consumers.

### Where it stops

* **A degenerate start.** The method is first-order. If the Jacobian is rank-deficient
along the direction the solution needs — a horizontal line given `Vertical` *and* a length
datum, where both rows are parallel at the start — the iteration cannot begin the rotation
and reports `-1` with both constraints named. A sketch whose constraints are already
nearly satisfied (the normal case, and the one `Draft.make_sketch` produces) has no such
problem.
* **A non-parameterized curve** is never moved and cannot be constrained.
* **`Block`** pins a geometry at the position it had when the solve started, which is what
  makes it idempotent across recomputes; it is not a stored reference point.

## What it refuses, and why

- **A constraint that does not apply.** `addConstraint` runs the solver's residuals, so a
  `Radius` on a line, a `Parallel` involving a circle, a `Tangent` without a circle or a
  zero-length line is a `ValueError` naming the reason. Nothing is stored.
- **Unknown constraint kinds.** `addConstraint` raises `ValueError` naming the type and
  listing the kinds it stores: `Coincident`, `PointOnObject`, `Vertical`, `Horizontal`,
  `Parallel`, `Perpendicular`, `Tangent`, `TangentViaPoint`, `Equal`, `Symmetric`, `Block`,
  `Distance`, `DistanceX`, `DistanceY`, `Radius`, `Diameter`, `Angle`, `Weight`. Upstream's
  `InternalAlignment` family and `SnellsLaw` are outside this slice.
- **A reference to a geometry that does not exist.** `IndexError`, raised while the record
  is read (the positional index is resolved to an id there). A record that names a vanished
  id — hand-edited or restored — is a `ValueError` from `solve()`'s point of view (`-1`) and
  from the `Constraints` getter, which will not invent a position for it.
- **External geometry.** `-3` and below is a `ValueError` naming the gap. `-1` and `-2`
  are the supported root axes.
- **Constraint labels.** `LabelPosition`/`LabelDistance` are a `TypeError` naming the gap:
  they place a drawing annotation and sketch constraint labels are not drawn.

## What `Draft.make_sketch` needed from the Part seam

Three seams, all fixed at the root rather than in the Draft shim:

1. **`Shape.findPlane()` answers for a wire and an edge.** It only knew planar *faces*
   (the normal came from the face's outer wire). A wire now takes the cross product of its
   first pair of non-parallel edge tangents; a straight single edge still has no plane,
   which is also upstream's answer.
2. **`Part.LineSegment(curve, first, last)`.** Draft's `orientEdge` trims the edge's basis
   line through this constructor; only `(start, end)` existed. The single-argument copy
   form works too.
3. **An angle can be a `Quantity`.** `DraftVecUtils.angle(...) * App.Units.Radian` is a
   quantity, so `Quantity.__rmul__`/`__mul__` accept a plain factor and `Shape.rotate`/
   `rotated` convert an angle quantity with `getValueAs("deg")`.

That last one exposed a real unit bug: `App::PropertyAngle` defaulted to a **millimetre**
quantity, so `obj.Rotation` read back as `45 mm` and only worked because `__float__` on a
mislabeled quantity happens to return the number. Angle properties now default to a degree
quantity, which is what the `.FCStd` writer already infers from the unit signature.

## Evidence

- `Draft.make_sketch`/`Draft.draftify` for a three-point wire: `test_modification.py`
  **33/0/0/0**, and the `--draft` gate **93/0/0/1** (was 60/0/0/1).
- `crates/ferrocad_sketch_solver/src/lib.rs` (**14** unit cases): a perturbed rectangle
  closing, a distance and a radius datum, an arc radius, redundancy, a conflict, `Block`,
  a refused radius-on-a-line, unparameterized geometry carried through, two broken islands
  diagnosed separately, an already-satisfied island still reporting DOF and redundancy, a
  soft prior dragging one free parameter, and a prior on a root refused.
- `crates/ferrocad_part_py/tests/part_sketcher.rs` (**18** cases): the model, the datum
  edits, the dangling record, the autoconstraint detection, identity across a deletion,
  construction addressability/exclusion, the root axes, `findPlane` on a wire, a free
  drag that keeps the DOF count and the redundancy set, and a fully constrained drag that
  moves nothing and reports `0`.
- `tests/test_sketch_constraints.py` (**40** cases): the record type, the document surface,
  the identity and construction surface, the migration of a sketch without the identity
  properties, the refusals, the `.FCStd` round trip, the end-to-end solve cases — a
  rectangle closing with four degrees of freedom and honouring `DistanceX`/`DistanceY`, a
  radius datum resizing a circle, a construction line driving the real profile, a redundant
  duplicate at 1-based index 2, and a contradictory pair refused with the geometry left
  untouched — and the drag cases (`drag()` on a free end, against a hard row, on a pinned
  point, and the two refusals).
- Part conformance stayed **531/0/0/12** after the seam changes; `workbenches` is
  **859/0/0/3** and `base` **707/0/0/8**.
