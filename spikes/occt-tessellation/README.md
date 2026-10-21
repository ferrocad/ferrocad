# Spike: OCCT tessellation and topology identity

Status: spike (2026-10-20). Answers F0.0b in
[`docs/interaction/topology-selection.md`](../../docs/interaction/topology-selection.md).

The question: can `opencascade-sys` 0.3 give us a display mesh with **per-face and
per-edge identity**, the adjacency a tangent chain needs, a chamfer, and fillet
lineage — without forking the binding crate?

## Run

```sh
export OCCT_INCLUDE_DIR=<prefix>/include/opencascade
export CMAKE_POLICY_VERSION_MINIMUM=3.5   # CMake >= 4 vs the crate's 3.1 minimum
cargo run --release
```

(`CMAKE_PREFIX_PATH=<prefix>` and `LD_LIBRARY_PATH=<prefix>/lib` as well if the
prefix is not on CMake's/ld's default paths.)

## What it does

1. Builds a 10×10×10 box.
2. Meshes it with `BRepMesh_IncrementalMesh(shape, 0.5)`.
3. Walks the faces with `TopExp_Explorer`, reads each face's `Poly_Triangulation`
   through `BRep_Tool::Triangulation`, applies the triangulation location, and
   reports node/triangle counts, normals and face orientation.
4. Samples every edge with `BRepAdaptor_Curve` + `GCPnts_TangentialDeflection`.
5. Builds `TopExp::MapShapesAndAncestors` maps both ways and reads them through the
   sibling bridge's `fc_list_size` / `fc_list_value`.
6. Chamfers the first edge (equal distance through the crate, two distances through
   the bridge) and fillets it, reading `Modified`/`Generated`/`IsDeleted`.

## Results (OCCT 7.8.1, 10×10×10 box, deflection 0.5)

- 1 solid, 6 faces, 12 edges, 8 vertices.
- 2 triangles per planar face (12 total); `Poly_Triangulation` indices are 1-based;
  `HasNormals` is false; 3 faces are `TopAbs_REVERSED`; locations are identity.
- `TopExp_Explorer` reports **24** edges (shared edges visited per parent);
  `TopExp::MapShapes` reports the **12** unique edges.
- `MapShapesAndAncestors(S, TS, TA, M)` is ancestor-only: `(EDGE, FACE)` gives each
  edge's two faces; face → edges needs an explorer over the face.
- Every edge samples to ≥ 2 points with `BRepAdaptor_Curve` +
  `GCPnts_TangentialDeflection` — no polygon bridge needed.
- Equal and two-distance chamfers both build (6 faces → 7 after one edge).
- Fillet builds (7 faces); for the seeded edge `Modified=0, Generated=1,
  IsDeleted=true` — the fillet face is generated.

Conclusion: only three C++ calls are missing from `opencascade-sys` (list access, the
two-distance chamfer, fillet/chamfer history). The full findings are recorded in
[`docs/interaction/topology-selection.md`](../../docs/interaction/topology-selection.md) §5.
