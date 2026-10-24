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
| `addModule(name: str) -> None` | Record a module name for the next commit without importing it. |
| `doCommand(text: str)` | Evaluate a Python command string in ``FreeCAD``'s namespace. |
| `getDocument(name)` | The ``GuiDocument`` for an open App document, or ``None``. |
| `getMainWindow()` | The headless main window (a stable singleton). |
| `listCommands() -> list` | The registered command ids, in registration order. |
| `removeCommand(name: str) -> None` | Drop ``name`` from the registry (a no-op when absent). |
| `removeDocumentObserver(observer) -> bool` | Remove a GUI document observer; returns whether it was registered. |
| `runCommand(name: str, *args)` | Invoke ``name`` and return whatever ``Activated`` returned. |

## Class `GuiDocument`

The GUI side of one open App document.

| Member | Summary |
| --- | --- |
| close() -> None | Forget the view and its scene graph; safe to call more than once. |

## Class `InputHint`

One status-bar hint: ``text`` plus any formatting ``args``.

| Member | Summary |
| --- | --- |
| `args` (attribute) | — |
| `text` (attribute) | — |

## Class `UserInput`

The small user-input vocabulary the task panels compare against.

| Member | Summary |
| --- | --- |
| `KeyDelete` (attribute) | — |
| `KeyEnter` (attribute) | — |
| `KeyEscape` (attribute) | — |
| `MouseLeft` (attribute) | — |
| `MouseMiddle` (attribute) | — |
| `MouseMove` (attribute) | — |
| `MouseRight` (attribute) | — |

## Class `View3DInventor`

A document's 3D view, shaped like upstream's ``View3DInventor``.

| Member | Summary |
| --- | --- |
| addEventCallback(name, callback) -> None | Register ``callback`` for the Coin event ``name``. |
| fitAll() | Accept a fit request (there is no camera in a headless view). |
| getPoint(callback=None, *args) | Start a Snapper point session on this view. |
| getSceneGraph() | The document's scene root (``None`` when Coin is unavailable). |
| redraw() | Accept a redraw request (the shell repaints from the scene graph). |
| removeEventCallback(name, callback) -> bool | Drop ``callback`` from ``name``; returns whether it was registered. |

## Object `HintManager`

The headless hint channel (``FreeCADGui.HintManager``).

| Member | Summary |
| --- | --- |
| hide() | Clear the last hint from the status channel. |
| `last` (property) | The last hint passed to :meth:`show`, or ``None``. |
| `last_hint` (property) | — |
| show(*hints) | Show ``hints`` and mirror their text into ``FreeCADGui.draft.hint``. |

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

## Object `Snapper`

The module-level snapper singleton (``FreeCADGui.Snapper``).

| Member | Summary |
| --- | --- |
| active_snaps() | The enabled mode names. |
| addHoldPoint(point=None) | Add ``point`` (or the last snapped point) to the hold-snap list. |
| `angle` (property) | The grid rotation in degrees (the native unit is radians). |
| clearPointConstraintProvider(provider=None) | Clear the provider, or only when it is ``provider``. |
| constrain(point, basepoint=None, axis=None) | Project ``point`` onto the constraint line through ``basepoint``. |
| constrain_reset() | Alias for :meth:`unconstrain`, kept next to :meth:`reset`. |
| `context` (property) | The current ``SnapContext`` for the workplane and grid state. |
| cycleSnapObject(*_args, **_kwargs) | Raise: cycling objects needs a screen-space cursor under the mouse. |
| disable(snap) | — |
| enable(snap) | — |
| getPoint(callback, *args, on_cancel=None, on_finish=None) | Start a point request and return immediately. |
| get_snap_toolbar() | The checkable snap-bar command ids, in toolbar order. |
| hide() | Hide the workplane grid (the native half of upstream ``hide``). |
| isEnabled(snap=None) | Whether snapping is globally enabled, or whether ``snap`` is on. |
| `mask` (property) | The active mode names (a live list, in activation order). |
| mode_enabled(mode) | Whether ``mode`` is in ``mask`` (an unsupported mode is never on). |
| off() | End the current point request without invoking its callback. |
| reset() | Restore the defaults (used by tests and a future workbench reset). |
| restack() | Raise: there is no Coin tracker to lower in a headless build. |
| setAngle(angle) | Rotate the snap grid about the workplane normal. |
| setEnabled(enabled=True) | Enable or disable snapping globally; returns the new state. |
| setGrid(enabled=True) | Show or hide (and enable or disable) the workplane grid. |
| setMode(snap, enabled=True) | Explicitly set ``snap`` on or off; returns the resulting state. |
| setPointConstraintProvider(provider) | Set the task panel that constrains snapped points. |
| setSelectMode(enabled) | Set the select-mode flag (upstream hides snapping while selecting). |
| setTrackers(update_grid=True) | Refresh and return the current ``SnapContext``. |
| setWorkplane(workplane) | Adopt ``workplane`` (a ``ferrocad_stage.Workplane``) and clear rotation. |
| set_enabled(enabled=True) | Enable or disable snapping globally; returns the new state. |
| set_mode(snap, enabled=True) | Explicitly set ``snap`` on or off; returns the resulting state. |
| set_workplane(workplane) | Adopt ``workplane`` (a ``ferrocad_stage.Workplane``) and clear rotation. |
| show() | Show the workplane grid (the native half of upstream ``show``). |
| snap(point, candidates=None) | Snap a world ``point`` and return the resolved ``(x, y, z)``. |
| `snap_context` (property) | — |
| supported_modes() | The modes with a native implementation, in toolbar order. |
| toggle(snap, set_to=None) | Toggle a mode in ``mask`` (or set it explicitly); returns its state. |
| toggleEnabled() | Flip the global snap switch; returns the new state. |
| toggleGrid() | Flip the grid flag; returns the new state. |
| toggle_enabled() | Flip the global snap switch; returns the new state. |
| toggle_snap(snap, set_to=None) | Toggle a mode in ``mask`` (or set it explicitly); returns its state. |
| unconstrain() | Drop the base point and constraint axis. |
| unsupported_modes() | The Draft-only modes that raise when enabled, in upstream order. |
| `workplane` (property) | The effective workplane, with :meth:`setAngle` applied. |

## Object `draftToolBar`

``FreeCADGui.draftToolBar``: the headless Draft task-panel adapter.

| Member | Summary |
| --- | --- |
| Activated() | Mark the task panel active and make it the Snapper's UI adapter. |
| Deactivated() | End the task, finishing a source command that owns the panel. |
| escape() | Cancel the current task: end snapping and finish the source command. |
| offUi() | Release the panel's command and provider state. |
| setAutoGroup(value=None) | Set (or clear) the autogroup name; returns the stored value. |
| setStyleButton(*_args, **_kwargs) | Raise: the Draft style button is a Qt widget FerroCAD does not build. |
| validatePoint(point=None) | Apply the current constraint to ``point`` and return it. |
