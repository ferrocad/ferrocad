# Feature providers

Status: guide (2026-10-22).

A **feature** is a named capability with more than one possible implementation.
A **provider** (or backend) is one implementation of that feature. The provider
registry keeps **exactly one active backend per feature** and persists the
choice.

The reference pair is the grid:

| Mod | feature | backend | label | priority |
| --- | --- | --- | --- | --- |
| `mods/GridOverlay` | `grid` | `ferrocad` | `FerroCAD (native)` | 10 |
| `mods/GridOverlayCoin` | `grid` | `coin` | `Coin (WGSL)` | 0 |

Both register from `InitGui.py`. Only one draws; `Mods_Settings` switches.

## Declaring a provider

```python
import FreeCADGui

FreeCADGui.registerProvider(
    "grid",                     # feature
    "coin",                     # backend id
    "Coin (WGSL)",              # label shown by Mods_Settings
    command="GridOverlayCoin_Settings",
    priority=0,
    activate=activate,          # make this backend the drawn one
    deactivate=deactivate,      # undo activate
    refresh=refresh,            # re-read shared state, no switch
)
```

| Argument | Meaning |
| --- | --- |
| `feature` | non-empty string; what is implemented |
| `backend` | non-empty string; this implementation's id |
| `label` | the user-facing name; falls back to `backend` |
| `command` | the command id this backend owns, or `None` |
| `priority` | larger wins the default; equal priority is broken by earliest registration |
| `activate` | optional zero-argument callback: become the drawn backend |
| `deactivate` | optional zero-argument callback: undo `activate` |
| `refresh` | optional zero-argument callback: shared state changed, re-read it |

`activate`, `deactivate` and `refresh` are optional and must be callable if
given. `activate` and `refresh` never run at registration; `deactivate` runs
there only for the takeover case below. `registerProvider` returns the new
backend's manifest.

Registering the same `(feature, backend)` again replaces the record without
moving it in the registration order. That makes a mod reload safe.

**Takeover.** If a registration changes the active backend — a higher priority,
or a saved choice that names the new backend — the registry runs the outgoing
backend's `deactivate` immediately. The incoming backend is still not activated
until the first `setActiveProvider`, so `activate` stays out of import time.

## Which backend is active

In order:

1. the persisted choice at
   `FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods")` →
   `Providers/<feature>`, **if** it names a registered backend;
2. otherwise the highest `priority`;
3. among equal priorities, the earliest registered backend.

A saved value that names a removed or renamed backend is ignored, not an error.

```python
import FreeCADGui

FreeCADGui.activeProvider("grid")     # "ferrocad" (the declared default)
FreeCADGui.providers("grid")          # manifests for both backends
FreeCADGui.providers()                # every feature
```

A manifest is JSON-safe:

```python
{
    "feature": "grid",
    "backend": "ferrocad",
    "label": "FerroCAD (native)",
    "command": "GridOverlay_Settings",
    "priority": 10,
    "active": True,
}
```

The callbacks stay private, which is why the manifest can cross the JSON
boundary.

## Switching

```python
result = FreeCADGui.setActiveProvider("grid", "coin")
```

`setActiveProvider(feature, backend)`:

1. persists the choice;
2. runs the outgoing backend's `deactivate()`, if any;
3. runs the incoming backend's `activate()`, if any;
4. records the incoming backend as applied.

The outgoing backend is the **applied** one when there is one, otherwise the
declared active one. That is what makes the first switch restore the default it
left, even if that default's `activate` never ran. Selecting the already-declared
backend is the bootstrap case: its `activate` runs once; later selections are
inert.

The callbacks may return the same dict a command may return. `view` is merged
across the switch and returned under `view`; `status` is folded into `status`.
Failures are caught, reported once per `(feature, backend, hook)`, and listed
under `errors`.

```python
{
    "ok": True,
    "feature": "grid",
    "backend": "coin",
    "previous": "ferrocad",      # declared before the call
    "view": {"grid": False, ...},
    "status": "grid: coin is the active provider",
    "errors": [],
}
```

An unknown feature or backend raises `ValueError`. A callback failure does not:
a failing `deactivate` does not skip the incoming `activate`.

`unregisterProvider(feature, backend)` drops a record and returns whether it
existed. It does **not** run `deactivate`; the caller owns the effects. It leaves
the persisted choice alone, so a re-registered backend keeps its selection.

## Refresh, without switching

A feature's state can change without a backend switch. `refreshProviders(feature)`
asks the **active** backend only to re-read it:

```python
FreeCADGui.refreshProviders("grid")
```

It returns:

```python
{
    "ok": True,
    "feature": "grid",
    "backend": "coin",          # None when no provider is registered
    "view": {"grid_overlay": True},
    "status": "",
    "errors": [],
}
```

The `refresh` callback is the counterpart of `activate`/`deactivate` for "shared
state changed, re-read it". It runs only when asked, never at registration or on
a switch. An absent callback, or no provider for the feature, is a no-op. Errors
are reported and collected, never raised.

## The feature owns the switch, not the provider

This is the rule that keeps two implementations from disagreeing about "on":

- The **feature** owns one master switch and the command that writes it.
- A provider's own knobs are for appearance, not for the master switch.
- `activate`/`deactivate` select an implementation; they are not the only place
  the feature is turned on.
- A provider that draws from its own store must implement `refresh` so a
  feature-level change reaches it.
- An inactive backend checks `activeProvider(feature)` before drawing and in
  `Activated`, and reports why it refused. `IsActive` stays `True` so the entry
  remains visible.

The grid is the worked example:

```text
App_ToggleGrid
  -> flips FreeCAD.Grid.enabled            (the one switch)
  -> builds the request from FreeCAD.Grid.apply()
  -> calls FreeCADGui.refreshProviders("grid")
       -> the active backend's refresh() re-reads FreeCAD.Grid.enabled
       -> the Coin backend re-binds its overlay and returns
          {"view": {"grid_overlay": <enabled>}}
  -> merges that grid_overlay claim into the request and returns it
```

`mods/GridOverlayCoin` now reads `FreeCAD.Grid.enabled` for `enabled` when the
semantic API exists (upstream FreeCAD falls back to its private `SETTINGS` knob).
Its colour, opacity, spacing and shadow knobs stay in its own parameter group.
The native backend reads the same switch directly in `apply()`.

The internal note [`../workbenches/mod-providers.md`](../workbenches/mod-providers.md)
covers the selection and switch contract; this page adds the current `refresh`
hook and the feature-owned switch rule.

## Checklist for a backend

- [ ] Register from `InitGui.py` with a unique `(feature, backend)`.
- [ ] Give `Mods_Settings` a unique `label` within the feature.
- [ ] Check `FreeCADGui.activeProvider(feature)` before drawing and in
      `Activated`; return a `{"status": ...}` naming the active backend when it
      is not you.
- [ ] Make `activate` idempotent and safe with no document.
- [ ] Make `deactivate` undo only what `activate` added; leave user settings
      alone.
- [ ] Implement `refresh` if the feature's master switch lives outside your
      store.
- [ ] Keep appearance settings in your own `ParamGet` group.

## Runnable registry example

```python
import FreeCADGui

calls = []

def activate_a():
    calls.append("activate")
    return {"status": "A active"}

def refresh_a():
    calls.append("refresh")
    return {"view": {"grid": True}}

FreeCADGui.registerProvider(
    "demo", "a", "Backend A", priority=10,
    activate=activate_a, refresh=refresh_a,
)
FreeCADGui.registerProvider("demo", "b", "Backend B", priority=0)

print(FreeCADGui.providers("demo"))
print(FreeCADGui.activeProvider("demo"))
print(FreeCADGui.refreshProviders("demo"))
print(calls)

for entry in FreeCADGui.providers("demo"):
    FreeCADGui.unregisterProvider("demo", entry["backend"])
```

## Limits

- The persisted choice is in the in-memory `ParamGet` shim today; there is no
  cross-session persistence.
- Labels should be unique within a feature. `Mods_Settings` maps a value back to
  a backend by label first, then by id.
- The registry says which backend **should** be active. A third-party mod that
  ignores `activeProvider` can still draw; the contract is enforced by the mods
  that follow it.
- There is no native settings panel; `Mods_Settings` uses the ordinary dialog
  protocol.
- `refreshProviders` runs only the active backend. It does not switch, persist,
  or call `activate`.
