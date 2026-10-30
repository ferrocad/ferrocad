# Surface, freeform and cage modelling

Status: ideation (2026-10-29). Exploratory product ideas. Nothing here is a
commitment, a roadmap, or an accepted design. Companion to [`README.md`](README.md), [`freecad-pain-points.md`](freecad-pain-points.md) (section 6),
[`intent-and-automation.md`](intent-and-automation.md),
[`app-shell-vision.md`](../direction/app-shell-vision.md), the
[backlog](../TODO-31-10-2026.md) and the
[developer architecture](../../developer/architecture.md).

Freeform modelling is the place where FreeCAD's answers are spread across the most
modules while the underlying geometry has been capable for over a decade. This note
separates three questions that are usually mixed together: the *kernel* question is
mostly settled, the *ownership* question (which object regenerates the surface) is not,
and the *distribution* question (native workbench, bundled add-ons, or host) is where
the editions fit in.

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
- **Curves** is not in `src/Mod` at all — it is an external add-on, so its source is
  outside the tree verified here (section 7 lists what the add-on index says about it,
  not what its code does).
- **`Part::FeaturePython`** absorbs whatever the above do not cover, with each script
  inventing its own properties, dependencies and recompute conventions.

### The upstream attempt: FEP-0014 (Forms)

Upstream's current proposal for the missing piece is a new workbench, not a
consolidation. FEP-0014 ("Forms", `FreeCAD/FreeCAD-Enhancement-Proposals` PR #59,
opened 2026-08-20, labelled `Stage: Draft`, still open when checked on 2026-10-29)
proposes interactive subdivision-surface modelling on a low-resolution polygonal control
cage: the cage is stored parametrically in the document and converted during recompute
into an OCCT BRep surface or solid, so the output stays ordinary BRep geometry.

The thread is the clearest public record of the questions this note is asking:

- **The name is not settled.** Forms (Fusion 360), Free-form / subdivision modelling
  (Solid Edge), Sculpt, Subdivision, FreeForm (SolidWorks) and Shape and Imagine (CATIA)
  are all proposed in the thread. An edition that ships this experience inherits that
  naming problem.
- **Subdivision or T-splines?** The proposal uses subdivision; one reviewer's
  understanding is that Rhino switched to subdivision from T-splines after Autodesk
  acquired T-splines, and that the two are not fully interchangeable for BRep output.
- **Where does it live?** A reviewer recommends shipping it as an add-on first; the author
  declines, on the grounds that it "relies on changes to PartDesign so it actually cannot
  be done" as an add-on, and that another add-on adds to the fragmentation.
- **How does it compose?** A user describes the Fusion flow: create a form body, edit it
  without caring about history, combine it with parametric bodies, and keep editing it
  from the timeline with the change propagating downstream.

The PartDesign dependency the author names is the allow-list from section 3, seen from
the other side: a cage workbench cannot enter a Body until `Body` admits its type.

### Design question

**One canonical freeform feature system, or a compatibility union?** The upstream model
is a union: several modules, some parametric, some not, one redirect, two orphan icons —
and the 2026 answer to the gap is another workbench rather than a consolidation.

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

What neither upstream's built-ins nor FerroCAD offer is **direct manipulation of the cage
as a document operation**: dragging a pole, watching the surface and curvature respond,
and committing one undoable transaction. A third-party workbench now claims exactly that
(section 7), which turns "is this possible?" into "where should it live?".

FEP-0014 answers half of this already: its cage is document data, rebuilt at recompute.
It leaves the other half open — the thread asks whether cages should be driven by
dimensions and constraints, and whether editing should preserve history.

### Design question

**Is cage editing a geometry feature or a viewport interaction over an existing
feature?** These are separable: *storage* decides whether the cage is a recomputing
property, *editing* decides whether a drag is one document edit or a preview that commits
once.

### FerroCAD posture

Both, in that order. Store the cage as a property of a recomputing feature (the FEP's
choice), and make the drag an interaction rather than a transaction per frame:
`ferrocad_stage` already compiles viewport items to GPU batches, `ferrocad_viewport`
already separates `PreviewPhase` from `InteractionCommit`, `ferrocad_gpui` hosts the
gizmos, and `mods/GizmoOverlay` is the existing overlay precedent; `ferrocad_core`
provides the transactions that make the commit one undo step. The honest architectural
gap is that **`ferrocad_curves` is curves only** (`curve.rs`, `wire.rs`) — there is no
pure-Rust surface algebra. Surface evaluation currently comes from OCCT through
`GeometryBackend`. So a live cage preview that evaluates thousands of surface points per
frame has to decide between kernel calls with caching and a small Rust evaluator for the
display mesh.

The reference implementation in section 7 is also a useful shape: it splits the job into
two document objects — the cage, and a `Part::FeaturePython` solid with an
`App::PropertyLink` back to it that rebuilds `Shape` in `execute()` — rather than one
large feature. That is the decomposition to copy, because the linkage is what carries the
lineage from cage edits to the solid.

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

## 7. Three ways to ship freeform: host, bundle, native

Rhino keeps coming up as the comparison. Underneath the branding it is four
capabilities — NURBS surfacing, subdivision modelling, mesh handling and a node-based
workflow. All four now exist as FreeCAD workbenches, but not in the same place: the
official index (`FreeCAD/Addons`, `Data/Index.json`, 180 entries on 2026-10-29) has the
NURBS, mesh and node tools, while the only subdivision workbench is installed from a
custom repository. These are the entries that map onto the freeform story:

| Add-on | Repository | GitHub description |
| --- | --- | --- |
| `Curves` | `tomate44/CurvesWB` | "Experiments for a FreeCAD Surface workbench" |
| `Silk` | `edwardvmills/Silk` | "NURBS Surface modeling tools focused on low degree and seam continuity" |
| `nurbs` | `microelly2/freecad-nurbs` | *(none)* |
| `CurvedShapes` | `chbergmann/CurvedShapesWorkbench` | "Workbench for creating 3D shapes from 2D curves" |
| `ImportNURBS` | `KeithSloan/ImportNURBS` | "Add Import of 3DM files to FreeCAD" |
| `MeshRemodel` | `mwganson/MeshRemodel` | "used to aid in manually remodeling mesh objects" |
| `Nodes` | `j8sr0230/Nodes` | "Visual scripting workbench" |
| `Ship` | `FreeCAD/freecad.ship` | "aiding in naval vessel design" |
| `SubD Modeler` | `sedatkrn/FcSubDModelerWB` | LightWave/Blender-style cage modelling in the 3D view, converted to a B-rep solid — **not in the official index** |

The last one is worth reading closely, because it is the closest thing to a worked answer
to sections 5 and 6. Its source:

- stores the cage on a `Mesh::FeaturePython` object (`CagePoints` + `CageFaces`) and
  previews the Catmull-Clark surface as a mesh;
- builds each quad as an untrimmed patch with
  `Part.BSplineSurface.buildFromPolesMultsKnots(...)` + `toShape()`;
- assembles the patches with `Part.Shell(faces)`, `shell.sewShape(...)`, and
  `Part.Solid(...)` when exactly one closed shell comes back;
- wraps the result in a second object, a `Part::FeaturePython` with an `App::PropertyLink`
  back to the cage plus `Samples` and `Tolerance`, which rebuilds `Shape` in `execute()`
  and follows the cage's `Placement`.

FerroCAD already answers every geometry call in that list — `Part.BSplineSurface`,
`Part.Shell`, `Part.Solid`, `Shape.sewShape`, `Part.makeFace`. The one thing it lacks is
the document type the cage lives on: its `Mesh` module registers `Mesh.Mesh` and
`MeshPart`, but not `Mesh::FeaturePython`. So the cost of hosting this particular
workbench is a document type, not geometry.

Two caveats before treating it as the answer. It is early — version 0.2.0, first commit
2026-10-27, MIT — and the accuracy figures (about 0.02 mm deviation at its G0 preset,
below 0.0001 mm at G1/G2) are the author's README claims, not something this note
measured. And its solid being a `Part::FeaturePython` means it is exactly the type §3's
commented-out allow-list line excludes, so it reaches a Body only through a binder.

None of them is in `src/Mod` at `33adf61`. So the question is not whether the capability
exists in the ecosystem — it does, several times over — but how a FerroCAD distribution
should present it. Three tiers, not mutually exclusive:

| Tier | What it is | What it costs in FerroCAD | What it cannot do |
| --- | --- | --- | --- |
| **Host** | run the existing Python workbenches through the facade | the Qt shim already used for Draft ([`qt-strategy.md`](../../developer/workbenches/qt-strategy.md)), plus `pivy.coin` (answered by `ferrocad_inventor_py`); a workbench whose objects live on a type FerroCAD does not register needs that type first | no new manipulation UX; each add-on's document objects arrive with its own conventions |
| **Bundle** | an edition that pre-installs a chosen set and brands the experience | a payload plus a `HostConfig`: an edition that only changes config, scripts or branding shares a binary ([`edition-publishing.md`](../../developer/architecture/edition-publishing.md) §1) | cannot close gaps that need new Rust; the add-ons' licences and maintenance become the edition's |
| **Native** | freeform features and cage tools owned by FerroCAD | a linked module with new document types and a manipulator contributed by the edition — its own binary, like `ferrocad_parametric` | slowest, and duplicates geometry an add-on already solved |

The bundle tier is cheap because of a decision already made: the base app is published to
crates.io and stays kernel-free, editions are thin binaries that register modules
(`ferrocad_combined_py::register_builtins()`) and contribute manipulators
(`Contributions::with_manipulator`), and `HostConfig` carries the app name, entry module
and Python paths. A `Sculpt` (or `Freeform`) edition that bundles `Curves`, `Silk`,
`CurvedShapes`, `Nodes` and `SubD Modeler` needs no new Rust composition; a *native* cage
tool does. The SubD workbench is MIT, which at least removes the licence question from
that set.

### Design question

**Does FerroCAD compete at the feature level or the composition level?** Feature-level
means more commands than `Curves` or `Silk`; composition-level means the same commands
behave better because they share a document model, lineage and one undo stack.

### FerroCAD posture

Composition. The differentiator is not a longer command list; it is that a bundled
add-on's operations gain FerroCAD interaction — cage gizmos, live preview, one-undo
drags, lineage — instead of a Qt dialog and a frozen shape. Sequence it host first
(cheapest, and it proves the facade against real code), bundle second (a named edition
with a curated set), native third and only for the gaps the first two cannot reach:
the membership rule in section 3, cage editing in section 5, and the parametric
surface-to-solid features in section 6.

Two caveats. Bundling third-party workbenches makes their licences and their upstream
maintenance part of the edition's support surface; that needs a per-add-on review this
note does not do. And in the FEP thread the third-party add-on route is exactly the
thing some commenters call fragmentation, so an edition that bundles add-ons is a
distribution choice, not an answer to the upstream design argument.

## 8. Capability matrix

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
| Direct cage manipulation | — | `SubD Modeler` (unindexed) | — | viewport interaction |
| Subdivision cage → BRep | FEP-0014 (draft, unmerged) | `SubD Modeler` (unindexed, MIT, Python) | not present | bundle first, then native |
| Freeform experience out of the box | `Surface` only (seven features) | 180-entry index plus unindexed workbenches | editions choose modules and payload | bundle |
| Curvature comb / zebra | — | — | — | `ferrocad_stage` display |

## 9. What to validate next

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
6. **Review the licence and dependency surface of the bundle set** (`Curves`, `Silk`,
   `CurvedShapes`, `Nodes`) before any edition ships them; they are outside the tree
   verified here, and bundling makes their licences and maintenance part of the edition's
   support surface.
7. **Host spike: `Curves`, `Silk` and `SubD Modeler`.** Load them through the facade, run
   their non-GUI objects headlessly, and record which `Part`/`PartGui` calls they make
   that FerroCAD does not answer. For the SubD workbench, also check whether registering
   `Mesh::FeaturePython` is enough or whether its in-viewport Qt overlay needs shim work.
8. **Watch FEP-0014 and its AstoCAD implementation.** If Forms lands upstream, decide
   whether FerroCAD hosts it or bundles its own; do not design a native cage format in
   advance of that decision.
9. **Choose the cage-to-BRep technology** — subdivision (Rhinoceros and Blender
   interchange), T-splines (Fusion lineage, Autodesk-encumbered) or NURBS only — because
   the choice constrains both what a cage can express and what a bundled add-on can
   exchange with it. `SubD Modeler` has already picked subdivision with per-quad bicubic
   patches, so evaluate that implementation before inventing another.
