# Mods: the external contract

Status: guide (2026-10-22).

This section is for people who write a FerroCAD mod, drive `FreeCAD` from a
script, or drive the JSON bridge from an automated client. It describes the
surface FerroCAD actually accepts today, with runnable snippets.

It is deliberately **not**:

- [`../workbenches/`](../workbenches/) — internal design notes about bringing up
  upstream workbenches.
- [`../python-api/`](../python-api/) — the generated signature reference.

Use this section for the contract and the patterns; use those for the internals
and the exact signatures.

## Who this is for

| You are | Start here |
| --- | --- |
| Writing a mod or workbench | [`minimal-mod.md`](minimal-mod.md), then [`mod-anatomy.md`](mod-anatomy.md) |
| Writing a script that drives `FreeCAD` | [`commands.md`](commands.md) and [`display-and-interaction.md`](display-and-interaction.md) |
| Writing an automated client (AI agent, MCP server) | the JSON table below, then [`commands.md`](commands.md) |

## What a mod is in FerroCAD

A mod is a **directory** under a mods path, containing Python scripts that run at
boot:

```text
mods/
  MyMod/
    Init.py        # non-GUI hook
    InitGui.py     # GUI hook (commands, feature providers, overlays)
    Resources/     # icons, shaders, data
    package.xml    # conventional FreeCAD metadata; not read by FerroCAD
```

The loader does not use an upstream `Workbench` class, there is no build step,
and there is no package manager. `FreeCADGui.addWorkbench` does not exist. The
loader runs `Init.py` then `InitGui.py` for every subdirectory (see
[`mod-anatomy.md`](mod-anatomy.md)). Registration is a side effect of running
the script.

A mod is not sandboxed: it runs in the same interpreter as the application. The
isolation rules in [`mod-anatomy.md`](mod-anatomy.md) say where a failure is
contained and where it is not.

The reference mods in this tree are:

| Mod | What it contributes |
| --- | --- |
| `mods/GridOverlay` | the `GridOverlay_Settings` command and the `ferrocad` backend of the `grid` feature |
| `mods/GridOverlayCoin` | the `GridOverlayCoin_Settings` command, the `coin` backend, and a Coin overlay |
| `python/ferrocad_toy/__init__.py` | a complete small mod in one file: two commands, a display provider, an interaction provider |

## The five things a mod can contribute

| Contribution | API | Registration lifetime |
| --- | --- | --- |
| **Commands** | `FreeCADGui.addCommand(name, command)` | until `removeCommand(name)`; a duplicate id raises |
| **A display provider** | `FreeCAD.register_display_provider(provider)` | process lifetime; there is no unregister |
| **An interaction provider** | `FreeCAD.register_interaction_provider(provider)` | process lifetime; there is no unregister |
| **Feature providers** | `FreeCADGui.registerProvider(...)` | until `unregisterProvider(feature, backend)`; re-registering replaces the record |
| **Overlays** | `FreeCADGui.addOverlay(node)` / `removeOverlay(node)` | until removed; needs a `pivy.coin` facade that provides the bridge |

- A **display provider** is asked per document object: "give me a mesh, or
  `None`".
- An **interaction provider** is asked per manipulator commit: "apply this edit,
  or return `None`".
- A **feature provider** is a named backend for a named feature (for example the
  native grid versus the Coin grid). Exactly one backend per feature is active.
- An **overlay** is a Coin node drawn beside the document scene, not a per-object
  display mesh.

Settings and parameters are not a sixth contribution: they are the persistence
layer a mod uses ([`settings-and-parameters.md`](settings-and-parameters.md)).
A feature provider may also implement `refresh`, which
`FreeCADGui.refreshProviders(feature)` runs when the feature's shared state
changes without a switch ([`providers.md`](providers.md)).

## Reading order

1. [`minimal-mod.md`](minimal-mod.md) — a complete mod you can load today.
2. [`mod-anatomy.md`](mod-anatomy.md) — directory, scripts, lifecycle, isolation.
3. [`commands.md`](commands.md) — commands, resources, dialogs, view/tool requests.
4. [`display-and-interaction.md`](display-and-interaction.md) — meshes and edits.
5. [`providers.md`](providers.md) — one backend per feature, and who owns the switch.
6. [`settings-and-parameters.md`](settings-and-parameters.md) — `ParamGet` and `FreeCAD.Grid`.
7. [`testing-a-mod.md`](testing-a-mod.md) — headless tests, no GPU.

## The JSON boundary (for automated clients)

`ferrocad_shell` is the host bridge. Every function below returns a **JSON
string**; all of them run with `FreeCAD.GuiUp == 0`.

| Function | Returns |
| --- | --- |
| `bootstrap(doc_name="Shell")` | the sample document, version and backend |
| `model_tree()` | documents and their objects |
| `properties(object_name)` | property rows for the editor |
| `set_property(object_name, prop, text)` | `{ok, error}`; one transaction |
| `evaluate(code)` | `{ok, output}` from one console line |
| `stage_meshes()` | the display meshes to draw |
| `interaction(object_name, manipulator, action, element, values_json)` | `{ok, ...}` from the interaction provider |
| `commands()` | command ids, resources, `enabled`, `checked` |
| `run_command(name)` | `{ok, status?, value?, view?, tool?, dialog?, token?}` |
| `dialog_accept(payload_json)` | the callback's result; payload is `{"token": ..., "values": {...}}` |
| `dialog_cancel(token)` | `{ok: true}` |
| `set_selection(pairs_json)` | `{ok: true}`; payload is `[{"object": name, "elements": [...]}, ...]` |
| `set_view_state(state_json)` | `{ok: true}`; mirrors the shell's view state |

The generated signatures are in
[`../python-api/ferrocad_shell.md`](../python-api/ferrocad_shell.md).

```python
import json
import ferrocad_shell

for command in json.loads(ferrocad_shell.commands()):
    print(command["id"], command["enabled"])
```

`FreeCADGui` and `FreeCAD` are ordinary Python modules. An automated client that
can call Python can use them directly; a client that can only exchange JSON
should go through the functions above.

## Background

- [`../user/explanation/how-the-shell-talks-to-the-document.md`](../user/explanation/how-the-shell-talks-to-the-document.md)
  — why the shell is a client of the facade, and why dialogs are data.
- [`../user/how-to/toy-workbench.md`](../user/how-to/toy-workbench.md) — a
  complete small mod exercised end to end.
- [`../user/how-to/selection-and-commands.md`](../user/how-to/selection-and-commands.md)
  — selection, command enablement, and the dialog round trip.

## What is not implemented

These are stated here so a client does not code against them:

- `FreeCADGui.addWorkbench`, `FreeCADGui.activeDocument`, and `ViewProvider` are
  absent. A mod is the script contract above, not an upstream workbench object.
- `FreeCADGui.Selection` is read-only. The shell pushes the snapshot; a mod cannot
  add or remove selection entries.
- `package.xml` is not read by the FerroCAD loader.
- Parameters are in memory (`ParamGet` is a shim); there is no cross-session
  persistence yet.
- Overlays need a `pivy.coin` build that exposes `addOverlay`/`removeOverlay`.
  Drawing them needs the viewport and a GPU.
