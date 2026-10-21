# `FreeCADGui`

The GUI facade: commands, selection, view state, providers and overlays.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

The ``Gui`` module: the command registry, and (later) the viewer.

_`Selection` and `ViewState` are read-only snapshots pushed by the shell._

## Functions

| Function | Summary |
| --- | --- |
| `activeProvider(feature)` | The active backend for ``feature``, or ``None`` when none is registered. |
| `addCommand(name: str, command) -> None` | Register ``command`` under ``name`` (``"<Workbench>_<Action>"``). |
| `addOverlay(node) -> None` | Register a Coin node as a viewport overlay (FerroCAD extension). |
| `listCommands() -> list` | The registered command ids, in registration order. |
| `providers(feature=None) -> list` | The provider manifests, in registration order. |
| `refreshProviders(feature: str) -> dict` | Ask the active backend of ``feature`` to re-read the shared state. |
| `registerProvider(feature: str, backend: str, label: str, command=None, priority: int = 0, activate=None, deactivate=None, refresh=None, settings=None)` | Declare that ``backend`` provides ``feature`` (called from ``InitGui.py``). |
| `removeCommand(name: str) -> None` | Drop ``name`` from the registry (a no-op when absent). |
| `removeOverlay(node) -> bool` | Drop a previously registered overlay, keyed by Python object identity. |
| `runCommand(name: str, *args)` | Invoke ``name`` and return whatever ``Activated`` returned. |
| `setActiveProvider(feature: str, backend: str) -> dict` | Make ``backend`` the active provider for ``feature`` and apply it. |
| `setSetting(feature: str, name: str, value) -> dict` | Coerce, validate and store one setting, then refresh the provider. |
| `settings(feature: str, backend=None) -> list` | The settings rows of ``feature``'s active provider (or ``backend``). |
| `unregisterProvider(feature: str, backend: str) -> bool` | Drop a provider record; returns whether it existed. |

## Object `Selection`

A snapshot of what the shell has selected.

| Member | Summary |
| --- | --- |
| getSelection() -> list | The selected document objects (upstream ``Gui.Selection``). |
| getSelectionEx() -> list | ``[(object, [subelement names])]`` for the current selection. |
| isSelected(obj) -> bool | Whether ``obj`` (or an object name) is in the selection. |

## Object `ViewState`

The shell's presentation state, pushed by the shell (read-only here).

| Member | Summary |
| --- | --- |
| `axis_cross` (attribute) | — |
| `grid` (attribute) | — |
| `grid_major` (attribute) | — |
| `grid_minor` (attribute) | — |
| `grid_radius` (attribute) | — |
| `projection` (attribute) | — |

## Values

| Name | Summary / value |
| --- | --- |
| `PROVIDER_PREFERENCES` | 'User parameter:BaseApp/Preferences/Mods' |
