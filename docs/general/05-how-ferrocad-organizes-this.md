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
| `ferrocad_part` | The Part workbench: registers `Part::Feature` and `Part::PropertyPartShape` on top of the backend. |
| `ferrocad_mesh` | The triangle-mesh core: STL/bms, analysis/editing and `Mesh::Feature`. **No OCCT.** |
| `ferrocad_core` | Documents, properties, links, expressions and the recompute DAG. No kernel and no UI. |

The `Part` path follows the vector workflow: OCCT holds NURBS and B-Rep, and
`Part::Feature` is the document object that carries a shape. The `Mesh` path follows
the bitmap workflow: `ferrocad_mesh` holds triangles and never links OCCT. Both paths
meet in the document core, not in the kernel.

### 5.2 Documents and recompute

`ferrocad_core` owns the document graph, properties, transactions and the recompute
DAG. `Part::Feature` and `Mesh::Feature` are object types registered in the one
process-wide core; the Python modules are views over it, not separate models.

This is also why the edition and the combined Python extension can share one core:
[`kernel-state-sharing.md`](../developer/architecture/kernel-state-sharing.md) records
the one-core invariant and the compositions that enforce it.

### 5.3 Naming: the externalized TNS

This is where FerroCAD differs most from upstream FreeCAD.

Upstream keeps `ElementMap` inside `Part::TopoShape`. FerroCAD separates the concerns:

- **`ferrocad_lineage`** owns the payload-generic vocabulary: element references,
  operation history and stable-name maps. It knows nothing about OCCT or Part.
- **The kernel reports provenance.** `ferrocad_occt` translates OCCT's operation
  history into lineage records.
- **Part stores and applies the policy.** `ferrocad_part` persists the lineage maps
  and resolves names at the document boundary.

That is the "pluggable TNS" idea from Chapter 4, realized as a layer rather than an
event-broker interface. The naming policy can evolve without changing the geometry
kernel, and a different kernel can supply provenance without adopting OCCT's
`ElementMap`.

See [`lineage-and-naming.md`](../developer/architecture/lineage-and-naming.md) for the
design and [`geometry-and-topology.md`](../project/research/geometry-and-topology.md)
for the research behind it.

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
| Naming | `Part::TopoShape`'s `ElementMap` | `ferrocad_lineage` plus the Part policy |
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
- [`../developer/architecture/kernel-state-sharing.md`](../developer/architecture/kernel-state-sharing.md)
  — the one-core invariant behind the Python modules.
- [`../developer/viewport/stage.md`](../developer/viewport/stage.md) and
  [`../developer/viewport/inventor-renderer.md`](../developer/viewport/inventor-renderer.md)
  — the modern and Coin-compatible presentation stacks.
- [`../user/explanation/ui-and-api.md`](../user/explanation/ui-and-api.md) — the UI
  and API as two front doors.
