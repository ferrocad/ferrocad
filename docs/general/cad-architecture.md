# A gentle introduction to 3D CAD architecture

*From mathematical curves to GPU silicon and parametric history.*

When you first open a 3D CAD program like FreeCAD, the 3D world on your screen looks
unified and seamless. You can rotate a cylinder, click on its edges, and modify its
size. Under the hood, the software is running a dual-identity system: it constantly
translates pure, abstract mathematics into a visual language the hardware understands.

To use 3D design well — or to architect and script CAD software — it helps to
understand the difference between what a 3D model *is* in memory, how its history is
tracked, and how it is drawn.

Chapters 1–4 are written against **FreeCAD**, because its vocabulary (OCCT, B-Rep,
Coin3D, `Part::TopoShape`) is the one most CAD users and workbench authors already
share. Chapter 5 maps each layer to **FerroCAD**'s crates.

## Chapter 1: The DNA of 3D geometry (vectors vs. bitmaps in 3D)

The easiest way to understand 3D geometry is to compare it with 2D graphic design. In
the 2D world we divide images into **vector graphics** (Illustrator, SVG) and
**raster/bitmap graphics** (Photoshop, JPEG). 3D software has the same divide.

| 2D design concept | 3D CAD equivalent | What it actually does |
| --- | --- | --- |
| **Bézier curves (math)** | **NURBS** | Stores infinite, smooth mathematical equations for curves and surfaces. |
| **Compound paths / layer masks** | **B-Rep (boundary representation)** | Stitches mathematical surfaces together into a structured, watertight solid part. |
| **Flat JPEG / pixel grid** | **Polygon mesh (STL / OBJ)** | Discards the math, baking the shape into a rigid network of flat triangles. |
| **Live on-screen canvas preview** | **Viewport data (Coin3D)** | Temporarily converts the math into triangles so the monitor can display it. |

### 1.1 NURBS: the 3D vector curves

To build smooth, organic or exact shapes, CAD software uses **NURBS (non-uniform
rational basis splines)**.

- **How it works.** Instead of plotting thousands of individual points, NURBS uses a
  small set of weighted control points and polynomial equations to guide a curve or
  surface — much like pulling on a sheet of digital spandex.
- **Infinite resolution.** Because it is stored as an equation, you can scale a NURBS
  surface up by 1,000,000% and the curve remains smooth.
- **In code.** Inside FreeCAD's geometric kernel (**Open CASCADE / OCCT**), NURBS are
  handled at the C++ level by classes such as `Geom_BSplineCurve` and
  `Geom_BSplineSurface`, and exposed to users through the **Part** and **Curves**
  workbenches.

### 1.2 B-Rep: the structural ledger

NURBS provides the math for an individual sheet or curve, but it does not know what a
"solid object" is. **B-Rep (boundary representation)** is the organizing system that
stitches surfaces together into a solid volume. It combines two layers:

1. **Geometry (the muscle).** The exact mathematical equations of the surfaces
   (NURBS, planes, cylinders).
2. **Topology (the skeleton).** A structural ledger tracking which **faces** meet at
   which **edges**, and which edges terminate at which **vertices**.

Because B-Rep is exact, formats such as **STEP (.stp)** are the usual exchange format
for CNC machining and mechanical engineering.

### 1.3 Polygon meshes: the 3D bitmap

A **mesh** abandons mathematical equations entirely. It approximates a 3D shape with a
dense web of flat polygons, usually triangles.

- **The faceted reality.** In a mesh, a cylinder is not a curve — it is dozens of flat
  rectangular strips arranged in a ring. Zoom in closely and you see the edges.
- **Zero intelligence.** A mesh has no concept of "features". A cylindrical hole in an
  **STL** or **OBJ** file is not a cylinder with a radius; it is an empty space
  surrounded by hundreds of anonymous triangles.

## Chapter 2: The great translation (workbenches, viewports and GPUs)

Here is the fundamental limitation of modern computer hardware: **graphics cards cannot
read or draw B-Rep or NURBS equations.**

Low-level graphics APIs such as **OpenGL** and **Vulkan** are built to do one thing at
blistering speed: process millions of flat triangles in parallel. To display a
mathematical CAD model, the software must translate it on the fly. That translation is
called **tessellation**.

### 2.1 The data pipeline

```text
       [ MATHEMATICAL DATA ]                              [ DISCRETE DATA ]
     smooth / infinite / CPU-bound                     faceted / rigid / baked

  +-----------------------------+
  |       1. NURBS DATA         |
  |  control points & weights   |
  |  polynomial equations       |
  +--------------|--------------+
                 | forms the surfaces of
                 v
  +-----------------------------+
  |        2. B-REP DATA        |
  |  geometry: NURBS / planes   |
  |  topology: faces, edges,    |
  |  vertex connectivity        |
  +--------------|--------------+
                 |
           TESSELLATED TO
                 v
  +-----------------------------+                +-------------------------------+
  |  3. TEMPORARY VISUAL MESH   |                |      4. PERMANENT MESH        |
  |  discardable triangles      |                |  fixed triangle faces         |
  |  regenerates whenever       |                |  static vertex array          |
  |  B-Rep dimensions change    |                |  imported from STL / OBJ      |
  +--------------|--------------+                +---------------|---------------+
                 |                                               |
                 +-----------------------+-----------------------+
                                         |
                                  both feed into
                                         v
                      +-------------------------------------+
                      |       5. VIEWPORT SCENE GRAPH       |
                      |           (Coin3D / Pivy)           |
                      |  ingests meshes only                |
                      |  SoIndexedFaceSet (mesh arrays)     |
                      +------------------|------------------+
                                         |
                                         v
                      +-------------------------------------+
                      |       6. GPU HARDWARE PIPELINE      |
                      |         (OpenGL / Vulkan)           |
                      |  vertex shader: 3D -> screen        |
                      |  fragment shader: lighting / color  |
                      +-------------------------------------+
```

### 2.2 How FreeCAD workbenches differ

- **Part / Part Design (vector workflow).** Stores true B-Rep math through OCCT. Every
  time you modify a dimension, FreeCAD silently tessellates the B-Rep into a
  **temporary visual mesh** and hands it to **Coin3D**, the Open Inventor viewport
  engine, which Python reaches through **Pivy**.
- **Mesh (bitmap workflow).** Holds a **permanent mesh**. Because the data is already
  made of triangles, FreeCAD skips the mathematical kernel and passes the triangles
  straight to Coin3D.

### 2.3 Down to the silicon: shaders

Once Coin3D organizes the triangles into scene-graph nodes such as
`SoIndexedFaceSet`, it packs the coordinates into memory arrays (**vertex buffer
objects**) and sends them to OpenGL/Vulkan:

1. **The vertex shader** runs on every triangle corner and calculates where it sits in
   2D screen space for the current camera.
2. **The fragment shader** colours the pixels between those corners. To keep the model
   from looking like a blocky disco ball, the viewport calculates **vertex normals**
   perpendicular to the original NURBS surface. The fragment shader blends the lighting
   across the flat triangles, creating the optical illusion of a smooth, glossy
   surface.

## Chapter 3: Time travel and the topological naming system (TNS)

Because B-Rep CAD is **parametric**, a model is not just a static shape — it is a
chronological recipe: sketch a square, extrude it into a box, fillet `Edge4`.

### 3.1 The topological naming problem (TNP)

Every time you change an earlier step, the CAD kernel (OCCT) destroys the old 3D shape
and calculates a brand-new one from scratch.

If you go back to the first step and turn the square into a hexagon, the new solid has
18 edges instead of 12. What used to be `Edge4` might now be `Edge9`. If the fillet step
only remembered "fillet `Edge4`", the fillet jumps to the wrong edge or the model
breaks.

### 3.2 Where the topological naming system lives

The **topological naming system (TNS)** is the genealogical ledger that addresses this.
It assigns persistent, historical names to geometry based on how it was born — "the
edge created by intersecting Face1 and Face2" — rather than volatile index numbers.

- **Invisible to the GPU and the viewport.** TNS lives on the CPU. Meshes, Coin3D and
  OpenGL know nothing about topological history.
- **Global utility vs. namespace reality.** Architecturally, every workbench needs
  TNS: **Part Design** for features, **TechDraw** so 2D dimensions do not detach when
  the 3D model updates, and **Assembly** to keep joints mated. In FreeCAD's C++ source,
  however, the TNS ledger (`ElementMap`) is physically housed inside the **`Part`**
  namespace (`Part::TopoShape`), because `Part::TopoShape` is the foundational wrapper
  around OCCT's `TopoDS_Shape` and its operation history (`BRepTools_History`).

## Chapter 4: Advanced CAD architecture — decoupling the TNS

There is no mathematically perfect solution to the topological naming problem; every
algorithm is a heuristic mitigation. Coupling the naming ledger directly inside
`Part::TopoShape` therefore limits architectural flexibility.

For CAD architects who want to externalize the TNS into a **pluggable,
engine-agnostic subsystem**, the in-object ledger can be refactored into an
event-driven broker pattern:

```text
+--------------------------------------------------------+
|               1. THE GEOMETRIC DRIVERS                 |
|  Part::TopoShape (OCCT)  |  Mesh (triangle data)  | ... |
+---------------------------+-----------------------------+
                            | emits raw mutation events
                            | (input ids, op type, kernel history)
                            v
+--------------------------------------------------------+
|          2. THE ABSTRACT TNP BROKER (App core)         |
|       ITopologicalNamingEngine (a proposed interface)  |
+---------------------------+-----------------------------+
                            | routes to the active strategy
                            v
+--------------------------------------------------------+
|           3. PLUGGABLE TNS IMPLEMENTATIONS             |
| +-------------------+ +-------------------+ +--------+ |
| | Genealogy hasher  | | Spatial heuristic | | Hybrid | |
| | (upstream         | | matcher           | | solver | |
| |  ElementMap idea) | |                   | |        | |
| +-------------------+ +-------------------+ +--------+ |
+--------------------------------------------------------+
```

This is a design pattern, not the current FreeCAD API. Its three pillars are:

1. **A pure abstract interface.** Strip string hashing and genealogy resolution out of
   `Part`. Define an abstract contract in the core application layer
   (`TrackOperation()`, `ResolveElement()`, `GetStableUUID()`).
2. **Dumb geometric wrappers and event broadcasting.** `Part::TopoShape` remains a
   wrapper for OCCT geometry. When a Boolean or extrude operation occurs, `Part`
   translates OCCT's `BRepTools_History` (generated, modified and deleted shapes) into
   a generic mutation struct and emits it to the broker.
3. **Externalized graph storage.** Instead of mutating fields inside `TopoShape`, the
   active TNS implementation maintains an external directed acyclic graph (DAG) of
   element lineage. That lets developers benchmark competing TNP strategies side by
   side, preserve backward compatibility with legacy files, or swap CAD kernels.

## Chapter 5: How FerroCAD organizes this

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
