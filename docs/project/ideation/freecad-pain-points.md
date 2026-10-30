# FreeCAD pain points and the FerroCAD response

Status: ideation/analysis (2026-10-28). A qualitative consolidation of
community-reported pain points and an architectural review; not a benchmark or a
commitment. Companion to [`README.md`](README.md),
[`intent-and-automation.md`](intent-and-automation.md), the
[backlog](../TODO-31-10-2026.md) and the
[developer architecture](../../developer/architecture.md).

This note collects the recurring FreeCAD pain points that a FerroCAD host could
address, then maps each one to the crate or layer that would carry the response. It is
a working document for prioritisation, not a claim that every item is solved.

Two cautions:

- **The pain points are qualitative.** They come from user reports and observed
  upstream behaviour, not a controlled benchmark. Treat frequencies and remarks such
  as "about three times as many clicks" as impressions.
- **The FerroCAD column is an architectural assessment.** "Solved" means the design
  removes a root cause; it does not mean the user-facing workflow has shipped and been
  measured. Items that depend on upstream workbench logic or on new mathematics are
  called out as such.

## 1. UX and workflow friction

### Pain points

- Excessive clicks: roughly three times as many as Solid Edge or NX in common flows,
  with too many manual confirmations and activations that should be automatic.
- Poor intent prediction: clicking a face and then **Sketch** should start a sketch on
  that face, but FreeCAD warns and forces a body selection from a list unless the body
  is already active.
- No soft protection for unconstrained geometry: fillets, trims and extends can make
  unconstrained geometry move "willy-nilly"; commercial CAD keeps an implied temporary
  hold during the operation.

### Upstream root causes

- **"Strict mode" philosophy.** The architecture prioritises explicit mathematical
  determinism over heuristic inference. Commercial packages invest heavily in intent
  engines; requiring an explicit Active Body historically avoided ambiguous dependency
  graphs.
- **Sketcher solver rigidity.** The Eigen-based GCS solver computes exact degrees of
  freedom and has no intermediate soft springs or drag damping. Trimming or extending
  an unconstrained line therefore produces the least-cost mathematical solution, which
  can be a radical coordinate jump. Commercial tools apply hidden soft anchors (virtual
  grounded springs) during drag and trim so geometry keeps its approximate screen
  position.

### FerroCAD posture and revision

- **Direct viewport interaction is already a strength.** `ferrocad_stage` and
  `ferrocad_viewport` replace dialog-heavy flows and Coin3D's `SoDragger` /
  `SoEventCallback` with declarative manipulators, anchored gizmos, the radial wheel,
  the floating move form and rich snap feedback. Workbench manipulators (an edge
  fillet arc, a thickness grip) register through the generic `Manipulator` seam and
  `InteractionCommit`, which cuts click counts substantially. See
  [`selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md).
- **The remaining gap is intent and soft constraints.** FerroCAD aims to run unmodified
  upstream Python workbenches through its `FreeCAD`/`FreeCADGui` facade, so any Active
  Body enforcement inside upstream workbench logic carries over unless the host shell
  intercepts it. Soft anchors during sketch drag or trim need either a damped 2D
  constraint solver in Rust or pre-weighted parameters before the solver is invoked.

## 2. External geometry and referencing

### Pain points

- The ShapeBinder layer feels foreign: there is no direct WYSIWYG referencing. A visible
  edge cannot simply be referenced because it belongs to another body; the user must
  backpedal and explicitly create a binder first.

### Upstream root causes

- **The PartDesign DAG boundary.** A Body is a single contiguous solid; referencing
  geometry outside that container directly in a sketch violates its local scoping.
- **ShapeBinder as a firewall.** `SubShapeBinder` is a protective boundary for
  dependency tracking and topological-naming mitigation, not a UX convenience. Without
  it, an external reference could create a cycle or break recompute when topology is
  re-indexed.

### FerroCAD posture and revision

- **Kernel-neutral selection already exists.** Viewport picks produce a kernel-neutral
  `SelectionRef` and stable `ElementRef` with explicit provenance in
  `ferrocad_viewport` and `ferrocad_lineage`. Because `ferrocad_core` owns the DAG and
  `ferrocad_lineage` tracks cross-operand provenance, the interactive tool layer in
  `ferrocad_stage` has the foundation to synthesize or implicitly manage cross-body
  links when a user clicks an external edge.
- **Caveat.** Upstream workbench code still owns its own Body/ShapeBinder rules. The
  host can make the common case implicit, but it cannot silently change the semantics
  of an upstream feature that expects a binder object.

## 3. Model tree and selection

### Pain points

- Selecting a Body in the 3D view and right-clicking targets the Body's **Tip** rather
  than the Body container, so body-level operations such as Transform are missing from
  the context menu; the user hunts through the tree to select the actual Body.
- Auto-expansion on errors shows parts the user is not working on and loses highlight
  and scroll position.
- The tree does not autoscroll to a part selected in the 3D view.

### Upstream root causes

- **Tip versus container.** A Body is a feature accumulator, not a standard viewport
  object. The viewport renders the Tip (the current end state, for example `Pad002`),
  not the abstract Body wrapper. Clicking a face raycasts the Tip's evaluated shape,
  and the scene graph historically lacked a context-aware selection dispatcher.
- **Tree-view sync deficits.** The Qt tree and the document engine are decoupled.
  Error auto-expansion fires on broken DAG nodes; bidirectional sync such as
  viewport-to-tree autoscroll was omitted because deep Qt tree traversals on every
  viewport click were expensive.

### FerroCAD posture and revision

- **The GPUI shell removes the Qt bottleneck.** A GPU-accelerated `ferrocad_gpui` shell
  and a virtualized inspector make bidirectional autoscroll and preserve-scroll-on-error
  feasible without UI freezes.
- **Context-aware selection routing.** `InventorViewport` emits a `SelectionEvent`
  carrying a `SelectionRef`; the host maps it to a document object. Because the host
  mediates the mapping and owns the radial wheel and anchored gizmos, it can expose
  both feature-level and parent-container actions on right-click or hover, resolving
  the Tip-versus-Body hunt.

## 4. Recompute performance

### Pain points

- Editing even an unreferenced cell heading in a spreadsheet can trigger a full
  recompute, which is slow on complex models.

### Upstream root causes

- **Coarse dependency tracking.** The DAG triggers recomputes at the object level, not
  the sub-property level. Any cell change marks the entire `Spreadsheet::Sheet` dirty.
- **Downstream cascades.** Every feature or sketch referencing any property of that
  spreadsheet is flagged, so even a cosmetic string-label change forces sequential
  re-evaluation downstream.

### FerroCAD posture and revision

- **The dependency graph is ours.** `ferrocad_core` reimplements the document object
  model, typed properties and the recompute graph in Rust. While it keeps conformance
  with FreeCAD's public `Document.py` API, the internal graph can track fine-grained
  property and cell-alias edges rather than coarse whole-object dirty flags.
- **Caveat.** This is a design capability. The conformance surface and real-world
  spreadsheet workloads still need measurement before claiming a speed-up.

## 5. Geometry robustness (fillets, chamfers, pockets)

### Pain points

- Fillets and chamfers fail, sometimes spectacularly, on complex geometry and
  occasionally on elementary shapes; features cannot consume filleted or chamfered
  geometry reliably.
- Pocketing into curved surfaces can explode the model into invalid geometry and
  requires extra care around edges and seams.

### Upstream root causes

- **OCCT kernel limits.** Unlike commercial kernels such as Parasolid or ACIS, OCCT's
  B-Rep blending struggles with complex vertex topologies, especially rolling-ball
  fillets where three or more radii converge or where a fillet terminates tangent to a
  curve.
- **Non-manifold transitions.** Pocketing into curved surfaces can create
  infinitesimal tolerances or tangent self-intersections at seams. OCCT enforces
  manifold geometry, so near-zero-length edges or indeterminate face boundaries fail
  the Boolean and collapse the body evaluation.

### FerroCAD posture and revision

- **The kernel is inherited, but the seam is ours.** OCCT limits remain in
  `ferrocad_occt`, but `ferrocad_geom::GeometryBackend` isolates the kernel and
  `ferrocad_curves` provides pure-Rust analytic primitives. That seam allows
  preprocessing and healing of seam geometry, interactive fallback previews in
  `ferrocad_stage`, or a future alternative B-Rep kernel without rewriting the
  document model. See [`occt-integration.md`](../../developer/architecture/occt-integration.md)
  and [`part-draft-layering.md`](../../developer/architecture/part-draft-layering.md).

## 6. Surface modeling

### Pain points

- Insufficient control for compound curves such as car panels, boat hulls and mice.
  FreeCAD can "sort of" do it, but it is painful. Blender is not a substitute: it is
  mesh-based and lacks exact analytical tolerances and watertight STEP output.

### Upstream root causes

- **No G2/G3 continuity solvers.** The Surface and Curves workbenches offer basic
  Bézier/B-spline patches but lack Class-A surfacing: curvature-comb visualisation,
  boundary matching with G2 (curvature) or G3 (torsion) continuity, and multi-sided
  blends.
- **The mesh/NURBS divide.** Polygonal meshes and subdivision surfaces lack exact
  analytical tolerances; analytical NURBS via OCCT are unforgiving unless the control
  net is well conditioned.

### FerroCAD posture and revision

- **The foundation is laid; the solvers are still open.** `ferrocad_curves` provides
  pure-Rust analytic primitives (Bézier and NURBS curves, conics, wires), and
  `ferrocad_stage` compiles declarative items directly to GPU draw batches. Real-time
  curvature combs, zebra striping and interactive control-net cages are therefore
  straightforward to render in `ferrocad_wgpu`.
- **Caveat.** True Class-A G2/G3 surface continuity and multi-sided blending are
  dedicated mathematical work, not a consequence of the core rewrite.

## 7. TechDraw

### Pain points

- TechDraw is hard to use for a presentable professional drawing.

### Upstream root causes

- **HLR performance.** 2D vector projections from 3D B-Rep depend on OCCT's hidden-line
  removal, which is CPU-intensive on complex geometry and can produce duplicate,
  broken or missing lines.
- **Dimension detachment.** Dimensions project from 3D sub-elements. On recompute and
  edge renumbering they can lose vertex bindings, leaving broken references, redrawn
  leaders and manual cleanup after minor upstream changes.

### FerroCAD posture and revision

- **Dimension detachment is mitigated by the lineage layer.** `ferrocad_lineage` folds
  operation history into a persistent lineage (`ElementMap` and `ReverseMap`), so
  projected dimensions can bind to stable `ElementRef` identities and recover
  split/merged edges through `ReverseMap` rather than positional `EdgeN`/`VertexN`
  indices. See [`lineage-and-naming.md`](../../developer/architecture/lineage-and-naming.md).
- **The HLR bottleneck remains.** If 2D projection continues to rely on OCCT's CPU-bound
  HLR through `ferrocad_occt`, projection speed and line artifacts persist unless the
  work moves to background Rust threads or is supplemented by GPU-assisted
  silhouette/depth-buffer extraction in `ferrocad_wgpu`.

## 8. Design management and modularity

### Pain points

- If the whole design cannot be held in one head, it must be split into modules, or
  unavoidable changes become unmanageable.

### Upstream root causes

- **Monolithic memory footprint.** FreeCAD does not automatically virtualize or create
  lightweight representations of inactive bodies; large top-down designs cause memory
  bloat and long recompute stalls.
- **No PDM primitives.** There is no native product data management for revision
  tracking, part configurations or lifecycle states. Users fall back to strict
  bottom-up sub-assembly modelling (`App::Part` or external file references) to isolate
  dependency chains.

### FerroCAD posture and revision

- **Headless core, lightweight visuals.** `ferrocad_core` is a headless pure-Rust
  object model, and `ferrocad_graphics`/`ferrocad_stage` provide a shared draw-batch
  vocabulary. Inactive bodies can be retained as lightweight GPU draw batches in
  `ferrocad_wgpu` without keeping heavy Coin3D node trees alive.
- **Caveat.** This is not PDM. Revision tracking, configurations and lifecycle states
  are a separate product design, and editions/packaging are not a substitute for them.

## 9. Topological naming problem (TNP)

### Pain point

- Changing an early feature in the tree shifts internal edge/face ids (for example
  `Face3` to `Face5`), breaking downstream sketches, fillets and datum attachments.

### Upstream root cause

- OCCT historically lacked persistent internal topology tracking. FreeCAD 1.0
  integrates long-awaited TNP-mitigation algorithms, but legacy files and deeply nested
  dependencies can still encounter edge-tracking friction.

### FerroCAD posture and revision

- **Naming is a first-class architectural primitive.** Rather than bolting mitigation
  onto C++ `TopoDS_Shape` pointers, FerroCAD isolates topological naming in
  `ferrocad_lineage`, a kernel-independent Rust crate. Every operation records a
  `History` (modified, generated and deleted relations with operand ids and
  provenance), which folds forward into a shape's `Lineage` (`ElementMap`,
  `ReverseMap` and `MapVersion` under a pluggable `NamePolicy`). Sub-elements are
  addressed by stable `ElementRef` names rather than positional indices, so TNP
  resilience is testable in pure Rust independently of the kernel.

## 10. Assembly fragmentation

### Pain point

- For years FreeCAD lacked an official core assembly workbench, forcing users to choose
  between competing add-ons (A2plus, Assembly3, Assembly4), each with incompatible
  constraint solvers.

### Upstream root cause

- FreeCAD 1.0 integrated a native Assembly workbench based on the Ondsel solver, but
  compatibility, joint kinematics and multi-file referencing workflows are still
  maturing compared with commercial mates.

### FerroCAD posture and revision

- **Dual-track strategy.** The `FreeCAD`/`FreeCADGui` and `pivy.coin`
  (`ferrocad_inventor_py`) facades are designed to host unmodified upstream Python
  workbenches, while the release matrix distinguishes the base `ferrocad` app from the
  `FerroCAD: Parametric` edition. FerroCAD is positioned to offer a cohesive first-party
  parametric and assembly experience alongside legacy workbench support.
- **Caveat.** A first-party assembly workbench is a product/edition decision; the
  facade and edition split only make it possible.

## 11. Part versus PartDesign

### Pain point

- Two modelling paradigms coexist: CSG boolean primitives in Part and a feature-tree
  single-solid body in PartDesign. Mixing them produces broken DAG trees and user
  confusion.

### Upstream root cause

- Historical baggage: Part was the original workbench around raw OCCT operations;
  PartDesign was added later to mimic modern parametric feature-tree modelling, but
  legacy workflows were never deprecated.

### FerroCAD posture and revision

- **API preservation, UI unification.** At the engine level, FerroCAD preserves
  `Part::Feature` and `Part::PropertyPartShape` in `ferrocad_part` so existing scripts
  and upstream workbenches such as Draft run unchanged. Because `ferrocad_gpui` and
  `ferrocad_stage` control the interactive presentation, primitives, booleans and
  feature-tree bodies can be presented and manipulated more consistently without
  breaking Python API compatibility.
- **Caveat.** The compatibility constraint limits how far the UI can unify the two
  models without surprising existing scripts.

## 12. Undo/redo instability

### Pain point

- Pressing Ctrl+Z across workbench transitions or during multi-step operations can
  corrupt document state, drop constraint data or crash the application.

### Upstream root causes

- Undo transactions rely on C++/Python state serialization that does not always roll
  back intermediate UI state or complex sub-object edits cleanly.

### FerroCAD posture and revision

- **Transactions are native.** In `ferrocad_core`, transactions and undo/redo are
  value-oriented Rust primitives in the document object model, eliminating
  C++/Python pointer aliasing and partial-rollback crashes. On the UI side,
  `ferrocad_stage` and `ferrocad_viewport` separate transient interaction
  (`PreviewPhase`) from final state mutations (`InteractionCommit`), so in-flight drags
  never pollute the document transaction log.
- **Caveat.** Python-backed custom features must participate correctly in the same
  transaction discipline; the core provides the mechanism, not automatic correctness
  for arbitrary Python side effects.

## 13. Single-core CPU bottleneck

### Pain point

- Large recomputes, complex sketches and meshing operations cannot use multiple cores,
  leaving modern CPUs largely idle.

### Upstream root causes

- OCCT's core modelling kernel and FreeCAD's document dependency-resolution engine are
  fundamentally single-threaded in the paths that dominate these operations.

### FerroCAD posture and revision

- **Offload what can be offloaded.** FerroCAD shifts scene compilation
  (`ferrocad_inventor`, `ferrocad_stage`) and rendering (`ferrocad_wgpu`) onto modern
  GPU draw batches. `ferrocad_core`'s pure-Rust DAG can identify independent branches
  for parallel evaluation, and mesh tessellation can be parallelized across faces in
  Rust.
- **Caveats.** CPython's GIL still constrains Python-backed custom features, and C++
  OCCT has its own thread-safety constraints in `ferrocad_occt`. Parallel scheduling is
  an optimization to design and measure, not a free consequence of Rust.

## Summary matrix

The impact column is an architectural assessment, not a measured result.

| # | Pain point | FerroCAD impact | Key crates |
| --- | --- | --- | --- |
| 1 | UX / workflow friction | High (interaction), partial (solver): wheel, floating form, gizmos and snap feedback modernize the flow; soft anchors still need a damped solver. | `ferrocad_gpui`, `ferrocad_viewport`, `ferrocad_stage`, `ferrocad_widgets` |
| 2 | External geometry and referencing | High: kernel-neutral `SelectionRef`/`ElementRef` provenance lets tools automate cross-body binding without manual backpedaling. | `ferrocad_lineage`, `ferrocad_viewport`, `ferrocad_core` |
| 3 | Model tree and selection | Solved / high: GPUI removes Qt tree lag; host-mediated `SelectionEvent` plus the radial menu resolves Tip-versus-Body context menus. | `ferrocad_gpui`, `ferrocad_viewport`, `ferrocad_widgets` |
| 4 | Recompute performance | High: the pure-Rust DAG and typed properties allow fine-grained dependency tracking. | `ferrocad_core`, `ferrocad_py` |
| 5 | Geometry robustness | Abstracted: OCCT limits remain, but `GeometryBackend` decouples the engine for healing or future kernels. | `ferrocad_geom`, `ferrocad_occt`, `ferrocad_part`, `ferrocad_curves` |
| 6 | Surface modeling | Foundational: `ferrocad_curves` and GPU `ferrocad_stage` enable NURBS/comb visuals; G2/G3 solvers remain future work. | `ferrocad_curves`, `ferrocad_stage`, `ferrocad_geom` |
| 7 | TechDraw | Partial: lineage prevents dimension detachment; OCCT HLR speed still needs threading or GPU silhouette offloading. | `ferrocad_lineage`, `ferrocad_occt`, `ferrocad_wgpu` |
| 8 | Design management and modularity | High: headless core plus draw batches allow lightweight inactive bodies without Coin3D bloat. PDM is still separate. | `ferrocad_core`, `ferrocad_graphics`, `ferrocad_stage` |
| 9 | Topological naming | Solved architecturally: `ferrocad_lineage` tracks history, provenance, element maps and reverse maps per operation. | `ferrocad_lineage`, `ferrocad_geom` |
| 10 | Assembly fragmentation | Addressed via editions: upstream workbenches through the facade, plus the `FerroCAD: Parametric` edition. | `ferrocad`, `ferrocad_py`, `ferrocad_inventor_py` |
| 11 | Part versus PartDesign | Mitigated in the UI: preserve `Part::Feature` for scripts while unifying interactive manipulation in `ferrocad_stage`. | `ferrocad_part`, `ferrocad_stage`, `ferrocad_gpui` |
| 12 | Undo/redo instability | Solved architecturally: native Rust transactions plus strict `PreviewPhase`/`InteractionCommit` separation. | `ferrocad_core`, `ferrocad_viewport`, `ferrocad_stage` |
| 13 | Single-core CPU | High: the DAG enables parallel branch scheduling and wgpu offloads rendering, bounded by the GIL and OCCT thread safety. | `ferrocad_core`, `ferrocad_wgpu`, `ferrocad_inventor` |

## What to validate next

1. **Click counts.** Measure representative PartDesign, Draft and TechDraw flows against
   the "about three times" impression before making it a target.
2. **Spreadsheet dependency microbenchmark.** Compare a cell-label edit against a
   geometry-driving cell edit in `ferrocad_core` and in upstream FreeCAD.
3. **Cross-body referencing.** Prototype the implicit-binder path with the actual
   selection and lineage APIs, including a cyclic-reference failure case.
4. **Soft anchors.** Prototype a damped 2D solver (or pre-weighting) on trim/extend and
   measure the coordinate jump against the current behaviour.
5. **HLR.** Measure OCCT HLR time on representative drawings, then compare background
   threading and a GPU silhouette/depth experiment.
6. **Recompute parallelism.** Identify independent DAG branches in a real model and
   measure the speed-up, including GIL and OCCT contention.
7. **Assembly and editions.** Decide whether the first-party assembly story is part of
   the base app, the Parametric edition, or a separate edition before designing it.
