# Axis Cross (Coin)

The **Coin/WGSL** backend of FerroCAD's world axis cross. It draws the cross as a
screen-space overlay with a shader over one four-vertex placeholder quad, using
FerroCAD's pull overlay bridge (`FerroCADGui.addOverlay`). The native
`mods/AxisOverlay` backend (`axis`/`ferrocad`, priority 10) remains the default;
this backend (`axis`/`coin`, priority 0) is selectable in `Mods_Settings`.

## Compatibility

This mod imports `FreeCAD`/`FreeCADGui`, `FerroCAD`/`FerroCADGui` and
`pivy.coin`, so it is **FerroCAD-only**: it is not usable as-is on upstream
FreeCAD. The FerroCAD imports are guarded, and the command reports a clear status
instead of crashing when the semantic axis API or the overlay bridge is absent.
See [`docs/developer/architecture/api-namespaces.md`](../../docs/developer/architecture/api-namespaces.md).

## What it is

- The tree is `SoSwitch` → (`SoPickStyle` `UNPICKABLE`, `SoLightModel`
  `BASE_COLOR`) → `SoSeparator` (`renderCulling = OFF`) → `SoDepthBuffer`
  (`test = true`, `write = false`, `function = LESS`) → `SoShaderProgram` →
  the four-corner `SoCoordinate3` + `SoFaceSet` placeholder.
- `Resources/axis_overlay.wgsl` is **byte-identical** to
  `crates/ferrocad_wgpu/shaders/axis_overlay.wgsl`, the reference copy. A mod
  ships loose files and cannot reach into the crate at runtime; a test asserts
  the equality.
- The shader projects the world origin to top-left pixel space, projects each
  unit axis to get its screen direction, and draws three antialiased capsules of
  the configured pixel length and width. The X stem uses `grid_color.rgb`, the Y
  and Z stems use `axis_color_y`/`axis_color_z`, all in linear light. A small
  Z-coloured origin dot honours `axis_origin`. The fragment writes
  `origin_clip.z / origin_clip.w` to `@builtin(frag_depth)`.

## What it shares

The settings rows are the canonical `_axis.SETTINGS_SCHEMA` with `get`/`set`
hooks into `FerroCAD.Axis`, so `Settings/Axis` and both backends are one store
(`User parameter:BaseApp/Preferences/Mods/axis/ferrocad`). `enabled` is the one
shared switch. `AxisOverlayCoin_Settings` selects `Settings/Axis` when an axis
provider is registered; its declarative fallback dialog writes
`FerroCAD.Axis` and returns `FerroCAD.Axis.apply()`.

## What it does not do yet

- **No ghost pass.** A single depth-tested pass cannot also draw occluded stems
  with `Greater`. `axis_ghost_alpha` is in the ABI and is written into the shader
  block, but this shader ignores it; the native backend owns the ghost pass.
- **No labels.** WGSL cannot draw text. `axis_labels` is in the ABI but unused
  here; the native gpui labels are a separate layer.
- **No per-frame Python and no live scene graph.** The tree is extracted through
  the pull bridge and re-extracted when the mod rebuilds and re-registers the
  same root.

## Running it

The mod is picked up by the shell's workbench loader like any other `mods/`
entry. `AxisOverlayCoin_Settings` opens the settings node (or the fallback
dialog); select the Coin backend in `Mods_Settings` to draw it. The headless tree,
provider and settings lifecycle are covered by
`tests/test_axis_overlay_coin.py` with a stubbed `pivy.coin`. Drawing itself needs
a GPU and a manual run (`cargo run -p ferrocad_gpui`) — no GPU readback is part of
this slice.
