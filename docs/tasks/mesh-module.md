# Task — the `Mesh` / `MeshPart` / `Mesh::Feature` contract

Status: implemented 2026-10-26 (M1); the measured results are at the end of §5.
P4 in [`../TODO-26-10-2026.md`](../TODO-26-10-2026.md)
(§P4). Companion to
[`../workbenches/simplecad-bringup.md`](../workbenches/simplecad-bringup.md) §7
(S1 baseline, both images) and §S3.4.

This started as a measured contract; it now records what the implementation does.

## 1. Why this exists

The 2026-10-26 S1 baseline ran all 88 SimpleCAD unit modules on the bare
`python/` extension: **17 loaded, 71 reported a load error**; 61 named `Part`, 10
named `Mesh` (`../workbenches/simplecad-bringup.md:713-714,724-725`). The ten are
the first `Mesh` consumers:

```
test_analytic  test_clipboard  test_idle_work  test_mesh_conversion  test_mesh_glue
test_mesh_slicing  test_mesh_stretch  test_mesh_surfaces  test_precision  test_surface_wrap
```

Five of the ten are in the sixteen-file `test_mesh_*.py` block counted in
`../workbenches/simplecad-bringup.md:415`: `test_mesh_conversion`,
`test_mesh_glue`, `test_mesh_slicing`, `test_mesh_stretch`, `test_mesh_surfaces`.
The other eleven files in that block load today (including `test_mesh_collisions`,
whose `Mesh` import is guarded), so the block is not a load-error set. The
remaining five of the ten are the non-prefixed modules listed above and are
outside the block.

Four more unit files import `MeshPart` **after** `Part` and are therefore hidden
inside the 61 `Part` errors today: `test_intersection.py:7-8`,
`test_joints.py:7-8`, `test_spheres.py:8-9`, `test_union.py:9-11`. A fifteenth
file, `test_mesh_collisions.py:108-111`, guards the whole import and skips its
`NativeAgreementTests` when `Mesh` is missing. That is the 15 of
`../workbenches/simplecad-bringup.md:677-678`: ten stop at `import Mesh`, four
stop earlier at `Part`, one is masked as a skip. All three counts are correct at
different points in the import order.

All ten also `import Part` and build their fixtures from `Part.makeBox` /
`makeCylinder` / `makePlane` (e.g. `test_mesh_glue.py:37`,
`test_mesh_slicing.py:26`, `test_mesh_stretch.py:48`). The Mesh track removes the
**Mesh/MeshPart** load error; the Part track is separate. "Running" below means
the module body executes and its tests are collected — not that every case
passes.

## 2. The measured surface

Paths are relative to the sibling checkout `../simplecad`. The `10` column means
"used by at least one of the ten modules"; `—` means only the wider tree uses it.

### 2.1 Constructors and factories

| Call | Returns | Semantics the caller relies on | 10 | Wider |
| --- | --- | --- | --- | --- |
| `Mesh.Mesh()` | empty mesh | `CountFacets == 0`; constructing is not an error (`test_mesh_conversion.py:142` feeds it to `convert`, which raises `ValueError` on the facet count). | ✅ | — |
| `Mesh.Mesh(other)` | copy | Copies points, facets and **placement**; upstream `MeshObject::operator=` copies the transform (`Mesh.cpp:176-187`). | — | — |
| `Mesh.Mesh((points, facets))` | mesh | `points` is a sequence of `Base.Vector` (or 3-float sequences); `facets` a sequence of 3-index sequences; identity placement; point/facet order preserved. | ✅ `test_mesh_conversion.py:59`, `test_mesh_glue.py:96`, `test_mesh_stretch.py:16` | ✅ |
| `Mesh.Mesh([tri, tri, …])` | mesh | Each `tri` is a 3-sequence of points (or 9 floats, or a `Facet`); a flat list of 3-float points (three per facet) is also accepted upstream. | ✅ `test_precision.py:26,101`, `test_mesh_slicing.py:34` | ✅ |
| `Mesh.Mesh(path)` | mesh | Reads by suffix. `.stl` auto-detects binary/ASCII; the result has identity placement. Upstream also reads `.bms` (`MeshPyImp.cpp:65-140,150-203`). | ✅ `.stl` (`test_mesh_glue.py:31`, `test_mesh_stretch.py:56`) | ✅ `.bms` in `scripts/mesh_worker.py:27-48` |
| `Mesh.createBox(l, w, h)` | mesh | **Centred at the origin** (`-l/2 … +l/2`), 8 shared points, 12 outward triangles. Defaults are 10/10/10; the optional upstream `edgelen` subdivision is not used. | ✅ `test_mesh_glue.py:90`, `test_mesh_slicing.py:48,74` | ✅ GUI tests |

Upstream's tuple form is stricter than its list form: `addFacets` only copies
vertices that are `Base.Vector` and silently skips plain float triples, leaving
facet indices into an empty array (`MeshPyImp.cpp:634-756`). SimpleCAD documents
the resulting C++ crash at `simplecad/mesh/stretch.py:77-79`. Our binding must
accept both and must not reproduce the silent drop.

### 2.2 Instance attributes and methods

| Member | Form | Returns | Semantics | 10 | Wider |
| --- | --- | --- | --- | --- | --- |
| `Topology` | property | `(list[Vector], list[(int,int,int)])` | The stored point array **with the mesh placement applied** and the stored facet indices (0-based). No re-welding, no reordering, so `fingerprint` over two reads is stable (`Mesh.cpp:313-335`; `simplecad/mesh/conversion.py:17-35`). | ✅ all | ✅ |
| `Points` | property | `list[MeshPoint]` (`x`, `y`, `z`, `Vector`) | Upstream returns the **local** points (`Mesh.cpp:1873-1879`). SimpleCAD uses `Topology[0]`, not `Points`. | — | — |
| `Facets` | property | `list[Facet]` | `Facet.Points` is 3 points and `Facet.Normal` a `Vector`; both are the **local** kernel values upstream (`FacetPyImp.cpp:91-93,169-182`). | ✅ `conversion.py:167-168`, `glue.py:29,334`, `test_mesh_glue.py:42,62` | ✅ |
| `CountPoints`, `CountFacets` | properties | `int` | Stored counts. | ✅ | ✅ |
| `BoundBox` | property | `App.BoundBox` | **Placement-applied** AABB of the transformed local AABB (`Mesh.cpp:150-163`); `XMin/XMax/YMin/YMax/ZMin/ZMax`, `Center`, `XLength/YLength/ZLength`, `DiagonalLength`. | ✅ `test_mesh_glue.py:19`, `test_clipboard.py:85,90`, `slicing.py:38,85-89,195` | ✅ |
| `Volume` | property | `float` | `fabs` of the tetrahedron sum over the **local** points; non-negative, orientation-independent, float32 upstream (`MeshKernel.cpp:1126-1148`). | ✅ `test_mesh_glue.py:46`, `test_mesh_slicing.py:32,45,52` | ✅ |
| `Area` | property | `float` | Sum of facet areas on the local points (`MeshKernel.cpp:1103-1111`). | ✅ `test_mesh_slicing.py:71,90`, `test_mesh_stretch.py:70` | ✅ |
| `Placement` | property get/set | `App.Placement` | Stored beside the points, **never baked**. Setting it is absolute: after `m.copy(); m.Placement = App.Placement()` the points are local again. `copy()` preserves it. | ✅ `test_mesh_slicing.py:57`, `test_mesh_glue.py:79`, `test_mesh_stretch.py:33,41` | ✅ `jobs/mesh.py:55,194,224` |
| `isSolid()` | method | `bool` | **No border edges** only (`MeshEvalSolid`, `Evaluation.cpp:293-305`). It does *not* imply manifold or consistent orientation. | ✅ `conversion.py:146,172`, `slicing.py:198`, `stretch.py:68` | ✅ |
| `hasNonManifolds()` | method | `bool` | An edge is shared by **more than two** facets (`MeshEvalTopology`, `Evaluation.cpp:333-389`). | ✅ `conversion.py:146,204`, `glue.py:330`, `stretch.py:66` | ✅ |
| `hasNonUniformOrientedFacets()` | method | `bool` | `countNonUniformOrientedFacets() > 0`; the count is the minority-orientation set per connected component (`MeshPyImp.cpp:1084-1090`, `Evaluation.cpp:135-157,189-...`). | ✅ `glue.py:318,330`, `stretch.py:66,84`, `slicing.py:191,245` | ✅ |
| `hasSelfIntersections()` | method | `bool` | Any pair of triangles intersects. | ✅ `test_mesh_glue.py:40` | ✅ `region_edit.py:61`, `deformation.py:101` |
| `getSelfIntersections()` | method | `list[(int, int, Vector, Vector)]` | Two 0-based facet indices and two **local** intersection points (`MeshPyImp.cpp:995-1030`). SimpleCAD uses the indices and the distance only (`region_edit.py:51-74`). | ✅ `glue.py:335` | ✅ |
| `copy()` | method | `Mesh` | Deep copy, placement included. | ✅ `conversion.py:200`, `clipboard.py:43`, `stretch.py:41`, `slicing.py:49` | ✅ |
| `write(path)` | method | `None` | Writes **local** coordinates (`MeshObject::save`, `Mesh.cpp:359-404`); `.stl` is binary. `.bms` is the same extension-selected format family. | ✅ `.stl` (`test_mesh_glue.py:31`, `test_mesh_stretch.py:55`) | ✅ `.bms` in `jobs/mesh.py:56` |
| `removeDuplicatedPoints()`, `removeDuplicatedFacets()` | methods | `None` | In-place cleanup (`MeshPyImp.cpp:1236-1256`). | ✅ `conversion.py:144-145,201-202` | ✅ |
| `fixDegenerations()` | method | `None` | In-place (`MeshPyImp.cpp:1223-1234`). | ✅ `conversion.py:203` | ✅ |
| `harmonizeNormals()` | method | `None` | In-place winding correction (`MeshPyImp.cpp:1116-1127`). | ✅ `conversion.py:148,206`, `glue.py:323` | ✅ |
| `getSeparateComponents()` | method | `list[Mesh]` | One mesh per connected component; placement is copied (`MeshObject::meshFromSegment`, `Mesh.cpp:1789-1803`). | ✅ `conversion.py:209` | ✅ |
| `getPlanarSegments(dev, minFacets=0)` | method | `list[list[int]]` | Groups of adjacent facets whose points lie within `dev` of one plane (`MeshPyImp.cpp:1818-1844`). The argument is a distance, not an angle. | ✅ `planar.py:105`, `curved.py:133` | ✅ |
| `countComponents()` | method | `int` | Connected components (`MeshPyImp.cpp:1129-1136`). | ✅ `test_mesh_glue.py:83`, `test_mesh_slicing.py:67` | ✅ |
| `countSegments()`, `getSegment(i)` | methods | `int`, `list[int]` | Facet groups created by `MeshPart.meshFromShape(..., Segments=True)`. | — | ✅ `deformation.py:39-42` |

### 2.3 `MeshPart`

| Call | Returns | Semantics | 10 | Wider |
| --- | --- | --- | --- | --- |
| `MeshPart.meshFromShape(Shape=…, LinearDeflection=…, AngularDeflection=0.5, Relative=False, Segments=False)` | `Mesh` | OCC incremental mesh of the shape; vertices welded within `gp::Resolution()` (`~1e-7`); identity mesh placement. `AngularDeflection` is radians. `Segments=True` creates one segment per **face** (`AppMeshPartPy.cpp:476-560`, `Mesher.cpp:153,186-300`). | — | ✅ `deformation.py:36`, `test_intersection.py:58`, `test_joints.py:167`, `test_spheres.py:79`, `test_union.py:135`, `test_mesh_collisions.py:128` |
| `MeshPart.meshFromShape(Shape=…)` | `Mesh` | The no-deflection branch (Mefisto/default upstream) (`AppMeshPartPy.cpp:605-620`). | — | — (no SimpleCAD call site) |
| `MeshPart.wireFromSegment(mesh, group)` | `list[Part.Wire]` | `group` is a Python **list** of 0-based facet indices. Computes the boundary of those facets on the **local kernel** and returns closed `Part.Wire`s in reverse boundary order (`AppMeshPartPy.cpp:414-446`). | ✅ via `planar.py:169`, `curved.py:137` | ✅ |

The ten modules call `wireFromSegment` transitively through
`simplecad/mesh/planar.py` and `simplecad/mesh/curved.py` (e.g.
`test_analytic.py:22`, `test_precision.py:87`, `test_mesh_conversion.py:18`).
They never call `meshFromShape`; that is the wider tree, listed as M1b in §4.

### 2.4 `Mesh::Feature`

| Form | Returns / effect | Semantics | 10 | Wider |
| --- | --- | --- | --- | --- |
| `doc.addObject('Mesh::Feature', name)` | document object | Defaults: `Mesh` (empty) and `Placement` (identity). Type id is exactly `'Mesh::Feature'`. | ✅ `test_clipboard.py:81`, `test_idle_work.py:188`, `test_mesh_stretch.py:31`, `test_surface_wrap.py:66` | ✅ `jobs/mesh.py:221,254` |
| `.Mesh` (read) | `Mesh.Mesh` | A snapshot in the object's frame: `Topology`/`BoundBox` reflect `obj.Placement`; `Points`/`Facet.Points` stay local. | ✅ `test_clipboard.py:85,88,90`, `test_mesh_stretch.py:34` | ✅ `clipboard.py:43`, `jobs/mesh.py:37,214` |
| `.Mesh = mesh` | assignment | Stores the mesh's local points and adopts its placement into `.Placement` (upstream `onChanged`, `MeshFeature.cpp:77-87`). | ✅ `test_clipboard.py:82`, `test_idle_work.py:189,200`, `test_mesh_stretch.py:32` | ✅ `clipboard.py:69` |
| `.Placement` | `App.Placement` get/set | The authoritative object transform; `.Mesh` reflects it immediately, without a recompute. | ✅ `test_clipboard.py:83,89`, `test_mesh_stretch.py:33,38-40` | ✅ `jobs/mesh.py:194,224` |
| `.Visibility` | `bool` | View property, forwarded by `ferrocad_py` (`crates/ferrocad_py/src/lib.rs:5725-5736`). No Mesh-specific work. | ✅ (`has_faces` reads `ViewObject.Visibility`, `skin.py:15`) | ✅ `jobs/mesh.py:228`, `gui/…` |
| `.TypeId` | `str` | `'Mesh::Feature'`. | ✅ `test_clipboard.py:87` | ✅ `clipboard.py:19,38,68` |
| `.isDerivedFrom('Mesh::Feature')` | `bool` | True for `Mesh::Feature`; also true for `App::GeoFeature` (upstream base at `MeshFeature.h:54`). | — | ✅ `command_state.py:116`, `viewport_style.py:961`, `conversion.py:14` |
| `.ViewObject` | view object | Exists headlessly; `has_faces` reads `.Visibility` (`simplecad/surfaces/skin.py:11-20`). | ✅ `test_surface_wrap.py:66-67` | ✅ |

### 2.5 Which module needs what

| Module | Direct Mesh use | `MeshPart` (transitive) | `Mesh::Feature` |
| --- | --- | --- | --- |
| `test_analytic` | `Mesh.Mesh(tessellate)` `:21`; `Topology`, `CountFacets`, validators through `planar.reconstruct` | `planar.py:169`, `curved.py:137` | — |
| `test_clipboard` | tuple ctor `:82`; `CountFacets`, `BoundBox.XMin/ZMax` `:88-90`; `copy` (`clipboard.py:20,43`) | — | `addObject` `:81`, `.Mesh` `:82`, `.Placement` `:83`, `.TypeId` `:87` |
| `test_idle_work` | `Mesh.Mesh(tessellate)` `:189,200`; `Topology` via `fingerprint` | — | `addObject` `:188`, `.Mesh` `:189`, `.Placement` `:208` |
| `test_mesh_conversion` | ctors `:11,59,142`; `Topology`; `Facets.Points`; validators/mutators/components through `convert` | `conversion.py:253` → `planar.py`/`curved.py` | — |
| `test_mesh_glue` | `createBox` `:90`; tuple ctor `:96,107`; path ctor/write `:31`; `Topology`; `Facets.Points/Normal` `:24,42,55,62`; `Volume` `:46`; `BoundBox.Center` `:19`; `countComponents` `:83`; `getSelfIntersections` `:40` | — | — |
| `test_mesh_slicing` | `createBox` `:48,74`; tuple ctor `:244`; list-of-`Facet.Points` ctor `:34`; `Placement` `:57`; `Volume`/`Area`/`BoundBox`/`countComponents` | — | — |
| `test_mesh_stretch` | tuple ctor `:16`; path ctor `:56`; write `:55`; `Topology`; `Placement`/`copy` `:41`; validators | — | `addObject` `:31`, `.Mesh` `:32`, `.Placement` `:33` |
| `test_mesh_surfaces` | ctor `:15`; `convert` `:104,118` | `conversion.py:253` | — |
| `test_precision` | list ctor `:26,101`; ctor `:87`; `Topology` `:95` | `planar.py:169` | — |
| `test_surface_wrap` | ctor `:67` | — | `addObject` `:66`, `.Mesh` `:67` |

## 3. What upstream FreeCAD does

The pinned `../freecad-upstream` is a sparse, `blob:none` checkout at `33adf61`
with only `src/Ext` and `src/Mod/{BIM,Draft,Part,Test}` checked out. **It does not
contain `src/Mod/Mesh` or `src/Mod/MeshPart`**, and the blobs cannot be fetched
offline. The Mesh/MeshPart sources below were read from the local FreeCAD 0.19
tree at `/home/john/projects/freecad/source` (`b9bd1d8`, `0.19.16100`). Paths are
relative to that tree; line numbers apply to it. Where 0.19 lacks an attribute
that modern FreeCAD/SimpleCAD has, the stable mechanism is what we rely on and
the gap is labelled.

### 3.1 The four named ambiguities

| Question | Upstream answer | Evidence |
| --- | --- | --- |
| Return type of `Shape.tessellate` | `(list[Base.Vector], list[(int,int,int)])`. Vertices are merged within `gp::Resolution()` (`~1e-7`), degenerate facets are dropped, and the point list is shared across faces. | `TopoShapePyImp.cpp:1723-1759`; `TopoShape.cpp:3285-3409` |
| Point type in `Topology` | `list[Base.Vector]`; facets are tuples of Python ints. `Topology` returns the **stored** points (placement applied), not a re-welded set. | `MeshPyImp.cpp:2026-2054`; `Mesh.cpp:313-335` |
| How `isSolid` / `hasNonManifolds` are defined | `isSolid` = no border edge. `hasNonManifolds` = at least one edge shared by >2 facets. Neither checks the other. | `Evaluation.cpp:293-305,333-389`; `Mesh.cpp:253-257,1340-1344` |
| What `wireFromSegment(mesh, group)` returns | A Python list of `Part.Wire`, one per boundary loop, built on the **local kernel** with `BRepBuilderAPI_MakePolygon` in reverse boundary order. `group` must be a `list` of facet indices. | `AppMeshPartPy.cpp:414-446` |

### 3.2 Placement is a live transform, not a baked one

`MeshObject` stores a `Base::Matrix4D _Mtrx` beside the kernel. The accessors split
into two frames:

| Accessor | Frame |
| --- | --- |
| `Topology`, `BoundBox` | placement applied (`getPoint` / `getBoundBox`, `Mesh.cpp:150-163,313-335`) |
| `Points`, `Facet.Points`, `Facet.Normal`, `getSelfIntersections` points | local (`Mesh.cpp:1873-1951`, `FacetPyImp.cpp:91-93,169-182`, `MeshPyImp.cpp:995-1030`) |
| `Volume`, `Area`, `isSolid`, `hasNonManifolds`, orientation | local kernel (rigid-invariant) |
| `write`, `.bms`/STL output | local kernel (`Mesh.cpp:344-404`) |
| `copy`, `getSeparateComponents` | preserve `_Mtrx` (`Mesh.cpp:176-187,1789-1803`) |
| `getPlanarSegments`, `wireFromSegment` | local indices/points |

`Mesh::Feature` keeps the object's `Placement` and the mesh's `_Mtrx` in sync:
`onChanged(&Placement)` writes the matrix into the mesh, and `onChanged(&Mesh)`
adopts the mesh transform into `Placement` (`MeshFeature.cpp:77-87`). That is why
`obj.Mesh.Topology` follows `obj.Placement` **without a recompute**.

0.19's `MeshPy` exposes `Points`, `Facets`, `CountPoints`, `CountFacets`,
`Topology`, `Area` and `Volume` (`MeshPy.xml:502-554`), but **not**
`Mesh.Placement` or `Mesh.BoundBox`. Both are required by SimpleCAD
(`test_mesh_slicing.py:57-59`, `test_mesh_stretch.py:34-40`,
`test_mesh_glue.py:19`). Their semantics above are therefore inferred from the
`_Mtrx` mechanism and from `MeshObject::getBoundBox`; they are not read directly
from a 1.0 `MeshPyImp.cpp` in this checkout.

The decisive SimpleCAD test for the frame is
`test_mesh_slicing.test_main_axis_follows_rotated_elongated_mesh`
(`tests/unit/test_mesh_slicing.py:54-59`): it rotates a `Mesh.Mesh` and expects
`mesh.Placement.Rotation.multVec(local axis)` and the eigenvector of
`mesh.Topology` to be the same world direction. Local `Topology` fails that; the
placement-applied `Topology` above passes it.

### 3.3 Constructors, STL, `createBox`

- `Mesh.Mesh` dispatch: no argument, a `Mesh` copy, a list/tuple through
  `addFacets`, or a path through `load` (`MeshPyImp.cpp:65-140`).
- `addFacets`: list = triangles or a flat point list; tuple = `(points, facets)`;
  only `Base.Vector` points survive the tuple form (`MeshPyImp.cpp:634-756`).
- `Mesh.createBox` calls `BuildRegularGeoms.Cube`, which is centred on the
  origin with 8 shared points and 12 triangles (`AppMeshPy.cpp:460-497`,
  `BuildRegularGeoms.py:136-173`). Defaults are 10/10/10.
- `write(path)` selects the format by suffix; `.stl` is binary (`BSTL`); the
  writer emits the local kernel (`MeshPyImp.cpp:204-314`, `Mesh.cpp:359-404`).
- `.bms` is FreeCAD's binary mesh format (`MeshIO::BMS`), not a SimpleCAD
  invention. It preserves duplicate point identities that STL cannot
  (`glue.py:306-308`), which is why the job layer uses it.
- `.FCStd` stores a mesh as `<Mesh file="…">` with a `MeshKernel.bms` member
  (`MeshProperties.cpp:576-592`). The native FerroCAD writer errors when a module
  value has no `.FCStd` payload (`crates/ferrocad_core/src/document/fcstd_write.rs:993-1003`).

### 3.4 `MeshPart.meshFromShape`

The standard branch is
`meshFromShape(Shape, LinearDeflection, AngularDeflection=0.5, Relative=False,
Segments=False, GroupColors=[])` and calls `BRepMesh_IncrementalMesh` with those
values (`AppMeshPartPy.cpp:94-130,476-560`; `Mesher.cpp:186-193`). Vertices are
welded through a `std::set` with `Mesher::Vertex::deflection = gp::Resolution()`
(`Mesher.cpp:153,220-278`). `Segments=True` adds one facet-index group per face
domain (`Mesher.cpp:279-300`). The result is a new `Mesh.Mesh` with identity
placement. `Shape`-only, `MaxLength`, `MaxArea`, `LocalLength`, `Deflection`,
`MinLength/MaxLength` and the netgen signatures exist too; SimpleCAD uses only the
standard branch, always with `Relative=False`.

### 3.5 Where a naive guess is wrong

1. **`Mesh.createBox` is not corner-at-origin.** It is centred: `±l/2` on every
   axis (`BuildRegularGeoms.py:136-173`). `test_mesh_slicing.py:48-52` cuts
   `Mesh.createBox(20,20,20)` at `(0,0,0)` and expects two solid halves; a
   corner-at-origin box would put the plane on its boundary.
2. **`isSolid` does not mean "closed, manifold and consistently oriented".** It
   means "no border edges" (`Evaluation.cpp:293-305`). SimpleCAD checks
   `hasNonManifolds()` and `hasNonUniformOrientedFacets()` separately for exactly
   this reason (`stretch.py:66-68`, `glue.py:318-330`). An empty mesh is "solid"
   by this definition (no edges); no consumer depends on it.
3. **`Volume` is absolute and float32.** Upstream sums tetrahedra in `float` and
   returns `fabs` (`MeshKernel.cpp:1126-1148`), so it is non-negative even for an
   inverted mesh. SimpleCAD comments on the precision in
   `test_mesh_glue.py:18` and `simplecad/mesh/slicing.py:248-249`.
4. **`Topology` is placed, but `write` is local.** Neither the STL writer nor the
   `.bms` writer applies `_Mtrx` (`Mesh.cpp:359-404`). The job layer resets the
   placement before writing for that reason (`jobs/mesh.py:54-56`).
5. **`Facets[i].Points` and `Facet.Normal` are local, not placed**, while
   `Topology` is placed (`FacetPyImp.cpp:91-93,169-182` vs `Mesh.cpp:313-335`).
   No SimpleCAD test distinguishes the frames, but an implementation that makes
   all point queries placed would differ from upstream.
6. **`wireFromSegment` returns `Part.Wire`, not a mesh, and uses the local
   kernel.** A placed mesh's wires are unplaced. `planar.reconstruct` rebuilds
   exact faces from them (`planar.py:159-176`).
7. **`Shape.tessellate` welds and drops degenerate facets.** A naive
   "per-face triangles with duplicated vertices" result is not a closed mesh and
   `Mesh.Mesh(tessellate(...)).isSolid()` is false. The weld radius is
   `gp::Resolution()`, not exact-bit equality (`TopoShape.cpp:3285-3321`).
8. **`MeshPart.meshFromShape(Segments=True)` is one segment per face**, not one
   per connected region (`Mesher.cpp:279-300`).
9. **`obj.Mesh` follows `obj.Placement` without a recompute.** The sync is
   `Mesh::Feature::onChanged` (`MeshFeature.cpp:77-87`), not `execute`.
10. **Upstream's tuple constructor silently ignores non-Vector points**
    (`MeshPyImp.cpp:634-756`); do not copy that bug.

## 4. The implementation split

### 4.1 `ferrocad_mesh` — Rust core, no OCCT, no Python

Owns the payload and the document type. Dependencies stay at `ferrocad_core`
(registries) and `ferrocad_types` (`Vector3`, `Placement`).

| Module | Content |
| --- | --- |
| `mesh` | `Mesh { points: Vec<Vector3>, facets: Vec<[u32; 3]>, placement: Placement }`; constructors (`new`, `from_facet_points`, `box_/create_box`); validation; `counts`, local `bounds`, placed `bounds`, `volume` (absolute), `area`; `is_solid` (no border edges), `has_non_manifolds`, `has_non_uniform_oriented_facets`, `self_intersections`, `components`, `planar_segments`; mutators (`remove_duplicated_*`, `fix_degenerations`, `harmonize_normals`); `copy` preserving placement. |
| `stl` | Binary write from **local** points; binary/ASCII read; exact f32 weld on read (a written triangle soup becomes one closed mesh again). No `.bms`. |
| `mesh_property` | `Mesh::PropertyMeshKernel` as a `Property::Extension` (`ferrocad_core::property_types`), with a versioned save/restore of points, facets and placement; `Mesh::Feature` as an `ObjectType` with default `Mesh` + `Placement`; `register()`. |
| `feature` sync | Mirror `Placement` into the mesh property and back, using equality guards. The object `Placement` is authoritative; the mesh property carries the mirror so the Python converter can return a placed `Mesh.Mesh` without knowing the owner. Neither side is baked. |

`Mesh::PropertyMeshKernel` is upstream's name (`MeshProperties.cpp:49`); the
native envelope stores the placement beside the points, and `Mesh::Feature`'s
`on_property_changed` hook (added to `ferrocad_core::ObjectType`) mirrors
`Placement` into the mesh and adopts a new mesh's transform into `Placement`.
Undo/redo re-runs the same hook.

### 4.2 `ferrocad_mesh_py` — the `Mesh` and `MeshPart` modules

Two built-in modules in one crate (`#[pymodule] fn Mesh`, `#[pymodule] fn
MeshPart`), registered with `append_to_inittab!`.

- `Mesh.Mesh`: the constructor forms in §2.1; the attributes and methods in §2.2.
  `Mesh.createBox` is a module function, not a class method.
- `Facet`: `Points` (3 points; Vectors are safe, upstream 0.19 uses float tuples)
  and `Normal` (`Vector`).
- `Mesh::PropertyMeshKernel` needs **no** Python class: `addObject('Mesh::Feature')` is
  generic core behaviour. `ferrocad_mesh_py` registers
  `ferrocad_py::register_extension_converter(ferrocad_mesh::TYPE_NAME, …)` so
  `obj.Mesh` converts to/from `Mesh.Mesh` (the same seam as `Part::PropertyPartShape`).
- `MeshPart.wireFromSegment`: compute boundary loops in `ferrocad_mesh`, then
  build the wires through `Part.makePolygon`
  (`crates/ferrocad_part_py/src/curves.rs:7815-7818`) or a small `pub` helper in
  `ferrocad_part_py`. Using `makePolygon` also makes `PyWire::isClosed` answer
  correctly; a wire wrapped from a kernel shape alone returns `False`
  (`curves.rs:7373-7385`).
- `MeshPart.meshFromShape` (M1b): reuse the shape's welded tessellation. Either
  expose the `PartShape` handle (currently `pub(crate) fn shape`,
  `ferrocad_part_py/src/lib.rs:133`) or call the `Shape.tessellate`/`mesh()`
  binding; do not duplicate the merge. `Segments=True` maps the tessellation's
  per-face parts to facet groups.
- Dependencies: `ferrocad_mesh`, `ferrocad_py` (Vector/Placement/BoundBox).
  `Part` is used only through Python (`Shape.tessellate`, `Part.makePolygon`), so
  `ferrocad_mesh_py` has **no** library dependency on `ferrocad_part_py` (it is a
dev-dependency for the booted tests). `ferrocad_mesh` itself stays free of both.

### 4.3 One image

A standalone `Mesh.abi3.so` links its own `ferrocad_core`, so it would not see
the `FreeCAD`/`Part` documents and registries (the same reason
`part_conformance.rs:3-8` boots an in-process image). The harness must append
`ferrocad_py::ferrocad`, `ferrocad_part_py::Part`, and
`ferrocad_mesh_py::{Mesh, MeshPart}` to the inittab in one process, exactly as
`crates/ferrocad_part_py/tests/part_conformance.rs:25-31` does, and call
`ferrocad_mesh::register()` before any document is created or restored.

The working-tree `crates/ferrocad_part_py/tests/simplecad_conformance.rs` boots
`ferrocad`, `Part`, `Mesh` and `MeshPart` in one inittab, as built. It resolves
the checkout from `SIMPLECAD_DIR`/`FERROCAD_SIMPLECAD`/`../simplecad` and
self-skips without one; `FERROCAD_SIMPLECAD_FILES` narrows a run.

### 4.4 Cross-crate prerequisites

| Change | Why |
| --- | --- |
| `ObjectType` post-write hook `on_property_changed` called from `Document::set_property` **and** `restore_property_value` (`crates/ferrocad_core/src/object_registry.rs`, `document/mod.rs`, `document/transaction.rs`) | **Landed.** `Mesh::Feature` mirrors `Placement` ↔ the mesh property; undo/redo re-runs it. |
| `type_parent` entries for `Mesh::Feature` (and `Mesh::FeaturePython`) in `ferrocad_py` (`crates/ferrocad_py/src/lib.rs:154-180`) | Upstream base is `App::GeoFeature` (`MeshFeature.h:54`); `isDerivedFrom('Mesh::Feature')` must also answer `App::GeoFeature`. |
| `PyBoundBox::DiagonalLength` (`crates/ferrocad_py/src/lib.rs:6311-6366`) | Required by `slicing.py:89,195`; the class has `XLength/YLength/ZLength` and `Center` but no `DiagonalLength`. |
| `Shape.tessellate` that welds at `gp::Resolution()` and drops degenerate facets | **Landed** in `ferrocad_part_py`; `Mesh.Mesh(tessellate).isSolid()` holds for a box (8 points / 12 facets). |
| `Part.Wire.OrderedVertexes` (`crates/ferrocad_part_py/src/curves.rs`, next to `PyWire::Vertexes`) | `planar.py:160` reads it directly; `curved.py:501` falls back to unordered `Vertexes`. `wireFromSegment`'s wires must answer it. |
| `Mesh::PropertyMesh::fcstd_members` returning `None` | Core's `.FCStd` writer then **errors** on any document with a `Mesh::Feature` (`crates/ferrocad_core/src/document/fcstd_write.rs:993-1003`). That is acceptable for the unit slice (they never save) but must be stated; `.FCStd` support is coupled to `.bms` (`MeshKernel.bms`). |

### 4.5 Deferred

- `.bms` read/write and the job/worker path (`jobs/mesh.py:56,213`,
  `scripts/mesh_worker.py:27-48`).
- `.FCStd` mesh members (`<Mesh file="…">` + `MeshKernel.bms`); the native
  `.ferrocad` extension property is enough for the ten.
- `MeshPart.meshFromShape` is not in M1; it is M1b (it unlocks `test_intersection`,
  `test_joints`, `test_spheres`, `test_union`, `test_mesh_collisions` once Part
  is present).
- `Mesh.createSphere/createCylinder/createCone/createTorus/createPlane`.
- `decimate` (only reached when `convert(..., simplify>0)`; the ten pass 0).
- Boolean/self-intersection **repair** (`removeSelfIntersections`,
  `fixSelfIntersections`, `removeFoldsOnSurface`), mesh booleans and smoothing.
- Other mesh features (`Mesh::Box`, `Mesh::Sphere`, `Mesh::FeaturePython`),
  `Mesh.show`, `Mesh.read`, `Mesh.transformShape`, `Points` mutation,
  `getSegmentsOfType`, `getSegmentsByCurvature`, OBJ/PLY/AMF/3MF and the netgen
  `meshFromShape` signatures.
- A live `obj.Mesh` view (write-through mutation). No SimpleCAD caller mutates
  `obj.Mesh` in place.

## 5. Acceptance

Each row is one contract item and the test that pins it. `M1` is the ten-module
slice; `M1b` is the wider `MeshPart.meshFromShape` slice.

| # | Contract item | Test | Slice |
| ---: | --- | --- | --- |
| 1 | `Mesh.Mesh()` empty; `(points, facets)`; list of triangles; `Mesh.Mesh(mesh)` copy | Rust unit tests in `ferrocad_mesh`; Python constructors test | M1 |
| 2 | `createBox` is origin-centred, 8 points / 12 facets, `isSolid` | Rust `ferrocad_mesh` test asserting `±l/2`, volume `l·w·h`; SimpleCAD `test_mesh_slicing.py:48-52` | M1 |
| 3 | `Topology` returns Vectors + int triples, **placed**, stored order | Python test comparing `Topology` before/after `Placement`; SimpleCAD `test_mesh_slicing.py:54-59` | M1 |
| 4 | `Points` / `Facet.Points` / `Facet.Normal` are local | Python test on a rotated mesh (they must **not** move with `Placement`) | M1 |
| 5 | `BoundBox` placed, `DiagonalLength` present | Python test; `test_mesh_slicing.py:89`, `test_mesh_glue.py:19` | M1 |
| 6 | `Volume` absolute (inverted mesh still positive), `Area` finite | Rust tests: box, inverted box, open plane | M1 |
| 7 | `Placement` stored, never baked; `copy()` preserves; reset recovers local | Rust + Python test; `test_mesh_stretch.py:41-43` | M1 |
| 8 | `write` writes local coordinates; STL read round-trips closed | Rust round-trip (binary + ASCII); `test_mesh_glue.py:31`, `test_mesh_stretch.py:55-57` | M1 |
| 9 | `isSolid` = no border edges; closed-but-non-manifold still `True` | Rust test with open plane, box, 3-facet-edge mesh | M1 |
| 10 | `hasNonManifolds` = edge with >2 facets; not non-manifold vertices | Rust test | M1 |
| 11 | `hasNonUniformOrientedFacets` = minority-orientation set > 0 | Rust test: box with one facet flipped | M1 |
| 12 | `getSelfIntersections` records `(f1, f2, Vector, Vector)`; `hasSelfIntersections` | Rust test with two crossing triangles; `test_mesh_glue.py:40` | M1 |
| 13 | `removeDuplicatedPoints/Facets`, `fixDegenerations`, `harmonizeNormals` mutate in place | Rust tests; `conversion.py:144-148,201-206` | M1 |
| 14 | `getSeparateComponents` splits and preserves placement | Rust test on two disjoint boxes; `conversion.py:209` | M1 |
| 15 | `getPlanarSegments(dev)` groups adjacent coplanar facets | Rust test: box → 6 groups; `planar.py:105`, `curved.py:133` | M1 |
| 16 | `countComponents` | Rust test; `test_mesh_glue.py:83` | M1 |
| 17 | `doc.addObject('Mesh::Feature')`, `TypeId`, `PropertiesList`, `.Visibility`, `.isDerivedFrom` | Rust object test + Python object test; `test_clipboard.py:81-87` | M1 |
| 18 | `.Mesh` read is placed; `.Placement` write is visible immediately; `.Mesh = m` adopts `m.Placement`; undo/redo round-trips | Rust test (post-write hook) + Python test; `test_idle_work.py:191-210`, `test_mesh_stretch.py:28-43` | M1 |
| 19 | Native `.ferrocad` save/load of a `Mesh::Feature` keeps mesh + placement; `.FCStd` reports `UnsupportedExtension` | Rust test; `fcstd_write.rs:993-1003` | M1 |
| 20 | `Shape.tessellate` returns welded Vectors + int triples; no degenerate facets | Part Rust/Python test: box → 8 points/12 facets; `Mesh.Mesh(tessellate).isSolid()` | prerequisite |
| 21 | `wireFromSegment(mesh, group)` → closed `Part.Wire`s with `OrderedVertexes` | Python test on a box top-face group: 1 wire, 4 ordered vertices, `isClosed()`; a face with a hole → 2 wires | M1 |
| 22 | `meshFromShape(Shape, LinearDeflection, AngularDeflection, Relative, Segments)` | Python test: box → 8/12, `isSolid`; `Segments=True` → 6 segments of 2 facets | M1b |
| 23 | The ten modules lose `ModuleNotFoundError: Mesh` / `MeshPart` | Measured run below | M1 |
| 24 | `test_mesh_collisions.NativeAgreementTests` un-skips | Same run | M1b |

### The measured run

Measured 2026-10-26 with the image described above:

- **88/88 modules load, 0 load errors.**
- **362 passed, 36 failed, 673 errored, 2 skipped.**
- `Mesh`/`MeshPart` load errors: **0**. The last load error was an unrelated
  `QtCore.Qt.UserRole` lookup in the PySide stub, now covered.

The remaining failures are per-test Part API (flat-face/`Surface` typing,
`sewShape`, `childShapes` direct children, `discretize`, `getObjectsByLabel`,
`Material.Transparency`, and the like, listed in `../TODO-26-10-2026.md` §P4),
not module boundaries. The earlier identity/query gaps (`Shape.copy()`,
`__hash__`/`__eq__`, `DiagonalLength`, `Placement(other)`, `OrderedVertexes`,
`isClosed`, `distToShape`, `valueAt`) are now implemented.

```sh
FERROCAD_SIMPLECAD=$PWD/../simplecad cargo test -p ferrocad_part_py \
  --test simplecad_conformance -- --nocapture
```

A single module is isolated with `FERROCAD_SIMPLECAD_FILES=test_mesh_slicing`
(the variable the harness reads). `git -C ../simplecad status --porcelain` must
stay empty afterwards.

### Checks that must stay green

```sh
cargo test -p ferrocad_mesh --offline
cargo test -p ferrocad_mesh_py --offline
cargo test -p ferrocad_py --offline
cargo test -p ferrocad_part_py --test part_conformance --offline -- --nocapture
```

`part_conformance` needs `FERROCAD_UPSTREAM` and stays a measurement (no
assertion on the pass count). `ferrocad_mesh` must build without OCCT.

## 6. Verification limits

- The pinned `../freecad-upstream` has no `src/Mod/Mesh` / `src/Mod/MeshPart`;
  all upstream Mesh claims are from the 0.19 tree at
  `/home/john/projects/freecad/source` (`b9bd1d8`). `Mesh.Placement` and
  `Mesh.BoundBox` do not exist in 0.19; their semantics are inferred from
  `_Mtrx` and `Mesh::Feature::onChanged`. The exact `Facet.Points` type and
  whether modern FreeCAD fixed its local-vs-placed split are unverified.
- `test_mesh_stretch.test_rotated_plane_parameters_follow_placed_mesh` is
  algebraically self-cancelling (`P · P⁻¹ · x = x`) and therefore does **not**
  prove the frame. `test_mesh_slicing.test_main_axis_follows_rotated_elongated_mesh`
  is the decisive test; the inference above is from that test plus the `_Mtrx`
  source.
- `wireFromSegment`'s reverse boundary order and the exact `.bms` encoding were
  read from 0.19; no SimpleCAD test depends on the order.
- The per-module test counts and pass counts are not measured here. The task
  deliberately records only load-error direction.
