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
every constraint names a geometry that exists.

It does **not** solve. `solve()` returns `0` when the references resolve and `-1` when one
dangles; geometry never moves, so a datum that the geometry does not satisfy still reports
`0`. `RedundantConstraints` is empty because there is no rank analysis to report. Those are
the documented approximations at the call sites; P4 SK2 (the PlanGCS adapter) is what
replaces them.

Constraint indices address the `Geometry` list. Construction geometry is stored in its own
list and is not addressable, and no constraint survives a reordering recompute, because
SK1's stable geometry identity has not landed.

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
