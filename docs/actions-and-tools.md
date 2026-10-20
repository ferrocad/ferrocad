# Actions and tools: the workbench command model

Status: design; A0 (registry) and A1 (actions bar) as built (2026-10-20). Companion to
[`visual-language.md`](visual-language.md) §6
(the status channel), [`axis-origin-grid.md`](axis-origin-grid.md) (the view requests a
command can make), [`gizmo-manipulators.md`](gizmo-manipulators.md) (the interactive
vocabulary) and [`app-shell-vision.md`](app-shell-vision.md) S2 (when this lands).

This note defines how a workbench registers **an action the user can invoke** — create
an object, delete it, transform it, or open a dialog — and how those actions reach a
toolbar, a context menu and a shortcut. It keeps FreeCAD's `Gui.Command` shape so
workbench scripts port, and it keeps the shell naming no workbench.

---

## 1. What existed before this slice

| Piece | State then | Now |
| --- | --- | --- |
| `python/FreeCADGui/__init__.py` | a **console stub**: deliberately no `addCommand`, no `Selection`, no `activeDocument` | ✅ registry + read-only `Selection` (§16) |
| `FreeCAD.GuiUp` | `0`, hardcoded in the facade | ✅ `1` in the app, `0` headless (§8, §16) |
| Command registry in the shell | ▢ none; the console has three hardcoded buttons (`version()`, `objects`, `recompute()`) | ✅ actions bar over the registry (A1) |
| Menu bar / toolbar / shortcuts | ▢ none (S2) | ◐ the actions bar; menus and `Accel` bindings are A4 |
| Workbench entry points | module hooks (`register_display_provider`, `register_interaction_provider`) and `ferrocad_shell.add_namespace` | ✅ plus `FreeCADGui.addCommand` |
| Interactive tools | `ferrocad_stage::Tool` (polyline), `Manipulator`, `TransformGizmo`; contributed in Rust | same |
| Toy's creation | `Toy.makeSlab(...)`, reachable through the console namespace | ✅ `Toy_MakeSlab` command as well |

So the workbench could *draw* and *edit*, but not **offer** anything. This is the
missing third extension point, and it is the one that unlocks toolbars, context menus
and editors.

---

## 2. Two concepts

| | **Command** | **Tool** |
| --- | --- | --- |
| Lifetime | one invocation | a session that owns pointer events until it ends |
| Examples | create, delete, move-by-dialog, toggle grid | polyline, push-pull, fillet drag |
| Where it lives | Python, on the `FreeCADGui` facade | Rust, in the workbench's presentation crate |
| Vocabulary | `Activated` / `IsActive` / `IsChecked` / `GetResources` | `ferrocad_stage::{Tool, Manipulator}`, `TransformGizmo` |
| Reaches the shell | command registry (§4) | a tool id in the command's return value (§7) |

A command is the **entry point**; a tool is what some commands start. The manipulators
and gizmo stay always-on and contextual, as they are today — they are not tools and do
not need a registry entry.

---

## 3. The command contract

Upstream-compatible, with no FerrroCAD-specific arguments:

```python
class MakeSlab:
    """Toy_MakeSlab — create a slab, reversibly."""

    def GetResources(self):
        return {
            "MenuText": "Create Slab",
            "ToolTip": "Add a parametric slab to the document",
            "Accel": "Shift+S",
            "Group": "Toy",          # optional; orders the actions bar
            "CmdType": "ForEdit",    # metadata only (see §9)
        }

    def IsActive(self):
        # `FreeCAD.ActiveDocument` and `FreeCADGui.Selection` are enough.
        return FreeCAD.ActiveDocument is not None

    def IsChecked(self):          # only for checkable commands
        return FreeCAD.GuiUp and _grid_on()

    def Activated(self):
        # May return None, a dialog spec (§6), or a view/tool request (§7).
        return {
            "title": "Create Slab",
            "fields": [
                {"name": "Width",  "label": "Width",  "kind": "quantity", "value": 40.0, "min": 0.1},
                {"name": "Depth",  "label": "Depth",  "kind": "quantity", "value": 40.0, "min": 0.1},
                {"name": "Height", "label": "Height", "kind": "quantity", "value": 20.0, "min": 0.1},
            ],
            "accept": "Create",
            "on_accept": lambda values: Toy.makeSlab(**values),
        }


FreeCADGui.addCommand("Toy_MakeSlab", MakeSlab())
```

### 3.1 Registration

```python
FreeCADGui.addCommand(name, command)   # name: "<Workbench>_<Action>"
FreeCADGui.listCommands()              # -> [name, ...] in registration order
FreeCADGui.runCommand(name, *args)     # -> whatever Activated returned (a dict/None)
```

- A duplicate id raises `ValueError` (a silent overwrite makes workbench collisions
  invisible).
- A command object needs `Activated`; `GetResources`, `IsActive`, `IsChecked` are
  optional with defaults (`{}`, `True`, `False`).
- A workbench registers at import time, like the display hooks.
- **Errors are isolated.** A raising `Activated` returns an error dict the shell shows
  in the status line; a raising `IsActive` disables that command and is reported once
  (stderr + console). One broken command never breaks the bar, the same policy as a
  broken workbench script.

### 3.2 Enablement and checked state

`IsActive()` and `IsChecked()` take **no arguments**, as upstream. They read:

- `FreeCAD.ActiveDocument` (already public),
- `FreeCADGui.Selection` (§5),
- `FreeCAD.GuiUp` (§8).

The shell evaluates them when it builds the action list, not per frame. The list is
rebuilt after a selection change, a document edit or undo/redo, an open/close, and a
viewport commit. A stale list is a visible bug; a per-frame GIL round-trip is a perf
bug. The cache lives in `ShellModel` and carries the resources with it.

### 3.3 Execution and transactions

**A command owns exactly one transaction per user-visible edit**, and must leave no
transaction open when it returns. The shell does not wrap `Activated`, because a
command may open a dialog and return immediately; wrapping it then would commit an
empty or half-finished edit.

```python
def Activated(self):
    doc = FreeCAD.ActiveDocument
    doc.openTransaction("Delete")
    try:
        for obj in FreeCADGui.Selection.getSelection():
            doc.removeObject(obj.Name)
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"status": f"delete failed: {exc}"}
    return {"status": f"deleted {n} object(s)"}
```

A dialog callback owns the transaction when the dialog is accepted (§6), for the same
reason.

---

## 4. The shell bridge

The shell still goes through the app script, never `import Toy`:

```python
# python/ferrocad_shell/__init__.py
def commands() -> str:
    """The registered commands, with their resources and live state, as JSON."""
    return _json(FreeCADGui._command_list())      # [{id, menu, tooltip, accel, group,
                                                  #   enabled, checked, cmd_type}, ...]

def run_command(name: str) -> str:
    """Invoke a command; returns {ok, status?, dialog?, token?, view?, tool?}."""
    return _json(FreeCADGui._run(name))

def dialog_accept(token: str, values_json: str) -> str: ...
def dialog_cancel(token: str) -> str: ...
```

`ferrocad_gpui` gains a thin `commands` module:

```rust
pub fn list() -> Result<Vec<CommandInfo>, String>;   // via call_json("commands")
pub fn run(id: &str) -> Result<CommandOutcome, String>;
```

The shell renders an **actions bar** from `CommandInfo`:

| Field | Use |
| --- | --- |
| `menu` | button label |
| `tooltip` | hover text |
| `accel` | a shortcut (§10) |
| `group` | ordering; absent falls back to registration order |
| `enabled` | greyed when false |
| `checked` | a toggled tint (grid, snap) |
| `cmd_type` | metadata; `ForEdit` implies an undoable edit, `ForView` does not |

The list is rebuilt on the events in §3.2. A command whose `enabled` is false stays
visible — the user should see that the action exists and why it is unavailable (the
tooltip can carry `GetResources()["ToolTip"]`).

---

## 5. Selection

`IsActive` needs a selection without a GUI document. The facade keeps a **snapshot**,
pushed by the shell after every viewport selection (the same place that fills the
inspector today):

```python
FreeCADGui.Selection.getSelection()     # [DocumentObject, ...]
FreeCADGui.Selection.getSelectionEx()   # [(DocumentObject, ["Face6", ...]), ...]
FreeCADGui.Selection.isSelected(obj)    # bool
```

Read-only in the first slice. `addSelection` / `removeSelection` / `clearSelection`
arrive with the multi-selection service (S3); they also make a selection change
observable, which the shell already has from the viewport.

`FreeCADGui.activeDocument()` (a **Gui** document with `getObject(name)` returning a
view object) is deliberately still absent — it is M5 (`ViewProvider`) work. Commands
use `FreeCAD.ActiveDocument` and `Gui.Selection`; the doc records this in the
conformance table (§11).

---

## 6. Dialogs

A command returns a **dialog spec**; the shell hosts it as a glass modal in the same
window (no OS dialog, no Qt). Because `Activated` cannot block on the shell, the
callback is kept by the facade and invoked later:

```text
shell  run_command("Toy_MakeSlab")
  → Activated() -> {title, fields, accept, on_accept}
  → facade stores on_accept under a token "d1"
  → returns {ok: true, dialog: {…}, token: "d1"}
  → shell renders the modal
  → accept: dialog_accept("d1", {Width: 50, Depth: 40, Height: 25})
      → facade calls on_accept(values); the callback owns its transaction
      → returns {ok, status}
  → cancel: dialog_cancel("d1")
```

- **Fields.** `kind` is one of `text`, `int`, `float`, `quantity`, `bool`, `enum`;
  `value` seeds it; `min`/`max` validate numerics; `values` lists an enum. These map
  onto the existing property-editor widgets (`TextInput`, the commit-on-enter path) and
  the visual language's 11/12 px scale.
- **Lifecycle.** One dialog at a time. A second dialog request while one is open is
  rejected with a status message. Closing the document, quitting, or `Esc` cancels and
  drops the token. A token is single-use and expires on cancel.
- **Validation.** The shell validates `min`/`max` and required text before accepting;
  the callback sees typed values (`float`, `int`, `bool`, `str`).
- **No nesting.** A callback may return a new dialog spec; the shell replaces the
  modal. This is how a wizard is built without a second protocol.

---

## 7. View and tool requests

A command that changes presentation rather than the document returns a request in the
same dict. This is what lets a **Python** workbench toggle the grid (G5) or start an
interactive tool without a second hook:

```python
return {"view": {"grid": False}}          # applied by the shell, not undoable
return {"tool": "toy_measure"}            # starts a registered tool session
```

- A `view` request is applied directly by the shell (grid on/off, workplane).
- A `tool` request resolves against the edition's Rust tool registry. The first cut
  ships **no** Python-declared tools: the id maps to a Rust factory contributed like a
  manipulator (`Contributions::with_tool`). Until that exists, `tool` is a documented
  no-op that reports "no such tool".
- Neither request opens a transaction.

---

## 8. GUI mode

The app shell is a GUI, so it sets `FreeCAD.GuiUp = 1` before the app script boots;
`python3` (headless tests, conformance) keeps `0`. `FreeCADGui` stays importable in
both, but `addCommand`/`runCommand` are only meaningful when `GuiUp` is 1 — the same
split upstream has.

```text
run_with / run_edition_with   GuiUp = 1, registry available, shell chrome
python3 / --check-init        GuiUp = 0, registry empty, stub behaviour
```

`check_init` verifies the app script's imports, not the chrome, so it does not need the
registry.

---

## 9. Built-in commands (first cut)

| Id | Action | Notes |
| --- | --- | --- |
| `App_Delete` | remove the selected objects | one transaction; `IsActive` = selection non-empty |
| `App_Move` | move the selection by `(dx, dy, dz)` | dialog; the non-interactive twin of the gizmo |
| `File_New` / `File_Open` / `File_Save` / `File_SaveAs` / `File_Close` | S2 | base app, not a workbench |
| `App_Undo` / `App_Redo` | S2 | `IsActive` from `getAvailableUndos` |
| `Toy_MakeSlab` | create a slab | the toy's `makeSlab`, wrapped as a command |
| `Part_Box` (later) | create a Part box | the edition's script |

`CmdType` is carried as metadata: `ForEdit` commands are undoable document edits,
`ForView` commands are presentation (grid, camera). The first slice only *reports* it;
using it to auto-wrap transactions is deliberately not done (§3.3).

---

## 10. Shortcuts and icons

- `Accel` is a string like `"Shift+S"` or `"Ctrl+Z"`. The first slice parses
  `Ctrl`/`Shift`/`Alt` + one key and builds gpui key bindings; the full upstream
  grammar (`S, H`) is deferred.
- Icons: `GetResources()["Pixmap"]` is accepted and ignored in the first cut (the bar
  is text). A `ferrocad_widgets` icon button plus an asset loader is a later slice; the
  command ids do not change.
- A shortcut fires the same `run_command` path as a click, so enablement isolation and
  transaction rules are identical.

---

## 11. Conformance

| Upstream (`Gui`) | FerroCAD | Notes |
| --- | --- | --- |
| `addCommand(name, cmd)` | ✅ same | duplicate ids raise |
| `runCommand(name, *args)` | ✅ same | returns `Activated()`'s value |
| `listCommands()` | ✅ same | registration order |
| `Command.Activated` | ✅ | may return a dialog/view/tool request |
| `Command.IsActive` | ✅ | no arguments |
| `Command.IsChecked` | ✅ | checkable commands |
| `Command.GetResources` | ✅ | `MenuText`, `ToolTip`, `Accel`, `Group`, `CmdType`; `Pixmap` accepted, ignored |
| `Gui.Selection.getSelection/Ex/isSelected` | ✅ read-only | writes with S3 |
| `Gui.activeDocument()` | ▢ | M5 `ViewProvider` work; documented as absent |
| `Gui.updateGui()` | ✅ no-op | the shell refreshes itself |
| `FreeCAD.GuiUp` | ✅ 1 in the app, 0 headless | §8 |
| Qt dialogs / `.ui` files | ❌ | the shell hosts declarative dialogs (§6) |
| Menus, docks, workbench switcher | ▢ | later; the registry is the input |

---

## 12. Slice plan

| Slice | Deliverable | Acceptance |
| --- | --- | --- |
| **A0 — registry** ✅ | `FreeCADGui.addCommand/runCommand/listCommands`, resource defaults, duplicate/error isolation; `GuiUp` flag plumbing; Python conformance tests | a registered command runs and returns its value; a raising `Activated`/`IsActive` is isolated; duplicates raise |
| **A1 — shell bridge + bar** ✅ | `ferrocad_shell.commands()/run_command()`, `ferrocad_gpui::commands`, an actions bar with enabled/checked, rebuild on selection/doc events; `Toy_MakeSlab` registered | headless toy test: the bar lists the toy command, a run creates the object, undo removes it |
| **A2 — selection + built-ins** | `FreeCADGui.Selection` snapshot (read-only), `App_Delete`, `App_Move` (no dialog yet) | deleting two selected objects is one undo step; `IsActive` flips with the selection |
| **A3 — dialogs** | the spec + token protocol, the glass modal, `text/int/float/quantity/bool/enum` fields, validation, cancel; `Toy_MakeSlab` and `App_Move` use it | create via the dialog; cancel creates nothing and leaves no transaction or token; a bad value is rejected in the shell |
| **A4 — shortcuts + status** | `Accel` bindings, tooltips, `{"status"}` results, the `HoverReport` pass-through line | a shortcut runs the same command as a click; the status bar shows the command's message and the overlay pass-through |
| **A5 — view/tool requests** | `{"view": …}` applied by the shell (grid toggle, workplane); `Contributions::with_tool` + `{"tool": id}` | a checkable grid command toggles G5's grid; an unknown tool id reports cleanly |

A0 and A1 are the vertical slice that makes a workbench able to offer an action; A3 is
what makes it able to ask the user something.

---

## 13. Decisions to settle

1. **Registry in Python.** Recommend `FreeCADGui` (conformance and workbench scripts);
   Rust would force a PyO3 shim with no benefit.
2. **Dialog protocol.** Recommend return-a-spec + token (non-blocking) over a blocking
   `Gui.dialog()` call (impossible synchronously) or a separate OS window.
3. **Command-owned transactions.** Recommend yes; auto-wrapping cannot survive a
   dialog.
4. **Enablement freshness.** Recommend rebuild-on-event (selection, edit, undo, open)
   rather than per frame.
5. **`GuiUp`.** Recommend 1 in the app and 0 headless; note that the existing facade
   hardcodes 0 and the app must set it before the script boots.
6. **Selection scope.** Recommend read-only first; writes with S3's multi-selection.
7. **Icons.** Recommend text first; `Pixmap` accepted and ignored so scripts port.
8. **Tools from Python.** Recommend deferring: tools stay Rust contributions, the
   command only carries a tool id.

---

## 14. Non-goals

- No Qt, PySide or `.ui` files. Dialogs are declarative and shell-hosted.
- No docking, workbench switcher or menu-bar layout engine in this design; the
  registry is the input for those later.
- No Python-defined interactive tools yet (§7).
- No command-driven transaction wrapping (§3.3).
- No persistence of toolbar layout or user shortcuts yet.

---

## 15. Verification

- **Python (A0)** — `tests/test_commands.py`: registration, duplicate ids, resource
  defaults, `runCommand` return, `IsActive`/`IsChecked`, error isolation. ✅
- **Shell (A1–A4)** — headless gpui tests: the bar's entries and states, a run refreshes
  the model, dialog accept/cancel tokens, the status line. The shell tests need the
  `ferrocad` extension built into `python/`, as the existing shell tests do. ✅ for
  `ferrocad_gpui::commands` (the JSON shape); the bar→model→undo path is asserted in
  `crates/ferrocad_toy/tests/commands.rs`, where a workbench is registered.
- **Toy e2e (A1–A3)** — extend
  `the_toy_workbench_creates_represents_manipulates_and_records`: create via
  `run_command`, delete via `App_Delete`, move via the `App_Move` dialog, and undo each.
- **Part e2e (A5, later)** — `Part_Box` through the same registry, proving a kernel
  workbench uses the identical contract.

The demo gate for the whole note: a user opens the app, sees the toy's "Create Slab"
button, clicks it, fills the form, gets an object, then deletes it from the same bar —
with two undo steps and no terminal.

---

## 16. As built (A0 + A1, 2026-10-20)

The vertical slice is in. It is deliberately small: a workbench can **offer** an
action, and the shell can **run** it, without the shell naming the workbench.

### Python

| File | What it does |
| --- | --- |
| `python/FreeCADGui/__init__.py` | the registry (`addCommand`, `removeCommand`, `listCommands`, `runCommand`); `Command` duck-typing (`Activated` required, `GetResources`/`IsActive`/`IsChecked` optional); per-command error isolation with `_report_once`; the private bridge `_command_list()` / `_run(name)`; the read-only `Selection` snapshot |
| `python/ferrocad_shell/__init__.py` | `commands()` and `run_command(name)` — the two functions `ferrocad_gpui::python::call_json` calls |
| `python/ferrocad_toy/__init__.py` | `Toy_MakeSlab` (`MenuText` "Create Slab", `ToolTip`, `Accel` "Shift+S", `Group` "Toy", `CmdType` "ForEdit"); `Activated` returns `{"status": "created a slab"}` |

Two shape decisions fell out of the build:

- **`IsActive` defaults to true; `IsChecked` defaults to false.** That matches upstream's
  intent (a command without the method is always available and not checkable), and it
  keeps a minimal command minimal.
- **The registry is populated in registration order, and the bar renders it in that
  order.** `Group` is carried but not yet used to lay the bar out; ordering by group is
  an A4/S2 concern.

### Rust

| File | What it does |
| --- | --- |
| `ferrocad_gpui/src/commands.rs` | `CommandInfo` (id/menu/tooltip/accel/group/cmd_type/enabled/checked + `label()`) and `CommandOutcome` (`ok`/`error`/`status`/`value`); `list()` and `run(id)` |
| `ferrocad_gpui/src/python.rs` | `GUI: OnceLock<bool>`; `gui()`; `configure(..., gui)`; `FreeCAD.GuiUp = 1` set **before** the app script is imported so import-time workbench code sees it; `commands()` / `run_command(name)` |
| `ferrocad_gpui/src/shell.rs` | `ShellModel.commands`; `refresh_commands` (after a refresh or a selection, never per frame); `run_command` (run → refresh model → re-sync stage → status line → console line); `actions_bar` above the main columns |

The bar is rendered only when the registry is non-empty, so the base app keeps its old
layout. A disabled command **stays visible** (its label in the muted token) — a user
should see that the action exists and why it is unavailable. A checked command carries
the accent background. There is no tooltip: `bite-gpui` 1.21 has no `.tooltip()` API,
so `CommandInfo.tooltip` is stored for A4 and unused today.

### Tests

| Test | Asserts |
| --- | --- |
| `tests/test_commands.py` (7) | register/list/run, resource defaults, duplicate id raises, a command without `Activated` raises, a raising `Activated` is isolated, a raising `IsActive` disables only that command, an unknown id raises |
| `crates/ferrocad_toy/tests/commands.rs` | `Toy_MakeSlab` is listed with its label/group/type and enabled; running it takes the slab count 1 → 2; one undo removes it; an unknown id returns `ok: false` |
| `crates/ferrocad_gpui/src/commands.rs` (3) | the JSON shape and its defaults (`MenuText` wins, absent fields are empty/false) |

### Known gaps

- **Tooltips** are stored, not shown (no `bite-gpui` API).
- **Disabled commands cannot explain themselves.** Upstream's `IsActive` is a bool;
  showing the reason needs a FerrroCAD extension (or the A4 status pass-through).
- **The bar is rebuilt, not diffed.** That is fine at toy scale; a large Part registry
  may want a keyed list (the same retained-vs-rebuild question as the inspector).
- **No default commands yet.** `App_Delete`/`App_Move` are A2; the base app's
  File/Edit menu is S2.
- **`Selection` is read-only and empty** until the shell pushes a snapshot (§5); the
  selection service is S3.
