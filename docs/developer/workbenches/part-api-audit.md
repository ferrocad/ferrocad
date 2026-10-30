# Part/OCCT API audit

`tools/part_api_audit.py` is the repeatable answer to "what is still missing from
Part/OCCT?". It counts the Python surface of `crates/ferrocad_part_py` and the
object types registered by both `crates/ferrocad_part` and
`crates/ferrocad_part_py`, compares them with a sibling `freecad-upstream`
checkout, then pins both in [`contracts/part-api.json`](../../../contracts/part-api.json).
At the 2026-10-27 pin our surface is 38 functions / 60 classes / 39 object types against
upstream's 50 / 96 / 80; the headline is the method coverage, not those totals.
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
| ours | **methods** | per-Python-class member names from `#[pyclass]` + `#[pymethods]`; `#[getter]`/`#[setter]` fold to the property name, and `extends = PartShape` inherits the base's members |
| upstream | module functions | `add_varargs_method` / `add_keyword_method` / `add_noargs_method` in `AppPartPy.cpp`, split by `Py::ExtensionModule` class |
| upstream | classes | `Base::Interpreter().addType(..., "Name")` in `AppPart.cpp`, which gives the real Python name and owning submodule |
| upstream | **methods** | per-class member names from the `*.pyi` stubs: `@export(PythonName="Part.X", …)` names the class, then its class attributes and `def` items (docstrings stripped; dunders excluded) |
| upstream | stub classes | file stems of `*.pyi` and `*PyImp.cpp` (a text pass, recorded but not used for the diff) |
| upstream | object types | `PROPERTY_SOURCE*(Part::X, ...)` across `Part/App` (recursive); a registered-type inventory, not an `addObject` audit |
| upstream | type-system types | `TYPESYSTEM_SOURCE*(Part::X, ...)`, recorded separately |
| upstream | `PART_*_TYPE` macros | counted; this checkout has none |

Everything is sorted and revision-stamped, never time-stamped, so two runs on
the same trees produce byte-identical JSON.

## What is *not* measured

- **Signatures, defaults, return types, docstrings, exceptions.** A name can
  match while the call contract differs. (The SimpleCAD runtime errors are the
  practical detector for these.)
- **Runtime registration.** `#[pyclass]` is a definition; the actual attribute
  may be conditional (for example `Cone`/`Toroid`/`BSplineSurface` are only
  added when the analytic half did not define them).
- **Upstream inheritance.** A `*.pyi` lists only the members that stub declares;
  members inherited from an upstream base class are counted under that base. Our
  side folds `extends = PartShape`, but a base class we do not expose
  (`Part::GeometryCurve`, `Part::Conic`, …) still contributes its names to the
  global denominator and appears in `missing.method_classes`.
- **Class-level dunders.** `__init__`/`__repr__`/`__eq__` are left out because
  pyo3 generates several and the stubs do not list them uniformly.
- **The Python shim** (`Init.py`, `PartEnums.py` and similar). Module functions
  are read from `AppPartPy.cpp` only, so names a Python shim adds are invisible.
- **`.pyi` names as Python names.** Stub files are named after the C++ class, so
  `TopoShape.pyi` is `Part.Shape` and `PartFeature.pyi` is `Part.Feature`; the
  method inventory uses the `@export(PythonName=…)` value instead.
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
  `ours.methods` and `upstream.methods` are per-class member lists; `count` sums
  them and `distinct` is the size of the name set.
- `missing` — upstream names absent from our surface, per function, class,
  document-object type and method. `missing.methods` is keyed by the classes we
  share (for example `Part::Shape`); `missing.method_classes` lists upstream
  classes we do not expose at all.
- `coverage.methods` — the headline number: how many of upstream's distinct
  declared names exist on at least one of our Python classes, the fraction, and
  the shared-class count. At the 2026-10-30 pin it is **252 of 408 names
  (61.8%) across 21 shared classes**; the per-class gaps are the SimpleCAD/Draft work
  list. Earlier pins were 248/408 (60.8%) and 220/408 (53.9%); always say which tree and
  which extension build produced the figure.
- `extra` — our names that upstream does not have in `Part/App` (for example
  `App::Link*`, `Sketcher::SketchObject`, `PartDesign::Body`, `Part.Precision`).
- `ours.gap_markers` — every honest gap, with `file:line`, kind and source text.
  Line numbers are recorded but not compared: unrelated edits shift them.
- `upstream.revision` — recorded but not compared; the surface is the check.

### Methods are compared; the heuristic count is not

`ours.methods` is the real inventory and participates in `--check`.
`ours.methods_heuristic` (every `fn` in a `#[pymethods]` block, private helpers
included) stays for scale and is deliberately dropped before the diff, because it
moves with unrelated edits.

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
