# Task — provider-advertised settings and the `Settings` tree

Status: task (2026-10-22). P0.7 in [`../TODO-22-10-2026.md`](../TODO-22-10-2026.md).
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
| `FreeCAD.Grid` is a bespoke Python object with hand-written properties | every new grid provider re-implements the same knobs |
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
FreeCADGui.registerProvider(
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
FreeCADGui.settings("grid")                  # rows for the ACTIVE provider
FreeCADGui.setSetting("grid", "spacing", 5.0)
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
- `FreeCAD.Grid.*` becomes a facade over the native provider's settings (S4), so
  existing scripts keep working; the two must not become separate stores again.

## Slices

| # | Slice | Verifiable headlessly |
| --- | --- | --- |
| **S0** | Schema + parameter-group storage + `settings()` / `setSetting()`, with validation and the no-schema fallback — ✅ Python tests: defaults, round-trip, out-of-range, unknown kind, provider without schema |
| **S1** | `Settings` root and `Settings/<Feature>` nodes in `model_tree()`; `properties()` / `set_property()` routing — ✅ Python tests: addressing, no collision with document names, provider switch changes rows |
| **S2** | Property pane selects the node and renders its rows (reuse the existing editor) | gpui model test: selecting `Settings/Grid` shows the rows; editing calls `set_property` |
| **S3** | File-backed `ParamGet`; persist the active provider; load at boot | Python tests: restart simulation, corrupt file, unknown backend |
| **S4** | Migrate `FreeCAD.Grid` onto the native provider's schema; keep the old attribute API as a facade; turn the two mod dialogs into aliases of the Settings node | Python tests: the old attributes and the new rows agree |
| **S5** | Document-scoped overrides and the precedence display (optional) | Python tests: document value wins, deleting it falls back |

`S0`–`S2` unblock `toolbar-chrome.md` T4/T5. `S3` unblocks the persistence promise.

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

S0 and S1 only; S2 (pane wiring) and S3 (file-backed `ParamGet`) are untouched.

- `FreeCADGui.registerProvider(..., settings=...)` validates and normalises the
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

## Risks

- **Schema drift.** S4 must make the native side authoritative; until then
  `FreeCAD.Grid` and a provider schema could disagree. Do not ship T4 before S4, or
  the popover will show values the renderer does not use.
- **File format.** Version the file from the first write; a corrupt file must fall
  back to defaults rather than fail the boot.
- **Precedence confusion.** Show the winning source in the row's `status`; a value
  that looks editable but is overridden by the document is a support burden.
- **Name collisions.** Synthetic names contain `/`; keep the routing strict (a document
  object can never contain `/`) and reject unknown `Settings/...` paths clearly.
