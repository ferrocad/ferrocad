# Sketch constraints — the model, not the solver

Status: stable contract for the sketch constraint surface (`Sketcher::SketchObject` and
`python/Sketcher`). It records what FerroCAD stores, what it diagnoses, and the decisions
that keep it honest until P4 SK2 binds a solver.

## The scope decision

P4's scope is unchanged: **FerroCAD owns the constraint model, its storage, its
selectors and its diagnostics; the numerical solve comes from PlanGCS (2D) or
OndselSolver (3D).** `TODO-15-11-2026.md` §P4 holds that decision and the slices.

So `SketchObject.solve()` does **not** move geometry. It checks that every constraint
names a geometry that exists and reports upstream's "converged" code (`0`), or `-1` when
a reference dangles. A sketch whose dimensions are not yet satisfied by its geometry
still reports `0`: the datum is the authored value, not a solved one. `RedundantConstraints`
is empty for the same reason — with no solve there is no rank information to report, and
an invented list would be worse than an empty one.

This is the approximation the project rule asks to state at the call site (the Rust
doc comments on `sketch_solve` and `sketch_redundant_constraints`) and here.

## Where it lives

| Piece | File |
| --- | --- |
| The document object, the model, the encoding and the Python descriptor | `crates/ferrocad_part_py/src/solids.rs` (the `Sketcher` section) |
| The Python record type | `python/Sketcher/__init__.py` |
| The `Part` seam `findPlane`/rotate/`LineSegment(curve, first, last)` support `Draft.make_sketch` needs | `crates/ferrocad_part_py/src/lib.rs`, `crates/ferrocad_part_py/src/curves.rs` |
| Tests | `crates/ferrocad_part_py/tests/part_sketcher.rs`, `tests/test_sketch_constraints.py` |

## Storage

A constraint is a record in the sketch's hidden `ConstraintData` property, an
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
Radius;first=0;value=5
Equal;first=0;second=1
Coincident;first=0;firstPos=2;second=1;secondPos=1
Distance;first=0;firstPos=2;value=12.5;name=overall
```

Keys: `first`, `firstPos`, `second`, `secondPos`, `third`, `thirdPos`, `value`,
`driving`, `active`, and a trailing `name=…`. `PointPos` uses upstream's numbering —
`0` none, `1` start, `2` end, `3` mid. A key equal to its default is omitted, so decoding
an entry without it yields the same record: `First`/`Second`/`Third` are `0`,
positions are `none`, `Value` is `0`, `Driving`/`IsActive` are `true`.

`name` is emitted **last** so a label may contain `;` or `=`: the decoder splits at the
first `;name=` and takes the rest of the string as the name.

## The Python surface

`SketchObject` gains these attributes (they are installed as descriptors on
`ferrocad.DocumentObject` by `install_sketch_methods`, which raises `AttributeError` for
every other object type, exactly as if the attribute did not exist):

| Name | Kind | Meaning |
| --- | --- | --- |
| `addGeometry(geometry, construction=False)` | method | Append one geometry object (or a sequence) to `Geometry`; returns the last index |
| `addConstraint(constraint)` | method | Append one record or a sequence; returns the last index, `-1` for an empty sequence |
| `delConstraint(index)` | method | Remove one record by zero-based index |
| `setDatum(index, value)` | method | Write a dimensional constraint's datum |
| `getDatum(index)` | method | Read it; a non-dimensional kind is a `ValueError` |
| `solve()` | method | `0` when every reference resolves, `-1` otherwise |
| `GeometryCount` | attribute | `len(Geometry)` |
| `ConstraintCount` | attribute | number of stored records |
| `Constraints` | attribute | the records as `Sketcher.Constraint` objects |
| `RedundantConstraints` | attribute | `[]` (no solver, no rank analysis) |
| `detectMissingPointOnPointConstraints(tol)` | method | Stage endpoint pairs that coincide but carry no `Coincident` |
| `makeMissingPointOnPointCoincident(includeSecond=True)` | method | Commit the staged ones; returns how many were added |
| `detectMissingVerticalHorizontalConstraints(tol)` | method | Stage straight edges already aligned with an axis |
| `makeMissingVerticalHorizontal(includeSecond=True)` | method | Commit the staged ones; returns how many were added |

The four detection methods keep their staged records in the hidden
`MissingPointOnPointConstraints` / `MissingVerticalHorizontalConstraints` properties, so
the two-phase upstream call shape works without per-object Rust state. Detection reads
the sketch's **local** geometry coordinates, which is the space upstream's constraints
live in. It skips closed edges (one vertex, no endpoints) and only classifies an edge as
vertical/horizontal when it is a straight segment.

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
the numbering as class attributes.

## What it refuses, and why

- **Unknown constraint kinds.** `addConstraint` raises `ValueError` naming the type and
  listing the kinds it stores: `Coincident`, `PointOnObject`, `Vertical`, `Horizontal`,
  `Parallel`, `Perpendicular`, `Tangent`, `TangentViaPoint`, `Equal`, `Symmetric`, `Block`,
  `Distance`, `DistanceX`, `DistanceY`, `Radius`, `Diameter`, `Angle`, `Weight`. Upstream's
  `InternalAlignment` family and `SnellsLaw` are outside this slice.
- **Constraint labels.** `LabelPosition`/`LabelDistance` are a `TypeError` naming the gap:
  they place a drawing annotation and sketch constraint labels are not drawn.
- **Construction geometry as a constraint target.** `ConstructionGeometry` is stored in its
  own list and is not addressable; constraint indices address `Geometry`. Upstream keeps one
  indexed list. This closes when SK1's stable geometry identity lands, which is also what a
  constraint needs to survive a reordering recompute.
- **A record written behind `addConstraint`'s back** is a `ValueError`, not a silent skip.

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
- `crates/ferrocad_part_py/tests/part_sketcher.rs`: the model, the datum edits, the
  dangling reference, the autoconstraint detection and `findPlane` on a wire.
- `tests/test_sketch_constraints.py` (22 cases): the record type, the document surface,
  the refusals and the empty redundancy list.
- Part conformance stayed **531/0/0/12** after the seam changes.
