# Commands

Status: guide (2026-10-22).

A command is a Python object registered with `FreeCADGui`. It is the entry point
for a user action; it may edit the document, return a dialog description, or
return a presentation request.

## The object

Only `Activated` is required. The other three methods are optional.

```python
import FreeCAD
import FreeCADGui
import FerroCADGui

class MakeThing:
    def GetResources(self):
        return {
            "MenuText": "Make Thing",
            "ToolTip": "Add a thing to the document",
            "Accel": "Ctrl+Shift+T",
            "Group": "MyMod",
            "CmdType": "ForEdit",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def IsChecked(self):          # only for a checkable command
        return FreeCAD.GuiUp and FerroCADGui.ViewState.grid

    def Activated(self):
        return {"status": "not implemented yet"}


FreeCADGui.addCommand("MyMod_MakeThing", MakeThing())
```

Register at import time from `InitGui.py` ([`mod-anatomy.md`](mod-anatomy.md)).
The conventional id is `"<Workbench>_<Action>"`.

| API | Behaviour |
| --- | --- |
| `FreeCADGui.addCommand(name, command)` | adds the command; a duplicate id raises `ValueError`; a missing callable `Activated` raises `TypeError` |
| `FreeCADGui.removeCommand(name)` | drops the command; a no-op when absent |
| `FreeCADGui.listCommands()` | the ids in registration order |
| `FreeCADGui.runCommand(name, *args)` | calls `Activated(*args)` and returns the raw Python result; an unknown id raises `KeyError` |

`runCommand` forwards arguments, as upstream does. The shell's JSON path calls
`Activated()` with no arguments.

## `GetResources`

The shell reads these keys when it builds the viewport header and the action dock:

| Key | Used as | Default |
| --- | --- | --- |
| `MenuText` | button label | the command id |
| `ToolTip` | hover text | `""` |
| `Accel` | shortcut string, for example `"Ctrl+Shift+S"` | `""` |
| `Group` | ordering label | `""` (registration order) |
| `CmdType` | metadata: `"ForEdit"` or `"ForView"` | `""` |
| `Checkable` | the row is a toggle: its popover stays open after the click so the status dot follows | `False` |
| `FerroCAD_Surface` | where the action lives: `grid`, `workplane`, `axis`, `snap`, `radial`, `mods`, `dock`, `hidden` | `dock` |
| `FerroCAD_Parent` | the surface's primary command, for a row inside its popover | `""` |
| `FerroCAD_Short` | a compact label for that popover row (`Grid` for `Snap Grid`) | `MenuText` |
| `FerroCAD_Tone` | a `radial` slot's semantic class: `structural`, `additive` or `destructive` | `structural` |
| `Pixmap` | accepted and ignored; the controls are text today | — |

`FerroCAD_Surface`, `FerroCAD_Parent`, `FerroCAD_Short` and `FerroCAD_Tone` are
**FerroCAD extensions**. Leave them out and the command lands on the dock, which is
always safe;
upstream FreeCAD ignores unknown resource keys. A command with
`FerroCAD_Surface: "grid"` and no `FerroCAD_Parent` is the primary of that header
control (its label runs it); a command with `FerroCAD_Parent: "App_ToggleGrid"`
appears in the popover under it. `hidden` keeps a command out of the header and the
dock; the Dev Tools command list and the app menu can still offer it. `radial` is
special: the command is not a dock row at all — it becomes a wedge in the in-situ
wheel, ordered by `FerroCAD_Tone` (structural, additive, destructive), so the
destructive slot is always last (south).

A popover row keeps the menu open when it represents **state**: a command declares
`Checkable: True` or defines `IsChecked`, and the generated provider rows are state
rows too. Several filters — or a backend choice — can then be set in one visit and
the status dots update in place. An **action** row (a settings jump, a reload) still
dismisses the menu, and the app menu (a launcher, not a control popover) closes on
its one row. The command decision uses the *presence* of the flag or hook, never
`IsChecked() == True`, so an unchecked toggle and a non-toggle row are told apart.

`CmdType` is metadata only. The shell does not wrap `Activated` in a transaction
because a command may return a dialog and act later; the command owns its
transaction ([`../workbenches/actions-and-tools.md`](../workbenches/actions-and-tools.md) §3.3).

A raising `GetResources` is treated as `{}` and reported once. The command stays
registered.

## `IsActive` and `IsChecked`

Both take no arguments and are evaluated when the shell rebuilds the action
list, not per frame.

| Method | Absent | Raises |
| --- | --- | --- |
| `IsActive` | treated as `True` | the command is disabled and the error is reported once |
| `IsChecked` | treated as `False` | treated as `False` and reported once |

They read:

- `FreeCAD.ActiveDocument` — may be `None`;
- `FreeCADGui.Selection` — the shell's read-only snapshot;
- `FerroCADGui.ViewState` — the shell's presentation mirror (`grid`,
  `axis_cross`, `projection`, `grid_minor`, `grid_major`, `grid_radius`).

A disabled command remains visible in the bar, so the user can see that it
exists.

## `Activated` return shapes

The shell calls `Activated()` through `FreeCADGui._run(name)`, which never
raises. The result is shaped as follows:

| `Activated` returns | Shell payload |
| --- | --- |
| `None` | `{"ok": true}` |
| a non-dict value | `{"ok": true, "value": repr(value)}` |
| `{"status": "..."}` | `{"ok": true, "status": "..."}` |
| `{"value": ...}` | `{"ok": true, "value": ...}` |
| `{"view": {...}}` | `{"ok": true, "view": {...}}` |
| `{"tool": "id"}` | `{"ok": true, "tool": "id"}` |
| a dialog spec (below) | `{"ok": true, "dialog": {...}, "token": "dN"}` |
| a raising `Activated` | `{"ok": false, "error": "Type: message"}` |

A dict may combine `status`, `value`, `view` and `tool`; the shell handles each.
Keys it does not know are ignored until a later slice gives them meaning.

```python
import json
import ferrocad_shell

result = json.loads(ferrocad_shell.run_command("App_ToggleGrid"))
print(result["view"]["grid"])
```

`ferrocad_shell.run_command` serialises `FreeCADGui._run`; `FreeCADGui.runCommand`
returns the raw Python dict instead.

## Dialogs

A dialog is **data plus a token**, not a callback on the wire. The command
returns `on_accept`; the facade keeps the callable and sends the shell a
description.

```python
def apply(values):
    return {"status": "got %r" % (values,)}

class Ask:
    def GetResources(self):
        return {"MenuText": "Ask"}

    def Activated(self):
        return {
            "title": "Ask a number",
            "accept": "Apply",
            "fields": [
                {"name": "Count", "label": "Count", "kind": "int",
                 "value": 1, "min": 1, "max": 9},
            ],
            "on_accept": apply,
        }
```

The shell receives:

```json
{
  "ok": true,
  "dialog": {
    "title": "Ask a number",
    "accept": "Apply",
    "fields": [
      {"name": "Count", "label": "Count", "kind": "int",
       "value": 1, "min": 1, "max": 9}
    ]
  },
  "token": "d1"
}
```

`on_accept` never crosses the JSON boundary. Only `title`, `fields`, `accept`
and `cancel` are copied into `dialog`.

### Field kinds

| `kind` | Control | Value sent to the callback | Validation |
| --- | --- | --- | --- |
| `text` | text input | `str` | non-empty when `required` |
| `int` | text input | `int` | integer; `min`/`max` |
| `float` | text input | `float` | float; `min`/`max` |
| `quantity` | text input | `float` in canonical millimetres | unit-aware parse; `min`/`max` |
| `bool` | on/off chip | `bool` | none |
| `enum` | previous/next choice | `str` (the selected option) | the index is in range; `values` lists the options |

The `value` key seeds the field. `min`/`max` apply to numeric kinds. The shell
validates before accepting; an invalid value stays in the modal.

### Accept and cancel

| Action | Shell bridge | Facade (private) |
| --- | --- | --- |
| Accept with typed values | `ferrocad_shell.dialog_accept('{"token": "d1", "values": {...}}')` | `FreeCADGui._accept(token, values)` |
| Cancel | `ferrocad_shell.dialog_cancel("d1")` | `FreeCADGui._cancel(token)` |

```python
import json
import ferrocad_shell

run = json.loads(ferrocad_shell.run_command("View_GridSettings"))
accepted = json.loads(ferrocad_shell.dialog_accept(json.dumps({
    "token": run["token"],
    "values": {"enabled": True, "color": "#00ff00"},
})))
print(accepted["ok"], accepted["view"]["grid_color"])
```

Output:

```text
True #00ff00
```

Lifecycle rules:

- A token is single-use. Accept pops it before calling the callback; a second
  accept returns `{"ok": false, "error": "no open dialog 'dN'"}`.
- Cancel drops the token without running the callback.
- A callback that raises spends the token; the error comes back as
  `{"ok": false, "error": "..."}`.
- The shell hosts one dialog at a time. In the app, a second request while one
  is open is rejected and its token is cancelled.
- `bootstrap()` calls `FreeCADGui._cancel_all()`, so a dialog opened against the
  previous sample document is dropped when a new one is created.
- The callback owns its transaction. An accepted dialog should be exactly one
  undo step.
- A callback may return another dialog spec; the shell replaces the modal. That
  is the wizard mechanism.

## View and tool requests

These are presentation-only and never open a transaction.

```python
return {"view": {"grid": True}}
return {"view": {"axis_cross": False}}
return {"view": {"projection": "ortho"}}
return {"view": {"grid_minor": 1.0, "grid_major": 10.0, "grid_radius": 0.0}}
return {"view": {"grid_color": "#94a3b8", "grid_alpha": 0.5}}
return {"view": {"grid_overlay": True}}
return {"view": {"workplane": "activate"}}
return {"tool": "toy_polyline"}
```

| Key | Meaning |
| --- | --- |
| `grid` | show or hide the reference grid |
| `axis_cross` | show or hide the world triad |
| `projection` | `"persp"`/`"perspective"` or `"ortho"`/`"orthographic"` |
| `grid_minor`, `grid_major` | world-grid steps in millimetres; both positive, major at least minor |
| `grid_radius` | fixed extent in millimetres; `0` is camera-derived |
| `grid_color` | `"#rgb"` or `"#rrggbb"` |
| `grid_alpha` | multiplier over the grid token alphas, `0..1` |
| `grid_overlay` | hide the native ground in favour of an authored overlay; registered overlays draw either way |
| `workplane` | an **action**: `"activate"`, `"clear"` or `"view"` |

The shell applies a request, then mirrors the resulting state to
`FerroCADGui.ViewState`, which is what makes a checkable command's `IsChecked`
close the loop. An invalid projection, a non-positive step, or a major below the
minor is rejected and nothing is applied.

`"tool"` names a tool registered by the **edition** in Rust
(`Contributions::with_tool`). Python cannot register a tool; it can only request
one. An unknown id reports `no such tool <id>` and starts nothing.

`grid_overlay` is a shell flag, not a Python `ViewState` attribute:
`ferrocad_shell.set_view_state()` does not set it. The Coin grid mod uses it to
hide the native ground while its overlay is bound.

## Transactions and errors

- One user-visible edit is one transaction. Open it in `Activated` (or in the
  `on_accept` callback), and leave no transaction open on return.
- On failure, abort the transaction and return `{"status": ...}`.
- `Activated` errors through the shell bridge are contained: the app keeps
  running and the status line gets the error.
- `IsActive`/`IsChecked` errors disable or uncheck one command only.

## Next

- [`display-and-interaction.md`](display-and-interaction.md) — what a command's
  display and interaction hooks look like.
- [`providers.md`](providers.md) — commands that select a feature backend.
- [`../workbenches/actions-and-tools.md`](../workbenches/actions-and-tools.md) —
  the internal command/tool design.
