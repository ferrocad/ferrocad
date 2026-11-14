# FerroCAD roadmap

Status: roadmap (2026-11-15). The actionable backlog is
[`TODO-15-11-2026.md`](TODO-15-11-2026.md); the measured history is
[`record/milestones.md`](record/milestones.md) and the latest rounds are
[`record/2026-11-14-p2-boolean-seam.md`](record/2026-11-14-p2-boolean-seam.md) and
[`record/2026-11-13-november-round.md`](record/2026-11-13-november-round.md); the letters
used below (`H1`, `P4`, `S4`, `M3` …) are indexed in [`labels.md`](labels.md).

## Where we are

FerroCAD is a Rust/GPUI + PyO3/RustPython + pure-Python reimplementation of FreeCAD. The
engine, the app shell and the Python `FreeCAD` surface exist; `Part` is a real workbench
(**531 upstream tests passing, 0 errored, 12 skipped**, re-measured 2026-11-15); the
native and `.FCStd` serializers are independent; and topological naming is a lineage layer
rather than a kernel concern. There is **one document implementation and four runtime
configurations** (CPython, RustPython, JS, none), and the browser runs the same shell as the
desktop.

The bottleneck is no longer geometry. It is the **breadth of the workbench API**, the
**interaction surface** on top of it, and the **host/platform seam** the November round
built. The Part audit covers **253 of 408 upstream method names (62.0%)**. The SimpleCAD
S4.1–S4.6 façade is in the tree. SimpleCAD's integrated baseline is **1006 passed / 24
failed / 17 errored / 7 skipped over 88/88 modules** (2026-11-15, clean `9180c4b`), with
the Qt groups classified as skips and the SC7 workflow acceptance green. **P1 is closed**
(2026-11-13), and the P2 boolean/seam rounds then closed **24 of the 49 failures** without
losing one: `Shape.generalFuse` is one build per call, a replaced vertex reports its image,
`Edge.Orientation` and an edge's `Placement` are cached, and `Shape.fuse`/`cut`/`common` honour their signed
tolerance. Every remaining failure is still attributed (**18 P2, 3 P3, 3 documented test
assumptions**; none unattributed). The rotation's two red readings were fixed the same day:
the B20 `RootNode` change was a real Part regression, and the SimpleCAD `surface_ops`
delta was the in-process harness not exporting the paths its worker subprocess needs.

The platform picture did change materially: the **browser shell** runs the same
`RustAppBackend` as the desktop, the OCCT seam is **Rust**, and mods are a shippable pack
format. The remaining work is breadth and correctness on top of that platform, not a second
architecture.

The **interaction surface** advanced on 2026-11-14: Push/Pull and Fillet are numeric
editors that **re-open the feature they produced**. A cap or a fillet round (its face or a
boundary edge) seeds the handle at the value the feature already has, a kernel bisection
bounds it, and the commit edits that feature in one transaction — while a refused operation
rolls back and re-arms instead of leaving a hole in the model or a stuck handle. The
remaining interaction work is the tail (chamfer re-open, a multi-edge range, previews) and
the creation pill.

## The three horizons

| Horizon | Goal | Exit criteria | Backlog |
| --- | --- | --- | --- |
| **H1 — Finish Part/OCCT** (closed) | Complete the Part contract honestly | Zero Part failures (achieved 2026-10-27, restored 2026-11-13); every reachable guarded map size enabled; only documented limits remain | P2 (the OCCT tail) |
| **H2 — Workbench platform** (built) | Make Part a platform Draft/BIM/SimpleCAD can build on | `Part::FeaturePython` executes and persists; Draft M2–M5 run headless; Q0–Q5 done; one document/backend with four runtimes | P2 (Draft), P5 |
| **H3 — Proof by use** (active) | Prove the platform end to end | SimpleCAD S0–S5 baseline; a published `Parametric` edition; a headless BIM subset | P1 (closed), P3, P6 |

### H1 — Finish Part/OCCT (closed 2026-10-27)

- **The element-map version.** Enabled for every shape that carries a lineage. The
  compound (52), fuse (58), cut/common (26), general-fuse (60), sweep (6), shell/solid
  (26), loft (24), filled face (9) and compound-cut (20/47) sizes match upstream. The
  2026-11-13 suite is **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures**.
- **The seam remainder.** Landed: partial spheres/revolutions, variable-radius fillets,
  `makeOffset2D` filling, hyperbola/parabola/Bezier (including `Geom2d`), multi-shell
  solids, `DirMode Edge/Normal`, B-spline tube spines, the exact `makeHelix`/
  `makeLongHelix` edge, `MakePipeShell` threads and multi-section sweeps, upstream's
  two-profile thread solid, and every `curve_of` kind (elliptical arc, hyperbola,
  parabola, Bezier, rational and periodic B-spline, offset).
- **The two documented non-results.** `FaceMakerCheese` (10) needs upstream's face-combo
  alias and `makeShellFromWires` (24) needs a different (ruled-shell) topology. Both keep
  `ElementMapVersion == ""` so upstream's guards skip. **Two advertised limits still
  raise**: `makeCone` with a partial revolution and STEP/IGES `Import`/`insert`.

**Exit met.** The Part suite is green, and the only tail is the optional alias policy and
the two documented limits.

### H2 — Workbench platform (built 2026-11-12)

- **One document, four runtimes.** `RustAppBackend` is the document half of the desktop and
the browser; `PythonScriptHost`, `PywasmScriptHost`, `JsScriptHost` and `runtime=plain` are
the script halves. The browser shell (M0–M6) runs the real shell in Chrome, and the
backend-parity ledger (B0–B21) closed the legacy `PythonAppBackend` and made every shown
command a real `FreeCADGui` command.
- **The OCCT seam is Rust.** `ferrocad_part_cxx` is gone; `ShapeFix`, `ChFi2d`, topology,
boolean history and the curve/geom2d helpers are Rust over `ferrocad_occt_cxx`'s sys
bindings. The kernel contract and its failure classification are now one code path.
- **Python feature proxies (P2a–P2e, landed).** `Part::FeaturePython` and
`Part::Part2DObjectPython` execute a Python `Proxy`, fire `onChanged`, honour
`attach=True`, persist natively and call `onDocumentRestored`; the attachment extension
supplies `AttachmentSupport`/`MapMode`.
- **Qt Q0–Q5 (landed).** The native `FreeCADGui.Snapper`, the async point session, the
native `Draft_Line`/`Draft_Wire` tools and `PlaneGui`, the checkable `Draft_Snap_*` bar and
the remaining forms. `--draft` is **50/0/0/1** at the 2026-11-13 re-run.
- **The platform tail.** E1 mod events (first slice built), document-scoped settings, the
gizmo-interface decision, and the backend-parity deferrals (§P5).

**Done when** Draft creates, edits and imports headless. The Qt-free path is largely there;
Draft's modification/import breadth is not, and the platform now makes that a workbench task
rather than an architecture task.

### H3 — Proof by use (active)

- **SimpleCAD S0–S3 — the image is in.** `ferrocad_mesh` (payload, STL, operations,
  `Mesh::Feature`), `ferrocad_mesh_py` (`Mesh`, `MeshPart`), `Part.Shape.tessellate`,
  `BOPTools.SplitAPI` and the import-time PySide surface all load in one in-process
  `ferrocad` + `Part` + `Mesh` image. The 2026-11-15 baseline is **1006 pass, 24 fail,
  17 error, 7 skip over 88/88 modules**; the SC7 workflow acceptance (draw → extrude →
  push/pull → cut → STL) is green and the 5 Qt groups are classified skips. The identity, query and representation
  frontier closed (deep `Shape.copy()`, `__hash__`/`__eq__`, `Placement(other)`, typed
  `Faces`/`Wires`, `Face.Surface` classification with `parameter()`).
- **SimpleCAD S4.1–S4.6 — the façade is closed.** The view facade, the ranked pick and
  the `SelectionObject` are implemented and covered by tests; S4.4 now publishes the
  viewport's active workplane and resolves an exact face frame through `FreeCADGui`, and
  S4.5's push/pull stem is verified end to end (selection → stem drag → one transaction).
  S4.6 keeps `FreeCADGui.Preview` as the shape-preview channel and records 3-D text as a
  non-goal; the imperative-Coin lifecycle (`getChild`/`insertChild`/`findChild`,
  `isOfType`) is complete enough for an overlay.
- **P1 closed (SC8), and the P2 seam rounds are in.** At SC8 the 49 failures were filed
  into five named buckets — boolean/topology partition (16), document undo/transaction/
  labels (5), Part query strictness/tolerance (10), shape identity/ownership/BRep export
  (6) and surface/freeform (11) — plus one documented test assumption. The seam rounds
  then closed 25 failures (the eight surface-line, both rotate-edges, both elongating and the
  analytic boss cases among them), leaving **18 → P2, 3 → P3, 3 documented test assumptions,
  0 unattributed**. The chamfered single-face error is filed as a face-order assumption; the
  next cluster is `test_mesh_glue` and `test_union`'s rod. The full table is in
  [`research/simplecad-bringup.md`](research/simplecad-bringup.md) §7.
- **The browser is a real host.** M0–M6 run the shell in Chrome over the wasm OCCT
  kernel; the `?runtime=plain|pywasm|js` switch selects the script half, and a fetched mod
  pack reaches the script half. This is the proof-of-use surface that did not exist at the
  last roadmap.
- **Publish the edition.** The `Parametric` artifact (OCCT closure, launcher, release
  matrix) and the Diátaxis book as a release artifact. v0.1.4 is still the latest tag;
  macOS packaging and signing are parked.
- **BIM headless subset.** The `Arch`/BIM modules that do not need a GUI. This has not
  started; it follows the Draft M4/M5 import/export work.

**Done when** an external user can install an edition, run a scripted model and a
workbench, and compare results with upstream.

## Not on the roadmap (for now)

PartDesign's feature tree, a from-scratch Sketcher solver (the P4 plan binds PlanGCS
rather than writing one), FEM, CAM, the GUI workbenches, and full upstream `.FCStd`
byte-for-byte compatibility. These are deliberately out of scope; the project measures a
subset and says so.

## How progress is measured

- **Part:** `FERROCAD_UPSTREAM=../freecad-upstream cargo test -p ferrocad_part_py --test part_conformance -- --nocapture`,
  with the upstream revision recorded next to every published number. Current: **531/0/0/12**.
- **Part API breadth:** `python3 tools/part_api_audit.py --check`; the pinned contract is
  `contracts/part-api.json`. Record whether the figure came from the committed tree or
  the worktree, and from which extension build.
- **Draft:** `python3 tools/conformance.py --root ../freecad-upstream --draft`, or the
  in-process `cargo test -p ferrocad_part_py --test draft_conformance`.
- **SimpleCAD:** `python3 tools/conformance.py --simplecad` plus
  `tools/simplecad_audit.py --check`, and the in-process
  `crates/ferrocad_part_py/tests/simplecad_conformance.rs`. The in-process test exports the
  repo Python/OCCT paths to SimpleCAD's worker subprocess; use it when the extension may
  be stale, because it builds the current tree. Current: **1006/24/17/7 over 88/88**
  (2026-11-15, clean checkout).
- **Engine/app:** per-crate `cargo test` and `pytest tests` (no workspace-wide build).
- **Docs:** `api_docs`, `userdocs`, `build_book` and the relative-link sweep in `--check`
  mode.

## Risks and standing constraints

| Risk | Mitigation |
| --- | --- |
| `/home` fills during a workspace-wide build | cap builds (`CARGO_BUILD_JOBS=2`), avoid concurrent workspace builds; 2026-11-15: 93 % used, 47 GB free |
| The upstream pin is informal | record `git rev-parse --short HEAD` with every report (currently `33adf61`) |
| A stale extension or test expectation reads as a regression | prefer the in-process conformance tests, which build the current tree; when a Python number is cited, say which `.so` produced it |
| A green claim outlives the change that broke it | re-run Part/SimpleCAD at every rotation; the 2026-11-13 rotation caught one real regression (B20 `RootNode`) and one harness gap (worker paths) |
| A proxy layer leaks GUI assumptions | keep the headless conformance image as the gate |
| Approximations drift into claims | each says “approximation” at the call site and in the record |
| Naming policy grows into the kernel | the lineage layer stays payload-generic; the kernel only reports provenance |
| Label collisions hide ownership | use the area-prefix rule in [`labels.md`](labels.md) for new work, and qualify old labels in prose |
