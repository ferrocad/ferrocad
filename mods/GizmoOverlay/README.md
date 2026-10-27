# Gizmo Overlay (FerroCAD)

A thin client of FerroCAD's semantic gizmo API: the **Gizmo** command
(`GizmoOverlay_Settings`) opens the settings surface for the manipulator
gizmos. Like `mods/GridOverlay` and `mods/AxisOverlay`, it is a thin client
rather than a scene-graph addon: there is no Qt, no `pivy`, no Coin tree and no
per-document attach code. The command selects `Settings/Gizmo` when a gizmo
provider is registered; its declarative fallback dialog writes
`FerroCAD.Gizmo` and returns `FerroCAD.Gizmo.apply()` (a `{"view": …}` request
the shell applies to the same state the native gizmo draws).

## Compatibility

This mod imports `FerroCAD`/`FerroCADGui`, so it is **FerroCAD-only**: it is not
usable as-is on upstream FreeCAD. The imports are guarded, and the command
reports a clear status instead of crashing when that API is absent. See
[`docs/developer/architecture/api-namespaces.md`](../../docs/developer/architecture/api-namespaces.md).

## Functional intent

The manipulator gizmo is the transform handle for the current selection. The
knobs are `enabled`, `active_color`, `inactive_color`, `size`, `width`,
`opacity`, `ghost_alpha`, `ring_snap_deg` and `show_readout`, stored in
`ParamGet("User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad")` — one
typed key per schema row, the same convention as the native grid and axis
providers. `backend` is not a stored setting: `FerroCAD.Gizmo.apply()` names
`FerroCADGui.activeProvider("gizmo")` (else `"ferrocad"`), and this provider's
activate/deactivate/refresh callbacks name `"ferrocad"` explicitly.

## Feature detection

`gizmo_api()` is the single discriminator: `getattr(FerroCAD, "Gizmo", None)`.
When it is `None` — that is, on upstream FreeCAD — `Activated()` returns a clear
status saying this build needs FerroCAD's semantic gizmo API.
