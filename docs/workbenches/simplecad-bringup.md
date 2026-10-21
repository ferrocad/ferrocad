# SimpleCAD bring-up: a canvas-first proving ground

Status: design (2026-10-21).

**Purpose.** Audit SimpleCAD 0.38.0 as FerroCAD's next integration target, test the
hypothesis that it is a better proving ground than `src/Mod/Draft`, map its geometry
and interaction onto what FerroCAD has already built, and give an ordered bring-up
plan **S0–S5**. This is a measurement and a plan; no source file was changed and no
test was executed for it.

Companion to [`TODO-21-10-2026.md`](TODO-21-10-2026.md) (the measurement stash and
the three open decisions), [`conformance.md`](conformance.md) (the upstream harness
this plan extends), [`topology-selection.md`](topology-selection.md) (F0: the
topology, selection and tessellation seam), [`stage.md`](stage.md) (the declarative
presentation stack), [`axis-origin-grid.md`](axis-origin-grid.md) (workplanes,
snapping and grid feedback), [`gizmo-manipulators.md`](gizmo-manipulators.md) (the
manipulator model), [`occt-integration.md`](occt-integration.md) (the kernel seam),
[`python-bindings.md`](python-bindings.md) (the Python facade) and
[`actions-and-tools.md`](actions-and-tools.md) (the command and dialog protocol), and
the two decisions written alongside the TODO: [`qt-strategy.md`](qt-strategy.md)
(the split plane) and [`part-draft-layering.md`](part-draft-layering.md) (the
`ferrocad_curves` split). This note applies those decisions to SimpleCAD rather than
reopening them.

---

## 0. Provenance and limits

| Item | Value |
| --- | --- |
| FerroCAD revision | `13552db` (2026-10-20), `main`; the working tree additionally holds this note and the untracked design notes (`TODO-21-10-2026.md`, `qt-strategy.md`, `part-draft-layering.md`) |
| SimpleCAD checkout | `../simplecad`, sibling of the git repo, **outside version control** |
| SimpleCAD revision | `9180c4b` — “Release 0.38.0”, 2026-10-20 |
| SimpleCAD history | shallow (`--depth 1`): **1 commit**; release cadence is not measurable from git |
| Upstream FreeCAD checkout | `../freecad-upstream` at `33adf61` (2026-10-14), sparse `src/Mod/{Test,Part,Draft,BIM}`, `src/Ext` |
| Package metadata | `package.xml` 0.38.0, MIT, `freecadmin` 1.1 |
| Test execution | **none.** Every test number here is a static count of `def test_`, test classes and assertion sites. |
| Network | unavailable; nothing was cloned, fetched or installed |

Counts are over the whole SimpleCAD tree unless a table says `tests/unit` or
`tests/gui`. “Measured” means a command in Appendix A was run against the checkout
above. “Estimate” is labelled wherever it appears.

Two numbers in the brief needed qualification:

- **“156 test files”** is true as *files under `tests/`*: 156 files. Only **154** are
  Python: 88 unit test modules, 1 unit helper (`surface_fixtures.py`), 63 GUI suites
  and 2 benchmarks. The rest are one `README.md` and one `.brep.gz`. There are
  **153 test/benchmark modules**, not 156.
- **“83 mention pivy/coin”** could not be reproduced. Token counts give **39** files
  containing `pivy` or `coin`, **36** importing `pivy.coin`, and **51** distinct
  `So*` class names. The `Part` count of **42** *is* correct once `MeshPart.*` is
  excluded (a naive `\bPart\.` search also matches `MeshPart.meshFromShape` and
  `MeshPart.wireFromSegment`).

---

## 1. Verdict

| Hypothesis | Evidence in the tree | Verdict |
| --- | --- | --- |
| **Sits directly on `Part`** | 180/283 files import `Part`; 0 imports of `Draft`; 1 file imports `PartDesign`; 0 `Sketcher`, 0 `ifcopenshell`, 0 `Arch`/`BIM` | **Supported** |
| **Uses almost no Qt** | Product code has exactly **one** direct `PySide` import (`simplecad/gui/qt.py`), **0** `.ui` files, **11** widget subclasses (9 `QDialog`, 2 `QWidget`) plus **8** `QObject` subclasses and **no** custom `Signal()` definitions. It still uses **58** distinct Qt names through that one seam. | **Supported relative to Draft; overstated as “almost none”** |
| **Canvas-first** | One `Controller`; two Coin event-callback sites; a 1,131-line `Snapper`; pick → snap → preview → commit; 52 command definitions and 25 tool modules, no task-panel workflow | **Supported** |
| **Better proving ground than Draft** | 1,066 headless unit tests vs Draft's 138 (38 GUI); same 42-name `Part` breadth; much smaller Qt surface | **Supported for S0–S4; S5 (GUI) is still a real port** |

The short version: SimpleCAD is a better **first integration** because its headless
half exercises the `Part` kernel and its interaction half exercises picking, hover,
workplanes and push/pull — the things FerroCAD already has. It is not a “no Qt”
target: it still needs a Qt workbench host for its 63 GUI suites, but its Qt is
concentrated behind one import seam and no `.ui` files, which is exactly where a
micro-shim can buy time.

---

## 2. The tree, measured

### 2.1 Size and layout

| Part of the tree | `.py` files | Notes |
| --- | ---: | --- |
| `simplecad/` package | 119 | `gui/` 18, `gui/tools/` 26 (25 modules + `__init__.py`), `gui/previews/` 5, `modeling/` 17, `mesh/` 14, `surfaces/` 9, `document/` 7, `mcp/` 6, `jobs/` 5, `core/` 4, `repair/` 4, `inspection/` 3, package root 1 |
| `tests/` | 154 | 88 unit modules + 1 helper, 63 GUI suites, 2 benchmarks |
| `scripts/` | 8 | workers, MCP stdio server, installers, test runners |
| root | 2 | `Init.py`, `InitGui.py` |
| **Total** | **283** | matches the brief |

Other measured facts:

| Fact | Measured |
| --- | --- |
| `.ui` files | **0** |
| `ViewProvider` references | **0** |
| Custom document objects / Python proxies | **0** (`face_browser.py` only mentions `DocumentObjectItem` in a docstring) |
| `Gui.addWorkbench` calls | 1 (`InitGui.py`) |
| Toolbar/menu command definitions | **52** (`TOOLS` 33 + `SURFACE` 9 + `SMART` 4 + `EXTRAS` 6) |
| GUI tool-session modules | 25 (+ `__init__.py`) in `simplecad/gui/tools/` |
| MCP tools advertised | 46 (the architecture note's “46-tool schemas”), served by `simplecad/mcp/catalog.py` |

`Init.py` is intentionally inert (one docstring). `InitGui.py` imports the workbench
class from a normal module and calls `Gui.addWorkbench(...)`; the architecture note
explains why the class lives outside the loader’s `exec` namespace
(`../simplecad/docs/development/architecture.md` §“Workbench startup and the controller”).

### 2.2 Tests: the unit/GUI split, honestly

| Directory | Files | Test methods | `TestCase` classes | Assertion sites | Shape |
| --- | ---: | ---: | ---: | ---: | --- |
| `tests/unit` | 89 (88 `test_*.py` + `surface_fixtures.py`) | **1,066** | **174** | **4,186** `self.assert*` | `unittest`; 8 `@unittest.skip*` |
| `tests/gui` | 63 | 0 `def test_` | 0 | **1,914** bare `assert` | 63 imperative `run()` scripts, one per suite |
| `tests/benchmarks` | 2 | 0 | 0 | 0 pass/fail | `run()` timing scripts |
| `tests/fixtures` | 0 `.py` | 0 | 0 | 0 | `README.md`, `fitted_surface_loop.brep.gz` |

So the honest split is: **1,066 discoverable unit tests** in a conventional unittest
tree, and **63 GUI scenarios whose pass condition is “`run()` did not raise”**, with
1,914 assertions spread through them. The GUI number is not comparable to the unit
number; do not add them.

Largest unit suites (test methods): `test_geometry.py` 32, `test_surface_line.py` 31,
`test_surface_wrap.py` 29, `test_surface_edit.py` 29, `test_center_snapping.py` 29,
`test_coincident.py` 28, `test_dense_drawing.py` 27, `test_edge_deletion.py` 25.
Surface suites (`test_surface_*.py`, 8 files) total **174** tests; mesh suites
(`test_mesh_*.py`, 16 files) total **136**.

### 2.3 Substrate: what it imports

| Imported module | Files | What it means for FerroCAD |
| --- | ---: | --- |
| `FreeCAD` (`App`) | **217** | facade surface is large but mostly the parts we already have (`Vector`, `Placement`, `Rotation`, `Document`, `Console`, `ParamGet`, observers) |
| `Part` | **180** | the real dependency; 42 distinct names (§4.1) |
| `FreeCADGui` (mentioned) | **111** | 101 import it at module top; the rest reference it in comments/tests |
| `pivy` / `coin` | **39** | 36 import `pivy.coin`; 51 distinct `So*` names |
| `PySide` (mentioned) | **15** | 14 direct imports + 1 comment-only (`gui_mcp.py`); see §6 |
| `numpy` | 37 | no packaging file in FerroCAD installs it; a test-image requirement (runtime unverified) |
| `scipy` | 2 | `surfaces/fit.py`, `modeling/analytic.py`; required only by surface fitting |
| `Mesh` | 35 | FerroCAD has no `Mesh` module |
| `MeshPart` | 13 | `meshFromShape` 12, `wireFromSegment` 2 |

Workbench dependencies, checked one by one:

| Dependency | Hits | Where |
| --- | ---: | --- |
| `Draft` | **0** | searches for `import Draft` and for `from Draft` return nothing |
| `PartDesign` / `Sketcher` | **1** | `simplecad/surfaces/offset.py:248`: `import PartDesign` to register `PartDesign::Draft`; the draft faces tool runs in a worker. `Sketcher`: 0 |
| `ifcopenshell` / `Arch` / `BIM` | **0** | — |
| `Surface` | **1** | `simplecad/surfaces/edit.py:698`: a temporary `Surface::Filling` object for Fill hole |
| `Mesh` object type | **2** | `simplecad/jobs/mesh.py:221,254`: `Mesh::Feature` for edited/split meshes |

This is the strongest form of the “directly on Part” claim: even the hidden
workbench dependencies are single tools in worker processes, not architecture.

### 2.4 The README claims vs the tree

| README says | Tree contains |
| --- | --- |
| “FreeCAD 1.1 or later” | `package.xml` `<freecadmin>1.1</freecadmin>`; `InitGui.py` imports `FreeCADGui` unconditionally |
| “Nothing else to install: FreeCAD’s installers include Python, OpenCascade, Qt, Coin, NumPy and SciPy” | true of an upstream FreeCAD install; **not** true of FerroCAD's bundled CPython, which ships the stdlib and the `FreeCAD`/`Part`/`pivy` facades, and whose packaging files install no NumPy, SciPy or PySide |
| “No custom document objects or saved Python proxies” | **true**: 0 `ViewProvider`, 0 proxy classes, `Part::Feature` everywhere, plus `SC_Support` (`App::PropertyLink`) |
| “Every edit is one Undo step” | `document/operations.py:15` wraps each edit in one `doc.openTransaction`; the tests assert Undo counts |
| “Results are ordinary Part and Mesh objects” | true; `Part::Feature` 16 call sites, `Mesh::Feature` 2, `Surface::Filling` 1, `PartDesign::*` 1 file |
| “Developed and used on macOS; Linux tests; Windows untested” | no Windows-specific code paths found; not exercised here |

### 2.5 Can `tests/unit` run headless under `GuiUp = 0`?

Mostly yes, with named exceptions. Measured across the 88 unit test modules:

| Requirement | Files | Consequence |
| --- | ---: | --- |
| Import `Part` | 69 | the core geometry suites run only once `Part` is wide enough (§4) |
| Import `FreeCAD` | 72 | document creation, `Vector`, `Placement`, `UndoMode` |
| Import `FreeCADGui` **at module top** | **0** | no unit suite requires a window just to import |
| Reach `FreeCADGui` at module import | 2 (`test_idle_work.py`, `test_push_pull_ui.py`) | both import GUI helpers; `test_arrow_planes.py` guards with `skipIf(Controller is None)` |
| Reference a module on the `gui.qt` chain | 18 | needs PySide6 or an import shim (§6); several do so only inside one test or with a stub |
| Import `pivy` | 9 | the app image installs `pivy.coin`; the bare extension does not |
| Import `numpy` | 14 | test-image dependency |
| Import `scipy` | 2 | surface fitting only |
| Import `Mesh` / `MeshPart` | 15 | no module in FerroCAD; expected load errors |
| Call `App.newDocument` | 50 | headless document lifecycle is exercised broadly |
| Create a `QApplication` in `setUpClass` | 1 (`test_push_pull_ui.py:43`) | needs `QT_QPA_PLATFORM=offscreen` or Qt |

**Verdict.** The unit tree is genuinely headless: no module imports `FreeCADGui` at
module scope, and half of it creates documents without a view. It is not
*universally* headless: 18 modules touch Qt, 15 need Mesh, and one constructs a
`QApplication`. S1 counts all 88 and reports the exceptions rather than hiding them.

---

## 3. Architecture in FerroCAD’s terms

```mermaid
flowchart TB
    subgraph SC["SimpleCAD 0.38.0"]
        InitGui["InitGui.py · Gui.addWorkbench"]
        Workbench["gui/workbench.py · toolbars, menus, context menus"]
        Commands["gui/commands.py · 52 command definitions"]
        Controller["gui/controller.py · one Controller"]
        Tools["gui/tools/* · 25 tool modules"]
        Snap["gui/snapping.py · Snapper, pick, ray"]
        Preview["gui/previews/* · temporary Coin nodes"]
        Ops["document/operations.py · one transaction per edit"]
        Algo["modeling/ surfaces/ repair/ · mesh/ inspection/"]
        Part["Part (OCCT)"]
    end
    subgraph FC["FerroCAD today"]
        Hooks["FreeCAD.display_mesh · FreeCAD.interaction"]
        View["ferrocad_viewport · InventorViewport"]
        Stage["ferrocad_stage · Stage, ShapeItem, Snap, Tool, Gizmo"]
        Pick["ferrocad_wgpu::pick · G-buffer"]
        Seam["ferrocad_geom · GeometryBackend"]
        Occt["ferrocad_occt"]
        PartPy["ferrocad_part_py · Part.Shape + makeBox"]
    end
    InitGui --> Workbench --> Commands --> Controller
    Controller --> Tools
    Controller --> Snap
    Controller --> Preview
    Tools --> Ops
    Snap --> Ops
    Ops --> Algo --> Part
    Hooks --> Stage
    View --> Stage
    View --> Pick
    Stage --> Seam
    Occt --> Seam
    PartPy --> Occt
    Ops -.->|"same Part semantics"| PartPy
```

| Concern | SimpleCAD | FerroCAD today | Consequence |
| --- | --- | --- | --- |
| **Document objects** | Plain `Part::Feature` (and `Mesh::Feature`); no proxy, no `ViewProvider`; `SC_Support` is an `App::PropertyLink`; `SC_*` properties are dynamic | `ferrocad_part::Part::Feature` with `Shape` + `Placement`; `PropertyLink` and `addProperty` exist; no Mesh | **Close match.** The document layer can run almost unchanged; Mesh/Surface object types are the only document-level gaps. |
| **Undo** | `document/operations.transaction()`: capture materials/locks, `openTransaction`, `settle`, validate, commit; every caller uses it | `ferrocad_core` transactions; `doc.UndoMode` exists; F0.6 proves undo/redo of computed shapes | **Works.** `UndoMode` is a no-op stub, which is enough for SimpleCAD's `if not doc.UndoMode` guard. |
| **Geometry** | Direct `Part` calls from every algorithm package; no kernel seam; `core/geometry.py` is 2,259 lines of shape-level work | `ferrocad_geom` seam + `ferrocad_occt`; 6 core operations plus tessellation/topology | **The gap.** The `Part` surface is 42 names wide; our backend exposes box/fuse/cut/fillet/chamfer/place/resolve/save/load. See §4. |
| **Display** | FreeCAD's built-in Part view provider for the shape; imperative Coin overlays for previews and style; no `ViewProvider` subclass | `FreeCAD.display_mesh(obj)` hook returns a tessellation dict; `ferrocad_stage` compiles `ShapeItem`s; `pivy.coin` is a partial facade | **Structurally close.** The overlays are the mismatch: SimpleCAD mutates a Coin subtree, FerroCAD rebuilds declarative items. |
| **Interaction** | One `Controller` owns the tool, captured document/view, mouse callback, preview and timers; `Snapper` resolves every pointer move; tool sessions in `gui/tools` | `InventorViewport` owns pointer dispatch, `Tool`/`Manipulator`/`TransformGizmo`, `SnapContext`/`SnapCandidate`, `SelectionEvent`/`SelectionRef` | **Same shape, missing adapter.** A `FreeCADGui` view facade over `InventorViewport` is the bring-up trick (§5). |
| **Commands** | `Gui.addCommand` + `SimpleCADWorkbench.Initialize` appends toolbars/menus; 52 commands; enablement in `command_state.py` | `FreeCADGui.addCommand` + an actions bar; no workbench class, no toolbars/menus/context menus | **S5.** The command bodies are portable; the chrome is not. |
| **Tests** | `unittest` headless + `run()` GUI suites inside a real FreeCAD window | conformance harness for unittest files; Xvfb for the window | **S0/S1** reuses the harness model; **S5** needs the window. |

### 3.1 Document objects

The architecture note states the rule plainly: results are ordinary Part and Mesh
objects, “no custom document objects or saved Python proxies”
(`../simplecad/docs/development/architecture.md` §“Package layout”). `document/operations.py`
creates `Part::Feature`, assigns `Shape`, and adds `SC_Support` as an
`App::PropertyLink` only for a profile drawn on a face (`operations.py:81–105`). The
Merge/Slice/Mesh tools create temporary documents (`hidden=True`, `temp=True`) to
use `Part`, `Surface` or `PartDesign` features, then copy the resulting `Shape` back
and close the document.

For FerroCAD this means:

- `Part::Feature` plus `PropertyLink` is enough for the main path.
- The temporary-document idiom works if `App.newDocument(..., hidden=True, temp=True)`
  and `closeDocument` exist; both do.
- `Mesh::Feature` and `Surface::Filling` do **not** exist; those tools are out of the
  Part proving-ground slice.

### 3.2 Geometry

`modeling/`, `surfaces/`, `repair/`, `mesh/` and `inspection/` call `Part` directly.
`core/geometry.py` is the centre of gravity: plane/rectangle/circle/sphere/polygon
construction, face division with lines and profiles, push/pull planning, opposite-face
contact, exact stop and past-face extension, `generalFuse`-based splitting, and
validity/area/volume checks. There is no kernel seam and no backend abstraction.

FerroCAD's rule is the opposite: presentation crates never name OCCT, and the document
core talks to a `dyn GeometryBackend`. The bring-up therefore cannot “load SimpleCAD
and let it call Part” until the `Part` facade is wide enough to be that seam. That is
S2/S3, and it is the critical path for Draft too.

### 3.3 View

There is no view-provider layer. FreeCAD draws `Part::Feature` with its own provider;
SimpleCAD adds transient Coin nodes through `view.getSceneGraph()` and mutates
per-object `ViewObject` properties (`ShapeColor`, `ShapeAppearance`, `LineColor`,
`RootNode`, `DisplayMode`). Under `GuiUp = 0` all of that is guarded by `App.GuiUp`
in the document layer, but a few GUI helpers still dereference `ViewObject`
unconditionally (`gui/pull_snapping.py:27`, `gui/box_selection.py:131`).

The FerroCAD equivalent is `FreeCAD.display_mesh` → `stage_meshes` → `Stage` →
`ShapeItem` (`python/ferrocad_shell/__init__.py:295`,
`python/ferrocad_parametric/__init__.py:32`). It already maps `Tessellation.parts`
to `ShapeItem::part_names`, which is what a pick needs. A `ViewProvider` is not
required; the hook is the seam.

### 3.4 Interaction

`gui/controller.py` (1,670 lines) is one `QtCore.QObject` singleton. It owns the
active tool, the captured `doc`/`view`, the preview, the snapshots, three timers, and
the event filter. `activate()` installs an application event filter and a status-bar
widget; `start(tool)` validates the selection with `command_state.reason`; `click`,
`motion`, `hover`, `numeric`, `commit` and `cancel` drive the tool. `coin_event`
(controller.py:801) is the only bridge from Coin to the controller: it translates
`SoMouseButtonEvent`/`SoLocation2Event` into `click`/`motion`, and calls
`callback.setHandled()` so FreeCAD's navigation style does not also act.

`Snapper` (`gui/snapping.py:608`) is the canvas brain: index edges, vertices, face
centres and circle centres; project them to screen; filter occluded candidates;
distinguish endpoint/midpoint/centre/on-edge/axis; infer world axes within a narrow
screen cone; and keep a screen-space point grid. `pick()` (snapping.py:94) turns
`getObjectsInfo` hits into `(object, subshape, point, name)`, preferring `SC_Support`
profiles and filtering hidden elements.

FerroCAD has the same decomposition: pointer → pick → snap → tool → commit, but with
declarative stage items and neutral `SelectionRef`s. The mapping is in §5.

### 3.5 Commands and undo

`gui/commands.py` defines 52 commands, `gui/workbench.py` appends five toolbars and
menus, and `command_state.py` computes enablement from the selection. `Gui.runCommand`
is used only for `Std_Copy`/`Std_Paste` in product code; the other 29 occurrences are
tests. Every edit goes through `document/operations.transaction()`; previews never
reach the document.

FerroCAD's `FreeCADGui.addCommand`/`runCommand` registry and the shell's actions bar
cover the command half. There is no workbench class, toolbar API or context menu yet;
that is S5.

---

## 4. Geometry: what the tests exercise, and what we have

### 4.1 The `Part` surface is 42 names

Measured over all 283 files, excluding the `MeshPart.*` prefix:

| Group | Names and occurrences |
| --- | --- |
| Primitives | `makeBox` 442, `makeCylinder` 183, `makeSphere` 39, `makeCone` 6, `makeTorus` 4 |
| Profiles and faces | `Face` 183, `makePlane` 139, `makeLine` 139, `makePolygon` 93, `makeCircle` 50, `makeShell` 57, `makeSolid` 26, `makeFace` 9, `makeLoft` 1, `makeFilledFace` 1, `makeWireString` 1, `makeCompound` 186 |
| Curve/surface classes | `Plane` 125, `Circle` 94, `Cylinder` 84, `Wire` 76, `Vertex` 65, `Line` 37, `Sphere` 17, `Ellipse` 15, `Shell` 14, `BSplineSurface` 14, `Arc` 11, `ArcOfCircle` 6, `Solid` 6, `Cone` 6, `Compound` 6, `Geom2d` 4, `LineSegment` 3, `Edge` 2, `BSplineCurve` 2, `Toroid` 1, `ShapeFix` 1 |
| `Shape` and helpers | `Shape` 24, `OCCError` 86, `sortEdges` 6, `__sortEdges__` 7 |

The **42-name breadth is identical to Draft's 42**, but the distribution is not:
Draft's top names are `LineSegment` 93, `Wire` 87, `OCCError` 58, `Shape`/`Face` 56;
SimpleCAD's are `makeBox` 442, `makeCompound` 186, `Face` 183. Draft builds shapes and
wires; SimpleCAD *edits* whole solids and faces. That difference favours SimpleCAD as
a kernel proving ground: it exercises booleans, push/pull, extrusion and whole-solid
queries rather than 2D wire assembly.

### 4.2 Shape-level surface

These are member accesses and calls on a shape value, not on the `Part` module.
Counts are textual occurrences over `simplecad/` and `tests/unit`; they show what a
`Part.Shape` must answer.

| Member | Occurrences | Member | Occurrences |
| --- | ---: | --- | ---: |
| `.Faces` | 1,159 | `.Volume` | 614 |
| `.Area` | 510 | `.Length` | 492 |
| `.Edges` | 480 | `.Solids` | 399 |
| `.CenterOfMass` | 343 | `.Surface` | 283 |
| `.Placement` | 272 | `.copy()` | 216 |
| `.isSame()` | 248 | `.Vertexes` | 241 |
| `.BoundBox` | 181 | `.isClosed()` | 177 |
| `.cut()` | 166 | `.Curve` | 139 |
| `.ShapeType` | 159 | `.isValid()` | 397 |
| `.common()` | 85 | `.Wires` | 82 |
| `.extrude()` | 75 | `.fuse()` | 26 |
| `.generalFuse()` | 19 | `.removeSplitter()` | 18 |
| `.makeFillet()` | 13 | `.makeChamfer()` | 2 |
| `.makeOffsetShape()` | 4 | `.makeThickness()` | 1 |

Unit tests alone make the priority clear: `makeBox` 357, `makeCylinder` 153,
`makePlane` 123, `makeLine` 112, `Face` 92, `makeCompound` 82, `makePolygon` 65;
`Faces` 780, `Volume` 462, `Area` 351, `cut` 140, `copy` 95, `extrude` 60.

### 4.3 Gap table

Status vocabulary: **exists** (Python and kernel), **partial** (kernel and/or Python
but not the upstream shape), **missing** (neither). Slices are defined in §7.

| # | Operation | Upstream `Part` API | FerroCAD today | Status | Slice |
| ---: | --- | --- | --- | --- | --- |
| 1 | Box primitive | `Part.makeBox` | `Part.makeBox` → `ferrocad_part::make_box` → OCCT; tessellation with part names | **exists** | — |
| 2 | Planar profiles | `makePlane`, `makePolygon`, `makeLine`, `Face(Wire)` | none | **missing** | S2.1 |
| 3 | Wire/edge/vertex | `Wire`, `Edge`, `Vertex`, `Line`, `sortEdges`, `__sortEdges__` | none | **missing** | S2.1 |
| 4 | Assembly | `makeCompound`, `makeShell`, `makeSolid`, `Compound`, `Shell`, `Solid` | none | **missing** | S2.1 |
| 5 | Shape queries | `.Faces/.Edges/.Vertexes/.Wires/.Shells/.Solids`, `.ShapeType`, `.isValid`, `.Volume`, `.Area`, `.Length`, `.BoundBox`, `.CenterOfMass` | `Shape` is opaque: `isNull`, `isSame`, `mesh`, BRep export; no measurement or accessors | **missing** | S2.1 |
| 6 | Copy / placement | `Shape.copy()`, `Shape.Placement`, `transformShape` | backend `place` (rigid) exists; `Part.Shape` has no `Placement` and no `copy` | **partial** | S2.1 |
| 7 | Booleans | `.fuse`, `.cut`, `.common`, `.multiFuse`, `.generalFuse` | backend `fuse`/`cut`; `common`/`generalFuse`/`multiFuse` absent; Python shape methods absent | **partial** | S2.2 |
| 8 | Extrude / push-pull | `Face.extrude`, `Shape.extrude` | absent; no `BRepPrimAPI_MakePrism` binding | **missing** | S2.2 |
| 9 | Refine | `.removeSplitter()` | absent | **missing** | S2.2 |
| 10 | Curved primitives | `makeCylinder`, `makeSphere`, `makeCone`, `makeTorus` | absent | **missing** | S3.1 |
| 11 | Curve/surface types | `Plane`, `Circle`, `Arc`, `ArcOfCircle`, `Ellipse`, `LineSegment`, `BSplineCurve`, `BSplineSurface`, `Cylinder`, `Sphere`, `Cone`, `Toroid`, `Geom2d` | absent; no type identity for `isinstance(face.Surface, Part.Plane)` | **missing** | S3.1 |
| 12 | Fillet/chamfer | `.makeFillet`, `.makeChamfer` | backend fillet/chamfer with lineage; `Part::Fillet`/`Part::Chamfer` features; no Python shape method | **partial** | S3.2 |
| 13 | Offset/shell/thicken | `.makeOffsetShape`, `.makeOffset2D`, `.makeThickness`, `makeShell` | absent | **missing** | S3.2 |
| 14 | Loft/sweep | `makeLoft`, `.loft`, `.sweep`, `makePipeShell` | absent | **missing** | S3.3 |
| 15 | BRep persistence | `Shape()`, `.read`, `.write`, `.exportBrep` | backend `save_shape`/`load_shape`; Python `exportBrep`, `exportBrepToString`; no `read`/`write` | **partial** | S2.1 |
| 16 | Tessellation | `.tessellate`, `MeshPart.meshFromShape` | `Shape.mesh()` (FerroCAD extension) and `tessellate`; no `MeshPart` | **partial** | S3.4 |
| 17 | Errors | `Part.OCCError` | absent; kernel errors surface as `RuntimeError` | **missing** | S2.1 |
| 18 | Name resolution | `.getElement`, `.getSubShape` | backend `resolve` is positional-only; no Python accessor | **partial** | S2.1 |

Two rows deserve emphasis:

- **`Part.OCCError` (86 references).** It is not cosmetic: the code writes
  `except (AttributeError, Part.OCCError, RuntimeError, ValueError)`. If
  `Part.OCCError` is absent, evaluating that tuple *while handling a geometry error*
  raises `AttributeError`, converting a handled failure into a crash. S2.1 must add
  it as the exception class the kernel actually raises.
- **`Shape.Placement` (272 references).** FreeCAD shapes carry a placement that is
  not baked into the geometry; SimpleCAD relies on `copy(); shape.Placement = ...`
  to work locally, and even compares `obj.Shape.Placement.isSame(obj.Placement)`
  (`document/operations.py:50`). FerroCAD's `place` bakes the transform and the
  handle has no placement. This is a *semantic* decision, not a binding detail: keep
  a placement on the Python `Shape` handle and apply it lazily, or bake and lose the
  `isSame` comparison. The recommendation is the former.

### 4.4 What the first run will hit, in order

| Family | Unit files | Test methods | First blocker |
| --- | ---: | ---: | --- |
| Draw, pull, profiles | `test_geometry`, `test_dense_drawing`, `test_dense_pull`, `test_line_regions`, `test_circle_regions`, `test_pull_contact`, `test_pull_references`, `test_push_pull_ui` | **137** (measured) | `Part.Plane/Face/Wire/Vertex`, `.Faces/.Edges`, `.extrude` |
| Surface tools | 8 `test_surface_*.py` | **174** | `Part.BSplineSurface`, `.Surface`, `.Curve`, `generalFuse`, `makeOffsetShape` |
| Mesh tools | 16 `test_mesh_*.py` | **136** | `import Mesh` / `MeshPart` — no module at all |
| Booleans and edits | `test_union`, `test_intersection`, `test_joints`, `test_edge_deletion`, `test_slicing`, `test_rounding` | 16 + 17 + 25 + 20 + … | `.common`, `.generalFuse`, `.removeSplitter`, `.makeFillet` |
| Snapping and UI | `test_center_snapping`, `test_edge_snapping`, `test_face_click_snap`, `test_pick`, `test_selection` | 29 + … | `getObjectsInfo`-shaped view; otherwise pure Python and runnable early |

The draw/pull family and the surface family are the two biggest coherent blocks
(311 tests together); the mesh block (136) is a separate subsystem and should not
gate the proving ground.

---

## 5. Interaction: upstream call by call

### 5.1 The viewport calls

| SimpleCAD site | What it needs | FerroCAD equivalent | Verdict |
| --- | --- | --- | --- |
| `view.getObjectsInfo(pos)` — `gui/snapping.py:84` | an **ordered list** of hits, each with `Document`, `Object`, `Component` (`Face6`), `x/y/z`; occlusion order and boundary priority | `InventorViewport::pick` returns **one** `Hit { id, part, point, normal }`; the G-buffer resolves a pixel, the CPU path a primitive; `hovered_selection()` names the part | **partial — top hit only** |
| `view.getObjectInfo(pos)` — 5 sites | the first hit with the same fields | same as above | **partial** |
| `view.getPointOnScreen(point)` — 26 sites | world → screen pixels | `ferrocad_stage::screen::project_device` | **exists** |
| `ray(view, pos)` — `snapping.py:25` | screen → world ray via `getCameraNode().getViewVolume().projectPointToLine` | `ferrocad_graphics::Camera::ray_through` / `pick_at_hit`; `Workplane::ray_local` | **exists** |
| `view.getViewDirection()` | camera forward | `ferrocad_stage::screen::camera_forward` | **exists** |
| `view.getCameraNode().getField(...)` — `camera_state` | camera type, pose, zoom, viewport | `ferrocad_graphics::Camera` fields | **exists (different API)** |
| `view.addEventCallbackPivy(SoEvent, cb)` — `controller.py:656`; `SoMouseButtonEvent` in `gui/tools/slice.py:146` | pointer callback while a tool is active | `InventorViewport::handle_pointer` / `handle_button` + `Tool::on_event` | **exists (different API)** |
| `view.removeEventCallbackPivy(...)` — `controller.py:217` | teardown | `set_tool(None)` / tool slot | **exists** |
| `Gui.Selection.getSelectionEx()` — 41 sites; `addSelection` 108; `clearSelection` 172; `setPreselection` 8 | selection state, sub-element names, picked points | `FreeCADGui.Selection` is a **read-only snapshot**; the shell owns selection; `SelectionEvent`/`SelectionRef` carry `(object, ["Face6"])` | **partial** |
| `Gui.updateGui()` — 149 sites | force a redraw after a preview change | the gpui shell repaints (`cx.notify`); no Python-visible `updateGui` | **missing (adapter needed)** |
| `Gui.activeDocument().activeView()` — 104 sites | the current 3D view | no document/view registry; the shell owns one `InventorViewport` | **missing (adapter needed)** |
| `Gui.getMainWindow().statusBar()` — 67 sites | messages and the `Controls` widget | gpui status bar; no Python API | **missing (S5)** |
| `Gui.activateWorkbench(...)` — 91 sites | switch workbench | the edition is one module; no workbench switch | **missing (S5, by design)** |
| `Gui.runCommand(...)` — 31 sites (2 product: `Std_Copy`, `Std_Paste`) | invoke another command | `FreeCADGui.runCommand` registry | **exists** |
| `QApplication.installEventFilter(self)` — `controller.py:144` | Tab/Enter/Esc/shortcut handling | gpui key dispatch / `InventorViewport::handle_key` | **partial** |

Two calls from the brief are deliberately absent: `Gui.Control(...)` has **0**
occurrences (there are no custom C++ widgets), and Draft's `Gui.Snapper`,
`Gui.InputHint` and `Gui.UserInput` have **0** occurrences — SimpleCAD carries its own
`Snapper` and resolves hints in Python. That is why its `FreeCADGui` surface is much
smaller than Draft's.

### 5.2 Picking and snapping

`pick()` (snapping.py:94) is not just “the top hit”. It sorts candidates so an
`SC_Support` profile wins over its host, filters `visibility.element_visible`, treats
a hit list as an occlusion stack, and has a `prefer_boundary` mode that picks the
front-most vertex/edge/face at that depth. FerroCAD's G-buffer returns the front-most
pixel result and the part id; it has no list to sort and no `SC_Support` priority.
S4 must either add a ranked pick API (a small list from the CPU path or a priority
tag in the pick pass) or accept the top hit and compensate in the tool. Do not
silently equate the two.

`Snapper` resolves in this order: exact feature points (endpoint, midpoint, face
centre, circle centre) → on-edge → world-axis inference → picked face → plane → grid.
FerroCAD's `SnapKind` covers `Point`, `Grid`, `Vertex`, `EdgeMid`, `Centre`, `Origin`
(`crates/ferrocad_stage/src/snap.rs`). Missing: circle centre, “on edge”, world-axis
inference (`Snapper.line_axis`, `axis_point`), the radius magnet (`_refine`), the
screen-space point grid, and exact projection onto the kernel surface. The last one is
already a documented F0 non-goal (“no exact intersector … `IntCurvesFace_ShapeIntersector`
later”, `docs/topology-selection.md` §11), so the port inherits a known precision gap;
`on_face()` in SimpleCAD exists precisely because the view reports single-precision
picks.

The **face workplane** is the best-matched piece. SimpleCAD's `Plane.from_face(face)`
and `Snapper.face_plane` become FerroCAD's `FaceFrame::resolve` over the stage
tessellation (`crates/ferrocad_stage/src/workplane.rs:121`) plus `Workplane`. Both
reject curved faces. The difference is provenance: SimpleCAD reads `Part.Plane` from
the kernel; FerroCAD fits the display triangles. For planar faces the results agree to
tessellation precision; for a nearly planar face they can differ. S4 should resolve
the workplane from the kernel through the new S2 query surface when precision matters.

### 5.3 Push/pull

The upstream flow is:

1. `click` → `pick(..., face_only=True)` → `(obj, face, anchor, name)`.
2. `extrusion_normal` → the face normal; `extrusion_contact` → the opposite face’s
   exact distance (`OppositeFace`).
3. `pull_drag(view, pos, anchor, normal, down_pos, step)` → a signed distance from the
   screen-space projection of the normal, with a face-on fallback scale.
4. `snap_pull` captures the opposite-face distance while the pointer is within the
   snap radius, and releases it beyond.
5. A Coin preview shows the prism; `commit` calls `document.operations.extrude`, which
   calls `core.geometry.push_pull` / `plan_push_pull`: `face.extrude`, `fuse` or `cut`,
   `removeSplitter`, validity/area/volume checks, and a single transaction.

FerroCAD has the **math** (`TransformGizmo`’s axis projection, `screen::scale_for`)
but not the **operation**: there is no `Face.extrude`, no push/pull manipulator, and
`ferrocad_part_gui` contributes only the edge dress-up arc. S2.2 supplies `extrude`;
S4 supplies the manipulator and the preview. The exact-volume acceptance already
exists in `tests/gui/gui_smoke.py`: a 40 × 30 rectangle becomes area 1200, then a
20 mm pull becomes volume 24000.

### 5.4 What each FerroCAD capability corresponds to

| FerroCAD file / document | Capability | SimpleCAD counterpart | Status |
| --- | --- | --- | --- |
| `ferrocad_stage/src/grid.rs` | world/workplane grid, adaptive steps, triad | `gui/viewport_style.py` ground grid and axes; `Snapper` grid points | **partial** — geometry exists; the visual style and per-face grid timing are different |
| `ferrocad_stage/src/snap.rs` | `SnapKind`/`SnapCandidate`, feature-over-grid | `Snapper.snap`, `nearby_points`, `endpoint_hint`, `midpoint_hint`, `center_hint` | **partial** — vertex/midpoint/centre/origin/grid; missing circle centre, on-edge, axis inference, radius magnet |
| `ferrocad_stage/src/workplane.rs` | `Workplane`, `FaceFrame::resolve` | `core.geometry.Plane`, `Snapper.face_plane`, `Controller.drawing_plane` | **partial** — fitted from the tessellation rather than `Part.Plane` |
| `ferrocad_stage/src/input.rs` | `PointerEvent`, `PointerKind`, `MouseButton` | `coin_event`’s `SoMouseButtonEvent`/`SoLocation2Event` payload | **exists** — a structured replacement |
| `ferrocad_stage/src/tool.rs` | `Tool` trait, transient items, stable identities | `gui/tools/*` sessions and `Controller`’s tool state machine | **exists (different API)** |
| `ferrocad_stage/src/interaction.rs` | `InteractionCommit`, `Manipulator` | `document/operations.commit*` plus each tool’s `commit()` | **partial** — the commit is neutral data, but the Python side has no `Manipulator` seam |
| `ferrocad_stage/src/gizmo.rs` | axis/plane/ring drag, projection, screen scale | Move/Rotate/Scale sessions; `axis_point`/`pull_drag` math | **partial** — transform gizmo exists; no push/pull mode |
| `ferrocad_viewport::InventorViewport` | pick, hover, selection, tool dispatch | `Gui.activeDocument().activeView()` plus `getObjectsInfo`/`addEventCallbackPivy` | **missing adapter** |
| `ferrocad_wgpu::pick` | G-buffer `[entity, part]` pick | `view.getObjectsInfo` (ordered list) / `getObjectInfo` | **partial** — top hit only |
| `docs/topology-selection.md` F0 | `SelectionRef { object, elements }`, part names | `Gui.Selection` plus `Component` (`Face6`) | **partial** — selection is a read-only snapshot |

### 5.5 Selection and previews

`Gui.Selection` is the state SimpleCAD reads and writes; the `SelectionObject` shape
matters (`item.Object`, `item.SubElementNames`, `item.PickedPoints`, `item.ObjectName`
— see `controller.py:353–397`, `mcp/api.py:373`, `display.py:212`). FerroCAD’s
`_Selection` returns `(object, elements)` tuples and has no `.Object`,
`.SubElementNames` or `.PickedPoints`. S4 must add the object shape, `addSelection`,
`removeSelection`, `clearSelection`, `setPreselection` and `getPreselection`; the
snapshot direction (shell → facade) is already built.

Previews are imperative Coin subtrees: `gui/previews/overlay.py` alone uses
`SoSeparator`, `SoDepthBuffer`, `SoText2`, `SoCoordinate3`, `SoBaseColor`,
`SoTranslation`, `SoSwitch`, `SoLineSet`, `SoDrawStyle`, `SoPointSet`, `SoPickStyle`,
`SoLightModel`; `shaded.py` adds `SoPolygonOffset`, `SoMaterial`, `SoFaceSet`. FerroCAD
has none of the line/point/text/draw-style nodes in `ferrocad_inventor_py` (its
constructible set is `SoSeparator`, `SoSwitch`, `SoCoordinate3`, `SoNormal`,
`SoMaterial`, `SoTransform`, `SoIndexedFaceSet`, `SoIndexedLineSet`,
`SoEventCallback`). The tree's most frequent Coin nodes are `SoSeparator` 49,
`SoCoordinate3` 24, `SoDepthBuffer` 22, `SoPickStyle`/`SoLightModel`/`SoDrawStyle`
18 each, `SoBaseColor` 17, `SoMaterialBinding` 15, `SoMouseButtonEvent`/`SoMaterial`
14, `SoLineSet` 13 and `SoText2` 10 (51 distinct `So*` names). **Do not extend Coin
to match.** The previews are transient, unpickable visuals — exactly what
`ferrocad_stage::ShapeItem` with `selectable(false)` and
`DrawMode::{Wireframe, ShadedWithEdges}` is for. Port the
`Preview` API to stage items. The one real gap is **3-D text** (`SoText2` labels for
lengths and dimensions); stage has no text primitive, so either add one to the stage
or keep the readout in the status bar.

---

## 6. The 15 PySide files

Fifteen files contain the token `PySide`: **14 import it** (12 GUI tests plus 2
infrastructure files) and `tests/gui/gui_mcp.py` only mentions it in a comment while
importing `simplecad.gui.qt` like the rest.

| File | Role | What it needs | Shim or port? |
| --- | --- | --- | --- |
| `simplecad/gui/qt.py` | **the product’s only Qt import seam** (32 product modules import it; 90 import sites tree-wide, tests included) | `QtCore`, `QtGui`, `QtWidgets`; tries `PySide`, then `PySide6`, then `PySide2` | **Micro-shim suffices for import.** Behaviour needs the real classes. |
| `scripts/run_gui_tests.py` | GUI test runner | hard `from PySide import QtCore, QtWidgets`; `QApplication`, `QTimer`, active window | **Replace** with the FerroCAD Xvfb runner; not product code |
| `tests/gui/gui_appearance.py` | GUI suite | `QtTest` (fallback) | **Port/skip**; it drives real widgets |
| `tests/gui/gui_face_browser.py` | GUI suite | `QtTest`, `QTreeWidget` | **Port/skip** |
| `tests/gui/gui_hover_appearance.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_locking.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_material_mapping.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_mcp.py` | GUI suite | comment-only `PySide` mention; imports `gui.qt` for `QApplication`/clipboard | **Port/skip** |
| `tests/gui/gui_mesh_region.py` | GUI suite | `QtTest`; Qt event filter + `QMdiArea` | **Port/skip; highest GUI risk** |
| `tests/gui/gui_pull_see_through.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_saved_ring_lines.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_surface_tools.py` | GUI suite | **hard** `from PySide import QtCore, QtWidgets`; live-preview dialogs | **Port/skip; the hard import is a D0 item** |
| `tests/gui/gui_union.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_viewport_style.py` | GUI suite | `QtTest` | **Port/skip** |
| `tests/gui/gui_visibility.py` | GUI suite | `QtTest` | **Port/skip** |

**Micro-shim assessment.** The frame is [`qt-strategy.md`](qt-strategy.md)’s
recommendation D — the split plane: keep the native spatial reference, load `.ui`
declaratively, and shim the widget subset. SimpleCAD has **no `.ui` files**, so the
`.ui` loader is not on its path; the native spatial reference is the
`ferrocad_stage` grid/snap/workplane already mapped in §5. What remains is the
**micro-shim** for code-built widgets. A `PySide` alias over the host’s PySide6 (the
D0 fix in [`TODO-21-10-2026.md`](TODO-21-10-2026.md) §3) is enough to make every import
in the first column resolve, including the two hard `from PySide` imports. It is **not**
enough to run the GUI suites, and it is not enough for any code path that constructs
a widget: the product uses 36 `QtWidgets` classes, 15 `QtCore` and 7 `QtGui` names,
with 9 `QDialog` subclasses, 2 `QWidget` subclasses (`Controls`, `JointControls`)
and 8 `QObject` subclasses (`Controller`, `FaceBrowser`, `DragSelection`,
`SeamDisplay`, `ViewportStyle`, `Bridge`, `Job`, `PanelAnchor`). Those are real
widgets, not import stubs.

The sensible split is:

- **Import-only shim** for S1: let the **18** unit files that statically reach
  `gui.qt` (directly or through `mcp.api`/`face_browser`/`tools`) load, so the Part
  baseline is not blocked on Qt. Several import it only inside one test.
- **Shim the stock dialogs; port the ones a shim cannot carry** for S5. The shell
  already hosts typed modal dialogs (`docs/actions-and-tools.md` §6), so a `QDialog`
  that only reads fields and returns them can become a `DialogSpec`. The exception is
  live-preview dialogs (`PreviewDialog` in `gui/tools/surface.py:117`, which re-renders
  on `valueChanged` via a timer): the shell’s protocol is one modal at a time and has
  no live `on_change` callback. S5 needs a modeless preview mechanism (a
  `view`/`preview` request, or a manipulator) before those tools can move.
- **Do not shim `QtTest`.** A no-op `qWait`/`mouseClick` would make 63 GUI suites
  pass vacuously. They must run against a real window or be explicitly skipped.

---

## 7. Bring-up plan S0–S5

The order is deliberately **measure first, then the shared kernel, then the canvas,
then Qt**. S0 and S1 are cheap and reduce uncertainty; S2/S3 are the largest shared
dependency and unblock Draft as well; S4 is the closest match to what we already
built; S5 is the known-expensive port and is gated on implementing the
[`qt-strategy.md`](qt-strategy.md) split plane.

| Slice | Deliverable | Acceptance | Risk | Depends on |
| --- | --- | --- | --- | --- |
| **S0** | Audit + conformance hook | `--simplecad` group lists and runs; static audit; no checkout committed; CI optional/skipped | **low** | — |
| **S1** | Headless unit-test baseline | all 88 unit modules attempted; every outcome classified; no harness crash; baseline recorded | **low–medium** (environment) | S0 |
| **S2** | `Part` planar/topology slice | construction, queries, booleans, extrude, persistence; the draw/pull and boolean suites execute | **high** | S1 |
| **S3** | `Part` curved/dress-up slice | cylinders/spheres/cones/tori, curve/surface types, offset/thickness, fillet/chamfer, loft/sweep | **high** | S2 |
| **S4** | Interaction parity | view adapter, ranked pick, snapping, push/pull manipulator, selection object | **medium–high** | S2 |
| **S5** | GUI workbench | qt-strategy split plane, commands/toolbars, previews on stage, GUI suites under Xvfb | **very high** | S3, S4 |

### S0 — Audit and CI hook (pure measurement)

Deliverables:

- Add a `simplecad` group to `tools/conformance.py`, alongside the `App` default
  group and `--part` (`docs/conformance.md` §1, `tools/conformance.py:40,224`).
  The group’s source is `<SIMPLECAD_DIR>/tests/unit/test_*.py`, not a subtree of
  `freecad-upstream`; the harness needs a group descriptor that carries **two path
  roots** instead of assuming `--root`.
- `--list --simplecad` prints the 88 unit stems; `--simplecad` prints the same
  `pass/fail/error/skip/load-error` report.
- A static audit records: 283 `.py`, 42 `Part` names, 0 `.ui`, 0 `Draft` imports, 1
  `PartDesign` file, 0 `ViewProvider`, 156 files under `tests/`. It is a drift alarm,
  not a gate on upstream content.
- CI: the `geometry` job may run it **only** when the checkout exists; normal PR CI
  stays hermetic, exactly as `part_conformance` self-skips without
  `FERROCAD_UPSTREAM` (`docs/conformance.md` §5). A scheduled/dispatch job can clone
  the pinned revision.

Acceptance:

- `python3 tools/conformance.py --simplecad --list` → 88 stems.
- `python3 tools/conformance.py --simplecad` → a summary line and zero harness
  exceptions; unset checkout → a skip message, exit 0.
- `git -C ../simplecad status --porcelain` is empty afterwards (the harness never
  edits the checkout).
- The revision (`9180c4b`, 0.38.0) is recorded with every report, as `conformance.md`
  already requires for `freecad-upstream`.

### S1 — Headless unit baseline

Run the 88 `tests/unit` modules **unchanged** against the FerroCAD image that links
`FreeCAD` + `Part` (+ `pivy`), the way `crates/ferrocad_part_py/tests/part_conformance.rs`
runs the Part group in-process. The standalone `python/ferrocad.abi3.so` is not
enough: `import Part` and `import FreeCAD` must share one core, and `pivy.coin` is
installed by the app image (`ferrocad_gpui::python::bootstrap`), not by the bare
extension.

Environment requirements:

- `numpy` (14 unit files reference it; 12 import it at module level); `scipy` (2 modules, and 2 unit files; only the surface-fitting tests need it).
- `PySide6` **or** a test-only `gui.qt` import stub for the **18** unit files that
  reference a module on the `gui.qt` chain (`mcp.api`/`face_browser`/`tools`);
  several do so only inside one test or with a stub.
- No `Mesh`/`MeshPart`: **15** unit files import one of them and will report a load
  error naming the missing module. That is the correct signal, not a harness failure.
- `FreeCAD.GuiUp = 0` (the facade already sets it); `obj.ViewObject` is not
  guaranteed, so a few GUI helpers (`pull_snapping`, `box_selection`) will error if a
  test calls them. Report, do not patch upstream.

Acceptance:

- All 88 modules attempted; 0 uncaught harness exceptions; every module ends in
  `pass`, `fail`, `error`, `skip` or `load error`.
- The report is recorded with the SimpleCAD revision and the FerroCAD revision; it
  has **no pass threshold**. This is the same contract as the existing conformance
  harness: a measurement, not a gate.
- Exclusions are explicit and counted as `skip` with a reason (for example
  `test_push_pull_ui.py` if Qt is absent). Nothing is silently dropped.
- A cargo test (`ferrocad_parametric` or a dedicated harness test) self-skips when
  `FERROCAD_SIMPLECAD` is unset, mirroring `FERROCAD_UPSTREAM`.
- No file under `../simplecad` is modified.

### S2 — The `Part` planar/topology slice

S2.1 (B-Rep construction and queries):

- `Part.makePlane`, `Part.makePolygon`, `Part.makeLine`, `Part.Face`, `Part.Wire`,
  `Part.Edge`, `Part.Vertex`, `Part.makeCompound`, `Part.makeShell`,
  `Part.makeSolid`, and the `Part.Compound/Shell/Solid/Edge/Wire` classes. The
  analytic classes (`Part.Line`, `Circle`, `LineSegment`, `Arc`, …) belong to the
  `ferrocad_curves` leaf recommended by [`part-draft-layering.md`](part-draft-layering.md);
  they arrive in S3.1, not here.
- `Shape.Faces/Edges/Vertexes/Wires/Shells/Solids`, `ShapeType`, `isValid`,
  `Volume`, `Area`, `Length`, `BoundBox`, `CenterOfMass`, `copy`, `Placement`,
  `getElement`/`getSubShape`, `read`/`write`.
- `Part.OCCError` as the kernel error class.
- The placement model from §4.3, decided and tested.

S2.2 (operations):

- `.fuse`, `.cut`, `.common`, `.multiFuse`, `.generalFuse`, `.extrude`,
  `.removeSplitter`.
- The backend additions belong in `ferrocad_occt` behind `ferrocad_geom`; the
  `common` history helpers already exist in `crates/ferrocad_occt/src/bridge.rs`, so
  `common` is closer than its absence suggests.

Acceptance:

- The draw/pull and boolean suites execute without load errors: `test_geometry.py`
  (32), `test_polygons.py`, `test_union.py` (16), `test_slicing.py` (20),
  `test_rounding.py`, `test_edge_deletion.py` (25).
- **Target (estimate, to be replaced by the measured baseline):** at least half of
  the 137 draw/pull tests and half of the boolean/edit tests pass.
- The Part conformance group does not regress, and `Part.makeBox`/tessellation tests
  stay green.
- No SimpleCAD source change is needed to reach the target.

### S3 — Curved geometry, dress-up and mesh boundary

- **S3.1** `makeCylinder`, `makeSphere`, `makeCone`, `makeTorus`; `Circle`, `Arc`,
  `ArcOfCircle`, `Ellipse`, `LineSegment`, `BSplineCurve`, `BSplineSurface`,
  `Plane`, `Cylinder`, `Sphere`, `Cone`, `Toroid`, `Geom2d`; `isinstance(face.Surface,
  Part.Plane)` must work.
- **S3.2** `makeFillet`, `makeChamfer`, `makeOffsetShape`, `makeOffset2D`,
  `makeThickness`, `makeShell`, solid/shell promotion.
- **S3.3** `makeLoft`, `.loft`, `.sweep`, `makePipeShell`, `makeFilledFace`,
  `ShapeFix` (low priority).
- **S3.4** Mesh boundary: either a minimal `Mesh`/`MeshPart` facade or an explicit
  “mesh suites are out of scope for the proving ground” decision. 136 unit tests are
  at stake; do not let them silently define success.

Acceptance:

- The surface suites (174 tests) and the circle/arc/sphere suites execute; target
  (estimate) at least 70% of the **non-mesh** unit tests pass.
- Fillet/chamfer keep their lineage; the existing `Part::Fillet`/`Part::Chamfer`
  tests and `part_conformance` do not regress.
- The mesh suites remain counted, with a documented decision.

### S4 — Interaction parity

S4.1 **View facade.** Add `FreeCADGui.activeDocument()` and a view object over
`InventorViewport` exposing `getObjectsInfo`, `getObjectInfo`, `getPointOnScreen`,
`getViewDirection`, `getCameraNode`/camera state, `addEventCallbackPivy`,
`removeEventCallbackPivy` and `updateGui`. This is the adapter that lets the
controller run close to unchanged; it is also the reusable substrate for any other
workbench.

S4.2 **Ranked pick.** `getObjectsInfo` returns an ordered list with
`Document/Object/Component/x/y/z`; at minimum, preserve top-hit semantics and expose
the part name. If `SC_Support` priority and occlusion order are required, add a ranked
API rather than pretending the single hit is a list.

S4.3 **Selection object.** `SelectionObject` with `.Object`, `.SubElementNames`,
`.PickedPoints`, `.ObjectName`; writable `addSelection`/`removeSelection`/
`clearSelection`/`setPreselection`/`getPreselection`.

S4.4 **Snapping.** Circle centres, on-edge, world-axis inference, the screen-space
point grid, and the radius magnet. Use `FaceFrame`/`Workplane` for the workplane; use
the S2 query surface for exact face planes where the tessellation is not enough.

S4.5 **Push/pull.** A `PushPull` manipulator on a face selection: normal, opposite
face, screen-space signed distance, capture/release, preview, one transaction.

S4.6 **Previews.** Port `Preview` to transient `ShapeItem`s; add 3-D text to the
stage or move labels to the status bar.

Acceptance:

- `test_pick.py`, `test_selection.py`, `test_center_snapping.py`,
  `test_edge_snapping.py`, `test_face_click_snap.py`, `test_pull_contact.py` pass
  against the view facade (or a documented subset with the failures named).
- A scripted rectangle → push/pull → line → push/pull reaches area 1200 and volume
  24000, the assertions already in `tests/gui/gui_smoke.py`.
- No kernel dependency reaches `ferrocad_stage`, `ferrocad_viewport` or
  `ferrocad_gpui`.

### S5 — GUI workbench

- Implement [`qt-strategy.md`](qt-strategy.md)’s split plane (D) for SimpleCAD: the
  native spatial reference is already `ferrocad_stage`; add the micro-shim for the
  code-built widgets. There is no `.ui` loader work because SimpleCAD has no `.ui`
  files. Where a shim would only paper over behaviour (live previews, `QTreeWidget`,
  `QtTest`), port to the shell or skip explicitly.
- Register `SimpleCADWorkbench`'s 52 commands, five toolbars and menus against the
  FerroCAD actions bar; map `command_state` enablement to the command contract.
- Port the 9 `QDialog` classes to `DialogSpec`; add a modeless live-preview mechanism
  for `PreviewDialog` and the mesh-region Qt event filter.
- Port the status-bar `Controls` (plane selector, snap toggle, value field, sides) to
  shell UI; add key handling for Tab/Enter/Esc/X/Y/Z/arrows.
- Run the 63 GUI suites under Xvfb, in the order `gui_smoke` → `gui_circles` →
  `gui_continuous_line` → `gui_slicing` → `gui_face_browser` (the last exercises
  `QTreeWidget` and the document observer), then all.

Acceptance:

- The five named GUI suites pass under Xvfb; then all 63.
- The 1,914 GUI assertions are not replaced by no-ops; `QtTest` calls have real
  equivalents.
- `gui_mcp` and `test_mcp_protocol` pass; the MCP bridge is unchanged.
- No product code imports `PySide` outside the single `gui/qt.py` seam (or that seam
  is replaced by the chosen strategy).

### 7.1 Wiring `tools/conformance.py`

Today the harness has two implicit groups: the default `App` files under
`src/Mod/Test`, and `--part` with `PART_DIRS`/`PART_FILES`
(`tools/conformance.py:40–68,224–245`). The SimpleCAD group needs a little more:

| Need | Today | S0 change |
| --- | --- | --- |
| A second root | `--root` is the FreeCAD checkout | `--simplecad PATH` (default `../simplecad`) or `--group simplecad` with a `SIMPLECAD_DIR` environment variable |
| Import path | `src/Mod/Part` and `python/` are inserted | also insert `<simplecad>` and `<simplecad>/tests/unit` (tests import `surface_fixtures` and sibling modules) |
| File discovery | fixed stems | `glob("test_*.py")`, sorted; `--list` prints stems |
| Requirements | Part group needs one image | SimpleCAD needs one image with `Part` + `pivy` (+ `numpy`); `Mesh`/`MeshPart` absent is expected |
| Report | same collector | unchanged: `[LOAD ERROR]` names the missing module, `[ERROR]` a runtime gap |
| GUI suites | not supported | **do not add a group**; they are `run()` scripts, not unittest. They belong to the Xvfb job in S5. |

The in-process runner should live beside `part_conformance` — either
`crates/ferrocad_parametric/tests/simplecad_conformance.rs` (the edition already
links `Part` and `ferrocad_gpui`) or a small dedicated harness test. It boots
`ferrocad` + `Part`, installs `pivy`, sets `sys.argv` to
`[conformance.py, --simplecad, <dir>]`, and captures stdout exactly as
`part_conformance.rs:34–79` does. It self-skips without `FERROCAD_SIMPLECAD`.

### 7.2 Where SimpleCAD lives

Like `freecad-upstream`, it is a **sibling checkout outside version control**:

```text
/home/john/projects/ferrocad/
  ferrocad/            the git repo
  freecad-upstream/    sparse FreeCAD checkout (33adf61)
  simplecad/           SimpleCAD checkout (9180c4b, 0.38.0)  <- never committed
```

- Clone: `git clone --depth 1 https://github.com/igsova/SimpleCAD.git simplecad`.
- Pin the revision in every report: `git -C ../simplecad rev-parse --short HEAD` →
  `9180c4b`, version `0.38.0` (2026-10-20).
- Environment: `FERROCAD_SIMPLECAD` (default `<repo>/../simplecad`).
- Never vendor it, never add it to `python/` or `mods/`, never edit it to make a test
  pass. The conformance model requires the upstream files to run unchanged.
- A CI job that needs it must clone at the pinned commit in that job; normal PR CI
  stays hermetic and the harness self-skips.

---

## 8. Draft vs SimpleCAD

| Axis | `src/Mod/Draft` (FreeCAD `33adf61`) | SimpleCAD 0.38.0 (`9180c4b`) | Winner for a first proving ground |
| --- | --- | --- | --- |
| **Qt surface** | 117/244 files mention `PySide`; **74** distinct Qt names (`QtWidgets` 30, `QtGui` 23, `QtCore` 21); **19 `.ui` files**; 11 `loadUi` sites; 32 distinct `.ui` classes (17 plain Qt, 15 `Gui::*`) but only **4** Python Qt subclasses ([`qt-strategy.md`](qt-strategy.md) §2.2–2.3) | 14 files import `PySide` (12 GUI tests + 2 infrastructure); 1 product file (`gui/qt.py`); **58** distinct Qt names; **0 `.ui`**; 11 widget subclasses (9 `QDialog`, 2 `QWidget`) | **SimpleCAD**, decisively |
| **`.ui` files** | **19** (11 `loadUi` sites) | **0** | **SimpleCAD** — there is no `.ui` loader on its path |
| **Part breadth** | **42** names; `LineSegment` 93, `Wire` 87, `OCCError` 58, `Shape`/`Face` 56 | **42** names; `makeBox` 442, `makeCompound` 186, `Face` 183, `makeCylinder` 183 | **Tie on breadth; SimpleCAD on relevance** (whole-solid editing) |
| **Test volume** | 138 test methods in 19 files; 5 GUI files / 38 GUI tests; ~100 headless | **1,066** unit tests in 88 files + 63 GUI suites with 1,914 assertions | **SimpleCAD**, roughly eight times the unit test methods |
| **Interaction model** | 65 `gui_*.py` modules, 7 task panels, `Gui.Snapper` 89 refs, `InputHint` 66, `UserInput` 59, 86 commands; task-panel/toolbar-driven | One `Controller`, Coin event callback, `Snapper`, 25 tool modules, 52 command definitions; canvas-driven | **SimpleCAD** (matches `Tool`/`Snapper`/`Manipulator`) |
| **Other workbenches** | Imports `Part`; 0 `Mesh`; uses `Gui` heavily | `Part` 180 files; 0 `Draft`; 1 `PartDesign` tool; 1 `Surface::Filling`; `Mesh` 35 files | **Draft, narrowly** on dependency cleanliness; SimpleCAD's extras are three tools, not a layer |
| **Upstream churn** | Part of FreeCAD; moves with core releases; Qt5→Qt6 and GUI API are core concerns | Independent addon at 0.38.0; package date 2026-10-20; 20 changelog sections (0.30.0→0.38.0); shallow clone, so cadence is not measurable | **SimpleCAD, narrowly** — smaller independent API, but a fast addon cadence is a maintenance tax |

### 8.1 Recommendation

**Both, in parallel, sharing one `Part` kernel slice — with SimpleCAD as the first
integration target.**

Concretely:

1. **Start S0/S1 now.** They are cheap, they do not touch SimpleCAD, and they turn
   “how much of SimpleCAD runs today” into a number instead of an opinion.
2. **Open the shared `Part` slice (S2/S3) as its own track.** Draft and SimpleCAD
   need the same names: `Face`, `Wire`, `Edge`, `Vertex`, `LineSegment`, `Circle`,
   `Arc`, `makePolygon`, `makeCompound`, `makeLine`, `OCCError`, plus shape queries.
   The analytic half belongs in the `ferrocad_curves` leaf recommended by
   [`part-draft-layering.md`](part-draft-layering.md); the B-Rep half extends
   `ferrocad_geom`/`ferrocad_occt`. This is the critical path for both; doing it
   twice would be the expensive mistake.
3. **SimpleCAD S4/S5 before Draft's GUI.** S4 builds the view facade over
   `InventorViewport`, the ranked pick, the selection object and the push/pull
   manipulator — reusable for any canvas-first workbench. Draft's GUI, by contrast,
   is 117 Qt files and 19 `.ui` files, and it is blocked on the Qt decision.
4. **Draft in parallel only on the headless side.** The five D0 fixes and the
   `--draft` conformance group (`test_base`, `test_grid_settings`, `test_pivy`,
   `test_airfoildat`, `test_import`) can land independently. Do not start Draft's
   GUI port before `qt-strategy.md` is settled.

If forced to choose one order, choose **SimpleCAD first**. It has the same `Part`
gap, a fraction of the Qt surface, roughly ten times the headless tests, and the
interaction model we already built. Draft first would spend the Qt budget before the
kernel is ready.

---

## 9. Open questions and what could not be verified

1. **No tests were run.** The conformance baseline for SimpleCAD does not exist yet;
   S1 creates it. All “target” percentages in §7 are estimates.
2. **Network was unavailable.** SimpleCAD could not be re-cloned, updated, or
   inspected beyond the local `--depth 1` checkout; no dependency was installed.
3. **Release cadence is not measurable.** The SimpleCAD clone has one commit. The
   changelog has 20 sections but no dates, so “releases frequently” is qualitative.
4. **The 83-file pivy/coin count is not reproducible.** Measured: 39 files, 36
   `pivy.coin` imports, 51 `So*` names. The `Part` 42-name count is reproducible once
   `MeshPart.*` is excluded.
5. **The Qt and Part-layering decisions are recorded** in the two companion notes
   (untracked in the working tree at `13552db`, written 2026-10-21): `qt-strategy.md`
   recommends the split plane, and `part-draft-layering.md` recommends the
   `ferrocad_curves` leaf. This note applies both; it does not reopen them.
6. **`Shape.Placement` semantics were not executed.** The recommendation (keep a
   placement on the handle, apply lazily) follows upstream behaviour and SimpleCAD's
   `copy(); Placement = ...` idiom, but the OCCT baking path in `ferrocad_occt::place`
   means the two models must be reconciled explicitly in S2.1.
7. **The bundled runtime’s third-party packages were not verified.** NumPy and SciPy
   are referenced by 14 and 2 unit files (12 import NumPy at module level); the app
   ships a python-build-standalone stdlib, and no packaging file in the tree installs
   NumPy/SciPy. S1 must settle the test image (system interpreter vs bundled runtime)
   before trusting a pass count.
8. **`Part::Feature` dynamic properties were not exercised.** `addProperty` and
   `App::PropertyLink` exist, but `SC_Support`/`SC_*` and `setEditorMode` were not
   run against a FerroCAD document.
9. **FreeCAD’s `ViewObject` behaviour at `GuiUp = 0` was not executed.** The
   `App.GuiUp` guards suggest the document layer is safe; a few GUI helpers are not.
10. **Draft’s “32 widget classes” is an inventory, not a subclass count.** Draft
    loads most panels from `.ui`; [`qt-strategy.md`](qt-strategy.md) counts 32 distinct
    classes across the 19 files and only 4 Python Qt subclasses. The 244 files, 42
    `Part` names, 19 `.ui`, 65 `gui_*.py`, 7 task panels, 138 tests and 86 commands
    were re-measured here.

---

## Appendix A. How the numbers were obtained

Representative commands (all read-only, all run in `../simplecad` unless noted):

| Number | Command |
| --- | --- |
| 283 `.py` | `find . -name '*.py' \| wc -l` |
| 156 files under `tests/` | `find tests -type f \| wc -l` (154 are `.py`) |
| 1,066 unit tests | `grep -rhoE '^[[:space:]]+def test_' tests/unit \| wc -l` |
| 174 `TestCase` classes | `grep -rhoE '^class [A-Za-z0-9_]+\([^)]*TestCase[^)]*\)' tests/unit \| wc -l` |
| 4,186 unit assertions | `grep -rhoE 'self\.assert[A-Za-z]+' tests/unit \| wc -l` |
| 63 GUI suites | `ls tests/gui/gui_*.py \| wc -l`; `grep -rl '^def run(' tests/gui \| wc -l` |
| 42 `Part` names | `grep -rhoE '(^\|[^A-Za-z_])Part\.[A-Za-z_][A-Za-z0-9_]*' --include='*.py' . \| sed -E 's/^[^P]*//' \| LC_ALL=C sort -u \| wc -l` |
| 0 Draft imports | `grep -rn 'import Draft\|from Draft' .` (no output) |
| 1 PartDesign file | `grep -rn 'import PartDesign\|from PartDesign' .` → `surfaces/offset.py:248` |
| 39 pivy/coin files | `grep -rlE 'pivy\|\\bcoin\\b' --include='*.py' . \| wc -l` |
| 15 PySide mentions | `grep -rl 'PySide' --include='*.py' . \| wc -l` |
| Draft comparison | same commands in `../freecad-upstream/src/Mod/Draft` |
