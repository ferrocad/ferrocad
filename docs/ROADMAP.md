# FerroCAD roadmap

Status: roadmap (2026-10-27). The actionable backlog is
[`TODO-27-10-2026.md`](TODO-27-10-2026.md); the measured history is
[`record/milestones.md`](record/milestones.md); the letters used below (`H1`, `P4`,
`S4`, `M3` …) are indexed in [`labels.md`](labels.md).

## Where we are

FerroCAD is a Rust/GPUI + PyO3 + pure-Python reimplementation of FreeCAD. The engine,
the app shell and the Python `FreeCAD` surface exist; `Part` is a real workbench
(**531 upstream tests passing, 0 errors, 12 skipped**); the native and `.FCStd`
serializers are independent; and topological naming is a lineage layer rather than a
kernel concern.

The bottleneck is no longer geometry. It is the **breadth of the workbench API** and the
**last interaction surface** on top of it. The Part audit covers **220 of 408 upstream
method names (53.9%)** in the committed tree; the uncommitted fifth round raises that to
**248/408 (60.8%)** and adds the SimpleCAD S4.1–S4.3 view facade, ranked pick and
selection object. SimpleCAD's integrated baseline is **702 passed / 44 failed / 368
errored / 2 skipped over 88/88 modules**; that number predates the fifth round and is now
a lower bound.

## The three horizons

| Horizon | Goal | Exit criteria | Backlog |
| --- | --- | --- | --- |
| **H1 — Finish Part/OCCT** (closed) | Complete the Part contract honestly | Zero Part failures (achieved); every reachable guarded map size enabled; no “not implemented” left in the seam for advertised API | P1 (optional tail only) |
| **H2 — Workbench platform** (in progress) | Make Part a platform Draft/BIM/SimpleCAD can build on | `Part::FeaturePython` executes and persists; Draft M2–M5 run headless; Q0–Q5 done | P2 |
| **H3 — Proof by use** (active) | Prove the platform end to end | SimpleCAD S0–S5 baseline; a published `Parametric` edition; a headless BIM subset | P4, P5 |

### H1 — Finish Part/OCCT (closed 2026-10-27)

- **The element-map version.** Enabled for every shape that carries a lineage. The
  compound (52), fuse (58), cut/common (26), general-fuse (60), sweep (6), shell/solid
  (26), loft (24), filled face (9) and compound-cut (20/47) sizes match upstream, and the
  Part suite reports **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures**.
- **The seam remainder.** Landed: partial spheres/revolutions, variable-radius fillets,
  `makeOffset2D` filling, hyperbola/parabola/Bezier (including `Geom2d`), multi-shell
  solids, `DirMode Edge/Normal`, B-spline tube spines, the exact `makeHelix`/
  `makeLongHelix` edge, `MakePipeShell` threads and multi-section sweeps, upstream's
  two-profile thread solid, and every `curve_of` kind (elliptical arc, hyperbola,
  parabola, Bezier, rational and periodic B-spline, offset).
- **The two documented non-results.** `FaceMakerCheese` (10) needs upstream's face-combo
  alias and `makeShellFromWires` (24) needs a different (ruled-shell) topology. Both keep
  `ElementMapVersion == ""` so upstream's guards skip; no advertised API raises “not
  implemented”. The only remaining H1 item is optional.

**Exit met.** The Part suite is green, and the only tail is the optional alias policy.

### H2 — Workbench platform (in progress)

- **Python feature proxies (P2a–P2e, landed).** `Part::FeaturePython` and
  `Part::Part2DObjectPython` execute a Python `Proxy`, fire `onChanged`, honour
  `attach=True`, persist natively and call `onDocumentRestored`; the attachment
  extension supplies `AttachmentSupport`/`MapMode`. The Draft M2 modules run green:
  `test_creation.py` **26 passed** and `test_array.py` **1 passed**, with **zero
  failures, errors or load errors**.
- **Qt Q0–Q5 (landed).** [`workbenches/draft-tools.md`](workbenches/draft-tools.md):
  `FreeCADGui.Snapper`, the async point session, the native `Draft_Line`/`Draft_Wire`
  tools and `PlaneGui`, the checkable `Draft_Snap_*` bar, and the four remaining forms.
  The Python suite is **588 passed / 3 skipped** at the last recorded baseline (225
  subtests); the fifth round adds 47 passing tests.
- **Draft M3 first slice and M4 import round (landed).** `getEditorMode`/
  `setEditorMode`, `Placement.isIdentity`/`move`, `Matrix` copy/multiply,
  `PropertyVectorDistance` and strict `addObject` took `test_modification` from
  **17/0/16** to **22/1/10**; the fifth round's M4 import work (the derived
  `Part::Feature.Shape` wrapper, `Part::Prism`, `Shape.fix`/`OrderedEdges`,
  `Quantity.__eq__`, `getHomePath` and the TechDraw shim) takes it to **31/0/2**. The
  `--draft` group is **44/0/0/0** across ten files. The two remaining errors are
  `Sketcher` and TechDraw's `DrawPage`.
- **Draft M4 remainder and M5.** Import/export (the SVG/wire wrapper, `Import.readDXF`,
  the `test_draftgeomutils` extension math) and the BIM surface on top — measured by
  the `drafttests` modules, not by feature claims. This is the main H2 gap.

**Done when** Draft creates, edits and imports headless, and no upstream workbench path
needs Qt for its non-GUI behaviour. The Qt-free path is largely there; the Draft
modification/import breadth is not.

### H3 — Proof by use (active)

- **SimpleCAD S0–S3 — the image is in.** The Mesh track landed 2026-10-26:
  `ferrocad_mesh` (payload, STL, operations, `Mesh::Feature`), `ferrocad_mesh_py`
  (`Mesh`, `MeshPart`), `Part.Shape.tessellate`, `BOPTools.SplitAPI` and the import-time
  PySide surface. All **88/88** SimpleCAD unit modules load in one in-process
  `ferrocad` + `Part` + `Mesh` image: **702 pass, 44 fail, 368 error, 2 skip** (down from 635 errors before the integration fixes). The
  identity, query and representation frontier closed (deep `Shape.copy()`,
  `__hash__`/`__eq__`, `Placement(other)`, typed `Faces`/`Wires`, `Face.Surface`
  classification with `parameter()`), and the S2–S3 baseline is recorded in
  [`workbenches/simplecad-bringup.md`](workbenches/simplecad-bringup.md) §7.
- **SimpleCAD S4.1–S4.3 (in the worktree).** The view facade, the ranked pick and the
  `SelectionObject` are implemented and covered by 47 tests. S4.4 snapping, the S4.5
  push/pull integration and S4.6 previews remain. **Re-measure after the rebuild** — the
  702/44/368 figure predates the Part API batch.
- **Publish the edition.** The `Parametric` artifact (OCCT closure, launcher, release
  matrix) and the Diátaxis book as a release artifact. v0.1.4 is tagged; macOS packaging
  and signing are still unbuilt.
- **BIM headless subset.** The `Arch`/BIM modules that do not need a GUI. This has not
  started; it follows the Draft M4/M5 import/export work.

**Done when** an external user can install an edition, run a scripted model and a
workbench, and compare results with upstream.

## Not on the roadmap (for now)

PartDesign's feature tree, Sketcher's solver and constraints, FEM, CAM, the GUI
workbenches, and full upstream `.FCStd` byte-for-byte compatibility. These are
deliberately out of scope; the project measures a subset and says so.

## How progress is measured

- **Part:** `cargo test -p ferrocad_part_py --test part_conformance -- --nocapture`,
  with the upstream revision recorded next to every published number.
- **Part API breadth:** `python3 tools/part_api_audit.py --check`; the pinned contract is
  `contracts/part-api.json`. Record whether the figure came from the committed tree or
  the worktree, and from which extension build.
- **Draft:** `python3 tools/conformance.py --root ../freecad-upstream --draft`, or the
  in-process `cargo test -p ferrocad_part_py --test draft_conformance`.
- **SimpleCAD:** `python3 tools/conformance.py --simplecad` plus
  `tools/simplecad_audit.py --check`, and the in-process
  `crates/ferrocad_part_py/tests/simplecad_conformance.rs`. Rebuild the combined
  extension before comparing.
- **Engine/app:** per-crate `cargo test` and `pytest tests` (no workspace-wide build).
- **Docs:** `api_docs`, `userdocs`, `build_book` and the relative-link sweep in `--check`
  mode.

## Risks and standing constraints

| Risk | Mitigation |
| --- | --- |
| `/home` is nearly full | cap builds (`CARGO_BUILD_JOBS=2`), no workspace-wide builds, keep the archive compact |
| The upstream pin is informal | record `git rev-parse --short HEAD` with every report |
| A stale extension or test expectation reads as a regression | rebuild `ferrocad_combined.abi3.so` before believing a Python or SimpleCAD delta; the fifth round's 3 full-suite failures are expectations (`Part::Prism`, the derived shape wrapper), not kernel behaviour |
| A proxy layer leaks GUI assumptions | keep the headless conformance image as the gate |
| Approximations drift into claims | each says “approximation” at the call site and in the record |
| Naming policy grows into the kernel | the lineage layer stays payload-generic; the kernel only reports provenance |
| Label collisions hide ownership | use the area-prefix rule in [`labels.md`](labels.md) for new work, and qualify old labels in prose |
