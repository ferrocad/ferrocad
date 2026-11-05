# Opening a large model: where the seconds are

Status: **task (2026-11-05). Stages 1 and 2 landed; stages 3–5 open.**
Companion: [`../../developer/architecture/host-backends.md`](../../developer/architecture/host-backends.md)
§10 (the mesh hop), and [`transform-session.md`](transform-session.md) for the
edit-time half of the same cost.

## 0. The complaint

Opening `~/Downloads/900a_GA3DtechProject_v1.0.FCStd` (41 MB, 42 members, 10 drawable
shapes, **1 251 921 triangles**, largest shape 327 954 triangles) took 46.5 s from
process start to a rendered frame. Four changes have landed since — `Shape.mesh` bakes
the object placement in the kernel, the mesh crosses to the host as packed `bytes`, the
`.FCStd` payloads are restored in parallel, and the display tessellation is memoised per
shape handle — and it is now **~13 s**, with a repeat sync at **0.7 s** instead of 5.5 s.
The rest of this document records what is left, why progressive tessellation alone does
not fix it, and the order the remaining fixes should land in.

## 1. The measured budget

Debug build, 12 cores, `ferrocad_parametric`, timing written from the script (stdout is
swallowed) and from env-gated `eprintln!` in the phases named below.

| Phase | Time | Where |
| --- | --- | --- |
| process start → interpreter ready | 0.8 s | `ferrocad_shell_python` boot |
| `FreeCAD.open(path)` | **1.7 – 2.0 s** (was 5.4 – 5.9 s) | `doc_format::read` → `parse_fcstd` |
| ├ zip index | 64 µs | `ZipArchive::parse` |
| ├ `Document.xml` → `SavedDocument` | **1.8 s** (was 5.88 s) | `parse_document`: 140 KB of XML around 40 MB of `PartShapeN.brp` members |
| ├ `GuiDocument.xml` | 11 ms | `parse_gui_document` |
| └ `from_saved` + `insert` + Python states | 7 ms | `openDocument` |
| `stage_meshes_typed()` (first call) | **8.5 – 9.3 s** | `_stage_mesh_rows` → `Shape.mesh(binary)` |
| ├ `BRepMesh_IncrementalMesh` (parallel) | 5.8 s cold / 2.8 s warm | `ferrocad_occt::topology::tessellate` |
| ├ sub-shape maps (faces/edges/vertices) | 0.4 s | same |
| ├ copy-out (nodes, triangles, edges) | 2.4 – 3.3 s | same |
| └ encode to `bytes` + PyO3 decode + `StageMesh` | ≲ 0.2 s | after the packed-buffer change |
| render (800×600 offscreen) + exit | ~3 s | `ferrocad_viewport` |

The payload restore was two halves, both per object: **3.34 s of ZIP inflation** (this
is a hand-written `inflate`, ~15 MB/s, not a fast library) and **2.04 s of BRep parse**
in OCCT. Stage 2 runs them on one thread per payload; the wall clock is now the largest
single member (~1.0 s) instead of the sum.

The mesh half is **OCCT**, not the shell: `BRepMesh` is release-quality C++ behind the
same numbers in any profile. The per-face floor in §2 is the thing to explain there.

Reference commands:

```sh
# end to end
time ./target/debug/ferrocad_parametric --open ~/Downloads/900a_GA3DtechProject_v1.0.FCStd \
     --render /tmp/o.png --size 800x600

# phase split (script stdout is swallowed → write a file)
./target/debug/ferrocad_parametric --script /tmp/timing.py --render /tmp/s.png --size 64x64

# the parallel restore, in situ: pinning to one CPU makes `available_parallelism` 1
taskset -c 0 ./target/debug/ferrocad_parametric --script /tmp/open-only.py --render /tmp/s.png --size 64x64
```

Both remaining large numbers are **OCCT**, not the shell: the ASCII BRep reader and
`BRepMesh`. Neither is affected by our Rust profile, so these are the numbers a release
build pays too (unlike the marshaling costs the earlier changes removed).

## 2. Progressive tessellation alone recovers a third

The intuition was "mesh coarse, draw, refine". Measured on fresh shapes (OCCT keeps an
existing triangulation when the next request is not finer, so each row needs a fresh
document):

| Deflection (mm) | Whole-document mesh | Triangles |
| --- | --- | --- |
| 0.5 (current default) | 8.7 s | 1 251 921 |
| 2 | 7.6 s | 999 873 |
| 8 | 7.8 s | 633 399 |
| 20 | 5.9 s | 349 343 |
| 50 | 5.5 s | 228 181 |

A 5.5× reduction in triangles buys 37 % of the time. The remainder is a **per-face
floor**: `IncrementalMesh` costs ~100–130 µs per face regardless of the deflection
(7 063 faces in the `45Deg` shape: 0.93 s at 50 mm, 1.59 s at 0.5 mm; the 720-face
`Plateau` shape: 82 ms at 50 mm). The same floor shows up warm, when OCCT only has to
*check* the stored triangulations (2.8 s), which is not credible as a tolerance
comparison and needs explaining (§4, stage 1).

So a progressive first frame is worth having, but it cannot be sold as "open becomes
instant". Two other levers are on the table:

- **Restore the shapes in parallel.** The ten `.brp` members are independent, and the
  5.88 s is a serial loop over them. `std::thread::scope` over the objects is enough;
  no new dependency.
- **Do not restore what is not drawn yet.** A `.FCStd` stores geometry as ASCII BRep,
  ~40 MB here, and the parse is pure OCCT. Deferring it changes what a document may
  contain between `open` and the first frame, so it is the largest change and last.

## 3. What the user should see

The acceptance is perceptual, so it is written as a frame budget rather than a total:

1. **≤ 1 s**: the window exists with the document's tree and property panels populated
   (they need `Document.xml` only, not the BRep members).
2. **≤ 3 s**: a coarse silhouette of every drawable object is on screen, camera fitted.
3. **Then**: the full-fidelity mesh replaces it without moving the camera, without a
   second fit, and without the tree or selection changing identity.

Picking during stage 2 is allowed to be coarse (it uses the same mesh), but a pick
must not fail: the triangle set a coarse mesh has is what the ray cast sees.

## 4. Stages

Each stage is independently shippable and measurable with the commands in §1.

### Stage 1 — explain the per-face floor (no user-visible change) — **landed 2026-11-05**

What the instrumented runs showed (`ferrocad_occt::topology::tessellate`, debug build):

- **The parallel mesher is doing its job.** Cold, `IncrementalMesh` takes 8.6 s with
  `theInParallel` and 23.9 s without it (whole document, same run). Warm it is 5.2 s vs
  7.0 s — barely parallel, because the second pass is per-face bookkeeping, not meshing.
- **The floor is per face, not per triangle.** 7063 faces cost 0.93 s at a 50 mm
  deflection and 1.59 s at 0.5 mm, while the triangle count moves 228 k → 1.25 M. A
  720-face shape pays the same ~100 µs per face. So `BRepMesh` spends the time on face
  setup (edge discretisation, pcurves), not on triangles.
- **A second call is not free.** OCCT's "incremental" reuse saves only ~40 %, which is
  why every display sync of an unchanged document cost 5.2 s.
- **A placed `Shape` is re-wrapped on every access.** `obj.Shape` builds a new
  `PartShape` (`shape_to_py` → `wrap_shape`), so a memo on the wrapper can never hit
  across two syncs; it has to hang off the `ShapeProperty` that owns the handle.

Decision: [x] memoise the tessellation (see the stage below) rather than chase a
correction to `BRepMesh`; [x] do not expect fidelity coarsening to carry the first
frame (it recovers a third, see §2).

### Stage 1b — memoise the display tessellation — **landed 2026-11-05**

- [x] `ferrocad_geom::TessellationCache`: one entry keyed by shape handle
  (`Shape::is_same`) and the deflection pair. Holding the handle in the entry keeps
  its identity alive, so a freed handle cannot alias a new one. Every kernel operation
  returns a new handle and none mutates one in place, which is what makes identity a
  sound invalidation signal.
- [x] `ShapeProperty` owns the cache and `shape_to_py` shares it with the wrapper it
  builds, so the memo survives the wrapper (and outlives one sync); a wrapper with no
  property behind it gets its own.
- [x] Verified invalidation on the three ways a shape changes: a repeat sync agrees
  byte for byte, moving an object shifts its geometry by exactly the offset, and
  assigning a new shape replaces the mesh (and resets the placement).
- Measured (debug, whole document):

| Sync | Before | After |
| --- | --- | --- |
| first (cold) | 7.6 s | 7.6 s |
| unchanged | 5.5 s | **0.67 s** |
| after moving the largest object | 5.5 s | **2.7 s** |

  The 0.67 s is the copy, the placement bake and the `bytes` encode; the 2.7 s is the
  one 328 k-triangle shape re-meshing. `open + render` also dropped ~1 s (12.4–13.1 s),
  because that path syncs twice.

### Stage 2 — restore the `.FCStd` members in parallel — **landed 2026-11-05**

- [x] Collect the payloads the walk will read from `Document.xml`, then inflate and
  restore them on `std::thread::scope` (one worker per member, capped at
  `available_parallelism`), and let the walk take the finished value out of the table.
  The collector mirrors `read_extension`'s dispatch, so a job always matches what the
  walk asks for; a test pins that (`payload_jobs_match_what_the_walk_reads`).
- [x] `Box<dyn ExtensionData>` was already `Send`, so no bound was needed. The real
  serialisation was elsewhere: `property_types::restore_extension` held the registry
  mutex *while* calling the restore hook, so every BRep parse queued on one lock. The
  lock is now released before the hook runs.
- [x] A `<Part>` with a `FerroCADSidecar` prefetches only the sidecar (the `file` member
  is its on-demand fallback), so a file that carries both does not decode twice.
- [x] `wasm32-unknown-unknown` has no threads, so the browser keeps the serial order
  (`cfg!(target_family = "wasm")` forces one worker).
- [x] Proved equivalence: the 900×650 render of the assembly is byte-identical to the
  pre-change PNG, two opens in one process produce the same `stage_meshes` digest, and
  the 100 core + 760 workbench + 691 base tests pass.
- Acceptance: `FreeCAD.open` **1.7 – 2.0 s** (target was ≤ 1.5 s — missed by the cost of
  the largest single member, which is now the floor), document identical.

### Stage 3 — a deflection the host can choose

- [ ] `FerroCAD.display_mesh(obj, deflection=None)` passes the request to the provider,
  and `ferrocad_shell.stage_meshes_typed(deflection=None)` forwards it. A provider
  that does not accept the argument keeps working (call it without the keyword).
- [ ] The Part hook forwards it to `Shape.mesh(deflection=...)`; the toy ignores it.
- [ ] The shell owns the quality policy: coarse on the first sync after an open
  (`diagonal / 60`, clamped), full `Deviation` afterwards.
- Acceptance: `stage_meshes_typed(deflection=20)` returns the 349 k-triangle document;
  a normal sync is unchanged.

### Stage 4 — two passes, one camera

- [ ] `DisplaySync` grows a *quality epoch*: a coarse sync marks the stage "refinable",
  and the next idle tick (not the next frame) requests the full-deflection sync and
  swaps the meshes in place, keeping `entity`, selection and camera.
- [ ] Refinement is cancellable: a new edit or a document switch drops the pending
  pass; a drag is never interrupted by it.
- [ ] The whole-document pass stays one call (the cost is per document, not per
  object), so the shell cannot be left with half-refined objects.
- Acceptance: the 900a file reaches a rendered frame in **≤ 3 s** and full fidelity
  without an input, the camera does not move between passes, and a mid-refinement
  camera orbit stays above 60 fps.

### Stage 5 — lazy geometry (only if stages 2–4 leave `open` above 1 s)

- [ ] Split `SavedDocument` into "properties known" and "shape payloads pending"; the
  tree and property rows come from the former.
- Acceptance: `open` costs the XML alone (≤ 0.3 s here), and the first sync restores
  what it needs.

## 5. Constraints that must not regress

- **Placement baking stays in the kernel** (`Shape.mesh(matrix=...)`); a display hook
  must not loop over vertices again.
- **The mesh crosses as packed arrays.** `stage_meshes()` keeps returning lists, so
  scripts and the docs tooling see the documented JSON shape
  ([`../../mods/display-and-interaction.md`](../../mods/display-and-interaction.md)).
- **One `stage_meshes` call per sync, whole document.** Per-object caching is stage 1's
  business; the seam stays document-shaped.
- **The shell names no workbench.** The deflection travels as data through
  `FerroCAD.display_mesh`, not as a Part-specific branch in `ferrocad_shell_python`.
- **A coarser mesh is a display decision only.** Nothing it feeds may be written back
  into the document, and `Shape.tessellate` (upstream's API) keeps its own precision.

## 6. Open questions

1. Does OCCT's stored triangulation survive `BRepTools::Read`? If it does, the warm
   2.8 s is a check and can be skipped; if it does not, the "incremental" claim in
   `Shape.mesh`'s docstring is wrong and every sync re-meshes.
2. Is the per-face floor the mesher or the BRep structure (many small faces from a
   stock assembly)? The `Plateau` shape (720 faces, 22 k triangles) pays the same
   per-face rate, which points at the mesher.
3. Should a coarse first pass also apply to `--render`/`--stl`? No for `--stl` (the
   export must be full fidelity); for `--render` it only changes the image, so it
   stays full fidelity by default.
4. A 40 MB archive at ~15 MB/s of inflation says the ZIP reader is the next structural
   cost once meshing is dealt with. Stage 2 hides it behind ten threads; a document
   with one huge member cannot. Swapping in a fast inflate (`miniz_oxide`) is the
   obvious follow-up and is a dependency decision, not a design one.
