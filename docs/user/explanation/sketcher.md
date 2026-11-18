# Sketcher: profiles, constraints and the pad

A **sketch** is a constrained 2-D profile stored in the document. The object is a
`Sketcher::SketchObject`; it holds points, line segments, circles and arcs, a list of
typed constraints, and the solver that keeps both in step. An extrusion whose `Base` is
the sketch turns the profile into a solid — the **pad**.

The sketcher is reached three ways, all over the same object:

- **Draft** turns an existing wire into one with `Draft.make_sketch`.
- **The Parametric edition** draws one with `Sketch_Polyline` and dimensions it with
  `Sketch_Dimension`.
- **Python** builds and edits it directly through `Sketcher::SketchObject` and
  `Sketcher.Constraint`.

## The model

`Sketcher.Constraint` is a record, not an opaque solver handle. It carries the
constraint type, the geometry references, the point positions and the datum:

```python
import FreeCAD, Part, Sketcher

doc = FreeCAD.newDocument("SketchDemo")
sketch = doc.addObject("Sketcher::SketchObject", "Sketch")
sketch.addGeometry(
    Part.LineSegment(FreeCAD.Vector(0, 0, 0), FreeCAD.Vector(10, 0, 0)), False
)
sketch.addConstraint(Sketcher.Constraint("Horizontal", 0))
sketch.addConstraint(
    Sketcher.Constraint("Distance", 0, 1, 0, 2, 10.0)  # geometry 0: start and end
)
sketch.solve()
```

The positional forms are upstream's: a GeoId, a `Sketcher.PointPos` (`start`, `end`,
`mid`) or a pair of points. `-1` and `-2` are the root axes; external geometry (`-3` and
below) is refused by name. Each geometry element has a stable id behind the positional
GeoId, so deleting an earlier element does not silently rebind a constraint to a
different curve. Construction geometry has its own flag and is addressable in the same
way; `Shape`/`InternalShape` are built from the non-construction geometry only.

## The solver

`ferrocad_sketch_solver` is a pure-Rust Levenberg-Marquardt solve, not a binding to an
external constraint library. It parameterizes points, line segments, circles and arcs,
builds analytic residuals from values that carry their exact first derivatives, and
splits the sketch into connected islands before iterating. The split is not just a speed
measure: a broken constraint in one island is still diagnosable when another island is
broken independently.

`solve()` moves the geometry and returns the remaining degrees of freedom (`0` is fully
constrained). It also reports:

- **`RedundantConstraints`** — 1-based indices whose rows add no rank.
- **`ConflictingConstraints`** — a diagnosis: constraints whose removal lets the rest of
  their island solve, rather than a dump of every violated row.

`addConstraint` runs the solver's residual code first, so a constraint that can never
apply — a `Radius` on a line, a `Tangent` without a circle — raises `ValueError` at that
point instead of being stored and ignored.

### Constraints it stores

`Coincident`, `PointOnObject`, `Vertical`, `Horizontal`, `Parallel`, `Perpendicular`,
`Tangent`, `TangentViaPoint`, `Equal`, `Symmetric`, `Block`, `Distance`, `DistanceX`,
`DistanceY`, `Radius`, `Diameter`, `Angle`, `Weight`.

`Block` pins a geometry at the position it had when the solve started, which is what makes
it idempotent across recomputes. A first-order solve can also stop at a degenerate start —
a horizontal line given both `Vertical` and a length datum, where the Jacobian is
rank-deficient along the needed direction; that returns `-1` with both constraints named
and leaves `Geometry` untouched.

## Drawing and padding in the edition

The Parametric edition registers two creation rails over the same stage polyline tool:

| Command | Tool id | Result |
| --- | --- | --- |
| `Part_Polyline` | `polyline` | a parametric `Part::FeaturePython` wire and the `Part::Extrusion` that links it |
| `Sketch_Polyline` | `sketch_polyline` | a constrained `Sketcher::SketchObject` and its linked pad |
| `Sketch_Dimension` | — | one driving datum on the selected sketch edge, at the value it already has |

Both creation commands are one named transaction: one undo removes the profile and the
pad together. The extrusion's `Dir` is the profile's own plane normal (`Solid = True`,
`DirMode = "Custom"`), so a sketch drawn on a tilted face pads perpendicular to that face.
Editing the wire's `Points`/`Closed` rebuilds it on the next recompute, and the linked
extrusion follows. If the kernel refuses a pad, the profile is kept and only the pad is
dropped.

The square the edition creates from four snapped points pins down the numbers:

- **4** `Part.LineSegment`s and **10** constraints: four shared corners, two origin pairs
  (the detector stages the origin pair for both segments meeting at the first point), and
  two `Horizontal` plus two `Vertical`;
- `solve() == 2`, and `RedundantConstraints == [6]` — the second origin pair is redundant
  in rank, not in intent;
- `Sketch_Dimension` adds a datum at index `10`; `setDatum(10, 20.0)` takes the pad's
  `YLength` from 10 to 20 on the next recompute.

## Dragging a vertex

`SketchObject.drag(geometry, pos, target)` is the interactive half of the same solve. It
adds one **soft prior** pulling the point toward `target` in the sketch's own plane,
solves, writes the moved geometry back, and returns the remaining DOF. The prior is a
penalty, not a constraint: it never enters the convergence test or the rank analysis, so a
hard constraint wins.

In the Parametric edition, a sketch vertex is a declared **plane handle**: one grip per
topological `VertexN` of the sketch shape, in the sketch's own frame. The grip's commit is
`sketch_drag` / `drag` with `[u, v]`; the document then runs `SketchObject.drag`, so the
constraints decide where the point lands and the linked pad follows on the same
recompute. A fully constrained vertex still gets a grip; it reports `0` DOF and does not
move. A drag is one undo step.

The grips arm with the `Push/Pull` and `Fillet` hands — the loadout slots that request a
manipulator. A dedicated sketch hand is a UI decision, not a missing solver feature.

## What it does not do

- There is no `Sketcher.Solver` object. `solve()` and `drag()` are the surface; the solver
  is internal.
- Ellipses and splines are stored and drawn but are carried through the solve untouched,
  and cannot be a constraint reference.
- External geometry beyond the two root axes is not supported. Upstream's
  `InternalAlignment` family and `SnellsLaw` are outside this slice.
- Constraint labels (`LabelPosition`/`LabelDistance`) are refused: constraint labels are
  not drawn.

## Where the detail lives

- [Sketch constraints — the model and the solver](../../developer/workbenches/sketch-constraints.md)
  is the stable contract: the encoding, the equations, the island rules and the exact
  refusals.
- [The sketch model round](../../project/record/2026-11-15-p4-sketch-model.md) records the
  solver, drag and vertex-grip rounds and their measurements.
- [Gizmo manipulators](../../developer/interaction/gizmo-manipulators.md) explains the
  plane handle a vertex grip uses.

## Evidence

The latest round measured `part_sketcher` **18/18**, `tests/test_sketch_constraints.py`
**40/40** and `tests/test_sketch_tools.py` **7**; the Python `workbenches` set is
**881 / 0 / 0 / 3** over 55 modules. The Draft gate is **93 / 0 / 0 / 1**, with
`test_modification` **33 / 0 / 0 / 0**.
