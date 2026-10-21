# Settings and parameters

Status: guide (2026-10-22).

Parameters are FerroCAD's persistence layer for configuration. A mod reads them
when it builds a dialog or a draw path, writes them when the user accepts a
dialog, and returns a `{"view": ...}` request to make the change visible.

## `FreeCAD.ParamGet`

```python
import FreeCAD

prefs = FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mod/MyMod")
prefs.SetBool("Enabled", True)
prefs.SetFloat("Opacity", 0.5)
prefs.SetString("Color", "#94a3b8")
```

`ParamGet(path="", create=True)` returns a `ParameterGrp`. The path is split at
the first colon: the part before it is the root, the part after it is the group.
The `create` argument is accepted but the shim always creates the group.

The store is **in memory today**. Values survive `ParamGet` calls and a mod
reload in the same interpreter, but not a process restart. Cross-session
persistence is a parameter-backend gap, not something a mod should work around.

## `ParameterGrp`

| Member | Behaviour |
| --- | --- |
| `GetInt(name, default=0)` / `SetInt` / `RemInt` | integer values |
| `GetUnsigned(name, default=0)` / `SetUnsigned` / `RemUnsigned` | 32-bit unsigned; stored in the same slot as `int` |
| `GetBool(name, default=False)` / `SetBool` / `RemBool` | booleans |
| `GetFloat(name, default=0.0)` / `SetFloat` / `RemFloat` | floats |
| `GetString(name, default="")` / `SetString` / `RemString` | strings |
| `GetGroup(name)` | a child group; creates it |
| `GetGroupName()` | the last path component |
| `HasGroup(name)` | whether the group was created |
| `Clear()` | removes the values in this group and its descendants |
| `RemGroup(name)` | accepted; **a no-op today** |
| `Export(path)` / `Import(path)` | JSON file, not the upstream FreeCAD parameter format |

`Clear()` removes values, not group names; `HasGroup` can still be true
afterwards. `RemGroup` does not do what its name says.

`Export` writes a JSON object whose keys are `"<relative group>\t<name>\t<kind>"`;
`Import` reads that shape back. These are ordinary file operations, not a
FreeCAD `.FCStd` parameter file.

## The worked example: `FerroCAD.Grid`

`FerroCAD.Grid` is the semantic grid settings object. It is resolved lazily as an
**attribute** of the `FerroCAD` module; `import FerroCAD.Grid` is not the API.

```python
import FerroCAD

grid = FerroCAD.Grid
grid.enabled = True
```

The values live in `FreeCAD.ParamGet("User parameter:BaseApp/Preferences/View")`:

| Property | Parameter | Default | Validation / meaning |
| --- | --- | --- | --- |
| `enabled` | `GridEnabled` | `False` | show the world grid's ground layer |
| `color` | `GridColor` | `"#94a3b8"` | `#rgb` or `#rrggbb`; stored canonical lowercase |
| `opacity` | `GridOpacity` | `1.0` | multiplier over the token alphas, `0..1` |
| `spacing` | `GridMinorStep` + `GridMajorStep` | `10.0` / `100.0` | getter is `minor`; setter writes the next decade as major |
| `minor` | `GridMinorStep` | `10.0` | positive, millimetres |
| `major` | `GridMajorStep` | `100.0` | positive, millimetres |
| `radius` | `GridRadius` | `0.0` | fixed extent in millimetres; `0` is camera-derived |
| `axis_cross` | `GridAxisCross` | `True` | the world triad |
| `shadows` | `GridShadow` | `False` | persisted; nothing draws it yet |
| `shadow_opacity` | `GridShadowOpacity` | `0.30` | persisted; nothing draws it yet, `0..1` |

Setters validate and raise `ValueError` on bad input; they do not silently
clamp.

`apply()` is the bridge to the viewport. It returns the request a command
returns:

```python
{
    "view": {
        "grid": True,
        "axis_cross": True,
        "grid_minor": 2.0,
        "grid_major": 20.0,
        "grid_radius": 0.0,
        "grid_color": "#123456",
        "grid_alpha": 0.5,
    }
}
```

It raises if the major step is below the minor one. The shadow preferences are
not included because nothing consumes them.

`snapshot()` is for seeding a dialog. With `FreeCAD.GuiUp` and a
`FerroCADGui.ViewState`, it reads the shell's mirror of what is actually drawn;
otherwise it reads the store. It also returns `shadows` and `shadow_opacity`,
and reports `grid_radius` as `None` when the stored value is `0`.

`reset()` clears the **whole** `BaseApp/Preferences/View` group, not only the
grid keys.

### A runnable read/write

```python
import FreeCAD
import FerroCAD

grid = FerroCAD.Grid
grid.reset()
grid.enabled = True
grid.color = "#123456"
grid.opacity = 0.5
grid.spacing = 2.0

print(grid.apply())
print(
    FreeCAD.ParamGet("User parameter:BaseApp/Preferences/View")
    .GetFloat("GridMinorStep", 0.0)
)
```

Output:

```text
{'view': {'grid': True, 'axis_cross': True, 'grid_minor': 2.0, 'grid_major': 20.0, 'grid_radius': 0.0, 'grid_color': '#123456', 'grid_alpha': 0.5}}
2.0
```

## Advertised settings (providers)

If the mod owns a **feature provider** (see [`providers.md`](providers.md)), do not
write another dialog: declare the knobs in `registerProvider(settings=[...])`.

```python
import FerroCADGui

FerroCADGui.registerProvider(
    "grid", "mymod", "My grid",
    settings=[
        {"name": "spacing", "label": "Spacing", "kind": "quantity",
         "unit": "mm", "default": 10.0, "min": 0.1},
    ],
)
```

FerroCAD stores each value under
`User parameter:BaseApp/Preferences/Mods/<feature>/<backend>` and renders the rows
under `Settings/<Feature>`; `setSetting` validates and then runs the backend's
`refresh`. That is what gives the user one surface across backends instead of a
different dialog per mod. A provider with no `settings` keeps its `command` as its
settings entry.

## The mod pattern

Use one group per mod, for example
`User parameter:BaseApp/Preferences/Mod/MyMod`:

```python
import FreeCAD

GROUP = "User parameter:BaseApp/Preferences/Mod/MyMod"

def _prefs():
    return FreeCAD.ParamGet(GROUP)

def apply_dialog(values):
    prefs = _prefs()
    if "Opacity" in values:
        prefs.SetFloat("Opacity", float(values["Opacity"]))
    return {"view": ...}
```

Rules that keep settings and drawing consistent:

1. **Read in `Activated`** to seed the dialog, and in the draw path if the
   provider draws from its own store.
2. **Write in `on_accept`**, then return the `{"view": ...}` request that makes
   the shell repaint.
3. **Do not use a parameter as a second master switch** for a feature that
   already has one. The feature owns the switch
   ([`providers.md`](providers.md)); appearance knobs are yours.
4. **Validate before writing**, so a rejected value leaves the store unchanged.
5. **Do not assume persistence across runs.**

`mods/GridOverlay` is the reference: its `on_accept` writes `FerroCAD.Grid` and
returns `FerroCAD.Grid.apply()`. `mods/GridOverlayCoin` keeps its appearance
knobs in its own group and reads `FerroCAD.Grid.enabled` for the shared switch
when that API exists.

## Next

- [`commands.md`](commands.md) — how the dialog and `view` request reach the shell.
- [`providers.md`](providers.md) — the feature switch and the `refresh` callback.
- [`../python-api/FreeCAD.md`](../python-api/FreeCAD.md) — the generated signature
  reference.
