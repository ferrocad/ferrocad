# `FreeCADGui`

Upstream-compatible GUI surface: commands, selection and dialogs.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

The ``Gui`` module: the command registry, and (later) the viewer.

_`Selection` is a read-only snapshot pushed by the shell._

## Functions

| Function | Summary |
| --- | --- |
| `addCommand(name: str, command) -> None` | Register ``command`` under ``name`` (``"<Workbench>_<Action>"``). |
| `listCommands() -> list` | The registered command ids, in registration order. |
| `removeCommand(name: str) -> None` | Drop ``name`` from the registry (a no-op when absent). |
| `runCommand(name: str, *args)` | Invoke ``name`` and return whatever ``Activated`` returned. |

## Object `Selection`

A snapshot of what the shell has selected.

| Member | Summary |
| --- | --- |
| getSelection() -> list | The selected document objects (upstream ``Gui.Selection``). |
| getSelectionEx() -> list | ``[(object, [subelement names])]`` for the current selection. |
| isSelected(obj) -> bool | Whether ``obj`` (or an object name) is in the selection. |
