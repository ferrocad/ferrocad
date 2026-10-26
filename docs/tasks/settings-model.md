# Task — provider-advertised settings and the `Settings` tree

Status: task (2026-10-22). P0.7 in [`../TODO-26-10-2026.md`](../TODO-26-10-2026.md) §P2.
Companion to [`toolbar-chrome.md`](toolbar-chrome.md) §6,
[`../workbenches/mod-providers.md`](../workbenches/mod-providers.md),
[`../architecture/property-types.md`](../architecture/property-types.md) and
[`../architecture/property-value-extension.md`](../architecture/property-value-extension.md).

## Goal

One model for capability settings: **the provider declares its knobs, FerroCAD stores
them, the model tree and the property pane edit them, and the active provider
survives a restart.** A user switches providers and sees one settings surface, not a
different dialog per mod.

## Why (measured)

| Today | Consequence |
| --- | --- |
| `FerroCAD.Grid` is a bespoke Python object with hand-written properties | every new grid provider re-implements the same knobs |
| `mods/GridOverlay` and `mods/GridOverlayCoin` each ship a settings command and a dialog (`GridOverlay_Settings`, `GridOverlayCoin_Settings`) | switching backends changes the dialog, and two stores can disagree |
| `properties()` only resolves `doc.getObject(name)` | grid settings cannot use the existing typed-row editor |
| `ParamGet` is an in-memory dict (`python/FreeCAD/__init__.py`) | the provider choice and every value are lost on restart |

The property editor already knows the row contract: `properties()` returns
`{name, type, value, status, editable, kind}`, and `_EDITABLE_KINDS` maps a type to a
control. Settings should reuse that contract rather than grow a third renderer.

## The model

### 1. A provider advertises a settings schema

`registerProvider` gains an optional `settings` list. FerroCAD stores the values in
the provider's own parameter group, so a mod does not implement get/set for simple
knobs.

```python
import FerroCADGui

FerroCADGui.registerProvider(
    "grid", "coin", "Coin (WGSL)",
    command="GridOverlayCoin_Settings",
    priority=0, activate=..., deactivate=..., refresh=...,
    settings=[
        {"name": "enabled", "label": "Enabled", "kind": "bool", "default": True},
        {"name": "color",   "label": "Colour",  "kind": "color", "default": "#94a3b8"},
        {"name": "spacing", "label": "Spacing", "kind": "quantity", "unit": "mm",
         "default": 10.0, "min": 0.1, "max": 1000.0},
        {"name": "opacity", "label": "Opacity", "kind": "float",
         "default": 1.0, "min": 0.0, "max": 1.0},
    ],
)
```

- `kind` is the editor's existing set: `text`, `bool`, `int`, `float`, `quantity`,
  `enum` (with `choices`), `color`. A kind the shell does not know renders read-only,
  like an unknown property type does today.
- `min`/`max`/`unit`/`choices` are validation inputs, not decoration: the setter
  rejects out-of-range values the way an enumeration rejects an unknown member.
- Optional per-setting `get`/`set` callables cover computed values. Without them the
  value lives in the parameter group `…/Mods/<feature>/<backend>/<name>`.
- A provider that advertises nothing keeps its `command` as the settings entry. That
  is the upstream-compatible fallback, and it must keep working.

Read/write API (headless, JSON-free for Python callers):

```python
import FerroCADGui

FerroCADGui.settings("grid")                  # rows for the ACTIVE provider
FerroCADGui.setSetting("grid", "spacing", 5.0)
```

`settings()` returns the same row shape as `properties()`, so the pane needs no new
renderer.

### 2. A `Settings` node in the model tree

`model_tree()` gains a root that is **not** a document:

```
Settings                    (synthetic root)
  Grid                      name "Settings/Grid", type "FerroCAD::Settings"
```

- `properties("Settings/Grid")` returns the active provider's rows.
- `set_property("Settings/Grid", name, text)` routes to `setSetting`.
- `/` is not legal in a FreeCAD object name, so the two namespaces cannot collide.
- The node's rows include the **provider selector** itself (an `enum` over the
  registered backends), which is how a provider switch reaches the popover and the
  pane from one place.
- Document-scoped settings are real document objects under the document (S5), not
  synthetic nodes. Resolution order is **document > user > provider default**, and
  the row's `status` says which source won.

### 3. Persistence

- `ParamGet` gains a file backend: one versioned JSON file under the platform config
  directory, loaded at boot and written on change (debounced).
- The active provider is stored under a stable key
  (`BaseApp/Preferences/Mods/<feature>/ActiveProvider`).
- A missing or unknown backend at boot falls back to the declared default (highest
  `priority`, earliest registration) and reports once.
- `FerroCAD.Grid.*` becomes a facade over the native provider's settings (S4), so
  existing scripts keep working; the two must not become separate stores again.

## Slices

| # | Slice | Verifiable headlessly |
| --- | --- | --- |
| **S0** | Schema + parameter-group storage + `settings()` / `setSetting()`, with validation and the no-schema fallback — ✅ Python tests: defaults, round-trip, out-of-range, unknown kind, provider without schema |
| **S1** | `Settings` root and `Settings/<Feature>` nodes in `model_tree()`; `properties()` / `set_property()` routing — ✅ Python tests: addressing, no collision with document names, provider switch changes rows |
| **S2** | Property pane selects the node and renders its rows (reuse the existing editor) — ✅ gpui tests: the tree row, `select`, the rows, and an edit |
| **S3** | File-backed `ParamGet`; persist the active provider; load at boot — ✅ Python tests: restart, corrupt file, unknown backend |
| **S4** | Migrate `FerroCAD.Grid` onto the native provider's schema; keep the old attribute API as a facade; turn the two mod dialogs into aliases of the Settings node — ✅ Python tests: the old attributes and the new rows agree |
| **S5** | Document-scoped overrides and the precedence display (optional) | Python tests: document value wins, deleting it falls back |

`S0`–`S4` are what `toolbar-chrome.md` T4/T5 consume; `S3` is the persistence
promise, and `S5` remains optional.

## Non-goals

- Upstream `Gui::PrefPage` / `.ui` compatibility (that is
  [`../workbenches/qt-strategy.md`](../workbenches/qt-strategy.md)).
- Arbitrary widget trees in a schema: rows only, so one renderer serves every
  capability.
- Persisting document geometry, or settings that belong to a document's content.
- A separate preferences window; the tree + pane is the surface.

## Acceptance

- Switching the grid provider changes the rows under `Settings/Grid` and nothing else
  in the UI; no second dialog appears.
- `set_property("Settings/Grid", …)` validates, stores and refreshes the provider.
- The active provider and all values survive a restart (S3).
- A mod that advertises no schema still works through its command.
- The shell names no mod and no feature-specific field; it renders the schema.
- No regression in `properties()` for document objects.
- `cargo test -p ferrocad_gpui`, `cargo test -p ferrocad_toy` and the Python suite stay
  green.

## As built (2026-10-22)

S0–S4 landed; S5 (document-scoped overrides) remains optional.

- `FerroCADGui.registerProvider(..., settings=...)` validates and normalises the
  schema, then stores it on the provider record. Nothing is written to the
  parameter group at registration: defaults are materialised by `settings()`.
  A malformed schema raises at the call site; an unknown `kind` is accepted,
  reported once, and rendered read-only.
- Values live in
  `User parameter:BaseApp/Preferences/Mods/<feature>/<backend>` with one typed
  key per setting (`bool`, `int`, `float`, and `string` for text/enum/color; a
  `quantity` is a float in its declared `unit`). A setting's optional `get` and
  `set` hooks replace the group; a `get` without a `set` is read-only, because
  writing the group would not be read back.
- A row is `{name, label, type, value, status, editable, kind}` plus `unit` for
  a quantity and `choices` for an enum. `type` is the property type id the
  editor already understands (`App::PropertyBool`, `App::PropertyQuantity`, …),
  and `value` is the display string (`"10 mm"`). `status` is `provider` for a
  group-backed value, `computed` for a `get` hook, `unsupported kind`, or
  `error` when a hook fails. There is no document/user precedence yet (S5).
- `ferrocad_shell.model_tree()` prepends `Settings` (not a document) once at
  least one provider feature is registered; with none there is nothing to
  configure, so the tree stays exactly the document list. The node segment is
  the feature id title-cased (`Settings/Grid`), and resolution accepts either
  the exact feature id or that title form. No `/` is legal in a document object
  name, so the namespaces cannot collide.
- `setSetting` writes, then calls `refreshProviders(feature)` and returns its
  `view`; a validation failure changes nothing and returns `error`. The
  provider selector in `properties("Settings/<Feature>")` switches by label or
  backend id through `setActiveProvider`.

### S2 — the pane renders the node

- `View_GridSettings` and the per-mod settings commands return
  `{"select": "Settings/Grid"}` when a grid provider is registered; the modal is
  the fallback only when none is. `CommandOutcome` gained a `select` directive (the
  facade's `_payload` forwards it), and `Shell::run_command` **opens the floating
  settings card** for a settings node instead of feeding it through the selection
  path. The tree spelling (`Settings/grid`) and the directive's display form
  (`Settings/Grid`) are treated as one node.
- A settings panel is a **tool, not a selection**. Opening it leaves the current
  object selected (and its gizmo armed); a later selection change does not close it.
  The card owns its node and rows (`SettingsCard { node, rows }`), re-read after
  every edit, reset or provider switch, because `ShellModel::properties` follows the
  selection. It is dismissed only by its `✕` or `Esc`, and it draws above the
  transient overlays (deferred priority 4). The card keeps its 420 px height: the
  rows scroll inside it (the content wrapper must be a **column**, or the panel is
  laid out at its full content height and clipped with nothing to scroll).
- Selecting a `Settings/<Feature>` node **in the tree** still retargets the card —
  that is the explicit inspector path — while the Dev Tools pane shows the node's
  rows. A settings *command* does not move the selection at all.
- The settings rows in a header menu are **actions**: they dismiss the menu. The mod
  settings commands no longer report `IsChecked` (which had made them look like state
  rows and kept the menu open), and `run_command_in_place` refuses to restore a menu
  whose run opened a settings card.
- gpui tests cover the tree row, the `select` directive, the rows (`provider`, the
  schema names), an edit, the card's open/drag/close/Esc lifecycle, that the pane
  creates a field per editable row, that the menu closes on a settings row, and that
  the selection and the card survive each other.

### S3 — the parameter store is file-backed

- `python/FerroCAD/_params.py` exposes `FerroCAD.set_parameter_file(path)` and
  `FerroCAD.parameter_file()`; the FreeCAD side keeps only private helpers, so the
  upstream namespace is unchanged.
- Path precedence is explicit → `FERROCAD_PARAM_FILE` →
  `$XDG_CONFIG_HOME/ferrocad/parameters.json` (`~/.config` fallback). The default
  file is enabled only when `FreeCAD.GuiUp` is truthy, so headless tests stay
  in memory and never touch the user's config.
- The file is versioned JSON, written atomically (`tempfile` + `os.replace`) on
  mutation. A corrupt file starts empty, reports once, and is replaced only by the
  next write. The active provider already lives under
  `BaseApp/Preferences/Mods/Providers`, so it persists with everything else; an
  unknown saved backend falls back to the declared default.

### S4 — one grid store

- `FerroCAD._grid.SETTINGS_SCHEMA` is the canonical nine-row vocabulary
  (`enabled`, `color`, `opacity`, `minor`, `major`, `radius`, `axis_cross`,
  `shadows`, `shadow_opacity`); `spacing` stays the `minor` alias that writes the
  next decade to `major`. `mods/GridOverlay` advertises the whole schema;
  `mods/GridOverlayCoin` advertises the subset it owns with `get`/`set` hooks into
  its existing store.
- `FerroCAD.Grid` now reads and writes
  `User parameter:BaseApp/Preferences/Mods/grid/ferrocad`; the first read of each
  key migrates the old `Preferences/View` value once, so existing settings are not
  lost. `reset()` clears both, and the attribute API, validation, `snapshot()`,
  `apply()` and the `{"view": …}` request are unchanged.
- `ferrocad_shell.grid_summary()` is the shell's read-only view of the active
  provider (`backend`, `label`, `spacing`, and the provider list). The chrome T4
  popover consumes it; the two stores can no longer disagree.

### S4b — the axis cross is a second capability

- The axis cross is no longer a grid row: `FerroCAD._axis` mirrors the grid
  facade with `FEATURE = "axis"`, its own `AXIS_SETTINGS_SCHEMA` (ten rows:
  `enabled`, `color_x`, `color_y`, `color_z`, `opacity`, `length`, `width`,
  `origin`, `labels`, `ghost_alpha`) and its own provider group
  (`User parameter:BaseApp/Preferences/Mods/axis/ferrocad`).
- `FerroCAD.Axis.apply()` returns `{"view": {"axis": {…}}}`; `App_ToggleAxisCross`
  toggles `FerroCAD.Axis.enabled` and returns that request, falling back to the
  old `{"view": {"axis_cross": …}}` when the build has no `Axis`.
- `mods/AxisOverlay` is the native `axis`/`ferrocad` provider and advertises the
  schema, so `Settings/Axis` and `FerroCAD.Axis` are one store. The first read of
  `enabled` migrates the legacy `GridAxisCross` value once (grid provider group,
  else `Preferences/View`); `FerroCAD.Grid.axis_cross` remains as a deprecated
  proxy.
- `mods/AxisOverlayCoin` adds the `axis`/`coin` (Coin/WGSL) backend. It
  advertises the same `AXIS_SETTINGS_SCHEMA` with `get`/`set` hooks into
  `FerroCAD.Axis`, so the two backends share one store; `enabled` is still the
  one switch. The Coin backend draws a depth-tested visibility pass only and
  leaves the ghost pass and the labels to the native backend.
- Both backends claim `view.axis.overlay`: native answers `false` (it draws the
  native triad), Coin answers `true` while active (it draws the axis itself).
  `_axis_view_request()` merges that claim into the toggled/toggled-off request
  (`enabled and owned`), so switching to the Coin backend hides the native triad
  and switching back restores it; `axis_summary()` names the active provider and
  its stem size for the chrome split button.
- The backend settings commands (`GridOverlay_Settings`, `GridOverlayCoin_Settings`,
  `AxisOverlay_Settings`, `AxisOverlayCoin_Settings`) and `Mods_Settings` are marked
  `FerroCAD_DevOnly: "true"`. The facade copies that into each command-list entry as
  `dev_only`, so they stay in the Dev Tools command list but are filtered out of the
  header popovers. The Grid and Axis popovers offer the provider rows plus the generic
  `View_GridSettings`/`View_AxisSettings` and `App_Reload*Settings` commands; a
  provider row switches through `ferrocad_shell.set_provider()`
  (`{"feature", "backend"}` → `{ok, error, status, view}`).

### S4c — gizmo is a capability

- The manipulator gizmo is now a capability like the grid and the axis cross:
  `FerroCAD._gizmo` defines `FEATURE = "gizmo"`, `GIZMO_SETTINGS_SCHEMA` (thirteen
  rows: `enabled`, `active_color`, `inactive_color`, `outline_color`, `size`,
  `width`, `halo_width`, `hover_scale`, `hit_radius_px`, `opacity`, `ghost_alpha`,
  `ring_snap_deg`, `show_readout`) and the provider group
  `User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad`. There is no legacy
  key to migrate; `reset()` clears only this group. The four WCAG rows
  (`outline_color`/`halo_width`, `hover_scale`, `hit_radius_px`, plus the
  `ghost_alpha` ceiling) are documented in
  [`../interaction/gizmo-manipulators.md`](../interaction/gizmo-manipulators.md) §17.
- `FerroCAD.Gizmo.apply()` returns `{"view": {"gizmo": {…}}}` and adds
  `backend` from `FerroCADGui.activeProvider("gizmo")` (else `"ferrocad"`).
  `backend` is not a schema row: `snapshot()` is the thirteen stored values, and
  `mods/GizmoOverlay` (the `gizmo`/`ferrocad` provider, priority 10) forces
  `"ferrocad"` in its activate/deactivate/refresh requests.
- `mods/GizmoOverlay/InitGui.py` advertises the schema, so `Settings/Gizmo` and
  `FerroCAD.Gizmo` are one store; `GizmoOverlay_Settings` returns
  `{"select": "Settings/Gizmo"}` when a gizmo provider is registered and a
  schema-derived fallback dialog otherwise. The command carries
  `FerroCAD_Surface: "gizmo"`, an empty `FerroCAD_Parent` (there is no header
  primary) and `FerroCAD_DevOnly: "true"`.
- This section is the settings/store half (G0). The Rust half (G1) landed as
  `view.gizmo`: `ViewRequest.gizmo` → `ViewState.gizmo: ferrocad_stage::GizmoStyle`,
  validated atomically (bad colour / non-positive size or width / empty backend /
  `hover_scale < 1` rejects the whole request; `opacity` clamps to `0..=1`,
  `ghost_alpha` to `0..=0.5`, `halo_width` to `0..=8`, `hover_scale` to `1..=4`,
  `hit_radius_px` to `12..=48`, and the ring detent to `0..=180` then to a 5°
  multiple). The hex colours are RGB-only, so the role's token alpha is
  kept. `Viewport::set_gizmo_style` forwards the style to every hosted manipulator
  and to the `TransformGizmo`, including objects hosted later, and owns the
  `enabled` (draw/pick) and `show_readout` gates; the shell arms the native handles
  even while disabled, so re-enabling needs no re-selection. `ghost_alpha` now
  reaches `ShapeItem`'s occluded pass (`with_ghost_alpha`/`set_ghost_alpha`) instead
  of being a stored-only value. The `outline`/`halo_width` pair is the WCAG 1.4.11
  contrast aid and `hover_scale` the 1.4.1 non-colour cue; the halo stays native
  under a shader skin.
- `backend` is stored as a **provider claim**, not an implementation. Hosting is
  data-driven: `ferrocad` is hosted by the build, and any other id is hosted by the
  **skin its provider supplies** through
  `registerProvider(..., skin=<WGSL source>)` → `FerroCADGui.providerSkin(feature)` →
  `ferrocad_shell.provider_skin` → `Viewport::set_gizmo_skin`. The shipped
  `mods/GizmoOverlayShader` owns `Resources/gizmo_skin.wgsl`; a backend with no skin is
  reported as `gizmo backend <id> supplied no skin (native handles stay active)`. The
  skin changes materials only — the native manipulator keeps hover, picking, drags and
  commits. See
  [`../interaction/gizmo-manipulators.md`](../interaction/gizmo-manipulators.md) §17–19.
- G3a adds the geometry half: a mod declares a handle through
  `FerroCAD.register_manipulator_provider` (mesh, optional geometry program, frame,
  axis, bounds, readout) and commits through the same `register_interaction_provider`
  hook. Declared handles follow the compiled manipulator for a selection; a bad spec is
  logged and skipped.

## Risks

- **Precedence confusion.** Show the winning source in the row's `status`; a value
  that looks editable but is overridden by the document is a support burden.
- **Name collisions.** Synthetic names contain `/`; keep the routing strict (a document
  object can never contain `/`) and reject unknown `Settings/...` paths clearly.
