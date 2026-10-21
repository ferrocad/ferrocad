# Task — the grid toggle only reaches the native grid

Status: task (2026-10-22). P0: a shipped command is wrong under the Coin
provider. Companion to [`../workbenches/mod-providers.md`](../workbenches/mod-providers.md),
[`../workbenches/grid-overlay-plan.md`](../workbenches/grid-overlay-plan.md) and
[`../interaction/axis-origin-grid.md`](../interaction/axis-origin-grid.md).

## Symptom

With the **Coin (WGSL)** grid provider active, `App_ToggleGrid` (the toolbar/menu
*Grid* command, and whatever the shell binds to it) does not hide the grid. The
native grid is not drawn either, so the button visibly does nothing: the overlay
keeps drawing the shader grid at every state of the check mark.

With the **FerroCAD-native** provider active the same command works, which is why
this survived the provider work.

## Reproduction (headless, no GPU)

```python
import FreeCAD, FreeCADGui
import ferrocad_shell, ferrocad_toy   # registers the built-in commands
import mods.GridOverlayCoin.InitGui as coin   # or load it the way tests do

FreeCADGui.setActiveProvider("grid", "coin")
coin.manager.refresh()                        # the overlay is now bound
print(FreeCADGui.runCommand("App_ToggleGrid"))  # {"view": {"grid": true}}
print(coin.setting("enabled"))                  # still True — the overlay stays
print(FreeCADGui.ViewState.grid)                # True
```

The last two are the bug: the check mark and the overlay disagree.

## Root cause — three independent "is the grid on" states

| State | Written by | Read by |
| --- | --- | --- |
| `FreeCADGui.ViewState.grid` (and `ViewRequest.grid`) | the shell, mirroring a `{"view": …}` result | `App_ToggleGrid.IsChecked`, the native renderer |
| `FreeCAD.Grid.enabled` (`User parameter:BaseApp/Preferences/View`, `GridEnabled`) | the grid settings dialog, a provider's `activate`/`deactivate` | the native grid's policy, the FerroCAD-native mod |
| `setting("enabled")` (`…/Preferences/Mod/GridOverlayCoin`) | the Coin mod's own dialog | `GridManager.ensure_grid` / `_bind_active` |

`App_ToggleGrid` ([`python/ferrocad_shell/__init__.py`](../../python/ferrocad_shell/__init__.py),
`_GridCommand.Activated`) only flips the **first** one:

```python
def Activated(self):
    return {"view": {"grid": not FreeCADGui.ViewState.grid}}
```

The Coin provider decides visibility from the **third** one
([`mods/GridOverlayCoin/InitGui.py`](../../mods/GridOverlayCoin/InitGui.py),
`setting("enabled")` → `_bind_active()`), and it only re-evaluates inside
`refresh()`, which today is called from its own dialog and the provider switch —
never from a grid toggle. `ViewRequest.grid_overlay`
([`crates/ferrocad_gpui/src/view.rs`](../../crates/ferrocad_gpui/src/view.rs))
is also only ever set by the mod's `apply_dialog`, so the shell's viewport keeps
"native ground hidden, overlay draws" even after the toggle.

So this is not a Coin bug: it is two owners for one switch. The native provider
happens to agree with itself because its `activate` writes `FreeCAD.Grid.enabled`
and the native renderer follows `ViewState.grid`.

## Decision

**`FreeCAD.Grid.enabled` is the one switch.** The toggle command owns it; the
renderer flag is a projection of it, never an independent input.

1. `_GridCommand.Activated` flips `FreeCAD.Grid.enabled` and returns the view
   request derived from it (`FreeCAD.Grid.apply()` extended with
   `grid_overlay = enabled and a provider owns the overlay`). `IsChecked` keeps
   reading `ViewState.grid`, which the shell already mirrors.
2. Extend the provider registry with one optional `refresh` callback (the
   counterpart of `activate`/`deactivate`, same error isolation and the same
   "never runs at registration" rule). The shell calls
   `FreeCADGui.refreshProviders("grid")` after applying a grid view request; a
   mod that draws from its own store re-reads `FreeCAD.Grid.enabled`.
3. The Coin provider reads `FreeCAD.Grid.enabled` for the master switch instead
   of its private `enabled` knob, keeping its own knobs for appearance. On
   upstream FreeCAD, where `FreeCAD.Grid` is absent, it keeps the private knob —
   that is the existing `grid_api()` discriminator.

Rejected alternatives:

- **Make the Coin mod observe `ViewState.grid`.** There is no change
  notification, and polling per frame in Python is exactly what the pull
  architecture avoids.
- **Have the shell special-case overlay providers.** The shell must not know a
  workbench or a provider; that is the rule the whole module contract exists to
  keep.
- **Leave the Coin mod's own `enabled` as-is and only sync it inside
  `Activated`.** It makes the command know one mod by name — the same violation
  one level down.

## Rename (landed with this task)

The provider id and label were generic and misleading:

| Before | After |
| --- | --- |
| backend `"semantic"` | backend `"ferrocad"` |
| label `"Semantic (native)"` | label `"FerroCAD (native)"` |
| status `Grid Overlay (semantic) …` | `Grid Overlay (FerroCAD) …` |

`"semantic"` remains the name of the *API* (`FreeCAD.Grid` is the semantic grid
API); only the provider id changed. The active provider is in-memory today
(`ParamGet`), so there is nothing to migrate; if it is ever persisted, accept
`semantic` as an alias for one release.

## Acceptance

- [x] With Coin active, `App_ToggleGrid` hides the overlay and the native ground
      together, and `IsChecked` follows. Two toggles return to the start state.
- [x] With the FerroCAD-native provider active, behaviour is unchanged.
- [x] Toggling does not lose the provider's appearance knobs (colour, opacity,
      spacing) — only the master switch changes.
- [x] Headless tests, no GPU:
      - `tests/test_mod_providers.py`: toggle → `coin.setting("enabled")` is False
        and `manager.items[...]` is unbound (`set_visible(False)` asserted on the
        recorded overlay);
      - `tests/test_view_tools.py`: the toggle still round-trips the view request
        and `IsChecked` for the native case;
      - a registry test that `refresh` is only called after an applied request and
        that a broken callback is reported, not raised.
- [x] `docs/user/` gains a workflow for the working toggle (it is a user-visible
      capability, and the index is the progress list). `how-to/grid-visibility.md`.

## Out of scope

- Persisting the active provider (tracked in the 2026-10-22 TODO).
- The `grid_overlay` request gaining a per-provider identity; one boolean is
  enough while only one provider can draw.
- Making the native grid's `grid` flag disappear from `ViewRequest` — it still
  describes the native layer, which the overlay hides.
