# `FreeCADGui`

Upstream-compatible GUI surface: commands, selection and dialogs.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

The ``Gui`` module: the command registry and the document views.

_`Selection` is a read-only snapshot pushed by the shell._

## Functions

| Function | Summary |
| --- | --- |
| `activeDocument()` | The ``GuiDocument`` for ``FreeCAD.ActiveDocument``, or ``None``. |
| `addCommand(name: str, command) -> None` | Register ``command`` under ``name`` (``"<Workbench>_<Action>"``). |
| `addDocumentObserver(observer) -> None` | Upstream-shaped alias for :func:`FerroCADGui.addDocumentListener`. |
| `getDocument(name)` | The ``GuiDocument`` for an open App document, or ``None``. |
| `listCommands() -> list` | The registered command ids, in registration order. |
| `removeCommand(name: str) -> None` | Drop ``name`` from the registry (a no-op when absent). |
| `removeDocumentObserver(observer) -> bool` | Remove a GUI document observer; returns whether it was registered. |
| `runCommand(name: str, *args)` | Invoke ``name`` and return whatever ``Activated`` returned. |

## Class `GuiDocument`

The GUI side of one open App document.

| Member | Summary |
| --- | --- |
| close() -> None | Forget the view and its scene graph; safe to call more than once. |

## Class `View3DInventor`

A document's 3D view, shaped like upstream's ``View3DInventor``.

| Member | Summary |
| --- | --- |
| getSceneGraph() | The document's scene root (``None`` when Coin is unavailable). |

## Object `PySideUic`

``FreeCADGui.PySideUic``: the headless ``.ui`` loader.

| Member | Summary |
| --- | --- |
| loadUi(path_or_file) | Load a form from a path, a ``:/`` resource or a ``QFile``-like object. |

## Object `Selection`

A snapshot of what the shell has selected.

| Member | Summary |
| --- | --- |
| getSelection() -> list | The selected document objects (upstream ``Gui.Selection``). |
| getSelectionEx() -> list | ``[(object, [subelement names])]`` for the current selection. |
| isSelected(obj) -> bool | Whether ``obj`` (or an object name) is in the selection. |
