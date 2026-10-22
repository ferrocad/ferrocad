# Mod providers: one backend per feature

Two mods now implement the same feature: `mods/GridOverlay` configures the
native grid and `mods/GridOverlayCoin` registers a Coin/WGSL overlay. Before this
registry both registered their commands and both could draw, and the user had no
way to say which one they wanted.

A **provider** is a named backend for a named feature. A mod declares its
provider from `InitGui.py`; `FerroCADGui` keeps exactly one backend active per
feature, persists the choice, and switches the viewport state when it changes.

## Declaring a provider

```python
# <mod>/InitGui.py
import FerroCADGui

FerroCADGui.registerProvider(
    "grid",                     # feature: what is implemented
    "coin",                     # backend: this mod's implementation id
    "Coin (WGSL)",              # label: what Mods_Settings shows
    command="GridOverlayCoin_Settings",
    priority=0,                 # the default: higher wins
    activate=activate,          # make this backend the drawn one
    deactivate=deactivate,      # undo `activate`
)
```

`registerProvider(feature, backend, label, command=None, priority=0,
activate=None, deactivate=None, refresh=None, settings=None, skin=None)` records the
manifest. `command` is the command
id the backend owns (`None` when it has none); `activate` and `deactivate` are
optional zero-argument callbacks. Registering the same `(feature, backend)` again
replaces the record — a mod reload is safe and does not move the provider in the
registration order.

`settings` is the optional row schema the backend advertises (see
[`../tasks/settings-model.md`](../tasks/settings-model.md)); `skin` is the optional
**material program** a backend supplies for a capability that hosts one. Today that is
the manipulator gizmo: the backend hands over the **WGSL source text** (a mod usually
reads it from its own `Resources/`), and the host compiles and draws it while that
backend is active. A supplied skin is data, not behaviour — it cannot add handles or
change what a drag does— so hosting it is safe; the host asks for the *active*
backend's source, so switching back to a backend that supplies none restores the native
look. Neither `settings` nor `skin` appears in the manifest `providers()` returns.

Registration **records**; it does not run `activate`. That happens on the first
switch, or when the mod's own command applies its state. Until then the declared
default is still the only backend allowed to draw (each mod checks
`activeProvider` in its draw path and its `Activated`).

One narrow exception keeps the "one active backend" rule true regardless of load
order: if a registration takes the selection over (a higher priority, or a saved
choice), the outgoing backend's `deactivate` runs immediately so a mod that drew
during its own import cannot keep drawing. The incoming backend is still not
activated until the first switch.

On a facade without the registry (upstream FreeCAD) the call is guarded with
`getattr(FerroCADGui, "registerProvider", None)`, so the mod still loads and
reports its usual upstream message.

## Selection and persistence

The active backend is, in order:

1. the persisted choice at
   `FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods")` →
   `Providers/<feature>` (a string backend id), **if** it names a registered
   backend;
2. otherwise the highest `priority` (larger wins);
3. among equal priorities, the **earliest registered** backend.

A persisted value that names a backend that is no longer registered (the mod was
removed or renamed) is ignored rather than raising, so a stale preference does
not break startup. Exactly one backend is active per feature.

`activeProvider(feature)` returns that backend id, or `None` when nothing is
registered for the feature. `providers()` / `providers(feature)` return
JSON-safe manifests in registration order:

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

## Switching

`setActiveProvider(feature, backend)` persists the choice, then:

1. runs the outgoing backend's `deactivate()`, if any;
2. runs the incoming backend's `activate()`, if any;
3. records the incoming backend as applied.

The outgoing backend is the one that was declared active before the call (the
previously applied one when there is one), so the **first** switch also restores
the default left by a backend whose `activate` never ran. Because `deactivate`
is contractually idempotent this is harmless. Selecting the already-active
backend is the bootstrap case: its `activate` runs once, and later selections
are inert. The choice is written before the callbacks, so a callback that
consults `activeProvider` sees the new backend.

Both callbacks may return the same dict a command may return: a `{"view": …}`
request is merged across the switch and returned under `view` (so the shell
repaints once), and a `{"status": …}` is folded into the returned `status`.
`setActiveProvider` returns:

```python
{
    "ok": True,
    "feature": "grid",
    "backend": "coin",
    "previous": "ferrocad",    # declared before the call
    "view": {"grid": False, ...},
    "status": "...",
    "errors": [],          # callback failures, reported but not fatal
}
```

An unknown feature or backend is a programmer error and raises `ValueError`.
Callback failures are **not**: they are caught, printed once per
`(feature, backend, hook)` to stderr, and listed under `errors`. A failing
`deactivate` does not skip the incoming `activate`; the registry stays usable.

`unregisterProvider(feature, backend)` drops a record (a reload/teardown
helper). It does not run `deactivate` — the caller owns the effects — and it
leaves the persisted choice alone, so a re-registered backend keeps its
selection.

## The document lifecycle

A provider's effects can be **per document**: the Coin backend keeps one overlay
per open document in `GridManager.items`. Before this slot, that attachment was
only dropped when the *next* document was activated, so a closed document's
overlay could linger. The registry now follows the document lifecycle through
`FerroCADGui`'s listener slot (`FreeCADGui.addDocumentObserver` forwards to it):

```python
FerroCADGui.addDocumentListener(observer)     # onDocumentCreated/Activated/
FerroCADGui.removeDocumentListener(observer)  # Deactivated/Closed(name)
```

The registry registers its own observer, so the behaviour needs no code in a
mod:

| Event | What the registry runs |
| --- | --- |
| active document **activated** | every feature's declared backend `activate` (the bootstrap / re-attach case) |
| active document **deactivated** or **closed** | the applied backends' `deactivate`, then forget them as applied |
| any document **created** | nothing; activation attaches the providers |

This is what makes "activate on open, deactivate on close" immediate and
symmetric. It is also **idempotent**: the applied record is removed *before* the
`deactivate` callback runs, so a document that is deactivated and then closed
(or closed twice) calls `deactivate` at most once. Closing an inactive document
only reports `closed` — the active document's attachment is untouched. A
provider's `deactivate` runs while the closing document is still open (and
still active), so it can address the document it is detaching from.

The lifecycle is fired by the App facade itself — `FreeCAD.newDocument`,
`open` / `openDocument`, `closeDocument` and `setActiveDocument` — so both an
upstream-shaped script and the `FerroCAD.*` aliases get the notifications.
`ferrocad_shell.bootstrap` goes through the same calls, so the app's startup and
"recreate the sample document" path behaves identically. `FreeCADGui`'s
upstream-shaped `addDocumentObserver` / `removeDocumentObserver` forward to this
slot; the FerroCAD name stays distinct so the namespaces do not blur. See
[`../architecture/api-namespaces.md`](../architecture/api-namespaces.md) for the
split.

A broken observer is reported once per `(observer, method)` to stderr and
dropped for that event; it cannot prevent the document from closing or the other
observers from running. Provider callbacks keep their own once-per
`(feature, backend, hook)` reporting.

## The deactivate contract

`activate` makes the backend the drawn one; `deactivate` must undo exactly that
and nothing more:

- detach/remove what `activate` added (the Coin mod removes its overlay with
  `manager.remove_grid()`);
- restore the shared default the incoming backend expects (the FerroCAD-native mod
  sets `FerroCAD.Grid.enabled = False`, the native grid's documented default);
- **do not** clear the user's own settings. The FerroCAD-native mod leaves colour,
  spacing and shadows alone, so switching away and back is lossless;
- be idempotent and safe when there is no document (`activate` on Coin with no
  active document is a no-op).

A backend whose command is not active must say so. `IsActive` stays `True` so
the entry remains visible and can explain itself; `Activated` (and a stale
dialog's `on_accept`) returns a `{"status": …}` naming the active backend and
`Mods_Settings`, and changes nothing. The mod's draw path checks the same thing,
so an inactive backend cannot draw behind the active one.

## The two grid providers

| Mod | feature | backend | priority | activate | deactivate |
| --- | --- | --- | --- | --- | --- |
| `mods/GridOverlay` | `grid` | `ferrocad` | 10 | `FerroCAD.Grid.enabled = True`, `apply()` | `enabled = False`, `apply()` |
| `mods/GridOverlayCoin` | `grid` | `coin` | 0 | `manager.ensure_grid()` | `manager.remove_grid()` |

Both keep their own command (`GridOverlay_Settings`,
`GridOverlayCoin_Settings`) and their own settings; neither implementation was
removed. The FerroCAD-native backend is the default because it needs no Coin facade.

## Switching from the shell

`Mods_Settings` (registered by `python/ferrocad_shell`) opens the ordinary dialog
spec: one `enum` field per feature, whose options are the providers' labels and
whose seed is the active one. Accepting calls `setActiveProvider` for every
feature that changed and returns the merged `{"view": …}` (plus a status line),
which the shell already applies from a dialog. It returns a status instead when
no provider is registered.

The enum value is the label; a backend id is also accepted, which makes the
callback easy to drive from a script or a test.

## Notes and limits

- **Advertised settings (S0–S4 landed).** A provider can declare a `settings`
  schema; [`../tasks/settings-model.md`](../tasks/settings-model.md) implements the
  schema, storage, `settings()`/`setSetting()`, the `Settings/<Feature>` tree node,
  and the pane wiring. `FerroCAD.Grid` is a facade over the native `ferrocad`
  provider's store (with a one-time legacy migration), and both grid mods advertise
  a schema. The Grid popover reads `ferrocad_shell.grid_summary()`; chrome T4/T5
  are wired.
- There is no native Rust settings panel; `Mods_Settings` still uses the dialog
  protocol (`docs/workbenches/actions-and-tools.md` §6), while grid settings use the
  modeless `Settings/Grid` node.
- Labels should be unique within a feature; the dialog maps a value back to a
  backend by label first, then by id.
- The store is file-backed when a GUI is up (`ParamGet` JSON, `FerroCAD.set_parameter_file`);
  headless tests stay in memory. The active provider and every advertised value
  survive a restart.
- The registry says which backend *should* be active. A third-party mod that
  ignores `activeProvider` can still do whatever it likes; the two grid mods
  are the reference implementations of the contract.
- **The losing backend's command is disabled in Dev Tools.** `provider_features()`
  carries each backend's `command` id; the shell dims that row and drops its click
  while another backend owns the feature, so the command list does not offer two
  equally-runnable "settings" entries. This is a UI filter only: the command stays
  in the registry, and a mod's own `IsActive` is still what refuses to run.
