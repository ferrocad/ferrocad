# Chapter 5: How FerroCAD organizes this

FerroCAD reimplements the FreeCAD API and splits the same pipeline into crates with
explicit seams. The first four chapters describe the concepts; this chapter maps them.

### 5.1 Geometry and the kernel

| Crate | Role |
| --- | --- |
| `ferrocad_types` | The value vocabulary: `Vector`, `Placement`, `Quantity`, units and matrices. No geometry kernel. |
| `ferrocad_curves` | Analytic curves and wires that do not need OCCT. |
| `ferrocad_geom` | The geometry seam: opaque `Shape`, lineage re-exports and the `GeometryBackend` trait. |
| `ferrocad_occt` | The OCCT backend: implements `GeometryBackend`, owns the OCCT 7.8 types, operations and operation history. |
| `ferrocad_part` | The Part workbench: registers `Part::Feature` and `Part::PropertyPartShape` on top of the backend (and injects the OCCT backend from this crate). |
| `ferrocad_mesh` | The triangle-mesh core: STL/bms, analysis/editing and `Mesh::Feature`. **No OCCT.** |
| `ferrocad_core` | Documents, properties, links, expressions and the recompute DAG. No kernel and no UI. |

The `Part` path follows the vector workflow: OCCT holds NURBS and B-Rep, and
`Part::Feature` is the document object that carries a shape. The `Mesh` path follows
the bitmap workflow: `ferrocad_mesh` holds triangles and never links OCCT. Both paths
meet in the document core, not in the kernel.

The mesh path is reachable from Python without the kernel. `Mesh.Mesh((points, facets))`
builds a mesh from raw triangles, and `MeshPart.meshFromShape` produces one from any
shape with a `tessellate` method (duck-typed, so the mesh crate still has no OCCT
dependency). Either result can be carried by `Mesh::Feature`.

### 5.2 Documents and recompute

`ferrocad_core` owns the document graph, properties, transactions and the recompute
DAG. `Part::Feature` and `Mesh::Feature` are object types registered in the one
process-wide core; the Python modules are views over it, not separate models.

This is also why the edition and the combined Python extension can share one core:
[`kernel-state-sharing.md`](../developer/architecture/kernel-state-sharing.md) records
the one-core invariant and the compositions that enforce it.

### 5.3 Naming: the lineage layer

This is where FerroCAD's layering differs most from upstream. Upstream already keeps the
map type in the base layer (`Data::ComplexGeoData`) and derives names inside the Part
wrapper (`Part::TopoShape`), as Chapter 4 describes. FerroCAD makes the vocabulary a
kernel-independent crate and separates the three concerns into three places:

- **`ferrocad_lineage` owns the payload-generic vocabulary.** `ElementRef<T>`,
  `ElementId`, `OperandId`, `Provenance`, `History<T>`, `ElementMap<T>`, `ReverseMap`,
  `Lineage<T>`, `MapVersion` and the `NamePolicy` trait. It has no workspace
  dependencies and knows nothing about OCCT or Part.
- **The kernel reports provenance.** `ferrocad_occt` translates OCCT's operation
  history into `History<Shape>` and folds it into a `Lineage`
  (`Lineage::from_history`). `ferrocad_geom` re-exports the vocabulary with `Shape` as
  the payload, so downstream code does not name the lineage crate directly.
- **Part stores and applies the policy.** `ferrocad_part` persists the lineage in
  `ShapeProperty`, folds the backend history after boolean and dress-up operations, and
  resolves stored sub-element names through the map first. A name absent from a
  non-empty map is reported as retired; it is not silently rebound to a positional
  neighbour.

Two things follow, and they are worth keeping apart:

- **The storage and types are kernel-neutral.** A `Lineage<String>` has no geometry in
  it, and the vocabulary is covered by unit tests that never link OCCT.
- **The naming semantics are still OCCT-shaped.** The backend mirrors OCCT's history
  behaviour on purpose. No second kernel has been run through the layer, and a
  pluggable naming *engine* is not implemented. That remains an open research
  question, written up in
  [`tns-decoupling.md`](../project/research/tns-decoupling.md); do not read the crate
  boundary as a promise of kernel portability.

See [`lineage-and-naming.md`](../developer/architecture/lineage-and-naming.md) for the
implemented design, [`tns-decoupling.md`](../project/research/tns-decoupling.md) for
the open question, and [`geometry-and-topology.md`](../project/research/geometry-and-topology.md)
for the earlier research.

### 5.4 Presentation

The presentation stack is deliberately layered, so the renderer never needs the
kernel:

| Crate | Role |
| --- | --- |
| `ferrocad_graphics` | The neutral draw contract: draw batches, camera/viewport, CPU picking. No wgpu, no gpui. |
| `ferrocad_stage` | The declarative presentation: `ShapeItem`s, tools, the `TransformGizmo` and the `Manipulator` seam. Workbench-agnostic. |
| `ferrocad_inventor` | The Coin/Inventor scene core: node AST, state accumulator, mesh compiler and events. Renderer-agnostic. |
| `ferrocad_wgpu` | The wgpu backend: pipelines, uniforms, MSAA, offscreen target, readback and the deterministic snapshot path. |
| `ferrocad_viewport` | The gpui viewport over the scene: picking, selection, the navigation cube and the workbench manipulators. |
| `ferrocad_ui` / `ferrocad_ui_py` | The Qt Designer `.ui` reader and typed form model. No Qt. |
| `ferrocad_widgets` / `ferrocad_gpui` | The reusable shell widgets and the app shell that boots CPython. |

`ferrocad_inventor_py` is the `pivy.coin` facade over the scene core, which is what
keeps upstream-style workbench code working. `ferrocad_stage` is the modern
declarative path used by the current viewport.

### 5.5 Python and editions

| Artifact | What it exposes |
| --- | --- |
| `ferrocad_py` | The `ferrocad` extension module over `ferrocad_core`. |
| `ferrocad_part_py` | The `Part` module. |
| `ferrocad_mesh_py` | The `Mesh` and `MeshPart` modules. |
| `ferrocad_combined_py` | One extension exposing `ferrocad`, `Part`, `Mesh` and `MeshPart` over a shared core. |
| `ferrocad_parametric` | The edition binary: the base app plus the Part workbench. |
| `python/FreeCAD`, `python/FerroCAD` | The pure-Python `FreeCAD`/`FerroCAD` facades layered over the extension. |

Which of those artifacts an edition may link is enforced, not just described:
`cargo xtask editions --check` pins each edition's transitive normal dependency
closure, and `tools/run_tests.py` pins which Python tests are allowed to need the
kernel. See [`testing.md`](../developer/testing.md).

The Python API is the product surface; the window is one client of it. The same
`FreeCAD` calls work in the console, a macro, a headless workflow, or the UI. That is
the distinction [UI users and API users](../user/explanation/ui-and-api.md) explains.

### 5.6 The pipeline, end to end

| Stage from Chapters 1–2 | FreeCAD | FerroCAD |
| --- | --- | --- |
| NURBS / B-Rep | OCCT, `Part::TopoShape` | `ferrocad_occt` behind `ferrocad_geom::GeometryBackend`; `ferrocad_part` registers the feature |
| Tessellation | `Shape.tessellate` | The same Python call; the producer tessellates, the renderer stays kernel-free |
| Permanent mesh | Mesh workbench | `ferrocad_mesh` (`Mesh::Feature`), no OCCT |
| Scene graph | Coin3D / Pivy | `ferrocad_inventor` and `ferrocad_stage`; `ferrocad_inventor_py` is the `pivy.coin` facade |
| GPU | OpenGL | `ferrocad_wgpu` (wgpu over Vulkan/Metal/DX12/GL) |
| Offscreen image | — | `ferrocad_wgpu::snapshot` / `ferrocad_snapshot` |
| Naming | `Data::ElementMap` on `ComplexGeoData`, filled by `Part::TopoShape` | `ferrocad_lineage` vocabulary, filled by `ferrocad_occt` provenance and persisted by `ferrocad_part` |
| App / document | `App::Application` | `ferrocad_core::Application` / `kernel()` |

The renderer is kernel-free: a producer tessellates a shape in its own process, and the
window and the headless snapshot renderer consume the same draw data. That is why the
documentation screenshots are reproducible without a running app.

## Where to read next

- [`../developer/architecture.md`](../developer/architecture.md) — the full crate map
  and call paths.
- [`../developer/architecture/part-draft-layering.md`](../developer/architecture/part-draft-layering.md)
  — the analytic/B-Rep split and the Part/Draft layers.
- [`../developer/architecture/lineage-and-naming.md`](../developer/architecture/lineage-and-naming.md)
  — the lineage layer in detail.
- [`../project/research/tns-decoupling.md`](../project/research/tns-decoupling.md) — why
  kernel-portable topology is an open problem rather than a shipped feature.
- [`../developer/architecture/kernel-state-sharing.md`](../developer/architecture/kernel-state-sharing.md)
  — the one-core invariant behind the Python modules.
- [`../developer/viewport/stage.md`](../developer/viewport/stage.md) and
  [`../developer/viewport/inventor-renderer.md`](../developer/viewport/inventor-renderer.md)
  — the modern and Coin-compatible presentation stacks.
- [`../user/explanation/ui-and-api.md`](../user/explanation/ui-and-api.md) — the UI
  and API as two front doors.
