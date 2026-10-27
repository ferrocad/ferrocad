# Closed task specs

Status: index (2026-10-27). These are the task specs whose implementation is recorded as
closed. They moved here from [`../../tasks/`](README.md) when their work
landed, so `tasks/` holds only active specs. The compact history is
[`../milestones.md`](../milestones.md); the backlogs that closed them are the
`TODO-*.md` files beside this folder.

Every spec keeps its original design and as-built sections; only the relative links and
the status preface changed when it moved. A spec with a documented tail stays here: the tail is carried in
the active backlog or listed below, not kept as an open task.

| Spec | Label | Closed | What it decided or delivered |
| --- | --- | --- | --- |
| [`d0-draft-import.md`](d0-draft-import.md) | Draft `D0` | 2026-10-22 | The five headless import fixes (`Quantity(value, Unit)`, `lazy_loader`, the `PySide` alias, the `Draft_rc` stub, `src/Ext` on `sys.path`), the `--draft` conformance group, and the `FreeCADGui.getDocument` API. |
| [`dev-tools-window.md`](dev-tools-window.md) | Dev Tools `D1–D6` | 2026-10-22 | The inspector, property editor, console and command list in a separate Dev Tools window; the main window becomes the viewport and status line. |
| [`element-map-policy.md`](element-map-policy.md) | Lineage `S6c` | 2026-10-24 | The compound-BOP policy and the naming cascade; every reachable guarded map size matches upstream. `FaceMakerCheese` (10) and `makeShellFromWires` (24) remain documented non-results. |
| [`format-serializers.md`](format-serializers.md) | Serializers `F1–F8` | 2026-10-23 | Native `.ferrocad` JSON and upstream `.FCStd` as independent serializers, including the real ZIP writer, `GuiDocument.xml` and the lineage sidecar. FCStd breadth remains a backlog tail. |
| [`grid-visibility.md`](grid-visibility.md) | Grid visibility `P0` | 2026-10-22 | One grid on/off switch over the native flag, the provider `refresh` hook and the `semantic` → `ferrocad` rename. |
| [`kernel-state-sharing.md`](kernel-state-sharing.md) | Kernel `K0–K4` | 2026-10-26 | The observable one-core identity, the mixed-image guard, interpreter-scoped Python tables and the `Kernel` facade. K2c and K4 are parked, not active. |
| [`links.md`](links.md) | Links `L0–L5` | 2026-10-24 | `App::Link`, link arrays, sub-element references and the lineage/format interlocks. Copy-on-change semantics remain a non-goal. |
| [`mesh-module.md`](mesh-module.md) | Mesh `M1/M1b` | 2026-10-26 | `ferrocad_mesh`, `ferrocad_mesh_py`, `Part.Shape.tessellate`, facet groups, `.bms` and the `.FCStd` mesh member. |
| [`p2-architecture.md`](p2-architecture.md) | Architecture `P2.1–P2.5` | 2026-10-22 | `ferrocad_curves`, the Part/Draft seam, the factory order and the SimpleCAD decision. The gated Qt-split-plane and optional `QtSvg` notes are retained as decisions. |
| [`part-feature-proxies.md`](part-feature-proxies.md) | Proxies `P2a–P2e` | 2026-10-24 | `Part::FeaturePython` and `Part::Part2DObjectPython` execute, change, attach, persist and restore; the Draft M2 modules run green. |
| [`qt-reduction.md`](qt-reduction.md) | Qt `Q0–Q5` | 2026-10-24 | The measured Qt surface, the `.ui` reader and micro-shim, the gpui forms and the native Draft tool layer. The remaining Qt-only GUI paths are documented in `draft-tools.md`. |
| [`settings-model.md`](settings-model.md) | Settings `S0–S4c` | 2026-10-22 | Provider-advertised settings, the `Settings` tree, file-backed persistence and the grid/axis/gizmo capabilities. Document-scoped overrides (S5) remain optional. |
| [`toolbar-chrome.md`](toolbar-chrome.md) | Chrome `T0–T7` | 2026-10-22 | The viewport-header split buttons, anchored popovers, the action dock and the title-bar app menu. |
| [`viewport-event-routing.md`](viewport-event-routing.md) | Event routing `P0.8` | 2026-10-22 | Pane events over the canvas no longer reach the viewport, and an editable settings field no longer closes its card. |
| [`viewport-feedback-and-window-bounds.md`](viewport-feedback-and-window-bounds.md) | Feedback `P0.8` | 2026-10-22 | The Grid control shows its state on hover; a maximized window maps pointer coordinates correctly and hides resize grips. |

The label column uses the historical names; [`../../labels.md`](../../labels.md) is the
index that disambiguates them (several letters were reused by different programs).
