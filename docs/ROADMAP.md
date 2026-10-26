# FerroCAD roadmap

Status: roadmap (2026-10-24). The actionable backlog is
[`TODO-26-10-2026.md`](TODO-26-10-2026.md); the measured history is
[`record/milestones.md`](record/milestones.md).

## Where we are

FerroCAD is a Rust/GPUI + PyO3 + pure-Python reimplementation of FreeCAD. The engine,
the app shell and the Python `FreeCAD` surface exist; `Part` is now a real workbench
(**531 upstream tests passing, 0 errors**); the native and `.FCStd` serializers are
independent; and topological naming is a lineage layer rather than a kernel concern.

The bottleneck is no longer geometry. It is the **workbench platform** — objects whose
behaviour is a Python callback — and the remaining Qt/Draft surface on top of it.

## The three horizons

| Horizon | Goal | Exit criteria | Backlog |
| --- | --- | --- | --- |
| **H1 — Finish Part/OCCT** | Complete the Part contract honestly | Zero Part failures (achieved); every reachable guarded map size enabled; no “not implemented” left in the seam for advertised API | P0 remainder, P1 remainder |
| **H2 — Workbench platform** | Make Part a platform Draft/BIM/SimpleCAD can build on | `Part::FeaturePython` executes and persists; Draft M2–M5 run headless; Q4/Q5 done | P2, P3 |
| **H3 — Proof by use** | Prove the platform end to end | SimpleCAD S1–S5 baseline; a published `Parametric` edition; a headless BIM subset | P4, P5 |

### H1 — Finish Part/OCCT (complete except a documented tail, 2026-10-24)

- **The element-map version.** Enabled for every shape that carries a lineage. The
  compound (52), fuse (58), cut/common (26), general-fuse (60), sweep (6), shell/solid
  (26), loft (24), filled face (9) and compound-cut (20/47) sizes match upstream, and the
  Part suite has **zero failures**. The two remaining guarded counts are documented
  non-results: `FaceMakerCheese` (10) needs upstream's face-combo alias, and
  `makeShellFromWires` (24) needs a different (ruled-shell) topology.
- **The seam remainder.** Landed: partial spheres/revolutions, variable-radius fillets,
  `makeOffset2D` filling, hyperbola/parabola/Bezier (including `Geom2d`), multi-shell
  solids, `DirMode Edge/Normal`, B-spline tube spines, the exact `makeHelix`/
  `makeLongHelix` edge, `MakePipeShell` threads and multi-section sweeps, and upstream's
  two-profile thread solid (`BRepOffsetAPI_ThruSections`, 2026-10-26).
- **The tail.** The `curve_of` kinds that still report `unsupported` (hyperbola,
  parabola, Bezier, B-spline, offset); it does not block another workbench.

**Done when** the tail above is closed or deliberately documented. The Part suite already
reports zero failures, and no advertised API raises “not implemented”.

### H2 — Workbench platform (in progress)

- **Python feature proxies (P2a–P2e, landed).** `Part::FeaturePython` and
  `Part::Part2DObjectPython` execute a Python `Proxy`, fire `onChanged`, honour
  `attach=True`, persist natively and call `onDocumentRestored`; the attachment
  extension supplies `AttachmentSupport`/`MapMode`. The Draft M2 modules run green:
  `test_creation.py` **26 passed** and `test_array.py` **1 passed**, with **zero
  failures, errors or load errors**.
- **Qt Q4/Q5 (landed).** [`workbenches/draft-tools.md`](workbenches/draft-tools.md):
  `FreeCADGui.Snapper` over a camera-free snap API, the async point session, the native
  `Draft_Line`/`Draft_Wire` tools and `PlaneGui`, the checkable `Draft_Snap_*` bar, and
  the four remaining forms. The Python suite is **538 passed / 8 skipped** (172
  subtests). The remaining gaps are recorded there: real viewport clicks are not routed
  into `draft.pick`/`escape`/`finish` yet, the headless `ViewObject` lacks the style
  properties SetStyle needs, and the nine Draft-only snap modes raise.
- **Draft M3–M5.** The modification layer, import/export, and the BIM surface on top —
  measured by the `drafttests` modules, not by feature claims.
- **SimpleCAD S1.** The next proving ground; the Q4/Q5 tool surface is the input layer
  it needs.

**Done when** Draft creates, edits and imports headless, and no upstream workbench path
needs Qt for its non-GUI behaviour.

### H3 — Proof by use (then)

- **SimpleCAD S1–S5.** A second consumer as the proving ground: run its suite against a
  Part/pivy image, classify every module, and grow the shared kernel where it hurts.
- **Publish the edition.** The `Parametric` artifact (OCCT closure, launcher, release
  matrix) and the Diátaxis book as a release artifact.
- **BIM headless subset.** The `Arch`/BIM modules that do not need a GUI.

**Done when** an external user can install an edition, run a scripted model and a
workbench, and compare results with upstream.

## Not on the roadmap (for now)

PartDesign's feature tree, Sketcher's solver and constraints, FEM, CAM, the GUI
workbenches, and full upstream `.FCStd` byte-for-byte compatibility. These are
deliberately out of scope; the project measures a subset and says so.

## How progress is measured

- **Part:** `cargo test -p ferrocad_part_py --test part_conformance -- --nocapture`,
  with the upstream revision recorded next to every published number.
- **Draft:** `python3 tools/conformance.py --root ../freecad-upstream --draft`.
- **SimpleCAD:** `python3 tools/conformance.py --simplecad` plus
  `tools/simplecad_audit.py --check`.
- **Engine/app:** per-crate `cargo test` and `pytest tests` (no workspace-wide build).
- **Docs:** `api_docs`, `userdocs`, `build_book` and the relative-link sweep in `--check`
  mode.

## Risks and standing constraints

| Risk | Mitigation |
| --- | --- |
| `/home` is nearly full | cap builds (`CARGO_BUILD_JOBS=2`), no workspace-wide builds, keep the archive compact |
| The upstream pin is informal | record `git rev-parse --short HEAD` with every report |
| A proxy layer leaks GUI assumptions | keep the headless conformance image as the gate |
| Approximations drift into claims | each says “approximation” at the call site and in the record |
| Naming policy grows into the kernel | the lineage layer stays payload-generic; the kernel only reports provenance |
