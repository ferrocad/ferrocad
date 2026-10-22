# `FerroCADGui`

FerroCAD-only GUI surface: providers, settings, overlays and view state.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

The FerroCAD-only ``Gui`` API (the extensions upstream FreeCADGui lacks).

_`ViewState` is a read-only snapshot pushed by the shell._

## Functions

| Function | Summary |
| --- | --- |
| `activeProvider(feature)` | The active backend for ``feature``, or ``None`` when none is registered. |
| `addDocumentListener(observer) -> None` | Register ``observer`` for document lifecycle notifications. |
| `addOverlay(node) -> None` | Register a Coin node as a viewport overlay (FerroCAD extension). |
| `providerSkin(feature, backend=None)` | The WGSL material program of ``feature``'s backend, or ``None``. |
| `providers(feature=None) -> list` | The provider manifests, in registration order. |
| `refreshProviders(feature: str) -> dict` | Ask the active backend of ``feature`` to re-read the shared state. |
| `registerProvider(feature: str, backend: str, label: str, command=None, priority: int = 0, activate=None, deactivate=None, refresh=None, settings=None, skin=None)` | Declare that ``backend`` provides ``feature`` (called from ``InitGui.py``). |
| `removeDocumentListener(observer) -> bool` | Unregister ``observer``; returns whether it was registered. |
| `removeOverlay(node) -> bool` | Drop a previously registered overlay, keyed by Python object identity. |
| `setActiveProvider(feature: str, backend: str) -> dict` | Make ``backend`` the active provider for ``feature`` and apply it. |
| `setSetting(feature: str, name: str, value) -> dict` | Coerce, validate and store one setting, then refresh the provider. |
| `settings(feature: str, backend=None) -> list` | The settings rows of ``feature``'s active provider (or ``backend``). |
| `unregisterProvider(feature: str, backend: str) -> bool` | Drop a provider record; returns whether it existed. |

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
