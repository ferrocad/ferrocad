# Qt reduction — taking Qt out of the paths that do not need it

Status: task (2026-10-23). Q0a, Q0b and Q0d landed 2026-10-23; Q1–Q5 remain.
Backlog entry **P2.3** in
[`TODO-23-10-2026.md`](../TODO-23-10-2026.md). Companion to
[`qt-strategy.md`](../workbenches/qt-strategy.md) (§2–§7 remain the design; this is
the execution plan), [`python-ui-research.md`](../interaction/python-ui-research.md),
[`actions-and-tools.md`](../workbenches/actions-and-tools.md),
[`part-draft-layering.md`](../architecture/part-draft-layering.md) §2.4/§6.4, and
[`d0-draft-import.md`](d0-draft-import.md).

**Verified** = checked in this tree on 2026-10-23 by reading code or running the
stated command. **Recommendation** = proposed, not built. Upstream counts are
attributed to `qt-strategy.md` and upstream `33adf61`, not re-derived here.

## 0. The two questions this answers

### 0.1 Is the Ctrl+K palette related to `ferrocad_parametric` or the Part GUI?

No. **Verified.** The palette lives entirely in
`crates/ferrocad_gpui/src/shell.rs` (`Palette`, `open_palette`, `close_palette`,
`toggle_palette`, `palette_filtered`, `is_palette_shortcut`) and
`crates/ferrocad_gpui/src/chrome.rs` (the dock's `Search (Ctrl+K)` affordance). It
renders gpui elements and filters `crate::commands::CommandInfo` from the command
registry; accepting a row runs the same `run_command` path as a click. Neither file
imports or mentions PySide, Qt, `ferrocad_parametric` or Part, and commit `6179d62`
touched exactly those two files (`chrome.rs` +1/−1, `shell.rs` +536/−14).
`ferrocad_parametric` is the edition binary (`crates/ferrocad_parametric/src/main.rs`)
that links `ferrocad_part_py` and contributes the fillet manipulator — an orthogonal
feature. The palette will serve the Part GUI when it exists, but is not a dependency,
prerequisite or proxy for it.

### 0.2 What Qt surface actually exists?

Not the palette. It is four things: (a) our `PySide` shim, (b) the upstream Draft
GUI scripts, (c) upstream `.ui` loading, and (d) tests/tools. Only upstream Draft
pulls Qt into the headless import path; the FerroCAD facade, the shell and the Part
crates do not. The goal is **not** "add a UI" but "stop requiring a Qt runtime for
paths that only need a translation stub, a resource file and a widget-free import".

## 1. Inventory: where Qt/PySide enters today

### 1.1 (a) Our compatibility shim for upstream scripts

| File | What it does | What it needs | Consumers |
| --- | --- | --- | --- |
| `python/PySide/__init__.py` | Aliases `PySide6.QtCore/QtGui/QtWidgets` (and `QtSvg` when present) into `sys.modules` under the Qt5 names; replaces `QtCore.QFile` in place with a subclass that serves `:/…` from a mod's loose `Resources/`; exposes `QT_TRANSLATE_NOOP` | **A real PySide6 install.** Blocking `PySide6` makes `from PySide import QtCore` raise `ImportError`, and `from draftutils import params` then fails with `ModuleNotFoundError: No module named 'PySide.QtCore'` (**Verified**) | The upstream Draft import chain only |
| `python/Draft_rc.py` | `qInitResources`/`qCleanupResources` no-ops plus `__title__` | Nothing (its docstring says it must not import PySide) (**Verified**) | `Draft.py`, `draftutils/utils.py` when `GuiUp` |
| `python/lazy_loader/` | Vendored 30-line `LazyLoader` (D0.2) | Nothing Qt-related | 25 upstream Draft files |
| `python/FreeCAD`, `python/FreeCADGui`, `python/ferrocad_shell`, `python/FerroCAD`, `python/FerroCADGui` | The facade, command registry, document views and shell bridge | **No PySide.** A fresh process importing all five has no `PySide*`/`shiboken*` in `sys.modules` (**Verified**) | Headless scripts, the shell |

The upstream call sites that force the shim to exist:

| Upstream path | Qt names evaluated at import | Needed by |
| --- | --- | --- |
| `draftutils/params.py` | `import PySide.QtCore as QtCore`; `QtCore.QFile`/`QIODevice.ReadOnly`/`QFile.Text`/`readAll().data()` at :736; parsing is `xml.etree` | `PARAM_DICT` (160 `Mod/Draft` entries, `textheight == 3.5`) (**Verified**) |
| `draftutils/translate.py` | `PySide.QtCore`, `PySide.QtWidgets`, `QCoreApplication.translate`, `QApplication.UnicodeUTF8` in a `try/except AttributeError` | every translated Draft module |
| `draftobjects/*` | `from PySide.QtCore import QT_TRANSLATE_NOOP` — 25 files in this checkout (**Verified**; the strategy cites 94 files/552 uses across all of `src/Mod/Draft`) | object property tooltips, headless included |
| `Draft.py` → `draftviewproviders/view_base.py` | `PySide.QtCore`, `PySide.QtGui`, `QT_TRANSLATE_NOOP` at module scope | `import Draft`, if it ever runs |

So the shim does two jobs: a genuine Qt bridge (the alias and the inheriting `QFile`)
and a name stub (`QT_TRANSLATE_NOOP`). Only the first needs Qt; the headless path
needs the second plus loose-file reads.

### 1.2 (b) Upstream Draft GUI modules (not re-measured here)

Measured in `qt-strategy.md` §2.1–§2.2 at upstream `33adf61`: **117 of 244** `.py`
files import PySide, **74** distinct Qt names, **4** Qt subclasses
(`DraftBaseWidget`/`DraftLineEdit` in `DraftGui.py`, `_FieldLockFilter` in
`draftguitools/gui_field_locks.py`, `Layers_Delegate` in `draftguitools/gui_layers.py`),
**0** `paintEvent`, **0** `QGraphics*`/`QOpenGL*`. The Qt-heavy packages are
`draftguitools/` (58 of 66), `draftviewproviders/` (11 of 23), `drafttaskpanels/`
(7 of 7) and the root `DraftGui.py`/`DraftTools.py`/`InitGui.py`. This is what Q3–Q5
would host; it is **not** on the headless path and is not the first priority.

### 1.3 (c) `.ui` loading

- 19 files under `src/Mod/Draft/Resources/ui`: 9 `preferences-*.ui` and 10 forms —
  `TaskPanel_{OrthoArray,PolarArray,CircularArray}.ui`, `TaskShapeString.ui`,
  `TaskSelectPlane.ui`, `TaskPanel_SetStyle.ui`, `dialogHatch.ui`, `dialogLayers.ui`,
  `dialog_AnnotationStyleEditor.ui`, `preferences-dxf-import.ui` (**Verified**).
- 11 `Gui.PySideUic.loadUi` sites (**verified**; breakdown in `qt-strategy.md` §2.1):
  the six `.ui`-backed panels, `drafttaskpanels/__init__.py`, and four dialogs.
- One non-form use matters headlessly: `draftutils/params.py` reads the nine
  preference `.ui` files through `QtCore.QFile` to build `PARAM_DICT`, so a missing Qt
  makes `import draftutils.params` fail even with `GuiUp == 0`. A pure-Python `QFile`
  over loose files removes that without touching upstream. `task_scale.py` has no
  `.ui`; it composes `QWidget`/`QGridLayout` in code and is Q5's one conversion.

### 1.4 (d) Tooling and tests

| Path | What it is | Qt need |
| --- | --- | --- |
| `tools/inventory.py` | M3a inventory: parses upstream `.pyi` stubs into an API model, groups by App/Base/Gui/Mod/PySide. `classify_area` recognises `src/Tools/typing/overlays/PySide`. It is **not** a runtime Qt-import auditor | None |
| `tools/test_inventory.py` | Hermetic tests for that parser (temp `.pyi` tree) | None |
| `tests/test_draft_import_shims.py` | Covers `lazy_loader`, the `PySide` alias, `Draft_rc`, `freecad.deprecation`. `PySideAliasTests` asserts `PySide.QtCore is PySide6.QtCore` and exercises the resource `QFile` | **PySide6 installed**; it will need rewriting with the pure shim |
| `tests/test_grid_overlay_coin.py`, `tests/test_grid_overlay_fork.py` | Load a mod by path and assert it imported no Qt/Coin; `fork` asserts the module's import set excludes `PySide`, `PySide2`, `PySide6`, `pivy`, `coin` | None — and they are the precedent for the guard in §5 |
| `tools/conformance.py --draft` | Runs the headless Draft test subset against upstream | The `PySide` shim, because `draftutils` imports it |

The honest summary: **there is no dedicated Qt-import auditor today.** The
`qt-strategy.md` counts were a one-off grep/AST pass; `tools/inventory.py` audits the
API surface, not imports. Q0 produces the missing artifact rather than pretending one
exists.

### 1.5 The headless Part path — verified Qt-free

**Verified** by fresh-process imports and source/manifest inspection:

- `import FreeCAD` and `import FreeCADGui` leave no `PySide*`/`shiboken*` module
  loaded; `GuiUp == 0`. `ferrocad_shell`, `FerroCAD` and `FerroCADGui` are the same.
- `python/ferrocad_parametric/__init__.py` imports `FreeCAD`, `FerroCAD`, `Part` and
  `ferrocad_shell` only — no PySide.
- `crates/ferrocad_part_py`, `ferrocad_part` and `ferrocad_part_gui` are Rust only;
  `cargo tree -p ferrocad_part_py` has no Qt/Shiboken edge, and `tests/part_boot.rs`
  imports `Part` and `ferrocad` without naming PySide. `import Part` is unavailable in
  the standalone dev build (`tests/test_part.py` skips); in the edition it is linked.

So the headless Part path imports nothing Qt; the only headless path that does is
upstream Draft through our `PySide` shim — "reduce Qt" means fixing that shim.

## 2. Definition of done

"Qt dependency reduced" is met when all of these hold and a test checks them.

| id | Milestone | Measurable check |
| --- | --- | --- |
| **D1** | `import FreeCAD` and `import FreeCADGui` never touch PySide | In a subprocess where `PySide6`/`PySide2`/`shiboken6` are blocked at the meta-path, both imports succeed and `sys.modules` has no `PySide*`/`shiboken*` |
| **D2** | `import Part` (edition image) likewise | `part_boot` runs the same assertion after importing `Part` and `ferrocad` |
| **D3** | Headless Draft imports without Qt | With Qt blocked, `from draftutils import params` succeeds, `textheight == 3.5`, `PARAM_DICT["Mod/Draft"]` has 160 entries, and the `--draft` group does not regress |
| **D4** | The shim is a **subset**, not an alias | `PySide` exports exactly the allowlist = import-time names (from §1.1) ∪ the measured panel contract in `qt-strategy.md` §2.4; any other name raises with file/widget/member context; zero silent `None`s |
| **D5** | Declarative `.ui` rendering needs no Qt runtime | The six core forms load through `ferrocad_ui`/`PySideUic.loadUi` and render in `#[gpui::test]` with `PySide6` uninstalled; preference `.ui` files are still *parsed* (for values), not rendered |
| **D6** | No Qt in headless CI | The `headless` job runs the suite on a runner with no PySide6 wheel; the D1–D4 guards are part of it |
| **D7** | The inventory cannot drift silently | A pinned contract file (19 files, 32 classes, 495 widgets, 72 contract pairs) is checked against the upstream revision; a changed count fails with the delta |

D1/D2 hold *today* by code inspection and import probes, so they need only the guard.
D3 fails today (verified). D5–D7 do not exist yet.

## 3. Plan Q0–Q5

`qt-strategy.md` §5 remains the design. The order below changes the emphasis: Q0 is
split so the smallest Qt removal lands first, before any loader work.

| Slice | Status (2026-10-23) | Work |
| --- | --- | --- |
| **Q0a** | Landed 2026-10-23 | `python/PySide/__init__.py` is now a pure-Python shim; real Qt is opt-in through `FERROCAD_PYSIDE_BACKEND=PySide6|PySide2`. Unknown names raise. |
| **Q0b** | Landed 2026-10-23 | `:/…` reads through plain-Python `QFile`/`QIODevice` over `<mod>/Resources/`; no `PySide6` inheritance. |
| **Q0c** | Landed (D0 + parameter work) | `lazy_loader`, `Draft_rc`, `Quantity(value, Unit)`, `ParameterGrp.GetStrings`, `src/Ext` on the path |
| **Q0d** | Landed 2026-10-23 | `tools/draft_contract.py` + `contracts/draft-forms.json`: 19 files, 32 classes, 495 widgets and 72 contract pairs pinned; `--check` reports a delta. |
| **Q1** | Not started | `ferrocad_ui`: parse all 19 `.ui` files into a form tree (widgets, layouts, properties, spacers, custom-widget registry, combo items); typed errors with source position; no rendering |
| **Q2** | Not started | `ferrocad_ui_py`: `FreeCADGui.PySideUic.loadUi`, `FormProxy`/`WidgetProxy`/`SignalProxy`, the six-operation layout model, error isolation; panels instantiate and `accept()` headlessly |
| **Q3** | Not started | Render the six core forms on `ferrocad_widgets`/gpui; async modal behind `FreeCADGui.Control.showDialog/closeDialog` |
| **Q4** | Not started | `FreeCADGui.Snapper`/`draftToolBar` over `ferrocad_stage`; `PlaneGui` rerouted; line/wire tools; snap bar as checkable commands |
| **Q5** | Not started | SetStyle, Hatch, AnnotationStyleEditor, `TaskPanel_Scale.ui`, `Gui::FileChooser`; the Layers dialog and preference pages stay deferred |

**Landed 2026-10-23.** Q0a/Q0b/Q0d and the D1–D3 guard are in:
`python/PySide/_qt_stub.py`, the rewritten `python/PySide/__init__.py`,
`tools/draft_contract.py`, `contracts/draft-forms.json` and
`tests/test_qt_isolation.py`. A fresh process with `PySide6`/`shiboken*` blocked now
imports `FreeCAD`, `FreeCADGui` and `from draftutils import params` (160 `Mod/Draft`
entries), and the `--draft` group is unchanged at 10 passed / 1 failed / 1 load error.
Q1–Q5 are the remaining slices.

**Recommendation — priority order.** Q0a + Q0b + the §5 guard first: they are small,
remove the actual runtime dependency, and unlock D2/D3/D6 without a loader. Then Q0d
(the contract) and Q1/Q2 in parallel, both testable with `python3` only. Q3 starts
when the Draft GUI work starts; Q4/Q5 follow the native input and remaining-form
demand. Nothing in Q3–Q5 is needed for `import Part` or the shell.

**What to remove when Q0 lands.** The automatic `_install_aliases()` path in
`python/PySide/__init__.py` (a real Qt import becomes an experiment, never the
default); the inheriting `ResourceQFile` class (replaced by the pure-Python reader);
the identity assertions in `test_draft_import_shims.py::PySideAliasTests` that require
`PySide6`; and any CI step or documented prerequisite that installs PySide6 for the
headless job. (The workflow installs none today — **verified** — but the test suite
implicitly requires it, which is the bug.)

## 4. Non-goals and what will still need a shim

**Non-goals (do not promise these):** arbitrary PySide/PySide6 addons or a general
`PySide` replacement; executing `QWidget` subclasses, custom delegates, event filters
or `QObject` parenting/ownership semantics (the four measured subclasses get explicit
local handling, not a base-class emulator); `QPainter`, `QGraphics*`, `QOpenGL*`,
`QApplication.exec_()` or a second event loop; `.ui` rendering beyond the supported
subset (no `<connections>`, dynamic `QTabWidget`, `QTreeView` model/view or preference
pages); Qt in the shell, palette, Part crates or mods; `QtSvg` as a default
requirement (icons stay native/SVG assets).

**What will still need a shim after Q0–Q5:** upstream Draft scripts are shipped as
source, so their `import PySide...` lines remain. The permanent shim is the name
subset in D4 (translation, timers, locale, loose-file `QFile`, and the panel
contract's methods/signals). Panels outside the supported set, preference pages,
Layers, Coin trackers and the nine advanced snap modes keep failing loudly until they
are ported natively or scheduled separately.

## 5. Tests and CI: keeping the shim honest

**Recommendation — a headless import guard.** Add `tests/test_no_qt_imports.py`
that runs each probe in a subprocess with a meta-path blocker installed first:

```python
import importlib.abc, sys

class BlockQt(importlib.abc.MetaPathFinder):
    def find_spec(self, name, path=None, target=None):
        if name.split(".")[0] in {"PySide", "PySide2", "PySide6", "shiboken", "shiboken6"}:
            raise ModuleNotFoundError(name)
sys.meta_path.insert(0, BlockQt())
import FreeCAD, FreeCADGui, ferrocad_shell          # must succeed
assert not [m for m in sys.modules if m.startswith(("PySide6", "shiboken"))]
```

- Allowlist: `PySide` (our shim) and its `Qt*` submodules are allowed **only** for the
  headless Draft probes; `PySide6`/`PySide2`/`shiboken*` never are.
- Rewrite `tests/test_draft_import_shims.py` so `params` is imported with Qt blocked
  and the resource reader is exercised through the pure-Python `QFile`.
- Reuse the grid-overlay precedent: `tests/test_grid_overlay_fork.py` already asserts
  a mod's import set excludes Qt/Coin.

**Tooling checks.** `tools/inventory.py`/`tools/test_inventory.py` stay as the `.pyi`
surface audit. **Recommendation:** add `tools/qt_inventory.py` (or a `--qt` mode) that
scans repo and upstream for `PySide|PyQt|QtCore|QtGui|QtWidgets`, emits file → names
JSON, and compares it with the allowlist — that is the missing Qt-specific auditor;
do not describe the current tool as one. The Q0d contract JSON is the second alarm:
D7 fails on any count change, including a new Qt name in a supported panel. Keep the
shim honest structurally by generating its public-name list from the contract and
testing that every exported name is allowlisted and every unsupported member raises
with the four-part message (file, widget class, widget name, member); a silent `None`
is a test failure. CI: keep the `headless` job Qt-free (the `geometry` job's
`part_boot` test adds D2); do not add a PySide6 install to either.
