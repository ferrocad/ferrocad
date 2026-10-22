# Gizmo Overlay (shader)

The **shader** backend of FerroCAD's manipulator gizmo. The native
`mods/GizmoOverlay` backend (`gizmo`/`ferrocad`, priority 10) stays the default
and the hit-test/drag owner; this backend (`gizmo`/`ferrocad-shader`, priority
0) is selectable in `Mods_Settings` / Dev Tools → Providers and supplies the
WGSL skin the shell draws the same handles with. The **Gizmo (shader)** command
(`GizmoOverlayShader_Settings`) opens the settings surface for both.

## Compatibility

This mod imports `FerroCAD`/`FerroCADGui`, so it is **FerroCAD-only**: it is not
usable as-is on upstream FreeCAD. The imports are guarded, and the command
reports a clear status instead of crashing when that API is absent. See
[`docs/architecture/api-namespaces.md`](../../docs/architecture/api-namespaces.md).

## Visual only

Switching to this backend **never changes what a drag does**. The native
`TransformGizmo` and its manipulators keep hover, picking, drags and commits;
this mod only supplies the material used to draw the same handle geometry.
Hover highlighting, snap readouts and the transform itself are all owned by the
native path.

## The skin is a mod resource

This mod owns `Resources/gizmo_skin.wgsl`. It is the mod's own material program,
not one compiled into the app: `InitGui.py` reads the file once at import and
passes the text through `FerroCADGui.registerProvider(..., skin=...)`. The host
compiles whatever source the **active** provider supplies
(`FerroCADGui.providerSkin("gizmo")` asks the active backend), so this is a
normal mod resource, like the Coin mods' shaders, and a provider switch cannot
leave an outgoing skin in place. There is no Qt, no `pivy` and no Coin tree.

Editing the file needs a **mod reload**: the source is read at import and cached,
so a running session keeps drawing the text it loaded.

### Skin ABI

The file is not a stand-alone overlay shader; it replaces the material of the
scene's geometry program:

- a **geometry program** — the packet carries the gizmo's own mesh (ribbons,
  glass planes, ghost wire), so only the material lives here;
- the scene `Globals` block at `@group(0) @binding(0)`, declared exactly as
  `scene.wgsl` declares it (`view_proj`, `model`, `color`, `depth_bias`, …), and
  the per-view `Frame` block at `@group(2) @binding(0)`;
- the scene vertex attributes: `position` (`@location(0)`), `normal`
  (`@location(1)`), `uv` (`@location(2)`) and `color` (`@location(4)`);
- the packet's alpha preserved (`globals.color.a`): the glass and ghost passes
  keep their meaning.

### Native fallback

Supplying no skin means the host draws its native gizmo material. This mod still
registers its provider when `Resources/gizmo_skin.wgsl` is missing or
unreadable: it prints one line to stderr and registers without a skin, so the
gizmo keeps working with the native material instead of the mod failing.

## What it shares

The settings rows are the canonical `_gizmo.GIZMO_SETTINGS_SCHEMA`, so
`Settings/Gizmo` and both backends are one store
(`User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad`, through
`FerroCAD.Gizmo`). `enabled` is the one shared switch, and the knobs are
`active_color`, `inactive_color`, `size`, `width`, `opacity`, `ghost_alpha`,
`ring_snap_deg` and `show_readout`. `backend` is not a stored setting:
`FerroCAD.Gizmo.apply()` names `FerroCADGui.activeProvider("gizmo")` (else
`"ferrocad"`), and this provider's activate/deactivate/refresh callbacks name
`"ferrocad-shader"` explicitly.

`GizmoOverlayShader_Settings` selects `Settings/Gizmo` when a gizmo provider is
registered; its declarative fallback dialog writes `FerroCAD.Gizmo` and returns
`FerroCAD.Gizmo.apply()`.

## Feature detection

`gizmo_api()` is the single discriminator: `getattr(FerroCAD, "Gizmo", None)`.
When it is `None` — that is, on upstream FreeCAD — `Activated()` returns a clear
status saying this build needs FerroCAD's semantic gizmo API.
