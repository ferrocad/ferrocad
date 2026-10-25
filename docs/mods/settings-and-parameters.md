# Settings and parameters

Status: guide (2026-10-25).

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

The store is **file-backed in a GUI session**. The first `ParamGet` after a file
is configured loads it, and every mutation rewrites the whole file atomically.
The default path is `$XDG_CONFIG_HOME/ferrocad/parameters.json` (falling back to
`~/.config/ferrocad/parameters.json`), enabled only when `FreeCAD.GuiUp` is
truthy. A headless process stays in memory unless a path is set with
`FerroCAD.set_parameter_file` or `FERROCAD_PARAM_FILE`
(`python/FreeCAD/__init__.py:224-244`, `:370-418`). Values survive a restart;
the shell's *drawn* state does not (see
[Precedence](#precedence-who-wins-and-when)).

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
| `GetGroups()` | the immediate child group names, sorted |
| `HasGroup(name)` | whether the group was created |
| `GetInts()` / `GetBools()` / `GetFloats()` / `GetStrings()` | the **names** in this group with a value of that type (upstream FreeCAD's shape) |
| `Clear()` | removes the values in this group and its descendants |
| `RemGroup(name)` | removes a child group, its values and descendants — **unless a live handle still references it** (upstream `BaseTests.testGroup`) |
| `Export(path)` / `Import(path)` | JSON file, not the upstream FreeCAD parameter format |

`Clear()` removes values, not group names; `HasGroup` can still be true
afterwards. Use `RemGroup` when the group itself must go.

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

The values live in the native grid provider's group,
`FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/grid/ferrocad")`,
under the schema key (`python/FerroCAD/_grid.py:41-42`, `:64-123`):

| Property | Key | Default | Validation / meaning |
| --- | --- | --- | --- |
| `enabled` | `enabled` | `False` | show the world grid's ground layer |
| `color` | `color` | `"#94a3b8"` | `#rgb` or `#rrggbb`; stored canonical lowercase |
| `opacity` | `opacity` | `1.0` | multiplier over the token alphas, `0..1` |
| `spacing` | `minor` + `major` | `10.0` / `100.0` | getter is `minor`; setter writes the next decade as major |
| `minor` | `minor` | `10.0` | positive, millimetres |
| `major` | `major` | `100.0` | positive, millimetres |
| `radius` | `radius` | `0.0` | fixed extent in millimetres; `0` is camera-derived |
| `shadows` | `shadows` | `False` | persisted; nothing draws it yet |
| `shadow_opacity` | `shadow_opacity` | `0.30` | persisted; nothing draws it yet, `0..1` |

`FerroCAD.Grid.axis_cross` is a **deprecated proxy** for `FerroCAD.Axis.enabled`,
not a grid key: it reads and writes the axis store (`python/FerroCAD/_grid.py:345-361`),
so `apply()` does not emit `axis_cross`.

The pre-S4 names (`GridEnabled`, `GridColor`, … in
`User parameter:BaseApp/Preferences/View`) are still read **once per key**: the
first read copies the legacy value into the provider group, and after that the
provider key is authoritative (`python/FerroCAD/_grid.py:126-135`, `:186-198`).
The axis `enabled` flag migrates separately from `GridAxisCross`
(`python/FerroCAD/_axis.py:180-196`).

Setters validate and raise `ValueError` on bad input; they do not silently
clamp.

`apply()` is the bridge to the viewport. It returns the request a command
returns:

```python
{
    "view": {
        "grid": True,
        "grid_minor": 2.0,
        "grid_major": 20.0,
        "grid_radius": 0.0,
        "grid_color": "#123456",
        "grid_alpha": 0.5,
    }
}
```

It raises if the major step is below the minor one. The shadow preferences and
the deprecated `axis_cross` proxy are not included because the shell's grid
request has no field for them.

`snapshot()` is for seeding a dialog. With `FreeCAD.GuiUp` and a
`FerroCADGui.ViewState`, it reads the shell's mirror of what is actually drawn;
otherwise it reads the store. It also returns `shadows` and `shadow_opacity`,
and reports `grid_radius` as `None` when the stored value is `0`.

`reset()` clears the grid provider's group and the grid's own legacy keys in
`BaseApp/Preferences/View`; it never clears that group wholesale, because it
also holds unrelated view preferences (`python/FerroCAD/_grid.py:456-471`).

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
    FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/grid/ferrocad")
    .GetFloat("minor", 0.0)
)
```

Output:

```text
{'view': {'grid': True, 'grid_minor': 2.0, 'grid_major': 20.0, 'grid_radius': 0.0, 'grid_color': '#123456', 'grid_alpha': 0.5}}
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

The supported kinds are `text`, `bool`, `int`, `float`, `quantity` (with `unit`),
`enum` (with `choices`) and `color`. A `color` value is a `#rrggbb` string, and the
property pane renders it as a swatch plus the hex field: clicking the swatch expands
`ferrocad_widgets::color::palette` under the row, and a pick commits through the same
path as typing the hex. The palette is a fixed set of neutrals and accents; typing an
arbitrary colour still works.

The `refresh` result is only a **request**. It reaches the viewport when the
caller applies it: a command or a dialog does, the `Settings/<Feature>` property
pane does not (see [Precedence](#precedence-who-wins-and-when)).

## Precedence: who wins, and when

There is no single resolver. Two pieces of code answer "what is the value?"
and they meet only when a `{"view": …}` request is applied:

1. **The parameter store (Python).** `FerroCAD.Grid`, `FerroCAD.Axis` and
   `FerroCAD.Gizmo` read one typed key per schema entry, with the schema's
   `default` as the `GetX(name, default)` fallback
   (`python/FerroCAD/_grid.py:201-212`, `_axis.py:199-211`, `_gizmo.py:144-154`;
   `FerroCADGui._read_setting` at `python/FerroCADGui/__init__.py:987-1008`).
   A stored value wins; the default is only a fallback and is never written by a
   read.
2. **The shell (Rust).** `ViewState` starts at `ViewState::default()`
   (`crates/ferrocad_gpui/src/view.rs:224-240`) and is changed only by
   `ViewState::apply`, which leaves every omitted field alone
   (`crates/ferrocad_gpui/src/view.rs:248-632`). It never reads `ParamGet`.

The two meet at the request boundaries: a command, dialog, provider switch or
`refreshProviders` result returns `{"view": …}`, and `apply_view` copies it into
`ViewState`, mirrors it to `FerroCADGui.ViewState`, and pushes it to the viewport
(`crates/ferrocad_gpui/src/shell.rs:2429-2496`).

### The layers

| Layer | Where it lives | Kind | Applies | Persists? | Can be overridden by |
| --- | --- | --- | --- | --- | --- |
| Built-in Rust default | `GizmoStyle::default()`, `AxisStyle::default()`, `GridStyle::default()`, composed by `ViewState::default()` (`crates/ferrocad_stage/src/gizmo.rs:92-107`, `axis.rs:66-80`, `grid.rs:129-139`, `crates/ferrocad_gpui/src/view.rs:224-240`) | fallback | The shell's initial state, and any field a request omits | No — recomputed each boot | Any applied view request |
| Mod/provider default | The `default` in a `settings=[…]` schema (`mods/GridOverlay/InitGui.py:258-267` advertises `_grid.SETTINGS_SCHEMA`) | fallback | Only when the store has no value for the key | No — never written at registration (`python/FerroCADGui/__init__.py:288-295`) | A stored value; a `get` hook |
| User override | `User parameter:BaseApp/Preferences/Mods/<feature>/<backend>`, one key per setting | override | Any facade read; drawn only after a request applies it | Yes (JSON file in a GUI session) | Another setter, a migration; the drawn state by a later request |
| Legacy value | `…/Preferences/View` `Grid*`, and `GridAxisCross` in the grid provider group | one-time override | The first read of a key with no provider value | Yes — copied into the provider group | A user write after the migration; `reset()` |
| Edition preset | `registerAppearancePreset` at import; `installAppearancePresets()` in the edition's `bootstrap()` writes it into the native provider's group with a `__appearance_preset__` marker (`python/ferrocad_toy/__init__.py:211-231`, `:379-384`; `python/ferrocad_parametric/__init__.py:178-192`, `:271-276`; `python/FerroCADGui/__init__.py:1321-1405`) | fallback | At boot, before the document is created, and only where the user has no value | Yes — as ordinary keys; the marker says which values the preset still owns | A user write; `resetAppearance` restores the preset rather than clearing it (`python/FerroCADGui/__init__.py:1519-1564`) |

The edition layer is a **fallback, not an override**. The edition module calls
`registerAppearancePreset` at import (declaration only, no write); its
`bootstrap()` then calls `installAppearancePresets()` **before** the base
bootstrap creates the document (`python/ferrocad_toy/__init__.py:225-231`,
`:379-384`; `python/ferrocad_parametric/__init__.py:186-192`, `:271-276`). The
install materialises each value into the native provider's group only when the
key is absent, and records what it wrote under `__appearance_preset__`
(`python/FerroCADGui/__init__.py:1367-1405`). On a later boot it replaces a value
only while it still equals the marker, so a user edit wins across restarts and
across preset changes; `resetAppearance` clears the capability store and installs
the preset again (`python/FerroCADGui/__init__.py:1507-1552`).

The toy preset is the visible example: with no user values,
`FerroCAD.Gizmo.active_color` is `#ffd9a3`, not the schema default `#38bdf8`
(`python/ferrocad_toy/__init__.py:211-222`). The preset writes to the native
group even while another backend is active (`_APPEARANCE_NATIVE`,
`python/FerroCADGui/__init__.py:1168`), because the semantic facades read
that group. The edition's Rust main still only composes contributions
(`crates/ferrocad_toy/src/main.rs:23-28`,
`crates/ferrocad_parametric/src/main.rs:67-68`); the layer lives entirely in the
Python `bootstrap()`.

### At process start

`Shell::boot` runs in this order (`crates/ferrocad_gpui/src/shell.rs:383-434`):

1. `python::bootstrap()` imports the edition module. Its `bootstrap()` calls
   `installAppearancePresets()` and then the base bootstrap creates the sample
   document. The edition preset is therefore in the native provider's group
   before any provider is registered. The document observer runs, but no mod is
   loaded yet, so there is no provider to activate.
2. `open_startup_document()` (`--open`) runs **before** the mods load.
3. `load_workbenches()` imports each `Init.py`/`InitGui.py`, which registers
   providers. Registration does not run `activate`
   (`python/FerroCADGui/__init__.py:261-378`), but it is not inert: if a saved
   choice names a backend that registers later, the takeover path runs the
   outgoing backend's `deactivate` (`python/FerroCADGui/__init__.py:368-375`).
   That callback detaches the outgoing backend and leaves the feature's
   `enabled` preference untouched.
4. `set_view_state(&ViewState::default())` and the model's `view:
   ViewState::default()`.
5. `appearanceView()` is folded into the boot `ViewState` (`shell.rs:427-433`),
   including the stored visibility switches.

So the first window draws the **Rust defaults** for anything the request
omits. A persisted user value is in the store, and the persisted provider choice
is resolved by `_selected` (`python/FerroCADGui/__init__.py:469-485`). Stored
appearance reaches the first window through `FerroCADGui.appearanceView()`
(`python/FerroCADGui/__init__.py:1471-1492`), which `Shell::boot` calls and
applies (`crates/ferrocad_gpui/src/shell.rs:427-433`), visibility included: the
provider lifecycle no longer writes `enabled`, so a stored switch survives the
document opening (the temporary `ViewRequest::without_visibility` strip was
removed once the mods stopped owning the switch).

### First window and later document events

`enable_viewport` applies `self.model.view` to the new viewport
(`shell.rs:1000-1021`). That is still `ViewState::default()` unless a command ran
before the viewport existed.

After the mods are loaded, creating or opening a document calls every active
provider's `activate` (`python/FerroCADGui/__init__.py:659-691`). The result is
**discarded** there: the shell does not repaint. The callbacks only read the
store and return an implementation claim (`overlay`, `backend`, `grid_overlay`);
they do not write the feature's `enabled` switch. A user's "off" therefore
survives document creation/activation and a provider switch, and a user's "on"
survives switching away and back. Visibility changes only through
`App_ToggleGrid`/`App_ToggleAxisCross` or a settings row (for the gizmo).

### A user edit

| Surface | Writes | Applies the view? |
| --- | --- | --- |
| `App_ToggleGrid` / `App_ToggleAxisCross` | the facade setter, then returns its `apply()` (and, for the grid, the provider's `refresh` claim — `python/ferrocad_shell/__init__.py:982-999`, `:1191-1194`, `:1220-1226`) | Yes, `run_command` calls `apply_view` (`shell.rs:1841-1850`) |
| A fallback settings dialog's `on_accept` (e.g. `mods/GridOverlay/InitGui.py:150-175`) | the facade setters, then `apply()` | Yes, `finish_dialog` calls `apply_view` (`shell.rs:2213-2224`) |
| `Mods_Settings` (`python/ferrocad_shell/__init__.py:1500-1538`) | `setActiveProvider` per changed feature | Yes, same dialog path |
| A provider switch from the header popover | `setActiveProvider`; the provider callbacks select the backend and claim the overlay/backend, without writing `enabled` | Yes, `activate_provider` calls `apply_view` (`shell.rs:1680-1758`) |
| `Settings/<Feature>` in the property pane | `FerroCADGui.setSetting` / `setActiveProvider`; `setSetting` also runs the backend's `refresh` | **No.** `ferrocad_shell.set_property` returns the `view`, but the Rust `SetResult` decodes only `ok`/`error` (`crates/ferrocad_gpui/src/python.rs:152-156`) and `commit_property` never calls `apply_view` (`shell.rs:2804-2832`) |
| A direct Python write (`FerroCAD.Grid.color = …`) | the store only | No; run `App_ReloadGridSettings`/`App_ReloadAxisSettings` or a toggle |

The last two are where the store and the drawing can disagree without an
error. `setSetting` still runs the backend's `refresh`, so a provider with a
side effect of its own (the Coin overlay) may update; the native ground, triad
and gizmo do not.

`App_ResetGizmoAppearance`, `App_ResetAxisAppearance` and
`App_ResetGridAppearance` are the reset path: each clears its capability store
and reinstalls the edition preset, and returns the merged view request
(`python/FerroCADGui/__init__.py:1507-1552`; the commands are registered in
`python/ferrocad_shell/__init__.py`). Because they are commands, `run_command`
applies their view. A generic property write of `reset` goes through the same
Python surface but is still dropped by the property pane.

### Defaults are not overrides

- A **default** is a fallback: the schema `default` passed to `GetX`, or
  `ViewState::default()`. It is used only when nothing else supplies the field.
- The **edition preset** is a default too: `installAppearancePresets` writes
  only an unset key (first install) or a value it still owns (the marker), so a
  user value always wins. `resetAppearance` is the one path that deliberately
  removes the user values and reinstalls the preset.
- An **override** is written unconditionally. A user's toggle or settings row is
the override for `enabled`; a provider's `activate`/`deactivate` is not. Those
callbacks select an implementation and return its claim, so switching grid,
axis or gizmo backends preserves whatever visibility the user chose.

A schema entry with a `get` hook has no default in practice: `_read_setting`
returns the hook's value and ignores `default`
(`python/FerroCADGui/__init__.py:994-996`). `mods/GridOverlayCoin` uses this for
all five of its rows, so the Coin rows report the Coin store's fallbacks
(`#808080`, opacity `0.65`, `mods/GridOverlayCoin/InitGui.py:63-74`) rather than
the canonical `_grid` defaults (`#94a3b8`, `1.0`).

### Persistence

| Value | Group | Survives a restart? |
| --- | --- | --- |
| grid appearance and switch | `…/Preferences/Mods/grid/ferrocad` (`python/FerroCAD/_grid.py:41-42`) | yes |
| axis appearance and switch | `…/Preferences/Mods/axis/ferrocad` (`python/FerroCAD/_axis.py:36-37`) | yes |
| gizmo appearance and switch | `…/Preferences/Mods/gizmo/ferrocad` (`python/FerroCAD/_gizmo.py:34-35`) | yes |
| any provider's advertised keys | `…/Preferences/Mods/<feature>/<backend>` (`python/FerroCADGui/__init__.py:738-740`) | yes |
| edition preset provenance | `…/Preferences/Mods/<feature>/ferrocad` → `__appearance_preset__` (`python/FerroCADGui/__init__.py:1170-1172`) | yes — it is how a later boot knows which values it may replace |
| the Coin overlay's own knobs | `…/Preferences/Mod/GridOverlayCoin` (singular `Mod`, `mods/GridOverlayCoin/InitGui.py:74`) | yes |
| the chosen backend | `…/Preferences/Mods` → `Providers/<feature>` (`python/FerroCADGui/__init__.py:75`, `:230-232`, `:577`) | yes |
| `ViewState` and the mirror | the shell process | no — rebuilt from `ViewState::default()` plus applied requests |
| applied-provider bookkeeping (`_PROVIDER_APPLIED`, `_PROVIDER_DOCUMENT`) | the interpreter process (`python/FerroCADGui/__init__.py:57-65`) | no |

The file is versioned JSON and is replaced atomically on every mutation
(`python/FreeCAD/__init__.py:370-418`). Headless runs are memory-only by
default; the GUI sets `GuiUp` before the edition module imports
(`crates/ferrocad_gpui/src/python.rs:349-356`), which enables the default path.

### Cache and refresh

`apply_view` is the only place the shell updates presentation state. It calls
`state.apply`, stores the new `ViewState`, pushes it to `FerroCADGui.ViewState`,
updates the viewport's grid/axis/gizmo, re-lists commands (for `IsChecked`) and
syncs overlays (`shell.rs:2429-2496`). `refresh` (`shell.rs:2700-2756`) and
`sync_stage` do **not** touch it, which is why a store write with no request
leaves the picture stale.

`FerroCADGui.ViewState` is a partial mirror: the shell pushes `grid`,
`axis_cross`, `projection`, the grid steps and the radius
(`python/ferrocad_shell/__init__.py:754-760`), not the colours or alpha. The grid
facade's `snapshot` therefore reads colour/alpha from the store and the rest from
the mirror (`python/FerroCAD/_grid.py:411-443`); the axis and gizmo snapshots
always read the store.

### What the code does not define

These are gaps, not rules:

- **Two default copies.** The Rust tokens and the Python `DEFAULT_*` constants
  agree today (e.g. `#38bdf8` is the sRGB spelling of
  `[0.040, 0.509, 0.939]`, `crates/ferrocad_stage/src/gizmo.rs:41`), but they are
  written twice and no test compares them across the boundary. A change to one
  side is invisible to the other's suite.
- **The settings pane now repaints.** `Settings/<Feature>` edits store the value
  and `SetResult` carries the optional `view` the facade returns, which
  `commit_property` applies (`crates/ferrocad_gpui/src/shell.rs`). Before
  2026-10-25 the reply had no `view` field at all and the edit was invisible
  until an unrelated repaint.

### Where this page (and its neighbours) drifted from the code

This section was audited against the code on 2026-10-25. The stale statements
it replaced were:

- **"The store is in memory today"**, repeated in this page's
  `ParamGet` section. The backend is file-backed in a GUI session
  (`python/FreeCAD/__init__.py:224-244`, `:370-418`). The same claim is still in
  `docs/mods/providers.md` ("Limits") and `docs/workbenches/mod-providers.md`.
- **Grid values in `BaseApp/Preferences/View` with `Grid*` keys.** The store
  moved to the provider group; `View` is read once for migration
  (`python/FerroCAD/_grid.py:41-45`, `:126-135`, `:186-198`).
- **`reset()` clears the whole `Preferences/View` group.** It clears the grid
  provider group and only the grid's legacy keys
  (`python/FerroCAD/_grid.py:456-471`).
- **`apply()` emits `axis_cross`.** It does not; the flag is the deprecated
  proxy to `FerroCAD.Axis.enabled` (`python/FerroCAD/_grid.py:345-361`, `:400-409`).
- **`setSetting` makes a provider "see" a value immediately**
  (`docs/mods/providers.md:101-102`). The `refresh` runs, but the
  `Settings/<Feature>` pane drops its `view` (`python/ferrocad_shell/__init__.py:518`,
  `crates/ferrocad_gpui/src/python.rs:152-156`, `crates/ferrocad_gpui/src/shell.rs:2804-2832`).
- **"The feature owns the switch and `activate` only selects an implementation"
  was documented as contradicted by the code** (`docs/mods/providers.md`). The
  mods now follow the rule: the shipped `activate`/`deactivate` callbacks no
  longer write `enabled` (fixed 2026-10-25); visibility is written only by a
  toggle or a settings row, and it is applied at boot like any other stored
  value.

## Edition appearance presets

An **edition** (the toy demo, the Part edition) owns the look of its own objects,
so it may also declare the default appearance of the workspace gizmo, the axis
cross and the grid. This is a layer **under** the user's settings, not a second
settings system: the declaration is validated against the capability's canonical
schema (`FerroCAD.Gizmo` / `FerroCAD.Axis` / `FerroCAD.Grid`), and the resolved
values live in the same provider parameter group the existing `Settings/<Feature>`
rows edit.

```python
import FerroCADGui

FerroCADGui.registerAppearancePreset(
    "gizmo",
    {"active_color": "#ffd9a3", "inactive_color": "#b08f6f"},
)
FerroCADGui.registerAppearancePreset("axis", {"color_z": "#fbbf24"})
FerroCADGui.registerAppearancePreset("grid", {"color": "#b8a894"})
```

A name the capability does not declare, or a value its schema rejects, raises at
registration, so a typo fails at import instead of silently doing nothing. The
app script calls this at import; the edition's `bootstrap()` then calls
`FerroCADGui.installAppearancePresets()` to materialise the declaration.

| Member | Behaviour |
| --- | --- |
| `registerAppearancePreset(feature, values)` | Declare (or replace) a preset; returns a JSON-safe manifest. Writes no parameter. |
| `installAppearancePresets(feature=None)` | Materialise declared presets under the user's values; idempotent. |
| `appearancePreset(feature)` / `appearancePresets()` | Read the declared preset(s). |
| `appearanceView(feature=None)` | Install, then return the merged `{"view": …}` request for the current policy. This is the boot hook. |
| `resetAppearance(feature=None)` | Clear the capability store and reinstall the preset; returns the merged `{"view": …}` request. |
| `unregisterAppearancePreset(feature)` | Remove a declaration and any values it still owns. |

### Precedence

1. a value the user stored in
   `User parameter:BaseApp/Preferences/Mods/<feature>/<backend>` (the editable
   row, or a write through `FerroCAD.Gizmo` and friends);
2. the edition preset;
3. the capability's own schema default;
4. the built-in `GizmoStyle`/`AxisStyle`/grid defaults (what the Rust viewport
   starts with when no request has been applied).

The preset is materialised into the same group as the user's values, so the
facades and the `Settings/<Feature>` pane agree without a second reader. To keep
layer 1 above layer 2 across restarts, the install records the values it wrote
under the non-setting key `__appearance_preset__`. On a later install a value is
replaced only while it still equals that record; a value the user changed is left
alone. Resetting clears the group (including the record), then installs the
preset again.

The one ambiguity is a user value that happens to equal the preset: provenance
is by value, so a later preset change may replace it. Every other user value is
preserved.

### Reset

```python
result = FerroCADGui.resetAppearance("gizmo")
print(result["view"]["gizmo"]["active_color"])  # the edition's preset
```

`resetAppearance()` with no argument resets the gizmo, axis and grid together.
It clears the legacy keys that would otherwise win over a cleared store (the
grid's pre-S4 keys and the old `GridAxisCross` flag). The built-in commands
`App_ResetGizmoAppearance`, `App_ResetAxisAppearance` and
`App_ResetGridAppearance` wrap the same Python surface; the axis and grid ones
sit in their header popovers, and every `Settings/<Feature>` card carries a
read-only `reset` action row whose `command` field names the command:

```python
{
    "name": "reset",
    "label": "Reset to edition defaults",
    "type": "FerroCAD::Command",
    "value": "",
    "status": "action",
    "editable": False,
    "kind": "command",
    "command": "App_ResetGizmoAppearance",
}
```

### Boot hook

The semantic facades read the parameter store, but the shell's initial
`ViewState` is built in Rust before any command runs. An app that wants the
preset (and a user's persisted values) drawn on the first frame should call
`appearanceView()` after the workbenches load and apply the returned request
exactly as it applies a command's `view`. `ferrocad_shell.appearance_view()`
serves the same request as a command outcome (`ok`/`error`/`status`/`view`), so
the host can decode it with the type it already uses for commands. See the
shell's boot sequence; the Python side is already in place:

```python
ferrocad_shell.appearance_view()
# {"ok": true, "error": "", "status": "", "view": {"gizmo": {...}, "axis": {...}, "grid": {...}}}
```

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
5. **Do not assume a parameter write repaints the viewport.** A write changes the
   store; return a `view` request (a dialog's `on_accept` does) or the user must
   run a reload/toggle command. Persistence itself depends on the parameter
   backend: a file in a GUI session, memory in a headless one.

`mods/GridOverlay` is the reference: its `on_accept` writes `FerroCAD.Grid` and
returns `FerroCAD.Grid.apply()`. `mods/GridOverlayCoin` keeps its appearance
knobs in its own group and reads `FerroCAD.Grid.enabled` for the shared switch
when that API exists.

## Next

- [`commands.md`](commands.md) — how the dialog and `view` request reach the shell.
- [`providers.md`](providers.md) — the feature switch and the `refresh` callback.
- [`../python-api/FreeCAD.md`](../python-api/FreeCAD.md) — the generated signature
  reference.
