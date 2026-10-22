# `FerroCAD`

FerroCAD-only surface: the semantic grid and the module hooks.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

The FerroCAD-only Python API (the extensions upstream FreeCAD does not have).

_A mod that imports this depends on FerroCAD; it is not upstream API._

## Functions

| Function | Summary |
| --- | --- |
| `display_mesh(obj)` | The display mesh for `obj`, from the first module that claims it. |
| `interaction(obj, manipulator, action, element='', values=None)` | Apply a manipulator commit through the first module that supports it. |
| `manipulator_specs(obj, element='')` | The declarative handles every module declares for ``obj``, in order. |
| `parameter_file()` | The parameter file in use, or ``None`` while the store is memory-only. |
| `register_display_provider(provider)` | Let ``provider(obj)`` supply the display mesh of a document object. |
| `register_interaction_provider(provider)` | Let ``provider(obj, manipulator, action, element, values)`` apply an edit. |
| `register_manipulator_provider(provider)` | Let ``provider(obj, element)`` declare handles for a selection. |
| `set_parameter_file(path)` | Point ``FreeCAD.ParamGet`` at ``path`` (always enabled). |

## Object `Axis`

The ``FerroCAD.Axis`` singleton (see the module docstring).

| Member | Summary |
| --- | --- |
| apply() -> dict | The ``{"view": …}`` request for the current settings. |
| `color_x` (property) | The X stem's ink as a canonical lowercase ``#rrggbb``. |
| `color_y` (property) | The Y stem's ink as a canonical lowercase ``#rrggbb``. |
| `color_z` (property) | The Z stem's ink as a canonical lowercase ``#rrggbb``. |
| `enabled` (property) | Whether the world axis cross is shown. |
| `ghost_alpha` (property) | The alpha of the occluded (ghosted) stems in ``0..1``. |
| `labels` (property) | Whether the ``X``/``Y``/``Z`` labels are drawn. |
| `length` (property) | The stem length in pixels. |
| `opacity` (property) | The stems' opacity in ``0..1``. |
| `origin` (property) | Whether the origin node is drawn at the cross's centre. |
| reset() -> None | Clear the axis store (only the axis provider's own group). |
| schema() -> list | A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises). |
| snapshot() -> dict | The stored axis policy, in the same shape as the ``axis`` view. |
| `width` (property) | The stem width in pixels. |

## Object `Gizmo`

The ``FerroCAD.Gizmo`` singleton (see the module docstring).

| Member | Summary |
| --- | --- |
| `active_color` (property) | The hovered/active handle's ink as a canonical lowercase ``#rrggbb``. |
| apply() -> dict | The ``{"view": …}`` request for the current settings. |
| `enabled` (property) | Whether the manipulator gizmos are shown. |
| `ghost_alpha` (property) | The alpha of the occluded (ghosted) handles in ``0..1``. |
| `inactive_color` (property) | The idle handle's ink as a canonical lowercase ``#rrggbb``. |
| `opacity` (property) | The handles' opacity in ``0..1``. |
| reset() -> None | Clear the gizmo store (only the gizmo provider's own group). |
| `ring_snap_deg` (property) | The rotation ring's snap increment in degrees. |
| schema() -> list | A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises). |
| `show_readout` (property) | Whether the drag readout is drawn next to the gizmo. |
| `size` (property) | The handle size in pixels. |
| snapshot() -> dict | The stored gizmo policy, in the same shape as the ``gizmo`` view. |
| `width` (property) | The handle stroke width in pixels. |

## Object `Grid`

The ``FerroCAD.Grid`` singleton (see the module docstring).

| Member | Summary |
| --- | --- |
| apply() -> dict | The ``{"view": …}`` request for the current settings. |
| `axis_cross` (property) | Deprecated: moved to :attr:`FerroCAD.Axis.enabled`. |
| `color` (property) | The grid ink as a canonical lowercase ``#rrggbb``. |
| `enabled` (property) | Whether the world grid's ground layer is shown. |
| `major` (property) | The world grid's major (line) step, in millimetres. |
| `minor` (property) | The world grid's minor (dot) step, in millimetres. |
| `opacity` (property) | A multiplier (``0..1``) over the token alphas. |
| `radius` (property) | The fixed world-grid extent (mm); ``0`` is camera-derived. |
| reset() -> None | Clear the grid store, including the pre-S4 legacy keys. |
| schema() -> list | A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises). |
| `shadow_opacity` (property) | The requested shadow strength, ``0..1`` (nothing draws it yet). |
| `shadows` (property) | Whether grid shadows are requested (nothing draws them yet). |
| snapshot() -> dict | The applied state when the shell mirrors it, else the stored policy. |
| `spacing` (property) | The minor step (mm); setting it writes the next decade as the major step. |
