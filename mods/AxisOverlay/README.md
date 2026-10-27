# Axis Overlay (FerroCAD)

A thin client of FerroCAD's semantic axis API: the **Axis Cross** command
(`AxisOverlay_Settings`) opens the settings surface for the native world axis
cross. Like `mods/GridOverlay`, it is a thin client rather than a scene-graph
addon: there is no Qt, no `pivy`, no Coin tree and no per-document attach code.
The command selects `Settings/Axis` when an axis provider is registered; its
declarative fallback dialog writes `FerroCAD.Axis` and returns
`FerroCAD.Axis.apply()` (a `{"view": …}` request the shell applies to the same
state the native axis cross draws).

## Compatibility

This mod imports `FerroCAD`/`FerroCADGui`, so it is **FerroCAD-only**: it is not
usable as-is on upstream FreeCAD. The imports are guarded, and the command
reports a clear status instead of crashing when that API is absent. See
[`docs/developer/architecture/api-namespaces.md`](../../docs/developer/architecture/api-namespaces.md).

## Functional intent

The axis cross is an orientation aid, independent of the grid (the grid is
placement). The knobs are `enabled`, `color_x`, `color_y`, `color_z`, `opacity`,
`length`, `width`, `origin`, `labels` and `ghost_alpha`, stored in
`ParamGet("User parameter:BaseApp/Preferences/Mods/axis/ferrocad")` — one typed
key per schema row, the same convention as the native grid provider.

The old `FerroCAD.Grid.axis_cross` flag remains as a deprecated proxy for
`FerroCAD.Axis.enabled`; on first read the axis store migrates the legacy
`GridAxisCross` value once (from the native grid provider's group, else from
`Preferences/View`).

## Feature detection

`axis_api()` is the single discriminator: `getattr(FerroCAD, "Axis", None)`.
When it is `None` — that is, on upstream FreeCAD — `Activated()` returns a clear
status saying this build needs FerroCAD's semantic axis API.
