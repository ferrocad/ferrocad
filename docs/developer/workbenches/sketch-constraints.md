# Sketch constraints — the model, not the solver

Status: stable contract for the sketch constraint surface (`Sketcher::SketchObject` and
`python/Sketcher`). It records what FerroCAD stores, what it diagnoses, and the decisions
that keep it honest until P4 SK2 binds a solver.

## The scope decision

P4's scope is unchanged: **FerroCAD owns the constraint model, its storage, its
selectors and its diagnostics; the numerical solve comes from PlanGCS (2D) or
OndselSolver (3D).** `TODO-15-11-2026.md` §P4 holds that decision and the slices.

So `SketchObject.solve()` does **not** move geometry. It checks that every constraint
names a geometry or root that exists and reports upstream's "converged" code (`0`), or
`-1` when a reference dangles. A sketch whose dimensions are not yet satisfied by its
geometry still reports `0`: the datum is the authored value, not a solved one.
`RedundantConstraints` is empty for the same reason — with no solve there is no rank
information to report, and an invented list would be worse than an empty one.

This is the approximation the project rule asks to state at the call site (the Rust
doc comments on `sketch_solve` and `sketch_redundant_constraints`) and here.

## Where it lives

| Piece | File |
| --- | --- |
| The document object, the model, the encoding and the Python descriptor | `crates/ferrocad_part_py/src/solids.rs` (the `Sketcher` section) |
| The Python record type | `python/Sketcher/__init__.py` |
| The `Part` seam `findPlane`/rotate/`LineSegment(curve, first, last)` support `Draft.make_sketch` needs | `crates/ferrocad_part_py/src/lib.rs`, `crates/ferrocad_part_py/src/curves.rs` |
| Tests | `crates/ferrocad_part_py/tests/part_sketcher.rs`, `tests/test_sketch_constraints.py` |

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
| `solve()` | method | `0` when every reference resolves, `-1` otherwise |
| `GeometryCount` | attribute | `len(Geometry)` — construction geometry is included |
| `ConstraintCount` | attribute | number of stored records |
| `Constraints` | attribute | the records as `Sketcher.Constraint` objects, in positional GeoIds |
| `RedundantConstraints` | attribute | `[]` (no solver, no rank analysis) |
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

## What it refuses, and why

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
- `crates/ferrocad_part_py/tests/part_sketcher.rs` (**16** cases): the model, the datum
  edits, the dangling record, the autoconstraint detection, identity across a deletion,
  construction addressability/exclusion, the root axes and `findPlane` on a wire.
- `tests/test_sketch_constraints.py` (**29** cases): the record type, the document surface,
  the identity and construction surface, the migration of a sketch without the identity
  properties, the refusals, the empty redundancy list and the `.FCStd` round trip (ids,
  flags, constraints and a root reference all survive a reload).
- Part conformance stayed **531/0/0/12** after the seam changes.
