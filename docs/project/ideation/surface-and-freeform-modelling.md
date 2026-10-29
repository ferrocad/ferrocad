# Surface, freeform and cage modelling

Status: ideation (2026-10-29). Exploratory product ideas. Nothing here is a
commitment, a roadmap, or an accepted design. Companion to [`README.md`](README.md),
[`freecad-pain-points.md`](freecad-pain-points.md) (section 6),
[`intent-and-automation.md`](intent-and-automation.md), the
[backlog](../TODO-27-10-2026.md) and the
[developer architecture](../../developer/architecture.md).

Freeform modelling is the place where FreeCAD's answers are spread across the most
modules while the underlying geometry has been capable for over a decade. This note
separates the two, because they lead to different questions: the *kernel* question is
mostly settled, and the *ownership* question is not.

Two cautions:

- **Upstream facts are source-verified, release history is not.** The FreeCAD claims
  below come from reading the pinned checkout at `33adf61` — specifically
  `src/Mod/Surface/App/{FeatureFilling,FeatureGeomFillSurface,FeatureSections,FeatureExtend,FeatureCut,FeatureSewing,Blending/FeatureBlendCurve}.h`,
  `src/Mod/Surface/App/{AppSurface,FeatureGeomFillSurface}.cpp`,
  `src/Mod/Surface/Gui/Command.cpp`, `src/Mod/Surface/App/CMakeLists.txt`,
  `src/Mod/Part/App/FeaturePartSpline.h` and `src/Mod/PartDesign/App/Body.cpp`. Claims
  about which release introduced a workbench, and the community issue numbers that
  float around this topic, are **unverified** here and are left out.
- **The FerroCAD column is an architectural assessment plus the current in-repo state.**
  "Exists" means the API or type is in this repository today. "Posture" is a proposed
  direction, not shipped work.

## 1. What the built-in Surface workbench actually contains

Inventory at `33adf61` (`src/Mod/Surface`), with the C++ base class each type derives
from:

| Python type | Base class | Core inputs |
| --- | --- | --- |
| `Surface::Filling` | `Part::Spline` → `Part::Feature` | boundary edges/faces + constraint orders, unbound edges/faces, free faces, constraint points, optional initial face |
| `Surface::GeomFillSurface` | `Part::Spline` | 2–4 boundary curves, per-curve reversal, fill style `Stretched` / `Coons` / `Curved` |
| `Surface::Sections` | `Part::Spline` | a list of section wires |
| `Surface::Extend` | `Part::Spline` | one face, independent U/V negative and positive extensions, symmetry flags, sample counts |
| `Surface::Sewing` | `Part::Feature` | shape list, tolerance, non-manifold / degenerate / free-edge options |
| `Surface::Cut` | `Part::Feature` | shape list |
| `Surface::FeatureBlendCurve` | `Part::Spline` | start and end edge, parameter, continuity, size |
| `Surface::BlendPoint`, `Surface::BlendCurve` | Python-side helpers | edge/parameter, continuity |

Three details matter more than the list:

- **The module is not trivial and it is not new.** The headers carry a 2014 copyright
  (Nathan Miller; `AppSurface.cpp` and `FeatureFilling.cpp`). Whatever release shipped
  it, the design predates the `PartDesign::Body` model that users now expect every
  feature to fit into.
- **`GeomFillSurface` emits either a Bézier or a B-spline face.** `getWire()` returns
  true only when every boundary edge is a Bézier curve; `execute()` then calls
  `createBezierSurface()`, otherwise `createBSplineSurface()`. So the "NURBS surface"
  output is sometimes not NURBS, which is a subtle compatibility point for anything
  that reads the result back.
- **Some commands are redirects.** `CmdSurfaceCurveOnMesh::activated()` does nothing
  but call `FreeCADGui.runCommand('MeshPart_CurveOnMesh')`, with a comment explaining
  that the string call avoids linking the Mesh module. The Surface workbench ships
  `Surface_BSplineSurface.svg` and `Surface_BezierSurface.svg` icons that no registered
  command uses.

The module links `FreeCADApp` and `Part` only (`App/CMakeLists.txt`); it does not link
`PartDesign`.

## 2. The feature objects are parametric; the command set is narrow

It is easy to describe Surface as "a few simple NURBS tools". The command coverage is
narrow, but the objects behind the commands are ordinary recomputing features:

`Surface::Filling` declares typed links (`App::PropertyLinkSubList` for
`BoundaryEdges`, `UnboundEdges`, `FreeFaces`, `Points`, `InitialFace`;
`App::PropertyStringList` for the faces; `App::PropertyIntegerList` for the per-boundary
constraint orders) and a full fitting parameter block — `Degree`, `PointsOnCurve`,
`Iterations`, `Anisotropy`, `MaximumDegree`, `MaximumSegments`, plus `Tolerance2d`,
`Tolerance3d`, `TolAngular` and `TolCurvature`. `execute()` builds a
`BRepFill_Filling`, loads the optional initial surface, adds the constraints and sets
`Shape`; on failure it returns `DocumentObjectExecReturn` with the kernel message, so
the failure lands in the tree as an error rather than a silently empty shape.

The properties are not decorative. The header comments state the continuity rules
directly: edges without a corresponding face are **C0 only**, while boundary and free
faces may be constrained to **C0, G1 or G2**, with the angular and curvature tolerances
bounding what counts as met. That is a G2-capable constraint solver behind a six-command
GUI.

### Design question

**Is the gap in the solvers or in the workflow?** Upstream evidence here points at the
workflow: the feature already models constraints, tolerances and failure, but the
commands expose a thin slice of it, and nothing bridges it into the history users
actually build.

### FerroCAD posture

Treat `Surface::Filling` as the reference for *feature ergonomics*, not just geometry.
A FerroCAD freeform feature should carry the same link-plus-constraint properties and
report kernel failures as feature errors — which the existing recompute path already
does — and expose the tolerances as ordinary editable properties rather than task-dialog
state.

## 3. One allow-list keeps surfaces out of a Body

`PartDesign::Body::isAllowed()` (`src/Mod/PartDesign/App/Body.cpp`) admits
`PartDesign::Feature`, `Part::Datum`, `Part::Part2DObject`,
`PartDesign::ShapeBinder`, `PartDesign::SubShapeBinder`, `App::VarSet`,
`App::DatumElement` and `App::LocalCoordinateSystem`. The line admitting bare
`Part::Feature` is **commented out**, next to a 2015-08-13 TODO by Fat-Zer and a note
that enabling the `Part::FeaturePython` variant failed to link on Windows.

Every built-in surface feature except `Cut` and `Sewing` derives from `Part::Spline`,
which derives from `Part::Feature` (`src/Mod/Part/App/FeaturePartSpline.h`). So the
exclusion is not about surfaces at all: it is a single class allow-list that was never
extended. `ShapeBinder` and `SubShapeBinder` are the only route from freeform geometry
into a Body.

The dependency direction makes a naive fix impossible: `Surface` links `Part` and never
`PartDesign`, so `Body::isAllowed` cannot name `Surface::Filling` without inverting the
module graph. The rule can only live in `Body`, and it has to be expressed about
something more general than a C++ type chain.

### Design question

**Is Body membership a class allow-list or a declared capability?** Today it is the
former. FerroCAD's `PartDesign::Body` stores `Group` as an untyped `LinkList`
(`crates/ferrocad_part_py/src/solids.rs`), so no rule exists yet — which is an
opportunity rather than a gap.

### FerroCAD posture

Make membership data, not inheritance: a feature type declares what it contributes to a
composition (a solid, a tool, a datum, a surface that participates at `Tip`, an
annotation), and the Body validates that declaration. That way a freeform feature enters
a history without `PartDesign` having to know the freeform module, and the upstream
allow-list never gets ported as a growing `isDerivedFrom` chain.

## 4. The surface commands are spread across five modules

Fragmentation is concrete here, not rhetorical:

- **Surface** owns filling, geometric fill, sections, extend, blend curve, sewing, cut.
- **MeshPart** owns curve-on-mesh; Surface's own command for it is a one-line redirect.
- **Part** owns the non-parametric surface primitives and document types
  (`Part::RuledSurface`, `Part::Extrusion` in FerroCAD; the `Part` module generally).
- **Sketcher** owns the 2D curves that most sections and boundaries come from.
- **Curves** is not in `src/Mod` at all — it is an external add-on, and therefore its
  status and API are outside the tree verified here.
- **`Part::FeaturePython`** absorbs whatever the above do not cover, with each script
  inventing its own properties, dependencies and recompute conventions.

### Design question

**One canonical freeform feature system, or a compatibility union?** The upstream model
is a union: several modules, some parametric, some not, one redirect, two orphan icons.

### FerroCAD posture

Keep the upstream names as the compatibility surface — `Surface::Filling` and friends
must resolve, and an `.FCStd` from upstream must load — while implementing them as one
internal feature family that shares constraint handling, tolerance defaults and failure
reporting. Also treat `Part::FeaturePython` as a first-class input to that family: a
scripted surface feature should be able to join a Body like a built-in one, which is the
case upstream's commented-out allow-list line was reaching for.

## 5. The cage is the missing user-facing artifact

Both implementations expose the control net as data. Upstream's
`Part.BSplineSurface` and FerroCAD's `Part.BSplineSurface`
(`crates/ferrocad_part_py/src/solids.rs`) provide poles, weights, multiplicities,
knot vectors and the two degrees; FerroCAD also builds from poles or interpolates points
through `GeomAPI_PointsToBSplineSurface`, and turns the result into a face with
`toShape()`. Both how-tos — [Model a NURBS surface](../../user/how-to/model-a-nurbs-surface.md)
and [Make a solid from a NURBS surface](../../user/how-to/solid-from-a-nurbs-surface.md) —
work from that data.

What neither offers is **direct manipulation of the cage as a document operation**:
dragging a pole, watching the surface and curvature respond, and committing one undoable
transaction.

### Design question

**Is cage editing a geometry feature or a viewport interaction over an existing
feature?** If it is a feature, every drag is a document edit and the cage becomes part of
the parametric model. If it is an interaction, the cage is a view over the feature's
poles and the commit writes them back once.

### FerroCAD posture

The second. `ferrocad_stage` already compiles viewport items to GPU batches,
`ferrocad_viewport`/`ferrocad_gpui` own gizmos and preview phases, and `mods/GizmoOverlay`
is the existing overlay precedent; `ferrocad_core` provides the transactions that make a
drag one undo step. The honest architectural gap is that **`ferrocad_curves` is curves
only** (`curve.rs`, `wire.rs`) — there is no pure-Rust surface algebra. Surface
evaluation currently comes from OCCT through `GeometryBackend`. So a live cage preview
that evaluates thousands of surface points per frame has to decide between kernel calls
with caching and a small Rust evaluator for the display mesh.

## 6. Solids from freeform surfaces

Upstream has document features for this: `Surface::Sewing` (tolerance plus
non-manifold, degenerate-shape, free-edge and control-only options) and `Surface::Cut`.

FerroCAD has the same kernel operations as **functions**, not recomputing features:
`Shape.sewShape(tolerance)` (`BRepBuilderAPI_Sewing` via `ferrocad_part::backend()`),
`Part.makeShellFromWires`, `Part.makeSolid`, `Part.makeFilledFace` (boundary wire →
filling surface), plus `Part.makeLoft`, `Part.makeRuledSurface` and `Part::RuledSurface` /
`Part::Extrusion` as document types. `Part.makeFilledFace` joins the edges into one wire
and fills it; upstream's fitting parameters are explicitly not implemented. The
user-facing recipe — extrude a face, or cap its open ends and sew — is already written
down in [Make a solid from a NURBS surface](../../user/how-to/solid-from-a-nurbs-surface.md).

### Design question

**Should closing a surface into a solid be a parametric feature or a script idiom?**
Today the answer differs by module: upstream makes it a feature, FerroCAD makes it a call.

### FerroCAD posture

Promote it to a feature when the freeform family exists — a "close / sew / thicken"
feature whose tolerances and options are properties and whose result stays editable,
rather than a `makeSolid` call that produces a frozen shape. Until then the how-to is the
correct home for it, because it is accurate about the function-level API.

## 7. Capability matrix

The upstream columns are source-verified for `33adf61`; the FerroCAD column is this
repository today, not a target.

| Capability | Upstream built-in | Upstream scripted | FerroCAD today | Posture |
| --- | --- | --- | --- | --- |
| B-spline surface data (poles, weights, knots, degrees) | `Part.BSplineSurface` | same | `Part.BSplineSurface` in `ferrocad_part_py` | exists |
| Surface from boundary curves, G0/G1/G2 | `Surface::Filling`, `Surface::GeomFillSurface` | `Part.makeFilledFace` (no fit parameters) | `Part.makeFilledFace` (no fit parameters) | feature family |
| Lofted / ruled surfaces | `Surface::Sections`, `Part::RuledSurface` | — | `Part.makeLoft`, `Part::RuledSurface` | exists |
| Extend a face | `Surface::Extend` | — | not present | candidate |
| Sew faces into a shell | `Surface::Sewing` (feature) | `Shape.sewShape` | `Shape.sewShape` | promote to feature |
| Trim surfaces against each other | `Surface::Cut` | booleans | booleans | exists |
| Curve blend with continuity | `Surface::FeatureBlendCurve` | `Part.BSplineCurve`, hand-built | no blend-curve feature | candidate |
| Surface in a `PartDesign::Body` | not allowed (`Body::isAllowed`) | via `SubShapeBinder` | no membership rule yet | declared capability |
| Direct cage manipulation | — | — | — | viewport interaction |
| Curvature comb / zebra | — | — | — | `ferrocad_stage` display |

## 8. What to validate next

1. **Read `BRepFill_Filling`'s actual constraint behaviour** through `Surface::Filling`
   on a real model: which `BoundaryOrder` values are accepted for edges versus faces,
   and how `TolAngular`/`TolCurvature` failures surface. The header comments are the
   spec; a script against upstream would confirm it.
2. **Decide the Body membership mechanism.** Write down the declared-capability schema
   and check it against the seven `isAllowed` cases plus a `Surface::Filling`, before any
   UI treats surfaces as first-class history entries.
3. **Measure cage-preview cost.** Tessellate a moderately dense patch from OCCT per frame
   versus from a cached grid, in `ferrocad_stage`, to see whether the kernel-only path is
   fast enough to skip a Rust surface evaluator.
4. **Audit `GeomFillSurface` output types.** Confirm how often `createBezierSurface` is
   taken in practice, since it changes what downstream code can assume about the face's
   surface type.
5. **Check `.FCStd` round-tripping of the surface types** FerroCAD already registers
   (`Part::RuledSurface`, `Part::Extrusion`) before adding `Surface::*` types, so the new
   types land on a tested serialisation path.
6. **Confirm the Curves workbench's licence and dependency surface** before referencing
   it as prior art in any accepted design; it is outside the tree verified here.
