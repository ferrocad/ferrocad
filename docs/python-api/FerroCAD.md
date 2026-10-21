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
| `register_display_provider(provider)` | Let ``provider(obj)`` supply the display mesh of a document object. |
| `register_interaction_provider(provider)` | Let ``provider(obj, manipulator, action, element, values)`` apply an edit. |

## Object `Grid`

The ``FerroCAD.Grid`` singleton (see the module docstring).

| Member | Summary |
| --- | --- |
| apply() -> dict | The ``{"view": …}`` request for the current settings. |
| `axis_cross` (property) | Whether the world axis cross (the triad) is shown. |
| `color` (property) | The grid ink as a canonical lowercase ``#rrggbb``. |
| `enabled` (property) | Whether the world grid's ground layer is shown. |
| `major` (property) | The world grid's major (line) step, in millimetres. |
| `minor` (property) | The world grid's minor (dot) step, in millimetres. |
| `opacity` (property) | A multiplier (``0..1``) over the token alphas. |
| `radius` (property) | The fixed world-grid extent (mm); ``0`` is camera-derived. |
| reset() -> None | Clear every grid preference in the view group (back to the defaults). |
| `shadow_opacity` (property) | The requested shadow strength, ``0..1`` (nothing draws it yet). |
| `shadows` (property) | Whether grid shadows are requested (nothing draws them yet). |
| snapshot() -> dict | The applied state when the shell mirrors it, else the stored policy. |
| `spacing` (property) | The minor step (mm); setting it writes the next decade as the major step. |
