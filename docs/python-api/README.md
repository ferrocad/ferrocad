# Python API reference

Generated from the live modules by `tools/api_docs.py`. Each page is a signature
and summary reference for what FerroCAD ships, not a tutorial: the workflows in
[`../user/`](../user/) show the same surface being used.

The product surface is the `FreeCAD`/`FreeCADGui` pair, which deliberately mirrors
upstream, so the authoritative description of *behaviour* remains FreeCAD's own
documentation. `FerroCAD`/`FerroCADGui` hold the FerroCAD-only extensions and are
documented here alone. What this section adds is the exact shape of the surface
here.

| Module | Public names | What it is |
| --- | --- | --- |
| [`FreeCAD`](FreeCAD.md) | 15 | Upstream-compatible surface: documents, objects, the base value types. |
| [`FreeCAD.Base`](FreeCAD.Base.md) | 12 | The `Base` value types FreeCAD exposes as a submodule. |
| [`FreeCAD.Units`](FreeCAD.Units.md) | 9 | Quantity parsing, units and number formatting. |
| [`FreeCAD.Console`](FreeCAD.Console.md) | 5 | The console sink a workbench prints through. |
| [`FreeCADGui`](FreeCADGui.md) | 11 | Upstream-compatible GUI surface: commands, selection and dialogs. |
| [`FerroCAD`](FerroCAD.md) | 11 | FerroCAD-only surface: the semantic grid and the module hooks. |
| [`FerroCADGui`](FerroCADGui.md) | 15 | FerroCAD-only GUI surface: providers, settings, overlays and view state. |
| [`ferrocad_shell`](ferrocad_shell.md) | 24 | The host bridge the shell calls. Not upstream API: it is how FerroCAD's window (and `tools/userdocs.py`) drive the facade. |

## Scope and honesty

- **Generated, not curated.** A page lists what the module actually exports when
  the generator runs. A missing name means missing here, not undocumented.
- **Signatures where Python exposes them.** Names implemented by the PyO3
  extension may not report a signature; those show as ``(...)``.
- **Not a parity claim.** Upstream has far more surface. The measured gap lives in
  [`../workbenches/conformance.md`](../workbenches/conformance.md).

## Regenerating

```sh
python3 tools/api_docs.py            # refresh every page
python3 tools/api_docs.py --check    # non-zero exit if a page is stale
```
