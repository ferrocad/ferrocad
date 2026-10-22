# API namespaces: FreeCAD-compatible vs FerroCAD-only

Status: decision (2026-10-22). Companion to
[`python-bindings.md`](python-bindings.md), [`../mods/README.md`](../mods/README.md) and
[`../workbenches/mod-providers.md`](../workbenches/mod-providers.md).

## Decision

Two Python namespaces, with a hard boundary:

| Namespace | Contains | Compatibility |
| --- | --- | --- |
| `FreeCAD`, `FreeCADGui` | the **upstream FreeCAD** surface, name for name | a mod that imports only these is FreeCAD-compatible |
| `FerroCAD`, `FerroCADGui` | **everything FerroCAD adds** | a mod that imports these is FerroCAD-only |

There are no aliases in the upstream namespaces. The split is what makes the claim
checkable: the conformance harness measures `FreeCAD`/`FreeCADGui` against upstream,
and `FerroCAD*` is excluded from parity by construction rather than by a list of names
to ignore.

## The rule

> If upstream FreeCAD has the name **and** the semantics, it stays in `FreeCAD` /
> `FreeCADGui`. Otherwise it goes in `FerroCAD` / `FerroCADGui`, even when the
> implementation is shared.

Values that are not extension points (`FreeCAD.backend`, `__version__`, `GuiUp`) stay
where they are; the rule is about the API a mod calls to extend the host.

## What moved (2026-10-22)

### `FreeCAD` → `FerroCAD`

| Name | What it is |
| --- | --- |
| `Grid` | the semantic grid-settings singleton |
| `register_display_provider` | a module's display-mesh hook |
| `register_interaction_provider` | a module's manipulator-edit hook |
| `display_mesh` | ask the registered provider for an object's mesh |
| `interaction` | ask the registered provider to apply an edit |
| `set_parameter_file`, `parameter_file` | which file backs `FreeCAD.ParamGet` |

### `FreeCADGui` → `FerroCADGui`

| Name | What it is |
| --- | --- |
| `registerProvider`, `unregisterProvider` | the feature-provider registry |
| `providers`, `activeProvider`, `setActiveProvider` | read/select a feature backend |
| `refreshProviders` | tell the active backend to re-read shared state |
| `settings`, `setSetting` | a provider's advertised settings |
| `addOverlay`, `removeOverlay` | the pull overlay bridge (Coin) |
| `ViewState` | the shell's presentation snapshot (`grid`, `projection`, …) |
| `PROVIDER_PREFERENCES` | the parameter path the registry reads |

### Resource keys

`GetResources()` keys that are FerroCAD extensions carry the prefix too:

| Before | After |
| --- | --- |
| `Surface` | `FerroCAD_Surface` |
| `Parent` | `FerroCAD_Parent` |

The shell's JSON (`commands()` → `surface`, `parent`) is unchanged: it is the host
bridge, already namespaced as `ferrocad_shell`.

## What stays upstream-shaped

`FreeCAD.newDocument`, `openDocument`, `closeDocument`, `getDocument`,
`listDocuments`, `activeDocument`, `ParamGet`, `ParameterGrp`, `Quantity`,
`Placement`, `Vector`, …; `FreeCADGui.addCommand`, `removeCommand`, `listCommands`,
`runCommand`, `Selection`, and the dialog token helpers used by the host bridge.

The FerroCAD **command return extensions** (`{"view": …}`, `{"tool": …}`, a dialog
spec plus token) are data, not names. Upstream ignores them; a FreeCAD-compatible mod
that returns `None` keeps working.

## Why not aliases

An alias would put the FerroCAD name back on the upstream namespace, which is exactly
what the split is for. FerroCAD is pre-1.0 and has no external mods, so the move is
unconditional. If a compatibility alias is ever needed, it must be opt-in (a
`ferrocad.compat` module), documented as deprecated, and kept out of `FreeCAD*`.

## Enforcement

`tests/test_namespace_separation.py` asserts the boundary in both directions:

- no FerroCAD extension name appears in `dir(FreeCAD)` or `dir(FreeCADGui)`;
- every FerroCAD-only name resolves on `FerroCAD` or `FerroCADGui`;
- every name in `FerroCAD.__all__`/`FerroCADGui.__all__` is absent from
  `dir(FreeCAD)`/`dir(FreeCADGui)` (and from a lazy `hasattr`) and resolves
  (this walks the declared surface, so a name added to a FerroCAD `__all__` is
  covered without editing the test);
- the upstream-shaped names still resolve, so the move did not take anything with it;
- no registered command's `GetResources()` declares a bare `Surface`/`Parent`
  key (checked at runtime over `FreeCADGui._COMMANDS`, with a raising
  `GetResources` treated as `{}` like the facade does);
- no `mods/*/InitGui.py` declares a bare `Surface`/`Parent` key (a source scan,
  because the suite does not import the mods).

## Audit

`test_namespace_separation.py` is the audit. Run it on its own with:

```sh
PYTHONPATH=python python3 -m pytest -q tests/test_namespace_separation.py
```

A mod importing only `FreeCAD`/`FreeCADGui` is **FreeCAD-compatible**. A mod
importing `FerroCAD` or `FerroCADGui` is **FerroCAD-only**: it depends on
something upstream FreeCAD does not have. The import line is the whole
statement, so a port of an upstream workbench should touch only the first pair
and a FerroCAD-only mod should say so in its README/`package.xml`.

The resource-key check makes the same split visible in `GetResources`: the keys
`FerroCAD_Surface`/`FerroCAD_Parent` are FerroCAD extensions, and the bare
`Surface`/`Parent` spellings are not accepted.

## How a mod states its compatibility

```python
import FreeCAD, FreeCADGui          # FreeCAD-compatible surface only

import FerroCAD, FerroCADGui        # this mod requires FerroCAD
```

A port of an upstream workbench should need only the first pair. If it needs the
second, that is a deliberate dependency and should be in its `package.xml`/README.
