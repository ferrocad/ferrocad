# Mod anatomy

Status: guide (2026-10-22).

A FerroCAD mod is a directory of Python scripts loaded at boot. This page is the
lifecycle and the failure model. The command, display, interaction and provider
contracts have their own pages.

## Directory

```text
mods/
  MyMod/
    Init.py          # non-GUI hook
    InitGui.py       # GUI hook
    Resources/
      icon.svg
    package.xml      # optional upstream metadata; see below
```

Both scripts are optional. A directory with neither contributes nothing. The
current authored mods (`mods/GridOverlay`, `mods/GridOverlayCoin`) use a one-line
`Init.py` and put everything in `InitGui.py`.

## `Init.py` and `InitGui.py`

| Script | Conventional role | In FerroCAD today |
| --- | --- | --- |
| `Init.py` | non-GUI registration: document types, display/interaction providers | executed at boot |
| `InitGui.py` | commands, feature providers, overlays | executed at boot, including when `FreeCAD.GuiUp == 0` |

FerroCAD does not skip `InitGui.py` in a headless process. The loader runs both
scripts unconditionally; the headless facade still has the command registry and
the provider registry.

Each script is executed, not imported as a package:

```python
namespace = {"__file__": path, "__name__": f"{name}.{script[:-3]}"}
exec(compile(source, path, "exec"), namespace)
```

Consequences:

- `__file__` is the script path and `__name__` is `"MyMod.InitGui"`.
- The two scripts get **separate namespaces**; a global in `Init.py` is not
  visible in `InitGui.py`.
- The script is not added to `sys.modules`. `import MyMod.InitGui` is not the
  loader path.
- Relative imports do not work. If a mod needs a sibling module, add its
  directory to `sys.path` before importing it.
- The loader catches exceptions per script. A failing `Init.py` does not prevent
  `InitGui.py` from running.

## `package.xml`

`package.xml` is FreeCAD package metadata. **FerroCAD's loader does not read it**
and no code path in this tree references it. Ship one only if the same directory
is also published to upstream FreeCAD; it has no effect on loading, commands or
the UI here. The authored mods in `mods/` do not include one.

For reference, the upstream format starts like this:

```xml
<?xml version="1.0" encoding="utf-8"?>
<package format="1" xmlns="https://wiki.freecad.org/Package_Metadata">
  <name>Minimal Box</name>
  <description>A minimal FerroCAD mod.</description>
  <version>0.1.0</version>
  <date>2026-10-22</date>
  <maintainer email="you@example.com">Your Name</maintainer>
  <license file="LICENSE">MIT</license>
</package>
```

Treat that as upstream-compatible metadata, not as something this build
validates.

## Registration at import time

A mod registers as a side effect of running its script. There is no
`register()` callback, no workbench activation, and no lazy load.

```python
# InitGui.py
import FreeCAD
import FreeCADGui

FreeCADGui.addCommand("MyMod_DoThing", DoThingCommand())
FreeCAD.register_display_provider(display_my_object)
FreeCADGui.registerProvider("my_feature", "my_backend", "My Backend")
```

Rules that follow from the registries:

- `addCommand` rejects a duplicate id with `ValueError`. A mod that may be
  executed twice in one process should call `FreeCADGui.removeCommand(id)` first;
  the minimal mod and `mods/GridOverlayCoin` do this.
- `registerProvider` replaces an existing `(feature, backend)` record, so
  re-registration is reload-safe.
- `register_display_provider` and `register_interaction_provider` are idempotent
  only for the **same function object**. Re-executing a script creates new
  function objects and appends new providers. There is no unregister.
- Registration should be cheap. Do not draw, open a document, or start a
  transaction at import time.
- A script that raises **after** registering something leaves the earlier
  registrations in place. Registration is immediate, not transactional.

`Init.py` and `InitGui.py` are not given a teardown call. If you attach a
document observer or open another resource, keep it in a module global and shut
it down yourself if the module is reloaded (the Coin mod's `GridManager` does
this).

## Lifecycle

1. The host configures Python, including `python/` on `sys.path`.
2. The host chooses the mods directories: `HostConfig.mods_paths`, else
   `FERROCAD_MODS_PATH`, else a discovered `mods/` beside `python/`.
3. `ferrocad_shell.load_workbenches(dirs_json)` scans each directory's immediate
   subdirectories in sorted order.
4. For each subdirectory it executes `Init.py` then `InitGui.py`, if present.
5. The app boots. Commands are listed on demand, display meshes are requested on
   a document sync, interactions on a manipulator commit, a provider switch on
   `Mods_Settings`, and a provider refresh after a feature-level state change
   such as the grid toggle.
6. There is no file watcher or hot reload. An edited script takes effect on the
   next process start; a test re-executes it deliberately.

The JSON result of a load is:

```json
{
  "loaded": ["MyMod/Init.py", "MyMod/InitGui.py"],
  "errors": [
    {"workbench": "OtherMod", "script": "InitGui.py", "traceback": "..."}
  ]
}
```

The shell prints `workbenches: loaded N script(s)` and one console line per
error; the traceback goes to stderr.

## The isolation rule

**One broken mod must not take down the application.** The rule is enforced at
different strengths:

| Failure | Behaviour |
| --- | --- |
| A script raises at load | caught by the loader; the script is recorded under `errors`; other scripts still run; the app boots |
| `GetResources()` raises | treated as `{}`; reported once; the command keeps its id |
| `IsActive()` raises | that command is disabled; reported once; other commands are unaffected |
| `IsChecked()` raises | treated as `False`; reported once |
| `Activated()` raises through the shell bridge | `{"ok": false, "error": "..."}`; the app keeps running |
| A provider `activate`/`deactivate`/`refresh` raises | caught, reported once per `(feature, backend, hook)`, listed under `errors`; the switch or refresh continues |
| A display provider raises | **not caught** by `FreeCAD.display_mesh` or `stage_meshes` |
| An interaction provider raises | **not caught** by `FreeCAD.interaction` or `ferrocad_shell.interaction` |
| `addOverlay` fails | **not caught**; it raises if the Coin facade is missing |

So the rule is: the loader and the registries isolate **their** calls, but your
display and interaction providers are called directly. The convention is to
return `None` for work you do not own, and to wrap your own conversion in
`try/except` and return `None` (or `{"ok": False}`) rather than raise. The toy's
`_display_mesh` does exactly that.

`IsActive` and `Activated` must also tolerate `FreeCAD.ActiveDocument is None`.
A command that needs a document should return `False` from `IsActive` or a
`{"status": ...}` from `Activated`.

## Next

- [`commands.md`](commands.md) — what a command may return.
- [`display-and-interaction.md`](display-and-interaction.md) — the two hook
  signatures and the mesh fields.
- [`providers.md`](providers.md) — feature providers and the switch.
- [`testing-a-mod.md`](testing-a-mod.md) — loading a script by path and driving
  it headlessly.
