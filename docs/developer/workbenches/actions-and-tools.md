# Actions and tools: the workbench command model

Status: design; A0–A5 as built (2026-10-20). Companion to
[`visual-language.md`](../interaction/visual-language.md) §6
(the status channel), [`axis-origin-grid.md`](../interaction/axis-origin-grid.md) (the view requests a
command can make), [`gizmo-manipulators.md`](../interaction/gizmo-manipulators.md) (the interactive
vocabulary) and [`app-shell-vision.md`](../../project/direction/app-shell-vision.md) S2 (when this lands).

This note defines how a workbench registers **an action the user can invoke** — create
an object, delete it, transform it, or open a dialog — and how those actions reach a
toolbar, a context menu and a shortcut. It keeps FreeCAD's `Gui.Command` shape so
workbench scripts port, and it keeps the shell naming no workbench.

---

## 1. What existed before this slice

| Piece | State then | Now |
| --- | --- | --- |
| `python/FreeCADGui/__init__.py` | a **console stub**: deliberately no `addCommand`, no `Selection`, no `activeDocument` | ✅ registry + `Selection` snapshot (§16) |
| `FreeCAD.GuiUp` | `0`, hardcoded in the facade | ✅ `1` in the app, `0` headless (§8, §16) |
| Command registry in the shell | ▢ none; the console has three hardcoded buttons (`version()`, `objects`, `recompute()`) | ✅ header controls, popovers and dock over the registry (A1, P0.6) |
| Multi-selection | ▢ the inspector held one name; nothing pushed it to Python | ✅ click / Ctrl·Shift-click, pushed to the facade (A2) |
| Built-in commands | ▢ none | ✅ `App_Delete`, `App_Move`, `App_Rotate` (A2) |
| Menu bar / toolbar / shortcuts | ▢ none (S2) | ◐ header controls, action dock and the title-bar app menu; `Accel` bindings are A4 |
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
            "Group": "Toy",          # optional; sort metadata only
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
  (stderr + console). One broken command never breaks the chrome, the same policy as a
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
    entries = FreeCADGui._command_list()
    # The bridge adds the FerroCAD_* keys from GetResources so the shell can place
    # each control. _command_list() itself stays upstream-shaped.
    for entry in entries:
        resources = FreeCADGui._resources(FreeCADGui._COMMANDS[entry["id"]])
        entry["surface"] = str(resources.get("FerroCAD_Surface") or "")
        entry["parent"] = str(resources.get("FerroCAD_Parent") or "")
        entry["short"] = str(resources.get("FerroCAD_Short") or "")
        entry["checkable"] = bool(resources.get("Checkable"))
    return _json(entries)                             # [{id, menu, tooltip, accel, group,
                                                      #   enabled, checked, checkable,
                                                      #   cmd_type, surface, parent, short}, ...]

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

The shell renders the **chrome** from `CommandInfo` (P0.6):

| Field | Use |
| --- | --- |
| `menu` | button label |
| `tooltip` | hover text |
| `accel` | a shortcut (§10) |
| `group` | metadata; no longer decides placement |
| `enabled` | greyed when false (the dock keeps the row) |
| `checked` | a toggled tint (grid, snap) |
| `checkable` | a popover row that toggles: the menu stays open after the click |
| `cmd_type` | metadata; `ForEdit` implies an undoable edit, `ForView` does not |
| `surface` | `grid` / `axis` / `workplane` / `snap` / `mods` / `dock` / `hidden` — the control that owns it; absent means `dock` |
| `parent` | the surface's primary command id, for a row in that control's popover |
| `short` | a compact popover label when `menu` repeats the control's name |

The list is rebuilt on the events in §3.2. A command whose `enabled` is false stays
visible — the user should see that the action exists and why it is unavailable (the
tooltip can carry `GetResources()["ToolTip"]`). Placement is `FerroCAD_Surface`-driven, never
derived from `Group`; a command with no `FerroCAD_Surface` lands on the dock so it cannot
disappear.

A **state** row runs through `Shell::run_command_in_place`, which restores the open
menu afterwards: several options can be set in one visit and the markers follow the
live state. Both a command toggle (checkbox) and a generated provider choice (radio)
are state rows, so the `Snap` checklist and the `Grid`/`Axes` backend switches behave
the same way. An **action** row (a settings jump, a reload) runs through
`Shell::run_command` and dismisses the menu. Closing a state menu is therefore
explicit — an outside click, `Esc`, or the caret. The app menu is a launcher, not a
control popover, so its one row still closes.

---

## 5. Selection

`IsActive` needs a selection without a GUI document. The shell owns the selection
and pushes a **snapshot** to the facade after every change — a click in the
inspector, a pick in the viewport, a delete, or an undo:

```python
FreeCADGui.Selection.getSelection()     # [DocumentObject, ...]
FreeCADGui.Selection.getSelectionEx()   # [(DocumentObject, ["Face6", ...]), ...]
FreeCADGui.Selection.isSelected(obj)    # bool
```

The snapshot is **read-only**: `addSelection` / `removeSelection` /
`clearSelection` are still absent, because selection changes originate in the
shell (the inspector and the viewport) and a script cannot yet drive them. The
push is one JSON call, `ferrocad_shell.set_selection(payload)`, where the payload
is `[{"object": name, "elements": [...]}, ...]`.

A selection is an ordered list; the **last** item is the primary (what the
property editor shows). A plain click replaces the list; Ctrl/Cmd- or Shift-click
toggles one object in or out. A viewport pick replaces the list with one object
plus the sub-elements it hit, so `getSelectionEx` carries `Face6`. Names the
document no longer has are pruned on every refresh, so a deleted object cannot
keep a command enabled.

`FreeCADGui.activeDocument()` (a **Gui** document with `getObject(name)` returning a
view object) is deliberately still absent — it is M5 (`ViewProvider`) work. Commands
use `FreeCAD.ActiveDocument` and `Gui.Selection`; the doc records this in the
conformance table (§11).

---

## 6. Dialogs

A command returns a **dialog spec**; the shell hosts it in the same window (no OS
dialog, no Qt) — a centred glass modal by default, or a **floating dielectric HUD**
when the spec asks to keep the canvas usable. Because `Activated` cannot block on
the shell, the callback is kept by the facade and invoked later:

```text
shell  run_command("Toy_MakeSlab")
  → Activated() -> {title, fields, accept, on_accept}
  → facade stores on_accept under a token "d1"
  → returns {ok: true, dialog: {…}, token: "d1"}
  → shell renders the dialog
  → accept: dialog_accept("d1", {Width: 50, Depth: 40, Height: 25})
      → facade calls on_accept(values); the callback owns its transaction
      → returns {ok, status}
  → cancel: dialog_cancel("d1")
```

- **Fields.** `kind` is one of `text`, `int`, `float`, `quantity`, `bool`, `enum`;
  `value` seeds it; `min`/`max` validate numerics; `values` lists an enum. These map
  onto the existing property-editor widgets (`TextInput`, the commit-on-enter path) and
  the visual language's 11/12 px scale.
- **Presentation.** `presentation` is `"modal"` (the default) or `"float"`. A float is
  a compact one-row HUD with **no backdrop**, placed beside the gizmo (falling back to
  the last pointer, then a comfortable default), so the scene stays bright and the
  pointer keeps orbiting, selecting and dragging. Only the panel itself occludes the
  pointer. Its first field takes focus and is selected; `Tab`/`Shift+Tab` cycle the
  fields. Use it for geometric verbs the user is looking at.
- **Live preview.** `preview: "translation"` drives a temporary transform from the
  `dx`/`dy`/`dz` fields on the current selection while the dialog is open. The document
  is untouched: no transaction, no undo step, and cancelling drops the ghost. Text that
  does not parse yet previews as zero rather than flashing a validation error. The
  binding is **two-way**: a gizmo drag ticks `ViewportEvent::TransformPreview`, which
  mirrors the accumulated delta back into the fields and previews the whole selection;
  the drag never commits on its own, so only Accept applies the pending move.
- **Anchoring.** The gizmo (and therefore the HUD) belongs to the picked point of
  contact: `SelectionEvent::Picked` carries a `SelectionAnchor` (world point + surface
  normal) from the viewport's G-buffer pick. A non-spatial selection (tree or command)
  falls back to the body's bounding-box base corner, never the interior centroid.
- **Following the object.** A live preview offsets the overlays with the selected
  geometry: the triad and every manipulator with a world anchor (the toy height grip)
  move by the same delta, and clearing restores their pre-preview pose. The triad stays
  glued even during its own drag — the interaction frame is frozen at the grab point and
  only the drawing moves — then the frame commits to the new pose on release. After a
  committed edit the shell **re-arms** the current selection against the new stage, so a
  grip sits on the moved top face and the triad survives its own commit. Any other
  interaction commit supersedes a pending translation HUD: the dialog closes and its
  token is spent, rather than applying a stale delta on top of a height change.
- **Lifecycle.** One dialog at a time. A second dialog request while one is open is
  rejected with a status message. Closing the document, quitting, or `Esc` cancels and
  drops the token. A token is single-use and expires on cancel.
- **Validation.** The shell validates `min`/`max` and required text before accepting;
  the callback sees typed values (`float`, `int`, `bool`, `str`).
- **No nesting.** A callback may return a new dialog spec; the shell replaces the
  modal. This is how a wizard is built without a second protocol.

### 6.1 As built (A3, 2026-10-20)

- **Shape.** `_run` splits a returned dict into a top-level `status`/`value` and a
  `dialog` object with `title`, `fields`, `accept`, `cancel`, `presentation` and
  `preview`; `on_accept` never crosses the JSON boundary. The outcome is
  `{ok, dialog, token, status?}`.
- **Fields.** `text`, `int`, `float`, `quantity`, `bool`, `enum`. A quantity is
  parsed by `ferrocad_types::Quantity` (so `25 mm` and `1 in` work) and sent to the
  callback as canonical millimetres. `min`/`max` apply to the numeric kinds;
  `required` applies to `text`. An unknown kind is a validation error, not a crash.
- **UI.** A centred glass panel over a dimmed, mouse-blocking backdrop, with
  per-kind controls (text inputs, an on/off toggle, `‹ ›` for an enum), an inline
  error line, and the accept/cancel buttons. `Enter` in a field or on the panel
  accepts; `Esc` cancels. A `"float"` spec instead paints a **one-row HUD over the
  undimmed canvas** (no backdrop, `WHEEL_GLASS` + `DOCK_BORDER`, axis-coloured labels),
  anchored beside the gizmo or last pointer; the canvas stays interactive. A disabled
  command stays visible in the chrome; the dialog only exists while a token is open.
- **Lifecycle.** One dialog at a time; a second request is rejected and its token
  is cancelled immediately. Opening a document (`bootstrap`) calls `_cancel_all`,
  so a callback from the previous document cannot be accepted. A token is popped
  on accept **and** on cancel; a callback that raises spends the token and its
  error stays in the modal.
- **Transactions.** The callback owns the transaction (`makeSlab` opens one; the
  move does too), so an accepted dialog is exactly one undo step. The shell never
  wraps the callback.
- **Wizards.** A callback that returns another spec opens a new token; the shell
  replaces the modal rather than stacking it.
- **Anchoring and two-way sync (2026-10-25).** The viewport reports the click's world
  point in `SelectionEvent::Picked { reference, anchor }`; the shell re-anchors the
  gizmo there (a tree selection uses `contributions::body_corner`). While a
  `preview: "translation"` dialog is open, the shell sets the viewport's **deferred**
  gizmo mode: drag ticks arrive as `ViewportEvent::TransformPreview { phase, total }`,
  the shell adds the `Begin` baseline and writes the result into the Δ inputs (with
  `TextInput::set_text`, which does not re-emit a change event), and only Accept runs
  the callback. `Esc` cancels from the viewport too, because the HUD does not retain
  focus once the canvas is clicked.

### 6.2 Fields and validation

| Kind | Control | Value sent to Python | Validation |
| --- | --- | --- | --- |
| `text` | `TextInput` | `str` | non-empty when `required` |
| `int` | `TextInput` | `int` | parses as an integer; `min`/`max` |
| `float` | `TextInput` | `float` | parses as a float; `min`/`max` |
| `quantity` | `TextInput` | `float` (canonical mm) | `Quantity::parse` (units allowed); `min`/`max` |
| `bool` | on/off chip | `bool` | none |
| `enum` | `‹ value ›` | `str` (the selected option) | the index is in range |

The seeds come from `value`: a number is shown as its JSON spelling, an enum's
string picks its option, and a bool is the chip's initial state. A missing seed is
empty (a numeric field then fails validation until filled).

---

## 7. View and tool requests

> **As built (2026-10-22).** The flat actions bar that this section and §16 described is
gone. Commands now live on the floating viewport pill (grid/axes/workplane/mods), their
anchored popovers, and the selection-driven action dock; every command is also listed in
the Dev Tools window, and app-level commands (Dev Tools first) are in the title-bar `☰`
menu and the status bar's bug button. See
[`../../project/record/tasks/toolbar-chrome.md`](../../project/record/tasks/toolbar-chrome.md) §10.
The **request shapes** below are unchanged — only where the buttons live.

A command that changes presentation rather than the document returns a request in the
same dict. This is what lets a **Python** workbench toggle the grid (G5) or start an
interactive tool without a second hook:

```python
return {"view": {"grid": False}}                       # applied by the shell, not undoable
return {"view": {"axis_cross": False}}                 # the world triad, independent of the grid
return {"view": {"grid_minor": 1.0, "grid_major": 10.0}}  # grid preferences (G5)
return {"view": {"workplane": "activate"}}              # G5: the hovered face becomes the UCS
return {"tool": "toy_measure"}                         # starts a registered tool session
```

- A `view` request is applied by the shell and mirrored to `FerroCADGui.ViewState`:
  `{"grid": true}` flips the reference-grid flag, `{"axis_cross": false}` hides the world
  triad (they are independent — the grid is placement, the cross is orientation), and
  `{"projection": "ortho"}` switches the viewport's projection (framing preserved);
  `grid_minor`/`grid_major` (mm) / `grid_radius` (mm, 0 = camera-derived) change the world
  grid's steps and extent. An unknown projection, a non-positive step or a major below
  the minor is rejected and **nothing** is applied (`ViewState::apply` validates before it
  mutates).
- A `workplane` request is an **action**, not state: `"activate"` (the face under the
  pointer), `"clear"` or `"view"` (look down the active normal). The shell performs it
  on the viewport and reports the outcome; without a viewport it reports
  `view: no viewport`.
- A `tool` request resolves through `Contributions::with_tool(id, factory)`, the same
  edition-level registry as the manipulators. The tool owns pointer events and draws
  transient geometry; the shell clears the selection's manipulators/gizmo while it
  runs. An unknown id reports `no such tool <id>` and starts nothing. Ending a tool is
  not modelled yet (the `Tool` trait has no stop/commit channel).
- Neither request opens a transaction. `App_ToggleGrid` is the built-in example, and
  the toy contributes `Toy_Polyline` → `toy_polyline` (`PolylineTool`).

  ### 7.1 The native Draft tool session (Q4)

  The Draft tools do not go through `Contributions`: a Python workbench cannot yet
  contribute a Rust `Tool` factory, and the native layer does not need one. Instead
  `FreeCADGui.Snapper` owns the pending point request and `FreeCADGui.draft` exposes the
  three driver calls a shell uses:

  ```python
  FreeCADGui.draft.pick(world_point)   # snap, then invoke the pending callback once
  FreeCADGui.draft.escape()            # drop it and run the request's on_cancel
  FreeCADGui.draft.finish()            # run on_finish (Enter), without picking
  ```

  `Draft_Line`/`Draft_Wire` register through the normal command registry, start a
  `Creator` session on `Activated`, and commit with `FreeCADGui.doCommand`. The six
  `Draft_Snap_*` modes and the `Draft_Snap_Lock` switch are ordinary checkable commands,
  but they declare `FerroCAD_Surface: "snap"`, so the shell renders them as the `Snap`
  split button beside `Grid`/`Axes`/`WP` rather than as a bottom bar. The modes name the
  switch as their `FerroCAD_Parent` (the caret popover) and carry a `FerroCAD_Short`
  label, so the checklist reads `Grid`, `Endpoint`, … without repeating `Snap`. The full
  surface, including `PlaneGui` and the four Q5 forms, is in
  [`draft-tools.md`](draft-tools.md).

  **Built (2026-10-27).** A real click is routed: `Creator.Activated` asks the shell
  for `python:draft`, and the shell starts a `ferrocad_stage.Tool` bridge whose
  `on_event` calls `draft.pick` with the resolved snap point. `Esc` calls
  `draft.escape` and `Enter` calls `draft.finish` through
  `ferrocad_shell.tool_command`. The commands are registered by importing
  `FerroCADGui.tools`.

  ### 7.2 The in-situ radial menu

  Object verbs are **contextual**, not ambient: they do not belong in the bottom
  strip. A command declares `FerroCAD_Surface: "radial"` (and optionally
  `FerroCAD_Tone: "additive" | "destructive"`), which removes it from the dock and
  makes it a wedge in the wheel. The built-ins today are `App_Move` and
  `App_Rotate` (structural) and `App_Delete` (destructive).

  | Gesture | Effect |
  | --- | --- |
  | Right-press | resolves the **context target** at the pointer (no selection change) and opens **armed** there |
  | Move while armed | highlights the wedge under the pointer (`HIGHLIGHT_TINT` + the tone arc) |
  | Release over a wedge (flick) | runs it and closes |
  | Quick release near the press (`< 200 ms`, `< 12 px`) | **latches**: a trackpad tap leaves the wheel open instead of flashing closed |
  | Move while latched | highlights without a button held (the overlay owns the hover) |
  | Primary click on a wedge while latched | runs it and closes; deadzone/outside cancels |
  | Long hold released in the deadzone | cancels |
  | `Space` | opens **latched** at the last pointer (or the window centre); over a latched wedge it commits; with no wheel it closes |
  | `Esc`, selection change | closes |

  Both triggers are **contextual**, not blind chords: at trigger time the shell
  re-runs the viewport's pick at the pointer (the presenter's G-buffer first — the
  same path a click uses), so a press that lands without an intervening move cannot
  act on a stale hover. **Opening the wheel never changes the selection.** The pick is
  a `WheelTarget`: if it is outside the selection it is promoted only when a verb is
  chosen; if it is already selected the whole selection is used; over void the
  selection's verbs open at the pointer, and with neither the wheel falls back to the
  **creation** verbs from the dock (`Toy_MakeSlab`, `Toy_Polyline`), so the same gesture
  is never a dead end. The full interaction state machine is
  [`selection-gizmos-and-forms.md`](../interaction/selection-gizmos-and-forms.md).

  The wheel is painted by `ferrocad_widgets::wheel` (`slot_at` is pure geometry).
  `ferrocad_widgets::wheel::WheelMenu` is the stateful **sidecar**: it owns the
  `Armed`/`Latched` phases, the hover, the tap-vs-flick timing, dismissal and its own
  anchored overlay (wrapped in `occlude`, so a click cannot also select the scene),
  and it reports `WheelEvent::{Selected, Closed}`. Because the overlay claims the
  pointer, the sidecar also carries its own hover surface. The shell only **triggers**
  it — `open_radial` (armed), `open_radial_latched` (keyboard), and forwarding pointer
  move/release — and maps a selected id to `commit_wheel`, which promotes the context
  target if it was not selected and then runs the command.
  Ordering is by tone (structural, additive, destructive), so the destructive slot is
  always **south** (`N: Move`, `S: Delete` with the two built-ins) and muscle memory
  does not move when a new verb lands.

  The bottom dock therefore contains only **creation + global** commands
  (`Toy_MakeSlab`, `Toy_Polyline`, `Search (Ctrl+K)`), so it never reflows when the
  selection changes. The Dock's `Move` split and its gizmo-options popover are gone;
  the gizmo is armed by selecting, and its settings remain reachable from
  `Settings/Gizmo` and Dev Tools.

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
| `App_Delete` ✅ | remove the selected objects | one transaction; `IsActive` = selection non-empty |
| `App_Move` ✅ | move the selection by `(dx, dy, dz)` | **floating** HUD beside the gizmo with a live translation preview; the canvas stays interactive, and `runCommand("App_Move", dx, dy, dz)` applies the same code without the dialog. Returns `"gizmo": "translate"` so the HUD and the arrows agree |
| `App_Rotate` ✅ | switch the transform gizmo to its rotation rings | a mode-only verb (no form, no transaction): returns `"gizmo": "rotate"`; the ring drag commits the same `transform` interaction a Move drag does. `Ctrl+Shift+R` |
| `File_New` / `File_Open` / `File_Save` / `File_SaveAs` / `File_Close` | S2 | base app, not a workbench |
| `App_Undo` / `App_Redo` | S2 | `IsActive` from `getAvailableUndos` |
| `Toy_MakeSlab` | create a slab | the toy's `makeSlab`, wrapped as a command (asks for Width/Depth/Height) |
| `Toy_Polyline` | start the polyline tool | `ForView`; returns `{"tool": "toy_polyline"}` |
| `App_ToggleGrid` ✅ | show or hide the reference grid | `ForView`, checkable; `IsChecked` reads the mirrored `ViewState` |
| `App_ToggleAxisCross` ✅ | show or hide the world triad | `ForView`, checkable, **independent of the grid**; on by default (upstream's `Std_ToggleAxisCross`) |
| `App_ReloadGridSettings` ✅ | apply the grid step/extent preferences | `ForView`; reads `User parameter:BaseApp/Preferences/View` (`GridMinorStep`, `GridMajorStep`, `GridRadius`, 0 = auto) |
| `App_ActivateWorkplane` ✅ | make the face under the pointer the workplane | `ForView`; returns `{"view": {"workplane": "activate"}}` |
| `App_ClearWorkplane` ✅ | return to the world ground plane | `ForView` |
| `App_ViewWorkplane` ✅ | look down the active workplane's normal | `ForView`; the camera snaps to the plane's `V`-up frame |
| `Part_Box` (later) | create a Part box | the edition's script |

`CmdType` is carried as metadata: `ForEdit` commands are undoable document edits,
`ForView` commands are presentation (grid, camera). The first slice only *reports* it;
using it to auto-wrap transactions is deliberately not done (§3.3).

---

## 10. Shortcuts, tooltips and icons

- `Accel` is a string like `"Ctrl+Shift+S"` or `"Del"`. `ferrocad_gpui::shortcuts`
  parses that subset (modifiers joined with `+`/`-`, then one key; `Del`/`Esc`/
  `Return` and `F1`–`F24` are mapped to gpui names) and matches it against the key
  events that bubble to the shell root.
- **Only safe global chords fire.** A printable key with no Ctrl/Alt/Cmd would shadow
  typing in the console and the property editor, so it is parsed but not bound
  (`Shift+S` today; upstream's multi-key grammar `S, H` is deferred too). A shortcut
  fires the same `run_command` path as a click, so enablement isolation and
  transaction rules are identical; a disabled command does not fire.
- **Tooltips** are the command's `GetResources()["ToolTip"]` plus its `Accel`, shown
  through gpui's tooltip builder on the control that runs it (header, popover row,
  dock row or Dev Tools row; bite-gpui 1.23 has the API).
- **Status.** A command's `{"status": …}` result replaces the status line, and the
  shell adds the neutral pass-through report beside it (`clicked through: navicube →
  Slab`) when the pointer is over an overlay's interior.
- Icons: `GetResources()["Pixmap"]` is accepted and ignored; the controls are text
  until the widgets/icons slice.

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
| **A1 — shell bridge + placement** ✅ | `ferrocad_shell.commands()/run_command()`, `ferrocad_gpui::commands`, a surface-placed command list with enabled/checked (the A1 flat bar was replaced by the P0.6 header/dock), rebuild on selection/doc events; `Toy_MakeSlab` registered | headless toy test: the command list surfaces the toy command, a run creates the object, undo removes it |
| **A2 — selection + built-ins** ✅ | `FreeCADGui.Selection` snapshot pushed by the shell, multi-selection in the inspector (Ctrl/Cmd·Shift toggle), `App_Delete`, `App_Move` (delta via `runCommand`; the dialog is A3), `App_Rotate` (session gizmo mode) | deleting two selected objects is one undo step; `IsActive` flips with the selection; Rotate returns `gizmo: rotate` |
| **A3 — dialogs** ✅ | the spec + token protocol, the glass modal **and the floating HUD** (`presentation`, `preview`), `text/int/float/quantity/bool/enum` fields, validation, cancel; `Toy_MakeSlab` and `App_Move` use it | create via the dialog; cancel creates nothing and leaves no transaction or token; a bad value is rejected in the shell; `App_Move` floats beside the gizmo and previews its delta |
| **A4 — shortcuts + status** ✅ | `Accel` parsing and window-root bindings, tooltips (ToolTip + Accel), `{"status"}` results, the `HoverReport` pass-through line | a shortcut runs the same command as a click; the status bar shows the command's message and the overlay pass-through |
| **A5 — view/tool requests** ✅ | `{"view": …}` applied by the shell (grid state + projection), `Contributions::with_tool` + `{"tool": id}` | a checkable grid command toggles the grid state and its check mark; an unknown tool id reports cleanly |

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
- **Shell (A1–A4)** — headless gpui tests: the chrome's entries and states (header
  controls, popover rows, dock rows and the Dev Tools list), a run refreshes
  the model, a multi-selection is pushed and flips `IsActive`, a dialog validates,
  accepts once, renders, and cancels on `Esc`, and a registered `Accel` runs its
  command. ✅ `crates/ferrocad_gpui/src/shell.rs` has
  `a_multi_selection_is_pushed_and_enables_the_built_ins`,
  `a_command_dialog_validates_accepts_and_cancels`,
  `the_command_dialog_renders_and_escape_cancels`, `a_shortcut_runs_the_command` and
  `the_pass_through_line_names_the_overlay_and_target`; the chrome→model→undo path is
  asserted in `crates/ferrocad_toy/tests/commands.rs`.
- **Viewport (A4)** — `navicube_negative_space_reaches_the_scene` also asserts the
  `HoverReport` names the overlay a click passes through. ✅
- **View/tools (A5)** — `tests/test_view_tools.py`: the defaults, the checkable grid
  loop, and the `tool`/`view` payload; `crates/ferrocad_gpui/src/shell.rs` has
  `a_view_request_toggles_the_grid_and_its_check_mark` and
  `a_tool_request_resolves_by_id` (known and unknown ids). ✅
- **Dialogs (A3)** — `tests/test_dialogs.py`: the token protocol, single use, cancel,
  wizards, callback isolation, `_cancel_all`. ✅
- **Toy e2e (A1–A3)** — `crates/ferrocad_toy/tests/commands.rs` creates through the
  dialog and undoes; `crates/ferrocad_toy/tests/selection.rs` pushes a selection,
  deletes two slabs in one undo step, and moves by a delta in one undo step. ✅
- **Part e2e (A5, later)** — `Part_Box` through the same registry, proving a kernel
  workbench uses the identical contract.

The demo gate for the whole note: a user opens the app, sees the toy's "Create Slab"
row in the action dock, clicks it, fills the form, gets an object, then deletes it
from the same dock — with two undo steps and no terminal.

---

## 16. As built (A0–A5, 2026-10-20)

The vertical slice is in: a workbench can **offer** an action, the shell can **run**
it from the chrome (the floating viewport pill, a popover row, the dock, the Dev Tools
list or the title-bar app menu) or a shortcut, a selection reaches both the UI and the
command predicates, a command can **ask** the user something, change presentation, or
start a tool, and the status line explains both the result and what a click would pass
through — without the shell naming the workbench.

### Python

| File | What it does |
| --- | --- |
| `python/FreeCADGui/__init__.py` | the registry (`addCommand`, `removeCommand`, `listCommands`, `runCommand`); `Command` duck-typing (`Activated` required, `GetResources`/`IsActive`/`IsChecked` optional); per-command error isolation with `_report_once`; the private bridge `_command_list()` / `_run(name)`; the **token registry** `_accept`/`_cancel`/`_cancel_all`; the read-only `Selection` snapshot |
| `python/ferrocad_shell/__init__.py` | `commands()` and `run_command(name)` — the two functions `ferrocad_gpui::python::call_json` calls; `set_selection(payload)` (the snapshot push); `set_view_state(payload)` (the view mirror); `dialog_accept(payload)` / `dialog_cancel(token)`; the built-ins `App_Delete`, `App_Move`, `App_Rotate` and the checkable `App_ToggleGrid` |
| `python/ferrocad_toy/__init__.py` | `Toy_MakeSlab` (`MenuText` "Create Slab", `ToolTip`, `Accel` "Ctrl+Shift+S", `Group` "Toy", `CmdType` "ForEdit") returns a three-quantity dialog whose `_make_from_dialog` callback creates the slab; `Toy_Polyline` returns `{"tool": "toy_polyline"}` |

Two shape decisions fell out of the build:

- **`IsActive` defaults to true; `IsChecked` defaults to false.** That matches upstream's
  intent (a command without the method is always available and not checkable), and it
  keeps a minimal command minimal.
- **The registry is populated in registration order; the chrome does not reorder it.**
  `Group` is carried for sorting metadata only. Placement is decided by
  `FerroCAD_Surface`/`FerroCAD_Parent` (§4): an unannotated command lands on the dock,
  and a header surface's primary runs from the control's label.

### Rust

| File | What it does |
| --- | --- |
| `ferrocad_gpui/src/commands.rs` | `CommandInfo` (id/menu/tooltip/accel/group/cmd_type/surface/parent/enabled/checked + `label()`, `surface()`, `parent()`) and `CommandOutcome` (`ok`/`error`/`status`/`value`/`dialog`/`token`/`view`/`tool`/`select`); `Surface` + `group_by_surface`/`popover_commands`; `list()`, `run(id)`, `accept(token, values)`, `cancel(token)` |
| `ferrocad_gpui/src/dialog.rs` | `DialogSpec`/`DialogField`/`Kind`/`ValueState` and the pure `collect()` validation (typed values, `min`/`max`, `required`, quantity parsing, unknown-kind rejection) |
| `ferrocad_gpui/src/view.rs` | `ViewRequest` (`grid`, `projection`), `ViewState` (grid + `ProjectionName`) and `apply()` — validation-first, returning a one-line summary |
| `ferrocad_gpui/src/selection.rs` | `SelectionItem` (`object` + `elements`, the last item is primary); `push()` — the one call that makes the UI selection visible to Python |
| `ferrocad_gpui/src/shortcuts.rs` | `Accelerator::parse` (the documented `Accel` subset), `is_global` (chords/named keys only) and `matches` (exact modifiers); the shell binds the globals to a window-root key handler |
| `ferrocad_gpui/src/contributions.rs` | the manipulator/gizmo factories plus `ToolFactory` and `with_tool(id, factory)` / `tool_for(id)` — the edition-level tool registry |
| `ferrocad_gpui/src/python.rs` | `GUI: OnceLock<bool>`; `gui()`; `configure(..., gui)`; `FreeCAD.GuiUp = 1` set **before** the app script is imported so import-time workbench code sees it; `commands()` / `run_command(name)` / `set_selection(items)` / `dialog_accept(token, values)` / `dialog_cancel(token)` |
| `ferrocad_gpui/src/shell.rs` | `ShellModel.commands`, `.selection` and `.view`; `refresh_commands` (rebuilds the shortcut list too); `run_command` (Dev Tools / dialog / view / tool / select / run → refresh → stage → status → console); `replace_selection` / `toggle_select` / `clear_selection`; the dialog state machine (`open_dialog_state`, `accept_dialog`, `finish_dialog`, `cancel_dialog`, `toggle_flag`, `cycle_choice`); `apply_view` (state + mirror + projection + re-list); `start_tool` (registry → viewport, pending until a viewport exists); `handle_key` (both windows); `toggle_dev_tools`/`open_dev_tools`; `dialog_overlay`; `settings_card_view`/`open_settings_card`/`commit_property`; `dev_tools_body`; `hover_line` and the Dev Tools bug button in the status bar; `start_scrub`/`drag_scrub`/`end_scrub` for the direct property handle and its scale overlay |
| `ferrocad_gpui/src/chrome.rs` | the P0.6 primitives: `viewport_header` (grid/axes/workplane/snap/mods), `split_button`, the anchored `popover` **adapter** over `ferrocad_widgets::menu`, `action_dock`, and the Dev Tools panes (`providers_panel`, `gpu_panel`, `command_list_panel`) — the panes carry no header of their own; the accordion supplies it |
| `ferrocad_widgets/src/menu.rs` | the domain-free menu panel: `MenuInfoRow`/`MenuRow`, the status dot, additive hover, dividers and outside-press dismissal; the caller supplies `on_row`/`on_dismiss` and owns the command semantics |
| `ferrocad_widgets/src/wheel.rs` | the radial marking menu: `WheelSlot`/`WheelTone`, pure `slot_at`/`slot_offset`, the custom-painted glass torus, hover tint, tone arc and labels, and the `WheelMenu` sidecar that owns the gesture and its overlay (`WheelEvent::{Selected, Closed}`) |
| `ferrocad_widgets/src/accordion.rs` | the vertical accordion: `AccordionSection` (title, optional summary, preferred height), `AccordionState` (any number of open sections, plus whether the user took the choice over), and the pure `resolve_layout`/`preferred_height` that decide between `All` and a shared-height `Expanded` set. Used by the Dev Tools window; `window_chrome::content_height` reports the height a framed window really leaves for it |
| `ferrocad_widgets/src/scroll.rs` | the scrollbar gpui does not paint: `scroll_area` (a tracked `overflow_y_scroll` container plus an overlay thumb) and `scroll_thumb` for views that own their container. Pure `thumb_bounds` sizes and places the thumb; the thumb is painted by a `canvas` so it follows the live offset without re-rendering, and it can be dragged (a track press jumps) |
| `ferrocad_widgets/src/icon.rs` | painted pictograms. `bug` draws the literal insect (oval body, head, six legs, antennae) with paths, so no font has to carry the glyph; `active` inks it with the hover accent. The element is presentational — the status bar's `chrome::dev_tools_button` owns its hit target, hover and tooltip |
| `ferrocad_widgets/src/color.rs` | colour rows for the property editor: `parse_hex`/`normalize_hex`, `swatch` (shows the stored colour) and `palette` (a fixed neutral/accent grid the host shows as an anchored overlay). `kind == "color"` is what selects it, so a mod's schema gets the picker without new UI code |
| `ferrocad_widgets/src/slider.rs` | the range slider's geometry: `fraction`/`value_at` (log when `min > 0`, linear otherwise), `format_value` for the committed editor string, and `trigger` (the mini track in the row that opens the overlay). The host selects it only for a row that declares both `min` and `max` |
| `ferrocad_widgets/src/scrub.rs` | the unbounded numeric control: `value_for` maps vertical travel to a value by a ratio (`48 px` doubles it; zero scrubs linearly), `Scrub` owns the starting value and travel, and `Scale`/`Tick` are the pure overlay model (a log window above zero, linear around zero and negatives, labelled ticks). `handle` paints the draggable double arrow and `scale` the overlay with its live readout and marker. Used for a quantity without a declared range; the host starts the gesture on the handle press, forwards moves from its window root and commits `Scrub::text` with the row's unit |
| `ferrocad_widgets/src/form.rs` | the form controls: `toggle` (a pill switch for a `bool`), `select_box` (the closed `enum`) and `choice_list` (the dropdown body the host anchors under it). Both the property pane and the command dialog use them, so `true`/`false` is never typed and an option is never spelled by hand. The pane draws document-object rows and `Settings/*` rows with the same code, so an object's own `App::PropertyBool`/`App::PropertyEnumeration` gets them too |
| `ferrocad_gpui/src/dev_tools.rs` | the `DevTools` second-window view over `Entity<Shell>` and its window options |
| `ferrocad_viewport/src/inventor_view.rs` | `HoverReport { target, through }` — updated on every pointer move from the same pick and overlay tests the click uses; `hover_report()` |

The commands are dispatched from the chrome (the floating viewport pill, popovers, dock)
or the Dev Tools list — there is no flat strip. Because `App_Delete`/`App_Move`/`App_Rotate` are registered
by the base app script, **every** edition has dock rows; they are greyed until
something is selected. A disabled command stays visible (its label in the muted token)
— the user should see that the action exists. A checked command carries the accent.
`FerroCAD_Surface`/`FerroCAD_Parent` (FerroCAD extensions to `GetResources`) decide placement; a command
without them is on the dock, never hidden.

The selection is an ordered list. A plain click replaces it, Ctrl/Cmd- or
Shift-click toggles one object, and a viewport pick replaces it with the object plus
the sub-elements it hit. The last item is the primary — the property editor's
subject, which is why a multi-selection still edits its last object. `refresh`
prunes names the document no longer has (a delete, or an undo of a creation) and
pushes the pruned set, so `IsActive` cannot latch onto a dead object.

### Tests

| Test | Asserts |
| --- | --- |
| `tests/test_commands.py` (7) | register/list/run, resource defaults, duplicate id raises, a command without `Activated` raises, a raising `Activated` is isolated, a raising `IsActive` disables only that command, an unknown id raises |
| `tests/test_selection.py` (8) | the snapshot resolves objects and `Face6` sub-elements; a deleted name drops out; `IsActive` flips with the snapshot; two selected objects delete in one undo step; `App_Move` translates every selected object and undoes as one step; `App_Rotate` reports `gizmo: rotate` and changes no geometry |
| `tests/test_dialogs.py` (6) | a spec becomes a token with no callable in the JSON; accept runs once and spends the token; cancel drops it; a callback can return a new dialog; a raising callback is isolated; `_cancel_all` clears everything |
| `tests/test_view_tools.py` (4) | the view-state defaults; the checkable grid command closes the loop through the mirror; `tool` and `view` survive the payload; a status and a view can coexist |
| `crates/ferrocad_toy/tests/commands.rs` | `Toy_MakeSlab` is listed with its label/group/type and enabled; running it returns a dialog and creates nothing; accepting creates the slab; the token is spent; one undo removes it |
| `crates/ferrocad_toy/tests/selection.rs` | the shell bridge's push flips `App_Delete`/`App_Move` enabled; two slabs delete in one undo step; the move dialog opens and can be cancelled; a delta path moves and undoes as one step |
| `ferrocad_gpui` unit tests | the JSON shapes (`CommandInfo`, `SelectionItem`, `DialogSpec`); `dialog` validation (types, ranges, required text, unknown kinds); the `Accel` parser and matcher; `view` apply/validation; tool-registry routing; `a_multi_selection_is_pushed_and_enables_the_built_ins`; `a_command_dialog_validates_accepts_and_cancels`; `the_command_dialog_renders_and_escape_cancels`; `a_shortcut_runs_the_command`; `the_pass_through_line_names_the_overlay_and_target`; `a_view_request_toggles_the_grid_and_its_check_mark`; `a_tool_request_resolves_by_id` |
| `ferrocad_gpui` P0.5/P0.6 tests | `the_main_window_carries_only_the_viewport_and_status`; `the_dev_tools_window_lists_commands_with_enablement`; `toggling_dev_tools_preserves_the_model`; `the_dev_tools_command_has_a_headless_status`; `every_registered_command_lands_on_exactly_one_surface`; `the_grid_split_button_runs_the_label_and_the_caret_opens`; `the_grid_popover_reports_the_active_provider_and_spacing`; `view_grid_settings_selects_the_settings_node`; `the_action_dock_follows_the_selection`; `the_app_menu_opens_dev_tools` |
| `ferrocad_viewport` | `navicube_negative_space_reaches_the_scene` also asserts the hover report names the navicube as the overlay a click passes through |

### Known gaps

- **Tooltips show, but only as text.** No icon, no shortcut styling beyond the `·`
  separator; a richer tooltip is a widget concern.
- **Disabled commands cannot explain themselves.** Upstream's `IsActive` is a bool;
  the tooltip still shows the static `ToolTip`.
- **The chrome is rebuilt, not diffed.** That is fine at toy scale; a large Part
  registry may want a keyed list (the same retained-vs-rebuild question as the
  inspector).
- **Only safe global chords fire.** A bare letter or `Shift`-letter `Accel` is parsed
  but not bound, because it would shadow typing; upstream's `S, H` sequences are
  deferred. The dialog owns the keyboard entirely while open.
- **Pass-through is reported for the navicube only.** The gizmo and manipulator
  interiors also fall through, but they have no `contains` query yet; the report's
  `through` is `None` there (the target is suppressed when a handle claims the
  pointer).
- **Grid appearance is still being tuned.** The grid is drawn (G2) and G5 owns the
  step/extent preferences; `App_ToggleGrid` drives whichever provider draws it (P0).
  What remains in [`axis-origin-grid.md`](../interaction/axis-origin-grid.md) is tuning
  the visual tokens, not wiring the flag.
- **Tools have no commit channel.** `Tool` reacts to pointer events and draws
  transients; a polyline cannot yet be committed to the document. The tool strip's
  Cancel (and `Esc`) ends the session and discards it without writing.
- **Only `grid` and `projection` are view requests.** Workplanes/UCS are not modelled
  yet (G-track); an unknown key in a `view` dict is ignored rather than rejected, so a
  newer workbench can send it to an older shell safely.
- **A view request does not touch the document.** There is no transaction, no undo
  entry, and no dirty flag — by design.
- **`App_Move`'s dialog is fixed.** The three delta fields are the whole spec; a
  richer move (axis picker, snapping) is a later command, not a change to this one.
- **The modal does not scroll.** A wizard with many fields needs a scrolling body
  (S6's pane work); the current panel is sized for a handful.
- **No icons or pixmaps.** `GetResources()["Pixmap"]` is still accepted and ignored;
  buttons are text until the widget/icons slice.
- **A document close has no command yet.** `bootstrap` clears dialog tokens, which is
  the safe default; the File menu (S2) should also call `_cancel_all`.
- **Selection changes cannot be driven from Python.** `FreeCADGui.Selection` is a
  snapshot and `addSelection`/`clearSelection` are absent; a script that wants to
  change the selection must wait for that API (S3), because the shell owns the state.
- **The viewport pick is single.** Ctrl-click in the inspector builds a
  multi-selection; extending one from the 3D view needs the pick to carry modifiers
  and is left for S3.
