# Part/OCCT API audit (H1)

`tools/part_api_audit.py` is the repeatable answer to "what is still missing from
Part/OCCT?". It counts the Python surface of `crates/ferrocad_part_py` and the
object types registered by both `crates/ferrocad_part` and
`crates/ferrocad_part_py`, compares them with a sibling `freecad-upstream`
checkout, then pins both in [`contracts/part-api.json`](../../contracts/part-api.json).
The contract is the source of truth: `--check` re-derives it and fails on any
surface change, so the roadmap does not have to rely on prose.

```sh
python3 tools/part_api_audit.py            # report the missing names
python3 tools/part_api_audit.py --check    # CI: exit 1 on drift
python3 tools/part_api_audit.py --write    # re-pin after an intended change
python3 tools/part_api_audit.py --json     # machine-readable
```

The checkout is `--root`, then `$FERROCAD_UPSTREAM`, then
`../freecad-upstream` beside the repo. `--root` may name the checkout or its
`src/Mod/Part/App` directory. Without a checkout `--check` prints a skip message
and exits **0**; with a checkout but no committed contract it exits **2**,
because that is a setup error rather than a measurement.

## What is measured

| Side | Surface | How it is read |
| --- | --- | --- |
| ours | module functions | `add_function(wrap_pyfunction!(...))`, qualified by the receiving module (`Part`, `Part.Precision`) |
| ours | function definitions | `#[pyfunction]`; `defined_not_registered` names helpers such as `sketch_add_geometry` |
| ours | classes | `#[pyclass(name = "...", module = "...")]`, qualified as `Part::Edge`, `Part.ShapeFix::Edge` |
| ours | object types | `*_TYPE` constants and literal ids passed to `object_registry::register` / a local `register` helper, in **both** `crates/ferrocad_part/src` and `crates/ferrocad_part_py/src`; loop tables are expanded when the loop variable is registered |
| ours | gap markers | literal `not implemented yet`, `not available in this slice`, `unsupported(` and `NotImplemented` |
| upstream | module functions | `add_varargs_method` / `add_keyword_method` / `add_noargs_method` in `AppPartPy.cpp`, split by `Py::ExtensionModule` class |
| upstream | classes | `Base::Interpreter().addType(..., "Name")` in `AppPart.cpp`, which gives the real Python name and owning submodule |
| upstream | stub classes | file stems of `*.pyi` and `*PyImp.cpp` (a text pass, recorded but not used for the diff) |
| upstream | object types | `PROPERTY_SOURCE*(Part::X, ...)` across `Part/App` (recursive); a registered-type inventory, not an `addObject` audit |
| upstream | type-system types | `TYPESYSTEM_SOURCE*(Part::X, ...)`, recorded separately |
| upstream | `PART_*_TYPE` macros | counted; this checkout has none |

Everything is sorted and revision-stamped, never time-stamped, so two runs on
the same trees produce byte-identical JSON.

## What is *not* measured

- **Methods.** `[pymethods]` blocks are only counted heuristically
  (`ours.methods_heuristic`: every `fn` item in the block, including private
  helpers). The count is recorded for scale but excluded from `--check`, and
  upstream methods are not measured at all. `Part.Shape` therefore looks much
  smaller here than it is at runtime.
- **Signatures, defaults, return types, docstrings, exceptions.** A name can
  match while the call contract differs.
- **Runtime registration.** `#[pyclass]` is a definition; the actual attribute
  may be conditional (for example `Cone`/`Toroid`/`BSplineSurface` are only
  added when the analytic half did not define them).
- **Inheritance, properties, enums, `__getattr__` shims.**
- **The Python shim** (`Init.py`, `PartEnums.py` and similar). Module functions
  are read from `AppPartPy.cpp` only, so names a Python shim adds are invisible.
- **`.pyi` names as Python names.** Stub files are named after the C++ class, so
  `TopoShape.pyi` is `Part.Shape` and `PartFeature.pyi` is `Part.Feature`; the
  class diff uses the `addType` names instead and keeps the stub list separate.
- **Property types and FaceMaker selectors.** `Part::PropertyPartShape`,
  `Part::PropertyTopoShapeList` and `Part::FaceMaker*` are not object types and
  are excluded from `ours.object_types`; only what reaches
  `object_registry::register` counts (property types go through
  `property_types::register_extension`).
- **Upstream object types are a registered-type list, not an `addObject` list.**
  The upstream extractor keeps `PROPERTY_SOURCE*` as-is. That includes
  `App::Extension` classes (`Part::AttachExtension*`, `Part::*PatternExtension`,
  `Part::PreviewExtension*`, `Part::PrismExtension`), abstract/base document
  classes (`Part::BodyBase`, `Part::Boolean`, `Part::Primitive`, `Part::Datum`,
  `Part::FeatureExt`, `Part::FilletBase`, `Part::LinkArray`) and Python-feature
  variants (`Part::FeaturePython`, `Part::Part2DObjectPython`). Treat
  `missing.object_types` as an upper bound: a name may be a class that is not
  directly addObject-able rather than a missing feature.
- **Known model differences.** `Part.Precision` is a submodule of functions here
  and a class upstream, so it appears in both `extra.functions` and
  `missing.classes`. The `Part.ShapeFix` classes here do not replace upstream's
  `Part.ShapeFix.sameParameter`-style module functions, so those stay in
  `missing.functions` until module wrappers exist. `TYPESYSTEM_SOURCE` names are
  mostly geometry/property types, not document objects, which is why they do not
  feed `missing.object_types`.

## Reading the contract

- `ours` / `upstream` — the counted names and the counts derived from them.
- `missing` — upstream names absent from our surface, per function, class and
  document-object type. For object types this is the upper bound of the H1 work
  list (see the extension/base-class caveat above).
- `extra` — our names that upstream does not have in `Part/App` (for example
  `App::Link*`, `Sketcher::SketchObject`, `PartDesign::Body`, `Part.Precision`).
- `ours.gap_markers` — every honest gap, with `file:line`, kind and source text.
  Line numbers are recorded but not compared: unrelated edits shift them.
- `upstream.revision` — recorded but not compared; the surface is the check.

## Refreshing

When the binding surface changes intentionally:

```sh
python3 tools/part_api_audit.py --write
python3 tools/part_api_audit.py --check
```

Commit the code and `contracts/part-api.json` together. When the upstream
checkout moves, re-run `--write` on the new revision and mention it in the
commit; a revision change alone does not fail `--check`, but a name change does.

CI runs `--check` in the `geometry` job only when a sibling
`../freecad-upstream` exists, next to the SimpleCAD audit. Normal PR CI does not
clone upstream, so the guard makes the step a no-op there.
